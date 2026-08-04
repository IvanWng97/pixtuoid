// The version the site DISPLAYS: the latest *released* tag, not main's
// in-progress Cargo.toml version. Between a mid-cycle bump and its release
// tag, main's Cargo.toml runs AHEAD of what `cargo install`/brew serves, so
// showing it would advertise a version nobody can install.

import { execSync } from 'node:child_process';

const RELEASE_TAG = /^v(\d+\.\d+\.\d+)$/;

/**
 * Latest release tag reachable from HEAD, or null when git/tag history is
 * unavailable (shallow CI checkout without fetch-depth: 0, tarball builds).
 * pages.yml/site.yml fetch full history so the REAL deploys never fall back.
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

export function resolveDisplayedVersion(tag, cargoVersion) {
  const m = typeof tag === 'string' ? tag.trim().match(RELEASE_TAG) : null;
  if (m) {
    return { version: m[1], source: 'tag' };
  }
  return { version: cargoVersion, source: 'cargo-toml' };
}
