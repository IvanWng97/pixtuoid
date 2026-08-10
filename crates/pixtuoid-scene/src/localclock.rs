//! Local wall-clock instants for tests.
//!
//! The sky model decodes `now` back through `chrono::Local` (`local_hour_frac`),
//! so a test that builds its instants as epoch offsets is really asking for
//! "whatever local hours this runner's `$TZ` happens to give". That is how
//! `the_foreground_layer_is_lit_by_the_clock` came to pass in CI (UTC) and fail
//! on a clean checkout in UTC-6, and why five copies of the same
//! `with_ymd_and_hms` incantation had accumulated across the sky, ambient and
//! floor suites before this module.
//!
//! Anything whose ASSERTION depends on the hour builds its instants here. A test
//! that just needs "some fixed moment" — a reducer lifecycle, an animation phase
//! — is TZ-independent already and should keep using a plain epoch offset.

use std::time::SystemTime;

use chrono::TimeZone;

/// Day 0 for every helper. January, so the sun arc is the same short winter one
/// in each call.
const BASE: (i32, u32, u32) = (2026, 1, 1);

/// `day` counts FORWARD from the base date by calendar arithmetic, not as a
/// day-of-month: a search that walks days to find a weather or moon phase runs
/// past 31, and `with_ymd_and_hms(.., 1, 40, ..)` is an invalid date that panics
/// in exactly the zones whose day-hash sequence needs the extra days.
fn local(day: u32, h: u32, m: u32) -> SystemTime {
    let date = chrono::NaiveDate::from_ymd_opt(BASE.0, BASE.1, BASE.2).expect("valid base date")
        + chrono::Days::new(u64::from(day));
    chrono::Local
        .from_local_datetime(&date.and_hms_opt(h, m, 0).expect("valid wall-clock time"))
        .single()
        .expect("a fixed January local time is unambiguous in every zone")
        .into()
}

/// Local `h:00` on the reference day.
pub(crate) fn at_hour(h: u32) -> SystemTime {
    local(0, h, 0)
}

/// Local `h:m` on the reference day.
pub(crate) fn at_hour_min(h: u32, m: u32) -> SystemTime {
    local(0, h, m)
}

/// Local `h:00`, `day` days after the base date — weather and moon phase vary by
/// day at a fixed hour, so a search over days reaches different sky states. Any
/// `day` is valid, including past the end of the month.
pub(crate) fn on_day(day: u32, h: u32) -> SystemTime {
    local(day, h, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the module: the same call must name the same LOCAL
    /// hour whatever `$TZ` is, which a raw epoch offset does not.
    #[test]
    fn every_helper_lands_on_the_local_hour_it_names() {
        use chrono::{Datelike, Timelike};
        for h in [0, 2, 5, 12, 17, 23] {
            let t: chrono::DateTime<chrono::Local> = at_hour(h).into();
            assert_eq!(t.hour(), h, "at_hour({h}) landed on {}", t.hour());
        }
        let t: chrono::DateTime<chrono::Local> = at_hour_min(7, 30).into();
        assert_eq!((t.hour(), t.minute()), (7, 30));
        let t: chrono::DateTime<chrono::Local> = on_day(8, 2).into();
        assert_eq!(
            (t.day(), t.hour()),
            (9, 2),
            "on_day counts days FORWARD from the base"
        );
        // Past the end of January: the form this replaced panicked here.
        let t: chrono::DateTime<chrono::Local> = on_day(45, 7).into();
        assert_eq!((t.month(), t.day(), t.hour()), (2, 15, 7));
    }

    /// Twelve hours apart must stay twelve hours apart — the property the
    /// lighting suite's day-vs-night pair rests on.
    #[test]
    fn hours_are_ordered_and_spaced_as_named() {
        let (noon, night) = (at_hour(12), at_hour(0));
        assert_eq!(
            noon.duration_since(night).expect("noon is after midnight"),
            std::time::Duration::from_secs(12 * 3600)
        );
    }
}

/// `name:line` for every local-instant construction under `root`, this module
/// excepted.
#[cfg(test)]
fn bypasses_in(root: &std::path::Path) -> Vec<String> {
    const FORMS: [&str; 3] = ["with_ymd_and_hms", "from_local_datetime", "Local.timestamp"];
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "rs")
                || path.file_name().is_some_and(|n| n == "localclock.rs")
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            for (i, line) in text.lines().enumerate() {
                if FORMS.iter().any(|f| line.contains(f)) && !line.trim_start().starts_with("//") {
                    out.push(format!("{name}:{}", i + 1));
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
fn crate_src() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[cfg(test)]
mod sweep {
    /// The recurrence gate. A helper nobody is forced to use is a helper the
    /// next hour-dependent test re-invents as an epoch offset, which is how the
    /// lighting suite came to pass in UTC and fail in UTC-6.
    #[test]
    fn nothing_outside_this_module_builds_a_local_instant() {
        let found = super::bypasses_in(&super::crate_src());
        assert!(
            found.is_empty(),
            "build local instants through `localclock`, not directly: {found:?}"
        );
    }

    /// The gate above passing means nothing until the scan is shown to FIND one.
    #[test]
    fn the_sweep_finds_a_planted_bypass() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("deep")).expect("mkdir");
        std::fs::write(
            dir.path().join("deep/planted.rs"),
            "fn f() {\n    chrono::Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 0);\n}\n",
        )
        .expect("write");
        assert_eq!(
            super::bypasses_in(dir.path()),
            vec!["planted.rs:2".to_string()],
            "the scan must reach a nested file and report its line"
        );
        // And it must not fire on the same form inside a comment.
        std::fs::write(
            dir.path().join("deep/planted.rs"),
            "fn f() {\n    // with_ymd_and_hms is what this replaces\n}\n",
        )
        .expect("write");
        assert!(
            super::bypasses_in(dir.path()).is_empty(),
            "prose naming the form is not a bypass"
        );
    }
}
