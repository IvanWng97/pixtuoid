//! Crash reporting: the panic hook that restores the terminal, appends a
//! timestamped backtrace to `~/.cache/pixtuoid/crash.log`, and prints a
//! pre-filled GitHub issue URL. Binary-crate module (lifted out of `main.rs`);
//! `main()` installs it first thing.

use std::fs::OpenOptions;
use std::path::PathBuf;

pub(crate) fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Same ordering contract as tui::teardown_terminal: mouse-capture
        // restore must precede disable_raw_mode (see the WHY there).
        let _ = crossterm::execute!(
            std::io::stderr(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();

        let version = env!("CARGO_PKG_VERSION");
        let crash_path = crash_log_path();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let panic_msg = extract_panic_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();

        let bt = std::backtrace::Backtrace::force_capture();
        let bt_str = bt.to_string();

        let mut report = String::new();
        report.push_str(&format!("pixtuoid v{version} crashed at {timestamp}\n"));
        report.push_str(&format!("{panic_msg}\n  at {location}\n\n"));
        report.push_str(&bt_str);
        report.push('\n');

        if let Some(parent) = crash_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            use std::io::Write;
            let _ = f.write_all(report.as_bytes());
        }

        let issue_url = build_issue_url(version, &panic_msg, &location, &bt_str, &crash_path);

        eprintln!("\n\x1b[1;31mpixtuoid v{version} crashed — sorry about that.\x1b[0m\n");
        eprintln!("  \x1b[2m{panic_msg}\x1b[0m");
        eprintln!("  \x1b[2mat {location}\x1b[0m\n");
        eprintln!("  \x1b[1mHelp fix it\x1b[0m — open this link to file a pre-filled bug report");
        eprintln!("  (panic + backtrace already included, no typing needed):\n");
        eprintln!("  \x1b[4m{issue_url}\x1b[0m\n");
        eprintln!(
            "  Full backtrace saved to \x1b[2m{}\x1b[0m",
            crash_path.display()
        );
        eprintln!("  \x1b[2m(attach if the reviewer asks — the link above only carries a truncated trace)\x1b[0m\n");
    }));
}

#[allow(deprecated)]
fn extract_panic_message(info: &std::panic::PanicInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic".to_string()
}

fn build_issue_url(
    version: &str,
    panic_msg: &str,
    location: &str,
    backtrace: &str,
    crash_path: &std::path::Path,
) -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    // Truncate an over-long panic message for the crash-report title.
    const PANIC_TITLE_MAX_LEN: usize = 80;
    let title_msg = if panic_msg.len() > PANIC_TITLE_MAX_LEN {
        let cut = truncate_to_char_boundary(panic_msg, PANIC_TITLE_MAX_LEN);
        format!("{}…", &panic_msg[..cut])
    } else {
        panic_msg.to_string()
    };
    let title = format!("Crash: {title_msg}");

    // Truncate backtrace to keep URL under GitHub's 8191-byte limit.
    const MAX_BT: usize = 1500;
    let bt_body = if backtrace.len() > MAX_BT {
        let cut = truncate_to_char_boundary(backtrace, MAX_BT);
        format!(
            "{}\n\n... truncated — see {} for full trace",
            &backtrace[..cut],
            crash_path.display()
        )
    } else {
        backtrace.to_string()
    };

    let body = format!(
        "## Environment\n\
         - **Version:** {version}\n\
         - **OS:** {os}/{arch}\n\n\
         ## Panic\n\
         ```\n{panic_msg}\n  at {location}\n```\n\n\
         ## Backtrace\n\
         ```\n{bt_body}\n```\n"
    );

    format!(
        "https://github.com/IvanWng97/pixtuoid/issues/new?labels=crash-report&title={}&body={}",
        percent_encode(&title),
        percent_encode(&body),
    )
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

fn crash_log_path() -> PathBuf {
    // Empty XDG_STATE_HOME = unset (see io::nonempty_env) — left unfiltered,
    // "" yields the root-absolute `/pixtuoid/...` (unwritable for non-root).
    if let Some(state) = pixtuoid::install::io::nonempty_env("XDG_STATE_HOME") {
        return PathBuf::from(format!("{state}/pixtuoid/crash.log"));
    }
    if let Some(home) = pixtuoid_core::platform::user_home_opt() {
        return PathBuf::from(home)
            .join(".cache")
            .join("pixtuoid")
            .join("crash.log");
    }
    std::env::temp_dir().join("pixtuoid-crash.log")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), 5);
        assert_eq!(
            &"hello world"[..truncate_to_char_boundary("hello world", 5)],
            "hello"
        );
    }

    #[test]
    fn truncate_multibyte_boundary() {
        // "café" is 5 bytes: c(1) a(1) f(1) é(2)
        let s = "café";
        assert_eq!(s.len(), 5);
        // Cutting at byte 4 lands inside the é (2-byte char starting at 3)
        let cut = truncate_to_char_boundary(s, 4);
        assert_eq!(cut, 3);
        assert_eq!(&s[..cut], "caf");
    }

    #[test]
    fn truncate_beyond_length() {
        assert_eq!(truncate_to_char_boundary("short", 100), 5);
    }

    #[test]
    fn percent_encode_ascii() {
        assert_eq!(percent_encode("hello"), "hello");
        assert_eq!(percent_encode("a b"), "a%20b");
    }

    #[test]
    fn percent_encode_special_chars() {
        assert_eq!(percent_encode("#&="), "%23%26%3D");
        assert_eq!(percent_encode("a\nb"), "a%0Ab");
    }

    #[test]
    fn build_issue_url_starts_with_github() {
        let url = build_issue_url(
            "0.4.0",
            "test panic",
            "file.rs:1:1",
            "bt",
            Path::new("/tmp/x"),
        );
        assert!(url.starts_with("https://github.com/IvanWng97/pixtuoid/issues/new?"));
        assert!(url.contains("labels=crash-report"));
        assert!(url.contains("title="));
        assert!(url.contains("body="));
    }

    #[test]
    fn build_issue_url_truncates_long_backtrace() {
        let long_bt = "x".repeat(2000);
        let url = build_issue_url("0.4.0", "msg", "loc", &long_bt, Path::new("/tmp/x"));
        // URL should stay under GitHub's 8191 byte limit
        assert!(url.len() < 8191);
    }
}
