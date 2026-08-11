//! Oh My Pi (`omp`, omp.sh) source. Watches the omp session transcripts
//! (`<omp_sessions_dir>/<encoded-cwd>/<ts>_<uuid>.jsonl`) via `JsonlWatcher`.
//! That root is USUALLY `<agent-dir>/sessions`, but the XDG redirect flattens
//! the `agent/` segment away — see `omp_sessions_dir`, which owns every axis.
//! TRANSCRIPT-ONLY: omp's "hooks" are in-process TS extension modules, so there
//! is no shell-hook install target.
//!
//! Wire shape (upstream `packages/coding-agent/src/session/`):
//! - **File**: `${fileSafeTimestamp}_${uuidv7}.jsonl`, under a per-cwd encoded
//!   dir that always starts with `-`. Line 1 is a fixed-width 256-byte
//!   `type:"title"` slot rewritten IN PLACE on rename (pwrite at offset 0 —
//!   the tail cursor sits past byte 256, so it is never re-read); line 2 the
//!   `type:"session"` header (id, cwd); legacy files lack the slot.
//! - **Turn**: `type:"message"` entries — `role:"assistant"` content carries
//!   `type:"toolCall"` blocks (`{id,name,arguments}`); `role:"toolResult"`
//!   closes one (`toolCallId`). A `custom` entry
//!   `customType:"tool_execution_start"` duplicates each toolCall right
//!   before execution — deliberately NOT decoded (same `tool_use_id` would
//!   double-count `tool_call_count`).
//! - **Exit**: a `custom` entry `customType:"session_exit"` is appended on
//!   every clean teardown incl. SIGINT/SIGTERM — the session-ended marker.
//!   Skipped when the session never produced an assistant message; SIGKILL
//!   writes nothing → stale-sweep.
//! - **Subagents**: the `task` tool persists each child as a SEPARATE file
//!   `<parent-path-minus-.jsonl>/<taskId>.jsonl`, recursively — linkage is the
//!   PATH NESTING, not a header field (the child header has no
//!   `parentSession`).
//!
//! Sharp edge: fork / branch / version-migration / tool-output pruning REWRITE
//! the file atomically (temp + rename → new inode); the watcher re-stats by
//! path, so a rewrite reads as a fresh transcript. Rare enough to live with.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::OmpSource;

/// The Oh My Pi (omp) source's registry name (its `SourceDescriptor.name`).
pub const SOURCE_NAME: &str = "omp";

/// omp's SESSIONS root, mirroring `dirs.ts` — deliberately NOT
/// `omp_agent_dir().join("sessions")`, because the XDG redirect FLATTENS the
/// `agent/` segment away (`$XDG_DATA_HOME/omp/sessions`).
///
/// Axes, in precedence order: `PI_CODING_AGENT_DIR` (dropped when it equals the
/// `PI_PROFILE`-derived dir); `OMP_PROFILE` else `PI_PROFILE` →
/// `profiles/<name>/agent`, where `OMP_PROFILE` wins whenever PRESENT so an
/// EMPTY value selects default rather than falling through; `PI_CONFIG_DIR` as
/// a directory NAME under home; XDG on linux+darwin, only with no override and
/// only when the target EXISTS (the file's header claims otherwise — its code
/// calls `fs.existsSync`).
pub(crate) fn omp_sessions_dir() -> PathBuf {
    let env = OmpEnv::from_process();
    resolve_omp_sessions_dir(
        &env,
        cfg!(any(target_os = "linux", target_os = "macos")),
        &|p| p.exists(),
    )
}

/// The process environment omp's resolver reads, injected so every arm — the
/// Windows one, the XDG one, the profile ones — unit-tests on any host.
struct OmpEnv {
    home: Option<PathBuf>,
    config_dir_name: Option<String>,
    omp_profile: Option<String>,
    pi_profile: Option<String>,
    /// PATH-valued, so `PathBuf` — a `String` here would drop a legal non-UTF-8
    /// override at the read. The three above are NAMES, not paths.
    agent_dir: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
}

impl OmpEnv {
    fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        Self {
            home: crate::platform::user_home_opt(),
            config_dir_name: var("PI_CONFIG_DIR"),
            omp_profile: var("OMP_PROFILE"),
            pi_profile: var("PI_PROFILE"),
            agent_dir: crate::platform::path_env("PI_CODING_AGENT_DIR"),
            xdg_data_home: crate::platform::path_env("XDG_DATA_HOME"),
        }
    }
}

