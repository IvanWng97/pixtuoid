# Upstream wire-format drift — incident log

The institutional record of every **actual** upstream drift that reached
pixtuoid: what changed, **which defense layer caught it**, how late, and the fix.

Why this exists: the drift defense is layered (semantic detection → in-code
self-monitoring → internal consistency tests → weekly upstream watch), and we've
*claimed* "layer 2 (real-stream self-monitoring) is the real backstop for
closed-binary CLIs." This log turns that claim into **data** — the
`#266` review-history census reads it to measure the defense's true-positive
rate and catch-latency, and to retire layers that never fire.

Layers (see `crates/pixtuoid-core/CLAUDE.md` "Keeping the decode mapping current"):
- **L1 — semantic detection** (e.g. `subagent_type` over the tool name): survives a
  rename by construction.
- **L2 — in-code self-monitoring** from the real stream (`bail!`/`debug!`
  breadcrumb / shape-drift warn): sees ground truth, no network. Catches
  *undocumented* drift.
- **L3 — internal consistency tests** (`every_registered_*_event_decodes`).
- **L4 — weekly upstream watch** (`scripts/check_upstream_drift.py`): docs/schema
  diff; weakest for closed binaries (docs lag/omit).

## How to add a row

When a real drift is found (a user report, a `pixtuoid doctor` finding, a weekly
watch alarm, or a maintainer observation), add a row. `caught_by` is the layer
that *first surfaced* it; `latency` is roughly how long it shipped before we
noticed (or "unknown" for undocumented renames found by observation).

| date | source | what drifted | caught_by | latency | fix |
|---|---|---|---|---|---|
| 2026-05 (approx) | claude-code | subagent-dispatch tool renamed `Task` → `Agent` (CC v2.1.63, **undocumented**, upstream #29677) — silently disabled subagent suppression + b1 completion (the parent showed the subagent's tools) | **L2** (observed live; L4 doc-scrape could NOT — the rename was undocumented) | unknown (undocumented) | L1 hardening: `make_tool_detail` keys on the stable `subagent_type` input field, not the tool name (survives the next rename by construction); the name set is now a fallback + a `debug!` drift breadcrumb. Wire-monitoring added (`#79/#80` era). |

## Reading the log

- Most rows in **L2** for closed binaries (CC/Cursor) ⇒ the doc-scrape (L4) is
  structurally blind to undocumented change; that's the argument for surfacing
  L2's breadcrumbs to users (the `doctor` / footer self-diagnosis work).
- An L4 row ⇒ the weekly watch earned its keep on that source.
- A long `latency` ⇒ the defense was slow there; consider a tighter cadence or a
  real-capture canary for that CLI.
