# PR Review Rules for pixtuoid

## Setup

Read `CLAUDE.md` at the repo root first. It contains architecture invariants, known sharp
edges, and conventions that are load-bearing. Your review must be grounded in that context.

Then read `gh pr diff` to understand all changes in this PR.

## What to review

Focus exclusively on HIGH-confidence findings. Every finding must be something you verified
by reading actual code — no guessing, no "this might be an issue."

### Must check

1. **Architecture invariant violations** (the 6 invariants in CLAUDE.md):
   - `pixtuoid-core` importing terminal dependencies (ratatui, crossterm)
   - Events bypassing the typed `mpsc` channel or hardcoding `Transport::Hook`
   - Source implementations not going through the `Source` trait
   - `install-hooks` not using `write_config_atomic` for settings.json
   - Hook shim doing anything other than exit 0 on error
   - Walkable mask blocking more than ground footprint

2. **Real bugs**: logic errors, off-by-one, race conditions, missing error propagation.

3. **Missing test coverage**: new behavior without a corresponding test (this repo is TDD-first).

4. **`unwrap()` in non-test code**: always a finding.

5. **Scope creep**: changes that add v2 features or speculative abstractions not in the v1 spec.

6. **Stale docs**: if the PR changes module structure, architecture, or public API without
   updating CLAUDE.md/README.md.

### Do NOT flag

- Formatting or style (rustfmt enforced in CI)
- Missing comments or docstrings (repo convention: no comments unless WHY)
- Clippy warnings (enforced in CI with `-D warnings`)
- Speculative future issues ("this could become a problem if...")
- Anything cargo-deny, cargo-machete, or CI already catches
- Performance unless measurable (this is a TUI rendering ~30fps, not a hot loop)

## Anti-hallucination protocol

- Every file:line you cite MUST be from a file you actually read with the Read tool
- Do not invent line numbers. If you can't find the exact line, describe the location
- If you're unsure whether something is a bug or intentional, check "Known sharp edges" in CLAUDE.md before filing
- Verify your premise before each finding: does the code actually do what you think it does?

## Severity

- **HIGH**: must fix before merge — real bug, invariant violation, missing critical test
- **MEDIUM**: worth fixing — scope creep, stale docs, defense-in-depth gap

No LOW findings. If it's not worth fixing, don't mention it.

## Output format

- Post inline comments on specific lines via `mcp__github_inline_comment__create_inline_comment`
- Cap at 5 findings total
- Always post exactly one summary comment via `gh pr comment`, even on clean PRs:

```
<!-- claude-auto-review:summary -->
## Claude Review

**Findings: N** (X high, Y medium) — or "No findings"

[One sentence overall assessment]

| # | Severity | File | Finding |
|---|----------|------|---------|
| 1 | HIGH     | path:line | description |

---
*Automated review by Claude Code*
```
