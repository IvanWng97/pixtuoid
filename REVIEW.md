# Review rules

Read [`AGENTS.md`](AGENTS.md), and the nested `AGENTS.md` of each directory the
diff touches, first; these rules add to generic defect hunting.

## Scope

The org's [shared review](https://github.com/pixtuoid/.github/blob/main/.github/workflows/claude-review.yml)
runs a correctness and a design lens, each over this file but the other's
section, so [Correctness](#correctness) and [Design](#design) keep those
headings. A local row lens applies its [row](#escalation).

## Common

### Traps (the obvious reading is wrong here)

- **Creation polarity** — only a proof-of-LIFE event creates or resurrects an
  entry; a death/exit/TTL signal for an absent id no-ops.
- **Lifecycle authority** — user/model-controllable content (transcript text,
  message bodies, tool args) never drives a state transition; structural
  markers and liveness signals only.
- **Config writes** — atomic, and never destructive on any error/skip/default
  arm: existing-but-unparseable is never rewritten, a skip never strips
  pre-existing hooks.
- **Upgrade path** — state a released version wrote survives; a fresh-install
  assumption that wipes an upgrader's config is blocking (#457).
- **Env** — a set-but-empty var reads as unset (`platform::path_env`).
- **Decode boundary** — untrusted input is sanitized where it enters, never at
  each use site.
- **Char-safe truncation** — user-visible text cuts on char/grapheme
  boundaries, never bytes.
- **Terminal egress** strips Cc controls and Cf bidi overrides (Trojan Source).
- **Denylists** — enumerate the platform's documented set, never memory;
  prefer an allowlist.
- **IPC endpoints** — owner-only at creation (create-restricted-then-rename,
  never a process-global umask); a pre-existing endpoint is hostile.

### Always check

- New behavior without a test; scope beyond what the PR body states.

### Do not flag

- Anything a [CI gate](.github/workflows/ci.yml) or clippy/rustfmt
  enforces, compiling included; pure style.
- Behavior documented where it is constrained: read the item's doc comment and
  the comments on the lines it governs first.
- Speculative defense in depth where a primary defense holds. Every layer a
  trap names is primary, and a concrete, reachable extra-layer gap at a trust
  boundary is non-blocking.
- Risks needing unlikely or unreachable preconditions; performance unless
  measurable (the TUI repaints at `anim::PAINT_FPS`).
- A missing comment ([comment audit](#every-changed-file) owns the rest).

## Every changed file

Read each changed file whole at the head (a removed file has only its hunks),
against the base tree. A defect anywhere in it is in scope, pre-existing when
the diff did not introduce it; a maintenance defect's failure scenario is the
edit that would leave a copy or a comment wrong.

1. **Comment audit**: for every file, a one-line change included, read its
   entire comment population against the code and AGENTS.md's comment rules
   (accuracy, value, rot, vestigial, self-repetition); a story told twice keeps
   the copy on the narrowest thing it constrains.
2. **DRY** — every new fn/type/helper/const gets a whole-tree search for an
   existing implementation, weighted by divergence risk; every value a new line
   reads is the authority its consumer uses, not a sibling's (#1042, #1155).
   Test fixtures stay inline.
3. **Drift** — docs naming a moved file/fn/flag/count, searched including
   `.github/` and `.claude/`.
4. **Test teeth** for every test the file holds, as [Correctness](#correctness)
   defines it.

## Lenses

### Correctness

Hold the diff to each claim the PR body's
[plan answers](.github/prompts/impl-plan.prompt.md#the-contract-with-review)
make. Name any CI-only gate the diff can turn red (`--lib` builds neither bin
modules nor examples).

- **Siblings** — a guard/cap/validation added to some of a sibling set
  (per-source decoders, install targets, platform arms, twin call sites) or to
  one caller of a shared fn: `rg` and verify the full set and every call site.
- **Wire format** — a decoder or drift watch vs the recorded upstream
  shape; every name a decoder reads owes a drift watch, a breadcrumb, or an
  adjudicated `EXEMPT` row (`drift_watch`'s census); `just drift-watch`
  runs the watches.
- **Manifest bridge** — `site/src/*.json` and generated schemas vs their Rust
  source of truth.
- **Unwired additions** — every new field/flag/parameter/asset/gate has a live
  consumer in the same diff (`_x` bindings and `pub` fields evade the lints).
- **Population** — a gate, build flag or config key that selects a set
  (crates, features, targets, jobs, a gate's own tests): name the set before
  and after; a silent shrink is a defect (#1012, #1101, #1103, #1123).
- **Test teeth** — every new or changed test, and every test asserting an
  effect the diff removes or reroutes, fails when the code it names is wrong
  (#889 left `occupied_floor_stays_lit` unable to). Refusal paths are pinned on
  both sides of every window, offsets derived from the constant under test; a
  new gate fires on the violation, stays silent on the legitimate case, and
  names the real requirement. No test reads ambient state: the local time
  zone, `HOME`, the developer's config, a fixed temp path (#1023, #1048).

#### Security

A trust boundary is the hook shim or socket/named-pipe transport; config
writes or install targets; path, home, process, permission or credential
handling; transcript, hook, JSONL, pack or asset ingestion. When the diff
touches none, the summary says so. Otherwise report only a concrete attack or
invariant-breaking sequence against:

1. The shim and config writes: AGENTS.md invariants 4–5 and `install/io.rs`'s
   lock, atomic-write and permission authority.
2. Socket / named pipe: no path traversal, symlink attack, unbounded read or
   unsafe ownership assumption.
3. Untrusted input (hook payloads, transcripts, JSONL, paths, pack data):
   bounded, validated, skipped without panicking.
4. Credentials and subprocesses: no secret in a command or log, no untrusted
   code in a secret-bearing process, no argument spliced into a shell as code
   (justfile, script, workflow step: #1126).

### Design

1. Trace the two nearest consumers of every changed surface for
   contradiction; propose replacement text where you object.
2. New data shapes: name the identity/key-space; consolidate shared identity,
   not shared topic; verify join keys against real production constants,
   never test fixtures.
3. Layering: mechanism calls route through the designated orchestrator.
4. **Proportion**, each with the cheaper alternative, non-blocking
   unless it breaches AGENTS.md: **over-engineering** (YAGNI), a branch,
   helper, parameter, type, fallback arm or test machinery serving no
   reachable case; **low ROI**, code, API change or coupling out of proportion
   to its payoff (an API made `Option` across three painters for a width
   almost never hit); **hard to maintain**, needless indirection or layers,
   special-case branches, one change smeared across many files; an invariant
   held by prose where a type or the one source of truth could make the
   failure impossible (#1142); a check outside [its own
   layer](docs/CONTRIBUTING.md#convergence-contract). A mechanism the diff
   extends is judged as if new against AGENTS.md's [upkeep
   rule](AGENTS.md#conventions); one that fails it draws "delete it".
   A hand-rolled mechanism for a need a maintained library, the platform or
   the upstream tool already meets (AGENTS.md's reuse rule) draws "reuse X",
   naming X and the source checked.
   Documented load-bearing defense (shim exit-0, config-never-wipe, liveness
   ladders) stays.
5. **Naming**: every new or renamed name, `pub(crate)` and modules included,
   is faithful, clear and concise. Each finding cites what the name breaks;
   non-blocking unless it breaches AGENTS.md.
   - **Faithful**: it says what the item is or does at head; a behavior change
     renames it.
   - **Clear**: a reader without this PR's context reads it right. One concept,
     one name across crates and painters (`rg` before coining), and no name
     for two concepts; compass words are [screen-space](crates/pixtuoid-scene/AGENTS.md).
   - **Concise**: the shortest name that stays clear, in the domain's existing
     word and Rust's [naming guidelines](https://rust-lang.github.io/api-guidelines/naming.html);
     never a placeholder.
6. **Sourced practice**: [AGENTS.md](AGENTS.md#conventions)'s fetched-claims
   rule; a claim with no fetched source, or one that contradicts it, breaches it.

## Escalation

The automatic review is the floor, and its summary names each row the diff
matches; each matching row adds one focused lens. The first column decides;
Paths are where it usually fires. A **local** row needs
the head tree read, built or run, or an upstream fetched, so its local run is
mandatory ([recorded](docs/CONTRIBUTING.md#the-merge-gate)), and that run is
its focused lens alone: the floor is never re-run locally.

| Diff touches… | Paths | Local | The added lens must… |
|---|---|---|---|
| Generated art / clips | | local | Extract frames and READ them; census the money shot. A PR's media diff is the smoke job's `gen-check-diffs` overlays. |
| Reducer / liveness / sweeps | `crates/pixtuoid-core/src/state/`, `…/source/jsonl/liveness.rs`, `…/source/exit_watch.rs`, `…/source/daemon.rs` | local | Trace the downstream interaction graph (rebind, sweeps, TTLs, cascade, dedup, polarity) and the provenance of every newly keyed signal. |
| A public rendered artifact | `site/src/`, `integrations/raycast/src/` | local | DRIVE the built page and MEASURE: WCAG in every interactive state, mobile pan, no-JS (#455). |
| An interactive TUI flow | | local | WALK each user path end-to-end: first run, failure branches, the no-CLI user (#359). |
| The shim | `crates/pixtuoid-hook/` | local | Audit the WHOLE shim for never-panic: `args_os()`, no slicing of untrusted bytes, bounded reads, every error path `exit(0)` (#198). |
| The hook's daemon side | `crates/pixtuoid-core/src/source/hook/` | local | The endpoint is never looser than owner-only, arbitration cannot steal a live owner's socket, both `unix.rs`/`windows.rs` arms hold (Windows runs only in CI, outside mutation testing), and each guard is PINNED. |
| Motion / pose / walk-leg | `crates/pixtuoid-scene/src/` `walk/`, `pose/`, `pathfind/`, `physics.rs` | local | Render and WATCH it (the snapshot example, or a `just hermetic` run's scene log for lifecycle) before the verdict (#61). |
| A string/layout a painter frames | | local | Render the COMPOSED frame; string-equality tests are blind to framing (#308). |
| Another CLI's config | `crates/pixtuoid/src/install/` | local | Enumerate every resolution axis and re-verify each against that CLI's upstream in-session; write ⊆ verify (#338). |
| A new source / hook integration (a `bot/bump-*` pin bump whose every commit is pixtuoid-bot's doesn't match: its queue's pinned hermetic run is the replay) | `crates/pixtuoid-core/src/source/registry.rs` | local | LIVE run or hermetic replay without capture-rig convenience flags; event shapes from canonical upstream docs, never a fork. |
| A refactor: a `refactor` PR, or a move or dedup across modules | | local | Adversarial toward revert, per consolidation: one reason-to-change per call site; name the conversions that moved semantics — a batch hides exactly one (#461). No test leaves `cargo nextest list` at head unless the PR body names it. |
| Geometry, sky, lighting or other domain math | `crates/pixtuoid-scene/src/` `sky/`, `celestial.rs`, `lighting/`, `layout/` | local | Enumerate the domain invariants and re-derive each across the parameter space, edges included (#471, #1049, #1053). |
| A crate edge, new dependency or widened public API (a version bump of an existing dependency doesn't match) | `Cargo.toml`, `crates/*/Cargo.toml`, `api/` | local | Justify every crate `cargo tree -e normal` adds at head, transitive ones included; every new workspace edge follows [AGENTS.md](AGENTS.md#layout)'s crate DAG; every item made `pub` has a consumer outside its crate. |
| CI, the merge gate or release tooling (a Dependabot `uses:` pin bump doesn't match) | `.github/workflows/`, `.github/actions/`, `.github/dependabot.yml`, `.mergify.yml`, `policy/`, `release-plz.toml` | local | Name every check, contract or row the diff loosens; trace each trigger (fork, bot actor, cancelled or superseded run) to a gate that fails closed. |

