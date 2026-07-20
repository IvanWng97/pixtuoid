#!/usr/bin/env node
// Keep the README in sync with the site's single-source data files:
//   • Features table          ← site/src/features.json  (GENERATED between markers)
//   • Supported-tools glimpse ← site/src/sources.json   (GENERATED between markers)
//   • Install block           ← site/src/install.json   (GENERATED — `readme:true` methods only)
// The site (Showcase.astro / SupportedTools.astro / Install.astro) reads the same
// JSON, so the README and the site can't drift. The supported-tools glimpse shows
// only the FEATURED tools + a link to the full tool × OS matrix on the site, so the
// README stays short as more agent CLIs are added. Run `just gen-readme` (or
// `node scripts/gen-readme.mjs`) after editing any JSON. `--check` writes
// nothing and exits non-zero on drift (used by CI's gen-check / `just gen-check`).
//
// NOTE: the manifest's *supported* set is pinned to the code's source registry
// (`registered_source_names()`)
// by a Rust test (crates/pixtuoid-core/tests/supported_sources_manifest.rs) that
// runs in the main CI — so the marketing list can never claim "supported" for a
// source that isn't actually wired (and a newly-wired source forces a manifest
// update). This script only owns rendering + README/site parity.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import process from 'node:process';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const readmePath = join(root, 'README.md');
const features = JSON.parse(readFileSync(join(root, 'site', 'src', 'features.json'), 'utf8'));
const sources = JSON.parse(readFileSync(join(root, 'site', 'src', 'sources.json'), 'utf8'));
const install = JSON.parse(readFileSync(join(root, 'site', 'src', 'install.json'), 'utf8'));

// MUST match `site` in site/astro.config.mjs. A repo-root Node script can't
// cheaply import the astro config (it pulls @astrojs/*), so this is a
// boundary-separated copy — gen-readme-check catches README drift, not a
// mismatch against the config, so keep the two in lockstep by hand.
const SITE = 'https://pixtuoid.dev';
const check = process.argv.includes('--check');
let readme = readFileSync(readmePath, 'utf8');
const errors = [];

// Every feature `pix` must resolve to a committed pixel-icon PNG. The site's
// PixIcon.astro throws at build ONLY for icons rendered through the roster
// (no-channel rows); a channel-bearing feature reaches this README `<img>` but
// never PixIcon, so a typo'd / ungenerated `pix` would ship a 404 image past
// the site build, gen-pix-icons --check (ICONS-only), AND gen-readme-check
// (README-vs-JSON, not img existence). One existsSync loop closes it for every
// row regardless of channel routing.
for (const f of features) {
  if (f.pix && !existsSync(join(root, 'docs', 'images', 'pix-icons', `${f.pix}.png`))) {
    errors.push(
      `feature "${f.name}" declares pix "${f.pix}" but docs/images/pix-icons/${f.pix}.png is missing — ` +
        `add "${f.pix}" to gen-pix-icons.py's ICONS and run \`just gen-icons\`.`
    );
  }
}

// Neutralize only what breaks a GFM table row: `|` splits columns (use the
// HTML entity — backslash-escaping would itself need backslash escaping first,
// CodeQL js/incomplete-sanitization) and newlines split rows. Cell text is
// intentionally markdown-bearing (backticks, `A\*`), so nothing else is touched.
const cell = (s) => String(s).replace(/\|/g, '&#124;').replace(/\r?\n/g, ' ');

