mod crash;
mod logging;
mod sources_cli;

use anyhow::Result;
use clap::Parser;
use pixtuoid::cli::{Cli, Cmd, SourceArgs, SourcesAction};
use pixtuoid::{config, doctor, floating, init_pack, install, runtime, setup, sources, validate};

fn main() -> Result<()> {
    crash::install_crash_hook();
    let (log_level, cli_theme, cmd) = Cli::parse().cmd_or_default();

    // The terminal `run` TUI is the only command that paints the pixel-art canvas:
    // --headless is a text summary, `doctor`/`sources` are plain output, and
    // `floating` paints real RGB via softbuffer — none need the terminal's color.
    let is_run_tui = matches!(
        &cmd,
        Cmd::Run {
            headless: false,
            ..
        }
    );

    // Color preflight (cross-platform — crossterm strips our 24-bit SGR to a bare
    // reset under $NO_COLOR, and a $TERM=dumb terminal can't render escapes at all;
    // either way the office, which has no legible monochrome fallback, would be
    // unreadable). Refuse the canvas with a one-line explanation instead of
    // rendering block-soup, honoring the BSD $CLICOLOR_FORCE > $NO_COLOR override
    // (crossterm needs the force call applied explicitly — it ignores
    // $CLICOLOR_FORCE on its own). This runs before the truecolor probe, so a dumb
    // terminal never gets DECRQSS escapes.
    if is_run_tui {
        use pixtuoid::term::ColorPreflight;
        match pixtuoid::term::color_preflight(
            std::env::var("NO_COLOR").ok().as_deref(),
            std::env::var("CLICOLOR_FORCE").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ) {
            ColorPreflight::Proceed => {}
            ColorPreflight::ForceColor => crossterm::style::force_color_output(true),
            ColorPreflight::RefuseNoColor => {
                eprintln!(
                    "pixtuoid: $NO_COLOR is set, so color output is disabled — the \
                     pixel-art office is 24-bit color with no legible monochrome mode \
                     and would render as unreadable blocks. Unset NO_COLOR (or set \
                     CLICOLOR_FORCE=1 to override) to run it, or use \
                     `pixtuoid run --headless` for a text summary."
                );
                return Ok(());
            }
            ColorPreflight::RefuseDumbTerm => {
                eprintln!(
                    "pixtuoid: $TERM=dumb — this terminal can't render the pixel-art \
                     office (no cursor addressing or color). Use a graphical terminal \
                     (Windows Terminal, iTerm2, Ghostty, Alacritty, kitty, WezTerm), \
                     or `pixtuoid run --headless` for a text summary."
                );
                return Ok(());
            }
        }
    }

    // Truecolor preflight: the terminal TUI renders 24-bit half-block pixels; a
    // non-truecolor terminal renders them approximated/garbled with no other hint.
    // Rather than guess from a $TERM allowlist, ASK the terminal (DECRQSS) when
    // $COLORTERM hasn't already declared truecolor — warn ONCE on the pre-altscreen
    // stderr channel only if the terminal doesn't confirm. Never gate on Unix;
    // Windows hard-gates VT separately in `tui::mod`. $PIXTUOID_NO_TRUECOLOR_WARN
    // is an explicit escape hatch for a terminal we can't auto-detect (#397). The
    // `floating` window paints real RGB pixels via softbuffer, not terminal SGR,
    // so it is exempt. The query only runs when warn_zone holds, so a healthy
    // truecolor session (COLORTERM set) pays nothing.
    #[cfg(not(windows))]
    if pixtuoid::term::warn_zone(
        is_run_tui,
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("PIXTUOID_NO_TRUECOLOR_WARN").ok().as_deref(),
    ) && pixtuoid::term::query_truecolor(pixtuoid::term::TRUECOLOR_PROBE_TIMEOUT) != Some(true)
    {
        eprintln!(
            "⚠ pixtuoid: your terminal didn't confirm truecolor support — the \
             pixel-art office renders in 24-bit color and may look wrong. Use a \
             truecolor terminal (Windows Terminal, iTerm2, Ghostty, Alacritty, kitty, \
             WezTerm), run `pixtuoid doctor` to check, or set \
             PIXTUOID_NO_TRUECOLOR_WARN=1 to silence."
        );
    }
    // The typed LogLevel's as_str is exactly the old free-string levels, so
    // every filter built in logging::init is unchanged — the enum only moved
    // typo rejection to the clap seam (a typo used to parse as a bogus
    // EnvFilter TARGET directive that silently filtered everything off,
    // #157 class).
    let log_level: &'static str = log_level.as_str();
    let tui_active = matches!(&cmd, Cmd::Run { headless, .. } if !*headless)
        || matches!(&cmd, Cmd::Floating { .. });
    logging::init(tui_active, log_level);

    match cmd {
        Cmd::Run {
            source,
            max_desks: cli_max_desks,
            headless,
        } => {
            let rc = build_run_config(cli_theme.as_deref(), source, cli_max_desks, headless)?;
            runtime::run(rc)
        }
        Cmd::Floating { source } => {
            // Floating reuses the TUI run prelude (theme/pack/pets/sources/log) but is
            // never headless and has no desk cap — capacity is seeded from the window.
            let rc = build_run_config(cli_theme.as_deref(), source, None, false)?;
            floating::run(rc)
        }
        Cmd::ValidatePack { pack_dir } => validate::validate_pack(&pack_dir),
        Cmd::InitPack { dest, force } => init_pack::init_pack(&dest, force),
        Cmd::Doctor => doctor::run(&logging::log_file_path()).map(|report| print!("{report}")),
        Cmd::Sources { action: None, json } => sources_cli::run_sources_list(json),
        Cmd::Sources {
            action: Some(SourcesAction::Set { ids }),
            json,
        } => sources_cli::run_sources_set(&ids, json),
        Cmd::Connect { ids, json } => sources_cli::run_change(&ids, json, |c, i| {
            sources::connect(c, i).map(|_| sources::ChangeOutcome::Connected)
        }),
        // A folded hook-removal failure is a PARTIAL failure (the flag IS
        // disconnected, but hooks remain) — surface it AND signal it via a
        // non-zero exit (run_change treats an Err op as failed), so a $?-checking
        // script isn't told a clean "disconnected".
        Cmd::Disconnect { ids, json } => {
            sources_cli::run_change(&ids, json, |c, i| match sources::disconnect(c, i)? {
                sources::DisconnectOutcome::HookRemovalFailed(e) => Err(anyhow::anyhow!(
                    "disconnected, but hook removal failed: {e}"
                )),
                _ => Ok(sources::ChangeOutcome::Disconnected),
            })
        }
        Cmd::Setup { yes } => sources_cli::run_setup(yes),
        // Packaging interfaces: emit ONLY the generated artifact to stdout (the
        // tracing subscriber above writes to stderr, so stdout stays clean for the
        // homebrew `generate_completions_from_executable` / `man` capture). Driven
        // off the same derived clap tree as `--help`, so they can't drift.
        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "pixtuoid",
                &mut std::io::stdout(),
            );
            Ok(())
        }
        Cmd::Man => {
            use clap::CommandFactory;
            clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout())?;
            Ok(())
        }
    }
}