/// Node's `path.join` for the ONE place the difference bites: a second segment
/// that is ROOTED. Node appends it to the base; Rust's `Path::join` discards the
/// base (`PathBuf::push`: an absolute path "replaces the current path", and one
/// with a prefix but no root "replaces self"). omp joins `PI_CONFIG_DIR` under
/// the home dir, so Rust's semantics would let it escape a home it never escapes
/// upstream.
///
/// Dropping the PREFIX matters as much as the root, and only on Windows: there
/// `C:\srv\omp` parses as Prefix+RootDir and replaces the base, while on Unix it
/// is one ordinary component that never could. Node keeps that `C:` as a literal
/// segment (`<home>\C:\srv\omp`), which Windows cannot even create — so for that
/// pathological input we deliberately produce the BOUND `<home>\srv\omp` rather
/// than reproduce an unusable path byte-for-byte. `..` is likewise left literal
/// rather than lexically normalized; the filesystem resolves it the same way.
fn node_join(base: &Path, segment: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for c in Path::new(segment).components() {
        match c {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `normalizeProfileName`: trimmed; empty or `"default"` is the implicit default
/// profile. An invalid name THROWS upstream, but every path we observe catches
/// it and proceeds with the default — so an invalid name is `None` here too.
fn normalize_profile_name(raw: Option<&str>) -> Option<String> {
    let n = raw?.trim();
    if n.is_empty() || n == "default" {
        return None;
    }
    // `^[a-z0-9][a-z0-9._-]{0,63}$`, plus the "." / ".." / trailing-dot bans.
    let ok_len = (1..=64).contains(&n.chars().count());
    let mut chars = n.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let tail_ok =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !ok_len || !head_ok || !tail_ok || n.ends_with('.') {
        return None;
    }
    // Windows reserved device basenames, extension included, case-insensitive.
    let stem = n.split('.').next().unwrap_or(n).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    (!reserved).then(|| n.to_string())
}

/// `resolveProfileEnv`: `OMP_PROFILE` wins whenever PRESENT — an empty value
/// selects the default profile rather than falling through to `PI_PROFILE`.
fn resolve_profile(env: &OmpEnv) -> Option<String> {
    match env.omp_profile.as_deref() {
        Some(omp) => normalize_profile_name(Some(omp)),
        None => normalize_profile_name(env.pi_profile.as_deref()),
    }
}

/// `getProfileConfigRoot` — `<home>/<PI_CONFIG_DIR|.omp>[/profiles/<profile>]`.
fn omp_config_root(env: &OmpEnv, profile: Option<&str>) -> Option<PathBuf> {
    let name = env
        .config_dir_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(".omp");
    let root = node_join(env.home.as_deref()?, name);
    Some(match profile {
        Some(p) => root.join("profiles").join(p),
        None => root,
    })
}

/// `DirResolver`'s `agentDir`, plus the `isDefault` flag the XDG arm gates on.
fn resolve_omp_agent_dir_parts(env: &OmpEnv) -> Option<(PathBuf, bool, Option<String>)> {
    let profile = resolve_profile(env);
    let default_agent = omp_config_root(env, profile.as_deref())?.join("agent");
    if profile.is_some() {
        // A named profile derives its own agent dir; the override is ignored.
        return Some((default_agent, true, profile));
    }
    // `resolvePreProfileAgentDir`: drop a value that IS the PI_PROFILE-derived
    // agent dir — it was exported by a parent's setProfile, not chosen here.
    let override_dir = env.agent_dir.clone().filter(|v| {
        let derived = normalize_profile_name(env.pi_profile.as_deref())
            .and_then(|p| omp_config_root(env, Some(&p)))
            .map(|r| r.join("agent"));
        derived.is_none_or(|d| *v != d)
    });
    match override_dir {
        Some(v) => Some((
            crate::platform::warn_if_relative_override("PI_CODING_AGENT_DIR", v),
            false,
            profile,
        )),
        None => Some((default_agent, true, profile)),
    }
}

fn resolve_omp_agent_dir(env: &OmpEnv) -> PathBuf {
    resolve_omp_agent_dir_parts(env)
        .map(|(d, _, _)| d)
        // The home-less fallback keeps the pre-#880 shape rather than inventing
        // a new one: an unresolvable home was already `/tmp`-rooted by `user_home`.
        .unwrap_or_else(|| crate::platform::user_home().join(".omp").join("agent"))
}

/// `getSessionsDir()` = `agentSubdir(undefined, "sessions", "data")`.
fn resolve_omp_sessions_dir(
    env: &OmpEnv,
    xdg_platform: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    let Some((agent_dir, is_default, profile)) = resolve_omp_agent_dir_parts(env) else {
        return resolve_omp_agent_dir(env).join("sessions");
    };
    let xdg = (xdg_platform && is_default)
        .then(|| xdg_app_root(env.xdg_data_home.as_deref(), profile.as_deref(), exists))
        .flatten();
    // The flatten lives here: with XDG the base REPLACES `<…>/agent`, it does
    // not sit under it.
    xdg.unwrap_or(agent_dir).join("sessions")
}

/// `resolveIf`: `$XDG_DATA_HOME/omp` (or its `profiles/<name>` child) when that
/// directory EXISTS — the existence gate is what makes a not-yet-migrated user
/// keep the home-rooted layout.
fn xdg_app_root(
    xdg_data_home: Option<&Path>,
    profile: Option<&str>,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let app_root = node_join(xdg_data_home?, "omp");
    let candidate = match profile {
        Some(p) => app_root.join("profiles").join(p),
        None => app_root,
    };
    exists(&candidate).then_some(candidate)
}

/// Does a path component look like a ROOT session file stem
/// (`${fileSafeTimestamp}_${uuid}`)? Subagent stems are task ids (`Alpha`,
/// `GoodWolf`) and never date-shaped. The `T` check is case-insensitive: on
/// Windows the per-line decoder receives the `normalize_path_key`'d path
/// (LOWERCASED), so requiring an upper-case `T` breaks the whole stem chain.
fn looks_like_session_stem(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() > 20
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10].eq_ignore_ascii_case(&b'T')
        && s.contains('_')
}

/// The stem chain from the root session down to this transcript, e.g.
/// `["<ts>_<uuid>", "Alpha", "Child"]` for a nested subagent file
/// `…/<ts>_<uuid>/Alpha/Child.jsonl`. A root transcript is `[stem]`.
///
/// PURE and case-preserving: raw in → raw out on every platform, so the
/// fixture-fed conformance goldens stay platform-invariant. The Windows
/// case-fold belongs at the WATCHER seam (walk.rs, and the probe's own
/// boundary), never here.
fn stem_chain(path: &Path) -> Vec<String> {
    let own = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut chain = vec![own];
    let mut cur = path.parent();
    while let Some(dir) = cur {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            break;
        };
        if looks_like_session_stem(name) {
            chain.push(name.to_string());
            break;
        }
        // Bound the climb at omp's layout boundaries (the `-`-prefixed per-cwd
        // dir, the `sessions` root). Without this the walk continues into the
        // USER'S path above the watched root, where a date-shaped component
        // (`~/backups/2026-…_snap/agent/…`) would misclassify every root
        // transcript as a subagent chain.
        if name == "sessions" || name.starts_with('-') {
            break;
        }
        // A task-id dir is indistinguishable from a foreign dir until the root
        // stem shows up above it, so collect speculatively and discard below.
        chain.push(name.to_string());
        cur = dir.parent();
    }
    if chain.last().is_some_and(|top| looks_like_session_stem(top)) {
        chain.reverse();
        chain
    } else {
        chain.truncate(1);
        chain
    }
}

/// AgentId key: the root stem for a root transcript; the `/`-joined stem
/// chain for a (nested) subagent, so `Alpha` under two different sessions
/// never collides.
pub fn omp_id_from_path(path: &Path) -> String {
    stem_chain(path).join("/")
}

/// The parent's key (the chain minus the last segment); `None` for a root.
pub(crate) fn omp_parent_key_from_path(path: &Path) -> Option<String> {
    let chain = stem_chain(path);
    (chain.len() > 1).then(|| chain[..chain.len() - 1].join("/"))
}

/// The COMPLETE `RawFileEntry` union of omp session-entry `type`
/// discriminators (upstream `session/session-entries.ts`), NOT just the ones we
/// decode: a `type` OUTSIDE this set breadcrumbs, a KNOWN-but-undecoded one
/// stays silent, because `drift::unknown_event` has NO dedup and omp writes
/// those routinely (drift.rs's anti-flood rule). The nested `customType` axis
/// is deliberately NOT guarded — unbounded and extension-authored. Kept honest
/// by `read_omp_known_types` in `check_upstream_drift.py`.
const KNOWN_ENTRY_TYPES: &[&str] = &[
    "branch_summary",
    "compaction",
    "credential_pin",
    "custom",
    "custom_message",
    "label",
    "message",
    "mode_change",
    "model_change",
    "reset_boundary",
    "service_tier_change",
    "session",
    "session_init",
    "thinking_level_change",
    "title",
    "title_change",
    "ttsr_injection",
];

/// Decode one omp session JSONL line into zero or more `AgentEvent`s.
/// Unknown entry types / roles and malformed shapes return `vec![]` — the
/// upstream loader is itself lenient (`parseJsonlLenient`).
pub fn decode_omp_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let path = Path::new(transcript_path);
    let acting = AgentId::from_parts(source, &omp_id_from_path(path));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let kind = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    let out = match kind {
        // session_id must be the SAME id-deriver key the watcher's first-sight
        // registration uses, so the two SessionStarts agree.
        "session" => {
            let cwd = obj.get("cwd").and_then(|c| c.as_str()).unwrap_or_else(|| {
                crate::source::drift::missing_field(source, "session", "cwd");
                ""
            });
            let parent_id = omp_parent_key_from_path(path).map(|k| AgentId::from_parts(source, &k));
            vec![AgentEvent::SessionStart {
                agent_id: acting,
                source: source.to_string(),
                session_id: omp_id_from_path(path),
                cwd: PathBuf::from(cwd),
                parent_id,
            }]
        }
        "message" => {
            let Some(msg) = obj.get("message") else {
                return Ok(vec![]);
            };
            match msg.get("role").and_then(|r| r.as_str()) {
                Some("assistant") => {
                    let mut out = Vec::new();
                    // The BARE `model` field, never the provider-prefixed
                    // `model_change` form, so TOP_MODELS prefix matching sees
                    // the same vocabulary CC/codex/copilot emit.
                    if let Some(model) = msg
                        .get("model")
                        .and_then(|m| m.as_str())
                        .filter(|m| !m.is_empty())
                    {
                        out.push(AgentEvent::ModelInfo {
                            agent_id: acting,
                            model: Some(ellipsize(model, MAX_DECODED_FIELD_CHARS)),
                            effort: None,
                        });
                    }
                    // omp's `usage.input` EXCLUDES the cache share
                    // (totalTokens = input + output + cacheRead + cacheWrite),
                    // so fresh = input + cacheWrite + output — cache READS are
                    // re-served context, not new spend.
                    if let Some(usage) = msg.get("usage").and_then(|u| u.as_object()) {
                        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                        let fresh = field("input")
                            .saturating_add(field("cacheWrite"))
                            .saturating_add(field("output"));
                        if fresh > 0 {
                            out.push(AgentEvent::Usage {
                                agent_id: acting,
                                fresh_tokens: fresh,
                            });
                        }
                    }
                    let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
                        return Ok(out);
                    };
                    // `ask` BLOCKS on human input: its Start binds the
                    // reducer's `gated_before_waiting` gate to the ask's own
                    // tool_use_id, so the answer's toolResult resolves the
                    // Wait. Ask pairs are appended LAST because a sibling's
                    // later ActivityStart would flip the slot back to Active
                    // and drop the gate, stranding the Wait forever.
                    let mut asks = Vec::new();
                    for b in blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
                    {
                        // `id` is a REQUIRED pairing key on a block we decode,
                        // so its absence is upstream drift, not a line we
                        // ignore — breadcrumb before dropping.
                        let Some(id) = b.get("id").and_then(|i| i.as_str()) else {
                            crate::source::drift::missing_field(source, "toolCall", "id");
                            continue;
                        };
                        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or_else(|| {
                            crate::source::drift::missing_field(source, "toolCall", "name");
                            ""
                        });
                        let is_ask = name == "ask";
                        let dst = if is_ask { &mut asks } else { &mut out };
                        dst.push(AgentEvent::ActivityStart {
                            agent_id: acting,
                            tool_use_id: Some(id.to_string()),
                            detail: Some(omp_tool_detail(name, b.get("arguments"))),
                        });
                        if is_ask {
                            asks.push(AgentEvent::Waiting {
                                agent_id: acting,
                                reason: omp_ask_reason(b.get("arguments")),
                            });
                        }
                    }
                    out.extend(asks);
                    out
                }
                Some("toolResult") => {
                    let Some(tool_call_id) = msg.get("toolCallId").and_then(|i| i.as_str()) else {
                        // An unkeyable End can never close its Start (leaks
                        // Active forever) — breadcrumb, then drop.
                        crate::source::drift::missing_field(source, "toolResult", "toolCallId");
                        return Ok(vec![]);
                    };
                    vec![AgentEvent::ActivityEnd {
                        agent_id: acting,
                        tool_use_id: Some(tool_call_id.to_string()),
                    }]
                }
                // user / developer / bashExecution / … — not sprite-visible.
                _ => vec![],
            }
        }
        // Clean teardown marker: reason/kind ignored — every kind
        // ("normal"|"signal"|"fatal"|"process_exit") IS an end.
        "custom" if obj.get("customType").and_then(|c| c.as_str()) == Some("session_exit") => {
            vec![AgentEvent::SessionEnd {
                agent_id: acting,
                as_child: omp_parent_key_from_path(path).is_some(),
            }]
        }
        // A `type` OUTSIDE KNOWN_ENTRY_TYPES is a structural wire change — the
        // live-stream signal for a brand-new omp entry SHAPE that CI's
        // fetch-based defense can't see in a user's own stream.
        other if !other.is_empty() && !KNOWN_ENTRY_TYPES.contains(&other) => {
            crate::source::drift::unknown_event(source, other);
            vec![]
        }
        // KNOWN types that are not sprite-visible, silent so they can't flood.
        // (model_change stays undecoded even though the burn tier reads model:
        // its value is the provider-prefixed combined form, and every assistant
        // message re-stamps the bare `model` anyway — one turn's lag.)
        _ => vec![],
    };
    Ok(out)
}

