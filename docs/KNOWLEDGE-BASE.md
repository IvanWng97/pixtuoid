# The knowledge base

How this repo stops paying for the same lesson twice.

pixtuoid is built almost entirely by AI coding agents, reviewed by fleets of
AI reviewers, and maintained across hundreds of agent sessions. That makes it
a live laboratory for a question every team adopting agents hits eventually:
**where does the knowledge live, and who pays when it doesn't?**

This page documents the system we run — four layers, each with a job, a known
failure mode, and a counter-measure — plus the numbers we collected before
turning it on, so the after-side is measurable rather than vibes.

## Why — the baseline numbers

Between May 29 and June 11, 2026, this repo ran 21 review-class multi-agent
workflows: **1,177 agents, 7.1M output tokens** — and **75.3% of those output
tokens were spent in the Verify stage**, adversarially adjudicating candidate
findings (the June-9 whole-codebase review alone dispatched 110 verifiers
against 3 finders). Worse, the two whole-codebase reviews re-adjudicated overlapping
claims: June 9 refuted 26 of 49 candidates; in the June-10 review a third of
distinct candidates (12 of 37) ended refuted — among them re-derivations of
ground June 9 had already adjudicated (June-10's `/tmp`-socket and EMFILE
refutations are preserved as ledger rows R0610-13/14; June-9's copies survive
only in its grouped record — exactly the bookkeeping gap the ledger closes).
Full data:
[`baseline-2026-06.md`](review-metrics/baseline-2026-06.md).

Verification spend re-adjudicating known ground is the cost the ledger
attacks. The knowledge base is the fix — under one design constraint learned
the hard way (see "The graveyard rule" below).

## Layer 1 — Context files (knowledge agents load without asking)

