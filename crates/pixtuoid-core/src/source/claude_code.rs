use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{cwd_basename_label, make_tool_detail};
use crate::source::hook::HookSocketListener;
use crate::source::jsonl::{JsonlWatcher, ProbeSnapshot};
use crate::source::{AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "claude-code";

/// CC's session/agent id = the transcript filename stem, which is
/// cwd-independent (the cwd-derived project-dir is the *parent* dir, not the
/// stem): `<uuid>.jsonl` → `<uuid>` for a root, `agent-<id>.jsonl` →
/// `agent-<id>` for a subagent. Mirrors `codex_id_from_path`. CC session UUIDs
/// and agent-ids are lowercase, so the Windows path fold is inert here.
pub fn cc_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

pub struct ClaudeCodeSource {
    pub socket_path: PathBuf,
    pub projects_root: PathBuf,
}

/// Resolve `CLAUDE_CONFIG_DIR` (an empty value is treated as unset). `pub` +
/// `#[doc(hidden)]` so the `pixtuoid` install crate's settings.json resolver
/// shares this one definition — the two CC path sites must not drift. Internal
/// cross-crate helper, not a stable API.
#[doc(hidden)]
pub fn claude_config_dir() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// CC's first-party live-process registry: `<claude_home>/sessions/<pid>.json`,
/// one tiny JSON file per running CC process (`{pid, sessionId, cwd, status,
/// startedAt, procStart, ...}` — undocumented, drift-watched by
/// check_upstream_drift.py). Returns the session UUIDs (+ owning pid each) of
/// entries whose pid is still ALIVE — a registry file can outlive a crashed
/// CC, so each entry is verified with kill(pid, 0).
///
/// Failure is explicit (#223): an UNREADABLE-or-MISSING registry dir is
/// `None` — on a machine where CC runs the dir's absence is ambiguous (older
/// CC without the registry, a permissions problem), so the probe declares
/// failure and the watcher changes nothing. An EMPTY but readable dir is
/// `Some(empty)`: a healthy "no CC running" observation that the negative
/// vouch may act on.
///
/// PID-reuse guard (#220): kill(0) only proves SOME process owns the pid. When
/// the entry carries `startedAt` (ms epoch, stamped by CC at startup) AND the
/// kernel can report the process start time ([`pid_start_time_secs`] — macOS
/// only today), the two must agree within [`PID_START_TOLERANCE_SECS`] or the
/// pid was recycled by an unrelated process and the entry is skipped. Either
/// side missing falls back to pid-alive-only (the previous behavior — the
/// check is additive). This matters more now that the probe is ONGOING
/// liveness (a recycled pid would hold a dead session's sweep exemption open,
/// not just admit one transient sprite).
pub fn live_cc_session_ids(sessions_dir: &Path) -> Option<ProbeSnapshot> {
    #[cfg(unix)]
    {
        let mut live = ProbeSnapshot::default();
        let Ok(entries) = std::fs::read_dir(sessions_dir) else {
            tracing::debug!(
                "CC session registry {} unreadable or missing; probe pass failed",
                sessions_dir.display()
            );
            return None;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Regular files only, decided WITHOUT following symlinks (so a
            // symlink-to-FIFO is rejected too): reading a writer-less FIFO
            // named `x.json` blocks forever, hanging the probe and silently
            // killing the whole CC watcher task it runs inside.
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            // An unreadable file is usually a CC exiting and removing its own
            // entry mid-scan — not format drift; skip silently.
            let Ok(bytes) = read_registry_entry_bounded(&path) else {
                continue;
            };
            let Some(reg) = parse_registry_entry(&bytes) else {
                // Undocumented format — warn ONCE per process, never per scan
                // (the probe runs every scan pass).
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    tracing::warn!(
                        "unparseable CC session-registry file (format drift?): {}",
                        path.display()
                    );
                });
                continue;
            };
            if !pid_alive(reg.pid) {
                continue;
            }
            if let (Some(claimed_ms), Some(actual_secs)) =
                (reg.started_at_ms, pid_start_time_secs(reg.pid))
            {
                if (claimed_ms / 1000).abs_diff(actual_secs) > PID_START_TOLERANCE_SECS {
                    tracing::debug!(
                        pid = reg.pid,
                        claimed_secs = claimed_ms / 1000,
                        actual_secs,
                        "pid recycled — registry startedAt does not match process start; skipping"
                    );
                    continue;
                }
            }
            live.pid_of.insert(reg.session_id.clone(), reg.pid);
            live.ids.insert(reg.session_id);
        }
        Some(live)
    }
    #[cfg(not(unix))]
    {
        // Windows has no kill(0); pid liveness there needs OpenProcess +
        // exit-code semantics we haven't validated against CC-on-Windows.
        // The probe CANNOT enumerate → None (explicit failure): the watcher
        // changes nothing — admission keeps today's pure-mtime behavior and
        // the negative vouch never arms.
        let _ = sessions_dir;
        None
    }
}

