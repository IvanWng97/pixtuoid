//! Oh My Pi (`omp`, omp.sh) source. Watches the omp session transcripts
//! (`<omp_agent_dir>/sessions/<encoded-cwd>/<ts>_<uuid>.jsonl`) via
//! `JsonlWatcher`. TRANSCRIPT-ONLY (Copilot/Antigravity-class): omp has NO
//! shell-hook seam (its "hooks" are in-process TS extension modules —
//! upstream `docs/hooks.md`, `extensibility/extensions/loader.ts` filters
//! discovered hook files to `.ts`/`.js`), so there is no install target; the
//! Sources panel shows `om·` as a no-target flag-flip row.
//!
//! Grounded in the upstream source @ v16.3.12 (`can1357/oh-my-pi`, commit
//! ff1fe5f) — pin comments below cite the upstream files —
//! and byte-real anchored against a live omp 16.4.0 install:
//! the conformance fixtures under `tests/sources/fixtures/omp/` are
//! sanitized captures (a real `omp -p` run and a real `task`-subagent child
//! file at 16.4.0; a real interactive ask round), and
//! the registry row's `verified_version` is that install's `omp --version`.
//!
//! Wire shape (upstream `packages/coding-agent/src/session/`):
//! - **File**: `${fileSafeTimestamp}_${uuidv7}.jsonl` (`session-manager.ts`),
//!   under a per-cwd encoded dir that always starts with `-`
//!   (`session-paths.ts`). Line 1 is a fixed-width 256-byte `type:"title"`
//!   slot rewritten IN PLACE on rename (pwrite at offset 0 — never re-decoded
//!   by a tail cursor past it); line 2 the `type:"session"` header (id, cwd);
//!   legacy files lack the slot (header first). Entries append via a
//!   kept-open `O_APPEND` fd.
//! - **Turn**: `type:"message"` entries — `role:"assistant"` content carries
//!   `type:"toolCall"` blocks (`{id,name,arguments}`, `pi-ai` types.ts);
//!   `role:"toolResult"` closes one (`toolCallId`). A `custom` entry
//!   `customType:"tool_execution_start"` duplicates each toolCall right
//!   before execution — deliberately NOT decoded (same `tool_use_id` would
//!   double-count `tool_call_count`).
//! - **Exit**: a `custom` entry `customType:"session_exit"` is appended +
//!   flushed on every clean teardown incl. SIGINT/SIGTERM
//!   (`agent-session.ts::#recordSessionExit`, `exit-diagnostics.ts`) — the
//!   session-ended marker. Skipped when the session never produced an
//!   assistant message; SIGKILL writes nothing → stale-sweep.
//! - **Subagents**: the `task` tool persists each child as a SEPARATE file
//!   `<parent-path-minus-.jsonl>/<taskId>.jsonl` (`task/executor.ts`),
//!   recursively — linkage is the PATH NESTING, not a header field (the
//!   child header has no `parentSession`). `omp_id_from_path` keys the whole
//!   chain; the header decode re-emits a parented SessionStart which enriches
//!   the watcher's parentless first-sight registration.
//!
//! Sharp edges:
//! - fork / branch / version-migration / tool-output pruning REWRITE the file
//!   atomically (temp + rename → new inode); the watcher re-stats by path, so
//!   a rewrite reads as a fresh transcript. Rare enough to live with.
//! - The title-slot pwrite mutates line 1 of an already-cursored file; the
//!   cursor sits past byte 256, so it is never re-read (and a `type:"title"`
//!   line decodes to nothing anyway).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::source::decoder::{ellipsize, MAX_DECODED_FIELD_CHARS};
use crate::source::{AgentEvent, ToolDetail};
use crate::AgentId;

// The runtime half (`OmpSource` + its watcher wiring, the first-sight
// session-ended checker, and the open-write-fd liveness probe) — ONE gate for
// the whole `native` layer of this source; the re-export keeps
// `source::omp::OmpSource` public. The probe fn stays module-private (no
// focus point-query consumer — see its doc comment).
#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::OmpSource;

