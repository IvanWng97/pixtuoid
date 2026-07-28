//! Crash reporting: the panic hook that restores the terminal, appends a
//! timestamped backtrace to `~/.cache/pixtuoid/crash.log`, and prints a
//! pre-filled GitHub issue URL. Binary-crate module (lifted out of `main.rs`);
//! `main()` installs it first thing.

use std::fs::OpenOptions;
use std::path::PathBuf;

pub(crate) fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let _ = restore_terminal(&mut std::io::stdout());
        // Stream-INDEPENDENT, unlike the two above: crossterm resolves its own
        // fd from /dev/tty here. Must still run AFTER the mouse-capture restore
        // — same ordering contract as tui::teardown_terminal (the WHY is there).
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

/// Undo the terminal modes `tui::setup_terminal` entered, writing into `w`.
///
/// `w` MUST be the stream the alternate screen was ENTERED on — `stdout`. These
/// are ANSI sequences delivered to whatever writer you hand `execute!`, and
/// every other site in the terminal-mode state machine drives stdout
/// (`setup_terminal` does `execute!(stdout(), EnterAlternateScreen,
/// EnableMouseCapture)`; `teardown_terminal` writes through `Terminal<
/// CrosstermBackend<Stdout>>`). Writing the restore to stderr instead worked
/// only while stderr happened to be the same tty: under `pixtuoid run
/// 2>/dev/null` — a plausible move, since main.rs eprintlns its truecolor and
/// config warnings — the sequences went to the redirect and the user was left
/// on the alternate screen with mouse reporting on, needing `reset`.
///
/// The human-readable report deliberately stays on stderr (see the caller): a
/// non-TUI command with a redirected stdout gets a few escape bytes in its
/// output on a panic, which is the cheap side of this trade — those commands
/// never entered the alt screen, and their output is already truncated by the
/// crash.
fn restore_terminal(w: &mut impl std::io::Write) -> std::io::Result<()> {
    crossterm::execute!(
        w,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )
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

    // Derive from the ONE repo-URL authority (the lib's version_popup.rs REPO_URL — the
    // same const the version popup + bulletin board open; crash.rs is a BIN-crate
    // module, hence the `pixtuoid::` path). The test pins the expanded literal.
    format!(
        "{}/issues/new?labels=crash-report&title={}&body={}",
        pixtuoid::tui::widgets::REPO_URL,
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
    // Empty or RELATIVE XDG_STATE_HOME = unset (XDG spec; nonempty_abs_env) — an
    // unfiltered "" yields root `/pixtuoid/...`, a relative one lands CWD-relative.
    if let Some(state) = pixtuoid::install::nonempty_abs_env("XDG_STATE_HOME") {
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

    /// The escape the alternate screen is left with. Asserted as a literal
    /// rather than re-executing the command into a second buffer, which would
    /// only prove `execute!` is deterministic.
    const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

    #[test]
    fn the_restore_writes_the_leave_sequence_into_the_writer_it_is_given() {
        let mut buf: Vec<u8> = Vec::new();
        restore_terminal(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains(LEAVE_ALT_SCREEN),
            "the restore must reach the writer handed to it, not a fixed stream: {s:?}"
        );
    }

    /// Which STREAM the installed hook writes to, end to end. The panic hook is
    /// process-global and writes to a real fd, so the only way to observe its
    /// choice is from outside: re-exec this test binary with the child marker,
    /// let a real panic fire the hook, and read the two pipes apart.
    ///
    /// Unix-only: on Windows `execute!` falls back to WinAPI console calls when
    /// the writer is not VT-capable, so a piped stdout carries no bytes to
    /// assert on — and the defect itself is the Unix fd-redirection case.
    #[cfg(unix)]
    #[test]
    fn the_hook_restores_on_stdout_and_keeps_the_report_on_stderr() {
        const CHILD: &str = "PIXTUOID_CRASH_HOOK_STREAM_CHILD";
        if std::env::var_os(CHILD).is_some() {
            install_crash_hook();
            panic!("deliberate panic: the crash-hook stream probe");
        }
        // Isolated XDG_STATE_HOME so the child's crash.log never lands in the
        // developer's real ~/.cache.
        let state = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash::tests::the_hook_restores_on_stdout_and_keeps_the_report_on_stderr",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("XDG_STATE_HOME", state.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stdout.contains(LEAVE_ALT_SCREEN),
            "the alt screen is ENTERED on stdout (tui::setup_terminal), so the panic \
             restore must go there — on stderr, `pixtuoid run 2>/dev/null` strands it.\n\
             stdout: {stdout:?}"
        );
        assert!(
            !stderr.contains(LEAVE_ALT_SCREEN),
            "stderr is the human-readable channel, not the terminal-mode one: {stderr:?}"
        );
        assert!(
            stderr.contains("crashed"),
            "the crash report itself still belongs on stderr: {stderr:?}"
        );
    }

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

    #[test]
    fn crash_log_path_rejects_a_relative_xdg_state_home() {
        // Sibling of log_file_path's test — same nonempty_abs_env call site: a
        // relative/empty XDG_STATE_HOME → ~/.cache fallback (shares the bin ENV_LOCK).
        let _env = crate::logging::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved_xdg = std::env::var_os("XDG_STATE_HOME");
        let home = pixtuoid_core::platform::user_home_opt().expect("a home dir in the test env");
        let cache = PathBuf::from(home)
            .join(".cache")
            .join("pixtuoid")
            .join("crash.log");
        for rel in ["", "   ", "rel/state", "~/state"] {
            std::env::set_var("XDG_STATE_HOME", rel);
            assert_eq!(
                crash_log_path(),
                cache,
                "relative XDG_STATE_HOME {rel:?} must fall back to ~/.cache"
            );
        }
        let abs = if cfg!(windows) { "C:/state" } else { "/state" };
        std::env::set_var("XDG_STATE_HOME", abs);
        assert_eq!(
            crash_log_path(),
            PathBuf::from(format!("{abs}/pixtuoid/crash.log"))
        );
        match saved_xdg {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}
