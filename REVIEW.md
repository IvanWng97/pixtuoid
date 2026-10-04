# Review rules

Read [`AGENTS.md`](AGENTS.md), and the nested `AGENTS.md` of each directory the
diff touches, first; these rules add to generic defect hunting.

## Scope

- A lens bot applies [What to check](#what-to-check), its [lens](#lenses)
  and [Re-review](#re-review); the **correctness** bot also applies every
  matching [escalation](#escalation) row to the diff and names the local rows
  in its summary.
- A local row lens applies only its [row](#escalation).
- All apply [Do not flag](#do-not-flag), the Lenses preamble,
  [Severity](#severity) and [Output](#output).

## What to check

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

- A breach of AGENTS.md (invariants, conventions, "Things NOT to do").
- New behavior without a test; scope beyond what the PR body states.

### Sweeps that leave the diff

- **Siblings** — a guard/cap/validation added to some of a sibling set
  (per-source decoders, install targets, platform arms, twin call sites) or to
  one caller of a shared fn: `rg` and verify the full set and every call site.
- **DRY** — every new fn/type/helper/const gets a whole-tree search for an
  existing implementation, weighted by divergence risk; every value a new line
  reads is the authority its consumer uses, not a sibling's (#1042, #1155).
  Test fixtures stay inline.
- **Drift** — docs naming a moved file/fn/flag/count, searched including
  `.github/` and `.claude/`.
- **Wire format** — a decoder or drift-watch row vs the recorded upstream
  shape; whether a source owes a row is `source/drift.rs`'s header.
- **Unwired additions** — every new field/flag/parameter/asset/gate has a live
  consumer in the same diff (`_x` bindings and `pub` fields evade the lints).
- **Manifest bridge** — `site/src/*.json` and generated schemas vs their Rust
  source of truth.
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

### Do not flag

- Anything a [CI gate](docs/CONTRIBUTING.md#ci-gates) or clippy/rustfmt
  enforces, compiling included; pure style.
- Behavior documented where it is constrained: read the item's doc comment and
  the comments on the lines it governs first.
- Speculative defense in depth where a primary defense holds. Every layer a
  trap names is primary, and a concrete, reachable extra-layer gap at a trust
  boundary is `issue (non-blocking)`.
- Risks needing unlikely or unreachable preconditions; performance unless
  measurable (the TUI repaints at `anim::PAINT_FPS`).
- A missing comment ([comment audit](#design) owns the rest).

## Lenses

Every finding states its evidence before its claim, cites a `file:line` from a
file actually read, and survives a sharp-edge check: the same seam is not the
same claim. A claim about an external artifact (action tag, crate release,
upstream shape) not fetched this session says "unverified" (#112).

### Correctness

Hold the diff to each claim the PR body's
[plan answers](.github/prompts/impl-plan.prompt.md#the-contract-with-review)
make. Name any CI-only gate the diff can turn red (`--lib` builds neither bin
modules nor examples).

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
4. **Comment audit**, every diff: for EVERY file the diff touches, a one-line
   change included, read its entire comment population against the code and
   AGENTS.md's comment rules (accuracy, value, rot, vestigial,
   self-repetition); a story told twice keeps the copy on the narrowest thing
   it constrains.
5. **Proportion**, each with the cheaper alternative, `issue (non-blocking)`
   unless it breaches AGENTS.md: **over-engineering** (YAGNI), a branch,
   helper, parameter, type, fallback arm or test machinery serving no
   reachable case; **low ROI**, code, API change or coupling out of proportion
   to its payoff (an API made `Option` across three painters for a width
   almost never hit); **hard to maintain**, needless indirection or layers,
   special-case branches, one change smeared across many files; an invariant
   held by prose where a type or the one source of truth could make the
   failure impossible (#1142); a check outside [its own
   layer](docs/CONTRIBUTING.md#convergence-contract). Documented load-bearing
   defense (shim exit-0, config-never-wipe, liveness ladders) stays.
6. **Naming**: every new or renamed name, `pub(crate)` and modules included,
   is faithful, clear and concise. Each finding cites what the name breaks;
   severity per [Severity](#severity), non-blocking by default.
   - **Faithful**: it says what the item is or does at head; a behavior change
     renames it.
   - **Clear**: a reader without this PR's context reads it right. One concept,
     one name across crates and painters (`rg` before coining), and no name
     for two concepts; compass words are [screen-space](crates/pixtuoid-scene/AGENTS.md).
   - **Concise**: the shortest name that stays clear, in the domain's existing
     word and Rust's [naming guidelines](https://rust-lang.github.io/api-guidelines/naming.html);
     never a placeholder.
7. **Sourced practice**: a PR body, design or comment calling an approach best
   practice, idiomatic or standard cites what it was checked against (a doc
   URL or `path:line`). A missing or contradicting source is
   `issue (non-blocking)`, blocking only under [Severity](#severity)'s rule:
   verified against the code as a correctness, security or invariant breach.

## Escalation

The two lens bots are the floor; each matching row adds one focused lens. The
first column decides; Paths are where it usually fires. A **local** row needs
the head tree read, built or run, or an upstream fetched, so its local run is
mandatory ([recorded](docs/CONTRIBUTING.md#the-merge-gate)), and that run is
its focused lens alone: the floor is never re-run locally.

| Diff touches… | Paths | Local | The added lens must… |
|---|---|---|---|
| Generated art / clips | | local | Extract frames and READ them; census the money shot. |
| Reducer / liveness / sweeps | `crates/pixtuoid-core/src/state/`, `…/source/jsonl/liveness.rs`, `…/source/exit_watch.rs`, `…/source/daemon.rs` | local | Trace the downstream interaction graph (rebind, sweeps, TTLs, cascade, dedup, polarity) and the provenance of every newly keyed signal. |
| A public rendered artifact | `site/src/`, `integrations/raycast/src/` | local | DRIVE the built page and MEASURE: WCAG in every interactive state, mobile pan, no-JS (#455). |
| An interactive TUI flow | | local | WALK each user path end-to-end: first run, failure branches, the no-CLI user (#359). |
| The shim | `crates/pixtuoid-hook/` | local | Audit the WHOLE shim for never-panic: `args_os()`, no slicing of untrusted bytes, bounded reads, every error path `exit(0)` (#198). |
| The hook's daemon side | `crates/pixtuoid-core/src/source/hook/` | local | The endpoint is never looser than owner-only, arbitration cannot steal a live owner's socket, both `unix.rs`/`windows.rs` arms hold (Windows runs only in CI, outside mutation testing), and each guard is PINNED. |
| Motion / pose / walk-leg | `crates/pixtuoid-scene/src/` `walk/`, `pose/`, `pathfind/`, `physics.rs` | local | Render and WATCH it (the snapshot example, or `scripts/lib/tier-replay.sh` for resume/lifecycle) before the verdict (#61). |
| A string/layout a painter frames | | local | Render the COMPOSED frame; string-equality tests are blind to framing (#308). |
| Another CLI's config | `crates/pixtuoid/src/install/` | local | Enumerate every resolution axis and re-verify each against that CLI's upstream in-session; write ⊆ verify (#338). |
| A new source / hook integration | `crates/pixtuoid-core/src/source/registry.rs` | local | LIVE run or hermetic replay without capture-rig convenience flags; event shapes from canonical upstream docs, never a fork. |
| A refactor: a `refactor` PR, or a move or dedup across modules | | local | Adversarial toward revert, per consolidation: one reason-to-change per call site; name the conversions that moved semantics — a batch hides exactly one (#461). No test leaves `cargo nextest list` at head unless the PR body names it. |
| Geometry, sky, lighting or other domain math | `crates/pixtuoid-scene/src/` `sky/`, `celestial.rs`, `lighting/`, `layout/` | local | Enumerate the domain invariants and re-derive each across the parameter space, edges included (#471, #1049, #1053). |
| A crate edge, new dependency or widened public API (a version bump of an existing dependency doesn't match) | `Cargo.toml`, `crates/*/Cargo.toml`, `api/` | local | Justify every crate `cargo tree -e normal` adds at head, transitive ones included; every new workspace edge follows [AGENTS.md](AGENTS.md#layout)'s crate DAG; every item made `pub` has a consumer outside its crate. |
| CI, the merge gate or release tooling | `.github/workflows/`, `.github/actions/`, `.mergify.yml`, `policy/`, `release-plz.toml`, `REVIEW.md`, `docs/CONTRIBUTING.md` | local | Name every check, contract or row the diff loosens; trace each trigger (fork, bot actor, cancelled or superseded run) to a gate that fails closed. |

## Severity

[Conventional Comments](https://conventionalcomments.org/) labels, the
[`review-schema.json`](.github/prompts/review-schema.json) `severity` enum;
nits and taste are never posted. Dispositions:
[CONTRIBUTING](docs/CONTRIBUTING.md#pull-requests).

- `issue (blocking)` — correctness, security or invariant. It blocks only once
  the agent that ran the review or a maintainer confirms it against the code;
  the finder's label alone never blocks.
- `issue (non-blocking)` — any other real defect this PR introduced.
- `issue (pre-existing)` — real, not introduced here.

## Re-review

Never re-flag a finding that already has a thread in
`.claude-review/prior-threads.json` (this lens's own threads on the PR),
resolved or not
([re-review convergence](https://code.claude.com/docs/en/code-review#what-you-can-tune));
new findings follow [Severity](#severity).

## Output

Bots return only the structured result
[`review-schema.json`](.github/prompts/review-schema.json) defines:

- `summary`: one sentence.
- `severity`: the [label](#severity)'s decoration.
- `path`: repository-relative (the `b/` side of `pr.diff`), never absolute.
- `line`: the absolute head-side line, never invented.
- Every blocking finding, first; then at most 5 non-blocking and pre-existing
  ones, pre-existing last. The summary counts every finding left out,
  including any past the schema's `maxItems` ceiling.
- Each `body`: the verified finding and a concrete failure scenario.
