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

## The contract is the coupling (read this first)

`src/pixtuoid.ts`'s `SourceStatus` / `OutcomeRow` interfaces **mirror** the Rust
`--json` output. The Rust side pins that shape with the
`source_status_json_shape` test (`crates/pixtuoid/src/sources.rs`) — **mirror any
change there here, and vice versa.** This hand-mirrored interface is a known
drift risk (two copies of one shape); the principled fix is to *generate* the
`.d.ts` from the serde types (`schemars` → JSON Schema → codegen) so a producer
change becomes a compile error here — tracked as a future improvement in
[`PARALLEL-DELIVERY.md`](../../docs/PARALLEL-DELIVERY.md) ("the contract should
emit a schema, not be hand-mirrored").

## Sharp edges (don't be surprised by these)

- **A non-zero CLI exit DISCARDS stdout in the usual `execFile` path** — but the
  CLI still printed a JSON array. Recover it from `err.stdout` in the bridge; a
  toast-only error path is dead code (the failed approach). The `--json` outcome
  rows arrive even on a partial failure.
- **The `binaryPath` preference is RE-READ on every call**, not cached — only
  the PATH auto-detect is memoized. A user who fixes the pref mid-session
  shouldn't have to relaunch.
- **The toolchain pins (`Node 22` / `eslint 9` / `typescript 5`) MATCH Raycast's
  own toolchain — do NOT bump them ahead** of what `@raycast/api` expects, or
  `ray build` (store publish) breaks even though `tsc` passes.
- **`OutcomeRow.outcome` ∈ `connected | disconnected | failed: <msg>`** for the
  single-id `connect`/`disconnect` the extension calls; `no_op` is emitted only
  by `pixtuoid sources set` (the declarative reconcile this extension never
  invokes). Parse structurally, not by an exhaustive union.

## Gates

CI (`.github/workflows/raycast.yml`, Linux runner): `npm ci` → `npx tsc
--noEmit` → `npx eslint .`. Run those two locally before "done." **`ray build` /
`ray lint`** (manifest + icon validation, the Prettier pass) need the **macOS
Raycast app** and only run before a store publish — they are NOT in CI, so a
green PR does not prove the manifest is publishable. See the
[README](README.md) for `npm run {build,dev,lint}`.
