"use strict";

// Shared platform → native-binary resolution for the two launcher shims.
//
// Mirrors the esbuild / Biome / git-cliff pattern: npm installs exactly the ONE
// `@pixtuoid/cli-<platform>-<arch>` optionalDependency that matches the host
// (filtered by each platform package's `os`/`cpu`/`libc`), and we
// `require.resolve` the native binary out of it. No postinstall, no download.

const fs = require("fs");
const path = require("path");

// pixtuoid's matrix is ASYMMETRIC — exactly one package per (platform, arch),
// so there is no runtime gnu-vs-musl choice to make here:
//   • linux-x64   ships a STATIC musl build (portable to glibc too) — the
//     package declares no `libc`, so it installs on every linux-x64 host.
//   • linux-arm64 ships a glibc build — its package declares `libc:["glibc"]`,
//     so npm skips it on musl/Alpine-arm64 (where it would crash) rather than
//     installing a broken binary.
// The os/cpu/libc filtering happens at npm INSTALL time; here we just map.
const PACKAGE = {
  "darwin arm64": "@pixtuoid/cli-darwin-arm64",
  "darwin x64": "@pixtuoid/cli-darwin-x64",
  "linux x64": "@pixtuoid/cli-linux-x64",
  "linux arm64": "@pixtuoid/cli-linux-arm64",
  "win32 x64": "@pixtuoid/cli-win32-x64",
  "win32 arm64": "@pixtuoid/cli-win32-arm64",
};

function exeName(bin) {
  return bin + (process.platform === "win32" ? ".exe" : "");
}

// Resolve the absolute path to a named native binary ("pixtuoid" or
// "pixtuoid-hook"). Returns null if this platform isn't shipped — callers
// decide how to handle that (the TUI errors; the hook stays silent).
function binaryPath(bin) {
  // Dev / CI override: PIXTUOID_BINARY points at a directory holding the
  // binaries (or at the pixtuoid binary itself). Lets a local debug build be
  // driven through the npm shim without publishing; composes with the Rust-side
  // PIXTUOID_HOOK override used by install-hooks.
  const override = process.env.PIXTUOID_BINARY;
  if (override) {
    // One guarded statSync (not existsSync + statSync) — the two-syscall form
    // can throw ENOENT on a delete-between-calls race, and binaryPath() must
    // return null, never throw (the hook shim relies on it).
    let st = null;
    try {
      st = fs.statSync(override);
    } catch (_e) {
      st = null;
    }
    const dir = st && st.isDirectory() ? override : path.dirname(override);
    const p = path.join(dir, exeName(bin));
    if (fs.existsSync(p)) return p;
  }

  const pkg = PACKAGE[process.platform + " " + process.arch];
  if (!pkg) return null;
  try {
    return require.resolve(pkg + "/" + exeName(bin));
  } catch (_e) {
    return null;
  }
}

module.exports = { binaryPath, exeName };
