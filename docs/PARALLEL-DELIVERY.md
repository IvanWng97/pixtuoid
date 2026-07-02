# Parallel delivery

Shipping **one cross-cutting change across many areas** — a backend or shared
library plus its consumers (a web app, mobile apps, a CLI or extension) — **in
parallel**, whether the workers are humans or AI agents. This is the work-
decomposition half of building a repo at agent scale; its sibling,
[`KNOWLEDGE-ENGINEERING.md`](KNOWLEDGE-ENGINEERING.md), is the knowledge-
persistence half (how lessons stop being re-paid). This page is the system,
grounded in industry practice and worked through pixtuoid, then mapped onto a
polyglot product monorepo (server / Android / iOS / a Lynx web frontend).

## The problem: one idea, N areas

A single idea — "add an X" — lands in several places at once: the producer (a
backend or core library) grows a new capability, and every consumer has to
render, call, or expose it. The naive serial path (finish the producer, then the
web client, then iOS, then…) is slow and still fails at the seam: the moment two
areas code against an **imagined** contract, they integrate to a runtime
surprise. Parallelism without a frozen contract is just distributed guessing.

## The thesis: the contract is the synchronization primitive

Freeze the contract first. Then the areas fan out and build against it
independently. Then they re-join on the producer. Three phases, one barrier and
one join:

```
Phase 0  CONTRACT FREEZE  ── one author. NOT parallel.
         a versioned, machine-readable spec + the codegen + the pinning gates
                  │  ← hard barrier: nothing forks until the spec is reviewed & agreed
   ┌──────────────┼───────────────────────────────┐
Phase 1 (parallel — each worker/agent in its OWN git worktree)
   producer        web/consumer A      mobile/consumer B      CLI/extension
   implement the   build against the   build against the      build against the
   spec; export    spec (+ generated   spec (+ generated      spec (+ generated
   the artifact    mock if producer    SDK); breakage =       types); breakage =
                   isn't done yet)     a local compile error  a local compile error
   └──────────────┼───────────────────────────────┘
                  │  ← JOIN (not embarrassingly parallel — see below)
Phase 2  producer merges FIRST, then each consumer regenerates + verifies
         against the merged producer; merge-queue / stacked PRs serialize the land
```

The contract makes the *code* parallel. Verification still **re-joins on the
producer** — a consumer's generated client doesn't prove the running producer
honors the shape, and any media/artifact the consumer derives from the real
producer (a rendered demo, a live smoke) must run after the producer merges. So
the producer is the join point; it merges first, consumers verify against it.

## Phase 0 — freeze the contract, and keep it living

