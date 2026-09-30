# Review rules

Read [`AGENTS.md`](AGENTS.md) first; these rules add to generic defect hunting.
Nested `AGENTS.md` files (crates/*, tests/, site/, integrations/raycast/) add
rules for their trees.

## Scope

- **Claude Security Review** applies only [§ Security](#security).
- Every other reviewer (**Claude Review**, local lenses) applies
  [What to check](#what-to-check), both [lenses](#lenses) and every matching
  [escalation](#escalation) row, naming local rows in its summary.
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
- **Dead fallback** — an arm whose trigger cannot fire, or that duplicates an
  authority, is debt; documented load-bearing defense (shim exit-0,
  config-never-wipe, liveness ladders) stays.
- **Version** — does this diff move the public surface or ship a feature, and
  is the 0.x bump right (patch = fix, minor = feature/breaking)?

### Always check

- A breach of AGENTS.md (invariants, conventions, "Things NOT to do").
- New behavior without a test; scope beyond what the PR states; docs a
  structure, API or workflow change left stale.
- A fix round adds no new gate: a wanted check is its own PR. Prefer a failure
  made impossible (derived from the one source of truth) over one detected; a
  check asserts facts in its own layer (a Rust fact from Rust, never a Python
  regex over `.rs`).

### Sweeps that leave the diff

- **Siblings** — a guard/cap/validation added to some of a sibling set
  (per-source decoders, install targets, platform arms, twin call sites): `rg`
  the full set and verify each.
- **DRY** — every new fn/type/helper/const gets a whole-tree search for an
  existing implementation, weighted by divergence risk; a wrapper whose name
  hides its cost is the same finding.
- **Drift** — docs naming a moved file/fn/flag/count: `rg --hidden` (bare `rg`
  skips `.github/` and `.claude/`).
- **Wire format** — a decoder or drift-watch row vs the real upstream shape;
  whether a source owes a row is `source/drift.rs`'s header.
- **Unwired additions** — every new field/flag/parameter/asset/gate has a live
  consumer in the same diff (`_x` bindings and `pub` fields evade the lints).
- **Manifest bridge** — `site/src/*.json` and generated schemas vs their Rust
  source of truth.
- **Test teeth** — mentally mutate each fix: would its test fail? Refusal
  paths are pinned on both sides of every window, offsets derived from the
  constant under test; a new gate fires on the violation, stays silent on the
  legitimate case, and names the real requirement.

### Do not flag

- Anything a [CI gate](docs/CONTRIBUTING.md#ci-gates) or clippy/rustfmt
  enforces; pure style.
- Behavior documented where it is constrained: read the item's doc comment and
  the comments on the lines it governs first.
- Speculative defense in depth where a primary defense holds. Every layer a
  trap names is primary, and a concrete, reachable extra-layer gap at a trust
  boundary is `issue (non-blocking)`.
- Risks needing unlikely preconditions; performance unless measurable (the TUI
  ticks at `FRAME_TICK_MS`).
- A missing comment ([comment audit](#design) owns the rest).
- A claim about an external artifact (action tag, crate release, tap) from
  memory: verify it in-session (`gh api`, the registry) or write "unverified"
  (#112).

## Lenses

Every finding states its evidence before its claim, cites a `file:line` from a
file actually read, and survives a sharp-edge check: the same seam is not the
same claim. Locally, it also carries an integer confidence 0–100.

### Correctness

Locally, run the applicable gates and report each exit code as observed, never
through a pipe; name any CI-only gate the diff can turn red (`--lib` builds
neither bin modules nor examples).

#### Security

A trust boundary is the hook shim or socket/named-pipe transport; config
writes or install targets; path, home, process, permission or credential
handling; transcript, hook, JSONL, pack or asset ingestion. When the diff
touches none, the security bot returns clean and its summary says so.
Otherwise report only a concrete attack or invariant-breaking sequence
against:

1. The shim: always exits 0, never blocks the agent CLI, keeps
   `pixtuoid-hook`'s `transport::WRITE_TIMEOUT` bound.
2. Config writes: through `install/io.rs`'s lock, atomic-write, permission and
   symlink-resolution authority.
3. Socket / named pipe: no path traversal, symlink attack, unbounded read or
   unsafe ownership assumption.
4. Untrusted input (hook payloads, transcripts, JSONL, paths, pack data):
   bounded, validated, skipped without panicking.
5. Credentials and subprocesses: no secret in a command or log, no untrusted
   code in a secret-bearing process.
6. `unwrap()` on a production path.

### Design

1. Trace the two nearest consumers of every changed surface for
   contradiction; propose replacement text where you object.
2. New data shapes: name the identity/key-space; consolidate shared identity,
   not shared topic; verify join keys against real production constants,
   never test fixtures.
3. Layering: mechanism calls route through the designated orchestrator; a
   `pub` whose only callers are in-crate is `pub(crate)`.
4. **Comment audit**, every diff: for EVERY file the diff touches, a one-line
   change included, read its entire comment population against the code and
   AGENTS.md's comment rules (accuracy, value, rot, vestigial,
   self-repetition; one home per story). Locally, report N items each with a
   disposition, never "passed", plus the diff's net added comment lines and
   every sentence deletable with nothing lost.

## Escalation

Two lenses are the floor; each matching row adds one focused lens. The first
column decides; Paths are where it usually fires. A **local** row needs the
head built or run, or an upstream fetched, so its local run is mandatory,
recorded as a PR comment starting `<!-- local-row:<row>:<head sha> -->`.

| Diff touches… | Paths | Local | The added lens must… |
|---|---|---|---|
| Generated art / clips | | local | Extract frames and READ them; census the money shot. |
| Reducer / liveness / sweeps | `crates/pixtuoid-core/src/state/`, `…/source/jsonl/liveness.rs`, `…/source/exit_watch.rs`, `…/source/daemon.rs` | | Trace the downstream interaction graph (rebind, sweeps, TTLs, cascade, dedup, polarity) and the provenance of every newly keyed signal. |
| A public rendered artifact | `site/src/`, `integrations/raycast/src/` | local | DRIVE the built page and MEASURE: WCAG in every interactive state, mobile pan, no-JS (#455). |
| An interactive TUI flow | | local | WALK each user path end-to-end: first run, failure branches, the no-CLI user (#359). |
| The shim | `crates/pixtuoid-hook/` | | Audit the WHOLE shim for never-panic: `args_os()`, no slicing of untrusted bytes, bounded reads, every error path `exit(0)` (#198). |
| The hook's daemon side | `crates/pixtuoid-core/src/source/hook/` | | The endpoint is never looser than owner-only, arbitration cannot steal a live owner's socket, both `unix.rs`/`windows.rs` arms hold (Windows runs only in CI, outside mutation testing), and each guard is PINNED. |
| Motion / pose / walk-leg | `crates/pixtuoid-scene/src/` `motion/`, `pose/`, `pathfind/`, `physics.rs` | local | Render and WATCH it (the snapshot example, or `scripts/lib/tier-replay.sh` for resume/lifecycle) before the verdict (#61). |
| A string/layout a painter frames | | local | Render the COMPOSED frame; string-equality tests are blind to framing (#308). |
| Another CLI's config | `crates/pixtuoid/src/install/` | local | Enumerate every resolution axis and re-verify each against that CLI's upstream in-session; write ⊆ verify (#338). |
| A new source / hook integration | `crates/pixtuoid-core/src/source/registry.rs` | local | LIVE run or hermetic replay without capture-rig convenience flags; event shapes from canonical upstream docs, never a fork. |
| A dedup / "behavior-preserving" refactor | | | Adversarial toward revert, per consolidation: one reason-to-change per call site; name the conversions that moved semantics — a batch hides exactly one (#461). |
| A physical/domain feature, or an arc's last PR | | | Enumerate the domain invariants and re-derive each across the parameter space (#471). |

## Severity

[Conventional Comments](https://conventionalcomments.org/) labels; nits and
taste are never posted. Dispositions:
[CONTRIBUTING](docs/CONTRIBUTING.md#pull-requests).

- `issue (blocking)` — correctness, security or invariant. It blocks only once
  the orchestrator or a maintainer confirms it against the code; the finder's
  label alone never blocks.
- `issue (non-blocking)` — any other real defect this PR introduced.
- `issue (pre-existing)` — real, not introduced here.

Until `review-schema.json`'s enum follows, a bot's `severity` is `HIGH` for
blocking and `MEDIUM` otherwise.

## Output

Bots return only the structured result
[`review-schema.json`](.github/prompts/review-schema.json) defines, and never
post or call GitHub APIs:

- `summary`: one sentence.
- `path`: repository-relative (the `b/` side of `pr.diff`), never absolute.
- `line`: the absolute head-side line, never invented; when unsure, the
  nearest line read, with the location described in the body.
- At most the schema's `maxItems` findings, blocking first, pre-existing last.
- Each `body` opens with its label, then the verified finding and a concrete
  failure scenario.