/// Read one registry entry, bounded to 64 KiB. Real entries are <1 KiB; the
/// bound keeps junk dropped into the registry dir from ballooning a read that
/// runs on every scan pass (truncated bytes just fail the JSON parse and hit
/// the warn-once skip).
#[cfg(unix)]
fn read_registry_entry_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    const MAX_REGISTRY_ENTRY_BYTES: u64 = 64 * 1024;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_ENTRY_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// One parsed registry entry — the fields the liveness join needs.
#[cfg(unix)]
struct RegistryEntry {
    pid: i32,
    session_id: String,
    /// `startedAt` — ms-epoch CC stamps at startup (~1.2s after process start,
    /// measured live). Optional: an older CC without the field still probes
    /// pid-alive-only.
    started_at_ms: Option<u64>,
}

/// Extract `{pid, sessionId, startedAt}` from one registry file.
/// `serde_json::Value` on purpose — the format is undocumented, so we read
/// only the fields the join needs and tolerate everything else changing.
/// `startedAt` is optional (missing/malformed → `None`, never a parse fail):
/// it only powers the additive PID-reuse identity check.
#[cfg(unix)]
fn parse_registry_entry(bytes: &[u8]) -> Option<RegistryEntry> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    // pid <= 0 is never a single process (kill(0)/kill(-n) target process
    // GROUPS — a corrupt entry must not probe our own group as "alive").
    let pid = i32::try_from(v.get("pid")?.as_i64()?)
        .ok()
        .filter(|p| *p > 0)?;
    let session_id = v.get("sessionId")?.as_str().filter(|s| !s.is_empty())?;
    let started_at_ms = v.get("startedAt").and_then(|s| s.as_u64());
    Some(RegistryEntry {
        pid,
        session_id: session_id.to_string(),
        started_at_ms,
    })
}

/// Tolerance for the `startedAt` ↔ kernel-start-time identity check. Measured
/// live: CC writes `startedAt` ≈ 1.2s after `pbi_start_tvsec` (Node boot +
/// module load before the stamp), so 10s is ~8× margin while still being far
/// below any plausible pid-recycling interval.
#[cfg(unix)]
const PID_START_TOLERANCE_SECS: u64 = 10;

/// Kernel-reported process start time in epoch seconds — the identity half of
/// the PID-reuse guard. macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` →
/// `pbi_start_tvsec` (same libproc family as `fd_probe.rs`). Linux:
/// `/proc/<pid>/stat` field 22 is clock ticks since BOOT — epoch conversion
/// needs boot time + ticks-per-sec, so the identity check is macOS-only for
/// now and Linux returns `None` (pid-alive-only, today's behavior). `None` on
/// failure (pid gone mid-probe, EPERM) — never an error: the check is additive.
#[cfg(target_os = "macos")]
fn pid_start_time_secs(pid: i32) -> Option<u64> {
    // SAFETY: all-zero bytes are a valid value for this repr(C) plain-old-data
    // struct (integers + byte arrays only).
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly `size` bytes of a repr(C) struct matching
    // the macOS SDK's proc_bsdinfo layout (proc_info.h, ABI-stable since
    // 10.5), so the kernel fills only memory we own. PROC_PIDTBSDINFO returns
    // the full struct or <= 0 on failure.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut std::ffi::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    Some(info.pbi_start_tvsec)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_start_time_secs(_pid: i32) -> Option<u64> {
    None
}

/// kill(pid, 0) liveness: rc 0 = alive and signalable; EPERM = alive but owned
/// by another user; ESRCH (or anything else) = no such process.
#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 performs only the existence/permission check —
    // no signal is delivered, no memory is touched, no pointer args.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The sessions registry is a SIBLING of the projects root
/// (`<claude_home>/sessions` vs `<claude_home>/projects`). Derive it only when
/// the parent layout matches (the root's file_name is literally `projects`) —
/// a custom `--projects-root /tmp/fixture` replay points at an arbitrary dir
/// whose parent could hold an unrelated `sessions/`, so those runs get no
/// probe and keep the pure-mtime gate.
fn cc_sessions_dir(projects_root: &Path) -> Option<PathBuf> {
    if projects_root.file_name().and_then(|n| n.to_str()) != Some("projects") {
        return None;
    }
    projects_root
        .parent()
        // A bare relative `projects` has an EMPTY parent — not a claude home.
        .filter(|home| !home.as_os_str().is_empty())
        .map(|home| home.join("sessions"))
}

