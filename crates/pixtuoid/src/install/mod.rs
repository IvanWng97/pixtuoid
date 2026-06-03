pub mod claude;
pub mod codex;
pub mod io;
pub mod target;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use target::{Target, BACKUP_SUFFIX};

const NO_CLIS_MSG: &str = "no supported CLIs detected; pass --target claude|codex|all";

/// Filter a detection table to the targets that are present, dropping the flag.
fn present_targets(rows: &[(&'static Target, bool)]) -> Vec<&'static Target> {
    rows.iter().filter(|(_, p)| *p).map(|(t, _)| *t).collect()
}

pub struct InstallArgs {
    pub hook_path: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub struct UninstallArgs {
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub enum Plan {
    Targets(Vec<&'static Target>),
    NothingDetected,
    Conflict(String),
}

/// Pure policy: decide which targets to act on. No filesystem, no stdin.
/// `present` is the injected detection result; `explicit_config` is whether
/// `--config` was passed (only valid for a single target).
pub fn plan_targets(
    requested: Option<&str>,
    explicit_config: bool,
    present: &[(&'static Target, bool)],
    is_tty: bool,
) -> Plan {
    match requested {
        Some("all") => {
            if explicit_config {
                return Plan::Conflict(
                    "--config applies to a single target; use --target claude|codex".into(),
                );
            }
            let chosen = present_targets(present);
            if chosen.is_empty() {
                Plan::NothingDetected
            } else {
                Plan::Targets(chosen)
            }
        }
        Some(name) => match target::by_name(name) {
            Some(t) => Plan::Targets(vec![t]),
            None => Plan::Conflict(format!("unknown target: {name}")),
        },
        None => {
            // `--config`/`--settings` without `--target` is the legacy Claude-only
            // contract (pre-multi-CLI scripts). The supplied path IS the target
            // selection signal — `$HOME` detection is meaningless here — so default
            // to Claude rather than coupling the explicit path to ambient detection.
            if explicit_config {
                return match target::by_name("claude") {
                    Some(t) => Plan::Targets(vec![t]),
                    None => Plan::Conflict("claude target not registered".into()),
                };
            }
            let detected = present_targets(present);
            match detected.len() {
                0 => Plan::NothingDetected,
                1 => Plan::Targets(detected), // TTY or not: a single detected target is safe
                _ if is_tty => Plan::Targets(detected), // caller confirms interactively
                _ => {
                    Plan::Conflict("multiple CLIs detected; pass --target claude|codex|all".into())
                }
            }
        }
    }
}

/// Parse a confirm answer: empty/Enter or y/yes → true; anything else → false.
fn parse_confirm(answer: &str) -> bool {
    let a = answer.trim().to_ascii_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    parse_confirm(&line)
}

fn detection() -> Vec<(&'static Target, bool)> {
    target::TARGETS
        .iter()
        .map(|t| (*t, target::is_present(t)))
        .collect()
}

/// Whether `t` is a candidate for the interactive uninstall picker. A dry-run
/// uninstall that would change the parsed doc means managed hooks are present.
/// An absent/empty config is excluded; a config that is present but unreadable
/// or unparseable is INCLUDED (true) so a hooks-bearing-but-malformed config
/// still appears and the user sees the real error from `run_uninstall`, rather
/// than a misleading "nothing to remove".
fn has_hooks(t: &'static Target) -> bool {
    let path = (t.default_config_path)();
    match io::read_config(&path) {
        Ok(c) if c.trim().is_empty() => false,
        Ok(c) => (t.merge_uninstall)(&c).map(|o| o.changed).unwrap_or(true),
        Err(_) => true,
    }
}

/// Interactive checklist of `candidates`, all pre-checked. Returns the chosen
/// targets, or `None` if the user cancelled (Esc). TTY-only — callers gate on it.
fn select_targets(
    prompt: &str,
    candidates: &[&'static Target],
) -> Result<Option<Vec<&'static Target>>> {
    let options: Vec<&str> = candidates.iter().map(|t| t.display_name).collect();
    let all: Vec<usize> = (0..options.len()).collect();
    let chosen = inquire::MultiSelect::new(prompt, options)
        .with_default(&all)
        .raw_prompt_skippable()
        .context("target selection prompt failed")?;
    // Map back by INDEX, not display label — two targets sharing a display_name
    // must not both get selected when only one is checked.
    Ok(chosen.map(|sel| sel.into_iter().map(|opt| candidates[opt.index]).collect()))
}

/// Both stdin AND stdout must be a terminal before we run an interactive prompt:
/// inquire reads keys via /dev/tty but renders to the output stream, so gating on
/// stdin alone would let `install-hooks > log` render a garbled prompt into the
/// redirected file. Output redirection ⇒ treat the run as non-interactive.
fn interactive_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// True when the run is an interactive bare invocation — no explicit `--target`
/// or `--config`, not `--yes`, on a TTY — i.e. the case the checklist serves.
fn interactive_pick(
    target: &Option<String>,
    config: &Option<PathBuf>,
    yes: bool,
    is_tty: bool,
) -> bool {
    target.is_none() && config.is_none() && !yes && is_tty
}

/// Shared interactive picker flow for install + uninstall: 0 candidates → print
/// `empty_msg`; 1 → act directly (no list to pick from); >1 → checklist, where
/// Esc/none-selected aborts. Keeps install's and uninstall's UX identical.
fn run_interactive(
    candidates: Vec<&'static Target>,
    empty_msg: &str,
    prompt: &str,
    verb: &str,
    op: impl Fn(&'static Target) -> Result<()>,
) -> Result<()> {
    let chosen = match candidates.len() {
        0 => {
            println!("{empty_msg}");
            return Ok(());
        }
        1 => candidates,
        _ => match select_targets(prompt, &candidates)? {
            Some(sel) if !sel.is_empty() => sel,
            Some(_) => {
                println!("nothing selected");
                return Ok(());
            }
            None => {
                println!("aborted");
                return Ok(());
            }
        },
    };
    run_each(&chosen, verb, op)
}

pub fn install(args: InstallArgs) -> Result<()> {
    let is_tty = interactive_terminal();

    // Interactive picker: detected CLIs as a checklist (all pre-checked) so the
    // user installs into a subset instead of always all. Explicit `--target` /
    // `--config` / `--yes` / non-interactive take the flag-driven path below.
    if interactive_pick(&args.target, &args.config, args.yes, is_tty) {
        let detected = present_targets(&detection());
        return run_interactive(
            detected,
            NO_CLIS_MSG,
            "Install pixtuoid hooks into",
            "install",
            |t| run_install(t, None, args.hook_path.clone()),
        );
    }

    // Flag-driven path (explicit/--yes/non-interactive). Bare interactive
    // multi-target is handled by the picker above, so install never needs a
    // text confirm here — act directly.
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &detection(),
        is_tty,
    );
    let targets = resolve_plan(plan)?;
    run_each(&targets, "install", |t| {
        run_install(t, args.config.clone(), args.hook_path.clone())
    })
}

pub fn uninstall(args: UninstallArgs) -> Result<()> {
    let is_tty = interactive_terminal();

    // Interactive picker: list only CLIs that ACTUALLY have pixtuoid hooks.
    if interactive_pick(&args.target, &args.config, args.yes, is_tty) {
        let installed: Vec<&'static Target> = target::TARGETS
            .iter()
            .copied()
            .filter(|t| has_hooks(t))
            .collect();
        return run_interactive(
            installed,
            "no pixtuoid hooks found to remove",
            "Remove pixtuoid hooks from",
            "uninstall",
            |t| run_uninstall(t, None),
        );
    }

    // Flag-driven path. Destructive: confirm an explicit multi-target run (e.g.
    // `--target all`) on a terminal — it rewrites configs + deletes backups.
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &detection(),
        is_tty,
    );
    let targets = resolve_plan(plan)?;
    if needs_confirm(targets.len(), args.yes, is_tty)
        && !confirm_targets("remove pixtuoid hooks from", &targets)
    {
        println!("aborted");
        return Ok(());
    }
    run_each(&targets, "uninstall", |t| {
        run_uninstall(t, args.config.clone())
    })
}

fn resolve_plan(plan: Plan) -> Result<Vec<&'static Target>> {
    match plan {
        Plan::Targets(t) => Ok(t),
        Plan::NothingDetected => {
            println!("{NO_CLIS_MSG}");
            Ok(vec![])
        }
        Plan::Conflict(msg) => bail!(msg),
    }
}

/// Confirm a destructive multi-target run before acting. Only uninstall calls
/// this (it rewrites configs + deletes backups); install's interactive case is
/// handled by the picker, and its flag path never confirms. Skipped by `--yes`,
/// a non-interactive terminal, or a single target.
fn needs_confirm(n: usize, yes: bool, is_tty: bool) -> bool {
    !yes && is_tty && n > 1
}

fn confirm_targets(verb: &str, targets: &[&'static Target]) -> bool {
    let names: Vec<_> = targets.iter().map(|t| t.display_name).collect();
    confirm(&format!("{verb} {}?", names.join(" + ")))
}

/// Run `op` for each target independently. A failure on one target is reported
/// but does NOT abort the others — otherwise a malformed second config (e.g.
/// `--target all` with bad TOML) could hide that the first target was already
/// modified. Returns Err iff any target failed.
fn run_each(
    targets: &[&'static Target],
    verb: &str,
    op: impl Fn(&'static Target) -> Result<()>,
) -> Result<()> {
    let mut failed = 0usize;
    for &t in targets {
        if let Err(e) = op(t) {
            eprintln!("error: {verb} for {} failed: {e:#}", t.display_name);
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} of {} target(s) failed", targets.len());
    }
    Ok(())
}

/// Resolve the hook binary for a target. An explicit `--hook-path` always wins.
/// Otherwise `locate` tries to find `pixtuoid-hook`; if that fails we only hard-error
/// for targets that EMBED the path (`needs_resolved_binary`, e.g. Codex). Targets
/// that write the bare name and rely on PATH (Claude) fall back to the bare name so
/// a fresh-machine install still succeeds — the PATH warning in `run_install` covers
/// the not-yet-on-PATH case.
fn resolve_hook_binary(
    t: &Target,
    hook_path: Option<PathBuf>,
    locate: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    if let Some(p) = hook_path {
        return Ok(p);
    }
    match locate() {
        Ok(p) => Ok(p),
        Err(e) if t.needs_resolved_binary => Err(e),
        Err(_) => Ok(PathBuf::from("pixtuoid-hook")),
    }
}

fn run_install(t: &Target, config: Option<PathBuf>, hook_path: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let binary = resolve_hook_binary(t, hook_path, io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary)?;
    let content = io::read_config(&path)?;
    let outcome = (t.merge_install)(&content, &hook_cmd)
        .with_context(|| format!("processing {}", path.display()))?;
    // The PATH check is an install-time environment check, independent of whether
    // the file content changed — always surface it (a no-op re-install on a box
    // where pixtuoid-hook isn't on PATH would otherwise warn nothing).
    if t.needs_path_warning && !io::hook_on_path() {
        println!("warn: `pixtuoid-hook` not found on PATH (checked against this shell).");
        println!("      Install it on PATH, e.g. `cargo install --path crates/pixtuoid-hook`.");
    }
    if !outcome.changed {
        println!("[{}] already up to date — {}", t.name, path.display());
        return Ok(());
    }
    let backup = io::backup_once(&path, BACKUP_SUFFIX)?;
    io::write_config_atomic(&path, &outcome.content)?;
    println!(
        "ok: installed pixtuoid hooks into {} ({})",
        path.display(),
        t.display_name
    );
    if let Some(b) = backup {
        println!(
            "backup: {} (removed automatically on uninstall-hooks)",
            b.display()
        );
    }
    if let Some(note) = t.post_install_note {
        println!("{note}");
    }
    println!(
        "→ start a new {} session for this to take effect.",
        t.restart_noun
    );
    Ok(())
}

fn run_uninstall(t: &Target, config: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let content = io::read_config(&path)?;
    let outcome =
        (t.merge_uninstall)(&content).with_context(|| format!("processing {}", path.display()))?;
    if !outcome.changed {
        // SEMANTIC no-op (covers file-absent — content == "" — and no managed
        // entries). Never rewrite the file or delete the backup here: the backup
        // is the user's only recovery path. A byte comparison here would falsely
        // fire on any hand-formatted config and destroy the backup.
        println!(
            "[{}] no pixtuoid hooks found in {} — nothing to remove",
            t.name,
            path.display()
        );
        return Ok(());
    }
    io::write_config_atomic(&path, &outcome.content)?;
    println!(
        "ok: removed pixtuoid hooks from {} ({})",
        path.display(),
        t.display_name
    );
    if let Some(b) = io::remove_backup(&path, BACKUP_SUFFIX)? {
        println!("removed backup: {}", b.display());
    }
    println!(
        "→ start a new {} session for this to take effect.",
        t.restart_noun
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::target::{MergeOutcome, Target, CLAUDE, CODEX};

    // A second fake target for "both present" rows (avoids depending on Phase 2's CODEX).
    static FAKE: Target = Target {
        name: "fake",
        display_name: "Fake",
        restart_noun: "Fake",
        default_config_path: || std::path::PathBuf::from("/nonexistent/fake"),
        hook_command: |_| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        needs_path_warning: false,
        needs_resolved_binary: false,
        post_install_note: None,
    };

    fn present(claude: bool, fake: bool) -> Vec<(&'static Target, bool)> {
        vec![(&CLAUDE, claude), (&FAKE, fake)]
    }

    #[test]
    fn resolve_hook_binary_explicit_path_wins() {
        // --hook-path always short-circuits resolution (locate is never called).
        let got = resolve_hook_binary(&CLAUDE, Some(PathBuf::from("/x/hook")), || {
            panic!("locate must not be called when --hook-path is given")
        });
        assert_eq!(got.unwrap(), PathBuf::from("/x/hook"));
    }

    #[test]
    fn resolve_hook_binary_claude_falls_back_to_bare_name_when_unresolvable() {
        // Regression: a fresh-machine `install-hooks` hard-failed when pixtuoid-hook
        // wasn't yet on PATH. Claude writes the bare name and relies on PATH, so an
        // unresolvable binary must fall back to the bare name (the PATH warning covers
        // the not-found case), NOT abort the install.
        let got = resolve_hook_binary(&CLAUDE, None, || Err(anyhow::anyhow!("could not locate")));
        assert_eq!(got.unwrap(), PathBuf::from("pixtuoid-hook"));
    }

    #[test]
    fn resolve_hook_binary_codex_errors_when_unresolvable() {
        // Codex embeds the absolute path in the command, so an unresolvable binary
        // is genuinely fatal for that target.
        let got = resolve_hook_binary(&CODEX, None, || Err(anyhow::anyhow!("could not locate")));
        assert!(got.is_err());
    }

    #[test]
    fn explicit_target_claude_ignores_detection() {
        let p = plan_targets(Some("claude"), false, &present(false, false), false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn explicit_all_with_config_is_conflict() {
        let p = plan_targets(Some("all"), true, &present(true, true), true);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_tty_returns_detected() {
        let p = plan_targets(None, false, &present(true, true), true);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 2));
    }

    #[test]
    fn no_target_non_tty_single_claude_installs_claude() {
        let p = plan_targets(None, false, &present(true, false), false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn no_target_non_tty_multiple_present_is_conflict() {
        let p = plan_targets(None, false, &present(true, true), false);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_nothing_present_is_nothing_detected() {
        let p = plan_targets(None, false, &present(false, false), false);
        assert!(matches!(p, Plan::NothingDetected));
    }

    #[test]
    fn confirm_answer_parses_default_yes() {
        assert!(parse_confirm(""));
        assert!(parse_confirm("y"));
        assert!(parse_confirm("YES"));
        assert!(!parse_confirm("n"));
        assert!(!parse_confirm("no"));
        assert!(!parse_confirm("garbage")); // anything not yes/empty → no
    }

    #[test]
    fn interactive_pick_only_on_bare_tty() {
        let none: Option<String> = None;
        let no_cfg: Option<PathBuf> = None;
        // Bare (no --target/--config), not --yes, on a TTY → show the checklist.
        assert!(interactive_pick(&none, &no_cfg, false, true));
        // Any of: non-TTY, --yes, explicit --target, or --config → flag path.
        assert!(!interactive_pick(&none, &no_cfg, false, false));
        assert!(!interactive_pick(&none, &no_cfg, true, true));
        assert!(!interactive_pick(
            &Some("claude".into()),
            &no_cfg,
            false,
            true
        ));
        assert!(!interactive_pick(
            &none,
            &Some(PathBuf::from("/x")),
            false,
            true
        ));
    }
}
