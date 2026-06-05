#!/usr/bin/env node
// Keep the README in sync with the site's single-source data files:
//   • Features table  ← site/src/features.json   (GENERATED between markers)
//   • install commands ← site/src/install.json    (CHECKED — must appear verbatim)
// The site (Features.astro / Install.astro) reads the same JSON, so the README and
// the site can't drift. Run `just gen-readme` (or `node site/scripts/gen-readme.mjs`)
// after editing either JSON. `--check` writes nothing and exits non-zero on drift
// (used by CI: `npm run readme:check`).
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import process from 'node:process';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const readmePath = join(root, 'README.md');
const features = JSON.parse(readFileSync(join(root, 'site', 'src', 'features.json'), 'utf8'));
const install = JSON.parse(readFileSync(join(root, 'site', 'src', 'install.json'), 'utf8'));

const check = process.argv.includes('--check');
let readme = readFileSync(readmePath, 'utf8');
const errors = [];

// --- Features table (generated between markers) ---
const START =
  '<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->';
const END = '<!-- features:end -->';
const rows = features.map((f) => `| ${f.icon} | **${f.name}** | ${f.desc} |`);
const block = `${START}\n${['| | Feature | Description |', '|---|---|---|', ...rows].join('\n')}\n${END}`;
const re = new RegExp(`${escapeRe(START)}[\\s\\S]*?${escapeRe(END)}`);
if (!re.test(readme)) {
  console.error(`gen-readme: features markers not found in README.md. Expected:\n\n${block}\n`);
  process.exit(1);
}
const withFeatures = readme.replace(re, block);
if (check) {
  if (withFeatures !== readme) {
    errors.push(
      'README Features table is stale — run `just gen-readme` after editing features.json.'
    );
  }
} else if (withFeatures !== readme) {
  readme = withFeatures;
  writeFileSync(readmePath, readme);
  console.log(`✓ README Features table regenerated (${features.length} features)`);
} else {
  console.log('README Features table already up to date ✓');
}

// --- Install commands (checked, not generated — the README install prose is
// hand-curated, but every canonical command must appear in it verbatim) ---
const current = readFileSync(readmePath, 'utf8');
for (const m of install) {
  if (!m.readmeCheck) continue; // site-only method (e.g. Debian)
  for (const cmd of m.cmds) {
    if (!current.includes(cmd)) {
      errors.push(
        `README is missing the ${m.label} install command from install.json: \`${cmd}\` — update the README to match, or fix install.json.`
      );
    }
  }
}

if (errors.length) {
  console.error(errors.map((e) => `✗ ${e}`).join('\n'));
  process.exit(1);
}
if (check) console.log('README is in sync with features.json + install.json ✓');
else console.log('README install commands match install.json ✓');

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
