//! What a new version means at boot: whether its popup shows, and the notices
//! due to a user who last ran an older one.

use std::collections::HashSet;

use semver::Version;

use crate::install::target::{self, Target};

fn is_newer_version(current: &str, last_seen: &str) -> bool {
    matches!(
        (Version::parse(current), Version::parse(last_seen)),
        (Ok(c), Ok(l)) if c > l
    )
}

fn is_valid_version(s: &str) -> bool {
    Version::parse(s).is_ok()
}

pub(crate) struct BootDecision {
    pub(crate) should_show_popup: bool,
    pub(crate) should_persist: bool,
    /// `None` on a first install or an unparseable record, which name no
    /// version to be newer than.
    pub(crate) versions: Option<Versions>,
}

/// The version a config recorded before this boot, and this build's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Versions {
    pub(crate) previous: Version,
    pub(crate) current: Version,
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
        versions: last_seen
            .and_then(|v| Version::parse(v).ok())
            .zip(Version::parse(current_ver).ok())
            .map(|(previous, current)| Versions { previous, current }),
    }
}

/// Something a user who last ran an older version must know.
struct Notice {
    /// The last release without the change it announces.
    last_without: Version,
    /// What it says, while the state that needs it holds.
    says: fn(&Facts<'_>) -> Option<String>,
}

/// Every notice, in the order the popup lists them: an action before a tip.
const NOTICES: [Notice; 2] = [
    Notice {
        last_without: Version::new(0, 19, 0),
        says: reconnect,
    },
    Notice {
        last_without: Version::new(0, 19, 0),
        says: cutaway_tip,
    },
];

/// The hosts #1586 moved to a plugin.
const REPLUGGED: [&Target; 3] = [&target::CLAUDE, &target::CODEX, &target::REASONIX];

/// The run that turns the cutaway on, and the config line that keeps it on.
const CUTAWAY_RUN: [&str; 4] = ["pixtuoid", "run", "--graphics", "auto"];
const CUTAWAY_CONFIG: &str = "graphics = \"auto\"";

/// The hooks an older pixtuoid merged no longer count as a connection, so each
/// installed host that reads disconnected is named.
fn reconnect(facts: &Facts<'_>) -> Option<String> {
    let names: Vec<&str> = REPLUGGED
        .iter()
        .filter(|t| {
            facts.installed.contains(&t.core_source) && !facts.connected.contains(t.core_source)
        })
        .map(|t| t.display_name)
        .collect();
    let key = crate::panels::SOURCES_KEY;
    let ask = match names.as_slice() {
        [] => return None,
        [_] => format!("reconnect it in Sources ({key}) if you use it"),
        _ => format!("reconnect the ones you use in Sources ({key})"),
    };
    Some(format!(
        "pixtuoid now connects to {} as a plugin: {ask}.",
        english_list(&names)
    ))
}

/// The cutaway office (#1189) ships off, so a terminal that draws it is told.
fn cutaway_tip(facts: &Facts<'_>) -> Option<String> {
    (facts.cutaway.off && facts.cutaway.drawable).then(|| {
        format!(
            "This terminal can draw the cutaway office: try `{}`, or set {CUTAWAY_CONFIG} in the config.",
            CUTAWAY_RUN.join(" ")
        )
    })
}

/// `a`, `a and b`, `a, b and c`.
fn english_list(names: &[&str]) -> String {
    match names {
        [init @ .., last] if !init.is_empty() => format!("{} and {last}", init.join(", ")),
        _ => names.concat(),
    }
}

/// What the notices read when the version popup opens.
pub(crate) struct Facts<'a> {
    /// The agent CLIs on this machine, as [`crate::sources::detect`] finds them.
    pub(crate) installed: &'a [&'static str],
    pub(crate) connected: &'a HashSet<String>,
    pub(crate) cutaway: CutawayTip,
}

/// What a painter's look says for the cutaway tip; the default where it draws
/// no cutaway.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CutawayTip {
    /// The run paints classic because the user left graphics off.
    pub(crate) off: bool,
    /// The terminal's name alone says it draws the cutaway.
    pub(crate) drawable: bool,
}

