use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use pixtuoid::cli::{Cli, Cmd};
use pixtuoid::{config, init_pack, install, runtime, validate};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    install_crash_hook();
    let (log_level, cli_theme, cmd) = Cli::parse().cmd_or_default();
    let make_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&log_level));

    // Log routing:
    //   TUI mode: silent by default (no file, no stderr). Only logs when
    //     $PIXTUOID_LOG is set or --log-level is debug/trace.
    //     Crash reporting is handled separately by the panic hook.
    //   Non-TUI (install-hooks, uninstall-hooks, --headless): stderr.
    let tui_active = matches!(&cmd, Cmd::Run { headless, .. } if !*headless);
    let wants_verbose = matches!(log_level.as_str(), "debug" | "trace");
    let explicit_log_file = std::env::var("PIXTUOID_LOG").is_ok();

    if tui_active && (wants_verbose || explicit_log_file) {
        if let Ok(path) = log_file_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                let writer = Arc::new(Mutex::new(f));
                tracing_subscriber::fmt()
                    .with_env_filter(make_filter())
                    .with_ansi(false)
                    .with_writer(move || MutexFileWriter(writer.clone()))
                    .init();
            }
        }
    } else if !tui_active {
        tracing_subscriber::fmt()
            .with_env_filter(make_filter())
            .with_writer(std::io::stderr)
            .init();
    }

    match cmd {
        Cmd::Run {
            socket,
            projects_root,
            codex_sessions_root,
            pack_dir,
            max_desks: cli_max_desks,
            headless,
        } => {
            let cfg_path = config::config_path();
            let cfg = config::load(&cfg_path);
            let theme_name = config::resolve_theme(&cfg, cli_theme);
            let desk_cap = cli_max_desks.or(cfg.max_desks);
            let pack_dir = config::resolve_pack_dir(&cfg, pack_dir);
            let pets = config::resolve_pets(&cfg);
            runtime::run(
                runtime::RunConfig {
                    socket,
                    projects_root,
                    codex_sessions_root,
                    pack_dir,
                    desk_cap,
                    headless,
                    config_path: cfg_path,
                    pets,
                },
                theme_name,
            )
        }
        Cmd::InstallHooks {
            hook_path,
            config,
            target,
            yes,
        } => install::install(install::InstallArgs {
            hook_path,
            config,
            target,
            yes,
        }),
        Cmd::UninstallHooks {
            config,
            target,
            yes,
        } => install::uninstall(install::UninstallArgs {
            config,
            target,
            yes,
        }),
        Cmd::ValidatePack { pack_dir } => validate::validate_pack(&pack_dir),
        Cmd::InitPack { dest, force } => init_pack::init_pack(&dest, force),
    }
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stderr(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        );

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

    let title_msg = if panic_msg.len() > 80 {
        let cut = truncate_to_char_boundary(panic_msg, 80);
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
    if let Ok(state) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(format!("{state}/pixtuoid/crash.log"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(format!("{home}/.cache/pixtuoid/crash.log"));
    }
    PathBuf::from("/tmp/pixtuoid-crash.log")
}

fn log_file_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("PIXTUOID_LOG") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(state) = std::env::var("XDG_STATE_HOME") {
        return Ok(PathBuf::from(format!("{state}/pixtuoid/log")));
    }
    let home = std::env::var("HOME")?;
    Ok(PathBuf::from(format!("{home}/.cache/pixtuoid/log")))
}

/// Adapter that gives `tracing-subscriber` a `Write`-able file behind a Mutex.
struct MutexFileWriter(Arc<Mutex<std::fs::File>>);

impl std::io::Write for MutexFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .flush()
    }
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
