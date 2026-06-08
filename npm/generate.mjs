#!/usr/bin/env node
// Generate the per-platform `@pixtuoid/cli-*` npm packages + stamp the launcher.
//
// Mirrors Biome's generate-packages.mjs, adapted for pixtuoid's TWO binaries
// (pixtuoid + pixtuoid-hook) and ASYMMETRIC 6-target matrix. Reads the prebuilt
// binaries from --artifacts (layout: <dir>/<rust-target>/{pixtuoid,pixtuoid-hook}
// [.exe]) — the exact binaries release.yml already ships in its tarballs — and
// writes, under npm/:
//   • @pixtuoid/cli-<platform>-<arch>/  (per target: package.json + both binaries)
//   • re-stamps the launcher (npm/pixtuoid) version + its 6 optionalDependencies
//     pins to <version>.
//
// Usage: node npm/generate.mjs --version X.Y.Z --artifacts <dir> [--npm-dir <dir>]
// Writes into the npm/ tree in place (default: this script's own dir) — version
// + dep pins are stamped here; the per-platform @pixtuoid/cli-* dirs are
// gitignored. --npm-dir overrides the target tree (used by the test to generate
// into a temp copy instead of mutating the tracked launcher).
//
// Nothing per-platform is committed (git-cliff style) — the packages are
// generated here at publish time and gitignored.

import {
  existsSync,
  mkdirSync,
  copyFileSync,
  chmodSync,
  readFileSync,
  writeFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCOPE = "@pixtuoid";
const BINS = ["pixtuoid", "pixtuoid-hook"];

// The asymmetric 6-row target table — mirrors release.yml's build matrix.
//   linux-x64   → static musl build (portable to glibc too) → NO `libc` gate,
//                 so npm installs it on every linux-x64 host.
//   linux-arm64 → glibc build → `libc:["glibc"]` so npm SKIPS it on musl /
//                 Alpine-arm64 (where it would crash) rather than mis-installing.
const TARGETS = [
  { rust: "aarch64-apple-darwin", pkg: "darwin-arm64", os: "darwin", cpu: "arm64" },
  { rust: "x86_64-apple-darwin", pkg: "darwin-x64", os: "darwin", cpu: "x64" },
  { rust: "x86_64-unknown-linux-musl", pkg: "linux-x64", os: "linux", cpu: "x64" },
  { rust: "aarch64-unknown-linux-gnu", pkg: "linux-arm64", os: "linux", cpu: "arm64", libc: ["glibc"] },
  { rust: "x86_64-pc-windows-msvc", pkg: "win32-x64", os: "win32", cpu: "x64" },
  { rust: "aarch64-pc-windows-msvc", pkg: "win32-arm64", os: "win32", cpu: "arm64" },
];

function arg(name, def) {
  const i = process.argv.indexOf("--" + name);
  return i >= 0 && i + 1 < process.argv.length ? process.argv[i + 1] : def;
}

const version = arg("version");
const artifacts = arg("artifacts");
const NPM_DIR = arg("npm-dir", dirname(fileURLToPath(import.meta.url)));
if (!version || !artifacts) {
  console.error("usage: node npm/generate.mjs --version X.Y.Z --artifacts <dir>");
  process.exit(1);
}
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`refusing to stamp a non-semver version: ${version}`);
  process.exit(1);
}

const launcherPath = join(NPM_DIR, "pixtuoid", "package.json");
const launcher = JSON.parse(readFileSync(launcherPath, "utf8"));

for (const t of TARGETS) {
  const isWin = t.os === "win32";
  const pkgName = `${SCOPE}/cli-${t.pkg}`;
  const pkgDir = join(NPM_DIR, SCOPE, `cli-${t.pkg}`);
  rmSync(pkgDir, { recursive: true, force: true });
  mkdirSync(pkgDir, { recursive: true });

  const files = [];
  for (const bin of BINS) {
    const exe = bin + (isWin ? ".exe" : "");
    const src = join(artifacts, t.rust, exe);
    if (!existsSync(src) || !statSync(src).isFile()) {
      throw new Error(`missing prebuilt binary for ${t.rust}: ${src}`);
    }
    const dst = join(pkgDir, exe);
    copyFileSync(src, dst);
    // the upload/download-artifact round-trip strips the exec bit — restore it.
    if (!isWin) chmodSync(dst, 0o755);
    files.push(exe);
  }

  const pkg = {
    name: pkgName,
    version,
    description: `pixtuoid prebuilt binaries for ${t.pkg}`,
    license: launcher.license,
    repository: launcher.repository,
    homepage: launcher.homepage,
    os: [t.os],
    cpu: [t.cpu],
    ...(t.libc ? { libc: t.libc } : {}),
    files,
  };
  writeFileSync(join(pkgDir, "package.json"), JSON.stringify(pkg, null, 2) + "\n");
  launcher.optionalDependencies[pkgName] = version; // re-pin to this version
  console.log(`generated ${pkgName}@${version} (${files.join(", ")})`);
}

launcher.version = version;
writeFileSync(launcherPath, JSON.stringify(launcher, null, 2) + "\n");
console.log(`stamped launcher pixtuoid@${version} (${TARGETS.length} optionalDependencies)`);
