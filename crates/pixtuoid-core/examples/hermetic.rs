//! Run each profiled agent CLI for real against a scripted model (aimock), with
//! a headless pixtuoid on the same run, and judge what pixtuoid made of it.
//! Profiles, scripts and the judgement: `pixtuoid_core::harness::hermetic`.
//!
//! `--cli pinned` (the default) installs each CLI at its registry
//! `verified_version`, the merge queue's end-to-end gate; `--cli latest` installs
//! what npm's `latest` tag points at, the nightly drift run.
//!
//! Exits 0 when every run passed, 1 on any other verdict (the report's H1 names
//! the worst), and anything else when the tier itself could not run.

#[cfg(not(unix))]
fn main() {
    eprintln!(
        "hermetic runs the recorder, which records at a Unix socket; run it on macOS or Linux"
    );
    std::process::exit(2);
}

#[cfg(unix)]
fn main() {
    match run::main() {
        Ok(all_passed) => std::process::exit(i32::from(!all_passed)),
        Err(e) => {
            eprintln!("hermetic: the tier could not run: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(unix)]
mod run {
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use pixtuoid_core::harness::hermetic::verify::{self, Label, Verdict};
    use pixtuoid_core::harness::hermetic::{
        self, DUMMY_KEY, MANIFEST, Manifest, NOT_HERMETIC, PROFILES, Package, Profile, SCENE_LOG,
        Scenario,
    };
    use pixtuoid_core::harness::{CAPTURE_RECORD, CAPTURE_WORKSPACE, CaptureRecord};

    /// How long a server gets to come up before the run is BLOCKED.
    const BOOT: Duration = Duration::from_secs(60);
    /// How long the headless scene runs on after the recorder returns, to
    /// decode the last payloads and log any breadcrumb they provoke.
    const SCENE_SETTLE: Duration = Duration::from_secs(4);
    /// The variables a CLI may inherit from the host: everything else is
    /// cleared, so an operator's provider keys can never reach a CLI whose
    /// base-URL wiring slipped and bill a real provider.
    const INHERITED: &[&str] = &[
        "PATH", "LANG", "LC_ALL", "TERM", "TMPDIR", "SHELL", "USER", "LOGNAME",
    ];

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Versions {
        Pinned,
        Latest,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Args {
        versions: Versions,
        source: Option<String>,
        scenario: Option<String>,
    }

    impl Args {
        /// A selection that names nothing runnable is an operator's typo, not
        /// a run where everything was blocked.
        fn check(&self) -> Result<(), String> {
            let Some(source) = self.source.as_deref() else {
                return Ok(());
            };
            if let Some((_, why)) = NOT_HERMETIC.iter().find(|(s, _)| *s == source) {
                return Err(format!("{source} has no hermetic profile: {why:?}"));
            }
            let p = hermetic::profile(source)
                .ok_or_else(|| format!("{source} is not a registered source"))?;
            match self.scenario.as_deref() {
                Some(sc) if !p.scenarios.iter().any(|s| s.shape.name() == sc) => {
                    Err(format!("{source} has no scenario {sc}"))
                }
                _ => Ok(()),
            }
        }

        fn selects(&self, source: &str, scenario: &str) -> bool {
            self.source.as_deref().is_none_or(|s| s == source)
                && self.scenario.as_deref().is_none_or(|s| s == scenario)
        }
    }

    fn parse(args: &[String]) -> Result<Args, String> {
        let (versions, rest) = match args {
            [flag, v, rest @ ..] if flag == "--cli" => match v.as_str() {
                "pinned" => (Versions::Pinned, rest),
                "latest" => (Versions::Latest, rest),
                _ => return Err("--cli takes pinned or latest".into()),
            },
            [flag] if flag == "--cli" => return Err("--cli takes pinned or latest".into()),
            rest => (Versions::Pinned, rest),
        };
        match rest {
            [] | [_] | [_, _] => Ok(Args {
                versions,
                source: rest.first().cloned(),
                scenario: rest.get(1).cloned(),
            }),
            _ => Err(format!("unexpected {:?}", &rest[2..])),
        }
    }

    /// Whether every selected run passed.
    pub(crate) fn main() -> std::io::Result<bool> {
        let args = match parse(&std::env::args().skip(1).collect::<Vec<_>>()) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}\nusage: hermetic [--cli pinned|latest] [<source> [<scenario>]]");
                std::process::exit(2);
            }
        };
        if let Err(e) = args.check() {
            eprintln!("{e}");
            std::process::exit(2);
        }
        let tier = tier_dir()?;
        let runs = tier.join("runs");
        match std::fs::remove_dir_all(&runs) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::create_dir_all(&runs)?;
        let ctx = Ctx {
            built: Built::find()?,
            aimock: Aimock::install(&tier)?,
            tier,
            versions: args.versions,
        };

        let mut judged = Vec::new();
        for p in PROFILES {
            for scenario in p.scenarios {
                let name = scenario.shape.name();
                if !args.selects(p.source, name) {
                    continue;
                }
                let dir = runs.join(p.source).join(name);
                std::fs::create_dir_all(&dir)?;
                let key = format!("{}/{name}", p.source);
                println!("== {key}");
                let (cli, mut verdict) = match run_one(&ctx, p, scenario, &dir) {
                    Ok(manifest) => (Some(manifest.capture.version), verify::verify(&dir)?),
                    Err(e) => (
                        None,
                        Verdict {
                            label: Label::Blocked,
                            findings: vec![(Label::Blocked, e.to_string())],
                        },
                    ),
                };
                // A merge gate never passes on "nothing ran".
                if verdict.label == Label::Blocked && args.versions == Versions::Pinned {
                    verdict.label = Label::Fail;
                }
                println!("   {:?}", verdict.label);
                judged.push(Judged { key, cli, verdict });
            }
        }
        let report = report(args.versions, &judged);
        std::fs::write(runs.join(REPORT), &report)?;
        println!("\n{report}");
        Ok(judged.iter().all(|j| j.verdict.label == Label::Pass))
    }

    /// The run's report, beside the runs; its H1 is the tracking issue's title.
    const REPORT: &str = "report.md";

    struct Judged {
        key: String,
        /// The CLI's `--version`, when it ran.
        cli: Option<String>,
        verdict: Verdict,
    }

    fn report(versions: Versions, judged: &[Judged]) -> String {
        let worst = judged
            .iter()
            .map(|j| j.verdict.label)
            .max()
            .unwrap_or(Label::Blocked);
        let which = match versions {
            Versions::Pinned => "verified",
            Versions::Latest => "latest",
        };
        let mut out = format!("# hermetic, {which} CLIs: {worst:?}\n");
        for j in judged {
            let cli = j.cli.as_deref().unwrap_or("did not run");
            out += &format!("\n## {} ({cli}): {:?}\n", j.key, j.verdict.label);
            for (label, finding) in &j.verdict.findings {
                out += &format!("- {label:?}: {finding}\n");
            }
        }
        out += "\nNot covered:";
        for (source, absence) in NOT_HERMETIC {
            out += &format!(" {source} ({absence:?})");
        }
        out + "\n"
    }

    /// Where installs and runs live: `hermetic/` in the target dir this example
    /// was built into, which holds `<profile>/examples/hermetic`.
    fn tier_dir() -> std::io::Result<PathBuf> {
        let me = std::env::current_exe()?;
        me.ancestors()
            .nth(3)
            .map(|target| target.join("hermetic"))
            .ok_or_else(|| std::io::Error::other("an example sits three below its target dir"))
    }

    /// The build under test, beside this example in its profile's directory:
    /// the binary, the shim and the recorder. Never a `pixtuoid` off PATH: a
    /// released one would pass with none of the change under test in it.
    struct Built {
        pixtuoid: PathBuf,
        shim: PathBuf,
        recorder: PathBuf,
    }

    impl Built {
        fn find() -> std::io::Result<Self> {
            let me = std::env::current_exe()?;
            let profile_dir = me
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| std::io::Error::other("an example sits two below its target dir"))?;
            let found = Self {
                pixtuoid: profile_dir.join("pixtuoid"),
                shim: profile_dir.join("pixtuoid-hook"),
                recorder: me.with_file_name("capture_fixture"),
            };
            for p in [&found.pixtuoid, &found.shim, &found.recorder] {
                if !p.is_file() {
                    return Err(std::io::Error::other(format!(
                        "{} is not built — `just hermetic` builds it",
                        p.display()
                    )));
                }
            }
            Ok(found)
        }
    }

    /// The scripted model, installed from the committed lockfile, scripts off.
    struct Aimock {
        bin: PathBuf,
        version: String,
    }

    impl Aimock {
        fn install(tier: &Path) -> std::io::Result<Self> {
            let src = hermetic::scripts_dir().join("aimock");
            let lock: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(src.join("package-lock.json"))?)?;
            let version = lock["packages"]["node_modules/@copilotkit/aimock"]["version"]
                .as_str()
                .ok_or_else(|| std::io::Error::other("the aimock lockfile pins no aimock"))?
                .to_string();
            // `npm ci` every run: it installs exactly the lock, and aimock alone
            // installs in well under a second.
            let dir = tier.join("aimock");
            std::fs::create_dir_all(&dir)?;
            for f in ["package.json", "package-lock.json"] {
                std::fs::copy(src.join(f), dir.join(f))?;
            }
            npm(&dir, &["ci", "--ignore-scripts"])?;
            Ok(Self {
                bin: dir.join("node_modules/.bin/llmock"),
                version,
            })
        }
    }

    /// Marks a finished install; written only after npm succeeds.
    const INSTALLED: &str = ".hermetic-installed";

    /// `install` into a fresh `dir` unless an earlier one finished there: an
    /// interrupted install, or one whose scripts npm blocked, leaves a binary
    /// stub behind, so the binary's presence proves nothing.
    fn installed(
        dir: &Path,
        install: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        if dir.join(INSTALLED).is_file() {
            return Ok(());
        }
        match std::fs::remove_dir_all(dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::create_dir_all(dir)?;
        install(dir)?;
        std::fs::write(dir.join(INSTALLED), "")
    }

    fn npm(dir: &Path, args: &[&str]) -> std::io::Result<()> {
        let status = Command::new("npm")
            .args(args)
            .args(["--no-audit", "--no-fund"])
            .current_dir(dir)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "npm {args:?} exited {status}"
            )))
        }
    }

    /// The CLI at the version this run tests, installed into its own project the
    /// way users install it. Its own install scripts run, since they link the
    /// binary: npm 12 blocks a dependency's unless the project's `allowScripts`
    /// names it (`npm help install-scripts`), and older npm ignores the field.
    fn install_cli(tier: &Path, p: &Profile, versions: Versions) -> std::io::Result<PathBuf> {
        let Package::Npm(package) = p.package;
        let version = match versions {
            Versions::Pinned => p.descriptor().verified_version.to_string(),
            Versions::Latest => {
                let out = Command::new("npm")
                    .args(["view", package, "version"])
                    .output()?;
                if !out.status.success() {
                    return Err(std::io::Error::other(format!("npm view {package} failed")));
                }
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
        };
        let prefix = tier.join("cli").join(format!("{}-{version}", p.source));
        let pinned = format!("{package}@{version}");
        installed(&prefix, |dir| {
            let project = serde_json::json!({
                "private": true,
                "allowScripts": { &pinned: true },
            });
            std::fs::write(dir.join("package.json"), project.to_string())?;
            npm(dir, &["install", &pinned])
        })?;
        Ok(prefix.join("node_modules/.bin"))
    }

    /// One sandbox: every home a CLI or pixtuoid resolves points inside it, so
    /// nothing reads or writes the operator's.
    struct Sandbox {
        dir: tempfile::TempDir,
        env: Vec<(String, OsString)>,
        sock: PathBuf,
    }

    impl Sandbox {
        fn new(p: &Profile, built: &Built, cli_bin: &Path, port: u16) -> std::io::Result<Self> {
            let dir = tempfile::tempdir()?;
            let at = |rel: &str| -> std::io::Result<PathBuf> {
                let d = dir.path().join(rel);
                std::fs::create_dir_all(&d)?;
                Ok(d)
            };
            let bin = at("bin")?;
            std::os::unix::fs::symlink(&built.shim, bin.join("pixtuoid-hook"))?;
            let mut env: Vec<(String, OsString)> = INHERITED
                .iter()
                .filter(|k| **k != "PATH")
                .filter_map(|k| std::env::var_os(k).map(|v| ((*k).to_string(), v)))
                .collect();
            let host_path = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                [bin, cli_bin.to_path_buf()]
                    .into_iter()
                    .chain(std::env::split_paths(&host_path)),
            )
            .map_err(std::io::Error::other)?;
            env.push(("PATH".into(), path));
            env.push(("HOME".into(), at("home")?.into()));
            for (var, rel) in [
                ("XDG_CONFIG_HOME", "xdg/config"),
                ("XDG_DATA_HOME", "xdg/data"),
                ("XDG_STATE_HOME", "xdg/state"),
                ("XDG_CACHE_HOME", "xdg/cache"),
            ] {
                env.push((var.into(), at(rel)?.into()));
            }
            let homes = p
                .descriptor()
                .home_env
                .into_iter()
                .chain(p.homes.iter().copied());
            for var in homes {
                env.push((var.into(), at(&format!("homes/{var}"))?.into()));
            }
            let expand = |t: &str| {
                t.replace("{port}", &port.to_string())
                    .replace("{key}", DUMMY_KEY)
            };
            for (k, v) in p.env {
                env.push(((*k).to_string(), expand(v).into()));
            }
            let sandbox = Self {
                sock: dir.path().join("pix.sock"),
                dir,
                env,
            };
            for c in p.config {
                let home = sandbox.var(c.home).ok_or_else(|| {
                    std::io::Error::other(format!(
                        "{}: config home {} is not sandboxed",
                        p.source, c.home
                    ))
                })?;
                let file = home.join(c.path);
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&file, expand(c.template))?;
            }
            Ok(sandbox)
        }

        fn var(&self, key: &str) -> Option<PathBuf> {
            self.env
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| PathBuf::from(v))
        }

        /// A child of the run. Its stdin is empty: `opencode run` reads a non-TTY
        /// stdin to its end (`packages/cli/src/run/run.ts:76`), so an inherited
        /// one that never closes hangs the run.
        fn command(&self, program: &Path) -> Command {
            let mut c = Command::new(program);
            c.env_clear()
                .envs(self.env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null());
            c
        }
    }

    /// A child killed when the run ends, however it ends.
    struct Reaped(Child);

    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// What every run in one invocation shares.
    struct Ctx {
        tier: PathBuf,
        built: Built,
        aimock: Aimock,
        versions: Versions,
    }

    fn run_one(
        ctx: &Ctx,
        p: &Profile,
        scenario: &Scenario,
        dir: &Path,
    ) -> std::io::Result<Manifest> {
        let Ctx {
            tier,
            built,
            aimock,
            versions,
        } = ctx;
        let cli_bin = install_cli(tier, p, *versions)?;
        let port = free_port()?;
        let sandbox = Sandbox::new(p, built, &cli_bin, port)?;

        let connect = sandbox
            .command(&built.pixtuoid)
            .args(["connect", p.source, "--json"])
            .output()?;
        std::fs::write(dir.join("connect.log"), &connect.stdout)?;
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&connect.stdout).unwrap_or_default();
        if !rows.iter().any(|r| r["outcome"] == "connected") {
            return Err(std::io::Error::other(format!(
                "pixtuoid connect {} did not connect: {}",
                p.source,
                String::from_utf8_lossy(&connect.stderr).trim()
            )));
        }

        let script = render_script(p, scenario, sandbox.dir.path())?;
        let model = Reaped(
            Command::new(&aimock.bin)
                .stdin(Stdio::null())
                .args(["-p", &port.to_string(), "-f"])
                .arg(&script)
                .stdout(log(dir, "aimock.log")?)
                .stderr(log(dir, "aimock.log")?)
                .spawn()?,
        );
        wait_until(|| http_get(port, "/v1/models").is_ok(), "aimock")?;

        let scene = Reaped(
            sandbox
                .command(&built.pixtuoid)
                .args(["run", "--headless", "--log-level", "warn", "--socket"])
                .arg(&sandbox.sock)
                // Its log colors a file too; the report quotes its lines.
                .env("NO_COLOR", "1")
                .stdout(log(dir, SCENE_LOG)?)
                .stderr(log(dir, SCENE_LOG)?)
                .spawn()?,
        );
        wait_until(|| sandbox.sock.exists(), "the headless pixtuoid's socket")?;

        let recorder = sandbox
            .command(&built.recorder)
            .arg("--out")
            .arg(dir)
            .arg("--tee")
            .arg(&sandbox.sock)
            .args([p.source, scenario.shape.name()])
            .args(p.invocation)
            .stdout(log(dir, "recorder.log")?)
            .stderr(log(dir, "recorder.log")?)
            .status()?;
        std::thread::sleep(SCENE_SETTLE);
        drop(scene);

        let journal: serde_json::Value =
            serde_json::from_str(&http_get(port, "/__aimock/journal")?)?;
        drop(model);
        std::fs::write(
            dir.join("aimock-journal.json"),
            serde_json::to_string_pretty(&journal)?,
        )?;
        let entries = journal.as_array().cloned().unwrap_or_default();
        let unmatched = entries
            .iter()
            .filter(|e| e["response"]["fixture"].is_null())
            .count();

        // The recorder writes its record whenever the CLI ran, so a missing
        // one means the CLI never did.
        let capture: CaptureRecord = match std::fs::read_to_string(dir.join(CAPTURE_RECORD)) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(e) => {
                return Err(std::io::Error::other(format!(
                    "the recorder exited {recorder} with no {CAPTURE_RECORD}: {e}"
                )));
            }
        };
        let manifest = Manifest {
            source: p.source.into(),
            scenario: scenario.shape.name().into(),
            capture,
            aimock: aimock.version.clone(),
            requests: entries.len(),
            unmatched,
        };
        std::fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&manifest)?)?;
        Ok(manifest)
    }

    /// The scenario's script with `{workspace}` expanded: a CLI that takes an
    /// absolute path gets the one the recorder runs it in.
    fn render_script(p: &Profile, scenario: &Scenario, into: &Path) -> std::io::Result<PathBuf> {
        let text = std::fs::read_to_string(scenario.script(p.source))?;
        let workspace = Path::new(CAPTURE_WORKSPACE);
        let workspace =
            std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let out = into.join("script.json");
        std::fs::write(
            &out,
            text.replace("{workspace}", &workspace.to_string_lossy()),
        )?;
        Ok(out)
    }

    /// One child's stdout and stderr, interleaved in one file of the run.
    fn log(dir: &Path, name: &str) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
    }

    fn free_port() -> std::io::Result<u16> {
        Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
    }

    fn wait_until(mut ready: impl FnMut() -> bool, what: &str) -> std::io::Result<()> {
        let started = Instant::now();
        while started.elapsed() < BOOT {
            if ready() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(std::io::Error::other(format!("{what} never came up")))
    }

    /// A bare HTTP/1.0 GET to aimock on loopback: the core crate carries no
    /// HTTP client, and this is the whole of the protocol the tier needs.
    fn http_get(port: u16, path: &str) -> std::io::Result<String> {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        write!(s, "GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")?;
        let mut raw = String::new();
        s.read_to_string(&mut raw)?;
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
            return Err(std::io::Error::other(format!(
                "GET {path}: {}",
                head.lines().next().unwrap_or("")
            )));
        }
        Ok(body.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn args(v: &[&str]) -> Vec<String> {
            v.iter().map(|s| (*s).to_string()).collect()
        }

        #[test]
        fn the_versions_flag_leads_and_the_rest_selects() {
            let a = parse(&args(&["--cli", "latest", "codex", "tool-run"])).expect("parse");
            assert_eq!(a.versions, Versions::Latest);
            assert!(a.selects("codex", "tool-run"));
            assert!(!a.selects("codex", "other"));
            assert!(!a.selects("opencode", "tool-run"));
            let a = parse(&[]).expect("parse");
            assert_eq!(a.versions, Versions::Pinned, "the merge gate's default");
            assert!(a.selects("anything", "at-all"));
            assert!(parse(&args(&["--cli"])).is_err());
            assert!(parse(&args(&["--cli", "newest"])).is_err());
            assert!(parse(&args(&["codex", "tool-run", "extra"])).is_err());
        }

        #[test]
        fn the_reports_title_is_the_worst_verdict() {
            let judged = [
                Judged {
                    key: "codex/tool-run".into(),
                    cli: Some("codex-cli 0.155.1".into()),
                    verdict: Verdict {
                        label: Label::Pass,
                        findings: vec![],
                    },
                },
                Judged {
                    key: "opencode/tool-run".into(),
                    cli: None,
                    verdict: Verdict {
                        label: Label::Blocked,
                        findings: vec![(Label::Blocked, "npm view failed".into())],
                    },
                },
            ];
            let r = report(Versions::Latest, &judged);
            assert_eq!(r.lines().next(), Some("# hermetic, latest CLIs: Blocked"));
            assert!(r.contains(
                "## opencode/tool-run (did not run): Blocked\n- Blocked: npm view failed"
            ));
            assert!(r.contains("## codex/tool-run (codex-cli 0.155.1): Pass"));
        }

        #[test]
        fn a_selection_naming_nothing_runnable_is_refused() {
            let check = |v: &[&str]| parse(&args(v)).expect("parse").check();
            assert!(check(&["codex"]).is_ok());
            assert!(check(&["codex", "tool-run"]).is_ok());
            assert!(check(&["codex", "no-such-scenario"]).is_err());
            assert!(check(&["no-such-source"]).is_err());
            let (absent, _) = NOT_HERMETIC[0];
            assert!(check(&[absent]).is_err(), "an unprofiled source says why");
        }
    }
}
