pub fn is_newer_version(current: &str, last_seen: &str) -> bool {
    parse_semver(current)
        .zip(parse_semver(last_seen))
        .is_some_and(|(c, l)| c > l)
}

pub fn is_valid_version(s: &str) -> bool {
    parse_semver(s).is_some()
}

pub struct BootDecision {
    pub should_show_popup: bool,
    pub should_persist: bool,
}

/// Decide whether the version popup should fire on boot and whether to
/// persist `last_seen_version`. Pure function so the boot logic is testable
/// without spinning up a terminal.
///
/// Persist happens in three cases:
/// - The popup is firing (record current so it only fires once).
/// - First-time install (no recorded version yet).
/// - The recorded version is unparseable — overwrite to recover, otherwise a
///   corrupted/hand-edited value silently disables the popup forever.
pub fn boot_decision(current_ver: &str, last_seen: Option<&str>) -> BootDecision {
    let last_seen_parseable = last_seen.is_some_and(is_valid_version);
    let should_show_popup = match last_seen {
        Some(last) if last_seen_parseable => {
            is_newer_version(current_ver, last) && release_notes(current_ver).is_some()
        }
        _ => false,
    };
    let should_persist = should_show_popup || last_seen.is_none() || !last_seen_parseable;
    BootDecision {
        should_show_popup,
        should_persist,
    }
}

/// Parses `major.minor.patch[-prerelease]` into a tuple where the 4th
/// component is `0` for a prerelease and `1` for a release, so that
/// `0.5.0-rc1 < 0.5.0` per semver precedence rules.
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

