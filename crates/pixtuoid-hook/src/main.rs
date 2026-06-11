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

/// Explicit `u128 → u64` narrowing (`try_from`, not a truncating `as` cast):
/// ms-since-epoch fits u64 for ~580M years, so the `unwrap_or(u64::MAX)` arm
/// is unreachable in practice — it exists to make the narrowing visible and
/// the fn total. A pre-epoch clock maps to 0 (same as before).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn main() -> Result<()> {
    let socket = default_socket_path();

    let mut buf = String::new();
    if std::io::stdin()
        .take(STDIN_CAP)
        .read_to_string(&mut buf)
        .is_err()
    {
        return Ok(());
    }
    let mut payload: Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        // If we can't parse, exit 0 silently so CC isn't blocked.
        Err(_) => return Ok(()),
    };

    if let Value::Object(map) = &mut payload {
        // Source precedence: the `--source <name>` argv flag (the Windows install
        // form — cmd.exe /C can't express a POSIX `VAR=value cmd` env-prefix) wins,
        // then the `PIXTUOID_SOURCE` env var (the Unix install form). Either way the
        // daemon only ever sees the resulting `_pixtuoid_source` stamp.
        //
        // `args_os` + lossy, NOT `args()`: `std::env::args()` PANICS on any
        // non-Unicode argument (legal Unix argv), breaching invariant #5's
        // silent exit-0. Lossy rather than filter_map: dropping a non-UTF-8
        // arg would shift `--source <value>` pairing so the NEXT arg gets
        // read as the value; lossy preserves arity, and a U+FFFD-mangled
        // value simply fails the daemon's registry lookup downstream.
        let args: Vec<String> = std::env::args_os()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let source = source_from_argv(&args).or_else(|| std::env::var("PIXTUOID_SOURCE").ok());
        enrich_payload(map, source, now_ms());
    }

    // Best-effort send, hard-bounded so a stuck daemon can never block CC's
    // subprocess wait — see transport.rs for the per-platform mechanism.
    let mut line = serde_json::to_vec(&payload).unwrap_or_default();
    line.push(b'\n');
    transport::send_line(&socket, &line);
    Ok(())
}