impl ClaudeCodeSource {
    pub fn default_socket_path() -> PathBuf {
        if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
            return PathBuf::from(p);
        }
        #[cfg(unix)]
        {
            if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
                return PathBuf::from(format!("{dir}/pixtuoid.sock"));
            }
            // SAFETY: getuid() is a trivial syscall with no pointer args; cannot fail.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
        }
        #[cfg(windows)]
        {
            // Mirrors pixtuoid-hook/src/paths.rs — parity-pinned by
            // tests/socket_path_parity.rs, not shared (no dep edge between
            // shim and core).
            let user = std::env::var("USERNAME")
                .unwrap_or_else(|_| "default".into())
                .replace('\\', "-");
            PathBuf::from(format!(r"\\.\pipe\pixtuoid-{user}"))
        }
    }

    pub fn default_paths() -> Self {
        let projects_root = claude_config_dir()
            .unwrap_or_else(|| PathBuf::from(crate::platform::user_home()).join(".claude"))
            .join("projects");
        Self {
            socket_path: Self::default_socket_path(),
            projects_root,
        }
    }
}

impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        // SocketBusy (another live instance owns the endpoint) must not take
        // the whole source down: the JSONL watcher works fine concurrently,
        // so this instance degrades to transcript-only — hooks (and the
        // hook-only Reasonix source riding the same socket) stay with the
        // owning instance. Every other bind error is fatal as before.
        let socket = match HookSocketListener::bind(self.socket_path.clone()).await {
            Ok(s) => Some(s),
            Err(e) if e.downcast_ref::<super::hook::SocketBusy>().is_some() => {
                tracing::warn!(
                    "{e:#}; continuing transcript-only — hook-borne signals \
                     (permission Waiting, instant lifecycle) belong to the owning instance"
                );
                None
            }
            Err(e) => return Err(e),
        };
        let mut watcher = JsonlWatcher::new(
            self.projects_root.clone(),
            SOURCE_NAME.to_string(),
            decode_cc_line,
            cc_derive_label,
            cc_session_ended,
        )
        .with_id_deriver(cc_id_from_path);
        if let Some(sessions_dir) = cc_sessions_dir(&self.projects_root) {
            watcher = watcher.with_liveness_probe(std::sync::Arc::new(move || {
                live_cc_session_ids(&sessions_dir)
            }));
        }

        let Some(socket) = socket else {
            return watcher.run(tx).await;
        };
        let tx_hook = tx.clone();
        let tx_jsonl = tx.clone();
        let hook_task = tokio::spawn(async move { socket.run(tx_hook).await });
        let jsonl_task = tokio::spawn(async move { watcher.run(tx_jsonl).await });

        let hook_abort = hook_task.abort_handle();
        let jsonl_abort = jsonl_task.abort_handle();

        let inner: Result<()> = tokio::select! {
            r = hook_task => {
                tracing::warn!("hook listener exited first; aborting jsonl watcher");
                jsonl_abort.abort();
                r?
            }
            r = jsonl_task => {
                tracing::warn!("jsonl watcher exited first; aborting hook listener");
                hook_abort.abort();
                r?
            }
        };
        inner
    }
}

/// Decode one CC JSONL transcript line into 0..N AgentEvents.
pub fn decode_cc_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    // Key on the session UUID (filename stem), NOT the raw path — matches the
    // hook decoder's `IdKey::SessionId` and the watcher's `cc_id_from_path`
    // deriver, so all four CC keying sites coalesce (mirrors Codex).
    let agent_id = AgentId::from_parts(source, &cc_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    let mut out = Vec::new();
    let ty = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    // `.filter(non-empty)`: an empty `attributionAgent` would emit `Rename {
    // label: "" }`, blanking a good hook-derived label with no recovery until the
    // next Rename — same empty-string guard as the decoder's id fields.
    if let Some(name) = obj
        .get("attributionAgent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        let label = name.rsplit(':').next().unwrap_or(name).to_string();
        out.push(AgentEvent::Rename { agent_id, label });
    }

    let Some(message) = obj.get("message").and_then(|m| m.as_object()) else {
        return Ok(out);
    };
    let content = message.get("content");
    match (ty, content) {
        ("assistant", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_use" {
                    continue;
                }
                let id = bobj.get("id").and_then(|s| s.as_str()).map(String::from);
                let name = bobj.get("name").and_then(|s| s.as_str()).unwrap_or("?");
                out.push(AgentEvent::ActivityStart {
                    agent_id,
                    tool_use_id: id,
                    detail: Some(make_tool_detail(name, bobj.get("input"))),
                });
            }
        }
        ("user", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_result" {
                    continue;
                }
                let id = bobj
                    .get("tool_use_id")
                    .and_then(|s| s.as_str())
                    .map(String::from);
                out.push(AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id: id,
                });
            }
        }
        // No content arm: user-message content is user-controllable and must
        // never drive session lifecycle (a message QUOTING the slash-command
        // wrapper would false-positive), and modern CC persists no /exit
        // marker in the transcript anyway. Lifecycle = the SessionEnd hook +
        // the idle sweep.
        _ => {}
    }
    Ok(out)
}

