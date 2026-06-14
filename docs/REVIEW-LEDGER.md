# REVIEW-LEDGER — adjudicated whole-codebase-review findings

The institutional record of every adjudicated review finding (confirmed, refuted,
accepted-as-residual) from the whole-codebase reviews and review-shaped arcs.
Purpose: future reviews stop re-paying for the same refutations — and stop
trusting stale ones. A refutation is a claim about the code **as it stood at a
HEAD**; the code moves, so this ledger is premise-anchored: every row carries
the seam, the exact claim, the anchor paths/mechanism the verdict rested on,
and the HEAD it was rendered at.

## Usage protocol (for every future review run)

1. **Dedup stage**: match each candidate finding against this ledger by
   **seam + claim** (not seam alone — see Notable flips below).
2. **A match against a REFUTED row does NOT kill the candidate — it DEMOTES
   it.** Run `git diff <verdict-head>..HEAD -- <anchor paths>`. If the anchors
   changed, the entry is **EXPIRED** for this candidate: full verification,
   with the old entry attached as context only. If the anchors are unchanged,
   route the candidate to a single cheap checker agent asking exactly: *"does
   the cited mechanism still refute THIS claim?"* The June-9/June-10
   socket-steal case (below) is WHY: same seam, different claim — one refuted,
   one REAL.
3. **Tiering**: only verdicts citing a documented sharp edge or a merged PR
   get the demote fast-path (**tier A**). Judgment-call verdicts
   ("accepted residual", grouped-summary rows, reconstructed mechanisms) are
   **tier B** = always fully re-verify.
4. **A match against a CONFIRMED-fixed row becomes a regression check**:
   verify the fix PR's mechanism is still present at the anchor paths — not a
   re-derivation of the original bug.
5. **Every NEW design-intent refutation MUST cite an existing sharp edge**
   (workspace or nested `CLAUDE.md`, or an in-code WHY doc) **or add one in
   the same PR**. No undocumented refutations enter this ledger.
6. **Append-only.** A flipped verdict appends a superseding row that links the
   old one; the old row is never edited or deleted.
7. **After adjudication, append**: a new dated `##` section, one row per
   adjudicated finding (same 8 columns), ID prefix `RMMDD-NN` (date-based);
   grouped rows only where individual claims weren't preserved (those are
   tier B by definition).
8. **Calibration**: every Nth review runs ledger-blind. A finding the ledger
   would have demoted but the blind run confirms = a false suppression —
   record the rate in `docs/review-metrics/`.

Head notation: `(n/r)` = HEAD not recorded in the backfill source — treat
anchor expiry as tier B.

Verdict vocabulary: `CONFIRMED→PR#N` (optionally via a deferral issue),
`REFUTED-design` (matched documented intent), `REFUTED-wrong-premise`
(the claim's factual premise was false), `ACCEPTED-residual` (real, consciously
not fixed; WHY documented).

---

## Notable flips — premise drift in action

**The socket-steal seam (`crates/pixtuoid-core/src/source/hook/unix.rs`,
liveness arbitration) — the canonical example.** Same seam, three adjudications
in three days (plus the documented mixed-version residual), verdicts in both directions:

1. *2026-06-09 review @151e38d* — a socket-steal-shaped candidate at this seam
   fell into the refuted set (the then-current stale-socket reclaim was the
   documented design). Not individually preserved — it lives only in the
   grouped 26-refuted record (row R0609-17), which is exactly the
   record-keeping failure this ledger exists to fix.
2. *External review finding #11* — CONFIRMED: the bind path **silently stole a
   live daemon's transport**, and the alternative bail killed the whole CC
   source. Fixed in PR #232 (connect-errno probe → typed `SocketBusy` →
   transcript-only degradation). Merged 2026-06-11T02:31Z.
3. *2026-06-10 review @7bc2777 — hours after #232 merged* — CONFIRMED
   (MEDIUM): **the probe itself misclassifies**. The repo's own kernel note
   (`pixtuoid-hook/tests/shim.rs` stalled-listener test) documents that a
   backlog-saturated LIVE daemon yields `ECONNREFUSED` on macOS / `EAGAIN` on
   Linux — so the errno probe steals from a live-but-saturated owner. Fixed in
   PR #235: exclusive advisory lock on a sibling `<sock>.lock` held for the
   daemon's lifetime; connect errnos never decide liveness for a lock-holding
   owner (now a sharp edge in `crates/pixtuoid-core/CLAUDE.md`'s `hook/`
   entry, with the mixed-version lock-less residual honestly documented).

   Lesson: a refuted match must be **demoted, not killed** — and a *fix* is
   itself a premise that the next review may legitimately overturn.

**Two grounded sibling examples (same seam, different claim, opposite
verdicts):**

- *Install lock* (`crates/pixtuoid/src/install/io.rs`): June-9 REFUTED
  "advisory `try_lock` fail-on-contention is a bug" (it is documented design —
  io.rs doc: "FAIL on contention rather than block"); the external review
  CONFIRMED "the lock doesn't cover the whole read→merge→write round"
  (lost-update TOCTOU) → PR #229's `ConfigLock` RAII. Both verdicts correct.
- *Resurrect-in-place* (`state/fsm.rs` + reducer): June-9 REFUTED
  "resurrect-in-place active_tasks bypass" (matched a documented edge) while
  the SAME review CONFIRMED "resurrect drops the open Active span" → PR #210;
  then the external review CONFIRMED a third claim — "resurrect leaves the
  dead life's correlation state, suppressing the new life's hooks" → PR #232.
  Three claims, one seam, three different correct verdicts.

---

## 2026-06-07 — 0.6.0 sweep (v0.5.0..HEAD; 3 engines: bug / refactor / cargo-mutants + adversarial verify)

Source: maintainer memory (`project_codebase-sweep-060`). Engine totals:
bugs 3 MED + 12 low confirmed, 2 rejected; refactor 1 HIGH + 2 MED confirmed,
21 dropped; mutants 86/98 caught. Headline: 23 candidates rejected by
adversarial verify = "very clean".

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| S60-01 | logging `make_filter` (try_from_default_env) | empty `RUST_LOG` silently turns ALL logging off (errs only when UNSET; empty = zero directives) | CONFIRMED→PR#172 | PR #172 (5b6193c): pure `filter_directives` helper; empty = unset policy (reused by #235/#236 env contracts) | A | 0.6.0-dev (n/r) | 2026-06-07 |
| S60-02 | `Activity` enum + `render_to_rgb_buffer` monolith | dead `Activity` (Typing/Reading) feeds nothing; 735-line enqueue monolith | CONFIRMED→PR#173 + PR#174 | PR #173 (8421e73, ~90 sites, golden byte-identical), PR #174 (24495a6, 9 named phases, 60-frame byte-identity matrix) | A | 0.6.0-dev (n/r) | 2026-06-07 |
| S60-03 | `state/{reducer,scope,fsm}` mutation survivors + API hygiene | 12 mutants survive; tuning consts public; event enums not `#[non_exhaustive]` | CONFIRMED→PR#171; partial ACCEPTED-residual | PR #171 (082b32c): 6 survivors killed, 3 boundary mutants documented-equivalent; `AgentEvent`/`ToolDetail` non_exhaustive; `AgentSlot`/`SceneState` REFUSED (literal-constructed in ~12 binary files — disproportionate churn) | A | 0.6.0-dev (n/r) | 2026-06-07 |
| S60-04 | reducer `detail == ToolDetail::Task.display()` | "magic string" Delegating comparison should be a typed state | REFUTED-wrong-premise (+ROI) | one single-source comparison, both sides reference the same enum method — no drift risk; clean fix = 63 construction-site edits + touches the `detail: Arc<str>` cheap-clone invariant | B | 0.6.0-dev (n/r) | 2026-06-07 |
| S60-05 | (grouped) 23 rejected candidates across both engines | various behavior-risk / churn refactor and bug candidates | REFUTED-design (grouped) | adversarial-verify rejections (behavior-risk / intentional-design / not-worth-churn); individual claims not preserved | B | 0.6.0-dev (n/r) | 2026-06-07 |

## 2026-06-09 — 5-lens improvement audit → PR #208

Source: maintainer memory (`project_improvement-audit-pr208`). Architecture
lens returned zero findings (verified correct, not lazy).

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| A08-01 | workspace `rust-version` + fs4 lock sites | MSRV 1.78 is fiction (serde_spanned needs edition2024 ⇒ Cargo ≥1.85); fs4 redundant once std file locks exist | CONFIRMED→PR#208 (closes #194) | PR #208 (151e38d): MSRV→1.89, fs4 dropped for std `File::try_lock`, `just msrv` CI gate; contention parity proven by a 2-handle test | A | pre-#208 (n/r) | 2026-06-09 |
| A08-02 | `SceneLayout::compute_with_seed` per-frame call (tui renderer) | per-frame layout recompute is a perf win waiting on a cache | REFUTED-wrong-premise | profiled 12–42µs/call = 0.04–0.13% of a 33ms frame, visible floor only — cache adds stale-on-resize risk for nothing | B | pre-#208 (n/r) | 2026-06-09 |

## 2026-06-09 — whole-codebase review @151e38d (16-reviewer Workflow) → PR #210 (+ #218, #219)

