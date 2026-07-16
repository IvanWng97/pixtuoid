// The version the site DISPLAYS: the latest *released* tag, not main's
// in-progress Cargo.toml version. Between a mid-cycle bump (e.g. #550's
// 0.15.0, the ambient-audio 0.16.0) and its release tag, main's Cargo.toml
// runs AHEAD of what `cargo install`/brew actually serves — showing that
// pre-release number on pixtuoid.dev advertised a version nobody could
// install. Tag-first fixes the honesty gap; the Cargo.toml version stays
// the FALLBACK for builds without tag history.
//
// Pure kernel + impure shell split (the csp-hashes/gh-stars pattern) so the
// precedence and validation are unit-testable without a git repo.

import { execSync } from 'node:child_process';

/** `vX.Y.Z`-shaped tag (the repo's release-tag format), capture the bare version. */
const RELEASE_TAG = /^v(\d+\.\d+\.\d+)$/;

/**
 * Latest release tag reachable from HEAD, or null when git/tag history is
 * unavailable (shallow CI checkout without fetch-depth: 0, tarball builds).
 * Callers fall back to the Cargo.toml version — pages.yml/site.yml fetch
 * full history so the REAL deploys never take the fallback.
 */
export function latestReleaseTag() {
  try {
    return execSync('git describe --tags --abbrev=0', {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    return null;
  }
}

/**
 * Resolve the displayed version: a well-formed `vX.Y.Z` tag wins; anything
 * else (null, malformed, non-release tags) falls back to `cargoVersion`.
 * Returns `{ version, source }` so the build can log which path it took.
 */
export function resolveDisplayedVersion(tag, cargoVersion) {
  const m = typeof tag === 'string' ? tag.trim().match(RELEASE_TAG) : null;
  if (m) {
    return { version: m[1], source: 'tag' };
  }
  return { version: cargoVersion, source: 'cargo-toml' };
}
