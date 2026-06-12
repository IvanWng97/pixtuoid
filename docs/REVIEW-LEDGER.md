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
