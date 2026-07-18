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
    enrich_payload(map, Some("x".repeat(64)), u64::MAX, Some(u32::MAX));
    let stamped = serde_json::to_vec(&p).unwrap();
    // minus the bare `{}` baseline, plus the trailing '\n' main appends.
    let overhead = (stamped.len() - 2 + 1) as u64;
    assert!(
        overhead <= STAMP_HEADROOM,
        "stamps ({overhead}B) must fit within STAMP_HEADROOM ({STAMP_HEADROOM}B)"
    );
}

#[test]
fn stamps_parent_pid_under_pid_when_absent() {
    let mut p = json!({ "hook_event_name": "Stop" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, Some("hermes".into()), 1, Some(4242));
    assert_eq!(map["_pid"], json!(4242u32));
}

#[test]
fn an_upstream_pid_is_kept_never_overwritten() {
    // opencode's plugin / CodeWhale's env-mode supply their own `_pid` —
    // more authoritative than the shim's getppid (which under a plugin
    // runtime may be an intermediary). The shim only fills the gap.
    let mut p = json!({ "_pid": 777 });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, Some("opencode".into()), 1, Some(4242));
    assert_eq!(map["_pid"], json!(777), "upstream value kept");
}

#[test]
fn now_ms_narrowing_saturates_instead_of_wrapping() {
    // Real magnitude: 2024-01-01 in ms passes through unchanged.
    assert_eq!(ms_u128_to_u64(1_704_067_200_000), 1_704_067_200_000);
    assert!(now_ms() > 1_704_067_200_000);
    // TEETH: a value past u64::MAX must SATURATE to u64::MAX. A truncating
    // `as u64` cast would WRAP these to small numbers — this assertion fails
    // the moment `unwrap_or(u64::MAX)` regresses to `as u64`.
    assert_eq!(ms_u128_to_u64(u64::MAX as u128), u64::MAX);
    assert_eq!(ms_u128_to_u64(u64::MAX as u128 + 1), u64::MAX);
    assert_eq!(ms_u128_to_u64(u128::MAX), u64::MAX);
}