pub const SOURCE_NAME: &str = "omp";

/// omp's agent config dir: `$PI_CODING_AGENT_DIR` if set non-empty (omp's own
/// relocation knob — upstream `packages/utils/src/dirs.ts` DirResolver), else
/// `~/.omp/agent`. omp resolves home via Node's `os.homedir()` (USERPROFILE
/// on Windows) → `user_home()`. The XDG split ($XDG_DATA_HOME/omp) engages
/// only after an explicit `omp config init-xdg` migration, and `OMP_PROFILE`
/// relocates to `~/.omp/profiles/<name>/agent` — both deliberately unmirrored
/// (opt-in minority setups; the watcher just sees an empty default dir).
pub fn omp_agent_dir() -> PathBuf {
    match crate::platform::nonempty(std::env::var("PI_CODING_AGENT_DIR").ok()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(crate::platform::user_home())
            .join(".omp")
            .join("agent"),
    }
}

/// Does a path component look like a ROOT session file stem —
/// `${fileSafeTimestamp}_${uuid}` (ISO date prefix + the `_` separator,
/// upstream `session-manager.ts::fileSafeTimestamp`)? Subagent stems are task
/// ids (`Alpha`, `GoodWolf`) and never date-shaped. The `T` check is
/// case-insensitive: on Windows the per-line decoder receives the
/// `normalize_path_key`'d path (LOWERCASED), so the on-disk `T` arrives as
/// `t` — rejecting it broke the whole stem chain there (windows-test caught
/// it; CC dodges the fold only because its UUIDs are already lowercase).
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
/// PURE and case-preserving: raw in → raw out on every platform (fixture-fed
/// conformance goldens stay platform-invariant). The Windows case-fold
/// happens at the WATCHER seam instead — walk.rs normalizes the path before
/// BOTH the id-deriver and the per-line decoder, and the probe folds at its
/// own boundary — so all runtime lanes mint ONE (folded) id per file there
/// while this fn stays deterministic over whatever form it is given. The
/// only fold-awareness HERE is `looks_like_session_stem`'s case-insensitive
/// `T`, so an already-folded path still parses as a chain.
fn stem_chain(path: &Path) -> Vec<String> {
    let own = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut chain = vec![own];
    let mut cur = path.parent();
    // A root file's ancestors are the per-cwd encoded dirs (never
    // date-shaped); a subagent file's ancestor chain ends at the root
    // session's artifacts dir, whose name IS the root stem.
    while let Some(dir) = cur {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            break;
        };
        if looks_like_session_stem(name) {
            chain.push(name.to_string());
            break;
        }
        // Bound the climb at omp's own layout boundaries: the per-cwd encoded
        // dir (always `-`-prefixed — session-paths.ts, all three branches) or
        // the `sessions` root itself. Without this the walk continues into the
        // USER'S path above the watched root, where a date-shaped component
        // (`~/backups/2026-…_snap/agent/…`) would misclassify every root
        // transcript as a subagent chain.
        if name == "sessions" || name.starts_with('-') {
            break;
        }
        // Only intermediate SUBAGENT dirs are collected — but we can't tell a
        // task-id dir from a foreign dir until we see the root stem above it,
        // so collect speculatively and discard below if no root was found.
        chain.push(name.to_string());
        cur = dir.parent();
    }
    if chain.last().is_some_and(|top| looks_like_session_stem(top)) {
        chain.reverse();
        chain
    } else {
        // No root-stem ancestor → this IS a root transcript (or a foreign
        // layout); key on the file stem alone.
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

/// The parent's key (the chain minus the last segment) for a subagent
/// transcript; `None` for a root.
pub(crate) fn omp_parent_key_from_path(path: &Path) -> Option<String> {
    let chain = stem_chain(path);
    (chain.len() > 1).then(|| chain[..chain.len() - 1].join("/"))
}

/// Decode one omp session JSONL line into zero or more `AgentEvent`s.
/// Unknown entry types / roles and malformed shapes return `vec![]` — the
/// upstream reference loader is itself lenient (`parseJsonlLenient`), so a
/// defensive skip mirrors the CLI's own posture.
pub fn decode_omp_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let path = Path::new(transcript_path);
    let acting = AgentId::from_parts(source, &omp_id_from_path(path));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let kind = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    let out = match kind {
        // The header (line 2, after the title slot). session_id = the SAME
        // id-deriver key as the watcher's first-sight registration, so the
        // two SessionStarts agree (`emit_first_sight` uses `id_derive`); the
        // header's own uuid is embedded in that key anyway (the file stem).
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
                // Assistant content blocks carry the tool CALLS.
                Some("assistant") => {
                    let mut out = Vec::new();
                    // Model identity rides EVERY assistant message (pi-ai
                    // types.ts: AssistantMessage requires provider+model) —
                    // the burn-tier carrier (#545). The BARE `model` field,
                    // never the provider-prefixed `model_change` form, so
                    // TOP_MODELS prefix matching sees the same vocabulary
                    // CC/codex/copilot emit; the reducer's last-seen-wins
                    // dedups the per-turn re-stamp.
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
                    // Token-meter usage observation (#632): assistant messages
                    // carry per-turn `usage`. omp's `input` EXCLUDES the cache
                    // share (fixture-verified: totalTokens = input + output +
                    // cacheRead + cacheWrite), so fresh = input + cacheWrite +
                    // output — cache READS are re-served context, excluded
                    // like CC's. Zero readings skipped.
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
                    // A blocks-less/text-only turn still stamps the model.
                    let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) else {
                        return Ok(out);
                    };
                    // `ask` (the built-in user-question tool) BLOCKS on human
                    // input: its Start is followed by a Waiting so the session
                    // renders waiting, not active. The Start binds the
                    // reducer's `gated_before_waiting` gate to the ask's own
                    // tool_use_id, so the answer's toolResult (ActivityEnd,
                    // same id) resolves the Wait — no separate clearing event.
                    // Ask pairs are collected separately and appended LAST:
                    // when an ask is batched with parallel toolCalls, a
                    // sibling's later ActivityStart would flip the slot back
                    // to Active and drop the gate, so the answered ask could
                    // never resolve the Wait.
                    let mut asks = Vec::new();
                    for b in blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("toolCall"))
                    {
                        // Un-keyable calls are dropped (can't be closed) — but
                        // breadcrumb it: `id` is a REQUIRED pairing key on a
                        // toolCall block we're committed to decoding, so its
                        // absence is upstream drift, not a line we ignore.
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
                // A tool result closes its call.
                Some("toolResult") => {
                    let Some(tool_call_id) = msg.get("toolCallId").and_then(|i| i.as_str()) else {
                        // The ActivityEnd pairing key — its absence is drift on a
                        // lifecycle event we're committed to decoding (mirror of the
                        // toolCall `id` gate above), and an unkeyable End can never
                        // close its Start (leaks Active forever). Breadcrumb, then drop.
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
        // Clean teardown marker (`exit-diagnostics.ts`): reason/kind ignored —
        // every kind ("normal"|"signal"|"fatal"|"process_exit") IS an end.
        "custom" if obj.get("customType").and_then(|c| c.as_str()) == Some("session_exit") => {
            vec![AgentEvent::SessionEnd {
                agent_id: acting,
                as_child: omp_parent_key_from_path(path).is_some(),
            }]
        }
        // title / title_change / model_change / compaction / session_init /
        // custom_message / thinking_level_change / … — not sprite-visible.
        // (model_change stays undecoded even though the burn tier reads model:
        // its value is the provider-prefixed combined form, and every assistant
        // message re-stamps the bare `model` anyway — one turn's lag at most.)
        _ => vec![],
    };
    Ok(out)
}

/// omp's tool-detail dispatch. The subagent dispatch is the `task` tool,
/// detected by NAME only (the Copilot/Reasonix spoof guard: `arguments` are
/// model-authored, so a hallucinated `subagent_type` key must not flip an
/// ordinary tool to Delegating). omp's builtin arg vocabulary keys targets
/// under these names (bash→command, read/edit/write→path, grep/glob→pattern,
/// web_search→query — upstream `src/tools/`).
fn omp_tool_detail(tool: &str, args: Option<&Value>) -> ToolDetail {
    if tool == "task" {
        return ToolDetail::Task;
    }
    const KEYS: &[&str] = &["command", "path", "pattern", "query"];
    crate::source::decoder::generic_keyed_detail(tool, args, KEYS)
}

/// The Waiting reason for an `ask` round: the first question's text
/// (`arguments.questions[0].question` — the ask schema requires one), falling
/// back to the call's intent (`arguments.i`), then the bare tool name. Capped
/// at the decode boundary like every other content-derived Waiting reason
/// (copilot/opencode/reasonix) — the text is model-authored wire content and
/// persists in the slot + headless summary.
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

    // Root session transcript path, real on-disk shape:
    // <sessions>/<encoded-cwd>/<fileSafeTimestamp>_<uuidv7>.jsonl
    const ROOT: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001.jsonl";
    const ROOT_KEY: &str = "2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001";
    // Subagent: <parent-path-minus-.jsonl>/<taskId>.jsonl (task/executor.ts).
    const CHILD: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001/Alpha.jsonl";
    const GRANDCHILD: &str = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T08-00-00-000Z_0197f0aa-0000-7000-8000-000000000001/Alpha/GoodWolf.jsonl";

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

    /// The per-cwd encoded dirs (`-dev-proj`, `-tmp-x`, legacy `--abs--`) are
    /// never date-shaped, so a root file keys on its own stem even though the
    /// chain walk climbs through them; a same-named task id under TWO
    /// different sessions never collides (the chain is session-prefixed).
    #[test]
    fn same_task_id_under_two_sessions_keys_distinctly() {
        let other = "/home/u/.omp/agent/sessions/-dev-proj/2026-07-09T09-00-00-000Z_0197f0bb-0000-7000-8000-000000000002/Alpha.jsonl";
        assert_ne!(
            omp_id_from_path(Path::new(CHILD)),
            omp_id_from_path(Path::new(other))
        );
    }

    /// The Windows decoder lane: walk.rs hands every derivation the
    /// `normalize_path_key`'d path (LOWERCASED, forward-slashed there). The
    /// deriver is pure, so the folded form must still parse as a stem chain
    /// (the case-insensitive `T`) — pure string code, so the Windows arm is
    /// pinned on every platform.
    #[test]
    fn stem_chain_survives_the_windows_case_fold() {
        // The decoder-lane shape on Windows: already lowercased.
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

    /// The climb is BOUNDED at omp's layout boundaries (the `-`-prefixed
    /// per-cwd dir / the `sessions` root): a date-shaped component in the
    /// USER'S path above the watched root must not turn every root transcript
    /// into a phantom subagent chain.
    #[test]
    fn date_shaped_dirs_above_the_sessions_root_do_not_misclassify() {
        let p = format!(
            "/home/u/backups/2026-01-01T00-00-00-000Z_snap/agent/sessions/-dev-proj/{stem}.jsonl",
            stem = ROOT_KEY
        );
        assert_eq!(omp_id_from_path(Path::new(&p)), ROOT_KEY);
        assert_eq!(omp_parent_key_from_path(Path::new(&p)), None);
    }

    // ── header / lifecycle ──

    #[test]
    fn session_header_registers_root_with_cwd_and_no_parent() {
        // v3 header shape (session-entries.ts SessionHeader).
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
        // exit-diagnostics.ts SessionExitData; every kind is an end.
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

    // ── tool rounds ──

    #[test]
    fn assistant_usage_becomes_a_fresh_token_observation() {
        // Fixture-verified shape (16.4.0): totalTokens = input + output +
        // cacheRead + cacheWrite, so `input` EXCLUDES cache — fresh =
        // input + cacheWrite + output = 122 + 1000 + 1491 = 2613.
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
        // A zero reading stays silent.
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
        // AssistantMessage content with a toolCall block (pi-ai types.ts).
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
        // Every assistant message carries the BARE `model` (+ a separate
        // `provider`) — pi-ai types.ts requires both. The bare field is the
        // burn-tier carrier (#545): the same shape CC/codex/copilot emit, so
        // TOP_MODELS prefix matching sees one vocabulary (the
        // provider-prefixed `model_change` form is deliberately NOT decoded);
        // the reducer's last-seen-wins dedups the per-turn re-stamp.
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
        // A text-only assistant turn still stamps the model.
        let text_only = r#"{"type":"message","id":"m2","parentId":null,"timestamp":"t","message":{"role":"assistant","provider":"anthropic","model":"claude-fable-5","content":[{"type":"text","text":"done"}],"timestamp":2}}"#;
        match &decode(text_only)[..] {
            [AgentEvent::ModelInfo { model: Some(m), .. }] => {
                assert_eq!(m.as_str(), "claude-fable-5");
            }
            other => panic!("expected one ModelInfo, got {other:?}"),
        }
        // An empty/missing model must not mint a phantom observation.
        let empty = r#"{"type":"message","id":"m3","timestamp":"t","message":{"role":"assistant","model":"","content":[],"timestamp":3}}"#;
        assert!(decode(empty).is_empty());
        // A content-ABSENT message (defensive: pi-ai types.ts requires
        // `content`, so this can't occur on real wire) still stamps the model
        // — pins the let-else early return's `Ok(out)`, not `Ok(vec![])`.
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
        // arguments are model-authored; only the `task` NAME delegates (the
        // Copilot/Reasonix spoof-vector guard).
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

    // ── ask (user-question gate) ──

    #[test]
    fn ask_call_starts_activity_then_waits_on_the_question() {
        // Byte-real shape (captured omp ask round): arguments carry an intent
        // `i` + a `questions` array. The ORDER is load-bearing: the Start
        // (applied first) makes the slot Active on the ask's tool_use_id, so
        // the reducer's `gated_before_waiting` binds to it and the answer's
        // toolResult (ActivityEnd, same id) resolves the Wait.
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
        // No questions array → the call's intent `i`.
        let intent = r#"{"type":"message","id":"m8","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK2","name":"ask","arguments":{"i":"Confirming scope"}}],"timestamp":1}}"#;
        match &decode(intent)[..] {
            [_, AgentEvent::Waiting { reason, .. }] => assert_eq!(reason, "Confirming scope"),
            other => panic!("expected Start+Waiting, got {other:?}"),
        }
        // No arguments at all → the bare tool name.
        let bare = r#"{"type":"message","id":"m9","parentId":null,"timestamp":"t","message":{"role":"assistant","content":[{"type":"toolCall","id":"tool_ASK3","name":"ask"}],"timestamp":1}}"#;
        match &decode(bare)[..] {
            [_, AgentEvent::Waiting { reason, .. }] => assert_eq!(reason, "ask"),
            other => panic!("expected Start+Waiting, got {other:?}"),
        }
    }

    #[test]
    fn ask_batched_with_parallel_tool_calls_decodes_last() {
        // An ask batched BEFORE a sibling toolCall must still decode after
        // it: a sibling's later ActivityStart would flip the slot back to
        // Active and drop the `gated_before_waiting` gate, so the answered
        // ask could never resolve the Wait.
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
        // The question is model-authored wire content and persists in the
        // slot — cap where it enters, like every content-derived reason.
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
        // No block id → un-keyable (its result could never close it) → drop,
        // but the drop must leave a `missing_field` breadcrumb (`id` is a
        // REQUIRED pairing key on a block we're committed to decoding).
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
        // Missing name → drift breadcrumb + empty name; "" is not "task".
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

    /// A `toolResult` missing its `toolCallId` (the ActivityEnd pairing key) is
    /// dropped — an unkeyable End can't close its Start — but must leave a
    /// `missing_field` breadcrumb, the mirror of the `toolCall` `id` gate above.
    /// It is NOT an ignorable line (it's a lifecycle event we decode), so it does
    /// not belong in the "ignored, not panicked" bundle.
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
        // Duplicates the assistant toolCall block with the SAME toolCallId
        // (exit-diagnostics.ts) — decoding both would double-count.
        let line = r#"{"type":"custom","id":"c1","parentId":null,"timestamp":"t","customType":"tool_execution_start","data":{"toolCallId":"toolu_01AAA","toolName":"read","startedAt":"t"}}"#;
        assert!(decode(line).is_empty());
    }

    #[test]
    fn non_lifecycle_entries_and_malformed_lines_are_ignored_not_panicked() {
        for line in [
            // The 256-byte title slot (line 1 of every v3 file).
            r#"{"type":"title","v":1,"title":"Fix flaky test","source":"auto","updatedAt":"t","pad":"   "}"#,
            r#"{"type":"title_change","id":"x","parentId":null,"timestamp":"t","title":"New","source":"user"}"#,
            r#"{"type":"model_change","id":"x","parentId":null,"timestamp":"t","model":"anthropic/claude-opus-4-5"}"#,
            r#"{"type":"compaction","id":"x","parentId":null,"timestamp":"t","summary":"…","firstKeptEntryId":"y","tokensBefore":1}"#,
            r#"{"type":"session_init","id":"x","parentId":null,"timestamp":"t","systemPrompt":"…","task":"…","tools":[]}"#,
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t","message":{"role":"user","content":"hi","timestamp":1}}"#,
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t","message":{"role":"bashExecution","command":"ls","output":"","exitCode":0,"timestamp":1}}"#,
            // message entry with no message object.
            r#"{"type":"message","id":"x","parentId":null,"timestamp":"t"}"#,
            // custom entry of an unrelated customType.
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
    fn session_header_without_cwd_registers_with_empty_cwd() {
        let line = r#"{"type":"session","version":3,"id":"0197","timestamp":"t"}"#;
        match &decode(line)[..] {
            [AgentEvent::SessionStart { cwd, .. }] => {
                assert_eq!(cwd, Path::new(""), "missing cwd → empty path fallback");
            }
            other => panic!("expected one SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn omp_agent_dir_honors_non_empty_env_override() {
        // Env-mutating → take the process-global guard and restore.
        let _env = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("PI_CODING_AGENT_DIR");

        std::env::set_var("PI_CODING_AGENT_DIR", "/custom/agent");
        assert_eq!(omp_agent_dir(), PathBuf::from("/custom/agent"));

        // Set-but-empty OR whitespace-only is treated as unset (trim-based, the
        // #172 policy — a `"  "` value must not resolve the dir to a relative "  ").
        for blank in ["", "   "] {
            std::env::set_var("PI_CODING_AGENT_DIR", blank);
            let dflt = omp_agent_dir();
            assert!(
                dflt.ends_with(Path::new(".omp/agent")),
                "blank override {blank:?} → ~/.omp/agent fallback, got {dflt:?}"
            );
        }

        std::env::remove_var("PI_CODING_AGENT_DIR");
        assert!(omp_agent_dir().ends_with(Path::new(".omp/agent")));

        match saved {
            Some(v) => std::env::set_var("PI_CODING_AGENT_DIR", v),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
    }

    // The session-ended checker tests live with the runtime half in `native.rs`.
}
