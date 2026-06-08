#!/usr/bin/env node
// Tests for generate.mjs — the per-platform package generator. Runs via the
// zero-dep built-in runner: `node --test npm/generate.test.mjs` (also wired as a
// pre-step of release.yml's npm job, so a broken generator fails the release
// BEFORE anything is published).
//
// Strategy: spawn the real script against a throwaway --npm-dir + fake binaries,
// then assert on what it wrote. No mocking — we exercise the actual semver guard,
// the asymmetric libc gate, dep-pin stamping, and the missing-binary throw.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync,
  rmSync,
} from "node:fs";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "generate.mjs");

// The same 6-row matrix generate.mjs ships — kept here independently so the test
// fails loudly if the production table is edited without updating the contract.
const TARGETS = [
  { rust: "aarch64-apple-darwin", pkg: "darwin-arm64", os: "darwin", cpu: "arm64", win: false },
  { rust: "x86_64-apple-darwin", pkg: "darwin-x64", os: "darwin", cpu: "x64", win: false },
  { rust: "x86_64-unknown-linux-musl", pkg: "linux-x64", os: "linux", cpu: "x64", win: false },
  { rust: "aarch64-unknown-linux-gnu", pkg: "linux-arm64", os: "linux", cpu: "arm64", win: false, libc: ["glibc"] },
  { rust: "x86_64-pc-windows-msvc", pkg: "win32-x64", os: "win32", cpu: "x64", win: true },
  { rust: "aarch64-pc-windows-msvc", pkg: "win32-arm64", os: "win32", cpu: "arm64", win: true },
];

// Build a temp tree: a minimal launcher package.json + an artifacts dir holding
// fake pixtuoid / pixtuoid-hook binaries for every (optionally all-but-one)
// target. Returns { dir, npmDir, artifacts } and registers cleanup.
function scaffold(t, { skip } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "pixtuoid-gen-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  const npmDir = join(dir, "npm");
  mkdirSync(join(npmDir, "pixtuoid"), { recursive: true });
  writeFileSync(
    join(npmDir, "pixtuoid", "package.json"),
    JSON.stringify(
      {
        name: "pixtuoid",
        version: "0.0.0",
        license: "MIT",
        repository: { type: "git", url: "git+https://example/x.git" },
        homepage: "https://example/x",
        optionalDependencies: {},
      },
      null,
      2,
    ),
  );

  const artifacts = join(dir, "artifacts");
  for (const tg of TARGETS) {
    if (tg.rust === skip) continue;
    const tdir = join(artifacts, tg.rust);
    mkdirSync(tdir, { recursive: true });
    const ext = tg.win ? ".exe" : "";
    writeFileSync(join(tdir, "pixtuoid" + ext), "fake-tui");
    writeFileSync(join(tdir, "pixtuoid-hook" + ext), "fake-hook");
  }
  return { dir, npmDir, artifacts };
}

function run(npmDir, artifacts, version) {
  return spawnSync(
    process.execPath,
    [SCRIPT, "--version", version, "--artifacts", artifacts, "--npm-dir", npmDir],
    { encoding: "utf8" },
  );
}

test("stamps launcher + all 6 platform packages with the asymmetric libc gate", (t) => {
  const { npmDir, artifacts } = scaffold(t);
  const r = run(npmDir, artifacts, "1.2.3");
  assert.equal(r.status, 0, r.stderr);

  const launcher = JSON.parse(readFileSync(join(npmDir, "pixtuoid", "package.json"), "utf8"));
  assert.equal(launcher.version, "1.2.3", "launcher version stamped");

  for (const tg of TARGETS) {
    const name = `@pixtuoid/cli-${tg.pkg}`;
    assert.equal(launcher.optionalDependencies[name], "1.2.3", `${name} pinned in launcher`);

    const pkgPath = join(npmDir, "@pixtuoid", `cli-${tg.pkg}`, "package.json");
    assert.ok(existsSync(pkgPath), `${name} package.json written`);
    const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
    assert.equal(pkg.version, "1.2.3");
    assert.deepEqual(pkg.os, [tg.os]);
    assert.deepEqual(pkg.cpu, [tg.cpu]);

    // The libc gate is the load-bearing asymmetry: ONLY linux-arm64 (glibc
    // build) declares it; the static-musl linux-x64 and everything else omit it.
    if (tg.libc) assert.deepEqual(pkg.libc, tg.libc, `${name} declares libc`);
    else assert.equal("libc" in pkg, false, `${name} omits libc`);

    // Each package ships BOTH binaries, with the .exe suffix only on win32.
    const ext = tg.win ? ".exe" : "";
    assert.deepEqual(pkg.files.sort(), ["pixtuoid" + ext, "pixtuoid-hook" + ext].sort());
    for (const f of pkg.files) {
      assert.ok(existsSync(join(npmDir, "@pixtuoid", `cli-${tg.pkg}`, f)), `${f} copied`);
    }
  }
});

test("accepts a prerelease semver", (t) => {
  const { npmDir, artifacts } = scaffold(t);
  assert.equal(run(npmDir, artifacts, "0.6.0-rc.1").status, 0);
});

test("rejects a non-semver version without writing", (t) => {
  const { npmDir, artifacts } = scaffold(t);
  const r = run(npmDir, artifacts, "1.2"); // not X.Y.Z
  assert.equal(r.status, 1);
  assert.match(r.stderr, /non-semver/);
  // launcher must be untouched on a guard failure
  const launcher = JSON.parse(readFileSync(join(npmDir, "pixtuoid", "package.json"), "utf8"));
  assert.equal(launcher.version, "0.0.0");
});

test("throws when a target's prebuilt binary is missing", (t) => {
  const { npmDir, artifacts } = scaffold(t, { skip: "x86_64-unknown-linux-musl" });
  const r = run(npmDir, artifacts, "1.2.3");
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /missing prebuilt binary for x86_64-unknown-linux-musl/);
});
