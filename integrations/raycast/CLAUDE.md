# integrations/raycast — agent guide

The **Raycast extension**: a self-contained **TypeScript / Node** project (NOT
Rust) and a thin presenter over the `pixtuoid … --json` CLI contract. It ships
two commands — `Manage Sources` (connect/disconnect over `pixtuoid
sources|connect|disconnect --json`) and `Start Floating`. Parent guide: the
workspace [`../../CLAUDE.md`](../../CLAUDE.md). The cross-area development model
this consumer sits in: [`../../docs/PARALLEL-DELIVERY.md`](../../docs/PARALLEL-DELIVERY.md).

> **You are in the TS consumer, not the Rust producer.** The workspace
> `CLAUDE.md` still loads above this file — but its Rust house rules
> (TDD-in-Rust, `cargo`/`clippy`, `just preflight`, `semver`, `gen-check`)
> **do not apply here**. This is a Node project; the gates are `tsc` + `eslint`.
> Don't run `cargo` anything for a change scoped to this directory.

## What it is

A login-shell-resolved shell over the CLI — it does **not** bundle the binary
(resolves it via `$PATH` + a `binaryPath` preference). `src/pixtuoid.ts` is the
CLI bridge; `manage-sources.tsx` / `start-floating.tsx` are the Raycast command
UIs. No server, no state of its own — every fact comes from the CLI's JSON.

## The contract is GENERATED, not hand-mirrored (read this first)

BOTH wire types — `SourceStatus` AND `OutcomeRow` — are **generated**, not
hand-typed. The Rust serde types (`crates/pixtuoid/src/sources.rs`) emit
committed JSON Schemas (`contract/source-status.schema.json` +
`contract/outcome-row.schema.json`, via their `schemars` derives + the
`*_schema_matches_the_committed_contract` golden tests); `npm run gen:contract`
(json-schema-to-typescript) regenerates `src/contract.ts` +
`src/contract-outcome.ts` from those schemas; and `pixtuoid.ts` re-exports the
generated types (`export type { SourceStatus }` / `{ OutcomeRow }`). So a
producer shape change **can't hand-drift** — three gates catch it: the Rust
struct↔schema golden tests (`just test`), the schema↔TS-type freshness check
(raycast CI regenerates both files and `git diff --exit-code`s them), and the
TS-type↔usage `tsc --noEmit` pass. **After changing `SourceStatus` or
`OutcomeRow`, run `just gen-contract`** (re-emits the schemas + the TS types)
and commit all of it. `src/contract.ts` / `src/contract-outcome.ts` are
generated — eslint/prettier-ignored, never hand-edit them. This is
`PARALLEL-DELIVERY.md`'s "codegen-from-one-source" applied to pixtuoid itself.
(The `source_status_json_shape` / `outcome_row_json_shape` byte tests still pin
the exact wire JSON; `OutcomeRow` is `{id, outcome, message?}` — a bare machine
token plus an optional failure-detail field, split from the old folded
`failed: <msg>` form BEFORE store publication — see the sharp edge below and
the wire-shape sharp edge in `crates/pixtuoid/CLAUDE.md`.)

## Sharp edges (don't be surprised by these)

- **A non-zero CLI exit DISCARDS stdout in the usual `execFile` path** — but the
  CLI still printed a JSON array. Recover it from `err.stdout` in the bridge; a
  toast-only error path is dead code (the failed approach). The `--json` outcome
  rows arrive even on a partial failure.
- **The `binaryPath` preference is RE-READ on every call**, not cached — only
  the PATH auto-detect is memoized. A user who fixes the pref mid-session
  shouldn't have to relaunch.
- **Toolchain bumps must stay within what Raycast DECLARES — check the peers,
  don't guess.** `eslint`/`typescript` are gated by `@raycast/eslint-config`'s
  peerDependencies (2.2.0 declares `eslint ^10`, `typescript <6.1.0` — so
  eslint 10 + TS 6.0 are in-range); `@types/node` stays on the `22.x` MAJOR
  (dependabot bumps minors within it — `.github/dependabot.yml` ignores only
  the major — so the manifest floats at `^22.x`, currently `^22.20.1`).
  `@raycast/api`'s exact peer (22.19.17; Raycast's runtime is Node 22) is a
  warning-level mismatch npm tolerates under the committed lockfile, not a hard
  pin the manifest must equal. And
  `ray build` type-checks with its OWN bundled tsc (5.6 as of api 1.104.21),
  so `tsconfig.json` must stay parseable by BOTH that and the local TS: the
  TS 6 migration was `moduleResolution: "Bundler"` + an explicit
  `types: ["node"]` (TS 6.0 stopped auto-including `node_modules/@types`);
  `ignoreDeprecations: "6.0"` would have broken `ray build` (TS 5.x rejects
  the value).
- **`OutcomeRow.outcome` ∈ `connected | disconnected | failed`** (bare tokens)
  for the single-id `connect`/`disconnect` the extension calls; `no_op` is
  emitted only by `pixtuoid sources set` (the declarative reconcile this
  extension never invokes). Failure detail rides in the optional `message`
  field (present exactly when `outcome === "failed"`) — match tokens exactly,
  no prefix-stripping. This clean split landed while the in-repo extension was
  the ONLY consumer (it ships atomically with the binary; NOT yet on the
  Raycast store). **After store publication, installed copies parse the wire
  independently of the binary's version — any further wire change needs a
  version handshake, not a flag-day edit.**

## Gates

CI (`.github/workflows/raycast.yml`, Linux runner): `npm ci` → `npx tsc
--noEmit` → `npx eslint .`. Run those two locally before "done." **`ray build` /
`ray lint`** (manifest + icon validation, the Prettier pass) need the **macOS
Raycast app** and only run before a store publish — they are NOT in CI, so a
green PR does not prove the manifest is publishable. See the
[README](README.md) for `npm run {build,dev,lint}`.
