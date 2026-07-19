---
name: add-source
version: 1.1.0
description: "Wire a new agent-CLI Source adapter into pixtuoid (a new coding CLI whose sessions become office sprites). Use when the user says 'add support for <CLI>', 'add a source for <tool>', or 'integrate <agent CLI>'. Orchestrates the cross-crate checklist whose steps have TEST TEETH — the ones a diff-scoped edit silently misses (site manifest bridge, per-source badge hue, home-dir fn, plus the two `pixtuoid/tests/*` integration goldens that `-p <crate> --lib` never builds)."
metadata:
  scope: "pixtuoid repo only"
---

# add-source (v1)

Adding an agent CLI is **not a single-file change** — it spans `pixtuoid-core`
(decoder + registry + tests), the `pixtuoid` binary (runtime wiring + install
target + badge hue), and the site manifest. Several steps have **test teeth**
that only `just preflight`'s FULL run catches, not the targeted source/install
suites — so an agent that stops at "it compiles" ships a red PR.

## When to use

- "Add support for <CLI>" / "integrate <agent tool>" / "add a source for X".
- A new transcript-bearing OR hook-only coding CLI should show up as sprites.

## The authoritative checklist

The complete, current step list lives in **[`crates/pixtuoid-core/CLAUDE.md`](../../../crates/pixtuoid-core/CLAUDE.md)**
("multi-source decoding" / "Adding a new agent CLI") — read it first; it is the
source of truth and stays current. The Copilot-format summary is
[`.github/prompts/add-source.prompt.md`](../../../.github/prompts/add-source.prompt.md).
Before you start, decide **transcript-bearing vs hook-only** (invariant #3): a
hook-only CLI (Reasonix/CodeWhale/opencode/Cursor) sets `transcript: None`, skips
the runtime wiring + `Source` impl, and ships a `hook.custom` decoder + an
`install/` target instead.

## The test-teeth steps agents miss

These are the ones with a failing test attached — do NOT stop before them:

- **`site/src/sources.json` row** — a manifest bridge test fails until it exists;
  then `just gen-readme` to sync the README. (CLAUDE.md step 5.)
- **Per-source badge hue** — a `Theme::source` (`SourceColors`) field in EVERY
  theme file + a `dashboard_line` match arm; two guard tests fail otherwise.
  (CLAUDE.md step 7.)
- **`pub fn <cli>_home()`** if the CLI has a custom config root — one fn honoring
  its `*_HOME` precedence, called from BOTH the watcher's `default_paths()` AND the
  installer's `default_config_path()` so they can't disagree. (CLAUDE.md step 6.)
- **A captured fixture** under `tests/sources/fixtures/<name>/<scenario>/`
  exercising the **SessionStart hook** — the conformance test forces one, and its
  one-AgentId assertion guards against the reason-field ghost.
- **`sources --json` CLI golden** — `crates/pixtuoid/tests/snapshots/cli/sources.json`,
  the real-binary end-to-end golden the Raycast `--json` contract rides. The new
  source's row must appear or `cli_json::sources_json_lists_every_source_in_an_isolated_home`
  reds; regenerate with `SNAPSHOTS=overwrite cargo test -p pixtuoid --test cli_json`.
- **`wire_to_pixels` matrix row** — `crates/pixtuoid/tests/wire_to_pixels.rs`: add a
  `WireCase` (reuse your SessionStart fixture; a hook-only source uses
  `DecodeKind::Hook` + `Transport::Hook`, like cursor/hermes — a transcript source
  the JSONL variant) AND the per-source `<name>_..._renders_a_painted_sprite` test,
  or `wire_matrix_covers_every_registered_source` reds ("registration is not
  coverage").

**Those last two live in `pixtuoid/tests/*.rs` INTEGRATION BINARIES that
`-p <crate> --lib` never builds** — the #692 (kimi) miss: a green `--lib` run AND a
multi-lens review BOTH passed while these two were red, caught only by the pre-push
`just preflight`. Run `cargo nextest run --workspace` / `just test` (NOT just
`--lib`) before declaring green — a targeted `--lib` run is not the full suite.

(The exact test names + full step list are in `crates/pixtuoid-core/CLAUDE.md`
"Adding a new agent CLI" and `add-source.prompt.md` — this skill headlines the
teeth, those own the specifics.)

## Finish

- Wire it into `runtime/driver.rs::build_source_set` (transcript-bearing only —
  the one construction site, called by `run_async`). This step HAS teeth:
  `build_source_set_wires_every_transcript_bearing_source_plus_the_hook_router`
  (`driver.rs`) FAILS for a registered source left unwired, so `just preflight`
  catches the miss — don't make it wait that long.
- Capture the real wire shape and set `verified_version` (`"unknown"` until a
  byte-real capture anchors it). Drift-watch it (see the add-a-CLI list's
  drift-watch note in `crates/pixtuoid-core/CLAUDE.md`).
- `just gen-contract` only if you touched the `--json`/`SourceStatus`/`OutcomeRow`
  SHAPE (adding a row doesn't).
- `just preflight` before the PR, then run the **two-lens-review** skill.