pub fn release_notes(version: &str) -> Option<&'static [&'static str]> {
    match version {
        // `just bump` injects the new version's arm right after the marker below;
        // anchoring on a marker is whitespace-independent — matching the `match`
        // brace would silently break if the indentation ever shifted.
        // [bump-inject-here]
        "0.11.0" => Some(&[
            "Windows: a `~`-prefixed `--pack-dir` / `pack-dir` now expands to your home directory (no more literal `~` in the path)",
            "Internals tidied — a large code-quality pass (deduplication and a unified pixel-buffer type); the office renders identically, just leaner under the hood",
        ]),
        "0.10.0" => Some(&[
            "`pixtuoid doctor` now flags an OpenClaw plugin whose files went missing — a source that would silently never load is reported broken instead of healthy",
            "Smoother Sources panel — connecting or disconnecting a CLI no longer hitches the office while it writes hook config",
            "Windows: agent hooks now install to the exact path each CLI reads — CodeWhale, OpenClaw, Reasonix and Cursor sessions show up reliably (no more installed-but-invisible)",
        ]),
        "0.9.0" => Some(&[
            "OpenClaw gateway visualized as a presence-gated wandering lobster mascot whose motion tracks the daemon (idle ambles, busy shuttles, down walks out); connect it in the Sources panel (press s)",
            "Cursor CLI sessions now visualized (cu·) — connect it in the Sources panel",
            "GitHub Copilot CLI sessions now visualized (cp·), permission prompts and sub-agents included — connect it in the Sources panel",
            "New `pixtuoid doctor` — a read-only self-diagnosis: which sources are connected, whether their hooks are installed and sound, and a live footer nudge when an upstream wire-format change starts dropping events",
        ]),
        "0.8.0" => Some(&[
            "New in-TUI Sources panel (press s) — connect or disconnect any agent CLI live; its characters walk in when you connect and out when you disconnect, no restart",
            "Manage every CLI from that panel: the `install-hooks` / `uninstall-hooks` commands are gone — connecting a CLI is now the panel's job (press s)",
            "CodeWhale sessions now visualized (cw·), subagents included — connect it in the Sources panel",
            "opencode sessions now visualized (oc·), subagents included — connect it in the Sources panel",
            "Every popup (Connection, agent dashboard, themes, help) is now borderless for a cleaner look",
        ]),
        "0.7.0" => Some(&[
            "Agents leave the moment they finish — instant exit detection (process watch + SubagentStop hooks); connect Claude Code in the Sources panel (press s) to enable",
            "Workflow fleets render right: instant seats on arrival, desks free in seconds, parent links survive worktree splits and revivals",
            "Attach mid-session and every live agent appears immediately — even idle or permission-parked ones",
            "Agent dashboard: Tab opens a foldable agent tree with per-CLI badges; Enter jumps floors (now up to 10)",
            "Windows: Codex + Reasonix hooks and the Antigravity watcher work end-to-end",
            "Your config is never wiped: installs and saves preserve comments & permissions under one atomic locked round",
        ]),
        // 0.6.1 re-runs the 0.6.0 release with the npm-launcher publish fix
        // (#186) — 0.6.0 shipped to crates.io/homebrew but the `pixtuoid` npm
        // launcher failed, so 0.6.1 is the first fully-published version. Same
        // highlights, since most users first land here.
        "0.6.1" => Some(&[
            "Windows support — native hook transport, installer, and release builds",
            "Install via npm — `npm i -g pixtuoid` on macOS, Linux & Windows",
            "Reasonix sessions now visualized — connect it in the Sources panel (press s)",
            "Sharper agent activity — fewer ghost & duplicate sprites, and Codex stays active during web & tool search",
            "Diagnostics you can see — source-death footer warnings, config warnings on stderr, an always-on log file",
            "New project site — live demos, architecture & contributing docs, weather gallery",
        ]),
        // 0.6.0 shipped to crates.io/homebrew but its npm launcher failed, so
        // 0.6.1 (above) superseded it as the first fully-published release.
        "0.6.0" => Some(&[
            "Windows support — native hook transport, installer, and release builds",
            "Install via npm — `npm i -g pixtuoid` now works on macOS, Linux & Windows",
            "Reasonix sessions now visualized — connect it in the Sources panel (press s)",
            "Sharper agent activity — fewer ghost & duplicate sprites, and Codex stays active during web & tool search",
            "Diagnostics you can see — source-death footer warnings, config warnings on stderr, an always-on log file",
            "New project site — live demos, architecture & contributing docs, weather gallery",
        ]),
        "0.4.0" => Some(&[
            "Renamed from ascii-agents to pixtuoid",
            "Reconnect your CLI in the Sources panel (press s) to update hooks",
            "New env vars: PIXTUOID_SOCKET/HOOK/LOG",
            "Flaky startup test fixed + 250ms rescan",
        ]),
        "0.4.1" => Some(&[
            "Per-floor boot capacity fixes invisible-agent edge case",
            "Connecting a CLI now strips legacy `_ascii_agents` entries again",
            "Resize mid-slide lands on destination floor, not source",
            "Version popup URL no longer mis-clicks on narrow terminals",
            "Corrupted last_seen_version self-heals on next launch",
        ]),
        "0.5.0" => Some(&[
            "Now visualizes Codex sessions too — connect it in the Sources panel (press s)",
            "Office overhaul: unified furniture + smarter approach/seating pathfinding",
            "Glass meeting rooms, denser desk pods, day/night lighting",
            "Physics-grounded weather: storms, lightning, moonlight",
            "Real-physics walking, animated floor transitions, emergent meeting chitchat",
            "Custom pet names via `[[pets]]` config",
        ]),
        _ => None,
    }
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
    fn release_notes_known_version() {
        assert!(release_notes("0.4.0").is_some());
    }

    #[test]
    fn release_notes_unknown_version() {
        assert!(release_notes("9.9.9").is_none());
    }

    /// Guards against a silent regression: bumping `Cargo.toml` without
    /// adding a matching `release_notes` arm would make the popup
    /// permanently invisible for the new release. This test fails fast.
    #[test]
    fn current_version_has_release_notes() {
        let current = env!("CARGO_PKG_VERSION");
        assert!(
            release_notes(current).is_some(),
            "release_notes({current:?}) returned None — add an arm for the current version"
        );
    }

    /// Guard for #110: the `pixtuoid → pixtuoid-core` path-dep `version` is a
    /// hardcoded requirement (NOT workspace-inherited), so a bump that misses it
    /// breaks `cargo publish`. Assert it tracks the crate version. `just bump`
    /// (cargo set-version) keeps them synced; this fails fast — in `just test`,
    /// preflight, and the release `check` job — if they ever drift.
    #[test]
    fn path_dep_version_tracks_crate_version() {
        let manifest = include_str!("../Cargo.toml");
        let dep_line = manifest
            .lines()
            .find(|l| l.trim_start().starts_with("pixtuoid-core") && l.contains("path ="))
            .expect("a pixtuoid-core path-dependency line in crates/pixtuoid/Cargo.toml");
        let dep_version = dep_line
            .split_once("version = \"")
            .and_then(|(_, rest)| rest.split('"').next())
            .expect("a version requirement on the pixtuoid-core path-dep");
        assert_eq!(
            dep_version,
            env!("CARGO_PKG_VERSION"),
            "pixtuoid-core path-dep version ({dep_version}) != crate version ({}) — run `just bump` (see #110)",
            env!("CARGO_PKG_VERSION")
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

    // Regression for the silent-disable bug: a hand-edited or corrupted
    // last_seen_version (e.g. `v0.4.0` matching the git-tag spelling) must
    // be overwritten on boot, not left in place to suppress every future
    // popup.
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
        // Use 0.4.0 (which has release_notes) as the current version so this
        // test stays stable across version bumps.
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