/// The trusted CLI source from `--source <name>` (or `--source=<name>`) in argv.
/// This is the Windows install form: the codex hook command runs under `cmd.exe
/// /C`, which has no inline `VAR=value cmd` env-prefix syntax (it would try to exec
/// a program literally named `PIXTUOID_SOURCE=codex`), so the source rides as a
/// flag instead. Absent or empty → `None` so the caller falls back to the
/// `PIXTUOID_SOURCE` env var (the unchanged Unix install form). Total + panic-free
/// per invariant #5 (the shim must never block CC).
fn source_from_argv(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(val) = arg.strip_prefix("--source=") {
            return Some(val).filter(|s| !s.is_empty()).map(str::to_string);
        }
        if arg == "--source" {
            return it.next().filter(|s| !s.is_empty()).cloned();
        }
    }
    None
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
fn enrich_payload(map: &mut serde_json::Map<String, Value>, source: Option<String>, ts_ms: u64) {
    map.remove("_pixtuoid_source");
    map.insert("_shim_ts_ms".into(), Value::from(ts_ms));
    if let Some(src) = source {
        if !src.is_empty() {
            map.insert("_pixtuoid_source".into(), Value::from(src));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stdin_cap_plus_headroom_equals_the_pipe_quota() {
        // The daemon's Windows pipe in-buffer (hook/windows.rs IN_BUFFER_SIZE)
        // is 1 MiB; the wire line is capped stdin + stamps + newline. Pin the
        // arithmetic the "one payload always fits the quota" claim rests on.
        assert_eq!(STDIN_CAP + STAMP_HEADROOM, 1 << 20);
    }

    #[test]
    fn stamp_headroom_covers_worst_case_stamps() {
        let mut p = json!({});
        let map = p.as_object_mut().unwrap();
        // Worst realistic stamps: a 20-digit u64::MAX timestamp + a source
        // name far longer than any registered CLI name (claude-code / codex /
        // reasonix / antigravity are all ≤ 11 chars; allow 64 for custom ones).
        enrich_payload(map, Some("x".repeat(64)), u64::MAX);
        let stamped = serde_json::to_vec(&p).unwrap();
        // minus the bare `{}` baseline, plus the trailing '\n' main appends.
        let overhead = (stamped.len() - 2 + 1) as u64;
        assert!(
            overhead <= STAMP_HEADROOM,
            "stamps ({overhead}B) must fit within STAMP_HEADROOM ({STAMP_HEADROOM}B)"
        );
    }

    #[test]
    fn now_ms_keeps_real_magnitude() {
        // 2024-01-01 in ms — a truncating-narrowing bug would land far below.
        assert!(now_ms() > 1_704_067_200_000);
    }

    #[test]
    fn stamps_cli_source_under_private_key_and_leaves_public_source_untouched() {
        // A CC SessionStart payload's `source` is the start *reason* — must survive.
        let mut p = json!({ "hook_event_name": "SessionStart", "source": "startup" });
        let map = p.as_object_mut().unwrap();
        enrich_payload(map, Some("claude-code".into()), 123);
        assert_eq!(map["_pixtuoid_source"], json!("claude-code"));
        assert_eq!(map["source"], json!("startup"), "public reason untouched");
        assert_eq!(map["_shim_ts_ms"], json!(123u64));
    }

    #[test]
    fn no_source_env_omits_private_key_so_decoder_defaults_to_claude() {
        let mut p = json!({ "hook_event_name": "Stop" });
        let map = p.as_object_mut().unwrap();
        enrich_payload(map, None, 1);
        assert!(map.get("_pixtuoid_source").is_none());
    }

    #[test]
    fn empty_source_env_is_ignored() {
        // Seeded with a spoofed inbound key: the empty-source path must strip
        // it too, not just decline to insert.
        let mut p = json!({ "_pixtuoid_source": "codex" });
        let map = p.as_object_mut().unwrap();
        enrich_payload(map, Some(String::new()), 1);
        assert!(map.get("_pixtuoid_source").is_none());
    }

    #[test]
    fn inbound_spoofed_private_key_is_stripped_when_no_source_resolves() {
        // `_pixtuoid_source` is shim-OWNED: the daemon trusts it exclusively
        // for CLI attribution (AgentId namespacing), so a spoofed/replayed
        // inbound key must never pass through on the bare-CC (no source) path.
        let mut p = json!({ "hook_event_name": "Stop", "_pixtuoid_source": "codex" });
        let map = p.as_object_mut().unwrap();
        enrich_payload(map, None, 1);
        assert!(
            map.get("_pixtuoid_source").is_none(),
            "inbound spoofed key must be stripped, not passed through"
        );
    }

    #[test]
    fn inbound_spoofed_private_key_is_overwritten_when_source_resolves() {
        let mut p = json!({ "_pixtuoid_source": "codex" });
        let map = p.as_object_mut().unwrap();
        enrich_payload(map, Some("reasonix".into()), 1);
        assert_eq!(map["_pixtuoid_source"], json!("reasonix"));
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn source_from_argv_reads_space_form() {
        assert_eq!(
            source_from_argv(&argv(&["pixtuoid-hook", "--source", "codex"])),
            Some("codex".into())
        );
    }

    #[test]
    fn source_from_argv_reads_equals_form() {
        assert_eq!(
            source_from_argv(&argv(&["pixtuoid-hook", "--source=codex"])),
            Some("codex".into())
        );
    }

    #[test]
    fn source_from_argv_absent_is_none() {
        assert_eq!(source_from_argv(&argv(&["pixtuoid-hook"])), None);
    }

    #[test]
    fn source_from_argv_missing_value_is_none() {
        // `--source` as the final arg → no value → None (env fallback).
        assert_eq!(
            source_from_argv(&argv(&["pixtuoid-hook", "--source"])),
            None
        );
    }

    #[test]
    fn source_from_argv_empty_value_is_none() {
        assert_eq!(
            source_from_argv(&argv(&["pixtuoid-hook", "--source", ""])),
            None
        );
        assert_eq!(
            source_from_argv(&argv(&["pixtuoid-hook", "--source="])),
            None
        );
    }

    // Env vars are process-global. This is the ONLY env-touching test in this
    // crate (the integration suite in tests/shim.rs runs in a separate binary
    // and sets PIXTUOID_SOCKET in the spawned child, not in-process), so it can
    // save/restore both vars and drive all three branches without serial_test.
    #[cfg(unix)]
    #[test]
    fn default_socket_path_branches() {
        let prior_socket = std::env::var("PIXTUOID_SOCKET").ok();
        let prior_xdg = std::env::var("XDG_RUNTIME_DIR").ok();

        // Arm 1: PIXTUOID_SOCKET set -> returned verbatim, wins over XDG.
        std::env::set_var("PIXTUOID_SOCKET", "/explicit/path.sock");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/0");
        assert_eq!(default_socket_path(), "/explicit/path.sock");

        // Arm 2: no PIXTUOID_SOCKET, XDG_RUNTIME_DIR set -> "{dir}/pixtuoid.sock".
        std::env::remove_var("PIXTUOID_SOCKET");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(default_socket_path(), "/run/user/1000/pixtuoid.sock");

        // Arm 3: neither set -> "/tmp/pixtuoid-{uid}.sock".
        std::env::remove_var("PIXTUOID_SOCKET");
        std::env::remove_var("XDG_RUNTIME_DIR");
        // Safety: getuid is always safe on Unix.
        let uid = unsafe { libc::getuid() };
        assert_eq!(default_socket_path(), format!("/tmp/pixtuoid-{uid}.sock"));

        match prior_socket {
            Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
            None => std::env::remove_var("PIXTUOID_SOCKET"),
        }
        match prior_xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    // The Windows twin only RUNS on a Windows runner (PR 3 turns that CI
    // job on); until then the ubuntu cross-check job keeps it compiling.
    #[cfg(windows)]
    #[test]
    fn default_socket_path_branches_windows() {
        let prior_socket = std::env::var("PIXTUOID_SOCKET").ok();
        let prior_user = std::env::var("USERNAME").ok();

        std::env::set_var("PIXTUOID_SOCKET", r"\\.\pipe\explicit");
        assert_eq!(default_socket_path(), r"\\.\pipe\explicit");

        std::env::remove_var("PIXTUOID_SOCKET");
        std::env::set_var("USERNAME", "ada");
        assert_eq!(default_socket_path(), r"\\.\pipe\pixtuoid-ada");

        // DOMAIN\user form is sanitized (backslashes are illegal in pipe names).
        std::env::set_var("USERNAME", r"CORP\alice");
        assert_eq!(default_socket_path(), r"\\.\pipe\pixtuoid-CORP-alice");

        std::env::remove_var("USERNAME");
        assert_eq!(default_socket_path(), r"\\.\pipe\pixtuoid-default");

        match prior_socket {
            Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
            None => std::env::remove_var("PIXTUOID_SOCKET"),
        }
        match prior_user {
            Some(v) => std::env::set_var("USERNAME", v),
            None => std::env::remove_var("USERNAME"),
        }
    }
}
