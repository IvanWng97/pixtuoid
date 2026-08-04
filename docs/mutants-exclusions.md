# cargo-mutants: exclusions and their derivations

Reference for `.cargo/mutants.toml`. The config file carries a one-line *why* per
entry; the measured derivations live here so a 23-line config isn't fronted by a
174-line essay. Re-derive before trusting any measurement below — they are dated
observations, not live state.

## Read this before acting on a `pixtuoid-core` / `pixtuoid-scene` survivor

`test_workspace` is deliberately left at its default, meaning **only the tests in
the mutated package are run** (`cargo mutants --help`). That is a systematic blind
spot for this repo, because core/scene behaviour is covered on purpose by the
**binary's** headless harness (~100 integration tests driving the real
`TuiRenderer`), not by core's own unit tests.

Measured, not theorised: making `SceneState::insert_daemon` a no-op reds 8 of 9
`harness::mascot` tests while all 53 of core's own daemon tests stay green — and
it still reported as a survivor.

**So: for a survivor in core/scene, REPRODUCE IT FIRST.** Apply the mutation and
run `-p pixtuoid` too. Only write a new test if the mutation genuinely survives the
whole workspace. Otherwise you are padding the suite, or "fixing" code that works.

Turning `--test-workspace true` on is the accurate-but-unusable option: the 2026-07
run took 2h for 221 mutants package-scoped (~33s each), while the full workspace
suite is ~135s per mutant — the same run would be ~8h. Pass it by hand
(`just mutants --test-workspace true`) when auditing core/scene specifically.

## Why `tui/mod.rs` and `runtime/driver.rs` are mutated but codecov-ignored

Both are line-uncoverable in **bulk** while still carrying a unit-tested pure
decision layer, and per-**function** mutants see that split where per-**line**
coverage cannot. Their untestable glue is pinned by function in `exclude_re`
rather than hidden by file, so the decision layers stay in scope.

File-level exclusion was the wrong tool here: it hid all 135 `tui/mod.rs` mutants
and all 9 in `driver.rs`, killable ones included — and **an exclusion that hides a
killable mutant is worse than a noisy row.**

Limits of what was actually measured (2026-07):

- `tui/mod.rs` carries 135 mutants (`cargo mutants --list --no-config --package
  pixtuoid --file crates/pixtuoid/src/tui/mod.rs`), 72 in `dispatch_key` alone.
  Exactly 22 have ever been RUN — those matching
  `-F 'toggle_intent|is_quit_chord|should_dispatch_key'` — scoring 19 caught, 1
  unviable, 2 missed (the `should_dispatch_key` match guards inside `run_tui`, now
  excluded by function). 14 of those 22 are `is_quit_chord` guards *inside*
  `dispatch_key`, so the run covered 22 of the 78 those four fns carry, **not** all
  of them. The other 113 are UNMEASURED: a survivor there is unknown, not known
  noise — measure it before dismissing it.
- `driver.rs` carries 9 mutants, none ever run under mutation. The 6 naming fns
  with zero test call sites are excluded; the 3 left in are reachable
  (`build_source_set` ×2 via the registry-drift test, `headless_loop_with_signal`
  via the two signal-arm tests).

## Equivalent-mutant derivations

Each was re-confirmed surviving on main, then proven **unobservable** — not merely
untested.

**`unclaim.rs` — `<` vs `<=` in `ChildEndUnclaims`.** Prunes on `Instant::now()`
deltas; the two differ only when two monotonic reads land on the same instant,
which no deterministic test can arrange. The non-boundary prune/dedupe behaviour
*is* pinned by unclaim.rs's unit tests.

**`hook/unix.rs` — `>` vs `>=` in `Listener::bind`.** The `tmp.len() > 100` picks
between two bind strategies (temp-rename vs direct+chmod) whose end states are
byte-identical. The boundary is `sun_path` headroom, distinguishable only by which
syscall sequence ran — invisible to any external observation.

