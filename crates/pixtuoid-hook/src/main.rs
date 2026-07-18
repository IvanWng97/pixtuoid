// Invariant #5 (non-negotiable): the shim must never block CC — it always exits
// 0 silently on any error. A prod `unwrap()`/`expect()`/`panic!` violates that
// (non-zero exit + a backtrace CC may surface), so they are compiler-denied in
// non-test builds for this safety-critical crate (tests unwrap freely). Scoped
// to the shim ONLY — a workspace-wide ban would churn ~150 grandfathered unwraps.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::Value;

mod paths;
use paths::default_socket_path;

mod transport;

/// Headroom reserved below the daemon's 1MiB pipe quota for what the shim
/// ADDS to stdin: the `_shim_ts_ms` stamp, the optional `_pixtuoid_source`
/// stamp, and the trailing newline (≲100 B worst case — pinned by
/// `stamp_headroom_covers_worst_case_stamps`). Without it, a payload within
/// ~65 B of 1MiB re-serializes to a wire line that exceeds the quota, and the
/// sync write can stall behind a momentarily busy daemon task until the
/// watchdog fires (event dropped).
const STAMP_HEADROOM: u64 = 256;

/// Stdin cap. `STDIN_CAP + STAMP_HEADROOM` equals the daemon's Windows pipe
/// in-buffer quota (`IN_BUFFER_SIZE = 1 << 20` in pixtuoid-core's
/// source/hook/windows.rs), so a stamped payload fits the pipe and the shim's
/// sync write can't stall on quota. The headroom covers what the SHIM adds;
/// pathological number canonicalization can still expand the body itself
/// (e.g. `1e9` re-serializes to `1000000000.0`) and an absurdly long
/// `--source` value can exceed the stamp budget — both degrade to the
/// pre-existing stall→watchdog→drop mode, never a block of CC.
const STDIN_CAP: u64 = (1 << 20) - STAMP_HEADROOM;

/// Saturating `u128 → u64` narrowing (`try_from`, NOT a truncating `as` cast,
/// which would WRAP a > u64::MAX value to a small number). Extracted as a pure fn
/// so the saturation is unit-testable with a synthetic over-MAX input — a real
/// `now_ms()` value never exercises the `u64::MAX` arm (ms-since-epoch fits u64
/// for ~580M years), so a test calling `now_ms()` alone can't pin the narrowing.
fn ms_u128_to_u64(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// Milliseconds since the epoch. A pre-epoch clock maps to 0 (same as before).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| ms_u128_to_u64(d.as_millis()))
}

fn main() -> Result<()> {
    let socket = default_socket_path();

    // `args_os` + lossy, NOT `args()`: `std::env::args()` PANICS on any
    // non-Unicode argument (legal Unix argv), breaching invariant #5's silent
    // exit-0. Lossy rather than filter_map: dropping a non-UTF-8 arg would
    // shift `--source <value>`/`--event <value>` pairing so the NEXT arg gets
    // read as the value; lossy preserves arity, and a U+FFFD-mangled value
    // simply fails the daemon's lookup downstream.
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let mut payload: Value = match event_from_argv(&args) {
        // CodeWhale env-mode: CodeWhale's hooks deliver identity as `DEEPSEEK_*`
        // ENV VARS, not a stdin JSON payload, and the registered command bakes
        // `--event <name>` (the event name is absent from the env). Critically,
        // for env-only events (session_start/tool_call_*/session_end) CodeWhale
        // does NOT pipe stdin, so the hook child INHERITS the TUI's terminal
        // stdin — a blind `read_to_string(stdin)` would BLOCK (and tool_call_before
        // runs synchronously, freezing the user's tool call until the hook
        // timeout). So when `--event` is present we build the envelope from env
        // and NEVER touch stdin. Mirrors CodeWhale 0.8.59's
        // `hooks.rs::execute_sync_inner`.
        Some(event) => Value::Object(env_payload(&event)),
        None => {
            let mut buf = String::new();
            if std::io::stdin()
                .take(STDIN_CAP)
                .read_to_string(&mut buf)
                .is_err()
            {
                return Ok(());
            }
            match serde_json::from_str(&buf) {
                Ok(v) => v,
                // If we can't parse, exit 0 silently so CC isn't blocked.
                Err(_) => return Ok(()),
            }
        }
    };

    if let Value::Object(map) = &mut payload {
        // Source precedence: the `--source <name>` argv flag (the Windows install
        // form — cmd.exe /C can't express a POSIX `VAR=value cmd` env-prefix) wins,
        // then the `PIXTUOID_SOURCE` env var (the Unix env-prefix form; grok
        // delivers the SAME var via its handler `env` map instead of a shell
        // prefix — this arm serves both). Either way the
        // daemon only ever sees the resulting `_pixtuoid_source` stamp. NB:
        // `--event` (env-mode) is orthogonal to source — CodeWhale's Unix install
        // resolves source via the env-prefix arm, its Windows install via `--source`;
        // `--event` never implies a source.
        let source = source_from_argv(&args).or_else(|| std::env::var("PIXTUOID_SOURCE").ok());
        enrich_payload(map, source, now_ms(), parent_pid());
    }

    // Best-effort send, hard-bounded so a stuck daemon can never block CC's
    // subprocess wait — see transport.rs for the per-platform mechanism.
    let mut line = serde_json::to_vec(&payload).unwrap_or_default();
    line.push(b'\n');
    transport::send_line(&socket, &line);
    Ok(())
}

