# Parallel delivery

Shipping one cross-cutting change across a producer and its consumers in
parallel — whether the workers are humans or AI agents. The principle: **the
contract is the synchronization primitive.** Freeze it first; fan out against
it; re-join on the producer. Parallelism without a frozen contract is just
distributed guessing.

## The three phases

- **Phase 0 — freeze the contract.** One author writes a versioned,
  machine-readable spec (OpenAPI / `.proto` / SDL / a JSON Schema emitted from
  your types) plus its codegen and pinning gates; nothing forks until it is
  reviewed. Spec lint + breaking-diff gates (Spectral, `oasdiff`,
  `buf breaking`) make "can't drift" mechanical; runtime conformance checks
  (Dredd / Schemathesis) close the gap spec-only gates miss — most APIs drift
  from their own spec in production.
- **Phase 1 — fan out by area.** Each worker builds against the frozen
  contract in its own git worktree. Typed SDKs generated from the one spec
  turn a producer break into a consumer **compile error**; generated mocks
  unblock consumers before the producer exists. Partition along
  dependency-graph boundaries; sequence workers that share files.
- **Phase 2 — join on the producer.** The producer merges first (it is the
  source of the shape); each consumer regenerates and verifies against it; a
  merge queue serializes the land.

## Worked through pixtuoid

The workspace is one producer chain (`pixtuoid-core ← pixtuoid-scene ←
{pixtuoid, pixtuoid-web}`) plus two non-Rust consumers — the Astro site and
the Raycast extension. The cross-area contract is `pixtuoid … --json`
(`SourceStatus` / `OutcomeRow`): `schemars` derives emit committed JSON
Schemas, the Raycast extension generates its TS types from them (CI-checked
fresh), and where no schema tool fits, golden/snapshot tests make a contract
change a reviewable PR diff (`gen-check`). Per-area gates verify
independently — `just preflight` + semver + gen-check (Rust),
`just site-check` (site), `tsc` + `eslint` (Raycast) — and each area's house
rules live in its own `CLAUDE.md`.

## The pitfalls that bite

- Mocks without runtime contract testing keep CI green while the real
  producer has already drifted.
- Codegen enforces only what the spec faithfully encodes — keep a runtime
  validation layer (e.g. Zod generated from the spec).
- Concurrent agents on one checkout race on `HEAD` — one worktree per agent.
- Agents over-claim "green": run the failing gate yourself, and never read an
  exit code through a pipe.

Pick the contract language by consumer polyglotism, not preference:
Protobuf/gRPC or Smithy for a multi-language fleet, OpenAPI for REST tooling,
GraphQL for schema-shaped frontends — tRPC cannot reach native Kotlin/Swift
consumers. Schema-compatibility mode dictates deploy order: `BACKWARD` ⇒
consumers first; `FORWARD` ⇒ producer first; `FULL` ⇒ any order.