**`cc_probe.rs` — `pid_start_time_secs`.** A cfg-twin pair (macOS `proc_pidinfo` /
non-macOS `None`): on any host the other platform's variant is compiled out, so its
mutants always survive as cross-compile noise. Both variants are pinned by cfg-twin
tests (`pid_start_time_is_plausible_epoch_seconds_for_own_process` /
`pid_start_time_is_deliberately_unavailable_off_macos`) on their own platforms.

**`compute.rs` — `plant_ground_in_bounds`.** Selects which scatter plants the #566
connectivity guard drops vs the `plants.clear()` last resort. Within the swept
space (narrow-band 32..=76 × {80,100,120,160} × seeds 0..11) it is unobservable: at
every config where the guard fires, both corridor plants are already aisle-resident,
so the targeted retain empties the list identically to clear-all; the configs that
keep a plant never seal. The targeted path is **not** dead — `plants` can also carry
meeting-room + Ficus plants far from `cubicle_aisle`, which is exactly what it
protects from the clear-all sledgehammer. The never-ship-a-pocket invariant is
pinned by `appliance_strip_not_sealed_at_a_single_pod_band` + the boundary scan.

**`liveness.rs` — `>` vs `>=` in `ProbeSnapshot::bind_pid`.** The #252 tiebreak
`if pid > *bound { *bound = pid }` differs only when `pid == *bound`, where the body
writes the value already there. `pid_of` is a `HashMap<String, i32>`, so no observer
can distinguish. Ordering itself *is* pinned both directions by
`bind_pid_keeps_the_larger_pid_in_both_orders`.

**`jsonl/mod.rs` — `no_cwd_from_path`.** The default `CwdDeriver` ("no opinion");
its sole caller collapses both spellings one line later:
`cwd.or_else(|| (decoders.cwd_derive)(path)).unwrap_or_default()`. `None` and
`Some(PathBuf::new())` both leave the same empty `PathBuf`, and walk.rs is the only
call site in the tree. The grok OVERRIDE that makes the seam matter is pinned
separately by the `with_cwd_deriver` test.

## Deliberately NOT excluded

`mascot_position`'s walk-in boundary (`if entered < MASCOT_ENTER_MS`) is an
equivalent mutant — at t=1 the walk-in's endpoint IS `home`, which is also the
wander's forced cycle-0 origin, so both branches render the identical `Point`. But
its **sibling** boundary, `if age < enter_delay`, is genuinely killable: the stagger
returns `None` rather than holding at the elevator, so `<=` changes drawn to
not-drawn at exactly `age == enter_delay`, caught by
`the_walk_out_starts_from_where_the_mascot_was_when_it_died`.

cargo-mutants describes both **identically** ("replace < with <= in
mascot_position"), so no regex can exclude one without the other. Hence neither is:
the walk-in shows up as a known-noise row, documented here, and the stagger one is
caught. Same call as the `% with /` pair in `mascot_spot_for` before that fn was
deleted.

## Reached-under-test but unobservable

**`hook::unix::ensure_owned_socket_dir`** substitutes the real
`/tmp/pixtuoid-{uid}` + our uid into `ensure_owned_socket_dir_in`. It IS called
under test (`tests/transport/socket.rs` drives it via `HookSocketListener::bind`)
but always with a tempdir endpoint — only the no-op branch, where `-> Ok(())` is
correct. Reaching the firing branch means creating and validating the REAL per-user
socket dir, which would race a live daemon on a dev box. **Keep this fn a pure
substitution:** the decision logic and its six pins live in
`ensure_owned_socket_dir_in`, and logic added here becomes unmeasurable.

**`ProbeSnapshot::from_open_fds`** is the same shape: reached through the omp/codex
probes, but only where the proc-table walk finds nothing, so
`Some(Default::default())` is indistinguishable from the real answer.
Distinguishing it needs a live process holding a transcript open under a temp root
— the real-FFI-plus-scheduling flakiness `fd_probe.rs` is excluded wholesale for.
The logic it used to hide is now measured: `_with` takes both enumerators, so
canonicalize-or-`Some(empty)`, enumeration-failure-is-`None`, and the pid→path join
are each killable.
