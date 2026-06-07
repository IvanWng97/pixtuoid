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

    /// Log verbosity: error, warn, info, debug, trace. The TUI always logs
    /// warn+ to a file (~/.cache/pixtuoid/log, or $PIXTUOID_LOG /
    /// $XDG_STATE_HOME/pixtuoid/log); debug/trace raise the file's
    /// verbosity. Non-TUI commands log to stderr at this level.
    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,

    /// Color theme: normal, cyberpunk, dracula, tokyo-night, catppuccin, gruvbox.
    #[arg(long, global = true)]
    pub theme: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the TUI (default if no subcommand given).
    Run {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        projects_root: Option<PathBuf>,
        /// Override the Codex sessions root (default ~/.codex/sessions).
        /// Point at a temp dir to replay fixtures into a headless run.
        #[arg(long)]
        codex_sessions_root: Option<PathBuf>,
        #[arg(long)]
        pack_dir: Option<PathBuf>,
        /// Cap desks per floor (auto-computed from terminal size if unset).
        #[arg(long, hide = true)]
        max_desks: Option<usize>,
        /// Skip the TUI entirely — useful for CI / scripting.
        /// Prints a JSON snapshot of SceneState every 200ms when it changes.
        #[arg(long, default_value_t = false)]
        headless: bool,
    },
    /// Install pixtuoid hooks into agent CLI config(s).
    InstallHooks {
        #[arg(long)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TargetName {
    Claude,
    Codex,
    All,
}

impl TargetName {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetName::Claude => "claude",
            TargetName::Codex => "codex",
            TargetName::All => "all",
        }
    }
}

impl Cli {
    pub fn cmd_or_default(self) -> (String, Option<String>, Cmd) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_name_as_str_covers_all_arms() {
        assert_eq!(TargetName::Claude.as_str(), "claude");
        assert_eq!(TargetName::Codex.as_str(), "codex");
        assert_eq!(TargetName::All.as_str(), "all");
    }

    #[test]
    fn cmd_or_default_returns_run_when_no_subcommand() {
        let cli = Cli {
            cmd: None,
            log_level: "info".into(),
            theme: None,
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, "info");
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
            log_level: "debug".into(),
            theme: Some("cyberpunk".into()),
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, "debug");
        assert_eq!(theme.as_deref(), Some("cyberpunk"));
        assert!(matches!(cmd, Cmd::UninstallHooks { .. }));
    }
}