/// omp's tool-detail dispatch. The subagent dispatch is the `task` tool,
/// detected by NAME only: `arguments` are model-authored, so a hallucinated
/// `subagent_type` key must not flip an ordinary tool to Delegating.
fn omp_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    if tool == "task" {
        return ToolDetail::Task;
    }
    const KEYS: &[&str] = &["command", "path", "pattern", "query"];
    crate::source::decoder::generic_keyed_detail(tool, args, KEYS)
}

/// The Waiting reason for an `ask` round: the first question's text, falling
/// back to the call's intent (`arguments.i`), then the bare tool name.
fn omp_ask_reason(args: Option<&Value>) -> String {
    args.and_then(|a| {
        a.get("questions")
            .and_then(|q| q.as_array())
            .and_then(|q| q.first())
            .and_then(|q| q.get("question"))
            .and_then(|q| q.as_str())
            .or_else(|| a.get("i").and_then(|i| i.as_str()))
    })
    .map(|t| ellipsize(t, MAX_DECODED_FIELD_CHARS))
    .unwrap_or_else(|| "ask".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ROOT: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001.jsonl";
    const ROOT_KEY: &str = "2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001";
    const CHILD: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001/Alpha.jsonl";
    const GRANDCHILD: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001/Alpha/GoodWolf.jsonl";

    #[test]
    fn a_root_stem_needs_both_the_date_shape_and_the_uuid_separator() {
        // The length gate is EXCLUSIVE: `<19-char ts>_` is the longest stem that
        // still carries no uuid, so it must not read as a root.
        assert!(!looks_like_session_stem("2026-07-16T12-00-05_"));
        assert!(looks_like_session_stem("2026-07-16T12-00-05_abc"));
        // Date-shaped and long enough, but no `_` at all — a subagent task id
        // can never reach this shape, so the separator is what makes it a root.
        assert!(!looks_like_session_stem("2026-07-16T12-00-05-999999"));
        // The `T` check stays case-insensitive for the lowercased Windows key.
        assert!(looks_like_session_stem("2026-07-16t12-00-05_abc"));
    }

    fn root() -> AgentId {
        AgentId::from_parts(SOURCE_NAME, ROOT_KEY)
    }
    fn decode_at(path: &str, line: &str) -> Vec<AgentEvent> {
        decode_omp_line(path, SOURCE_NAME, serde_json::from_str(line).unwrap()).unwrap()
    }
    fn decode(line: &str) -> Vec<AgentEvent> {
        decode_at(ROOT, line)
    }

    #[test]
    fn id_from_path_is_the_stem_for_a_root_and_the_chain_for_subagents() {
        assert_eq!(omp_id_from_path(Path::new(ROOT)), ROOT_KEY);
        assert_eq!(
            omp_id_from_path(Path::new(CHILD)),
            format!("{ROOT_KEY}/Alpha")
        );
        assert_eq!(
            omp_id_from_path(Path::new(GRANDCHILD)),
            format!("{ROOT_KEY}/Alpha/GoodWolf")
        );
    }

    #[test]
    fn parent_key_links_each_level_to_the_one_above() {
        assert_eq!(omp_parent_key_from_path(Path::new(ROOT)), None);
        assert_eq!(
            omp_parent_key_from_path(Path::new(CHILD)).as_deref(),
            Some(ROOT_KEY)
        );
        assert_eq!(
            omp_parent_key_from_path(Path::new(GRANDCHILD)),
            Some(format!("{ROOT_KEY}/Alpha"))
        );
    }

    #[test]
    fn same_task_id_under_two_sessions_keys_distinctly() {
        let other = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T09-00-00-000Z_0197f0bb-0000-7000-8000-000000000002/Alpha.jsonl";
        assert_ne!(
            omp_id_from_path(Path::new(CHILD)),
            omp_id_from_path(Path::new(other))
        );
    }

    #[test]
    fn stem_chain_survives_the_windows_case_fold() {
        let folded = "c:/users/u/.omp/agent/sessions/-dev-proj/2026-07-09t08-00-00-000z_0197f0aa-0000-7000-8000-000000000001/alpha.jsonl";
        assert_eq!(
            omp_id_from_path(Path::new(folded)),
            "2026-07-09t08-00-00-000z_0197f0aa-0000-7000-8000-000000000001/alpha",
            "a lowercased timestamp must still read as a session stem"
        );
        assert_eq!(
            omp_parent_key_from_path(Path::new(folded)).as_deref(),
            Some("2026-07-09t08-00-00-000z_0197f0aa-0000-7000-8000-000000000001"),
            "the parent link must survive the fold"
        );
    }

    #[test]
    fn date_shaped_dirs_above_the_sessions_root_do_not_misclassify() {
        let p = format!(
            "/home/u/backups/2026-01-01T00-00-00-000Z_snap/agent/sessions/-dev-proj/{stem}.jsonl",
            stem = ROOT_KEY
        );
        assert_eq!(omp_id_from_path(Path::new(&p)), ROOT_KEY);
        assert_eq!(omp_parent_key_from_path(Path::new(&p)), None);
    }

    #[test]
    fn session_header_registers_root_with_cwd_and_no_parent() {
        let line = r#"{"type":"session","version":3,"id":"0197f0aa-0000-7000-8000-000000000001","timestamp":"2026-07-09T08:00:00.000Z","cwd":"/home/u/proj"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(source, "omp");
                assert_eq!(
                    session_id, ROOT_KEY,
                    "session_id must match the watcher's id-deriver key"
                );
                assert_eq!(cwd, Path::new("/home/u/proj"));
                assert_eq!(*parent_id, None);
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn subagent_header_registers_child_parented_to_the_root() {
        let line = r#"{"type":"session","version":3,"id":"0197f0cc-0000-7000-8000-000000000003","timestamp":"2026-07-09T08:01:00.000Z","cwd":"/home/u/proj"}"#;
        match &decode_at(CHILD, line)[..] {
            [AgentEvent::SessionStart {
                agent_id,
                parent_id,
                ..
            }] => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, &format!("{ROOT_KEY}/Alpha"))
                );
                assert_eq!(*parent_id, Some(root()));
            }
            other => panic!("expected one parented SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn session_exit_ends_root_not_as_child() {
        let line = r#"{"type":"custom","id":"a1b2c3d4","parentId":"e5f6a7b8","timestamp":"2026-07-09T08:10:00.000Z","customType":"session_exit","data":{"reason":"exit command","kind":"normal","recordedAt":"2026-07-09T08:10:00.000Z"}}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(*agent_id, root());
                assert!(!*as_child);
            }
            other => panic!("expected root SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn session_exit_in_a_subagent_file_ends_the_child_as_child() {
        let line = r#"{"type":"custom","id":"a1b2c3d4","parentId":null,"timestamp":"t","customType":"session_exit","data":{"reason":"task complete","kind":"normal","recordedAt":"t"}}"#;
        match &decode_at(CHILD, line)[..] {
            [AgentEvent::SessionEnd { agent_id, as_child }] => {
                assert_eq!(
                    *agent_id,
                    AgentId::from_parts(SOURCE_NAME, &format!("{ROOT_KEY}/Alpha"))
                );
                assert!(*as_child);
            }
            other => panic!("expected child SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn assistant_usage_becomes_a_fresh_token_observation() {
        let line = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[],"usage":{"input":122,"output":1491,"cacheRead":1000,"cacheWrite":1000,"totalTokens":3613},"timestamp":1720512000000}}"#;
        let evs = decode(line);
        assert!(
            evs.iter().any(|e| matches!(
                e,
                AgentEvent::Usage {
                    fresh_tokens: 2613,
                    ..
                }
            )),
            "expected fresh=2613 (cacheRead excluded), got {evs:?}"
        );
        let line = r#"{"type":"message","id":"m2","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[],"usage":{"input":0,"output":0,"cacheRead":500,"cacheWrite":0,"totalTokens":500},"timestamp":1720512000000}}"#;
        assert!(
            !decode(line)
                .iter()
                .any(|e| matches!(e, AgentEvent::Usage { .. })),
            "cache-read-only reading must be silent"
        );
    }

    #[test]
    fn assistant_tool_calls_start_activity_keyed_on_block_id() {
        let line = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"text","text":"Reading."},{"type":"toolCall","id":"toolu_01AAA","name":"read","arguments":{"path":"/home/u/proj/main.rs"}}],"stopReason":"toolUse","timestamp":1720512000000}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                detail: Some(ToolDetail::Generic { display }),
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(tool_use_id.as_deref(), Some("toolu_01AAA"));
                assert!(
                    display.contains("main.rs"),
                    "read tool should show its path target, got {display:?}"
                );
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn assistant_message_surfaces_model_info_for_the_burn_tier() {
        let line = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"t","message":{"role":"assistant","provider":"kimi-code","model":"kimi-for-coding","content":[{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"ls"}}],"timestamp":1}}"#;
        match &decode(line)[..] {
            [AgentEvent::ModelInfo {
                agent_id,
                model: Some(model),
                effort: None,
            }, AgentEvent::ActivityStart { .. }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(model.as_str(), "kimi-for-coding");
            }
            other => panic!("expected ModelInfo then ActivityStart, got {other:?}"),
        }
        let text_only = r#"{"type":"message","id":"m2","parentId":null,"timestamp":"t","message":{"role":"assistant","provider":"anthropic","model":"claude-fable-5","content":[{"type":"text","text":"done"}],"timestamp":2}}"#;
        match &decode(text_only)[..] {
            [AgentEvent::ModelInfo { model: Some(m), .. }] => {
                assert_eq!(m.as_str(), "claude-fable-5");
            }
            other => panic!("expected one ModelInfo, got {other:?}"),
        }
        let empty = r#"{"type":"message","id":"m3","timestamp":"t","message":{"role":"assistant","model":"","content":[],"timestamp":3}}"#;
        assert!(decode(empty).is_empty());
        // Defensive: pi-ai types.ts requires `content`, so a content-ABSENT
        // message can't occur on real wire — this pins the let-else early
        // return's `Ok(out)`, not `Ok(vec![])`.
        let no_content = r#"{"type":"message","id":"m4","timestamp":"t","message":{"role":"assistant","model":"claude-fable-5","timestamp":4}}"#;
        match &decode(no_content)[..] {
            [AgentEvent::ModelInfo { model: Some(m), .. }] => {
                assert_eq!(m.as_str(), "claude-fable-5");
            }
            other => panic!("expected one ModelInfo, got {other:?}"),
        }
    }

    #[test]
    fn parallel_tool_calls_each_start_activity() {
        let line = r#"{"type":"message","id":"m1","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"cargo test"}},{"type":"toolCall","id":"t2","name":"grep","arguments":{"pattern":"fn main"}}],"timestamp":1}}"#;
        let evs = decode(line);
        assert_eq!(evs.len(), 2, "one ActivityStart per toolCall block");
        match &evs[..] {
            [AgentEvent::ActivityStart {
                tool_use_id: id1, ..
            }, AgentEvent::ActivityStart {
                tool_use_id: id2, ..
            }] => {
                assert_eq!(id1.as_deref(), Some("t1"));
                assert_eq!(id2.as_deref(), Some("t2"));
            }
            other => panic!("expected two ActivityStarts, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_ends_activity_keyed_on_tool_call_id() {
        let line = r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"t","message":{"role":"toolResult","toolCallId":"toolu_01AAA","toolName":"read","content":[{"type":"text","text":"fn main() {}"}],"isError":false,"timestamp":1720512001000}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(tool_use_id.as_deref(), Some("toolu_01AAA"));
            }
            other => panic!("expected one ActivityEnd, got {other:?}"),
        }
    }

    #[test]
    fn task_dispatch_is_delegating() {
        let line = r#"{"type":"message","id":"m3","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"t3","name":"task","arguments":{"task":"fix the flaky test","id":"Alpha"}}],"timestamp":1}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => assert!(d.is_task(), "task tool must be Delegating, got {d:?}"),
            other => panic!("expected Delegating ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn spoofed_subagent_type_arg_does_not_make_a_task() {
        let line = r#"{"type":"message","id":"m4","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"t4","name":"read","arguments":{"path":"x.rs","subagent_type":null}}],"timestamp":1}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                detail: Some(d), ..
            }] => assert!(
                !d.is_task(),
                "a spoofed subagent_type arg must stay Generic, got {d:?}"
            ),
            other => panic!("expected Generic ActivityStart, got {other:?}"),
        }
    }

    #[test]
    fn ask_call_starts_activity_then_waits_on_the_question() {
        let line = r#"{"type":"message","id":"m7","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK1","name":"ask","arguments":{"i":"Resolving packages/ui collision","questions":[{"id":"ui_collision","question":"packages/ui already exists. What should happen?","options":[{"label":"Replace"},{"label":"Merge"}]}]}}],"timestamp":1}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                agent_id,
                tool_use_id,
                ..
            }, AgentEvent::Waiting {
                agent_id: wid,
                reason,
            }] => {
                assert_eq!(*agent_id, root());
                assert_eq!(*wid, root());
                assert_eq!(tool_use_id.as_deref(), Some("tool_ASK1"));
                assert!(
                    reason.contains("packages/ui already exists"),
                    "reason carries the question text, got {reason:?}"
                );
            }
            other => panic!("expected ActivityStart then Waiting, got {other:?}"),
        }
    }

    #[test]
    fn ask_reason_falls_back_to_intent_then_bare_name() {
        let intent = r#"{"type":"message","id":"m8","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK2","name":"ask","arguments":{"i":"Confirming scope"}}],"timestamp":1}}"#;
        match &decode(intent)[..] {
            [_, AgentEvent::Waiting { reason, .. }] => assert_eq!(reason, "Confirming scope"),
            other => panic!("expected Start+Waiting, got {other:?}"),
        }
        let bare = r#"{"type":"message","id":"m9","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK3","name":"ask"}],"timestamp":1}}"#;
        match &decode(bare)[..] {
            [_, AgentEvent::Waiting { reason, .. }] => assert_eq!(reason, "ask"),
            other => panic!("expected Start+Waiting, got {other:?}"),
        }
    }

    #[test]
    fn ask_batched_with_parallel_tool_calls_decodes_last() {
        let line = r#"{"type":"message","id":"mB","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK5","name":"ask","arguments":{"i":"Confirming scope"}},{"type":"toolCall","id":"t7","name":"bash","arguments":{"command":"cargo check"}}],"timestamp":1}}"#;
        match &decode(line)[..] {
            [AgentEvent::ActivityStart {
                tool_use_id: bash, ..
            }, AgentEvent::ActivityStart {
                tool_use_id: ask, ..
            }, AgentEvent::Waiting { .. }] => {
                assert_eq!(bash.as_deref(), Some("t7"));
                assert_eq!(ask.as_deref(), Some("tool_ASK5"));
            }
            other => panic!("expected bash Start, then ask Start+Waiting, got {other:?}"),
        }
    }

    #[test]
    fn ask_reason_is_capped_at_the_decode_boundary() {
        let long = "q".repeat(MAX_DECODED_FIELD_CHARS * 10);
        let line = format!(
            r#"{{"type":"message","id":"mA","parentId":null,"timestamp":"t","message":{{"role":"assistant","content":[{{"type":"toolCall","id":"tool_ASK4","name":"ask","arguments":{{"questions":[{{"id":"x","question":"{long}"}}]}}}}],"timestamp":1}}}}"#
        );
        match &decode(&line)[..] {
            [_, AgentEvent::Waiting { reason, .. }] => {
                assert_eq!(reason.chars().count(), MAX_DECODED_FIELD_CHARS + 1);
                assert!(reason.ends_with('…'));
            }
            other => panic!("expected Start+Waiting, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_without_id_is_dropped_and_without_name_still_starts() {
        let no_id = r#"{"type":"message","id":"m5","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","name":"bash","arguments":{}}],"timestamp":1}}"#;
        let out = crate::test_capture::capture_logs(|| {
            assert!(decode(no_id).is_empty(), "un-keyable toolCall → no event");
        });
        for needle in [crate::source::drift::TARGET, "missing_field", "toolCall"] {
            assert!(
                out.contains(needle),
                "no id breadcrumb: missing {needle:?}\n{out}"
            );
        }
        let no_name = r#"{"type":"message","id":"m6","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"t6","arguments":{}}],"timestamp":1}}"#;
        let out = crate::test_capture::capture_logs(|| match &decode(no_name)[..] {
            [AgentEvent::ActivityStart {
                tool_use_id,
                detail: Some(d),
                ..
            }] => {
                assert_eq!(tool_use_id.as_deref(), Some("t6"));
                assert!(!d.is_task());
            }
            other => panic!("expected one ActivityStart, got {other:?}"),
        });
        for needle in [crate::source::drift::TARGET, "missing_field", "toolCall"] {
            assert!(
                out.contains(needle),
                "no name breadcrumb: missing {needle:?}\n{out}"
            );
        }
    }

    #[test]
    fn toolresult_without_id_drops_with_a_drift_breadcrumb() {
        let no_id = r#"{"type":"message","id":"m7","parentId":null,"timestamp":"t","message":{"role":"toolResult","toolName":"read","content":[],"timestamp":1}}"#;
        let out = crate::test_capture::capture_logs(|| {
            assert!(decode(no_id).is_empty(), "un-keyable toolResult → no event");
        });
        for needle in [crate::source::drift::TARGET, "missing_field", "toolResult"] {
            assert!(
                out.contains(needle),
                "no toolResult breadcrumb: missing {needle:?}\n{out}"
            );
        }
    }

    #[test]
    fn tool_execution_start_custom_entry_is_deliberately_ignored() {
        let line = r#"{"type":"custom","id":"c1","parentId":null,"timestamp":"t","customType":"tool_execution_start","data":{"toolCallId":"toolu_01AAA","toolName":"read","startedAt":"t"}}"#;
        assert!(decode(line).is_empty());
    }

    #[test]
    fn non_lifecycle_entries_and_malformed_lines_are_ignored_not_panicked() {
        for line in [
            r#"{"type":"title","v":1,"title":"Fix flaky test","source":"auto","updatedAt":"t","pad":"   "}"#,
            r#"{"type":"title_change","id":"x","parentId":null,"timestamp":"t","title":"New","source":"user"}"#,
            r#"{"type":"model_change","id":"x","parentId":null,"timestamp":"t","model":"anthropic/claude-opus-4-5"}"#,
            r#"{"type":"compaction","id":"x","parentId":null,"timestamp":"t","summary":"…","firstKeptEntryId":"y","tokensBefore":1}"#,
            r#"{"type":"session_init","id":"x","parentId":null,"timestamp":"t","systemPrompt":"…","task":"…","tools":[]}"#,
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t","message":{"role":"user","content":"hi","timestamp":1}}"#,
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t","message":{"role":"bashExecution","command":"ls","output":"","exitCode":0,"timestamp":1}}"#,
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t"}"#,
            r#"{"type":"custom","id":"x","parentId":null,"timestamp":"t","customType":"memory_write","data":{}}"#,
        ] {
            assert!(decode(line).is_empty(), "expected no events for {line}");
        }
        assert!(decode_omp_line(ROOT, SOURCE_NAME, json!("not an object"))
            .unwrap()
            .is_empty());
        assert!(decode_omp_line(ROOT, SOURCE_NAME, json!(["array"]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unknown_entry_type_breadcrumbs_but_known_types_stay_silent() {
        let novel = r#"{"type":"quantum_entry","id":"x","parentId":null,"timestamp":"t"}"#;
        let logs = crate::test_capture::capture_logs(|| {
            assert!(
                decode(novel).is_empty(),
                "an unknown entry type decodes to no events"
            );
        });
        assert!(
            logs.contains("unknown_event") && logs.contains("quantum_entry"),
            "a brand-new omp entry type must fire the drift breadcrumb, got:\n{logs}"
        );

        for line in [
            r#"{"type":"title","v":1,"title":"t","source":"auto","updatedAt":"t","pad":"   "}"#,
            r#"{"type":"model_change","id":"x","parentId":null,"timestamp":"t","model":"m"}"#,
            r#"{"type":"compaction","id":"x","parentId":null,"timestamp":"t","summary":"…"}"#,
            r#"{"type":"custom","id":"x","parentId":null,"timestamp":"t","customType":"memory_write","data":{}}"#,
            r#"{"type":"thinking_level_change","id":"x","parentId":null,"timestamp":"t"}"#,
            r#"{"type":"credential_pin","id":"x","parentId":null,"timestamp":"t","provider":"anthropic","hash":"h"}"#,
            r#"{"type":"reset_boundary","id":"x","parentId":null,"timestamp":"t"}"#,
        ] {
            let quiet = crate::test_capture::capture_logs(|| {
                assert!(decode(line).is_empty());
            });
            assert!(
                !quiet.contains("unknown_event"),
                "known omp entry type must NOT breadcrumb (it would flood), got:\n{quiet}"
            );
        }
    }

    #[test]
    fn session_header_without_cwd_registers_with_empty_cwd() {
        let line = r#"{"type":"session","version":3,"id":"0197","timestamp":"t"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart { cwd, .. }] => {
                assert_eq!(cwd, Path::new(""), "missing cwd → empty path fallback");
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    /// `PI_CONFIG_DIR` is user-controlled, so the invariant is that NO value
    /// escapes home. Its TEETH are Windows-CI-only, and deliberately so: the
    /// drive-letter escape exists only there (`C:\\srv` parses as Prefix+RootDir
    /// and replaces the base), while on Unix the same string is one ordinary
    /// component that never could — so a green macOS run does NOT prove this,
    /// exactly like the windows-test class the root `CLAUDE.md` describes. The
    /// earlier test pinned only the POSIX form and was blind to it.
    #[test]
    fn node_join_never_lets_a_rooted_segment_escape_the_base() {
        let base = Path::new("/home/u");
        for hostile in [
            "/srv/omp",
            r"C:\srv\omp",
            r"C:srv",
            r"\srv",
            r"\\?\C:\srv",
            "plain",
        ] {
            let got = node_join(base, hostile);
            assert!(
                got.starts_with(base),
                "{hostile:?} escaped the base: {got:?}"
            );
        }
    }

    /// Every arm injected, so the XDG and Windows-reserved-name branches run on
    /// any host.
    #[test]
    fn omp_sessions_dir_mirrors_every_axis_of_the_upstream_resolver() {
        let base = |f: &dyn Fn(&mut OmpEnv)| {
            let mut e = OmpEnv {
                home: Some("/home/u".into()),
                config_dir_name: None,
                omp_profile: None,
                pi_profile: None,
                agent_dir: None,
                xdg_data_home: None,
            };
            f(&mut e);
            e
        };
        let never = |_: &Path| false;
        let sessions = |e: &OmpEnv| resolve_omp_sessions_dir(e, true, &never);

        assert_eq!(
            sessions(&base(&|_| {})),
            PathBuf::from("/home/u/.omp/agent/sessions")
        );

        // PI_CONFIG_DIR is a NAME under home...
        assert_eq!(
            sessions(&base(&|e| e.config_dir_name = Some(".myomp".into()))),
            PathBuf::from("/home/u/.myomp/agent/sessions")
        );
        // ...and an ABSOLUTE value does NOT escape home, because upstream joins
        // it Node-style. Rust's own `Path::join` would return `/srv/omp/...`.
        assert_eq!(
            sessions(&base(&|e| e.config_dir_name = Some("/srv/omp".into()))),
            PathBuf::from("/home/u/srv/omp/agent/sessions"),
            "an absolute PI_CONFIG_DIR must stay bound under home"
        );

        // Profiles.
        assert_eq!(
            sessions(&base(&|e| e.omp_profile = Some("work".into()))),
            PathBuf::from("/home/u/.omp/profiles/work/agent/sessions")
        );
        assert_eq!(
            sessions(&base(&|e| e.pi_profile = Some("work".into()))),
            PathBuf::from("/home/u/.omp/profiles/work/agent/sessions"),
            "PI_PROFILE is the legacy fallback when OMP_PROFILE is ABSENT"
        );
        assert_eq!(
            sessions(&base(&|e| {
                e.omp_profile = Some(String::new());
                e.pi_profile = Some("work".into());
            })),
            PathBuf::from("/home/u/.omp/agent/sessions"),
            "an EMPTY OMP_PROFILE explicitly selects default — it does not fall through"
        );
        for bad in ["CON", "com1.txt", "Work", "..", "trailing.", "-lead"] {
            assert_eq!(
                sessions(&base(&|e| e.omp_profile = Some(bad.into()))),
                PathBuf::from("/home/u/.omp/agent/sessions"),
                "invalid profile {bad:?} degrades to default, it does not build a path"
            );
        }

        // PI_CODING_AGENT_DIR, and the profile-derived drop rule.
        assert_eq!(
            sessions(&base(&|e| e.agent_dir = Some("/custom/agent".into()))),
            PathBuf::from("/custom/agent/sessions")
        );
        assert_eq!(
            sessions(&base(&|e| {
                e.omp_profile = Some("work".into());
                e.agent_dir = Some("/custom/agent".into());
            })),
            PathBuf::from("/home/u/.omp/profiles/work/agent/sessions"),
            "a named profile derives its own agent dir; the override is ignored"
        );
        assert_eq!(
            sessions(&base(&|e| {
                e.omp_profile = Some(String::new());
                e.pi_profile = Some("work".into());
                e.agent_dir = Some("/home/u/.omp/profiles/work/agent".into());
            })),
            PathBuf::from("/home/u/.omp/agent/sessions"),
            "a PI_PROFILE-derived override is DROPPED, not adopted as the default baseline"
        );
    }

    /// XDG changes the SHAPE, not the prefix: it replaces the base.
    #[test]
    fn omp_xdg_flattens_the_agent_segment_and_only_when_the_dir_exists() {
        let env = |xdg: Option<&str>, profile: Option<&str>, agent: Option<&str>| OmpEnv {
            home: Some("/home/u".into()),
            config_dir_name: None,
            omp_profile: profile.map(str::to_string),
            pi_profile: None,
            agent_dir: agent.map(PathBuf::from),
            xdg_data_home: xdg.map(PathBuf::from),
        };
        let exists = |want: &'static str| move |p: &Path| p == Path::new(want);
        let home_default = PathBuf::from("/home/u/.omp/agent/sessions");

        assert_eq!(
            resolve_omp_sessions_dir(&env(Some("/xdg"), None, None), true, &exists("/xdg/omp")),
            PathBuf::from("/xdg/omp/sessions"),
            "the agent/ segment is FLATTENED, not prefixed"
        );
        assert_eq!(
            resolve_omp_sessions_dir(&env(Some("/xdg"), None, None), true, &|_| false),
            home_default,
            "an absent $XDG_DATA_HOME/omp keeps the home layout — the existence gate"
        );
        assert_eq!(
            resolve_omp_sessions_dir(&env(Some("/xdg"), None, None), false, &exists("/xdg/omp")),
            home_default,
            "XDG is linux/darwin only"
        );
        assert_eq!(
            resolve_omp_sessions_dir(
                &env(Some("/xdg"), None, Some("/custom/agent")),
                true,
                &exists("/xdg/omp")
            ),
            PathBuf::from("/custom/agent/sessions"),
            "an agent-dir override makes isDefault false, which disables XDG"
        );
        assert_eq!(
            resolve_omp_sessions_dir(
                &env(Some("/xdg"), Some("work"), None),
                true,
                &exists("/xdg/omp/profiles/work")
            ),
            PathBuf::from("/xdg/omp/profiles/work/sessions"),
            "a profile keys the XDG choice on the PROFILE path, never the app root"
        );
        assert_eq!(
            resolve_omp_sessions_dir(
                &env(Some("/xdg"), Some("work"), None),
                true,
                &exists("/xdg/omp")
            ),
            PathBuf::from("/home/u/.omp/profiles/work/agent/sessions"),
            "the base app root existing is NOT enough for a named profile"
        );
        assert_eq!(
            resolve_omp_sessions_dir(&env(Some(""), None, None), true, &exists("/xdg/omp")),
            home_default,
            "a blank XDG_DATA_HOME never yields an ABSOLUTE xdg root, so the probe misses"
        );
    }

    /// Live-env twin of the injected matrix: proves the PRODUCTION fn reads
    /// the process env, not merely that the pure core computes from injected
    /// values.
    #[test]
    fn omp_sessions_dir_honors_non_empty_env_override() {
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("PI_CODING_AGENT_DIR");

        std::env::set_var("PI_CODING_AGENT_DIR", "/custom/agent");
        assert_eq!(
            omp_sessions_dir(),
            PathBuf::from("/custom/agent").join("sessions")
        );

        for blank in ["", "   "] {
            std::env::set_var("PI_CODING_AGENT_DIR", blank);
            let dflt = omp_sessions_dir();
            assert!(
                dflt.ends_with(Path::new(".omp/agent/sessions")),
                "blank override {blank:?} → ~/.omp/agent fallback, got {dflt:?}"
            );
        }

        std::env::remove_var("PI_CODING_AGENT_DIR");
        assert!(omp_sessions_dir().ends_with(Path::new(".omp/agent/sessions")));

        match saved {
            Some(v) => std::env::set_var("PI_CODING_AGENT_DIR", v),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }
}