/// CodeWhale env-mode: synthesize the hook envelope from `DEEPSEEK_*` env vars
/// (CodeWhale's hooks carry identity there, not on stdin). `event` is the
/// baked `--event <name>`; `cwd` (the AgentId key), `tool`, and `tool_args` are
/// read from env. Pure assembly split from the `std::env` read so it is
/// testable without mutating process-global env (the source/socket env tests
/// are the crate's only env-touching ones — see `default_socket_path_branches`).
fn env_payload(event: &str) -> serde_json::Map<String, Value> {
    // CodeWhale runs the hook with current_dir = its working dir (= the
    // workspace), so the shim's own cwd is the reliable cwd fallback when
    // DEEPSEEK_WORKSPACE is unset (see env_payload_from).
    let cwd_fallback = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    env_payload_from(event, cwd_fallback, parent_pid(), |k| std::env::var(k).ok())
}

/// The spawning CLI's pid — the shim's parent. Two consumers: CodeWhale's
/// env-mode `_pid` (the daemon's liveness watch: an abrupt exit that fires no
/// `session_end` ends the sprite promptly) and the generic fill-if-absent
/// `_pid` stamp (the TUI's focus-jump). `sh -c` EXEC's the hook (verified: the
/// hook's getppid() == the CLI's pid), so the shim's parent IS the CLI — on
/// UNIX. On Windows the hook runs under `cmd /C`, so the parent is `cmd.exe`
/// (the WRONG pid, and it exits right after spawning the shim → a false
/// exit / a recycled-pid focus), so we send no pid there: CodeWhale falls
/// back to `session_end` + the stale-sweep, focus to a silent no-op.
/// Container caveat: with a bind-mounted socket this pid is a CONTAINER-
/// namespace pid the host-side daemon can't translate — the walk then hits
/// an unrelated host process (usually surface-less → no-op). Gated behind
/// deliberate socket sharing; default containers never reach the socket.
#[cfg(unix)]
fn parent_pid() -> Option<u32> {
    // getppid() is always safe (no args, infallible) and gives the hook's
    // parent — the spawning CLI, since `sh -c` exec's the hook (verified).
    u32::try_from(unsafe { libc::getppid() }).ok()
}
#[cfg(not(unix))]
fn parent_pid() -> Option<u32> {
    None
}

/// Per-field byte cap on env-mode values. The stdin arm enforces `STDIN_CAP`
/// (≈1 MiB) before parsing, so a stamped stdin payload always fits the daemon's
/// pipe quota; the env arm has no such gate, and `DEEPSEEK_TOOL_ARGS` can be
/// large (a big write/edit tool's input). Capping each of the ≤3 folded fields
/// keeps the serialized line well under 1 MiB (3 × 128 KiB ≪ 1 MiB), so a large
/// tool's `tool_call_before` still delivers instead of building a >1 MiB line
/// the 200 ms watchdog would drop (invariant #5 holds either way, but the event
/// — the sprite's "working" pulse — would otherwise be lost).
const ENV_FIELD_CAP: usize = 128 * 1024;

/// Byte-bounded, char-SAFE truncation (never split a UTF-8 scalar — same idiom
/// CodeWhale itself uses; the shim must never produce invalid UTF-8). The cap
/// is a hard ceiling: a scalar STRADDLING the boundary is dropped (floor to
/// the previous char boundary), never kept — bounding the char's START let
/// the result exceed the cap by up to 3 bytes. `cwd` is
/// the AgentId key but a real workspace path is far under the cap, so it is
/// never truncated in practice; a crafted oversized one is bounded to a stable
/// prefix (two such events still coalesce — correct). A truncated `tool_args`
/// just yields no target suffix (the decoder degrades gracefully on unparseable
/// JSON).
fn cap_env_field(mut val: String) -> String {
    if val.len() > ENV_FIELD_CAP {
        let end = val
            .char_indices()
            .take_while(|(i, c)| i + c.len_utf8() <= ENV_FIELD_CAP)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        val.truncate(end);
    }
    val
}