/// Resolve the shared [`runtime::RunConfig`] from CLI args + the on-disk config —
/// the common prelude for `run` (TUI) and `floating` (window). On a non-headless
/// launch it also surfaces config warnings + the broken-install preflight to
/// stderr (visible in the launching terminal / pre-altscreen scrollback, #87/#309).
fn build_run_config(
    cli_theme: Option<&str>,
    source: SourceArgs,
    cli_max_desks: Option<usize>,
    headless: bool,
) -> Result<runtime::RunConfig> {
    let SourceArgs {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
    } = source;
    let cfg_path = config::config_path();
    let mut cfg_warnings = Vec::new();
    let cfg = config::load(&cfg_path, &mut cfg_warnings);
    // First launch ever (no `[sources]` flags yet) → the TUI plays onboarding.
    // Since 0.12.0 an empty [sources] also means NOTHING connected (the
    // v0.4–0.7 migrate inference is gone), so onboarding IS the connect path
    // for an upgrader whose config predates the flags.
    // Right after load(), a non-empty warnings Vec means the file EXISTS but is
    // malformed/unreadable — "previously configured", never a first run (the
    // onboarding apply couldn't succeed anyway: update_config refuses to
    // rewrite a malformed config). A missing file warns nothing ⇒ first run.
    let first_run = setup::is_first_run(&cfg, &cfg_path, !cfg_warnings.is_empty());
    let theme = config::resolve_theme(&cfg, cli_theme, &mut cfg_warnings)?;
    // The config seam's twin of the clap range(1..) guard: a config max-desks = 0
    // is ignored with a collected warning (eager `.or` argument on purpose — the
    // warning must fire even when the CLI flag overrides).
    let desk_cap = cli_max_desks.or(config::resolve_max_desks(&cfg, &mut cfg_warnings));
    let pack_dir = config::resolve_pack_dir(&cfg, pack_dir);
    let pets = config::resolve_pets(&cfg, &mut cfg_warnings);
    // The connected-source set the office gates sprites on: explicit `[sources]`
    // true flags only (absent = disconnected; the install-state migrate
    // inference was dropped in 0.12.0).
    let connected = config::resolve_connected(&cfg);
    if !headless {
        // Config problems must reach the user's eyes, not only the log file (#87):
        // stderr BEFORE any alternate screen / window. Headless already has a
        // stderr tracing subscriber, so re-printing there would duplicate.
        for w in &cfg_warnings {
            eprintln!("⚠ pixtuoid: {w}");
        }
        warn_broken_installs(&connected);
    }
    Ok(runtime::RunConfig {
        socket,
        projects_root,
        codex_sessions_root,
        pack_dir,
        desk_cap,
        headless,
        config_path: cfg_path,
        theme,
        pets,
        connected,
        log_path: Some(logging::log_file_path()),
        first_run,
        audio: config::resolve_audio(&cfg),
    })
}

/// Boot preflight (#309): warn (stderr) when a CONNECTED source's hooks are
/// installed but structurally BROKEN — it would render zero sprites with no other
/// hint, so the fully-passive user who never opens the Sources panel still learns.
/// Routed through the SHARED `doctor::diagnose` rollup (empty log = skip the drift
/// scan; warns on broken installs only) so this surface can't drift from the panel
/// and the CLI report. Iterates TARGETS, not REGISTERED_SOURCES: only an
/// install-bearing source can be install-BROKEN.
fn warn_broken_installs(connected: &std::collections::HashSet<String>) {
    for &t in install::TARGETS {
        if !connected.contains(t.core_source) {
            continue;
        }
        let diag = doctor::diagnose(t.core_source, "");
        if diag.is_broken() {
            let issues = diag
                .install
                .as_ref()
                .map(|v| v.issues.join("; "))
                .unwrap_or_default();
            eprintln!(
                "⚠ pixtuoid: {} hooks are installed but BROKEN: {issues} — \
                 reconnect in the Sources panel (press s)",
                t.core_source
            );
        }
    }
}
