use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "pixtuoid",
    version,
    about = "Terminal pixel-art office for AI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,

    /// Log verbosity. The TUI always logs warn+ to a file
    /// (~/.cache/pixtuoid/log, or $PIXTUOID_LOG /
    /// $XDG_STATE_HOME/pixtuoid/log); debug/trace raise the file's
    /// verbosity. Non-TUI commands log to stderr at this level.
    /// ($RUST_LOG remains the escape hatch for full directive syntax.)
    #[arg(long, global = true, value_enum, default_value = "info")]
    pub log_level: LogLevel,

    /// Color theme: normal, cyberpunk, dracula, tokyo-night, catppuccin, gruvbox.
    #[arg(long, global = true)]
    pub theme: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the TUI (default if no subcommand given).
    Run {
        /// Empty/whitespace is rejected: an explicit override deserves a loud
        /// answer, and bind("") would strand a relative `.lock` in the CWD.
        #[arg(long, value_parser = parse_nonempty_path)]
        socket: Option<PathBuf>,
        #[arg(long)]
        projects_root: Option<PathBuf>,
        /// Override the Codex sessions root (default ~/.codex/sessions).
        /// Point at a temp dir to replay fixtures into a headless run.
        #[arg(long)]
        codex_sessions_root: Option<PathBuf>,
        #[arg(long)]
        pack_dir: Option<PathBuf>,
        /// Cap desks per floor, ≥ 1 (auto-computed from terminal size if
        /// unset). 0 is rejected: it would permanently zero every floor's
        /// capacity and silently drop every agent (the boot atomics only
        /// grow via fetch_max, gated on capacity > 0).
        #[arg(long, hide = true, value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
        max_desks: Option<usize>,
        /// Skip the TUI entirely — useful for CI / scripting.
        /// Prints a one-line `agents=[label@desk:state, ...]` summary every
        /// 200ms when it changes.
        #[arg(long, default_value_t = false)]
        headless: bool,
    },
    /// Install pixtuoid hooks into agent CLI config(s).
    InstallHooks {
        /// Empty/whitespace is rejected: an explicit override deserves a loud
        /// answer — unfiltered, `""` wins the precedence, cwd-joins to the
        /// current directory, and gets embedded as the hook with no warning.
        #[arg(long, value_parser = parse_nonempty_path)]
        hook_path: Option<PathBuf>,
        /// Config file override (single target only; conflicts with --target all).
        #[arg(long, alias = "settings")]
        config: Option<PathBuf>,
        #[arg(long, value_enum)]
        target: Option<TargetName>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Remove pixtuoid hook entries from agent CLI config(s).
    UninstallHooks {
        #[arg(long, alias = "settings")]
        config: Option<PathBuf>,
        #[arg(long, value_enum)]
        target: Option<TargetName>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Validate a custom sprite pack directory.
    ValidatePack {
        /// Path to the pack directory (must contain pack.toml).
        pack_dir: PathBuf,
    },
    /// Extract a skeleton sprite pack to a directory for customization.
    InitPack {
        /// Destination directory (created if absent).
        dest: PathBuf,
        /// Overwrite existing files.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

/// `--log-level` values. A typed enum (not a free `String`) so a typo like
/// `dbug` is a hard clap parse error instead of silently parsing as an
/// `EnvFilter` TARGET directive (`dbug=trace`) that filters everything off —
/// the #157 silent-diagnostics class. `$RUST_LOG` stays the escape hatch for
/// full tracing directive syntax (targets, per-module levels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// The tracing level token — exactly the strings the old free-form
    /// `--log-level` accepted, so every `EnvFilter` built from it is
    /// unchanged; the enum only moved typo rejection to the clap seam.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TargetName {
    Claude,
    Codex,
    Reasonix,
    Codewhale,
    All,
}

impl TargetName {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetName::Claude => "claude",
            TargetName::Codex => "codex",
            TargetName::Reasonix => "reasonix",
            TargetName::Codewhale => "codewhale",
            TargetName::All => "all",
        }
    }
}

