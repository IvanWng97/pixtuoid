---
mode: agent
description: "Add a new agent-CLI Source adapter to pixtuoid"
---

# Add a new agent-CLI Source

Wire up a new agent CLI (`${input:name}`) as a pixtuoid `Source`. This is **not**
a single-file change — read `crates/pixtuoid-core/CLAUDE.md` ("multi-source
decoding" / "Adding a new agent CLI") first, then:

1. Implement the `Source` trait (hook-only CLI? skip it + the runtime wiring —
   set `transcript: None` and ship a `hook.custom` decoder + install target
   instead). Per-source JSONL format knowledge lives in the
   source's **own decoder fn** (injected into `JsonlWatcher` via fn pointers), not
   a shared decoder.
2. Add ONE `SourceDescriptor` row in `source/registry.rs` (label prefix, decoders,
   hook keying, reducer caps) and the name to `source::REGISTERED_SOURCES` — the
   bridge + conformance tests force a coalescing fixture and table↔list equality.
3. Wire it into `runtime/driver.rs::build_source_set` (the one construction site,
   called by `run_async`) — the registry gates conformance tests, not spawning,
   but a bridge test (`build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router`)
   pins that set to the registry's transcript-bearing rows, so a registered-but-unwired
   source fails `just test`.
4. If you add an `AgentEvent` variant, add a matching arm to
   `AgentEvent::agent_id()` in `source/mod.rs`.
5. Update the four test areas that exercise the channel / `Source` / reducer
   together: `tests/reducer/`, `tests/e2e.rs`, `tests/transport/socket.rs`,
   `tests/watcher/`, plus `runtime/driver.rs` on the binary side.
6. Add a captured fixture under `tests/sources/fixtures/<name>/<scenario>/` (a
   unique lifecycle also gets a `tests/sources/<cli>.rs` module). The test
   layout + add-a-CLI steps are in `crates/pixtuoid-core/tests/CLAUDE.md`.
7. Add a row to `site/src/sources.json` (the tool × OS matrix + README glimpse);
   `tests/supported_sources_manifest.rs` pins its `supported` set to
   `REGISTERED_SOURCES`, so a new source **fails that test** until the row exists.
   Then `just gen-readme` to sync the README's Supported Tools section.
8. Add the per-source badge hue: a `Theme::source` (`SourceColors`) field in EVERY
   theme file + a `match` arm in `tui/widgets/dashboard.rs::dashboard_line`
   (`every_registry_source_has_a_non_fallback_badge_color` +
   `source_colors_cover_every_registered_source` **fail** otherwise).
9. If the CLI has a custom config/home root, add a `pub fn <cli>_home()` honoring
   its `*_HOME` env precedence, called from BOTH the watcher's `default_paths()`
   AND the installer's `default_config_path()` (one function, two consumers).

These cross-crate deps are caught only by `just preflight`'s FULL run, not the
targeted source/install suites. Respect the architecture invariants (no terminal
deps in `pixtuoid-core`/`pixtuoid-scene`; one `(Transport, AgentEvent)` channel)
and `.github/instructions/rust.instructions.md`. Run `just preflight` before the PR.
