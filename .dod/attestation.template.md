# Definition-of-Done attestation

Copy to `.dod/attestation.md` (gitignored) and tick the boxes for the change on
THIS branch. The Stop hook reads it and blocks turn-end on a *code* branch with
unticked required boxes (debounced once per tree-state). Boxes are class-gated to
avoid nagging: a docs-only/clean tree needs none; feature-shaped work (≥3 files /
new seam) also needs Design + Impl-plan; a public-surface change also needs
Docs-currency.

Bypass — AGENT-LAYER ONLY, always logged (the CI `definition-of-done` job ignores
it, so a bypassed push still meets a gate no env var can relax): set
`DOD_BYPASS="<reason>"` in the env, or add a line `DOD-BYPASS: <reason>` below.

- [ ] TDD: a failing test preceded the implementation
- [ ] Self-review: re-read the diff / ran `/simplify`
- [ ] Design: brainstormed / considered the design (feature-shaped work)
- [ ] Impl-plan: planned against `.github/prompts/impl-plan.prompt.md`
- [ ] Docs-currency: README + nearest `CLAUDE.md` updated for any public-surface change