/// CC session-end checker: parses lines as JSON and checks for
/// session lifecycle markers structurally (not byte scan).
pub fn cc_session_ended(tail: &[u8]) -> bool {
    let mut last_is_end = false;
    for line in tail.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
        let hook = v
            .get("hook_event_name")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if subtype == "session_start" {
            last_is_end = false;
        }
        if subtype == "session_end" || hook == "SessionEnd" {
            last_is_end = true;
        }
        // Only STRUCTURAL markers count. Message content is user-controllable
        // and must never drive lifecycle (quoting the slash-command wrapper
        // would false-positive), and modern CC persists no /exit marker at
        // all — a session whose end hook dropped is reaped by the idle sweep.
    }
    last_is_end
}

/// CC label: subagent paths → "subagent", otherwise "cc·" + cwd basename.
///
/// When `cwd` is unknown (a seed line that carries no `cwd` — the JSONL Rename
/// can fire on such a line), fall back to the CC **project dir** instead of a
/// bare "cc": the project dir name encodes the cwd path with '/'→'-', so its
/// last segment is the project basename. Without this, an empty-cwd Rename
/// silently degrades a good hook-derived `cc·dotfiles` back to `cc`.
pub fn cc_derive_label(path: &Path, _source: &str, cwd: &Path) -> String {
    // ONE shared predicate with `detect_parent_id` (both via the `SUBAGENTS_DIR` component)
    // so the two can't diverge — a loose `"subagents"` substring once mislabeled a
    // `subagents-paper` repo's parent transcript "subagent" with parent_id=None
    // (bug_004); the slash-bounded predicate fixes that at a single source.
    if crate::source::jsonl::is_subagent_path(path) {
        return "subagent".to_string();
    }
    if let Some(label) = cwd_basename_label("cc", cwd) {
        return label;
    }
    if let Some(base) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|proj| proj.rsplit('-').find(|s| !s.is_empty()))
    {
        return format!("cc·{base}");
    }
    "cc".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_prefers_cwd_basename_when_present() {
        let path = Path::new("/x/.claude/projects/-Users-me-repo/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/Users/me/work/myrepo")),
            "cc·myrepo"
        );
    }

    #[test]
    fn label_falls_back_to_project_dir_when_cwd_empty() {
        // Regression: an empty-cwd Rename must not degrade `cc·dotfiles` to `cc`.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_marks_subagent_paths() {
        let path = Path::new("/x/projects/proj/subagents/agent-1.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/repo")),
            "subagent"
        );
    }

    #[test]
    fn label_does_not_false_positive_on_subagents_in_project_name() {
        // A parent transcript for a repo named `subagents-paper` encodes to a
        // project dir containing the substring "subagents" but no `/subagents/`
        // segment — it must NOT be mislabeled "subagent".
        let path = Path::new("/Users/me/.claude/projects/-Users-me-subagents-paper/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/Users/me/subagents-paper")),
            "cc·subagents-paper"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_is_root() {
        // cwd = "/" fails the non-empty/non-root guard → falls to the project-dir
        // branch rather than the cwd basename.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("/")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_uses_project_dir_when_cwd_has_no_basename() {
        // A non-empty, non-root cwd whose file_name() is None (e.g. "..") enters
        // the cwd block but can't return → falls through to the project-dir branch.
        let path = Path::new("/Users/me/.claude/projects/-Users-me-dotfiles/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("..")),
            "cc·dotfiles"
        );
    }

    #[test]
    fn label_final_fallback_to_cc_when_no_project_dir() {
        // Degenerate path with no parent dir to decode AND empty cwd → bare "cc".
        assert_eq!(
            cc_derive_label(Path::new("abc.jsonl"), "claude-code", Path::new("")),
            "cc"
        );
    }

    // The socket-path and default-paths env precedence. All three socket
    // branches are checked in ONE test because the env vars are process-global —
    // splitting across tests would race under the default multi-thread runner.
    // Unix-specific branches (XDG_RUNTIME_DIR + getuid fallback) can only be
    // asserted on Unix; the platform-neutral default_paths check is split into
    // a separate test so it compiles + runs on all platforms.
    #[cfg(unix)]
    #[test]
    fn default_socket_path_env_precedence_and_default_paths() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_socket = std::env::var_os("PIXTUOID_SOCKET");
        let saved_xdg = std::env::var_os("XDG_RUNTIME_DIR");

        // PIXTUOID_SOCKET takes precedence (checked first).
        std::env::set_var("PIXTUOID_SOCKET", "/tmp/explicit.sock");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/tmp/explicit.sock")
        );

        // Without PIXTUOID_SOCKET, XDG_RUNTIME_DIR drives the path.
        std::env::remove_var("PIXTUOID_SOCKET");
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from("/run/user/1000/pixtuoid.sock")
        );

        // With neither set, fall back to the uid-suffixed /tmp socket.
        std::env::remove_var("XDG_RUNTIME_DIR");
        // SAFETY: getuid() is a trivial argless syscall.
        let uid = unsafe { libc::getuid() };
        assert_eq!(
            ClaudeCodeSource::default_socket_path(),
            PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
        );

        // Restore prior env so a later env-reading test in this binary isn't
        // poisoned by the cleared state.
        match saved_socket {
            Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
            None => std::env::remove_var("PIXTUOID_SOCKET"),
        }
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn default_paths_projects_root_honors_claude_config_dir() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_config = std::env::var_os("CLAUDE_CONFIG_DIR");
        let fallback_suffix = PathBuf::from(".claude").join("projects");

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let unset_paths = ClaudeCodeSource::default_paths();
        assert!(
            unset_paths.projects_root.ends_with(&fallback_suffix),
            "projects_root must end with .claude/projects, got {:?}",
            unset_paths.projects_root
        );

        let custom_dir = std::env::temp_dir().join("pixtuoid-claude-config-dir");
        std::env::set_var("CLAUDE_CONFIG_DIR", &custom_dir);
        assert_eq!(
            ClaudeCodeSource::default_paths().projects_root,
            custom_dir.join("projects")
        );

        std::env::set_var("CLAUDE_CONFIG_DIR", "");
        let empty_paths = ClaudeCodeSource::default_paths();
        assert!(
            empty_paths.projects_root.ends_with(&fallback_suffix),
            "empty CLAUDE_CONFIG_DIR must fall back to .claude/projects, got {:?}",
            empty_paths.projects_root
        );

        match saved_config {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }

    // CC on Windows slugs an absolute path like `C:\Users\foo\bar` into a project
    // dir name using `[^a-zA-Z0-9]→'-'` (regex from upstream CC source, drive
    // letter kept, no leading dash): `C--Users-foo-bar`. The fallback path in
    // `cc_derive_label` (empty cwd → rsplit on '-' → last non-empty segment)
    // must extract the project-basename `bar` and produce `cc·bar`. Verified
    // against upstream CC; real hook-payload fixture lands post-tester (PR 5).
    #[test]
    fn label_falls_back_to_project_dir_for_windows_slug() {
        // Windows slug: C:\Users\foo\bar  →  C--Users-foo-bar
        let path = Path::new("/Users/me/.claude/projects/C--Users-foo-bar/abc.jsonl");
        assert_eq!(
            cc_derive_label(path, "claude-code", Path::new("")),
            "cc·bar"
        );
    }

    // CC writes `message.content` as a plain STRING (not a block array) for
    // simple text turns — 4709 such lines in a local 2379-session / 822 MB
    // corpus. The tool-event match only fires on `Value::Array`, so a
    // string-content turn must decode to NOTHING (no events, no panic) — even
    // a slash-command wrapper line: content never drives lifecycle. A fuzz of
    // all 291k real lines through decode_cc_line confirmed zero panics; this
    // pins the common string-content shape the array-only fixtures never
    // exercise.
    // Coalescing guard: `cc_id_from_path` is invoked in multiple places that
    // must agree — the per-line decode (here), the watcher's `with_id_deriver`
    // (ClaudeCodeSource::run), and the hook decoder's session-id key. If the
    // per-line decode ever keys differently from the deriver, one CC session
    // splits into two sprites. Mirrors codex's
    // `decode_line_keys_agent_id_on_codex_id_from_path`.
    #[test]
    fn decode_cc_line_keys_agent_id_on_cc_id_from_path() {
        let path = "/Users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl";
        let events = decode_cc_line(
            path,
            "claude-code",
            serde_json::json!({"type":"assistant","attributionAgent":"explorer","message":{"content":[]}}),
        )
        .unwrap();
        let expected =
            AgentId::from_parts("claude-code", &cc_id_from_path(std::path::Path::new(path)));
        assert_eq!(
            events[0].agent_id(),
            expected,
            "decode_cc_line must key its AgentId on cc_id_from_path (the deriver)"
        );
    }

    // Lifecycle must never read chat content: a user message QUOTING the CC
    // slash-command wrapper mid-prose (common in sessions discussing CC
    // internals) is user-controllable text, not a lifecycle signal. Neither
    // the live decode nor the tail scan may treat it as a session end.
    #[test]
    fn quoted_exit_wrapper_in_user_content_never_ends_the_session() {
        let prose =
            "the transcript shows <command-name>/exit</command-name> as a wrapped line — why?";
        let v = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prose }
        });
        let events = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", v).unwrap();
        assert!(
            events.is_empty(),
            "quoting the wrapper must not emit SessionEnd: {events:?}"
        );

        let tail = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prose }
        })
        .to_string();
        assert!(
            !cc_session_ended(tail.as_bytes()),
            "tail scan must not end a session on quoted wrapper text"
        );
    }

    #[test]
    fn string_content_turns_emit_no_tool_events() {
        for ty in ["assistant", "user"] {
            let v = serde_json::json!({
                "type": ty,
                "message": { "role": ty, "content": "just some prose, no tool blocks" }
            });
            let out = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", v).unwrap();
            assert!(
                out.is_empty(),
                "{ty} turn with string content must emit no events"
            );
        }
        // Even an exact slash-command wrapper decodes to nothing — the old
        // content-based /exit → SessionEnd matcher is gone (zero true
        // positives in a 135-transcript corpus; lifecycle is hooks + sweep).
        let exit = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": "<command-name>/exit</command-name>" }
        });
        let out = decode_cc_line("/x/.claude/projects/p/s.jsonl", "claude-code", exit).unwrap();
        assert!(
            out.is_empty(),
            "slash-command content must not emit lifecycle events: {out:?}"
        );
    }
}