Sources: maintainer memory (`project_codebase-review-2026-06-09`,
`project_review-followups-pr210`) + PR #210 body. 49 candidates → 23 confirmed
(2 MED + lows) → PR #210 (merged 77c5ef5, 2026-06-09) + 2 deferrals; 26
refuted, killed by the design-intent skeptic against documented sharp edges.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0609-01 | `tui/widgets/tooltip.rs:76` disambig label | MED: `&session_id[..4]` byte-slice panics per-frame when a Reasonix cwd-id splits a UTF-8 codepoint at byte 4 | CONFIRMED→PR#210 | PR #210: `chars().take(4)`; mechanism later superseded by PR #232's 4-hex digest (see EXT-08) | A | 151e38d | 2026-06-09 |
| R0609-02 | `runtime/driver.rs:161` headless summary | MED: untrusted label/detail/Notification reason printed verbatim → ANSI/OSC escape injection in headless mode | CONFIRMED→PR#210 | PR #210: `sanitize_line` (strips `is_control()`) in `summarize` | A | 151e38d | 2026-06-09 |
| R0609-03 | `tui/widgets/hud.rs:285` footer | footer width/pad measured in bytes, not display columns (multi-byte ·/×/↑↓ over-counted; `[q]uit` not flush-right) | CONFIRMED→PR#210 (closes #211) | PR #210: `chars().count()`; first dropped for baseline churn, then RESTORED (0f5a588) + 24 baselines regenerated; tui/CLAUDE.md "byte width"→"column width" | A | 151e38d | 2026-06-09 |
| R0609-04 | `reducer.rs:374` resurrect | resurrect-in-place sets `state=Idle` directly, dropping the open Active span from `active_ms` | CONFIRMED→PR#210 | PR #210: `fsm::resurrect_in_place` folds the span; sharp edge now in core CLAUDE.md ("SessionStart on an EXITING root slot cancels the walkout") | A | 151e38d | 2026-06-09 |
| R0609-05 | `config.rs:147` + `install/claude.rs:112` (parallel-impl drift) | `update_config` skips the fsync `io.rs::write_config_atomic` does; `merge_install` silently coerces a non-object settings doc to `{}` | CONFIRMED→PR#210 | PR #210: fsync parity; bail on valid-JSON-but-non-object (codex immune — TOML doc is always a table). Write-path later fully consolidated by PR #229 | A | 151e38d | 2026-06-09 |
| R0609-06 | `tui_renderer/mod.rs:723/:657` cross-frame state | stale `last_popup_scale` on footer-only early-return; MotionState evicted on current floor only (leak) | CONFIRMED→PR#210 | PR #210: zero on Ok(None)/Err; evict across ALL floors (+ regression test). PoseHistory half resurfaced in the external review (EXT-17 → PR #233) | A | 151e38d | 2026-06-09 |
| R0609-07 | decode/format input-robustness lows | antigravity negative `step_index` mints unmatchable id; sprite hex accepts `+` prefix; final-frame parse error lacks line context; `RgbBuffer::get/put` unbounded | CONFIRMED→PR#210 | PR #210: `.filter(s>=0)`, `is_ascii_hexdigit` gate, error context, `debug_assert` bounds | A | 151e38d | 2026-06-09 |
| R0609-08 | `jsonl` oversized-skip path | >1MiB JSONL tail-skip can drop a buried CC SessionEnd terminator → slow stale-sweep only | CONFIRMED→PR#210 | PR #210 #8: known-file oversized skip tail-scans `check_session_ended` before EOF-seek | A | 151e38d | 2026-06-09 |
| R0609-09 | `source/mod.rs` `Transport` enum | `Transport` not `#[non_exhaustive]` — adding a variant is a needless semver break | CONFIRMED→#212→PR#218 | deferred for a HARD constraint (semver-checks baselines against crates.io 0.6.1 → would red every PR until release); shipped with the 0.7.0 bump (450888a) | A | 151e38d | 2026-06-09 |
| R0609-10 | desk-index / floor-index domain quantities | raw `usize` mixes global vs floor-local desk indices | CONFIRMED→#209→PR#219 | deferred (large cross-crate refactor); PR #219 (a509aa2) `GlobalDeskIndex`/`FloorLocalDeskIndex` — surfaced + fixed 2 real bugs | A | 151e38d | 2026-06-09 |
| R0609-11 | `reducer.rs` `sweep_stale` pass-2 | pass-2 already-cascaded skip is redundant | ACCEPTED-residual | OMITTED as un-pinnable: `mark_exiting` is already write-once, so removing the line changes no observable state — a regression test can't fail-first; documented in the PR #210 commit, not a vacuous test | B | 151e38d | 2026-06-09 |
| R0609-12 | reducer negative-branch test gaps | NEGATIVE/write-once/dedup-skip branches untested (subagent-resurrect gate, ghost-label-after-drop, capacity-dropped ordinal) | CONFIRMED→PR#210 | PR #210: +12 regression tests incl. `exiting-subagent-doesn't-resurrect`, `capacity-dropped-no-ghost-ordinal`, motion-eviction-on-non-current-floor | A | 151e38d | 2026-06-09 |
| R0609-13 | resurrect path vs `active_tasks` | resurrect-in-place bypasses `active_tasks` (subagent-leak suppression) | REFUTED-design | matched a documented sharp edge per the review record; the exact edge text cited then is not preserved and the seam was REWORKED by PR #232 (anchors changed → treat as EXPIRED; see Notable flips) | B | 151e38d | 2026-06-09 |
| R0609-14 | `crates/pixtuoid/src/version.rs::parse_semver` | prerelease identifiers collapse (intra-prerelease ordering ignored) | REFUTED-design | in-code WHY doc at `parse_semver`: 4th tuple component is 0 for prerelease / 1 for release so `0.5.0-rc1 < 0.5.0` per semver precedence — intra-prerelease ordering deliberately unmodeled | A | 151e38d | 2026-06-09 |
| R0609-15 | hook listener detached per-conn tasks | detached tasks are never drained/joined (events lost on shutdown) | REFUTED-design | review record: matched a documented edge; refuting doc not individually recoverable — mechanism today: permit-RAII + `CONN_TIMEOUT`-bounded `tokio::spawn` in `hook/unix.rs::run` | B | 151e38d | 2026-06-09 |
| R0609-16 | `install/io.rs::lock_config` | advisory `try_lock` failing on contention (instead of blocking) is a bug | REFUTED-design | io.rs doc: "FAIL on contention rather than block — `try_lock` returns `Err(TryLockError::WouldBlock)`"; parity preserved through the fs4→std migration (PR #208). Same seam later yielded a REAL confirmed claim (EXT-03 — see Notable flips) | A | 151e38d | 2026-06-09 |
| R0609-17 | (grouped) remaining ~22 refuted candidates | various — incl. detached-tasks/dedup/lifecycle probes | REFUTED-design (grouped) | killed against then-documented sharp edges (per-crate CLAUDE.md catalogs verified "load-bearing AND accurate"); individual claims not preserved — the gap this ledger closes | B | 151e38d | 2026-06-09 |

Findings #15/#18 of the review's numbering were "out of the agreed bundle" and
their content is not preserved in any source — deliberately NOT backfilled.

## 2026-06-10/11 — external 34-finding review → PRs #229–#233

Sources: maintainer memory (`project_external-review-fixes`) + PR bodies.
Triage at then-HEAD post-#227 (588f78d): 27 valid, 4 already-fixed, 3 duplicates/out-of-scope (not individually preserved), **0 hallucinated**.
External finding numbers (#N) follow the external report.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| EXT-01 | `config.rs` boot `save_version` + `update_config` (#3) | a one-char config typo + the automatic boot version-save silently wipes the whole hand-written config to defaults | CONFIRMED→PR#229 | PR #229 (75653d9): refuse to persist over a file failing TOML or typed `AppConfig` parse; one-time `.pixtuoid.bak` | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-02 | config write path (#15, #16) | saves destroy comments/unknown keys; a second hand-rolled tmp+rename write authority drifts from `io.rs` (no fsync, lock-unlink race) | CONFIRMED→PR#229 | PR #229: `toml_edit::DocumentMut` round-trip (new dep — toml 1.x no longer ships it transitively); ONE write authority via `install/io.rs` | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-03 | `install/io.rs` lock scope (#7) | the advisory lock doesn't cover read→merge→write — concurrent installs lose updates (TOCTOU) | CONFIRMED→PR#229 | PR #229: `ConfigLock` RAII held across the whole round; `write_config_atomic` = `lock_config().write_atomic()`. Counterpart refutation: R0609-16 (see Notable flips) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-04 | `install/io.rs` perms (#6) | rewrites silently widen a 0600 settings.json (API keys) to 0644 | CONFIRMED→PR#229 | PR #229: target mode preserved; new files 0600; tmp private from open | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-05 | hook-path / home resolution (#19, #20) | explicit `--hook-path` not honored for Claude on Unix; failed home resolution silently writes `./.reasonix/settings.json` nothing reads | CONFIRMED→PR#229 | PR #229: absolutized + quoted embed (relative resolves against CC's cwd at hook time); hard error "pass --config" | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-06 | `reducer.rs` `enter_delegating` (#4) | an out-of-dedup-window JSONL Task replay clobbers a Waiting parent | CONFIRMED→PR#232 | PR #232 (7bc2777): fire only on FIRST insert into `active_tasks`; sharp edges re-verified (#150 asymmetric dedup, `B1_CASCADE_GRACE`). NOTE: drain re-insert re-opened a lagged-replay hole → R0610-08 (PR #234 90s drained-tuid tombstone) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-07 | `hook/unix.rs` bind path (#11) | the Unix hook socket silently steals a live daemon's transport; the bail alternative kills the whole CC source + Reasonix (review MEDIUM on blast radius) | CONFIRMED→PR#232 | PR #232: connect probe → typed `SocketBusy` → transcript-only degradation + headless source-death surfacing. SUPERSEDED at the probe level by R0610-02 → PR #235 flock (see Notable flips) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-08 | `emit_first_sight` id derivation (#13) | hook- and JSONL-created slots carry different ids (Codex JSONL disambiguator was the constant `roll`) | CONFIRMED→PR#232 | PR #232: derive via the source's id deriver; tooltip disambig → 4-hex digest of the whole id (only shape-agnostic choice — rx cwd ids collide head AND tail) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-09 | `hook/unix.rs` socket perms (#17) | process-global umask dance races every other tokio worker's file creation | CONFIRMED→PR#232 | PR #232: temp-bind + chmod 0600 + atomic rename (+ `sun_path` length guard fallback); now a core CLAUDE.md hook/ sharp edge | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-10 | resurrect-in-place correlation state (#21) | a leftover tuid in `active_tasks`/`gated_before_waiting`/`pending_b1_cascades` suppresses every hook event of the resurrected life forever | CONFIRMED→PR#232 | PR #232: evict on resurrect; `recent_proof_of_life` deliberately survives (process alive by definition). Core CLAUDE.md resurrect edge. Counterpart refutation: R0609-13 (see Notable flips) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-11 | (review-round, PR #232) 4 reviewer counter-findings | insert-gate breaks #222 seeding / negative-vouch drains / refused-desk orphans; rename drops the listener; uuid-v7 tails collide; resurrect-eviction breaks the b1 trade | REFUTED-wrong-premise | PR #232 body: "Refuted with full traces" — e.g. old children are already cascade-exited, never 30-min lingerers | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-12 | `pixtuoid-hook` shim cluster (#26–#28 area) | non-UTF-8 argv panics (exit 101, visible to CC); near-1MiB payload stalls past the watchdog on Windows; spoofed inbound `_pixtuoid_source` mis-namespaces an AgentId | CONFIRMED→PR#231 | PR #231 (bae3541): `args_os` lossy; `STAMP_HEADROOM = 256`; unconditional inbound strip. Invariant #5 (always exit 0 silently) is the cited contract | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-13 | CLI/runtime cluster (#5, #10, #14, #18 area) | `--max-desks 0` = permanently empty office; `ctrl_c` recreated per-iteration swallows a SIGINT; `--headless` help claims JSON; `--log-level` typo silently disables logging | CONFIRMED→PR#231 | PR #231: parse error + pre-altscreen warn; pinned future; help text; `ValueEnum` | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-14 | CI/scripts/docs cluster (#29, #30, #34–#36 area) | smoke's `\|\| true` makes the invariant-#5 gate unfalsifiable; drift-script files junk issues on transient net errors; `just fuzz` "passes" on an empty corpus; gen-media validates jobs after the release build; 2 misleading code docs | CONFIRMED→PR#231 | PR #231: assert silence; transient bucket routing + exit 2; loud empty-corpus fail; early validation; physics/pathfind doc fixes | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-15 | `layout` `push_desk` (#8) | at 34–66px buffer widths the forced pod's second desk column lands past the band edge / off-buffer (invisible desks, out-of-mask anchors) | CONFIRMED→PR#233 | PR #233 (8a7046e): x clamp mirroring the y clamp; degrade to fewer desks; capacity from real desk count. Twin clamps followed: R0610-11 (#237 aisle slots), #239→PR #240 (south edge) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-16 | tui motion (#22, #23, #24) | thinking-gate release teleports into mid-walk; snap-back speed from straight-line not A* length; `pick_aimless_dest` can return a blocked cell | CONFIRMED→PR#233 | PR #233: SeatedIdle for the cycle; route once at arm + measure polyline (leg-freeze #66/#68 pins untouched); walkable fallback + `home_desk` param (core break absorbed by the 0.7.0 window) | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-17 | `evict_missing` (#25) | PoseHistory has NO eviction (one entry per AgentId ever rendered, forever); MotionState current-floor-only | CONFIRMED→PR#233 | PR #233: evict both across every floor. Overlaps R0609-06's MotionState half (PR #210) — the PoseHistory half was the net-new valid part at triage | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-18 | whiteboard walkable strip (#31) | TopLeft south-strip blocked ground sat 2px west of the wheels (east wheel column walkable, bare floor blocked) | CONFIRMED→PR#233 | PR #233: center the narrower footprint; 2 walkable goldens regenerated, diff confined; `just gen-check` 29/29 | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-19 | sprite format/painter guards (#32, #33) | `rows_to_frame` silently truncates past `u16::MAX`; `draw_dotted_hline` can overflow / not terminate on `dash=0,gap=0` | CONFIRMED→PR#233 | PR #233: bail on oversize; compute in u32; `blit_frame_outlined` halo contract documented | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-20 | `.github/workflows/release.yml` (#215) | workflow-wide write perms + a tag at an arbitrary unreviewed commit publishes | CONFIRMED→PR#230 (closes #215) | PR #230 (0fb73fd): top-level `contents: read`, per-job escalation; tag-on-main ancestry check (`v*-win.N`/`-rc.N` exempt by design); OIDC prep inert until registry dashboards | A | post-#227 (588f78d) | 2026-06-11 |
| EXT-21 | (grouped) 4 findings already fixed at triage | external #1 (mid-attach visibility), #2+#9 (PR #210 class), #12 (liveness follow-up class) | CONFIRMED→PR#224 / PR#210 / PR#227 (pre-dated) | triage verdict: fixed by the liveness arc before the report landed — d1283aa, 77c5ef5, 588f78d | A | post-#227 (588f78d) | 2026-06-11 |

## 2026-06-10 — whole-codebase review @7bc2777 (16 finders → design-intent + code-trace verifier pairs) → PRs #234–#237, #240

Sources: maintainer memory (`project_codebase-review-2026-06-10`) + PR bodies.
42 candidates → 37 distinct → 25 confirmed (0 crit / 0 high / 6 MED / 19 low),
12 refuted (10 by design). All 25 shipped 2026-06-11 (#234 f4c06fd, #235
09471c8, #236 dcc9851, #237 1d4fa7d); residuals #238/#239 closed by #240
(8ed424c).

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0610-01 | `jsonl.rs:346` `emit_session_exit` / `ctx.live` (MED) | instant-exit never purges the dead id from the live admission set → a probe-failure (`None`) pass lets `revouch_gated_files` re-admit the just-ended session → cursor 0 → full-replay ghost SessionStart unreachable by every fast rung | CONFIRMED→PR#234 | PR #234 (f4c06fd): purge in `emit_session_exit` (one exit path); negative-vouch confirm immune by construction (runs after `live` is replaced) | A | 7bc2777 | 2026-06-10 |
| R0610-02 | `hook/unix.rs:38` liveness arbitration (MED) | backlog-saturated LIVE daemon yields ECONNREFUSED (macOS) / EAGAIN (Linux) → #232's errno probe steals the live socket / dies fatally | CONFIRMED→PR#235 | PR #235 (09471c8): flock on sibling `<sock>.lock` for daemon lifetime (also closes the rename TOCTOU); kernel note in `pixtuoid-hook/tests/shim.rs`; now the core CLAUDE.md hook/ sharp edge, mixed-version lock-less steal = documented residual. See Notable flips | A | 7bc2777 | 2026-06-10 |
| R0610-03 | `hook/windows.rs:115` (MED) | SocketBusy degradation is Unix-only — Windows ACCESS_DENIED stays fatal, second instance kills the whole CC source incl. watcher | CONFIRMED→PR#235 | PR #235: map ACCESS_DENIED → typed `SocketBusy` → same transcript-only degradation; cfg(windows) twin in `tests/transport/pipe.rs` | A | 7bc2777 | 2026-06-10 |
| R0610-04 | `crates/pixtuoid-core/CLAUDE.md:21` hook/ entry (MED) | doc claims a busy socket is "surfaced via SourceDeath" while #232's shipped code degrades transcript-only with NO SourceDeath — an agent could "fix" the degradation away to match the doc | CONFIRMED→PR#235 | PR #235: entry rewritten to shipped behavior (docs are load-bearing here — design-intent skeptics refute against them) | A | 7bc2777 | 2026-06-10 |
| R0610-05 | `tui/pathfind.rs:74` `AStarRouter` (MED) | aimless wander (fresh random dest/cycle) + snap-back from live interpolated origins insert ever-new (from,to) keys — no cap, no evict hook, linear retain scan | CONFIRMED→PR#237 | PR #237 (1d4fa7d): `PATH_CACHE_CAP = 512`, clear-on-exceed; mid-leg clear safe because legs are frozen on `walk_path` (workspace CLAUDE.md sharp edge: A* polyline frozen once per leg) | A | 7bc2777 | 2026-06-10 |
| R0610-06 | `tests/transport/socket.rs:79` (MED) | `listener_drops_slow_connection_via_timeout` (+ pipe twin) passes with `CONN_TIMEOUT` deleted — the semaphore already prevents accept-blocking | CONFIRMED→PR#235 | PR #235: assert EOF on the slow conn; mutation-verified (red with the timeout wrapper removed) | A | 7bc2777 | 2026-06-10 |
| R0610-07 | (low theme) liveness edges: `jsonl.rs:608`, `codex.rs:300`, `jsonl.rs:622` | pid-rebind emits a wrong instant SessionEnd for a live resumed session; `CODEX_HOME` users silently lose the whole liveness ladder; watcher goes silently blind on an unreadable root | CONFIRMED→PR#234 | PR #234: unbind-before-insert pid migration; `codex_probe_root` honors resolved `codex_home()`; `FailureLatch` once-per-state-change warn + recovery log | A | 7bc2777 | 2026-06-10 |
| R0610-08 | (low theme) reducer/scope: `reducer.rs:955`, `scope.rs:106` | lagged Task-pair replay re-fires `enter_delegating` (re-opens the gate #232 closed — drained tuid re-insert counts as "first"); `parent_id` cycle = immortal self-exempting slots | CONFIRMED→PR#234 | PR #234: 90s drained-tuid tombstone (tuids never legitimately re-dispatch); `has_ancestor_where` seeds visited with the start node. Mutual-2-cycle residual → R0610-15 | A | 7bc2777 | 2026-06-10 |
| R0610-09 | (low theme) config/install env contracts | empty `PIXTUOID_SOCKET` honored verbatim; empty `XDG_CONFIG_HOME` → CWD-relative config; `PIXTUOID_HOOK` bypasses the `--hook-path` absolutize arm; `ConfigLock` re-resolves symlinks mid-round; `CMD_UNSAFE` misses tab `;` `,` `=`; drive-relative `C:foo.exe` silently no-ops | CONFIRMED→PR#235 + PR#236 | PR #235 (socket env, both shim+daemon, #172 empty=unset policy); PR #236 (dcc9851): XDG empty=unset, env flows through the flag arm + embeds like it `(path, explicit)`, `ConfigLock.read()/backup_once()` pinned to lock-time resolution, delimiter set, hard error; ONE trim-based `io::nonempty` helper | A | 7bc2777 | 2026-06-10 |
| R0610-10 | (low theme) runtime/TUI signals | no SIGINT/SIGTERM altscreen teardown (`tui/mod.rs:232` — external kill leaves raw mode + mouse reporting); headless `ctrl_c` Err arm silently exits 0 | CONFIRMED→PR#236 | PR #236: signals ride the frame-pacing select → `teardown_terminal`; Err arm logs + disarms via pending-future swap (`headless_loop_with_signal` seam) | A | 7bc2777 | 2026-06-10 |
| R0610-11 | (low theme) TUI/layout visuals: `hud.rs`, `layout/compute.rs` | elevator HUD centers/clips by byte length (multi-byte ▲/▼ shift it 2 cells); pod-decor horizontal-aisle slot lands off-buffer at 34–41px widths | CONFIRMED→PR#237 | PR #237: `chars().count()` (the PR #210 convention; baselines regenerated in-PR, 632px/0.04% delta); `push_slot` skip past `band_right` — mirror of #233's `push_desk` clamp | A | 7bc2777 | 2026-06-10 |
| R0610-12 | (low theme) test quality + docs drift ×3 | degradation test rides real FSEvents (nondeterministic); workspace CLAUDE.md claims snap-back is time-compressed (it runs pure physics); ARCHITECTURE.md misses `decode_hook_payload`'s `Vec<AgentEvent>`/Identity prepend + claims Unix-socket-only | CONFIRMED→PR#235 + PR#237 | PR #235: documented polling seam; PR #237: snap-back edge corrected (verified in `pose/mod.rs` first), ARCHITECTURE.md + README transport split | A | 7bc2777 | 2026-06-10 |
| R0610-13 | hook socket location `/tmp/pixtuoid-<uid>.sock` | socket in world-writable `/tmp` is a vulnerability | REFUTED-design | security boundary is the socket itself, not the dir: bound at a temp name, chmod 0600, atomic rename (core CLAUDE.md hook/ edge; `pixtuoid-hook/src/paths.rs` — uid-suffixed, `XDG_RUNTIME_DIR` preferred when set) | A | 7bc2777 | 2026-06-10 |
| R0610-14 | `hook/unix.rs::run` accept loop | accept-error arm (warn + continue) hot-spins under EMFILE | REFUTED-design | memory records "refuted by design"; mechanism reconstructed at backfill: semaphore permit acquired BEFORE `accept()` bounds in-flight conns (`MAX_CONCURRENT_CONNS`), so the listener can't exhaust its own FDs — re-verify against the cited code, not this note | B | 7bc2777 | 2026-06-10 |
| R0610-15 | (grouped) remaining 10 refuted candidates | various (8 more by-design + 2 other) | REFUTED-design / REFUTED-wrong-premise (grouped) | killed against CLAUDE.md sharp edges per the review record; individual claims not preserved | B | 7bc2777 | 2026-06-10 |
| R0610-16 | `scope.rs` mutual-Waiting 2-cycle (residual from #234's review) | a mutual-Waiting `parent_id` 2-cycle still mutually self-exempts from `sweep_stale` (immortal pair) | CONFIRMED→#238→PR#240 | PR #240 (8ed424c): `scope::would_create_cycle` filters at the reducer's ONE `parent_id` write seam (registration + orphan enrichment; Codex SubagentStart flows through it) — prevention over detection, sweeps need no cycle awareness | A | f4c06fd | 2026-06-11 |
| R0610-17 | `layout` pod-decor south edge (found in #237's review, pre-existing) | south decor visual crosses the cubicle-band bottom into the walkway on tall floors (repro 200×116 seed 2) | CONFIRMED→#239→PR#240 | PR #240: `push_slot` vertical twin of #237's east clamp. Premise corrected en route: issue said "south-anchored" but that described the FOOTPRINT — the visual blit is CENTER-anchored; clamp derived from real blit math (footprint ⊆ visual) | A | 1d4fa7d | 2026-06-11 |
| R0610-18 | (fix-arc bot round, PR #236) `update_config` read path | `update_config`'s read should go through the lock-pinned `ConfigLock::read` (the one REAL bot finding of the 4-PR round) | CONFIRMED→PR#236 | PR #236 review round; #234/#237 bot rounds clean, one #235 bot finding self-refuted (claim not preserved) | B | 7bc2777 | 2026-06-11 |

## 2026-06-11 — subagent-lifecycle arc adjudications (#241–#250)

Sources: maintainer memory (`project_cc-subagent-hooks-gap`) + PR bodies
#243/#245/#248/#249/#250. These are review-round and issue adjudications made
during the arc, including upstream-verified premise corrections.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| SUB-01 | `decoder.rs:717` CC SubagentStart/Stop narrowing | Workflow-fleet subagents have NO clean exit: b1 keys on `Agent` tool_use drain (a fleet is ONE `Workflow` call), no transcript end marker, hooks Codex-narrowed (pre-Workflow YAGNI) → batch sweep-reaps hold every desk | CONFIRMED→#241→PR#243 | PR #243 (e2736c5): `decode_cc_hook_custom` — Start→instant SessionStart (`agent-<bare_id>` + parent link), Stop→SessionEnd via `cc_id_from_path(agent_transcript_path)`; install EVENTS=7; drift watcher diffs the hooks.md event LIST. Verified: 7/7 real-transcript replay lab + 269,804-line fuzz | A | post-#240 (8ed424c) | 2026-06-11 |
| SUB-02 | `Workflow` in `make_tool_detail` | (fix-shape adjudication) mapping `Workflow`→`ToolDetail::Task` would fix the fleet exits | REFUTED-design | deliberately NOT mapped — the vouched-Delegating subtree shield would sweep-EXEMPT finished fleet agents for the workflow's whole life (worse starvation); WHY at `make_tool_detail` + workspace CLAUDE.md sharp edge ("Workflow … deliberately NOT mapped") | A | post-#240 (8ed424c) | 2026-06-11 |
| SUB-03 | (bot MEDIUM, PR #243 review) `agent_transcript_path` as slot key | keying SessionEnd on an untrusted hook-supplied path is a new spoof surface | REFUTED-wrong-premise | the same capability always existed via plain SessionEnd forgery on the 0600-mode socket — no NEW surface (socket perms edge: core CLAUDE.md hook/ entry); recorded in the arc memory | B | e2736c5 | 2026-06-11 |
| SUB-04 | hook reorder, CC + Codex twins | `SubagentStop` decoded BEFORE its `SubagentStart` mints a slot whose end already passed → lingers to the 10/30-min sweeps (tombstone only gated Activity synthesis) | CONFIRMED→#242→PR#245 | PR #245 (04d0ab5): one reducer SessionStart gate — `parent_id.is_some() && !slot_exists && tombstoned` → skip; child-scoped so Reasonix's documented parentless SessionEnd→SessionStart resurrect is exempt by construction; 5s TTL-bounded | A | e2736c5 | 2026-06-11 |
| SUB-05 | residual windows characterized in #245's review | Codex parentless first-sight bypasses the gate; known-id post-GC late discovery re-registers; post-TTL registration | ACCEPTED-residual→#244→PR#249 | PR #249 (17347a4): child ledger (`as_child` stamp on SessionEnd — only the two SubagentStop arms; ledger {applied parent, ended_at, 90s TTL}; parented-start skip closes w2, parentless adopt-don't-block re-link improves w1); post-TTL stays sweep-owned by design | A | 04d0ab5 | 2026-06-11 |
| SUB-06 | Codex multi-turn child re-registration (#246) | child fires SubagentStop per TURN → ended after turn 1; re-registration loses the parent link; NO SessionStart carrier exists at turn N+1 on either transport | CONFIRMED→PR#249 + PR#250 | PR #249 re-link + PR #250 (deb5d77) un-claim side-channel (tee in `ClaudeCodeSource::run`, watcher drains own claims, stragglers to EOF first per #228). KEY premise correction in #249's review: a live codex-rs source check CORRECTED the PR's original multi-turn claim pre-merge. KEY interaction: release = `seen→false` NOT removal — the FD probe vouches a LIVE child's open rollout; removal triggers `revouch_gated_files` full-replay (pinned: `released_claim_is_not_revouched_into_a_full_replay`) | A | 17347a4 | 2026-06-11 |
| SUB-07 | `sessions/<pid>.json` registry consumption (#247) | the probe's one undocumented upstream dependency has no drift detector — a silent key rename degrades mid-attach liveness to mtime-only with zero signal | CONFIRMED→PR#248 | PR #248 (5ec866b): `RegistryParse` Entry/Skip/`ShapeDrift(key)` + warn-once naming the vanished key; 13-key shape pin. Fix-approach "extend `check_upstream_drift.py`" REFUTED-design in the same PR: the script is strictly fetch-and-diff (no local state; CI has no registry) and the consumer warn is strictly faster | A | deb5d77 | 2026-06-11 |
| SUB-08 | (grouped) arc accepted residuals, WHY-documented in code | bytes between Stop and drain consumed silently (next append revives); revival inside the 4.5s exit grace swallowed (turn-N+2 re-arms); exit racing the side-channel paints a ≤4.5s self-correcting walkout ghost; Identity-arm (5s,90s] parentless straggler self-heals via ledger adoption | ACCEPTED-residual | PR #250 body + arc memory; core CLAUDE.md child-ledger sharp edge (clean-exit ladder entry) — each residual carries an in-code WHY | B | deb5d77 | 2026-06-11 |

## 2026-06-11/12 — module-geometry residuals (#252/#254) → PR #256

Source: PR #256 body (the #240 precedent: residuals batched in one PR).

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| GEO-01 | `live_cc_session_ids` registry scan (#252) | duplicate sessionId across two live registry entries binds id→pid by unspecified `read_dir` order — binding flaps across refreshes, churns exit-watch rebinds, and a losing pid's death emits a spurious SessionEnd for a live session | CONFIRMED→PR#256 | PR #256 (5e26cfb): fold into a winners map — newest `startedAt` > has-`startedAt` > larger pid + warn-once. The issue's own "keep the first" fix sketch REFUTED-wrong-premise (itself scan-order-dependent); winner rule proven a strict total order | A | post-#255 (f353350) | 2026-06-12 |
| GEO-02 | `codex.rs::rollout_ids_from_paths` (review-round sibling) | same last-writer-wins shape for two live processes holding one rollout (resume overlap) | CONFIRMED→PR#256 | PR #256 reviewer-2 finding: same larger-pid rule, pinned in both enumeration orders | A | post-#255 (f353350) | 2026-06-12 |
| GEO-03 | `tests/reducer/liveness.rs` unknown_cwd twins (#254) | `unknown_cwd_agent_uses_faster_stale_timeout` is a ~90% duplicate of `unknown_cwd_agent_reaps_faster` | CONFIRMED→PR#256 | PR #256: merged keeping BOTH distinguishing assertions (`unknown_cwd` flag + ghost-`#N` label); reviewer diffed both at base — only incidentals differed (couldn't merge inside move-only #255 without breaking its byte-identical property) | A | post-#255 (f353350) | 2026-06-12 |

## 2026-06-12 — Phase-2 A/B experiment @ a8aaae9 (source/ module) → issue #262

Source: the ledger's first controlled A/B run (its control arm doubles as
the ledger-blind calibration pass — protocol step 8)
([`phase2-ab-2026-06.md`](review-metrics/phase2-ab-2026-06.md), workflow
`wf_04e5f98b-735`): every candidate adjudicated twice — a full skeptic+trace
pair in the control arm, and the ledger-routed treatment arm (full pair for
the 9 unmatched candidates; ONE cheap regression-checker for routed C08).
The run also re-executed the regression-check on R0610-01 +
SUB-01/04/05/06/07 + GEO-01/02 — all fix mechanisms verified present at
a8aaae9 by each of C08's three adjudications (control pair + the routed
cheap check). Three 2-2 split candidates
(inline blocking probe enumeration; codex name-only vouch forgeability; the
un-claim "emits NOTHING" test-artifact claim) are NOT adjudicated — listed in
#262 for the next whole-codebase review to re-derive.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + cited sharp edge / fix PR) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0612-01 | `jsonl/walk.rs:127-128` probe-bypass WHY comment | "CC (the only probe user)" is stale — Codex wires `with_liveness_probe` since #220/#227 (`codex.rs:383-386`); the bypass's safety silently rests on `codex_session_ended` being constant-false | CONFIRMED→PR#272 | walk.rs first-sight gate comment vs codex.rs:383-386; the oversized branch (walk.rs:163-167) already states it correctly | B | a8aaae9 | 2026-06-12 |
| R0612-02 | `hook/unix.rs:172` + `hook/windows.rs:195` CONN_TIMEOUT wrap | timeout cancellation mid-`tx.send()` under >1s back-pressure drops the rest of a decoded payload with no warn breadcrumb (`let _ =` swallows Elapsed) | CONFIRMED→PR#272 | handle_conn's per-event send inside the timeout scope (hook/mod.rs:101); hooks best-effort end-to-end bounds impact — breadcrumb is the actionable part | B | a8aaae9 | 2026-06-12 |
| R0612-03 | `jsonl/walk.rs:90-101` walk_jsonl dir recursion | `tokio::fs::metadata` follows symlinks + unbounded `Box::pin` recursion — symlink loop recurses unbounded; out-of-root symlink walks foreign `.jsonl` (precondition: planted in the user's own root) | CONFIRMED→PR#272 | no `symlink_metadata`/visited-set anywhere in walk.rs; first-sight gate limits registration impact | B | a8aaae9 | 2026-06-12 |
| R0612-04 | `jsonl/walk.rs:138-147` truncation reset × exit-path drains | transcript truncated below cursor at the moment a negative-vouch/instant-exit/un-claim drain runs interacts with the #228 drain-before-unclaim discipline | CONFIRMED→PR#272 | truncation reset arm vs liveness.rs:212-248 drain ordering | B | a8aaae9 | 2026-06-12 |
| R0612-05 | `cc_probe.rs:303` pid_alive on unreaped zombie | healthy snapshot in a zombie window re-vouches a just-ended id → retired transcript replays as a phantom — self-healing within ~one loop turn (second exit synthesizes via ESRCH receipt / immediately-readable pidfd) | ACCEPTED-residual | exit arm purge (jsonl/mod.rs:375-389) + watch(pid) re-end path; rare ms-scale window, cosmetic burst | B | a8aaae9 | 2026-06-12 |
| R0612-06 | decode-boundary content strings (Notification reason, Rename label, Generic tool display) | "content-derived strings reach display uncapped" — display half REFUTED (tooltip 34-char cap, ticker 512, ratatui cell clipping bound every TUI site); headless-summary egress + AgentSlot lifetime retention CONFIRMED | CONFIRMED-partial→PR#272 | shared char-safe `ellipsize` at `MAX_DECODED_FIELD_CHARS=80` across five decode sites (incl. the two siblings the review round added: `cwd_basename_label`, Generic `tool_name`); correlation keys deliberately uncapped (AgentId hashes the full string) | A | 0a2e387 | 2026-06-12 |
| R0613-01 | `source/codewhale.rs` AgentId key (cwd vs session_id) | CodeWhale source keys on `cwd`, not `session_id` — VERIFIED real, not arbitrary: a live capture proved `DEEPSEEK_SESSION_ID` is inconsistent within one session (`sess_<8hex>` on session/turn/tool-after, a raw turn UUID on `tool_call_before` — distinct engine code path), so session_id keying would split every ActivityStart into a 2nd ghost sprite; `DEEPSEEK_WORKSPACE` is identical on every event | VERIFIED (3-lens review, correctness c95) | `codewhale.rs:107-112` keys on cwd; shim `env_payload` never folds session_id; the 2026-06-12 capture (session_id-inconsistency) is the anchor — do NOT "improve" to session_id without re-capturing | A | feat/codewhale-source | 2026-06-13 |
| R0613-02 | `install/codewhale.rs:199-202` uninstall `ours_only` drop | a user's pre-existing LONE `[hooks] enabled = false` (no hooks) is not restored on uninstall — install force-set it true | ACCEPTED-residual | install is an explicit opt-in to visualization (silent-non-fire is the worse outcome) and `enabled` is moot with no hooks, so the only loss is a no-op line; faithful restore would need install to record the flip — not worth the state. WHY documented in the uninstall comment | B | feat/codewhale-source | 2026-06-13 |
| R0613-03 | `pixtuoid-hook/src/main.rs` `env_payload_from` | env-mode folds `DEEPSEEK_TOOL_ARGS`/`WORKSPACE`/`TOOL_NAME` verbatim with NO size cap, unlike the stdin arm's `STDIN_CAP` — a large tool input (a big write/edit) builds a >1 MiB line the 200ms watchdog drops, losing that `tool_call_before` (invariant #5 holds, but the "working" pulse is lost). NOT pure defense-in-depth — a real event-drop for common large edits | CONFIRMED→fixed (PR#275) | per-field `ENV_FIELD_CAP=128 KiB` byte-bounded char-safe truncation (`cap_env_field`); 3×128 KiB ≪ 1 MiB restores the stdin arm's pipe-quota guarantee for env-mode; pinned by `env_payload_caps_oversized_fields_at_a_char_boundary`. **Caught by the PR claude-review bot — MISSED by this PR's own 3-lens review** (its correctness lens checked env reads are panic-free but not the cap-vs-stdin-arm asymmetry) | B | feat/codewhale-source | 2026-06-13 |
| R0613-04 | `install/mod.rs:17,57,92` target-list hints | `NO_CLIS_MSG` + the `--config` single-target conflict + the multi-CLI conflict all listed `claude\|codex\|reasonix[\|all]`, omitting the newly-added `codewhale` — a user whose only CLI is CodeWhale gets a hint that never names `--target codewhale` | CONFIRMED→fixed (PR#275) | added `codewhale` to all THREE strings (bot flagged only line 17; the other two are the same staleness class) | B | feat/codewhale-source | 2026-06-13 |
| R0613-05 | `pixtuoid-hook/src/main.rs` `env_payload` cwd | env-mode keyed cwd SOLELY on `DEEPSEEK_WORKSPACE`, which is UNSET for a fresh `codewhale` launched without `-C` (workspace is `None` at session_start until it resolves) — the shim sent a cwd-less envelope, the decoder dropped it (no AgentId key), and NO sprite appeared. **A real bug** — not defense-in-depth | CONFIRMED→fixed (PR#275) | fall back to the hook child's `current_dir()` (CodeWhale runs the hook with current_dir = its working dir = the workspace; the fallback resolves to the same path the workspace eventually does, so events coalesce). **Caught ONLY by live user testing** — the `-C` capture, the unit tests, the 3-lens pre-push review AND the PR bot review all masked it (the capture forced a workspace via `-C`). The lesson: a hook-only source's identity field must be probed WITHOUT the convenience flags a capture rig adds. Pinned by `env_payload_falls_back_to_cwd_when_workspace_unset` | A | feat/codewhale-source | 2026-06-13 |
| R0613-06 | `source/hook/pid_watch.rs` + shim `_pid` | abrupt CodeWhale exit (SIGKILL/crash/terminal-close) fires no `session_end` → the sprite ghosts 10–30 min (the hook-only-source gap, shared with Reasonix; the JSONL ladder's negative-vouch/instant-exit don't apply — no watcher) | DONE (PR#275) | NOT deferred (the user's no-phasing rule): the shim stamps CodeWhale's pid (`_pid` via getppid — `sh -c` exec's the hook, Unix only; Windows `cmd /C` parent is the wrong pid → omitted) and `HookPidWatch` reuses the standalone `ExitWatch` (kqueue/pidfd) to emit SessionEnd on pid death — a STANDALONE add that never touches the JSONL liveness ladder. False pid (a forking shell) self-heals via the resurrect path. End-to-end pinned by `killing_a_watched_pid_emits_session_end_for_its_agent` (macOS-local/Linux-CI, like the JSONL instant-exit) + registry unit tests | A | feat/codewhale-source | 2026-06-13 |
| R0613-07 | `tui/theme/catppuccin.rs` codewhale hue + `theme/mod.rs` legibility guard | catppuccin's codex (sky `#89dceb`) and codewhale (teal `#94e2d5`) are Manhattan-distance **39** and near-identical luminance (592 vs 587) — perceptually one color at 2-char badge scale; the `source_badges_legible_for_every_theme` guard only asserted exact RGB inequality, so the near-collision passed | CONFIRMED→fixed (PR#275, whole-PR re-review) | catppuccin codewhale → deep teal `#33c0ac` (min pairwise dist 39→110, consistent with the deep-teal codewhale the other 5 themes use); the guard gained a `MIN_SOURCE_HUE_DIST=60` **Manhattan** floor — lum-sum can't see this class (catppuccin's pair is lum-equal), and the tightest legit pair is 82 (normal codex-vs-cw) so 60 keeps margin. Mechanizes the invariant (cf. #273). NIT not defect — `dashboard_line` draws the `[cw]` text label as the primary disambiguator — but fixed anyway per future-proof→do-it. Found by the install-config-platform lens (c80) | B | feat/codewhale-source | 2026-06-13 |
| R0613-08 | `source/codewhale.rs` `decode_cw_subagent` parent_id ↔ shim cwd-fallback | the subagent→parent link resolves only if the child's `workspace` string byte-matches the parent's shim-resolved cwd; under a no-`-C` launch the parent cwd comes from `current_dir()` (R0613-05) while the child carries CodeWhale's internally-resolved `workspace` (NOT through the shim) — they COULD diverge (trailing slash / symlink / abs-vs-rel), pointing parent_id at a non-existent slot | VERIFIED→#276 CLOSED | the user surfaced a REAL captured subagent run (`~/dotfiles/.deepseek/state/subagents.v1.json`: child `agent_ad945f4c`, workspace `/Users/navepnow/dotfiles`). Upstream `ui.rs::execute_subagent_observer_hook` emits `workspace = context.workspace.display()` (the same `App.workspace` that backs `DEEPSEEK_WORKSPACE`), and `/Users/navepnow/dotfiles` is not a symlink (`realpath` == literal) so the shim's `current_dir()` == that string → **byte-match holds**. Child id is `agent_<hex>` (never collides with a `/`-path). Pinned by `real_codewhale_subagent_payload_nests_the_child_under_its_workspace_parent` (decodes the FULL upstream payload — 7+ fields — through `decode_hook_payload` → reducer, asserts the child nests). Residual (a workspace that IS a symlink where CodeWhale canonicalizes but `current_dir()` doesn't) degrades gracefully to an un-nested sprite — not a tracked defect. Found by the correctness lens (c55) | A | feat/codewhale-source | 2026-06-13 |

> **Whole-PR re-review @ a716c5f (PR#275, 2026-06-12→13):** a 4-lens fan-out
> (correctness/grounding · design/blast-radius/semver ·
> shim-transport-never-panic+concurrency · install-config-write+cross-platform)
> over the full diff, each MEDIUM+ finding adversarially verified. Result:
> **0 CRITICAL / 0 HIGH / 0 MEDIUM**, all lenses APPROVE / APPROVE-WITH-NITS.
> Five of the eight findings were pure verification traces (no change). Two NITs
> fixed in-PR (R0613-07 + a stale `// our 5`→`every managed CodeWhale event`
> comment); one LOW deferred with proof-of-impossibility (R0613-08 → #276). The
> shim never-panic / `_pid`-peek-inert / no-Mutex-across-await / Arc<ExitWatch>
> lifetime claims were all independently re-derived and held.

## 2026-06-13 — opencode integration review @ 6183494 (feat/opencode-source) → PR pending

A 4-lens fan-out (correctness/grounding · design/blast-radius/semver ·
install+TS-plugin-artifact+cross-platform · liveness/lifecycle) over the whole
opencode diff, each MEDIUM+ adversarially verified. opencode is the 6th source —
HOOK-ONLY via a bundled TS plugin (the FIRST install target that ships CODE). The
event SHAPES are grounded in the cloned upstream source + the real persisted
`opencode.db` event log (a free-model self-run; the full tool+subagent LIVE
capture is confirm-before-merge, blocked on the throttled free tier). Result:
**0 CRITICAL / 0 HIGH / 1 MEDIUM (held)**, all lenses APPROVE-WITH-NITS; all
findings fixed in-PR.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0613-09 | `hook/mod.rs` pid-bind only on `SessionStart` × opencode `resurrects_on_prompt:false` | a MID-ATTACHED opencode session registers via the prepended `Identity` (opencode emits `session.created` once per session, never re-emits it), so `HookPidWatch.note` — fired only on `SessionStart` — never binds its pid; on an abrupt TUI quit (no `session.deleted` either) the sprite ghosts to the 30-min sweep. Invisible for CodeWhale (its `message_submit` re-emits `SessionStart` every prompt) — genuinely new for opencode | CONFIRMED→fixed | bind on `Identity` too via the new pure `pid_bind_target(ev)` (SessionStart \| Identity) — the `_pid` is already on every forwarded event; `note` is idempotent per (pid,agent). Also hardens CodeWhale mid-attach. Pinned by `pid_bind_target_covers_both_registration_carriers`. Found by the liveness lens (c88), adversarially verified holds=true | A | 6183494 | 2026-06-13 |
| R0613-10 | `install/opencode_plugin.ts` tool-part gate + `source/opencode.rs` running→ActivityStart | opencode re-publishes a tool part with status STILL `running` on every output chunk (streaming `bash`/`task`, upstream `prompt.ts`), so the plugin forwarded one shim-spawn PER chunk and the reducer's `tool_call_count` over-counted (no per-callID dedup under the single Hook transport) — contradicting the "one connection per tool-state change" claim | CONFIRMED→fixed | the plugin now tracks last-seen status per `callID` (a `Map`, freed on terminal status) and forwards a tool part only on a status TRANSITION — collapses the spawn burst AND fixes the count. Grounded in upstream source (not schema-inference). Found by the correctness lens (c78) | B | 6183494 | 2026-06-13 |
| R0613-11 | `source/opencode.rs` `decode_permission` reason keys | the reason was read from `title`/`pattern`/`type`/`tool`/`permission`, but the real upstream `permission.v2.asked` schema (`Request.fields` = `{sessionID, action, resources, …}`) carries NONE of them — so Waiting reason ALWAYS degraded to the literal `"permission"` (the synthetic fixture's `title` never occurs upstream) | CONFIRMED→fixed | key order is now `["action","permission",…]` (the v2 verb + v1 name first); the conformance fixture uses the real `{sessionID, action, resources}` v2 shape (snapshot reason now `bash`); tests exercise both v2 `action` and v1 `permission`. Found by the design + correctness lenses (c80) | B | 6183494 | 2026-06-13 |
| R0613-12 | `source/opencode.rs` module doc | the module header still described the SUPERSEDED install design ("registers it in `~/.config/opencode/opencode.jsonc`") — the install actually DROPS a plugin file in the auto-discovered `<config>/plugin/` dir, no jsonc edit | CONFIRMED→fixed | header corrected to the plugin-file-drop design. Plus two NIT doc/comment fixes (the `handle_conn` `_pid`-peek comment named only CodeWhale → CodeWhale+opencode; the `render_plugin` JSON⊂JS-string caveat re U+2028/U+2029, sound for filesystem paths). Found by the install lens (c96) | B | 6183494 | 2026-06-13 |
| R0613-14 | `install/opencode.rs` `default_config_path` plugin DIR name | installed to `<config>/plugin/` (SINGULAR) — taken from the anomalyco fork's permissive `{plugin,plugins}` source glob (so it loaded on the fork, live-proven) — but the CANONICAL opencode docs (opencode.ai/docs/plugins) auto-discover `<config>/plugins/` (PLURAL); canonical opencode would NOT load a singular-dir plugin → a silent no-op for non-fork users | CONFIRMED→fixed | `default_config_path` → `plugins/` (plural): works on the fork (brace glob) AND canonical. Re-verified live (plugin loads from `plugins/`, `oc·` sprite renders). **Found when the user asked "did you verify the official docs first?"** — I'd worked from the fork SOURCE + the real event log + a live e2e, NOT the docs; reading them surfaced the singular/plural divergence. LESSON: read the canonical docs BEFORE reverse-engineering a fork's (more-permissive) source — the fork's brace glob masked a portability bug | A | (post-2941cbb) | 2026-06-13 |
| R0613-13 | `install/opencode.rs` `detect_installed` (the `presence_probe`) | the auto-detect probe keyed on OUR plugin's `@pixtuoid-opencode-plugin` sentinel — so a bare `pixtuoid install-hooks` (auto-detect) would NEVER offer opencode until AFTER our plugin was already installed (chicken-and-egg); the opencode CLI itself was never detected. Every other source's `presence_probe` detects the CLI, not our artifact | CONFIRMED→fixed | `detect_installed` now probes opencode's OWN dirs (`<config>` + `~/.local/share/opencode`), like Reasonix/CodeWhale. `config_present`/uninstall use FILE-existence (unaffected) and `merge_uninstall` keys on the sentinel, so removal stays exact. **Found POST-merge-claim while the user asked "did we install the plugin?"** — the 4-lens install lens checked the sentinel logic but conflated "our plugin installed" with "the CLI is present." Reinforces R0613-05: for a hook-only source, real-use/live checks catch what code review (and green CI) miss — the case AGAINST merging on green alone | A | 722daac | 2026-06-13 |
| R0613-15 | `install/opencode_plugin.ts` `lastToolStatus` dedup-map key | the streaming-dedup Map is MODULE-LEVEL (one plugin process serves a parent AND its `task`-spawned child session concurrently) and was keyed on `callID` ALONE — two concurrent sessions sharing a `callID` would drop the wrong session's tool event | CONFIRMED→fixed (claude[bot] PR review, MEDIUM, flagged twice) | key on `sessionID:callID` (composite) — eliminates cross-session collision; entries still freed on terminal status. PLUS the bot's informational dead-code: `render_plugin`'s `serde_json::to_string(&str).unwrap_or_else("")` is infallible-but-if-hit-deploys-a-broken-`HOOK_PATH=""` → now propagates via `Result`. Re-verified live (composite-key plugin loads, `oc·` sprite renders; a 0-event run in between was a TEST-harness socket-reclaim race from a leftover `/tmp` socket — a 2nd headless instance over a stale socket degrades to transcript-only by design, not a product bug) | B | (post-29b90ae) | 2026-06-13 |

## 2026-06-13 — Connection panel + borderless popups review @ ad1655b (feat/status-panel-borderless-popups) → PR pending

A 3-lens fan-out (correctness/grounding · design/blast-radius · film-critic on
the regenerated popup stills), each MEDIUM+ adversarially verified (0 refuted).
The feature: an in-TUI Connection panel (`c`) unifying hook install/uninstall + live
connection, plus a cross-cutting conversion of EVERY popup to a borderless padded
panel. Result: **1 CRITICAL / 0 HIGH / 1 MEDIUM / 2 NITs**, two lenses
REQUEST-CHANGES; all fixed in the review-round commit (1e63ec1). The borderless
geometry / version-popup click-rect / install-core byte-identity / facet-lifecycle
claims were independently re-derived and held. NITs (both FIXED): a stale
"bordered" module doc in `widgets/dashboard.rs`; a one-line doc on
`ConnectionUi.selected` (usize-index soundness).

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0613-16 | `tui/connection/mod.rs::build_rows` registry→target join via `install::target::by_name(d.name)` | the REGISTRY claude id is `claude_code::SOURCE_NAME = "claude-code"` but the install target name is `"claude"`, so `by_name("claude-code")` → None → the flagship CLI rendered as a non-actionable `JsonlNoHooks` row (i/u no-op); masked because the unit tests hand-fed `source_id:"claude"` and the snapshot fixture hardcoded the row. The impl-plan ASSUMED `SourceDescriptor.name == Target.name` (false for Claude) — a plan-miss | CONFIRMED→fixed | joined `build_rows` on the existing `Target.core_source` (added by #282) via a new `target::by_source` (my branch predated #282 so I re-derived core_source; the rebase dedups to #282 field + my by_source). Mechanized by `every_target_names_a_registered_source` (target.rs) + `build_rows_makes_every_source_with_a_target_actionable` (connection/tests.rs) — both fail on any future registry/target id drift. Found independently by the correctness AND design lenses (conf 96) | A | ad1655b | 2026-06-13 |
| R0613-17 | `scripts/media.json` — no `connection` render job | the borderless Connection panel shipped with zero rendered CI baseline (only `snapshot --connection` existed, unwired), so a visual regression of the new flagship panel would pass gen-check silently | CONFIRMED→fixed | added a `connection` render job (`snapshot --connection` → `docs/images/connection.png`, gen-check-gated), mirroring `dashboard`; baseline committed. Also moved the fixture's first row id `"claude"`→`"claude-code"` (production source id). Found by the design + film-critic lenses (conf 78) | A | ad1655b | 2026-06-13 |

## 2026-06-13 — Source-connection-gating review (Phase 2, working tree on 0109a8d)

A 5-lens fan-out (correctness · design/blast-radius · security · test-coverage ·
docs-accuracy), each finding adversarially verified by 3 perspective-diverse
skeptics (kept iff ≥2 confirm). The change: gate the ENTIRE source on a persisted
per-source `[sources]` connected flag (gate + idempotent per-tick reconcile) +
rework the Connection panel to a single connect/disconnect toggle + DELETE the
`install-hooks`/`uninstall-hooks` CLI (and its whole orchestration layer + `inquire`).
Result: **1 CRITICAL / 4 HIGH / 2 MEDIUM / 1 LOW confirmed, 8 refuted** — all 7
confirmed fixed in this round; preflight green (1532 tests). The CRITICAL was a
genuine hole the spec *claimed* the reconcile closed but the impl didn't.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0613-18 | `runtime/driver.rs::reducer_task` reconcile sweep iterating `REGISTERED_SOURCES` | an identity-less hook event for an unknown id slips the per-event gate (`event_source`→None, `None.is_some_and`==false) → reducer synthesizes a BLANK-source (`source==""`) slot → the reconcile loop only iterated registered source names, so `""` matched none and the ghost survived to the ~30-min stale sweep — contradicting the in-code comment that the sweep "catches anything that slipped the gate" | CONFIRMED→fixed | replaced `Reducer::disconnect_source(src)` (exact-match per registered source) with `Reducer::reconcile_connected(&connected_set)` keyed on the COMPLEMENT of the set (`!connected.contains(source)`), so blank/unregistered slots are evicted within one tick; one call replaces the registry loop. Pinned by `reconcile_connected_evicts_a_blank_source_gate_slipper` (+ the retargeted evict/idempotent twins) in `tests/reducer/lifecycle.rs`. Comment corrected | A | 0109a8d+wt | 2026-06-13 |
| R0613-19 | `tui/mod.rs::connect_source`/`disconnect_source` persist-then-flip ordering | `save_source_connected` failure was only `warn!`ed, then `connected.set` ran unconditionally → runtime gate flips but intent isn't persisted → next restart `resolve_connected` reverts it (connect: re-evicts the just-connected sprites; disconnect: re-shows) — a silent lie | CONFIRMED→fixed | both helpers now PERSIST-OR-ABORT: on save Err, return a "couldn't save config" result and flip NOTHING (no gate change, no hook write). Pinned by `connect_source_aborts_without_flipping_the_gate_when_persist_fails` + the success twins in `tui/mod.rs` tests | A | 0109a8d+wt | 2026-06-13 |
| R0613-20 | deleted `tests/install.rs` (per-target CLI integration suite) | the 320-line suite was the ONLY end-to-end coverage of `install_target`/`uninstall_target` across all 5 targets (real ConfigLock read/merge/backup/atomic-write); deleting it with the CLI left the cores the Connection panel now calls covered only for CLAUDE | CONFIRMED→fixed | added `install_target_round_trips_every_registered_target` (install→AlreadyUpToDate→uninstall→NothingToRemove over `target::TARGETS`, config=Some(tmp), no CLI) — restores per-target round-trip coverage straight against the cores. Per-target merge specifics stay unit-tested in each `install/<t>.rs` | A | 0109a8d+wt | 2026-06-13 |
| R0613-21 | `version.rs::release_notes` (0.4.0–0.7.0 arms) | the in-app version popup's historical notes instruct users to run `pixtuoid install-hooks`, a command this change deleted | CONFIRMED→fixed | reworded the 6 install-hooks fragments to "connect it in the Connection panel (press c)" — display-only strings, dead-command reference removed everywhere | A | 0109a8d+wt | 2026-06-13 |
| R0613-22 | `resolve_connected` first-boot coverage of a newly-added source | no explicit test pinned that every `REGISTERED_SOURCES` entry is decided by the migrate-default on first boot (a new source could fall through) | CONFIRMED→fixed | added `resolve_connected_covers_every_registered_source` (all-installed ⇒ result == the full registry set), so a new registry row is auto-gated by construction | A | 0109a8d+wt | 2026-06-13 |
| R0613-23 | `main.rs` log-routing comment | "Non-TUI (install-hooks, uninstall-hooks, --headless): stderr" names two deleted commands | CONFIRMED→fixed | reworded to "Non-TUI (--headless, validate-pack, init-pack): stderr" | A | 0109a8d+wt | 2026-06-13 |
| R0613-24 | `docs/CONFIGURATION.md` system-managed keys table | the new `[sources]` connection table (visible in users' config) was undocumented | CONFIRMED→fixed | added a `[sources]` row documenting the panel-managed flags + the migrate-default + the "hidden even if hooks/transcripts present" semantics | A | 0109a8d+wt | 2026-06-13 |
| R0613-25 | `runtime/driver.rs::event_source` slot fallback | "event_source can return a STALE cross-source slot for a reused id, gating the wrong source" | REFUTED | `AgentId` is an FNV-1a hash of `(source, opaque_id)` (`source/mod.rs::AgentId::from_parts`), so two sources can't collide on one id absent a hash collision — the slot fallback can't return a foreign source. The artificial mismatched-event test isn't production-reachable. No sharp edge needed (a hashing fact, not a design choice) | A | 0109a8d+wt | 2026-06-13 |
| R0613-26 | `Reducer::reconcile_connected` arg type | "`s.source.as_ref()` vs `&str` is a type mismatch / won't compile" | REFUTED | `Arc<str>::as_ref()` yields `&str`; `HashSet<String>::contains(&str)` holds via `String: Borrow<str>`. Compiles + preflight green | A | 0109a8d+wt | 2026-06-13 |

## 2026-06-13 — PR #284 bot-review round (claude-review + security-review on the live PR)

The two GitHub claude-action passes (review + security) on PR #284. Both converged
on the same real gap the pre-commit Workflow missed; all fixed in the same PR.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0613-27 | `tui/mod.rs::connect_source` install-failure path | persist-or-abort covered SAVE failure but not `install_target` failure: on a failed install the flag was already persisted `true` + the gate already open, so the next restart honors a "connected" with no hooks behind it — a hook-only source shows connected yet never produces an agent | CONFIRMED→fixed | connect_source now ROLLS BACK on `install_target` Err (re-save the flag `false` + `connected.set(false)`); disconnect needs none (fail-safe to hidden + inert residual hooks). Pinned by `connect_source_rolls_back_gate_and_flag_when_install_target_fails` (FAIL_TARGET whose default_config_path errs). Flagged HIGH(missing-test)+MEDIUM(security) by two independent bot passes | A | 9ce1afb+wt | 2026-06-13 |
| R0613-28 | `config.rs::save_source_connected` arg type | `source_id: &str` lets a non-registry (runtime) string become a persisted TOML key — defense-in-depth (toml_edit escapes keys, so not an injection, but the type can be tighter) | CONFIRMED→fixed | tightened the whole chain to `&'static str` (save_source_connected + connect_source + disconnect_source); callers pass `ConnectionRow.source_id` (registry `&'static`), so only compile-time-known ids reach the key. Security MEDIUM-2 | A | 9ce1afb+wt | 2026-06-13 |
| R0613-29 | `tui/connection/mod.rs` formatters | `format_connect_result`/`format_disconnect_result`/`no_action_hint` had no unit tests (43% patch coverage) | CONFIRMED→fixed | added 3 unit tests in `connection/tests.rs` (connected/up-to-date + backup/PATH notes; removed/nothing + backup-cleared; NoCli vs actionable hint). review MEDIUM | A | 9ce1afb+wt | 2026-06-13 |
| R0613-30 | site `HowItWorks.astro` / `Install.astro` quickstart | the "Up and running in a minute" tutorial + the install lead still said `pixtuoid install-hooks` / `pixtuoid run` (the deleted CLI) — found by the user, the `.github`/site dirs were outside the earlier grep | CONFIRMED→fixed | rewrote the 3-step tutorial to launch (`pixtuoid`) → press `c` connect → code; Install lead → "run `pixtuoid` and press `c`". `just site-check` green | A | 9ce1afb+wt | 2026-06-13 |

## 2026-06-13 — PR #284 protocol two-lens+escalation review (5 lenses, whole PR at 7ba429f)

The mandatory two-lens review (pr-review.prompt.md) + 3 escalation lenses (lifecycle/editorial/film —
state-machine seam, public copy, regenerated still), each MEDIUM+ adversarially verified by 3 skeptics
(kept iff >=2 confirm). Per-lens: correctness/design/editorial/film APPROVE; lifecycle REQUEST-CHANGES,
but all 3 of its findings refuted >=2/3. **0 confirmed, 37 refuted, 3 nits (1 fixed).** Verdict
APPROVE-WITH-NITS, merge-ready.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + trace) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0613-31 | `runtime/driver.rs` ProofOfLife vs the connection gate | "a disconnected source's ProofOfLife events BYPASS the gate / re-vouch + resurrect its slot, defeating the eviction" | REFUTED (false premise) | `event_source` (driver.rs:179) returns the EXISTING slot's `source` for a ProofOfLife (it carries none), so the gate (driver.rs:215) drops it like any disconnected-source event. And the reducer's ProofOfLife arm only refreshes a vouch on an existing NON-exiting slot — it never creates or resurrects (core/CLAUDE.md ProofOfLife sharp edge: "exiting slots are skipped on insert and aren't sweep candidates"). reconcile_connected's `mark_exiting` therefore sticks. Cite: core ProofOfLife sharp edge + the new pixtuoid CONNECTION GATE "liveness-ladder interactions" note | A | 7ba429f | 2026-06-13 |
| R0613-32 | `reducer.rs::reconcile_connected` / `scope::cascade_exit` cross-source cascade during active delegation | "disconnecting a delegating parent could desync / evict a different source's subtree" | REFUTED (safe by design) | `cascade_exit` BFS-walks children by `parent_id` regardless of source, and a child's source equals its parent's by construction (a subagent inherits the spawner's source), so a single source's subtree exits atomically and a different connected source's tree is never touched; `active_tasks` suppression is AgentId-hashed (source-inclusive). The finding itself concluded "appears safe" and proposed docs, not a code change. Cite: core cascade_exit sharp edge (parent_id BFS) + the new CONNECTION GATE note (cascade is source-agnostic) | A | 7ba429f | 2026-06-13 |
| R0613-33 | `runtime/driver.rs` reconcile vs stale-sweep | "the interaction between per-tick reconcile eviction and the minutes-scale stale-sweep is undocumented / could double-evict" | REFUTED (correct by design; doc gap closed) | A disconnected source is evicted by the 1-Hz `reconcile_connected` (write-once `mark_exiting`), long before any stale-sweep candidate window; `mark_exiting` is idempotent so a later sweep pass is a no-op on the already-exiting slot. Documentation-only gap — now closed by the CONNECTION GATE "liveness-ladder interactions" note in pixtuoid/CLAUDE.md | A | 7ba429f | 2026-06-13 |
| R0613-34 | `tui/connection/mod.rs:221` no_action_hint vs tui/CLAUDE.md | nit: panel renders "not detected on this machine" but the tui guide documented the shorter "not detected" | CONFIRMED→fixed | aligned the tui/CLAUDE.md detail-line doc to the implementation's wording ("not detected on this machine"). Cosmetic | A | 7ba429f | 2026-06-13 |

## 2026-06-14 — Whole-codebase review at b3fa800 (12 finders -> dedup -> 3-skeptic verify -> SYSTEM lens)

Whole-codebase review (the user-requested post-0.7.0 / Fable-5 core-refactor sweep). 12 ledger-BLIND finders
(8 subsystem + 3 lens + 1 SYSTEM) -> 48 raw -> 29 deduped -> each high/med adversarially verified by 3
perspective-diverse skeptics (design-intent-vs-sharp-edges / concrete-repro / CI-or-primary-defense). Result:
**2 confirmed MEDIUM (both defense-in-depth/test-anchor gaps, fixed here), 6 refuted, 21 low/nit.** Zero
high/critical, zero correctness defects. SYSTEM lens: invariant #1 holds, decomposition net-positive, the
registry/bridge-test mesh is the scalability backbone. Verdict: APPROVE — strong health.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix/trace) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0614-01 | `source/reasonix.rs::rx_tool_detail` decode-boundary cap | the tool NAME was emitted uncapped (`format!("{tool}{target}")`) while CodeWhale/OpenCode/`make_tool_detail` all ellipsize it — pitfall-3 (content-derived strings capped at the decode boundary) applied to 3 of 4 hook-only decoders | CONFIRMED→fixed (3/3) | ellipsize both name + target via `MAX_TOOL_TARGET_CHARS` (now matches codewhale.rs:271); pinned by `long_tool_name_is_truncated_at_the_decode_boundary` (reasonix.rs). Low real impact (names short) but a regression-prone pattern divergence | A | b3fa800 | 2026-06-14 |
| R0614-02 | `sprite/format.rs` pack load ⇄ `tui/pixel_painter/palette.rs::recolor_frame` | the unique-RGB invariant for recolor keys B/H/S/P (recolor substitutes by RGB equality) was documented as a sharp edge but UNENFORCED — a `--pack-dir` custom pack could collide two keys and silently fail to recolor one (no panic) | CONFIRMED→fixed (2/3; 1 skeptic called it accepted-documented-tradeoff) | added `validate_recolor_palette` called in `build_pack` (bails on a recolor-key RGB collision, runs for embedded AND custom packs); the right layer = core validates / tui consumes (dep direction kept). Pinned by `recolor_palette_rejects_colliding_recolor_keys` | A | b3fa800 | 2026-06-14 |
| R0614-03 | `source/jsonl/liveness.rs` pid rebind/migration | "rebind/migration untested" | REFUTED | covered by `tests/watcher/liveness.rs::rebound_session_survives_old_pid_death_and_follows_the_new_pid`; absence of a UNIT test ≠ missing coverage | A | b3fa800 | 2026-06-14 |
| R0614-04 | `source/jsonl/walk.rs` oversized probe-bypassed re-registration | "a probe-admitted oversized file isn't re-gated on first sight" | REFUTED | the should_seed_at_eof gate EXPLICITLY excludes probe-admitted files (walk.rs:138-143); re-gate via `ended_in_skip` (PR #204, documented design) | A | b3fa800 | 2026-06-14 |
| R0614-05 | `source/jsonl/unclaim.rs` drain-before-unclaim truncation | "concurrent truncation during the drain races" | REFUTED | R0612-04 / PR #272 (47cc1df) — `park_if_truncated_below_cursor` precedes the walk; the remaining window is an ACCEPTED RESIDUAL with WHY (walk.rs:374-378) | A | b3fa800 | 2026-06-14 |
| R0614-06 | `tui/motion` missing-wander-profile freeze | "an absent wander profile freezes the sprite" | REFUTED | provably unreachable — profile snapshotted at transition-in (motion/mod.rs:302/355); two explicit tests; the 1-frame stall is the documented never-freeze-silently convention | A | b3fa800 | 2026-06-14 |
| R0614-07 | `runtime/driver.rs` blank-source gate-slip race | "an identity-less event for a disconnected source slips the gate" | REFUTED | production correct: event_source None → blank synth → `reconcile_connected` catches next tick (R0613-18), pinned by `reconcile_connected_evicts_a_blank_source_gate_slipper`; the candidate self-acknowledged it's a test-gap not a defect | A | b3fa800 | 2026-06-14 |
| R0614-08 | `source/hook` UndeliveredEvents Drop breadcrumb | "the drop breadcrumb is untested" | REFUTED | R0612-02 / PR #272 — `cancelled_conn_leaves_breadcrumb_for_undelivered_events` deterministically asserts the warn fires | A | b3fa800 | 2026-06-14 |

**Low/nit backlog (21, unverified)** — captured in the review output. On closer verification, 2 of the 3 "substantive" nits were NON-issues: the **symlink-loop visited-set** is unneeded (walk.rs:104 refuses symlinked entries wholesale, so recursion can't follow one), and the **probe SessionEnd re-admission** is correct-by-design (ProofOfLife never creates a slot; a probe vouch means the session is alive). Only **hook-only source state-machine fixture breadth** was a real deferred finding → filed as a tracking issue. The rest (WHY comments, module headers, CONN_TIMEOUT breadcrumb, test-isolation `sync::Once`) are doc/test-hygiene for a future sweep. The "CC the only probe user" nit was REFUTED inline (walk.rs:144 already names both probe users incl. Codex's FD probe).

## 2026-06-14 — PR #286 two-lens review (correctness APPROVE, design REQUEST-CHANGES → both fixed)

Two-lens review of the whole-codebase-review follow-up PR. correctness=APPROVE (no behavior bugs); design=REQUEST-CHANGES, 2 confirmed (3/3 skeptics each, 0 refuted) — both latent-inconsistency / DRY defects, fixed in the same PR.

| # | seam (file/mechanism) | claim (1 line) | verdict | anchor (paths + fix) | tier | head | date |
|---|---|---|---|---|---|---|---|
| R0614-09 | `source/reasonix.rs::rx_tool_detail` cap constant | the R0614-01 fix capped the tool NAME at `MAX_TOOL_TARGET_CHARS` (40) and the comment falsely claimed parity with `make_tool_detail`, which caps the name at `MAX_DECODED_FIELD_CHARS` (80) — a latent cross-decoder inconsistency (CodeWhale/OpenCode also used 40 for the name) | CONFIRMED→fixed (3/3) | UNIFIED the tool-NAME cap to `MAX_DECODED_FIELD_CHARS` across ALL THREE hook-only decoders (reasonix + codewhale + opencode), matching `make_tool_detail` (name = content field → 80; the `: target` descriptor stays 40); comments corrected; reasonix test updated to `MAX_DECODED_FIELD_CHARS*3`. The whole codebase now caps tool name at field-width, target at target-width — one convention | A | d035411 | 2026-06-14 |
| R0614-10 | `sprite/format.rs::RECOLOR_KEYS` ⇄ `tui/pixel_painter/palette.rs::recolor_frame` ⇄ embedded test | the recolor key set `[B,H,S,P]` was hardcoded in 3 places (core const, the recolor_frame match arms, the embedded test) — a 5th key = a 3-place lockstep edit; the guard could under-check a key recolor uses | CONFIRMED→fixed (2/2) | made `RECOLOR_KEYS` `pub` in core = SINGLE source of truth: `recolor_frame` now iterates it (hoisted base→agent swaps, behavior-preserving), `validate_recolor_palette` guards it, the embedded test imports it. A 5th key is now a 1-place edit, and recolor + the guard can't drift. tui/CLAUDE.md sharp edge updated (invariant ENFORCED at load, not just documented) | A | d035411 | 2026-06-14 |
