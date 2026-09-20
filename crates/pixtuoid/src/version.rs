fn is_newer_version(current: &str, last_seen: &str) -> bool {
    parse_semver(current)
        .zip(parse_semver(last_seen))
        .is_some_and(|(c, l)| c > l)
}

fn is_valid_version(s: &str) -> bool {
    parse_semver(s).is_some()
}

pub(crate) struct BootDecision {
    pub(crate) should_show_popup: bool,
    pub(crate) should_persist: bool,
}

/// Decide whether the version popup should fire on boot and whether to persist
/// `last_seen_version`. Persist also happens on a first-time install and on an
/// UNPARSEABLE recorded version — overwrite to recover, else a corrupted or
/// hand-edited value silently disables the popup forever.
pub(crate) fn boot_decision(current_ver: &str, last_seen: Option<&str>) -> BootDecision {
    let last_seen_parseable = last_seen.is_some_and(is_valid_version);
    let should_show_popup = match last_seen {
        Some(last) if last_seen_parseable => is_newer_version(current_ver, last),
        _ => false,
    };
    let should_persist = should_show_popup || last_seen.is_none() || !last_seen_parseable;
    BootDecision {
        should_show_popup,
        should_persist,
    }
}

/// Parses `major.minor.patch[-prerelease]` into a tuple whose 4th component is `0`
/// for a prerelease and `1` for a release, so `0.5.0-rc1 < 0.5.0` per semver.
fn parse_semver(v: &str) -> Option<(u64, u64, u64, u8)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_str = parts.next().unwrap_or("0");
    let (patch_num, is_release) = match patch_str.split_once('-') {
        Some((num, _prerelease)) => (num.parse().ok()?, 0u8),
        None => (patch_str.parse().ok()?, 1u8),
    };
    Some((major, minor, patch_num, is_release))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_detected() {
        assert!(is_newer_version("0.2.0", "0.1.0"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!is_newer_version("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_not_newer() {
        assert!(!is_newer_version("0.1.0", "0.2.0"));
    }

    #[test]
    fn major_bump_detected() {
        assert!(is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn minor_bump_detected() {
        assert!(is_newer_version("0.5.0", "0.4.0"));
    }

    #[test]
    fn patch_bump_detected() {
        assert!(is_newer_version("0.4.1", "0.4.0"));
    }

    #[test]
    fn bad_input_safe() {
        assert!(!is_newer_version("not-semver", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "garbage"));
        assert!(!is_newer_version("", ""));
    }

    #[test]
    fn prerelease_newer_than_older_release() {
        assert!(is_newer_version("0.5.0-alpha", "0.4.0"));
    }

    #[test]
    fn release_newer_than_prerelease_of_same_version() {
        assert!(is_newer_version("0.5.0", "0.5.0-rc1"));
        assert!(!is_newer_version("0.5.0-rc1", "0.5.0"));
    }

    /// Guard for #110: every hardcoded intra-workspace path-dep `version` (NOT
    /// workspace-inherited) must track the crate version, or a bump that misses one
    /// breaks `cargo publish`. Checks EVERY `path =` + `version = "` line rather
    /// than a named subset, so a future workspace path-dep is covered the moment
    /// it's added.
    #[test]
    fn path_dep_version_tracks_crate_version() {
        let assert_tracks = |manifest: &str, who: &str| {
            let mut checked = 0;
            for line in manifest.lines() {
                let l = line.trim_start();
                // A version-inherited path-dep (no `version = "`) is not a hazard.
                if !(l.contains("path =") && l.contains("version = \"")) {
                    continue;
                }
                let dep_version = l
                    .split_once("version = \"")
                    .and_then(|(_, rest)| rest.split('"').next())
                    .unwrap_or_else(|| panic!("a version requirement on a path-dep in {who}"));
                assert_eq!(
                    dep_version,
                    env!("CARGO_PKG_VERSION"),
                    "{who}: path-dep version ({dep_version}) != crate version ({}) — release-plz rewrites every path-dep requirement in the release PR (see #110)",
                    env!("CARGO_PKG_VERSION")
                );
                checked += 1;
            }
            assert!(
                checked > 0,
                "{who}: expected at least one hardcoded path-dep version to guard (see #110)"
            );
        };
        assert_tracks(include_str!("../Cargo.toml"), "crates/pixtuoid/Cargo.toml");
        assert_tracks(
            include_str!("../../pixtuoid-scene/Cargo.toml"),
            "crates/pixtuoid-scene/Cargo.toml",
        );
    }

    #[test]
    fn is_valid_version_accepts_well_formed() {
        assert!(is_valid_version("0.4.0"));
        assert!(is_valid_version("1.2.3"));
        assert!(is_valid_version("0.5.0-rc1"));
    }

    #[test]
    fn is_valid_version_rejects_corrupted() {
        assert!(!is_valid_version("v0.4.0"), "leading v is not semver");
        assert!(!is_valid_version("garbage"));
        assert!(!is_valid_version(""));
    }

    // `v0.4.0` is the git-tag spelling — the corruption a hand-edit actually hits.
    #[test]
    fn boot_decision_overwrites_corrupted_last_seen() {
        let d = boot_decision("0.4.1", Some("v0.4.0"));
        assert!(
            !d.should_show_popup,
            "can't show popup when comparison fails"
        );
        assert!(
            d.should_persist,
            "corrupted last_seen must be overwritten to recover"
        );
    }

    #[test]
    fn boot_decision_first_run_persists_silently() {
        let d = boot_decision("0.4.1", None);
        assert!(!d.should_show_popup);
        assert!(d.should_persist);
    }

    #[test]
    fn boot_decision_upgrade_shows_popup_and_persists() {
        let d = boot_decision("0.4.0", Some("0.3.0"));
        assert!(d.should_show_popup);
        assert!(d.should_persist);
    }

    #[test]
    fn boot_decision_same_version_no_action() {
        let d = boot_decision("0.4.0", Some("0.4.0"));
        assert!(!d.should_show_popup);
        assert!(!d.should_persist);
    }

    #[test]
    fn boot_decision_downgrade_no_action() {
        let d = boot_decision("0.3.0", Some("0.4.0"));
        assert!(!d.should_show_popup);
        assert!(!d.should_persist);
    }
}
