---
mode: agent
description: "Add a new agent-CLI Source adapter to pixtuoid"
---

# Add a new agent-CLI Source

Wire up a new agent CLI (`${input:name}`) as a pixtuoid `Source`. This is **not**
a single-file change — read `crates/pixtuoid-core/CLAUDE.md` ("multi-source
decoding" / "Adding a new agent CLI") first, then:

1. Implement the `Source` trait (hook-only CLI? skip it + the runtime wiring —
   set `line_decoder: None` and ship a `hook.custom` decoder + install target
   instead). Per-source JSONL format knowledge lives in the
   source's **own decoder fn** (injected into `JsonlWatcher` via fn pointers), not
   a shared decoder.
2. Add ONE `SourceDescriptor` row in `source/registry.rs` (label prefix, decoders,
   hook keying, reducer caps) and the name to `source::REGISTERED_SOURCES` — the
   bridge + conformance tests force a coalescing fixture and table↔list equality.
3. Wire it into `runtime::run_async` — the runtime spawns sources by hand; the
   registry only gates the conformance tests, not runtime wiring.
4. If you add an `AgentEvent` variant, add a matching arm to
   `AgentEvent::agent_id()` in `source/mod.rs`.
5. Update the four test files that exercise the channel / `Source` / reducer
   together: `tests/reducer.rs`, `tests/e2e.rs`, `tests/hook_socket.rs`,
   `tests/jsonl_watcher.rs`, plus `runtime.rs` on the binary side.
6. Add a captured rollout fixture + a lifecycle regression test for the new CLI.

Respect the architecture invariants (no terminal deps in `pixtuoid-core`; one
`(Transport, AgentEvent)` channel) and `.github/instructions/rust.instructions.md`.
Run `just preflight` before opening the PR.
