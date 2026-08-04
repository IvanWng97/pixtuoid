// Passes only while the LIVE advisory set equals the adjudicated one below: an
// unreviewed advisory reds CI, and one that clears upstream ALSO reds it, so a
// refusal cannot outlive the condition it was written for.
import { spawnSync } from "node:child_process";

// Verify before adjudicating with `npm audit`, `npm view <pkg>@<v> dependencies`,
// and `gh api /advisories/<GHSA>` — that last one for a backport to the line you
// are actually pinned to, NOT just the lowest `first_patched_version` (#792 read
// "patched: 5.0.8" as "the 2.x line is unfixable" and refused a fix that landed
// in 2.1.3).
const ADJUDICATED = new Map();

const audit = spawnSync("npm", ["audit", "--json"], { encoding: "utf8" });
if (audit.error) throw audit.error;

let report;
try {
  report = JSON.parse(audit.stdout);
} catch {
  throw new Error(`npm audit produced no JSON (exit ${audit.status}):\n${audit.stderr || audit.stdout}`);
}

const live = new Map();
for (const vuln of Object.values(report.vulnerabilities ?? {})) {
  for (const via of vuln.via ?? []) {
    // A string `via` is a sibling package this one inherits from; only an object
    // carries the advisory itself, so ids come from the roots and never duplicate.
    if (typeof via === "object" && typeof via.url === "string") {
      live.set(via.url.split("/").pop(), `${via.severity}: ${via.title}`);
    }
  }
}

const unreviewed = [...live].filter(([id]) => !ADJUDICATED.has(id));
const stale = [...ADJUDICATED.keys()].filter((id) => !live.has(id));

for (const [id, detail] of unreviewed) {
  console.error(`UNREVIEWED ${id} — ${detail}\n  Fix it, or adjudicate it in ${import.meta.filename}.`);
}
for (const id of stale) {
  console.error(`STALE ${id} — no longer reported. Take the upstream fix and delete its entry.`);
}
if (unreviewed.length || stale.length) process.exit(1);

console.log(`audit: ${live.size} advisory/advisories, all adjudicated`);
