# pixtuoid-core/tests — agent guide

Integration tests for the headless lib, organized **by capability/layer** (the
suite's real axis), with the per-CLI dimension living where the actual variation
is — the source fixtures. 9 test binaries (each top-level `tests/*.rs` or
`tests/<area>/main.rs` is one binary):

```
tests/
├── sources/main.rs           the source/decode layer (1 binary)
│   ├── decode/mod.rs         cross-CLI decoder unit tests
│   │   └── fixtures/{hooks,jsonl}/   decode's OWN data (single-owner; NOT scanned)
│   ├── conformance.rs        per-source SessionStart→tool snapshot harness (insta)
│   ├── manager.rs            SourceManager spawn/health
│   ├── claude/mod.rs         CC subagent lifecycle
│   │   └── fixtures/hook-payloads.jsonl   CC's OWN data (single-owner; NOT scanned)
│   ├── codex/mod.rs          codex subagent lifecycle
│   │   └── fixtures/hook-payloads.jsonl   codex's OWN data (single-owner; NOT scanned)
│   ├── codewhale/mod.rs      codewhale subagent lifecycle (spawn/complete → child sprite)
│   │   └── fixtures/hook-payloads.jsonl   codewhale's OWN data (single-owner; NOT scanned)
│   ├── snapshots/            insta snaps  (sources__conformance__<source>__<scenario>)
│   └── fixtures/<source>/    ══ conformance scenarios ONLY — dir name MUST be a registered source ══
├── reducer/main.rs           state-machine behavior (1 binary; shared builders `start`/`delegating_pair` live in main.rs)
│   ├── lifecycle.rs          SessionStart/End arms: registration/capacity, resurrect-in-place, hook synthesis of unknown ids, duplicate-start backfill, `Identity`
│   ├── activity.rs           per-slot FSM: Active/Idle debounce, Waiting set/resolve gates, active_ms + tool_call_count
│   ├── tasks.rs              active_tasks suppression, hook-wins dedup, drains, b1 cascade grace + waiting-clobber pins
│   ├── liveness.rs           stale sweeps/timeouts, proof-of-life + vouch exemptions, cascade↓ / liveness↑ / readiness, cycle reap
│   ├── display.rs            labels: cwd-basename derivation, ghost ordinals, source prefixes, rename
│   ├── child_ledger.rs       SessionEnd tombstones + child-end ledger: gating, revival relink, parent adoption, cycle filter
│   └── snapshot.rs           full-scene serialization golden (#279): deterministic fixed-time script → insta YAML of the whole SceneState (locks tree shape + reducer output end-to-end)
├── e2e.rs                    end-to-end driver wiring (own binary)
├── watcher/main.rs           JsonlWatcher behavior (1 binary; the poll-seam harness — `fast_watch`,
│   │                         `cc_watcher`, `vouch_snapshot`, `write_lines`, `backdate` + the cc line builders — lives in main.rs)
│   ├── tailing.rs            cursor mechanics: append-tail emit, partial trailing line, truncation reset, non-UTF-8 skip
│   ├── first_sight.rs        the first-sight gate: stale/recent/ended/oversized seeds, probe bypass, cwd + id/label derivers, subagent parent links
│   ├── liveness.rs           proof-of-life emission, negative vouch, instant exit (pid death), probe-failure no-ops
│   ├── unclaim.rs            child-end un-claim: turn-N+1 re-register + in-flight multi-turn revival
│   ├── sources.rs            Source::run glue (codex / antigravity / claude-code / copilot bind+spawn)
│   └── attach.rs             the mid-attach scenario suite (attach shows exactly the live set)
├── transport/main.rs         #[cfg(unix)] mod socket;  #[cfg(windows)] mod pipe;
├── render/main.rs            mod {blit, format}  +  render/fixtures/ (sprites)
├── socket_path_parity.rs     FLAT — publish-excluded (see below)
├── supported_sources_manifest.rs   FLAT — publish-excluded
└── proof_fixture_disjointness.rs   FLAT — publish-excluded (see below)
```

## Governing principle

- **Code groups by capability/layer**, not by CLI. Only the subagent-lifecycle
  tests are single-CLI (`sources/{claude,codex,codewhale}`); decode/conformance are cross-CLI.
- **Data scopes to the binary that reads it, sub-grouped by CLI.** A fixture read
  by one test module lives *with that module* at `sources/<module>/fixtures/`;
  fixtures the conformance harness iterates live in `sources/fixtures/<source>/`.

## Adding a new agent CLI — the test steps

1. **Always:** add `tests/sources/fixtures/<registered-source>/<scenario>/` — at
   minimum a `SessionStart` conformance scenario. `conformance.rs` auto-discovers
   it; `supported_sources_manifest` forces the manifest row; `cargo insta review`
   to accept the new snapshot. The dir name MUST equal the `REGISTERED_SOURCES`
   name (`claude-code`, not `claude`).
2. **Only if the CLI has unique behavior** (subagent hooks, custom lifecycle): add
   `tests/sources/<cli>.rs` (or `<cli>/mod.rs` if it needs private fixtures) and
   register `mod <cli>;` in `tests/sources/main.rs`. Plain CLIs (antigravity,
   reasonix) need none — `decode/mod.rs` + `conformance.rs` cover them.

## Known sharp edges

- **Three tests stay FLAT and MUST NOT be moved into a grouped binary**:
  `socket_path_parity.rs` (`#[path]`-includes the hook shim's `paths.rs`),
  `supported_sources_manifest.rs` (reads `../../site/src/sources.json`), and
  `proof_fixture_disjointness.rs` (reads `../../site/src/components/Statusline.astro`
  — the proof-session fixture's disjointness pin against the statusline ticker's
  `FALLBACK` corpus). All three are in `Cargo.toml`'s `exclude` so the published
  `.crate` tarball builds without their sibling `site/`/`pixtuoid-hook` files; a
  submodule of a grouped binary can't be individually excluded (the parent's
  `mod` would fail to compile on the extracted crate).
- **A multi-file binary is `tests/<area>/main.rs`, NOT `tests/<area>.rs`.** A
  top-level `area.rs` is a *crate root* — its `mod foo;` resolves to `tests/foo.rs`
  (a sibling), not `tests/area/foo.rs`. The `<area>/main.rs` dir form makes `mod`
  resolve inside `<area>/`. (nextest still runs every `#[test]` in its own process,
  so fewer binaries = faster linking, same parallelism.)
- **`conformance.rs` (the harness) asserts every dir under `sources/fixtures/` is a
  registered source** (`descriptor_for(dirname).is_some()`) and that each scenario
  dir holds exactly one transcript/hook file → one AgentId. So single-owner,
  multi-payload fixtures (decode's hooks/jsonl, codex's lifecycle file) CANNOT live
  there — they'd be mis-scanned and panic. They co-locate with their module instead.
- **insta snapshot names = `<binary>__<module>__<explicit-name>`** → `sources__conformance__<source>__<scenario>.snap`. The decoded-event bodies hash an `AgentId` from the fixture's path *relative to `fixtures_root()`* — so moving the fixtures tree is snapshot-safe as long as the per-source suffix is preserved.

## Windows parity twins

`transport/pipe.rs` (in `transport/main.rs`) and the hook shim's
`tests/shim_pipe.rs` are `#[cfg(windows)]` twins of `transport/socket.rs`
and `tests/shim.rs` respectively — they run only on the `windows-test` CI
job (full nextest suite). Each branch executes only on its
target OS, and the windows job is part of the parity invariant: a behavior
pinned on one platform's transport must stay pinned on the other's twin.
