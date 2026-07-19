<!--
Thanks for contributing to pixtuoid! Before you open this PR:
- Read CLAUDE.md (root) — architecture invariants & conventions are load-bearing.
- Run `just preflight` locally (it IS what CI runs: fmt + machete + deny + clippy -D warnings + tests).
Delete sections that don't apply. Keep it short — the diff speaks for itself.
-->

## Summary

<!-- What does this change and why? One or two sentences. -->

## Related issue

<!-- e.g. "Closes #123" / "Part of #123" — or "n/a". -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] New theme / sprite pack
- [ ] New `Source` adapter (another agent CLI)
- [ ] Docs only
- [ ] Refactor / chore

## How I tested it

<!--
- `cargo test --workspace`  (or `just test`)
- just preflight (full CI gate)
- Live: ./target/release/pixtuoid run --headless --projects-root ~/.claude/projects
-->

## Visual verification (required for sprite / rendering changes)

<!--
Skip this block for non-visual changes. For any .sprite edit or change to the
pixel painter, attach a cropped snapshot and self-critique:
  cargo build --release --example snapshot
  ./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
  .venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3
-->

- [ ] Not a visual change, or — screenshot/GIF attached below and it reads at half-block scale.

## Checklist

- [ ] I read the relevant `CLAUDE.md` (root + the nested one for the crate I touched).
- [ ] New behavior has a test (this repo is TDD-first).
- [ ] No `unwrap()` in non-test code; errors propagate via `anyhow`/`thiserror`.
- [ ] No new `println!`/`eprintln!` on a production path (use `tracing`).
- [ ] Docs updated in the same commit if I changed module structure, architecture, or public API (`CLAUDE.md` / `README.md`).
- [ ] Checked against the [recurring pitfalls](https://github.com/IvanWng97/pixtuoid/blob/main/docs/CONTRIBUTING.md#recurring-pitfalls-this-codebases-review-history-distilled): char-safe slicing · no parallel copies without a bridge test · sanitize at the decode boundary · negative branches pinned.
- [ ] `just preflight` passes locally.

## AI assistance

<!-- If this PR was authored or heavily assisted by an AI agent, say so — a maintainer
     will visually verify before merge (see the `needs-human-verify` label). -->

- [ ] This PR was written/heavily-assisted by an AI agent.