#[test]
fn stamps_cli_source_under_private_key_and_leaves_public_source_untouched() {
    // A CC SessionStart payload's `source` is the start *reason* — must survive.
    let mut p = json!({ "hook_event_name": "SessionStart", "source": "startup" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, Some("claude-code".into()), 123, None);
    assert_eq!(map["_pixtuoid_source"], json!("claude-code"));
    assert_eq!(map["source"], json!("startup"), "public reason untouched");
    assert_eq!(map["_shim_ts_ms"], json!(123u64));
}

#[test]
fn no_source_env_omits_private_key_so_decoder_defaults_to_claude() {
    let mut p = json!({ "hook_event_name": "Stop" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, None, 1, None);
    assert!(map.get("_pixtuoid_source").is_none());
}

#[test]
fn empty_source_env_is_ignored() {
    // Seeded with a spoofed inbound key: the empty-source path must strip
    // it too, not just decline to insert.
    let mut p = json!({ "_pixtuoid_source": "codex" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, Some(String::new()), 1, None);
    assert!(map.get("_pixtuoid_source").is_none());
}

#[test]
fn inbound_spoofed_private_key_is_stripped_when_no_source_resolves() {
    // `_pixtuoid_source` is shim-OWNED: the daemon trusts it exclusively
    // for CLI attribution (AgentId namespacing), so a spoofed/replayed
    // inbound key must never pass through on the bare-CC (no source) path.
    let mut p = json!({ "hook_event_name": "Stop", "_pixtuoid_source": "codex" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, None, 1, None);
    assert!(
        map.get("_pixtuoid_source").is_none(),
        "inbound spoofed key must be stripped, not passed through"
    );
}

#[test]
fn inbound_spoofed_private_key_is_overwritten_when_source_resolves() {
    let mut p = json!({ "_pixtuoid_source": "codex" });
    let map = p.as_object_mut().unwrap();
    enrich_payload(map, Some("reasonix".into()), 1, None);
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

#[test]
fn event_from_argv_reads_both_forms_and_rejects_empty() {
    assert_eq!(
        event_from_argv(&argv(&[
            "pixtuoid-hook",
            "--source",
            "codewhale",
            "--event",
            "session_start"
        ])),
        Some("session_start".into())
    );
    assert_eq!(
        event_from_argv(&argv(&["pixtuoid-hook", "--event=tool_call_before"])),
        Some("tool_call_before".into())
    );
    assert_eq!(event_from_argv(&argv(&["pixtuoid-hook"])), None);
    assert_eq!(event_from_argv(&argv(&["pixtuoid-hook", "--event"])), None);
    assert_eq!(
        event_from_argv(&argv(&["pixtuoid-hook", "--event", ""])),
        None
    );
    assert_eq!(event_from_argv(&argv(&["pixtuoid-hook", "--event="])), None);
}

#[test]
fn env_payload_folds_codewhale_env_into_the_envelope() {
    // The live-captured shape: cwd (the AgentId key), tool, tool_args (raw
    // JSON string). Pure getter — no process-global env mutation.
    let env: std::collections::HashMap<&str, &str> = [
        ("DEEPSEEK_WORKSPACE", "/repo"),
        ("DEEPSEEK_TOOL_NAME", "exec_shell"),
        ("DEEPSEEK_TOOL_ARGS", r#"{"command":"ls -la"}"#),
    ]
    .into_iter()
    .collect();
    let map = env_payload_from("tool_call_before", None, Some(4321), |k| {
        env.get(k).map(|s| s.to_string())
    });
    assert_eq!(map["event"], json!("tool_call_before"));
    assert_eq!(map["cwd"], json!("/repo"));
    assert_eq!(map["tool"], json!("exec_shell"));
    assert_eq!(map["tool_args"], json!(r#"{"command":"ls -la"}"#));
    assert_eq!(
        map["_pid"],
        json!(4321),
        "CodeWhale's pid is stamped for the liveness watch"
    );
}

#[test]
fn env_payload_omits_missing_and_empty_env() {
    // session_start carries only DEEPSEEK_WORKSPACE (no tool) — empty/absent
    // tool fields must be omitted, not written as "".
    let env: std::collections::HashMap<&str, &str> =
        [("DEEPSEEK_WORKSPACE", "/repo"), ("DEEPSEEK_TOOL_NAME", "")]
            .into_iter()
            .collect();
    let map = env_payload_from("session_start", None, None, |k| {
        env.get(k).map(|s| s.to_string())
    });
    assert_eq!(map["cwd"], json!("/repo"));
    assert!(
        !map.contains_key("tool"),
        "empty DEEPSEEK_TOOL_NAME must be omitted"
    );
    assert!(
        !map.contains_key("tool_args"),
        "absent tool_args must be omitted"
    );
    assert!(!map.contains_key("_pid"), "no pid → no _pid");
    assert_eq!(map.len(), 2, "exactly event + cwd");
}

#[test]
fn env_payload_caps_oversized_fields_at_a_char_boundary() {
    // A large DEEPSEEK_TOOL_ARGS (e.g. a big write/edit tool's input) must be
    // capped so the serialized line stays under the daemon's 1 MiB pipe quota
    // — extending the stdin arm's STDIN_CAP guarantee to env-mode, so a large
    // tool's tool_call_before still delivers instead of being watchdog-dropped.
    // Multi-byte value: a byte-slice cap would split a UTF-8 scalar.
    let huge = "é".repeat(ENV_FIELD_CAP); // ~2·CAP bytes, well over the cap
    let env: std::collections::HashMap<&str, String> = [
        ("DEEPSEEK_WORKSPACE", "/repo".to_string()),
        ("DEEPSEEK_TOOL_ARGS", huge),
    ]
    .into_iter()
    .collect();
    let map = env_payload_from("tool_call_before", None, None, |k| env.get(k).cloned());
    let args = map["tool_args"].as_str().unwrap();
    assert!(
        args.len() <= ENV_FIELD_CAP,
        "tool_args must be capped to <= {ENV_FIELD_CAP} bytes, got {}",
        args.len()
    );
    assert!(
        args.len() > ENV_FIELD_CAP - 4,
        "cap should truncate NEAR the limit (last char boundary), not collapse"
    );
    assert!(
        args.chars().all(|c| c == 'é'),
        "no mid-scalar split → still valid é runs"
    );
    assert_eq!(
        map["cwd"],
        json!("/repo"),
        "the AgentId key (a real path) is untouched"
    );
}

#[test]
fn cap_never_exceeds_the_bound_when_a_multibyte_scalar_straddles_it() {
    // A 4-byte scalar (U+1D11E) STARTING inside the cap but ENDING past
    // it: the bound is on the char's END, so the straddler is dropped —
    // floor to the previous char boundary, never up to 3 bytes OVER the
    // documented `<= ENV_FIELD_CAP` contract. The é fixture above can't
    // catch this (2-byte chars over an even cap always end exactly ON a
    // boundary), so this pins the straddle case specifically.
    let mut val = "a".repeat(ENV_FIELD_CAP - 1);
    val.push('\u{1D11E}');
    let capped = cap_env_field(val);
    assert!(
        capped.len() <= ENV_FIELD_CAP,
        "cap is a hard byte ceiling, got {} > {ENV_FIELD_CAP}",
        capped.len()
    );
    assert_eq!(
        capped.len(),
        ENV_FIELD_CAP - 1,
        "floor to the last char boundary at or below the cap"
    );
    assert!(capped.chars().all(|c| c == 'a'), "the straddler is dropped");
}

#[test]
fn env_payload_falls_back_to_cwd_when_workspace_unset() {
    // A fresh `codewhale` without `-C` has no DEEPSEEK_WORKSPACE at
    // session_start, so a cwd-less envelope would be dropped (no sprite).
    // CodeWhale runs the hook with current_dir = its working dir, so the
    // shim must fall back to that.
    let no_ws: std::collections::HashMap<&str, String> =
        [("DEEPSEEK_TOOL_NAME", "exec_shell".to_string())]
            .into_iter()
            .collect();
    let map = env_payload_from("session_start", Some("/proj/here".to_string()), None, |k| {
        no_ws.get(k).cloned()
    });
    assert_eq!(
        map["cwd"],
        json!("/proj/here"),
        "cwd must fall back to the hook child's working dir when DEEPSEEK_WORKSPACE is unset"
    );

    // DEEPSEEK_WORKSPACE remains authoritative over the fallback when present.
    let ws: std::collections::HashMap<&str, String> = [("DEEPSEEK_WORKSPACE", "/ws".to_string())]
        .into_iter()
        .collect();
    let map = env_payload_from("session_start", Some("/proj/here".to_string()), None, |k| {
        ws.get(k).cloned()
    });
    assert_eq!(
        map["cwd"],
        json!("/ws"),
        "DEEPSEEK_WORKSPACE wins over the fallback"
    );

    // Neither present → no cwd (the decoder drops it; nothing to key on).
    let map = env_payload_from("session_start", None, None, |_| None);
    assert!(
        !map.contains_key("cwd"),
        "no workspace and no cwd fallback → no cwd field"
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

    // Arm 1b: set-but-empty/whitespace PIXTUOID_SOCKET = unset (the #172
    // RUST_LOG policy) -> falls through to XDG.
    std::env::set_var("PIXTUOID_SOCKET", "");
    std::env::set_var("XDG_RUNTIME_DIR", "/run/user/0");
    assert_eq!(default_socket_path(), "/run/user/0/pixtuoid.sock");
    std::env::set_var("PIXTUOID_SOCKET", "   ");
    assert_eq!(default_socket_path(), "/run/user/0/pixtuoid.sock");

    // Arm 2: no PIXTUOID_SOCKET, XDG_RUNTIME_DIR set -> "{dir}/pixtuoid.sock".
    std::env::remove_var("PIXTUOID_SOCKET");
    std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
    assert_eq!(default_socket_path(), "/run/user/1000/pixtuoid.sock");

    // Arm 2b: invalid (empty/whitespace/relative) XDG_RUNTIME_DIR is unset
    // per the XDG absolute-only spec -> /tmp subdir (parity with native.rs).
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    let tmp_fallback = format!("/tmp/pixtuoid-{uid}/pixtuoid.sock");
    for invalid in ["", "   ", "relative/run"] {
        std::env::set_var("XDG_RUNTIME_DIR", invalid);
        assert_eq!(default_socket_path(), tmp_fallback);
    }

    // Arm 3: neither set -> "/tmp/pixtuoid-{uid}/pixtuoid.sock" (#485: the
    // per-user 0700 SUBDIR, not a flat squattable name).
    std::env::remove_var("PIXTUOID_SOCKET");
    std::env::remove_var("XDG_RUNTIME_DIR");
    assert_eq!(
        default_socket_path(),
        format!("/tmp/pixtuoid-{uid}/pixtuoid.sock")
    );

    match prior_socket {
        Some(v) => std::env::set_var("PIXTUOID_SOCKET", v),
        None => std::env::remove_var("PIXTUOID_SOCKET"),
    }
    match prior_xdg {
        Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
        None => std::env::remove_var("XDG_RUNTIME_DIR"),
    }
}

// #485: the shim only validates dir ownership for the `/tmp` fallback it
// owns — never for an XDG or explicit-override endpoint (someone else's dir).
#[cfg(unix)]
#[test]
fn owned_tmp_socket_dir_matches_only_the_tmp_fallback() {
    // Safety: getuid is always safe on Unix.
    let uid = unsafe { libc::getuid() };
    let owned = std::path::PathBuf::from(format!("/tmp/pixtuoid-{uid}"));
    assert_eq!(
        paths::owned_tmp_socket_dir(&format!("/tmp/pixtuoid-{uid}/pixtuoid.sock")),
        Some(owned),
        "the /tmp fallback endpoint resolves its owned dir"
    );
    assert_eq!(
        paths::owned_tmp_socket_dir("/run/user/1000/pixtuoid.sock"),
        None,
        "XDG_RUNTIME_DIR is systemd's, not ours to police"
    );
    assert_eq!(
        paths::owned_tmp_socket_dir("/explicit/path.sock"),
        None,
        "an explicit PIXTUOID_SOCKET override is the user's, not ours"
    );
    // A different uid's tmp dir is NOT ours either (belt: parent must match).
    assert_eq!(
        paths::owned_tmp_socket_dir(&format!("/tmp/pixtuoid-{}/pixtuoid.sock", uid + 1)),
        None
    );
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

    // Set-but-empty/whitespace = unset (the #172 RUST_LOG policy) ->
    // USERNAME default.
    std::env::set_var("PIXTUOID_SOCKET", "");
    std::env::set_var("USERNAME", "ada");
    assert_eq!(default_socket_path(), r"\\.\pipe\pixtuoid-ada");
    std::env::set_var("PIXTUOID_SOCKET", "   ");
    assert_eq!(default_socket_path(), r"\\.\pipe\pixtuoid-ada");

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