**What:** `CLAUDE.md` at the workspace root plus nested per-crate files, with
[`AGENTS.md`](https://agents.md/) symlinked to the root file so every agent
CLI (Claude Code, Codex, Cursor, Copilot, Gemini…) reads the same source. The
load-bearing content is the **"Known sharp edges"** sections: things that look
like bugs but are deliberate, each with its WHY. In review after review, these
are what kill false positives — a design-intent skeptic armed with sharp edges
refuted 26 candidates in one June run.

**Failure mode:** monotonic bloat. Rules force additions; nothing forces
deletions; signal density decays. Industry evidence is brutal here: an ETH
Zurich study (arXiv:2602.11988) found LLM-generated context files *reduced*
agent success in 5 of 8 settings while raising cost ~20%; the converged
practice at frontier labs is a ~100–300 line **map, not a manual**, pointing
into deeper docs.

**Counter:** size budgets, citation tracking (a sharp edge no review has cited
in two quarters is a demotion candidate), and periodic audits
(`/revise-claude-md`) — paired metrics: auto-loaded token cost vs. sharp-edge
citation hit rate.

## Layer 2 — Retrieval (knowledge computed on demand, never stored)

**What:** agentic search (grep + LSP) over the live tree, plus build-system
dependency graphs computed at review time (`cargo modules`) to expand a diff
into its affected-subsystem set. Nothing is indexed ahead of time.

**Failure mode:** persistent code indexes drift the moment they're built — and
dependency graphs are blind to **non-code coupling** (this repo's
`media.json` ↔ `showcase.json` ↔ README-generation triangle has zero imports
between its corners).

**Counter:** indexes are computed per-use and discarded; cross-boundary
couplings are either declared in a small manual map or — better — pinned by a
**bridge test** (`supported_sources_manifest` pins a JSON manifest to a Rust
const list; the build fails if they diverge). If a coupling matters, a test
enforces it; if no test enforces it, reviews assume it will be missed.

## Layer 3 — Memory (episodic, not yet distilled)

**What:** agent session memory captures raw lessons with zero friction; a
periodic distillation pass promotes the keepers into the repo — sharp edges,
conventions, or this page. The pipeline is one-directional:
**capture → distill → promote → expire the raw entry.**

**Failure mode:** both directions die. Unmanaged automatic capture rots — a
publicly audited self-hosted memory deployment hit **97.8% junk in 32 days**
(mem0ai/mem0#4573). Gated capture starves — put an MR in front of writing a
memory and people stop writing. And recalled-but-not-applied is real: this
repo's maintainer agent repeated a known `preflight | tail` exit-code mistake
*while the lesson sat in its own memory*.

**Counter:** friction goes on the distiller, never the capturer; recurrence is
the promotion trigger (twice = it leaves prose and becomes a rule); and the
ladder has a top rung — see Layer 4. Git-native, review-gated promotion is
the only team-memory pattern we've found without a public postmortem as of
mid-2026.

## Layer 4 — The review ledger (adjudications as institutional memory)

**What:** [`REVIEW-LEDGER.md`](REVIEW-LEDGER.md) records every adjudicated
review finding: the seam, the claim, the verdict, and — critically — the
**anchor**: the specific mechanism (file + sharp edge + HEAD) that justified
it. Future reviews match candidates against the ledger before spending
verifier tokens.

**Failure mode:** a naive suppression list hides real bugs. This repo has the
proof: June 9 correctly refuted a socket-steal claim; June 10 found a
*different* socket-steal on the **same seam** that was real (a
backlog-saturated live daemon returns `ECONNREFUSED` on macOS, so a second
instance reclaims a live socket — fixed by flock arbitration in PR #235).
A fuzzy-matched kill list would have suppressed it.

**Counter — the protocol** (full version in the ledger header):

1. A ledger match **demotes, never kills**: the candidate goes to one cheap
   checker ("does the cited mechanism still refute *this* claim?") instead of
   a full adversarial panel.
2. Anchors expire: if `git diff <verdict-HEAD>..HEAD -- <anchor paths>` shows
   the anchoring code changed, the entry is void for that candidate.
3. Only verdicts citing a documented sharp edge or merged PR get the fast
   path; judgment calls always re-verify.
4. Every Nth review runs ledger-blind as a calibration pass; findings the
   ledger would have suppressed but the clean run confirms = the measured
   **false-suppression rate**.
5. Append-only; flipped verdicts supersede, never overwrite — the flip itself
   is knowledge.

And the terminal rung of the whole system: **a lesson that recurs stops being
text and becomes a lint, a CI gate, or a type**. Prose is the weakest storage
format for knowledge; this repo's MSRV lesson is a `just msrv` gate, its
shell-injection lesson is a `CMD_UNSAFE` guard, its socket-path lesson is a
parity test. Documentation can be ignored; the build cannot.

## The graveyard rule

This repo once had a beautifully written `.claude/agents/pixtuoid-dev.md` —
architecture, conventions, sprite workflow, exit criteria. It was never
loaded by anything, went stale within weeks (it described files that had been
restructured), and was eventually deleted with its one not-already-covered
rule salvaged into `CLAUDE.md`. The lesson generalizes to every layer above:

> **Knowledge files live or die by their load path, not their quality.**
> Design the automatic reader first, then write the content. Every layer's
> maintenance must ride an existing habit — the review workflow writes the
> ledger, CI runs the lints, the audit sits on a calendar — because knowledge
> that needs someone to *remember* to maintain it is already dead.

## Measuring it

The collector (`scripts/review-metrics.py`) turns any review workflow journal
into per-stage token/agent metrics. The experiment design, for anyone
replicating this on their own repo:

| experiment | metric | quality guard |
|---|---|---|
| A/B: same-HEAD review with / without ledger | verifier tokens, repeat-refutation count | confirmed-findings held constant |
| onboarding proxy: standard tasks with / without KB | first-pass gate rate, review nits | task completion |
| ledger calibration (every Nth review, ledger-blind) | false-suppression rate | — |
| context health (quarterly) | auto-loaded tokens vs. sharp-edge citation rate | — |

Efficiency metrics are only reported **paired with their quality guard** —
a review that got cheaper by finding less is not a saving.
