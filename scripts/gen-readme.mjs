#!/usr/bin/env node
// Regenerate the README's marker-delimited blocks from the site's single-source
// JSON (features.json, sources.json, install.json) — the same files
// Showcase/SupportedTools/Install read, so README and site can't drift.
// `--check` writes nothing and exits non-zero on drift.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import process from 'node:process';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const readmePath = join(root, 'README.md');
const features = JSON.parse(readFileSync(join(root, 'site', 'src', 'features.json'), 'utf8'));
const sources = JSON.parse(readFileSync(join(root, 'site', 'src', 'sources.json'), 'utf8'));
const install = JSON.parse(readFileSync(join(root, 'site', 'src', 'install.json'), 'utf8'));

// MUST match `site` in site/astro.config.mjs — a hand-kept copy (importing the
// astro config here would pull @astrojs/*), and nothing gates the two.
const SITE = 'https://pixtuoid.dev';
const check = process.argv.includes('--check');
let readme = readFileSync(readmePath, 'utf8');
const errors = [];

// Every feature `pix` must resolve to a committed pixel-icon PNG. PixIcon.astro
// throws at build ONLY for roster-rendered icons; a channel-bearing feature
// reaches this README `<img>` but never PixIcon, so a typo'd `pix` would ship a
// 404 image past every other check.
for (const f of features) {
  if (f.pix && !existsSync(join(root, 'docs', 'images', 'pix-icons', `${f.pix}.png`))) {
    errors.push(
      `feature "${f.name}" declares pix "${f.pix}" but docs/images/pix-icons/${f.pix}.png is missing — ` +
        `add "${f.pix}" to gen-pix-icons.py's ICONS and run \`just gen-icons\`.`
    );
  }
}

// Neutralize only what breaks a GFM table row. `|` uses the HTML entity —
// backslash-escaping would itself need backslash escaping first (CodeQL
// js/incomplete-sanitization). Cell text is intentionally markdown-bearing.
const cell = (s) => String(s).replace(/\|/g, '&#124;').replace(/\r?\n/g, ' ');

// The `() => block` replacer must stay a FUNCTION: it inserts the value
// literally, where a plain string would expand `$`-patterns ($$, $&, $') in the
// text and corrupt the README in a way --check can't see (both sides of its
// comparison would go through the same mangling).
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

// A feature is README-featured by DEFAULT — opt a secondary one OUT with
// `"featured": false` (the inverse of install.json's opt-IN `readme:true`).
// GitHub gives the empty-header icon column almost no width and forces
// `max-width:100%` on the <img>, so without explicit dimensions the icon
// collapses to an illegible blob; pin width/height from the PNG's own IHDR.
// Returns null if the PNG is missing — the existsSync guard above already
// reports that with an actionable message, so don't pre-empt it with an ENOENT.
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
// the image and collapses the column — hard in Safari, where the injected
// `max-width:100%` makes the img's min-content 0. The GFM fix is
// non-breaking-space "glue": real, text-measured content the collapse can't
// undo. Pad the icon HEADER only, so it doesn't inflate each row's max-content.
const NBSP_PX = 4; // a README-font &nbsp; ≈ 4px
const maxIconW = Math.max(...featuredFeatures.map((f) => pngWH(f.pix)?.[0] ?? 0));
const iconHeader = '&nbsp;'.repeat(Math.ceil(maxIconW / NBSP_PX) + 2);
regenSection(
  'Features table',
  '<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->',
  '<!-- features:end -->',
  [`| ${iconHeader} | Feature | Description |`, '|---|---|---|', ...featureRows].join('\n')
);

const OS_LABELS = { macos: 'macOS', linux: 'Linux', windows: 'Windows' };
const OS_ORDER = ['macos', 'linux', 'windows'];
const runsOn = (s) =>
  OS_ORDER.filter((os) => s.platforms?.[os] === 'yes' || s.platforms?.[os] === 'experimental')
    .map((os) => (s.platforms[os] === 'experimental' ? `${OS_LABELS[os]}\\*` : OS_LABELS[os]))
    .join(' · ');
const featured = sources.filter((s) => s.status === 'supported' && s.featured);
// Over the population that actually RENDERS the `\*` marker, NOT all supported
// sources — else the footnote could appear with no `\*` referent.
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
