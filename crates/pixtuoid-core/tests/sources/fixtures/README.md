# Source decode fixtures

Golden fixtures for the per-CLI decode + hook↔JSONL **coalescing** contract,
driven by `tests/sources/conformance.rs`. Each fixture is a directory:

```
tests/sources/fixtures/<source>/<scenario>/
    <transcript>.jsonl     # JSONL transcript lines, fed to the source's LineDecoder
                           # (JSONL-bearing sources only — a hook-only row,
                           # transcript: None, ships NO transcript)
    hook-payloads.jsonl    # one hook payload per line, fed to decode_hook_payload
    # expected snapshot lives in tests/sources/snapshots/ (insta), generated on first run
```

A scenario ships the transports its source actually has: both files
(CC/Codex), transcript-only (antigravity — no hooks), or hook-payloads-only
(reasonix — hook-only, no watchable JSONL).

(This tree is **conformance-scanned ONLY** — `conformance.rs` asserts every dir
here is a registered source. Single-owner fixtures read by one module — decode's
`sources/decode/fixtures/`, codex's `sources/codex/fixtures/`, render's
`render/fixtures/` — live with their module, NOT here. See
[`tests/CLAUDE.md`](../../CLAUDE.md) for the governing principle.)

The harness, for each fixture dir:
1. decodes the transcript lines (via the source's `LineDecoder`) and the hook
   payloads (via `decode_hook_payload`),
2. snapshots the full decoded `AgentEvent` sequence (`insta`),
3. **asserts every decoded event shares ONE `AgentId`** — the coalescing
   contract. This is the bug class that keeps biting (hook and JSONL keying a
   session differently → two sprites).

`{{TRANSCRIPT_PATH}}` in a hook payload's `transcript_path` is replaced at
runtime with the fixture's transcript file path (for a hook-only scenario: the
scenario dir's relative path), so a CC hook (which coalesces on
`transcript_path`) lines up with its JSONL file. Codex carries it too — to
prove Codex *ignores* it and still coalesces on `session_id`.

**Adding a CLI:** drop a new `fixtures/<source>/<scenario>/` dir — the decoder
comes from the source's `SourceDescriptor` row in `source/registry.rs` (a
hook-only row, `transcript: None`, ships only `hook-payloads.jsonl` instead
of a transcript). Run `cargo insta review` to accept the generated snapshot.
No harness edit, no other test code.

## Provenance

These were derived from **real** sessions (so the structure — field names,
nesting, event order — is authentic), then **sanitized**: every identifier and
value that could be real or personal (UUIDs, `cwd`/paths, timestamps,
`call_id`/`turn_id`, command output, agent messages) is replaced with a dummy.
Only the *shape* is load-bearing for decode, so this keeps the test honest while
committing no real data. UUIDs stay valid (`8-4-4-4-12` hex) and the coalescing
key is preserved (a fixture's hook `session_id` == its rollout-filename UUID;
CC's hook `transcript_path` == its transcript via `{{TRANSCRIPT_PATH}}`).

- **`codex/permission-flow/`** — the escalated path: `task_started`,
  `function_call` with `sandbox_permissions:"require_escalated"` → Waiting,
  `function_call_output` → resume, `task_complete`. Plus hooks
  (`UserPromptSubmit`, `PermissionRequest`, `Stop`).
- **`codex/tool-run/`** — the non-escalated path: a plain `function_call`
  (no escalation) → working, `function_call_output` → resume, `task_complete`.
  Hooks: `UserPromptSubmit`, `Stop` (no permission gate).
- **`claude-code/tool-call/`** — a `Glob` tool_use + its tool_result (attributed
  to a `code-architect` subagent → `Rename`), with `PreToolUse`/`PostToolUse`
  hooks. Proves **path-keyed** coalescing.
- **`reasonix/tool-run/`** — HOOK-ONLY (no transcript): a real session arc —
  `SessionStart`, `UserPromptSubmit`, a `read_file` and a `bash` tool, an
  `explore` subagent dispatch (→ `ToolDetail::Task`), `Stop`, `SessionEnd`.
  Proves **cwd-keyed** coalescing (the only identity Reasonix payloads carry).
  **Captured from a live Reasonix v1.3.0 session** (Homebrew `esengine/reasonix`,
  DeepSeek backend) via temporary tee hooks in `~/.reasonix/settings.json`, then
  sanitized per the provenance bar above: `cwd` normalized to one synthetic path
  (→ one `AgentId`), verbose/PII fields (`toolResult`, `lastAssistantText`,
  `turn`) dropped, field names + tool names/args kept verbatim. The
  `Notification` → Waiting approval-gate arm is NOT in this golden —
  non-interactive `reasonix run` has no approval gate, so it never fires — that
  arm is unit-pinned in `source/reasonix.rs` instead (closes #135).
- **`claude-code/proof-session/`** — the §3 site proof-split timeline: one root
  session, Read → Edit → Bash over `site/src/components/ElevatorShaft.astro`,
  self-referentially fixing the elevator-LED-lags-the-statusline desync (its
  own real bottom-clamp logic — see `Statusline.astro`'s `clampToLastFloorAtBottom`).
  ALSO read at render time by the `snapshot --proof` site-media renderer
  (scripts/media.json job `proof`) — its timestamps ARE the clip's beat
  timings, so retiming this fixture re-times the committed proof.webm. Its
  visible strings (task phrase, file path, shell command) are pinned disjoint
  from the statusline ticker's own FALLBACK corpus by
  `tests/proof_fixture_disjointness.rs` (STATUSLINE-COLLISION handoff,
  `docs/superpowers/plans/2026-07-05-wb-4-proof.md`) — the two are
  agent-narration surfaces sharing one viewport at 4F.
- **`omp/tool-run/`** — captured from a live **omp 16.4.0** `omp -p` run
  (2026-07-10, the #517 byte-real anchor for `verified_version`), sanitized:
  the fixed-width `type:"title"` slot, the v3 `session` header,
  `model_change`/`thinking_level_change` (not sprite-visible), a `bash`
  toolCall/toolResult round, the duplicate `tool_execution_start` custom
  entry (deliberately NOT decoded — same tool_use_id would double-count),
  and the `session_exit` teardown marker (`reason:"dispose"` — the real `-p`
  teardown reason, vs `"exit command"` interactively).
- **`omp/ask-round/`** — a real interactive `ask` round (captured 2026-07-05,
  sanitized; #519): the `ask` toolCall decodes to ActivityStart **then**
  Waiting (the ellipsize-capped question text), and the answering
  toolResult's ActivityEnd resolves the Wait via the `gated_before_waiting`
  binding — the order is the load-bearing contract.
- **`omp/2026-07-10T18-00-00-000Z_…000001/`** — a real `task`-subagent CHILD
  transcript (16.4.0): the scenario dir is named as the PARENT session stem
  because the parent link IS the path nesting (`<parent-stem>/<taskId>.jsonl`)
  — `omp_id_from_path` keys the chain and the header decode emits a parented
  SessionStart. Also pins that `session_init` / `developer`-role /
  `thinking`-block entries decode to nothing.