A contract-first spec is authored, reviewed, and **agreed before implementation
code** (APIs You Won't Hate). But a frozen shape plus generated mocks is only
half the discipline — **~75% of APIs don't conform to their own spec**
(APIContext, 650M API calls across 10k+ endpoints). Make "can't drift"
mechanical in three layers:

1. **Govern the spec in CI.** Lint structure/style against a house style guide
   (Spectral); detect breaking diffs on every PR — removed fields, type changes,
   narrowed enums — with `oasdiff` (OpenAPI) or `buf breaking` (Protobuf, baseline
   = a git ref or registry image). Governing a *spec* is mechanically enforceable
   in a way governing hand-written code is not.
2. **Enforce server-side.** A schema registry holds/rejects a breaking push
   before consumers ever see it (Buf BSR; Apollo `rover subgraph check` composes
   against all registered subgraphs and runs usage checks before publish).
3. **Verify runtime conformance.** Lint + diff prove the *spec* is compatible,
   not that the *code* matches it. Close the gap with response-vs-schema checks
   against the running producer (Dredd / Schemathesis / Prism) — the defense-in-
   depth a registry-only gate misses (totalshiftleft).

**Codegen-from-one-source is the load-bearing guard.** Generate typed SDKs from
the one spec and regenerate in CI, so a producer's breaking change becomes a
**local compile error in each consumer** instead of a runtime surprise (hey-api).
The proven shape is Stripe's: one internal source-of-truth → a CI-checked
OpenAPI/`.proto` on `main` → one generator → SDKs for every language, with
generated code in an outer layer that depends on hand-written inner infra so
regeneration never clobbers business logic (brandur).

**Pick the contract language by consumer polyglotism, not preference.** A
genuinely multi-language fleet (server + Kotlin + Swift + JS/TS) wants
**Protobuf/gRPC** or **Smithy** (one model → typed messages + stubs everywhere),
or **OpenAPI** if you want REST gateways/caches/tooling. **GraphQL** suits
schema-shaped frontends. **tRPC is disqualified as a cross-platform contract** —
it consumes the server's TypeScript type graph by inference and structurally
cannot reach native Kotlin/Swift (techbytes, reliasoftware).

**Compatibility mode dictates deploy order** — encode it in the rollout runbook:
`BACKWARD` ⇒ upgrade consumers first; `FORWARD` ⇒ producer first; `FULL` /
`*_TRANSITIVE` ⇒ any order (use transitive when consumers may lag across several
schema versions) (Confluent, AWS Glue).

## Phase 1 — fan out by area

- **Affected graph, computed not hand-maintained.** Map the git diff → projects →
  every downstream dependent, and run only those (Nx `affected`, Turborepo). Two
  traps: set the base to the **last *green* commit on `main`** (the merge-base for
  a PR), **not `HEAD~1`**, or you skip changes since the last passing run; and a
  lock-file / global-config bump must be special-cased or it silently invalidates
  the whole affected calc (Nx).
- **Affected-filtering alone collapses at scale.** Editing a high-fan-in shared
  library marks *almost the whole repo* affected, so treat **affected + remote
  caching + distributed execution** as three orthogonal layers, not alternatives
  (Nx, Turborepo). Let the build tool topologically order tasks from the graph
  and cap intra-machine concurrency with its own knob (Nx `--parallel=N`,
  Turborepo `--concurrency` — note Turborepo's `--parallel` is a footgun that
  *ignores* the graph).
- **Worktree isolation per worker.** Concurrent agents/sessions on one checkout
  race on `HEAD` and uncommitted changes — each gets its own `git worktree`. The
  contract artifact is the shared pin across worktrees: a `main`-branch variant
  each branch validates against and only mutates on merge.
- **Mocks unblock consumers before the producer exists.** A generated mock server
  (Prism, Microcks) lets a consumer make real progress against the agreed shape
  while the producer is still implementing it.
- **Partition along graph boundaries.** Assign workers to leaf projects that
  don't share files; *sequence* workers that touch the same shared file rather
  than parallelizing them.

## Phase 2 — join

The producer merges first (it's the source of the shape); each consumer then
regenerates its SDK and verifies against the merged producer. Serialize the land
itself with a **merge queue** / **stacked PRs** / trunk-based discipline so N
parallel branches integrate without clobbering each other. The contract test is
the integration guardrail: if the producer drifts from the frozen shape, the
consumer's regenerate-and-typecheck job fails in *that consumer's* CI — spec-
diffing flags *that* a break occurred, but only each consumer's own pipeline
shows *which* consumer breaks.

## The same model with AI agents

Everything above is agent-ready, because the contract removes the need for a
human to relay intent between workers:

- **The frozen, CI-governed spec is the coordination substrate.** A producer-
  agent and each consumer-agent build against the same artifact, and breakage
  surfaces as a *deterministic CI failure* (`oasdiff` / `buf breaking` /
  typecheck) — never as cross-agent miscommunication (apisyouwonthate, hey-api).
- **Generated mocks let a consumer-agent finish before the producer-agent does.**
- **One worktree per agent**; partition on graph boundaries; sequence agents that
  touch a shared file (parallel implementers conflict on shared files — verify
  each agent's output yourself).
- **Verification gates are non-negotiable**, because agents over-claim "done /
  green." Never trust a claim without running the gate that actually fails — and
  beware CI-only gates invisible to a local check, and `| head` / `| tail`
  masking exit codes. Where no registry tooling fits, a **snapshot/golden test is
  the parallel-safety gate**: an intentional contract change forces the author to
  regenerate the snapshot, so the PR diff literally shows reviewers how the
  contract is mutating — turning a silent break into a conscious, reviewable
  approval. A multi-lens review (correctness + design/blast-radius) plus reading
  the online review before merge catch different bug classes than one agent does.

How lessons from these runs persist (so the next idea is cheaper) is the job of
[`KNOWLEDGE-ENGINEERING.md`](KNOWLEDGE-ENGINEERING.md) — the conveyor that turns
an incident into a review finding into a checklist into a gate.

## Worked example: pixtuoid

pixtuoid is a Cargo workspace `pixtuoid-core ← pixtuoid-scene ← {pixtuoid,
pixtuoid-web}` (plus the standalone `pixtuoid-hook` shim), an Astro **site**,
and a Raycast **TS extension** — one producer chain + its consumers in one
monorepo (the `pixtuoid-web` wasm painter is itself a site build input:
`just gen-wasm` → the committed `site/public/wasm/`). The cross-area contract is **`pixtuoid … --json`** (the
`SourceStatus` / `OutcomeRow` DTOs).

- **Pinning:** the Rust side pins the shape with the `source_status_json_shape`
  test; where no schema tool fits (the serialized `SceneState`, the terminal
  render output) it freezes the shape with **golden / snapshot tests** whose
  regeneration forces a reviewable PR diff (`gen-check` stills, the snapshot
  golden). That snapshot-as-gate is exactly the parallel-safety mechanism above.
- **Codegen-from-one-source, applied:** the `--json` `SourceStatus` and
  `OutcomeRow` types are **generated, not hand-mirrored**. A `schemars` derive
  on each Rust serde type emits a committed JSON Schema
  (`integrations/raycast/contract/{source-status,outcome-row}.schema.json`,
  freshness-gated by Rust golden tests), and the Raycast extension generates its
  TS types from those schemas (`json-schema-to-typescript`, CI-checked fresh by a
  regenerate-and-`git diff` step). So a producer shape change is a **compile error
  in the consumer** — exactly the load-bearing guard above, dogfooded. (The
  earlier tier — a Rust byte-shape test + a hand-typed mirror — is what this
  replaced.)
- **Per-area gates** (each verifies independently): Rust → `just preflight` +
  `semver` + `gen-check`; site → `just site-check`; raycast → `tsc --noEmit` +
  `eslint`. Scoped per-area `CLAUDE.md`/`AGENTS.md` keep each agent on its own
  rules ([`integrations/raycast/CLAUDE.md`](../integrations/raycast/CLAUDE.md),
  [`site/CLAUDE.md`](../site/CLAUDE.md)).

The fan-out itself is a deterministic workflow: freeze the contract (one agent),
barrier, then three worktree-isolated agents, then the producer-first join. A
runnable shape (Claude Code's Workflow tool):

```js
// Phase 0 happened already: the --json shape + its pinning test are on the branch.
phase('Fan out')
const areas = [
  // guide is explicit: the rust area's house rules live in the ROOT CLAUDE.md
  // (there is no crates/CLAUDE.md), the consumers' in their own directories
  { key: 'rust',    dir: 'crates/',             guide: 'CLAUDE.md',                      gate: 'just preflight && just semver && just gen-check' },
  { key: 'site',    dir: 'site/',               guide: 'site/CLAUDE.md',                 gate: 'just site-check' },
  { key: 'raycast', dir: 'integrations/raycast/', guide: 'integrations/raycast/CLAUDE.md', gate: 'cd integrations/raycast && npx tsc --noEmit && npx eslint .' },
]
const results = await parallel(areas.map((a) => () =>
  agent(
    `Implement the <feature> in ${a.dir} against the FROZEN pixtuoid --json contract ` +
    `(SourceStatus/OutcomeRow). Read ${a.guide} for this area's house rules. ` +
    `Run its gate and report the EXIT CODE you observed: ${a.gate}`,
    { label: `area:${a.key}`, isolation: 'worktree' },   // each agent gets its own worktree
  )))
// Join: producer (rust) merges first, then site regenerates media against the real
// binary (just gen + gen-check) and raycast runs a live `pixtuoid … --json` smoke.
```

## Map it onto a server / Android / iOS / Lynx monorepo

Same shape, polyglot fleet → the cross-platform contract should be
**Protobuf/gRPC or Smithy** (one model → typed messages + stubs everywhere) or
**OpenAPI** for REST tooling; **tRPC is out** (can't reach Kotlin/Swift).

- **Server (backend / shared lib)** — owns and **exports** the contract (Stripe
  pattern: internal source-of-truth → CI-checked OpenAPI/`.proto` on `main` → one
  generator). Self-verifies at runtime (Dredd/Schemathesis vs its own spec, or
  provider verification / `buf breaking` on its `.proto`). It's the high-fan-in
  node, so it's where remote caching + distributed execution and contract pinning
  matter most.
- **Android (Kotlin)** — consumes a **pinned, pre-generated Kotlin SDK** from the
  package manager (grpc-kotlin / protobuf-lite via Gradle from a schema registry,
  or an OpenAPI-generated Retrofit client). Never run `protoc` locally; pin the
  *full* SDK version for determinism; measure APK-size impact for your build.
- **iOS (Swift)** — consumes SwiftProtobuf (messages) + grpc-swift (stubs) — two
  *separate* codegen projects — via SwiftPM from the registry, same pin
  discipline.
- **Lynx web (TS)** — if the surface is GraphQL, the SDL is the contract and
  GraphQL Code Generator types exactly the operations written in client code (a
  query on a dropped field fails at codegen); add a persisted-operations manifest
  enforced server-side. If REST/gRPC, consume `openapi-typescript` / `@hey-api`
  (types + Zod runtime validation) or `grpc-web`. Lynx's TS runtime *can* use a
  tRPC-style internal surface, but only over the same backend that serves the
  native platforms via the real cross-platform contract.

Deploy in the order the compatibility mode dictates (BACKWARD → consumers first;
FORWARD → producer first), gated by a version-aware release check (a registry's
`can-i-deploy`-style matrix) so each side ships when it passes against the
versions already in prod — no coordinated release window.

## Pitfalls (the ones that bite)

- **Mocks without runtime contract testing give false safety** — pair generated
  mocks with Dredd/Pact/Specmatic; without a runtime contract test, mock-backed
  tests keep passing CI while the real backend has already drifted.
- **Codegen is garbage-in-garbage-out** — the compile-time guard only enforces
  what the spec faithfully encodes; mis-specified nullability silently produces
  wrong-but-compiling clients. Keep a runtime-validation layer (e.g. Zod from the
  spec). An ad-hoc `--json` with hand-written consumer types **is** this failure
  mode unless the producer emits a committed, CI-checked schema.
- **`HEAD~1` as the affected base skips changes since the last green run** — use
  the last successful commit / the PR merge-base.
- **Never reuse a Protobuf field number;** `reserved` retired numbers/names or a
  future field silently re-binds an old wire slot (enforced by `protoc` + `buf
  breaking`, not review).
- **Schema-registry SDKs need a pinned, explicit version** — installing by a
  mutable `main`/branch reference resolves to different generated code over time.
- **Concurrent agents on the shared checkout race on `HEAD`** — always a worktree
  per agent. And agents over-claim "green": run the failing gate yourself.

## Steal this (adoption order)

1. **Name the contract** for your cross-cutting seam and make it machine-readable
   (OpenAPI / `.proto` / SDL / a JSON Schema emitted from your types).
2. **Pin it mechanically** — a breaking-diff gate on every PR; where no tooling
   fits, a golden/snapshot test whose regeneration forces a reviewable diff.
3. **Generate, don't hand-sync** consumer types from the one source; regenerate
   in CI so a producer break is a consumer compile error.
4. **Freeze, then fan out** — one author locks the contract (review it
   adversarially first); then parallel workers, each in its own worktree, scoped
   by a per-area context file.
5. **Join on the producer** — it merges first; consumers regenerate and verify
   against it; serialize the land with a merge queue.
6. **Persist the lessons** down the durability ladder
   ([`KNOWLEDGE-ENGINEERING.md`](KNOWLEDGE-ENGINEERING.md)) so the next idea is
   cheaper than this one.

## Sources

- APIs You Won't Hate — *A Developer's Guide to API Design-First* — https://apisyouwonthate.com/blog/a-developers-guide-to-api-design-first/
- APIContext — *OpenAPI Specifications in the Real World* (650M calls / 10k+ endpoints; ~75% of APIs don't conform to their spec — "API drift") — https://apicontext.com/resources/api-drift-white-paper/
- totalshiftleft — *API schema validation: catching drift* (the runtime-conformance gate; defense in depth) — https://totalshiftleft.ai/blog/api-schema-validation-catching-drift
- hey-api/openapi-ts — typed SDKs + Zod from one spec, compile-time break detection — https://github.com/hey-api/hey-api
- brandur.org — *The state of Stripe API library codegen* (one source → OpenAPI → many SDKs) — https://brandur.org/fragments/stripe-codegen
- Buf — *Detecting breaking changes* + *Consuming generated SDKs* (BSR, FILE/PACKAGE/WIRE) — https://buf.build/docs/breaking/ · https://buf.build/docs/bsr/generated-sdks/
- oasdiff — OpenAPI breaking-change CI gate — https://github.com/oasdiff/oasdiff-action
- Apollo — *Federated schema checks* (`rover subgraph check`) — https://www.apollographql.com/docs/federation/v1/managed-federation/federated-schema-checks
- Confluent — *Schema evolution & compatibility types* (BACKWARD/FORWARD/FULL, TRANSITIVE) — https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html
- Pact — *can-i-deploy* (independent deploys) — https://docs.pact.io/pact_broker/can_i_deploy
- Nx — *Run only tasks affected by a PR* + *Parallelization and distribution* — https://nx.dev/docs/features/ci-features/affected
- The Guild — *GraphQL Code Generator* (operation-aware types, persisted operations) — https://the-guild.dev/graphql/codegen
- Smithy 2.0 — protocol-agnostic multi-language IDL — https://smithy.io/2.0/
- Google AIP-191 + Protobuf best practices (versioned package, reserved field numbers) — https://google.aip.dev/191
