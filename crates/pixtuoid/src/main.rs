// The lib's rule, for the same reason (see its crate root).
#![cfg_attr(not(test), warn(clippy::print_stdout, clippy::print_stderr))]

mod crash;
mod logging;
mod sources_cli;

use std::io::Write;

use anyhow::Result;
use clap::Parser;
use pixtuoid::cli::{Cli, Cmd, SourceArgs, SourcesAction};
use pixtuoid::{config, doctor, floating, init_pack, install, runtime, setup, sources, validate};

/// `ExitCode`, not `Result`: std would print the error chain raw, and pack and
/// config text reach it; [`pixtuoid::fatal_error_text`] strips it.
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Not `eprintln!`, which panics when the write fails (a broken
            // pipe): std's own error path ignores the failure, and a clean
            // failure must not turn into a crash.
            let _ = writeln!(std::io::stderr(), "{}", pixtuoid::fatal_error_text(&e));
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    crash::install_crash_hook();
    let (log_level, cli_theme, cmd) = Cli::parse().cmd_or_default();

    // Only the terminal `run` TUI needs the terminal's color: `floating` paints
    // real RGB via softbuffer, and every other command is plain text.
    let is_run_tui = matches!(
        &cmd,
        Cmd::Run {
            headless: false,
            ..
        }
    );

    // The office has no legible monochrome fallback, so refuse the canvas with an
    // explanation rather than render block-soup. crossterm needs the
    // $CLICOLOR_FORCE override applied explicitly — it ignores the var on its own.
    // Runs before the truecolor probe, so a dumb terminal never gets DECRQSS.
    if is_run_tui {
        use pixtuoid::term::ColorPreflight;
        match pixtuoid::term::color_preflight(
            pixtuoid_core::platform::text_env("NO_COLOR").as_deref(),
            pixtuoid_core::platform::text_env("CLICOLOR_FORCE").as_deref(),
            pixtuoid_core::platform::text_env("TERM").as_deref(),
        ) {
            ColorPreflight::Proceed => {}
            ColorPreflight::ForceColor => crossterm::style::force_color_output(true),
            ColorPreflight::RefuseNoColor => {
                let _ = writeln!(
                    std::io::stderr(),
                    "pixtuoid: $NO_COLOR is set, so color output is disabled — the \
                     pixel-art office is 24-bit color with no legible monochrome mode \
                     and would render as unreadable blocks. Unset NO_COLOR (or set \
                     CLICOLOR_FORCE=1 to override) to run it, or use \
                     `pixtuoid run --headless` for a text summary."
                );
                return Ok(());
            }
            ColorPreflight::RefuseDumbTerm => {
                let _ = writeln!(
                    std::io::stderr(),
                    "pixtuoid: $TERM=dumb — this terminal can't render the pixel-art \
                     office (no cursor addressing or color). Use a graphical terminal \
                     (Windows Terminal, iTerm2, Ghostty, Alacritty, kitty, WezTerm), \
                     or `pixtuoid run --headless` for a text summary."
                );
                return Ok(());
            }
        }
    }

    // Rather than guess truecolor from a $TERM allowlist, ASK the terminal
    // (DECRQSS) when $COLORTERM hasn't declared it, and only WARN — never gate on
    // Unix (Windows hard-gates VT separately in `tui::mod`).
    // $PIXTUOID_NO_TRUECOLOR_WARN is the escape hatch for a terminal we can't
    // auto-detect. The query runs only inside `warn_zone`, so a healthy truecolor
    // session pays nothing.
    #[cfg(not(windows))]
    if pixtuoid::term::warn_zone(
        is_run_tui,
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
        pixtuoid_core::platform::text_env("COLORTERM").as_deref(),
        pixtuoid_core::platform::text_env("PIXTUOID_NO_TRUECOLOR_WARN").as_deref(),
    ) && pixtuoid::term::query_truecolor(pixtuoid::term::TRUECOLOR_PROBE_TIMEOUT)
        .warrants_warning()
    {
        let _ = writeln!(
            std::io::stderr(),
            "⚠ pixtuoid: your terminal didn't confirm truecolor support — the \
             pixel-art office renders in 24-bit color and may look wrong. Use a \
             truecolor terminal (Windows Terminal, iTerm2, Ghostty, Alacritty, kitty, \
             WezTerm), run `pixtuoid doctor` to check, or set \
             PIXTUOID_NO_TRUECOLOR_WARN=1 to silence."
        );
    }
    let log_level: &'static str = log_level.as_str();
    let tui_active = matches!(&cmd, Cmd::Run { headless, .. } if !*headless)
        || matches!(&cmd, Cmd::Floating { .. });
    let drift = logging::init(tui_active, log_level);

    match cmd {
        Cmd::Run {
            source,
            max_desks: cli_max_desks,
            headless,
            graphics,
        } => {
            let rc = build_run_config(
                cli_theme.as_deref(),
                source,
                cli_max_desks,
                headless,
                graphics,
                drift,
            )?;
            runtime::run(rc)
        }
        Cmd::Floating { source } => {
            // No desk cap: floating seeds its capacity from the window.
            let rc = build_run_config(cli_theme.as_deref(), source, None, false, None, drift)?;
            floating::run(rc)
        }
        Cmd::ValidatePack { pack_dir } => validate::validate_pack(&pack_dir),
        Cmd::InitPack { dest, force } => init_pack::init_pack(&dest, force),
        Cmd::Doctor { graphics } => {
            let report = doctor::run(&pixtuoid::run_log::LogLocation::from_env(), graphics)?;
            write!(pixtuoid::cli_stdout(), "{report}")?;
            Ok(())
        }
        Cmd::Sources { action: None, json } => sources_cli::run_sources_list(json),
        Cmd::Sources {
            action: Some(SourcesAction::Set { ids }),
            json,
        } => sources_cli::run_sources_set(&ids, json),
        Cmd::Connect { ids, json } => sources_cli::run_change(&ids, json, |c, i| {
            sources::connect(c, i).map(|_| sources::ChangeOutcome::Connected)
        }),
        // A folded hook-removal failure is PARTIAL (the flag IS disconnected, but
        // hooks remain), so map it to `Err` for the non-zero exit — a $?-checking
        // script must not be told a clean "disconnected".
        Cmd::Disconnect { ids, json } => {
            sources_cli::run_change(&ids, json, |c, i| match sources::disconnect(c, i)? {
                sources::DisconnectOutcome::HookRemovalFailed(e) => Err(anyhow::anyhow!(
                    "{}: {e}",
                    sources::HOOK_REMOVAL_FAILED_PHRASE
                )),
                sources::DisconnectOutcome::Uninstalled(r) if r.plugin_left_registered => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "{i}: {}",
                        sources::PLUGIN_LEFT_REGISTERED_PHRASE
                    );
                    Ok(sources::ChangeOutcome::Disconnected)
                }
                _ => Ok(sources::ChangeOutcome::Disconnected),
            })
        }
        Cmd::Setup { yes } => sources_cli::run_setup(yes),
        // Packaging interfaces: stdout carries ONLY the generated artifact (the
        // tracing subscriber writes to stderr) so the homebrew
        // `generate_completions_from_executable` / `man` capture stays clean.
        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            // Into a buffer first: clap_complete panics on a failed write.
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "pixtuoid", &mut script);
            pixtuoid::cli_stdout().write_all(&script)?;
            Ok(())
        }
        Cmd::Man => {
            use clap::CommandFactory;
            clap_mangen::Man::new(Cli::command()).render(&mut pixtuoid::cli_stdout())?;
            Ok(())
        }
    }
}

