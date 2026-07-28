// `npm ci` here prints "N high severity vulnerabilities" and nothing reads it, so a
// NEW advisory is indistinguishable from the adjudicated backlog. This gate turns the
// backlog into data: it passes only while the LIVE advisory set equals the adjudicated
// one below, so an unreviewed advisory reds CI, and an advisory that clears upstream
// ALSO reds it — a refusal cannot outlive the condition it was written for.
import { spawnSync } from "node:child_process";

// Verify a claim before trusting it: `npm audit`, `npm view <pkg>@<v> dependencies`,
// and `gh api /advisories/<GHSA>` are the three commands behind every line here.
const ADJUDICATED = new Map([
  [
    "GHSA-mh99-v99m-4gvg",
    "brace-expansion DoS. Every 1.x-4.x line is unpatched (first patched: 5.0.8), so " +
      "the only fix for the filelist/node_modules/brace-expansion 2.x copy is a major " +
      "jump — and 5.x turned the export from a function into an object while minimatch " +
      "5.1.9 calls it as `expand(pattern)`, so an override makes `npm audit` green over " +
      "code that throws 'expand is not a function' (tried, #792). The chain is pinned " +
      "from the top and we own no link in it: @oclif/core -> ejs ^3 -> jake ^10 -> " +
      "filelist ^1 -> minimatch ^5. jake 12 and filelist 2 already sit on minimatch ^10 " +
      "-> brace-expansion 5.0.8, so this clears when @oclif/core moves off ejs ^3.",
  ],
]);

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
