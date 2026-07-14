fn is_newer_version(current: &str, last_seen: &str) -> bool {
    parse_semver(current)
        .zip(parse_semver(last_seen))
        .is_some_and(|(c, l)| c > l)
}

fn is_valid_version(s: &str) -> bool {
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
pub(crate) fn boot_decision(current_ver: &str, last_seen: Option<&str>) -> BootDecision {
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

pub(crate) fn release_notes(version: &str) -> Option<&'static [&'static str]> {
    match version {
        // `just bump` injects the new version's arm right after the marker below;
        // anchoring on a marker is whitespace-independent — matching the `match`
        // brace would silently break if the indentation ever shifted.
        // [bump-inject-here]
        "0.15.0" => Some(&[
            "A busier, better-looking office — denser desk pods that pack the grid at every size, agents walking behind their desks, and a full interior-decor pass: glass walls with door jambs and mullions, a lounge aquarium, real head-of-table meeting chairs, greenery (two ficus trees) and mats",
            "Pantry v2 — a kitchen island with bartender slots and a snack shelf join the pantry; per-desk trash bins retire (the pantry keeps the office's one bin)",
            "Appliances come alive — the printer ejects pages and the vending machine drops a can while an agent stands there; the water cooler glugs on its own",
            "The web hero zooms in — the landing-page office renders at a closer camera (and the app's own 1F layout), so sprites finally read at a glance",
            "Sturdier under the hood — furniture declares how each piece meets the floor in one place (a sprite resize can't drift a walkable edge), and a generative placement sweep guards every size the office lays out",
        ]),
        "0.14.0" => Some(&[
            "Click an agent, land in its terminal — clicking a sprite (or pressing f on the dashboard's selected agent) brings that session's terminal app to the foreground on macOS, Windows and Linux; it's located through the session's own process, so a stale or reused pid never yanks the wrong window forward",
            "Top models wear their tier — agents running a frontier model now sport ember hair and a flame crown, so the office's horsepower reads at a glance, and the flames light and snuff the instant a session switches models",
            "New agent supported — Oh My Pi (omp) sessions show up as om· pixel-art coworkers, wired in like every other CLI, with full model burn-tier parity",
            "Crisper text everywhere — the old 8×8 bitmap font is retired: every name badge, label, and the neon wall board now render anti-aliased across the terminal, the floating window, and the web hero",
            "Sturdier under the hood — a whole-codebase review pass (correctness, naming, wire-decoding), a burn-tier upstream-drift watch, and focus-jump pid recycle-guards; the office looks the same, just steadier",
        ]),
        "0.13.0" => Some(&[
            "New agent supported — Hermes Agent (Nous Research) sessions now show up as animated pixel-art coworkers in the office, wired in like every other CLI",
            "A living sky — a sun and moon now arc past the office windows over the city skyline as the day turns, and weather became the atmosphere between them and your desk: a clear noon blazes, a storm dusk goes gloomy, fog swallows the sun, and a crescent moon rises at night, all tinted per theme",
            "See the office at a glance — a new on-screen HUD ties together a status footer, a wall board, and hover tooltips, so who's working, on what, and how busy the floor is are all readable without opening a panel",
            "Safer on a shared machine — agents now reach pixtuoid through a private per-user socket directory (macOS/Linux) and a peer-identity check on the Windows named pipe, so another user on the same host can't read or spoof your agents' activity",
            "Steadier across every CLI — an Antigravity session no longer shows up twice and a Cursor sprite no longer lingers \"working\" after a tool fails, and the upstream-wire-drift watch now covers each CLI's payload fields too, so a silent format change upstream is caught before it drops your agents from the office",
            "Under the hood — a whole-codebase review pass (correctness, security, performance) and a daemon-state refactor that makes illegal states unrepresentable; the office looks the same, just sturdier",
        ]),
        "0.12.0" => Some(&[
            "A sustained whole-codebase cleanup across several review passes — sturdier session tracking (a delegating agent no longer hides its own pending permission; live sessions survive log-content lookalikes), fresher sprites after a project rename, and pathfinding that recovers when a blocked route reopens",
            "The office is more honest under pressure — a full pantry re-steams your coffee, narrow floating windows seat exactly as many agents as fit, and the web hero hires only when a desk is truly free",
            "Under the hood: a deep architecture pass made illegal states unrepresentable and slimmed the render engine's public API (semver-gated), per-CLI transcript knowledge is fully routed through the source registry, and the daily security-audit signal is revived",
        ]),
        "0.11.1" => Some(&[
            "Maintenance release — the animated office is unchanged from 0.11.0; documentation polish across the site and READMEs, plus a supply-chain hardening of how pixtuoid ships",
            "Releases now publish to crates.io and npm via OIDC trusted publishing — CI holds no long-lived registry tokens, shrinking the supply-chain attack surface",
        ]),
        "0.11.0" => Some(&[
            "Pop the office out of the terminal — new `pixtuoid floating` opens a frameless, always-on-top desktop window of the same animated office",
            "First launch greets you with a cinematic move-in and helps you connect your installed agent CLIs; `pixtuoid setup` is the headless twin for scripting and CI",
            "Drive pixtuoid from Raycast — a new extension manages your sources, backed by scriptable `sources` / `connect` / `disconnect --json` commands",
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
            "Project renamed to pixtuoid",
            "Reconnect your CLI in the Sources panel (press s) to update hooks",
            "New env vars: PIXTUOID_SOCKET/HOOK/LOG",
            "Flaky startup test fixed + 250ms rescan",
        ]),
        "0.4.1" => Some(&[
            "Per-floor boot capacity fixes invisible-agent edge case",
            "Connecting a CLI now strips legacy hook entries again",
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

    /// Every shipped version, for the `release_notes_present_for_every_shipped_version`
    /// teeth. `just bump` prepends the new version at the marker below — the twin of
    /// the `[bump-inject-here]` match-arm injection — so this list can't silently
    /// drift out of sync with the arms the way the old hand-maintained array did.
    const SHIPPED_VERSIONS: &[&str] = &[
        // [bump-version-list-here]
        "0.15.0", "0.14.0", "0.13.0", "0.12.0", "0.11.1", "0.11.0", "0.10.0", "0.9.0", "0.8.0",
        "0.7.0", "0.6.1", "0.6.0", "0.5.0", "0.4.1",
    ];

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

    /// Every SHIPPED historical version must keep a non-empty `release_notes`
    /// arm — those arms back the upgrade popup, which `boot_decision` gates on
    /// `release_notes(current).is_some()`. An empty/absent slice renders a
    /// blank popup, so this fails if any arm is deleted or returns `Some(&[])`.
    /// (No exact-prose assertions — that would be brittle to copy edits.)
    #[test]
    fn release_notes_present_for_every_shipped_version() {
        for v in SHIPPED_VERSIONS {
            let notes =
                release_notes(v).unwrap_or_else(|| panic!("missing release_notes arm for {v}"));
            assert!(!notes.is_empty(), "empty release_notes for {v}");
        }
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

    /// The INVERSE of `release_notes_present_for_every_shipped_version`: `just
    /// bump` injects the new version at BOTH `[bump-inject-here]` (the arm) and
    /// `[bump-version-list-here]` (this list). An arm added without the list
    /// entry — the 0.15.0 drift the forward test can't see — makes that release a
    /// permanent gap once the next bump prepends over it. Pin the current version
    /// into the list so the miss fails fast.
    #[test]
    fn current_version_is_in_shipped_versions() {
        let current = env!("CARGO_PKG_VERSION");
        assert!(
            SHIPPED_VERSIONS.contains(&current),
            "CARGO_PKG_VERSION {current:?} is missing from SHIPPED_VERSIONS \
             (the `just bump` [bump-version-list-here] injection was skipped)"
        );
    }

    /// Guard for #110: every hardcoded intra-workspace path-dep `version` (NOT
    /// workspace-inherited) must track the crate version, or a bump that misses
    /// one breaks `cargo publish`. Three such pins exist — `pixtuoid →
    /// pixtuoid-core`, `pixtuoid → pixtuoid-scene`, and `pixtuoid-scene →
    /// pixtuoid-core` — and the whole workspace bumps in lockstep, so all equal
    /// `CARGO_PKG_VERSION`. `just bump` (cargo set-version --workspace) keeps them
    /// synced; this fails fast — in `just test`, preflight, and the release
    /// `check` job — if any drifts. Checks EVERY `path =` + `version = "` line
    /// (not a named subset), so a future workspace path-dep is covered the moment
    /// it's added, not silently left unguarded.
    #[test]
    fn path_dep_version_tracks_crate_version() {
        let assert_tracks = |manifest: &str, who: &str| {
            let mut checked = 0;
            for line in manifest.lines() {
                let l = line.trim_start();
                // An intra-workspace published path-dep carries BOTH `path =` and a
                // hardcoded `version = "..."`; that pair is the #110 publish surface.
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
                    "{who}: path-dep version ({dep_version}) != crate version ({}) — run `just bump` (see #110)",
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
