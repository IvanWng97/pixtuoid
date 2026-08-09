// Invariant #5 (non-negotiable): the shim must never block CC — it always exits 0
// silently on any error. A prod `unwrap()`/`expect()`/`panic!` violates that, so
// they are compiler-denied here (tests unwrap freely). Scoped to the shim only.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::Value;

mod cli_pid;
use cli_pid::cli_pid;

mod paths;
use paths::default_socket_path;

mod transport;

/// Headroom reserved below the daemon's 1MiB pipe quota for what the shim ADDS
/// to stdin (the `_shim_ts_ms`/`_pixtuoid_source` stamps and the trailing
/// newline). Without it a near-1MiB payload re-serializes to a wire line past
/// the quota, and the sync write can stall until the watchdog fires (event
/// dropped).
const STAMP_HEADROOM: u64 = 256;

/// Stdin cap. `STDIN_CAP + STAMP_HEADROOM` equals the daemon's Windows pipe
/// in-buffer quota, so a stamped payload fits the pipe and the shim's sync write
/// can't stall on quota. The headroom covers only what the SHIM adds; a
/// pathological body (number canonicalization, an absurdly long `--source`) can
/// still exceed it and degrade to the pre-existing stall→watchdog→drop mode,
/// never a block of CC.
const STDIN_CAP: u64 = (1 << 20) - STAMP_HEADROOM;

/// Saturating `u128 → u64` narrowing — a truncating `as` cast would WRAP a
/// > u64::MAX value to a small number.
fn ms_u128_to_u64(ms: u128) -> u64 {
    u64::try_from(ms).unwrap_or(u64::MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| ms_u128_to_u64(d.as_millis()))
}

fn main() -> Result<()> {
    let socket = default_socket_path();

    // `args_os` + lossy, NOT `args()`: `std::env::args()` PANICS on any
    // non-Unicode argument (legal Unix argv), breaching invariant #5. Lossy
    // rather than filter_map: dropping a non-UTF-8 arg would shift
    // `--source <value>` pairing so the NEXT arg gets read as the value.
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let mut payload: Value = match event_from_argv(&args) {
        // CodeWhale env-mode: identity arrives as `DEEPSEEK_*` env vars with
        // `--event <name>` baked into the registered command. Critically,
        // CodeWhale does NOT pipe stdin for these events, so the hook child
        // inherits the TUI's terminal stdin and a blind `read_to_string` would
        // BLOCK (freezing the synchronous tool call until the hook timeout) —
        // when `--event` is present we never touch stdin.
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
                Err(_) => return Ok(()),
            }
        }
    };

    // Everything past here is OUR work on CC's clock — `cli_pid` walks a process
    // snapshot on Windows — so the bound is armed first (see `arm_watchdog`).
    let Some(bound) = transport::arm_watchdog() else {
        return Ok(());
    };

    if let Value::Object(map) = &mut payload {
        // Source precedence: the `--source <name>` argv flag (the Windows install
        // form) wins over the `PIXTUOID_SOURCE` env var (the Unix env-prefix
        // form; grok delivers the same var via its handler `env` map, so this arm
        // serves both). `--event` is orthogonal and never implies a source.
        let source = source_from_argv(&args).or_else(|| std::env::var("PIXTUOID_SOURCE").ok());
        enrich_payload(map, source, now_ms(), cli_pid);
    }

    // Best-effort send, hard-bounded so a stuck daemon can never block CC's
    // subprocess wait — see transport.rs.
    let mut line = serde_json::to_vec(&payload).unwrap_or_default();
    line.push(b'\n');
    transport::send_line(&bound, &socket, &line);
    Ok(())
}

/// CodeWhale env-mode: synthesize the hook envelope from `DEEPSEEK_*` env vars.
/// The `std::env` reads live here so `env_payload_from` stays testable without
/// mutating process-global env. No `_pid`: `enrich_payload` is the one stamper.
fn env_payload(event: &str) -> serde_json::Map<String, Value> {
    // CodeWhale runs the hook with current_dir = its working dir (= the
    // workspace), so the shim's own cwd is the reliable fallback.
    let cwd_fallback = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    env_payload_from(event, cwd_fallback, |k| std::env::var(k).ok())
}

/// Per-field byte cap on env-mode values. The stdin arm enforces `STDIN_CAP`
/// before parsing; the env arm has no such gate and `DEEPSEEK_TOOL_ARGS` can be
/// large, so capping each folded field keeps the serialized line under the
/// daemon's pipe quota instead of building one the watchdog would drop.
const ENV_FIELD_CAP: usize = 128 * 1024;

/// Byte-bounded, char-SAFE truncation (never split a UTF-8 scalar). The cap is a
/// hard ceiling: a scalar STRADDLING the boundary is dropped, never kept —
/// bounding the char's START would let the result exceed the cap by up to 3
/// bytes.
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
    get: impl Fn(&str) -> Option<String>,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("event".into(), Value::from(event));
    // cwd is the AgentId KEY (the decoder drops a cwd-less event), and
    // DEEPSEEK_WORKSPACE is UNSET for a fresh `codewhale` launched without `-C`
    // until the workspace resolves — so `session_start` would otherwise never
    // register a sprite. The fallback resolves to the same path the workspace
    // eventually does, so a session's events coalesce on one AgentId.
    if let Some(cwd) = get("DEEPSEEK_WORKSPACE")
        .filter(|v| !v.is_empty())
        .or_else(|| cwd_fallback.filter(|v| !v.is_empty()))
    {
        map.insert("cwd".into(), Value::from(cap_env_field(cwd)));
    }
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
/// or `None` if absent or empty.
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

/// CodeWhale's env-mode trigger. Absent → the shim reads its payload from stdin
/// (the CC/Codex/Reasonix path).
fn event_from_argv(args: &[String]) -> Option<String> {
    flag_from_argv(args, "--event")
}

/// The trusted CLI source, in its Windows install form: the hook command runs
/// under `cmd.exe /C`, which has no inline `VAR=value cmd` env-prefix syntax (it
/// would try to exec a program literally named `PIXTUOID_SOURCE=codex`), so the
/// source rides as a flag. Absent → the caller falls back to `PIXTUOID_SOURCE`.
fn source_from_argv(args: &[String]) -> Option<String> {
    flag_from_argv(args, "--source")
}

/// Stamp the shim timestamp and, when a source is resolved, the trusted CLI
/// source under the PRIVATE `_pixtuoid_source` key.
///
/// We deliberately do NOT write the public `source` field: CC's SessionStart
/// payload already uses `source` for the start *reason* (startup/resume/clear/
/// compact), and reading that as the CLI source namespaced the agent under
/// "startup" — an un-reapable ghost. The private key is shim-OWNED, so any
/// inbound `_pixtuoid_source` (spoofed or replayed) is stripped unconditionally
/// before stamping.
fn enrich_payload(
    map: &mut serde_json::Map<String, Value>,
    source: Option<String>,
    ts_ms: u64,
    resolve_pid: impl FnOnce() -> Option<u32>,
) {
    map.remove("_pixtuoid_source");
    map.insert("_shim_ts_ms".into(), Value::from(ts_ms));
    if let Some(src) = source {
        if !src.is_empty() {
            map.insert("_pixtuoid_source".into(), Value::from(src));
        }
    }
    // The opencode/OpenClaw plugins stamp `process.pid` from inside the CLI —
    // keep theirs, and stay LAZY so they never pay for the Windows snapshot.
    if !map.contains_key("_pid") {
        if let Some(pid) = resolve_pid() {
            map.insert("_pid".into(), Value::from(pid));
        }
    }
}

#[cfg(test)]
mod tests;