// Regenerate the block between `start`/`end` markers from `body`. () => block:
// a replacer FUNCTION inserts the value literally — a plain string would expand
// `$`-patterns ($$, $&, $') lurking in the text and silently corrupt the README
// in a way --check can't see (both sides of its comparison would go through
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
// The README lists the HEADLINE features only; the site's Features grid shows the
// full set. A feature is README-featured by DEFAULT — opt a secondary one OUT with
// `"featured": false` (the inverse of install.json's opt-IN `readme:true`, because
// most features are headline and only a few are flavor). Edit the flag in
// features.json, never this table.
// Icon column: the office's own pixel PNGs (docs/images/pix-icons/). GitHub gives
// this empty-header column almost no width and forces `max-width:100%` on the
// <img>, so WITHOUT explicit dimensions the icon collapses to an illegible blob
// when the table is width-starved (this is why they looked tiny). Pin width/height
// from the PNG's own IHDR — GitHub keeps those attrs (it does on the 500px banner)
// — so the column reserves real space and the art renders 1:1: crisp, undistorted,
// sized by README_SCALE in gen-pix-icons.py (bump that const to resize).
// [w, h] from the PNG's IHDR, or null if missing (a missing PNG is already
// recorded by the existsSync guard above, which exits with a clean, actionable
// message — don't pre-empt it with a raw ENOENT here).
const pngWH = (pix) => {
  const p = join(root, 'docs', 'images', 'pix-icons', `${pix}.png`);
  if (!existsSync(p)) return null;
  const b = readFileSync(p);
  return [b.readUInt32BE(16), b.readUInt32BE(20)];
};
const pixDims = (pix) => {
  const wh = pngWH(pix);
  return wh ? ` width="${wh[0]}" height="${wh[1]}"` : '';
};
const iconCell = (f) =>
  f.pix ? `<img src="docs/images/pix-icons/${cell(f.pix)}.png" alt=""${pixDims(f.pix)}>` : cell(f.icon);
const featuredFeatures = features.filter((f) => f.featured !== false);
const featureRows = featuredFeatures.map(
  (f) => `| ${iconCell(f)} | **${cell(f.name)}** | ${cell(f.desc)} |`
);
// GitHub ignores an <img>'s width/height when its table cell is "shorter" than
// the image and collapses the column: Safari does this hard (the GitHub-injected
// `max-width:100%` makes the img's min-content 0), so the icons rendered ~9px in
// Safari while Chrome kept full size (verified in Playwright WebKit). The
// documented GFM fix is non-breaking-space "glue" — real, text-measured cell
// content the collapse can't undo. Pad the otherwise-empty icon HEADER (one cell,
// so it doesn't inflate each row's max-content the way padding beside the img
// would) to just clear the WIDEST icon, derived so it tracks README_SCALE.
const NBSP_PX = 4; // a README-font &nbsp; ≈ 4px (empirically 20 cleared the 70px lobster in WebKit)
const maxIconW = Math.max(...featuredFeatures.map((f) => pngWH(f.pix)?.[0] ?? 0));
const iconHeader = '&nbsp;'.repeat(Math.ceil(maxIconW / NBSP_PX) + 2);
regenSection(
  'Features table',
  '<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->',
  '<!-- features:end -->',
  [`| ${iconHeader} | Feature | Description |`, '|---|---|---|', ...featureRows].join('\n')
);

// --- Supported-tools glimpse (FEATURED only + a link to the full site matrix) ---
const OS_LABELS = { macos: 'macOS', linux: 'Linux', windows: 'Windows' };
const OS_ORDER = ['macos', 'linux', 'windows'];
const runsOn = (s) =>
  OS_ORDER.filter((os) => s.platforms?.[os] === 'yes' || s.platforms?.[os] === 'experimental')
    .map((os) => (s.platforms[os] === 'experimental' ? `${OS_LABELS[os]}\\*` : OS_LABELS[os]))
    .join(' · ');
const featured = sources.filter((s) => s.status === 'supported' && s.featured);
// Compute over the population that actually RENDERS the `\*` marker (the
// featured table, via runsOn), NOT all supported sources — else the footnote
// could appear with no `\*` referent (a non-featured experimental source would
// set the flag while the featured table shows no marker).
const hasExperimental = featured.some((s) =>
  Object.values(s.platforms || {}).includes('experimental')
);
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