fn env_payload_from(
    event: &str,
    cwd_fallback: Option<String>,
    pid: Option<u32>,
    get: impl Fn(&str) -> Option<String>,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("event".into(), Value::from(event));
    // CodeWhale's pid for the daemon's liveness watch (see `cw_parent_pid`).
    if let Some(pid) = pid {
        map.insert("_pid".into(), Value::from(pid));
    }
    // cwd is the AgentId KEY (the decoder drops a cwd-less event). Prefer
    // DEEPSEEK_WORKSPACE, but fall back to the hook child's own working dir:
    // CodeWhale runs the hook with current_dir = its working dir (= the
    // workspace), and DEEPSEEK_WORKSPACE is UNSET for a fresh `codewhale`
    // launched without `-C` until the workspace resolves — so `session_start`
    // would otherwise carry no cwd and never register a sprite. The fallback
    // resolves to the same path the workspace eventually does, so all of a
    // session's events coalesce on one AgentId.
    if let Some(cwd) = get("DEEPSEEK_WORKSPACE")
        .filter(|v| !v.is_empty())
        .or_else(|| cwd_fallback.filter(|v| !v.is_empty()))
    {
        map.insert("cwd".into(), Value::from(cap_env_field(cwd)));
    }
    // (env var, envelope field) — the remaining fields `source/codewhale.rs`
    // reads. A missing or empty value is omitted; a present value is capped.
    for (env_key, field) in [
        ("DEEPSEEK_TOOL_NAME", "tool"),
        ("DEEPSEEK_TOOL_ARGS", "tool_args"),
    ] {
        if let Some(val) = get(env_key).filter(|v| !v.is_empty()) {
            map.insert(field.into(), Value::from(cap_env_field(val)));
        }
    }
    map
}

/// The value of `--<flag> <val>` or `--<flag>=<val>` in argv (first match wins),
/// or `None` if absent or empty. Total + panic-free per invariant #5 — the one
/// scanner behind `event_from_argv` / `source_from_argv`.
fn flag_from_argv(args: &[String], flag: &str) -> Option<String> {
    let eq_prefix = format!("{flag}=");
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(val) = arg.strip_prefix(&eq_prefix) {
            return Some(val).filter(|s| !s.is_empty()).map(str::to_string);
        }
        if arg == flag {
            return it.next().filter(|s| !s.is_empty()).cloned();
        }
    }
    None
}

/// The baked event name from `--event <name>` (or `--event=<name>`) in argv —
/// CodeWhale's env-mode trigger. Absent or empty → `None` (the shim reads its
/// payload from stdin, the unchanged CC/Codex/Reasonix path). Total + panic-free
/// per invariant #5, mirroring `source_from_argv`.
fn event_from_argv(args: &[String]) -> Option<String> {
    flag_from_argv(args, "--event")
}

/// The trusted CLI source from `--source <name>` (or `--source=<name>`) in argv.
/// This is the Windows install form: the codex hook command runs under `cmd.exe
/// /C`, which has no inline `VAR=value cmd` env-prefix syntax (it would try to exec
/// a program literally named `PIXTUOID_SOURCE=codex`), so the source rides as a
/// flag instead. Absent or empty → `None` so the caller falls back to the
/// `PIXTUOID_SOURCE` env var (the unchanged Unix install form). Total + panic-free
/// per invariant #5 (the shim must never block CC).
fn source_from_argv(args: &[String]) -> Option<String> {
    flag_from_argv(args, "--source")
}

/// Stamp the shim timestamp and, when a source is resolved, the trusted CLI
/// source under the PRIVATE `_pixtuoid_source` key.
///
/// We deliberately do NOT write the public `source` field: CC's SessionStart
/// payload already uses `source` for the start *reason* (startup/resume/clear/
/// compact). Reading that as the CLI source namespaced the agent under
/// "startup", splitting it from the claude-code-keyed tool/JSONL/SessionEnd
/// events — an un-reapable ghost. The private key is shim-OWNED — the daemon
/// trusts it exclusively for CLI attribution — so any inbound
/// `_pixtuoid_source` (spoofed or replayed) is stripped unconditionally
/// before stamping; the daemon never sees a value the shim didn't write.
/// Absent any source (bare `pixtuoid-hook`, i.e. CC), no key is stamped and
/// the decoder defaults to claude-code.
fn enrich_payload(
    map: &mut serde_json::Map<String, Value>,
    source: Option<String>,
    ts_ms: u64,
    ppid: Option<u32>,
) {
    map.remove("_pixtuoid_source");
    map.insert("_shim_ts_ms".into(), Value::from(ts_ms));
    if let Some(src) = source {
        if !src.is_empty() {
            map.insert("_pixtuoid_source".into(), Value::from(src));
        }
    }
    // The agent process's pid under the EXISTING `_pid` key (the daemon's
    // envelope peek in hook/mod.rs already consumes it for the abrupt-exit
    // watch, and now for the TUI's focus-jump). Deliberately NOT shim-owned:
    // opencode's plugin and CodeWhale's env-mode supply their own `_pid`
    // (the plugin's value is more authoritative than our getppid), so an
    // inbound value is KEPT and the shim only fills the gap for the CLIs
    // that exec it directly. None (Windows cmd /C wrapper — see parent_pid)
    // fills nothing; focus degrades to a silent no-op there.
    if let Some(ppid) = ppid {
        map.entry("_pid").or_insert_with(|| Value::from(ppid));
    }
}

#[cfg(test)]
mod tests;
