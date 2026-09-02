# integrations/raycast — agent guide

The **Raycast extension**: a self-contained **TypeScript / Node** project (NOT
Rust) and a thin presenter over the `pixtuoid … --json` CLI contract. It ships
two commands — `Manage Sources` (connect/disconnect over `pixtuoid
sources|connect|disconnect --json`) and `Start Floating`. Parent guide: the
workspace [`../../CLAUDE.md`](../../CLAUDE.md). The cross-area development model
this consumer sits in: [`../../docs/PARALLEL-DELIVERY.md`](../../docs/PARALLEL-DELIVERY.md).

> **You are in the TS consumer, not the Rust producer.** The workspace
> `CLAUDE.md` still loads above this file — but its Rust house rules
> (TDD-in-Rust, `cargo`/`clippy`, `just preflight`, the crate CI gates)
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
`failed: <msg>` form back when this in-repo copy was the only consumer. The
wire is PUBLISHED — installed store copies parse it independently of the
binary's version; `OutcomeRow`'s doc comment in `crates/pixtuoid/src/sources.rs`
owns that rule.)

**A republish may still be owed.** The split (`e21ec7f0`, 2026-07-02) landed
AFTER the local `ray publish` marker `__raycast_latest_publish_ext/pixtuoid__`
(`b870d8ba`, 2026-06-19), so the version in the store was built against the
folded `failed: <msg>` form: it prefix-strips, and renders a bare `failed` toast
with the reason dropped. The parse in `src/` is already correct, so a republish
is the whole fix. That tag is never pushed, so a fresh clone has no copy — check
your own, then the listing at `raycast.com/IvanWng97/pixtuoid`, before assuming
it is clear.

## Toolchain policy

`package.json` cannot carry a comment, so these live here.

- **Toolchain bumps must stay within what Raycast DECLARES — check the peers,
  don't guess.** `eslint`/`typescript` are gated by `@raycast/eslint-config`'s
  peerDependencies (2.2.0 declares `eslint ^10`, `typescript <6.1.0` — so
  eslint 10 + TS 6.0 are in-range); `@types/node` stays on the `22.x` MAJOR
  (dependabot bumps minors within it — `.github/dependabot.yml` ignores only
  the major). `@raycast/api`'s exact peer is a warning-level mismatch npm
  tolerates under the committed lockfile, not a hard pin the manifest must
  equal. `ray build` type-checks with its OWN bundled tsc (5.6 as of api
  1.104.21), so `tsconfig.json` must stay parseable by BOTH that and the local
  TS: the TS 6 migration was `moduleResolution: "Bundler"` + an explicit
  `types: ["node"]` (TS 6.0 stopped auto-including `node_modules/@types`);
  `ignoreDeprecations: "6.0"` would have broken `ray build` (TS 5.x rejects the
  value).

## Gates

CI (`.github/workflows/raycast.yml`, Linux runner): `npm ci` → `npm run audit` →
the `gen:contract` freshness diff → `npx tsc --noEmit` → `npx eslint .`. Run them
locally before "done." **`ray build` /
`ray lint`** (manifest + icon validation, the Prettier pass) need the **macOS
Raycast app** and only run before a store publish — they are NOT in CI, so a
green PR does not prove the manifest is publishable. See the
[README](README.md) for `npm run {build,dev,lint}`.

- **`npm run audit` is plain `npm audit --audit-level=low`, same as site's.** It
  was a per-advisory allow-list script until its one entry, GHSA-mh99-v99m-4gvg,
  cleared — upstream BACKPORTED that fix to **2.1.3**, so the pinned 2.x copy
  took it in-range at 2.1.4 and the chain never had to move (the deleted entry's
  "first patched 5.0.8, so 2.x is unfixable" is the stale reading, and it has
  already misled one reviewer);
  `npm audit` has no per-advisory ignore, so if an unfixable advisory recurs
  here, restore that script from history rather than lowering `--audit-level`,
  which blinds a whole severity band to hide one id. Unfixable is realistic:
  the last one arrived via `@oclif/core → ejs ^3 → jake ^10 → filelist ^1 →
  minimatch ^5`, a chain we own no link of, and the override that would have
  patched it made audit green over code that throws (#792).
- **A chord Raycast RESERVES is swallowed, so its Action is unreachable — and
  `@raycast/no-reserved-shortcut` is escalated to `error` here.** Upstream ships
  it at warn and `eslint .` exits 0 on warnings, which is how an `Open Extension
  Preferences` action bound to `⌘,` (Raycast's own `OpenPreferences`) shipped
  dead. Nothing else sees it: `tsc` types the chord fine and `ray lint` is not in
  CI. Its sibling `@raycast/prefer-common-shortcut` stays a warning — style
  advice a routine version bump could turn into a surprise red.