impl Cli {
    pub fn cmd_or_default(self) -> (LogLevel, Option<String>, Cmd) {
        let level = self.log_level;
        let theme = self.theme;
        let cmd = self.cmd.unwrap_or(Cmd::Run {
            socket: None,
            projects_root: None,
            codex_sessions_root: None,
            pack_dir: None,
            max_desks: None,
            headless: false,
        });
        (level, theme, cmd)
    }
}

fn parse_nonempty_path(s: &str) -> Result<PathBuf, String> {
    if s.trim().is_empty() {
        Err("must not be empty — pass a path, or drop the flag for the default".into())
    } else {
        Ok(PathBuf::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hook_path_is_a_hard_parse_error() {
        // Unfiltered, an explicit `--hook-path ""` wins the override
        // precedence, cwd-joins to the launch directory (which exists), and
        // is embedded as the hook command with no warning at all.
        for v in ["", "   "] {
            let err =
                Cli::try_parse_from(["pixtuoid", "install-hooks", "--hook-path", v]).unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn empty_socket_is_a_hard_parse_error() {
        // An explicit override deserves a loud answer: bind("") would die
        // late with an opaque error and strand a relative `.lock` in the CWD.
        for v in ["", "   "] {
            let err = Cli::try_parse_from(["pixtuoid", "run", "--socket", v]).unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn log_level_typo_is_a_hard_parse_error() {
        // A free String silently parsed a typo like `dbug` as an EnvFilter
        // TARGET directive (everything filtered off — the #157 class); the
        // ValueEnum makes it a loud clap error at the seam.
        let err = Cli::try_parse_from(["pixtuoid", "--log-level", "dbug"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn max_desks_zero_is_a_hard_parse_error() {
        // 0 would permanently zero every floor's capacity (the per-frame
        // re-seed guards `> 0`, so the atomics never grow) — every agent
        // silently dropped. Rejected at the clap seam.
        let err = Cli::try_parse_from(["pixtuoid", "run", "--max-desks", "0"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn log_level_valid_values_map_to_the_same_filter_tokens_as_before() {
        // The enum's as_str must be exactly the strings the old free-form
        // --log-level accepted, and each must build a valid EnvFilter — the
        // typed seam may not change any accepted filter.
        for (raw, lvl) in [
            ("error", LogLevel::Error),
            ("warn", LogLevel::Warn),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("trace", LogLevel::Trace),
        ] {
            let cli = Cli::try_parse_from(["pixtuoid", "--log-level", raw]).unwrap();
            assert_eq!(cli.log_level, lvl);
            assert_eq!(lvl.as_str(), raw, "filter token must be unchanged");
            assert!(
                tracing_subscriber::EnvFilter::try_new(lvl.as_str()).is_ok(),
                "{raw} must parse as an EnvFilter level"
            );
        }
    }

    #[test]
    fn max_desks_positive_parses() {
        let cli = Cli::try_parse_from(["pixtuoid", "run", "--max-desks", "4"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Run {
                max_desks: Some(4),
                ..
            })
        ));
    }

    #[test]
    fn target_name_as_str_covers_all_arms() {
        assert_eq!(TargetName::Claude.as_str(), "claude");
        assert_eq!(TargetName::Codex.as_str(), "codex");
        assert_eq!(TargetName::Reasonix.as_str(), "reasonix");
        assert_eq!(TargetName::Codewhale.as_str(), "codewhale");
        assert_eq!(TargetName::All.as_str(), "all");
    }

    #[test]
    fn cmd_or_default_returns_run_when_no_subcommand() {
        let cli = Cli {
            cmd: None,
            log_level: LogLevel::Info,
            theme: None,
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, LogLevel::Info);
        assert!(theme.is_none());
        assert!(matches!(
            cmd,
            Cmd::Run {
                headless: false,
                max_desks: None,
                ..
            }
        ));
    }

    #[test]
    fn cmd_or_default_preserves_explicit_subcommand() {
        let cli = Cli {
            cmd: Some(Cmd::UninstallHooks {
                config: None,
                target: None,
                yes: false,
            }),
            log_level: LogLevel::Debug,
            theme: Some("cyberpunk".into()),
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, LogLevel::Debug);
        assert_eq!(theme.as_deref(), Some("cyberpunk"));
        assert!(matches!(cmd, Cmd::UninstallHooks { .. }));
    }
}