#[cfg(test)]
mod cc_id_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cc_id_from_path_root_is_filename_uuid() {
        let p = Path::new(
            "/Users/me/.claude/projects/-Users-me-proj/01000000-0000-7000-8000-0000000000cc.jsonl",
        );
        assert_eq!(cc_id_from_path(p), "01000000-0000-7000-8000-0000000000cc");
    }

    #[test]
    fn cc_id_from_path_subagent_is_agent_stem() {
        let p = Path::new("/Users/me/.claude/projects/-Users-me-proj/01000000-0000-7000-8000-0000000000cc/subagents/agent-a0a7dc28dd772bd0d.jsonl");
        assert_eq!(cc_id_from_path(p), "agent-a0a7dc28dd772bd0d");
    }

    #[test]
    fn cc_id_from_path_empty_for_no_stem() {
        assert_eq!(cc_id_from_path(Path::new("")), "");
    }

    #[test]
    fn cc_id_from_path_is_stable_across_path_separators() {
        // The first-sight deriver gets a raw &Path; the per-line decoder gets the
        // normalize_path_key'd string (lowercased + forward-slashed on Windows).
        // Both must yield the SAME stem for a lowercase-hex CC id, or one session
        // splits into two sprites (same assumption codex_id_from_path relies on).
        let raw =
            Path::new("/Users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        let normalized =
            Path::new("/users/me/.claude/projects/p/01000000-0000-7000-8000-0000000000cc.jsonl");
        assert_eq!(cc_id_from_path(raw), cc_id_from_path(normalized));
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    #[cfg(unix)]
    use std::collections::HashSet;

    // Only the cfg(unix) tests write registry entries (the Windows impl never
    // reads them) — keep the helper gated too or it's dead code there.
    #[cfg(unix)]
    fn write_entry(dir: &Path, name: &str, pid: i64, session_id: &str) {
        std::fs::write(
            dir.join(name),
            serde_json::json!({ "pid": pid, "sessionId": session_id, "status": "idle" })
                .to_string(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_entry_whose_pid_is_alive_and_binds_its_pid() {
        let dir = tempfile::tempdir().unwrap();
        // Our own pid is alive by construction.
        write_entry(
            dir.path(),
            "self.json",
            std::process::id() as i64,
            "alive-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert!(
            live.ids.contains("alive-session"),
            "an entry with a live pid must be kept, got {live:?}"
        );
        // #223: the snapshot binds each vouched id to its owning OS pid (the
        // exit-watch half).
        assert_eq!(
            live.pid_of.get("alive-session"),
            Some(&(std::process::id() as i32)),
            "the vouched id must bind to the registry entry's pid"
        );
    }

    #[cfg(unix)]
    #[test]
    fn drops_entry_whose_pid_is_dead() {
        // Spawn-and-reap a real child: its pid is guaranteed dead once wait()
        // returns (modulo an astronomically unlikely instant reuse — the
        // accepted PID-reuse caveat, see live_cc_session_ids).
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id() as i64;
        child.wait().unwrap();

        let dir = tempfile::tempdir().unwrap();
        write_entry(dir.path(), "dead.json", dead_pid, "dead-session");
        write_entry(
            dir.path(),
            "alive.json",
            std::process::id() as i64,
            "alive-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert!(
            !live.ids.contains("dead-session"),
            "a crashed CC's leftover registry file must not count as live, got {live:?}"
        );
        assert!(
            live.ids.contains("alive-session"),
            "the live sibling must survive the dead entry, got {live:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_and_incomplete_entries_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let own_pid = std::process::id() as i64;
        std::fs::write(dir.path().join("garbage.json"), "not json {{{").unwrap();
        // Missing sessionId.
        std::fs::write(
            dir.path().join("nosid.json"),
            serde_json::json!({ "pid": own_pid }).to_string(),
        )
        .unwrap();
        // pid <= 0 would kill(0) our own process GROUP — must be rejected.
        write_entry(dir.path(), "pid0.json", 0, "group-session");
        // pid as a string (format drift) — not silently coerced.
        std::fs::write(
            dir.path().join("strpid.json"),
            serde_json::json!({ "pid": own_pid.to_string(), "sessionId": "str-pid" }).to_string(),
        )
        .unwrap();
        // Non-.json files are not registry entries.
        write_entry(dir.path(), "notes.txt", own_pid, "txt-session");
        write_entry(dir.path(), "valid.json", own_pid, "valid-session");

        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "only the well-formed live entry may survive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn oversized_junk_json_entry_is_ignored() {
        // Real registry entries are <1 KiB; a 256 KiB blob is junk (or drift
        // we couldn't parse anyway). The bounded read keeps the per-scan
        // probe cost flat — the truncated bytes just fail the JSON parse and
        // land in the warn-once skip.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("huge.json"), vec![b'x'; 256 * 1024]).unwrap();
        write_entry(
            dir.path(),
            "valid.json",
            std::process::id() as i64,
            "valid-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "an oversized junk entry must be ignored, not break the probe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_named_json_does_not_hang_the_probe() {
        // A FIFO named like a registry entry must be skipped BEFORE any read:
        // reading a writer-less FIFO blocks forever, which would hang the
        // probe and silently kill the whole CC watcher task. The file_type()
        // filter (which doesn't follow symlinks) rejects it without touching
        // its contents.
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo.json");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo only reads the NUL-terminated path; 0o600 owner-only.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        write_entry(
            dir.path(),
            "valid.json",
            std::process::id() as i64,
            "valid-session",
        );
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["valid-session".to_string()]),
            "a FIFO entry must be skipped; the live sibling must survive"
        );
    }

    #[test]
    fn missing_dir_is_probe_failure_none() {
        // #223: a MISSING registry dir on a machine where CC runs is
        // ambiguous (older CC, permissions) — explicit failure, the watcher
        // changes nothing. (On Windows the probe can't enumerate at all, so
        // None holds there too.)
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(live_cc_session_ids(&missing).is_none());
    }

    #[test]
    fn unreadable_dir_is_probe_failure_none() {
        // A FILE where the sessions dir should be makes read_dir fail
        // deterministically (no chmod games) — same explicit-failure path.
        let dir = tempfile::tempdir().unwrap();
        let file_not_dir = dir.path().join("sessions");
        std::fs::write(&file_not_dir, b"not a dir").unwrap();
        assert!(live_cc_session_ids(&file_not_dir).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn empty_readable_dir_is_some_empty() {
        // An empty but READABLE registry is a healthy observation: no CC is
        // running (meaningful — the negative vouch may act on it), NOT a
        // probe failure.
        let dir = tempfile::tempdir().unwrap();
        let snap = live_cc_session_ids(dir.path()).expect("empty readable dir is healthy");
        assert!(snap.ids.is_empty());
        assert!(snap.pid_of.is_empty());
    }

    // --- PID-reuse identity check (#220) -----------------------------------

    #[cfg(unix)]
    #[test]
    fn parse_registry_entry_extracts_started_at_and_tolerates_absence() {
        let with = serde_json::json!({
            "pid": 64924, "sessionId": "s", "startedAt": 1_781_109_422_174_u64
        })
        .to_string();
        let entry = parse_registry_entry(with.as_bytes()).unwrap();
        assert_eq!(entry.started_at_ms, Some(1_781_109_422_174));

        // Older CC without the field — still a valid entry (pid-alive-only).
        let without = serde_json::json!({ "pid": 64924, "sessionId": "s" }).to_string();
        let entry = parse_registry_entry(without.as_bytes()).unwrap();
        assert_eq!(entry.started_at_ms, None);

        // Malformed startedAt (string / negative) degrades to None, never a
        // parse failure — the identity check is additive.
        let junk =
            serde_json::json!({ "pid": 64924, "sessionId": "s", "startedAt": "soon" }).to_string();
        let entry = parse_registry_entry(junk.as_bytes()).unwrap();
        assert_eq!(entry.started_at_ms, None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pid_start_time_secs_reports_a_fresh_child_as_just_started() {
        // Hermetic: a child spawned NOW must have a kernel start time within a
        // few seconds of the current wall clock (proves both the FFI call and
        // the epoch-seconds unit — a ticks-since-boot misread would be off by
        // decades).
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let started = pid_start_time_secs(child.id() as i32);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let _ = child.kill();
        let _ = child.wait();
        let started = started.expect("proc_pidinfo must report a live child's start time");
        assert!(
            started.abs_diff(now_secs) <= 5,
            "child start time {started} should be within 5s of now {now_secs}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pid_start_time_secs_none_for_dead_pid() {
        assert_eq!(pid_start_time_secs(999_999_999), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recycled_pid_entry_is_dropped_and_matching_one_kept() {
        let own_pid = std::process::id() as i32;
        let own_start = pid_start_time_secs(own_pid).expect("own start time");
        let dir = tempfile::tempdir().unwrap();
        // Plausible recycling: the registry claims a start an hour before the
        // process actually started — a dead CC whose pid was reused.
        std::fs::write(
            dir.path().join("recycled.json"),
            serde_json::json!({
                "pid": own_pid,
                "sessionId": "recycled-session",
                "startedAt": (own_start - 3600) * 1000
            })
            .to_string(),
        )
        .unwrap();
        // Live measurement: CC stamps startedAt ~1.2s after process start —
        // inside the 10s tolerance.
        std::fs::write(
            dir.path().join("genuine.json"),
            serde_json::json!({
                "pid": own_pid,
                "sessionId": "genuine-session",
                "startedAt": own_start * 1000 + 1200
            })
            .to_string(),
        )
        .unwrap();
        let live = live_cc_session_ids(dir.path()).expect("readable dir is a healthy probe");
        assert_eq!(
            live.ids,
            HashSet::from(["genuine-session".to_string()]),
            "a recycled-pid entry must be dropped; the identity-matching one kept"
        );
    }

    #[cfg(unix)]
    #[test]
    fn entry_without_started_at_keeps_pid_alive_only_behavior() {
        // Additive fallback: no startedAt → no identity check → the live pid
        // alone vouches (exactly the pre-#220 behavior).
        let dir = tempfile::tempdir().unwrap();
        write_entry(
            dir.path(),
            "legacy.json",
            std::process::id() as i64,
            "legacy-session",
        );
        assert!(live_cc_session_ids(dir.path())
            .expect("readable dir is a healthy probe")
            .ids
            .contains("legacy-session"));
    }

    #[test]
    fn sessions_dir_derives_only_from_the_standard_layout() {
        // <claude_home>/projects → the sessions sibling.
        assert_eq!(
            cc_sessions_dir(Path::new("/home/u/.claude/projects")),
            Some(PathBuf::from("/home/u/.claude/sessions"))
        );
        // A fixture replay (--projects-root /tmp/fixture) has no registry
        // sibling — no probe, pure-mtime gate.
        assert_eq!(cc_sessions_dir(Path::new("/tmp/fixture")), None);
        assert_eq!(cc_sessions_dir(Path::new("projects")), None);
    }
}