/// Resolve the shared [`runtime::RunConfig`] — the common prelude for `run`
/// (TUI) and `floating` (window).
fn build_run_config(
    cli_theme: Option<&str>,
    source: SourceArgs,
    cli_max_desks: Option<usize>,
    headless: bool,
    cli_graphics: Option<pixtuoid::GraphicsMode>,
    drift: pixtuoid::doctor::DriftSeen,
) -> Result<runtime::RunConfig> {
    let SourceArgs {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
    } = source;
    let cfg_path = config::config_path();
    let mut cfg_warnings = Vec::new();
    let (cfg, load_degraded) = config::load_with_status(&cfg_path, &mut cfg_warnings);
    // A degraded load means the file EXISTS but is malformed — "previously
    // configured", never a first run (the onboarding apply couldn't succeed
    // anyway: update_config refuses to rewrite a malformed config). A missing
    // file warns nothing ⇒ first run.
    let first_run = setup::is_first_run(&cfg, &cfg_path, load_degraded);
    let theme = config::resolve_theme(&cfg, cli_theme, &mut cfg_warnings)?;
    let desk_cap = config::resolve_desk_cap(&cfg, cli_max_desks, &mut cfg_warnings);
    let pack = config::resolve_pack_source(&cfg, pack_dir);
    let pets = config::resolve_pets(&cfg, &mut cfg_warnings);
    let graphics = config::resolve_graphics(&cfg, cli_graphics, &mut cfg_warnings);
    let motion = config::resolve_motion(&cfg, &mut cfg_warnings);
    let connected = config::resolve_connected(&cfg);
    if !headless {
        // Config problems must reach stderr BEFORE any alternate screen / window,
        // not just the log file. Headless already has a stderr tracing subscriber,
        // so re-printing there would duplicate.
        for w in &cfg_warnings {
            let _ = writeln!(std::io::stderr(), "⚠ pixtuoid: {w}");
        }
        warn_broken_installs(&connected);
    }
    Ok(runtime::RunConfig {
        socket,
        projects_root,
        codex_sessions_root,
        pack,
        desk_cap,
        headless,
        config_path: cfg_path,
        theme,
        pets,
        connected,
        log: Some(pixtuoid::run_log::LogLocation::from_env()),
        drift,
        first_run,
        audio: config::resolve_audio(&cfg),
        graphics,
        motion,
    })
}

/// Warn on stderr when a CONNECTED source's hooks are installed but structurally
/// BROKEN — it renders zero sprites with no other hint, so the passive user who
/// never opens the Sources panel still learns. Iterates TARGETS, not the source
/// registry: only an install-bearing source can be install-BROKEN.
fn warn_broken_installs(connected: &std::collections::HashSet<String>) {
    for &t in install::TARGETS {
        if !connected.contains(t.core_source) {
            continue;
        }
        let diag = doctor::diagnose(t.core_source, "", None);
        if diag.is_broken() {
            let issues = diag
                .install
                .as_ref()
                .map(|v| v.issues.join("; "))
                .unwrap_or_default();
            let _ = writeln!(
                std::io::stderr(),
                "⚠ pixtuoid: {} hooks are installed but BROKEN: {issues} — \
                 reconnect in the Sources panel (press s)",
                t.core_source
            );
        }
    }
}
