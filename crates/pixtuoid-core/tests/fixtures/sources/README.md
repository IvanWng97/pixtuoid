# Source decode fixtures

Golden fixtures for the per-CLI decode + hook↔JSONL **coalescing** contract,
driven by `tests/fixture_harness.rs`. Each fixture is a directory:

```
tests/fixtures/sources/<source>/<scenario>/
    <transcript>.jsonl     # JSONL transcript lines, fed to the source's LineDecoder
                           # (JSONL-bearing sources only — a hook-only row,
                           # line_decoder: None, ships NO transcript)
    hook-payloads.jsonl    # one hook payload per line, fed to decode_hook_payload
    # expected snapshot lives in tests/snapshots/ (insta), generated on first run
```

A scenario ships the transports its source actually has: both files
(CC/Codex), transcript-only (antigravity — no hooks), or hook-payloads-only
(reasonix — hook-only, no watchable JSONL).

(`tests/fixtures/` also holds sprite/hook/jsonl fixtures for unrelated tests, so
per-source decode fixtures live under the dedicated `sources/` subtree.)

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
hook-only row, `line_decoder: None`, ships only `hook-payloads.jsonl` instead
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
