#!/usr/bin/env node
// Keep the README in sync with the site's single-source data files:
//   • Features table          ← site/src/features.json  (GENERATED between markers)
//   • Supported-tools glimpse ← site/src/sources.json   (GENERATED between markers)
//   • Install block           ← site/src/install.json   (GENERATED — `readme:true` methods only)
// The site (Features.astro / SupportedTools.astro / Install.astro) reads the same
// JSON, so the README and the site can't drift. The supported-tools glimpse shows
// only the FEATURED tools + a link to the full tool × OS matrix on the site, so the
// README stays short as more agent CLIs are added. Run `just gen-readme` (or
// `node site/scripts/gen-readme.mjs`) after editing any JSON. `--check` writes
// nothing and exits non-zero on drift (used by CI: `npm run readme:check`).
//
// NOTE: the manifest's *supported* set is pinned to the code's REGISTERED_SOURCES
// by a Rust test (crates/pixtuoid-core/tests/supported_sources_manifest.rs) that
// runs in the main CI — so the marketing list can never claim "supported" for a
// source that isn't actually wired (and a newly-wired source forces a manifest
// update). This script only owns rendering + README/site parity.
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import process from 'node:process';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const readmePath = join(root, 'README.md');
const features = JSON.parse(readFileSync(join(root, 'site', 'src', 'features.json'), 'utf8'));
const sources = JSON.parse(readFileSync(join(root, 'site', 'src', 'sources.json'), 'utf8'));
const install = JSON.parse(readFileSync(join(root, 'site', 'src', 'install.json'), 'utf8'));

const SITE = 'https://ivanwng97.github.io/pixtuoid';
const check = process.argv.includes('--check');
let readme = readFileSync(readmePath, 'utf8');
const errors = [];

// Neutralize only what breaks a GFM table row: `|` splits columns (use the
// HTML entity — backslash-escaping would itself need backslash escaping first,
// CodeQL js/incomplete-sanitization) and newlines split rows. Cell text is
// intentionally markdown-bearing (backticks, `A\*`), so nothing else is touched.
const cell = (s) => String(s).replace(/\|/g, '&#124;').replace(/\r?\n/g, ' ');

// Regenerate the block between `start`/`end` markers from `body`. () => block:
// a replacer FUNCTION inserts the value literally — a plain string would expand
// `$`-patterns ($$, $&, $') lurking in the text and silently corrupt the README
// in a way readme:check can't see (both sides of its comparison would go through
// the same mangling). Updates the in-memory `readme`; writes the file on change.
function regenSection(label, start, end, body) {
  const block = `${start}\n${body}\n${end}`;
  const re = new RegExp(`${escapeRe(start)}[\\s\\S]*?${escapeRe(end)}`);
  if (!re.test(readme)) {
    console.error(`gen-readme: ${label} markers not found in README.md. Expected:\n\n${block}\n`);
    process.exit(1);
  }
  const next = readme.replace(re, () => block);
  if (next === readme) {
    console.log(`README ${label} already up to date ✓`);
    return;
  }
  if (check) {
    errors.push(`README ${label} is stale — run \`just gen-readme\` after editing the JSON.`);
  } else {
    readme = next;
    writeFileSync(readmePath, readme);
    console.log(`✓ README ${label} regenerated`);
  }
}

// --- Features table ---
const featureRows = features.map(
  (f) => `| ${cell(f.icon)} | **${cell(f.name)}** | ${cell(f.desc)} |`
);
regenSection(
  'Features table',
  '<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->',
  '<!-- features:end -->',
  ['| | Feature | Description |', '|---|---|---|', ...featureRows].join('\n')
);

// --- Supported-tools glimpse (FEATURED only + a link to the full site matrix) ---
const OS_LABELS = { macos: 'macOS', linux: 'Linux', windows: 'Windows' };
const OS_ORDER = ['macos', 'linux', 'windows'];
const runsOn = (s) =>
  OS_ORDER.filter((os) => s.platforms?.[os] === 'yes' || s.platforms?.[os] === 'experimental')
    .map((os) => (s.platforms[os] === 'experimental' ? `${OS_LABELS[os]}\\*` : OS_LABELS[os]))
    .join(' · ');
const hasExperimental = sources.some(
  (s) => s.status === 'supported' && Object.values(s.platforms || {}).includes('experimental')
);

const featured = sources.filter((s) => s.status === 'supported' && s.featured);
const otherSupported = sources.filter((s) => s.status === 'supported' && !s.featured);
const planned = sources.filter((s) => s.status === 'planned');
const link = (s) => `[${cell(s.name)}](${s.url})`;
const plannedTail = planned.length
  ? ` Planned: ${planned.map((s) => cell(s.name)).join(', ')}.`
  : '';
const alsoLine = otherSupported.length
  ? `_Also supported: ${otherSupported.map(link).join(', ')}.${plannedTail}_\n\n`
  : planned.length
    ? `_Planned: ${planned.map((s) => cell(s.name)).join(', ')}._\n\n`
    : '';
regenSection(
  'Supported-tools glimpse',
  '<!-- tools:start · generated from site/src/sources.json by `just gen-readme` — edit the JSON, not this table -->',
  '<!-- tools:end -->',
  [
    '| Tool | Runs on |',
    '|---|---|',
    ...featured.map((s) => `| ${link(s)} | ${cell(runsOn(s)) || '—'} |`),
    '',
    alsoLine + `**→ [Full tool × OS support matrix on the site](${SITE}/#tools)**`,
    ...(hasExperimental ? ['', '_\\* experimental — limited testing, unsigned binaries._'] : []),
  ].join('\n')
);

// --- Install block (GENERATED, like features/sources). The README shows only
// the `readme: true` methods (brew, npm); the rest (Cargo, GitHub Releases) live
// on the site's install tab. Single source: site/src/install.json — the same
// file Install.astro renders, so the two can't drift. ---
const installBody = install
  .filter((m) => m.readme)
  .map(
    (m) =>
      `**${cell(m.label)}**${m.blurb ? ` (${cell(m.blurb)})` : ''}:\n\n\`\`\`bash\n${m.cmds.join('\n')}\n\`\`\``
  )
  .join('\n\n');
regenSection(
  'Install block',
  '<!-- install:start · generated from site/src/install.json by `just gen-readme` — edit the JSON, not this block -->',
  '<!-- install:end -->',
  installBody
);

if (errors.length) {
  console.error(errors.map((e) => `✗ ${e}`).join('\n'));
  process.exit(1);
}
console.log(
  check
    ? 'README is in sync with features.json + sources.json + install.json ✓'
    : 'README regenerated from features.json + sources.json + install.json ✓'
);

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