/// Each notice due to a user who last ran `versions.previous`, now on
/// `versions.current`: one whose change the current carries and the previous
/// lacked, while its state holds. Without [`BootDecision::versions`] nothing is
/// new.
pub(crate) fn due(versions: Option<&Versions>, facts: &Facts<'_>) -> Vec<String> {
    let Some(Versions { previous, current }) = versions else {
        return Vec::new();
    };
    NOTICES
        .iter()
        .filter(|n| *previous <= n.last_without && n.last_without < *current)
        .filter_map(|n| (n.says)(facts))
        .collect()
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

    #[test]
    fn a_later_release_candidate_is_newer() {
        assert!(is_newer_version("0.5.0-rc.10", "0.5.0-rc.2"));
        assert!(!is_newer_version("0.5.0-rc.2", "0.5.0-rc.10"));
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
        assert!(!is_valid_version("0.5"), "semver needs the patch");
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

    fn v(s: &str) -> Version {
        Version::parse(s).expect("a test version")
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    const EVERY_HOST: [&str; 3] = ["claude-code", "codex", "reasonix"];

    const CUTAWAY_IDLE: CutawayTip = CutawayTip {
        off: true,
        drawable: true,
    };

    fn versions(previous: &str, current: &str) -> Versions {
        Versions {
            previous: v(previous),
            current: v(current),
        }
    }

    /// Every notice's state holds: each replugged host installed and
    /// disconnected, the cutaway idle in a terminal that draws it.
    fn all_hold() -> Facts<'static> {
        static NONE: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        Facts {
            installed: &EVERY_HOST,
            connected: &NONE,
            cutaway: CUTAWAY_IDLE,
        }
    }

    /// A first install and a hand-corrupted record name no previous version,
    /// so nothing is new to them.
    #[test]
    fn nothing_is_due_without_a_previous_version() {
        for last_seen in [None, Some("v0.4.0")] {
            let versions = boot_decision("0.20.0", last_seen).versions;
            assert_eq!(versions, None, "{last_seen:?}");
            assert!(due(versions.as_ref(), &all_hold()).is_empty());
        }
        assert_eq!(
            boot_decision("0.20.0", Some("0.19.0")).versions,
            Some(versions("0.19.0", "0.20.0"))
        );
    }

    /// Due across the change only: from a version without it to one with it,
    /// however many releases the upgrade skipped. A build of the very version
    /// that lacks it (a dev build before the release) is not across it.
    #[test]
    fn a_notice_is_due_only_across_its_change() {
        let due_from = |previous: &str, current: &str| {
            due(Some(&versions(previous, current)), &all_hold()).len()
        };
        assert_eq!(due_from("0.19.0", "0.20.0"), NOTICES.len());
        assert_eq!(
            due_from("0.17.2", "0.22.0"),
            NOTICES.len(),
            "skipped releases"
        );
        assert_eq!(
            due_from("0.19.0", "0.19.1"),
            NOTICES.len(),
            "a patch carries it too"
        );
        assert_eq!(due_from("0.18.0", "0.19.0"), 0, "the build that lacks it");
        assert_eq!(due_from("0.20.0", "0.21.0"), 0, "already past it");
    }

    /// `last_without` names a release, so a value guessed ahead of the build
    /// (the next release's number) fails here before any user misses it.
    #[test]
    fn every_notice_announces_a_change_after_a_release() {
        let current = v(env!("CARGO_PKG_VERSION"));
        for n in &NOTICES {
            assert!(n.last_without <= current, "{}", n.last_without);
        }
    }

    #[test]
    fn an_action_lists_before_a_tip() {
        assert_eq!(
            due(Some(&versions("0.19.0", "0.20.0")), &all_hold()),
            [
                reconnect(&all_hold()).expect("holds"),
                cutaway_tip(&all_hold()).expect("holds"),
            ]
        );
    }

    /// Only a replugged host that is installed and reads disconnected is named,
    /// by its registry name; with none, the notice is resolved.
    #[test]
    fn reconnect_names_each_installed_host_that_reads_disconnected() {
        let says = |installed: &[&'static str], connected: &[&str]| {
            reconnect(&Facts {
                installed,
                connected: &set(connected),
                ..all_hold()
            })
        };
        assert_eq!(
            says(&EVERY_HOST, &[]).as_deref(),
            Some(
                "pixtuoid now connects to Claude Code, Codex and Reasonix as a plugin: \
                 reconnect the ones you use in Sources (s)."
            )
        );
        assert_eq!(
            says(&["codex", "opencode"], &["opencode"]).as_deref(),
            Some(
                "pixtuoid now connects to Codex as a plugin: reconnect it in Sources (s) if you use it."
            )
        );
        assert_eq!(says(&EVERY_HOST, &EVERY_HOST), None, "all reconnected");
        assert_eq!(
            says(&["opencode"], &[]),
            None,
            "no replugged host installed"
        );
        assert_eq!(
            says(&EVERY_HOST, &["claude-code", "reasonix"]).as_deref(),
            Some(
                "pixtuoid now connects to Codex as a plugin: reconnect it in Sources (s) if you use it."
            )
        );
    }

    #[test]
    fn the_cutaway_tip_needs_graphics_off_in_a_terminal_that_draws_it() {
        for (off, drawable) in [(true, true), (true, false), (false, true), (false, false)] {
            let said = cutaway_tip(&Facts {
                cutaway: CutawayTip { off, drawable },
                ..all_hold()
            });
            assert_eq!(
                said.is_some(),
                off && drawable,
                "off={off} drawable={drawable}"
            );
        }
        assert_eq!(
            cutaway_tip(&all_hold()).as_deref(),
            Some(
                "This terminal can draw the cutaway office: try \
                 `pixtuoid run --graphics auto`, or set graphics = \"auto\" in the config."
            )
        );
    }

    #[test]
    fn english_list_joins_the_last_with_and() {
        assert_eq!(english_list(&["a"]), "a");
        assert_eq!(english_list(&["a", "b"]), "a and b");
        assert_eq!(english_list(&["a", "b", "c"]), "a, b and c");
    }

    /// The tip's command must run: a renamed flag or subcommand fails here, not
    /// in a user's shell.
    #[test]
    fn the_cutaway_run_turns_graphics_on() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(CUTAWAY_RUN).expect("parses");
        assert!(matches!(
            cli.cmd,
            Some(crate::cli::Cmd::Run {
                graphics: Some(crate::GraphicsMode::Auto),
                ..
            })
        ));
    }

    #[test]
    fn the_cutaway_config_line_turns_graphics_on() {
        let cfg: crate::config::AppConfig = toml::from_str(CUTAWAY_CONFIG).expect("parses");
        assert_eq!(
            crate::config::resolve_graphics(&cfg, None, &mut Vec::new()),
            crate::GraphicsMode::Auto
        );
    }
}
