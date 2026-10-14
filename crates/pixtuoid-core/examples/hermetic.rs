//! Run each profiled agent CLI for real against a scripted model (aimock), with
//! a headless pixtuoid on the same run, and judge what pixtuoid made of it.
//! Profiles, scripts and the judgement: `pixtuoid_core::harness::hermetic`.
//!
//! `--cli pinned` (the default) installs each CLI at its registry
//! `verified_version`, the merge queue's end-to-end gate; `--cli latest` installs
//! what npm's `latest` tag points at, the nightly drift run. `--record` writes a
//! source's fixtures, and with `latest` moves its pin up to the release recorded.
//!
//! Exits 0 when every run passed, 1 on any other verdict (a run's CLI reporting
//! a release npm did not install included), and anything else when the tier
//! itself could not run or refused a recording before any CLI ran.

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
    use std::cmp::Ordering;
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use pixtuoid_core::harness::hermetic::pin;
    use pixtuoid_core::harness::hermetic::verify::{self, Label, Verdict};
    use pixtuoid_core::harness::hermetic::{
        self, DUMMY_KEY, MANIFEST, Manifest, NOT_HERMETIC, NOTE_PLACEHOLDER, PROFILES, Package,
        Profile, SCENE_LOG, Scenario, USER_PLACEHOLDER,
    };
    use pixtuoid_core::harness::{
        CAPTURE_NOTE, CAPTURE_RECORD, CAPTURE_WORKSPACE, CaptureRecord, recorder_flag,
    };
    use semver::Version;

    /// How long a server gets to come up before the run gives up.
    const BOOT: Duration = Duration::from_secs(60);
    /// How often a server that is coming up is asked whether it is ready.
    const POLL: Duration = Duration::from_millis(200);
    /// How long one read from aimock may hang.
    const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long the headless scene runs on after the recorder returns, to
    /// decode the last payloads and log any breadcrumb they provoke.
    const SCENE_SETTLE: Duration = Duration::from_secs(4);
    /// The variables a CLI may inherit from the host: everything else is
    /// cleared, so an operator's provider keys can never reach a CLI whose
    /// base-URL wiring slipped and bill a real provider.
    const INHERITED: &[&str] = &[
        "PATH", "LANG", "LC_ALL", "TERM", "TMPDIR", "SHELL", "USER", "LOGNAME",
    ];
    /// What npm and aimock inherit beyond [`INHERITED`]: npm's own config, cache
    /// and network settings. A CLI's install scripts and the npm-installed aimock
    /// are untrusted code, so no provider key reaches them either.
    const TOOL_INHERITED: &[&str] = &[
        "HOME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "NODE_EXTRA_CA_CERTS",
    ];

    /// `program` with the host env cleared to [`INHERITED`], [`TOOL_INHERITED`]
    /// and npm's `npm_config_*` settings.
    fn tool(program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        c.env_clear().stdin(Stdio::null());
        for (k, v) in std::env::vars_os() {
            let keep = k.to_str().is_some_and(|k| {
                INHERITED.contains(&k)
                    || TOOL_INHERITED.contains(&k)
                    || k.to_ascii_lowercase().starts_with("npm_config_")
            });
            if keep {
                c.env(k, v);
            }
        }
        c
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Versions {
        Pinned,
        Latest,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Args {
        versions: Versions,
        record: bool,
        source: Option<String>,
        scenario: Option<String>,
    }

    impl Args {
        /// A selection that names nothing runnable is an operator's typo, not
        /// a run where everything was blocked.
        fn check(&self) -> Result<(), String> {
            if self.record && self.versions == Versions::Latest && self.source.is_none() {
                return Err("--record with latest moves one source's pin: name it".into());
            }
            if self.record && self.scenario.is_some() {
                return Err("--record writes a whole source: its census judges every run".into());
            }
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

    fn parse(mut rest: &[String]) -> Result<Args, String> {
        let (mut versions, mut record) = (Versions::Pinned, false);
        loop {
            match rest {
                [flag, v, tail @ ..] if flag == "--cli" => {
                    versions = match v.as_str() {
                        "pinned" => Versions::Pinned,
                        "latest" => Versions::Latest,
                        _ => return Err("--cli takes pinned or latest".into()),
                    };
                    rest = tail;
                }
                [flag] if flag == "--cli" => return Err("--cli takes pinned or latest".into()),
                [flag, tail @ ..] if flag == "--record" => {
                    record = true;
                    rest = tail;
                }
                _ => break,
            }
        }
        match rest {
            [] | [_] | [_, _] => Ok(Args {
                versions,
                record,
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
                eprintln!(
                    "{e}\nusage: hermetic [--cli pinned|latest] [--record] [<source> [<scenario>]]"
                );
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
        };

        let mut entries = Vec::new();
        for p in PROFILES {
            entries.extend(run_source(&ctx, &args, p, &runs)?);
        }
        let report = report(args.versions, &entries);
        std::fs::write(runs.join(REPORT), &report)?;
        println!("\n{report}");
        Ok(entries.iter().all(|e| e.verdict.label() == Label::Pass))
    }

    /// `p`'s selected runs, then, once every scenario ran, its census and the
    /// recording `args` asks for.
    fn run_source(ctx: &Ctx, args: &Args, p: &Profile, runs: &Path) -> std::io::Result<Vec<Entry>> {
        let selected: Vec<&Scenario> = p
            .scenarios
            .iter()
            .filter(|s| args.selects(p.source, s.shape.name()))
            .collect();
        let mut entries = Vec::new();
        if selected.is_empty() {
            return Ok(entries);
        }
        // Once per source, so every scenario runs, and records, one release.
        let installed = resolve(p, args.versions);
        let registry = match (&installed, args.record) {
            (Ok(v), true) => {
                let pin = pin::pinned(p.descriptor()).map_err(std::io::Error::other)?;
                let src = std::fs::read_to_string(pin::registry_path())?;
                repinned_registry(&src, p.source, &pin, v).map_err(std::io::Error::other)?
            }
            _ => None,
        };
        let (mut judged, mut banners, mut passed) = (Vec::new(), Vec::new(), true);
        for scenario in selected {
            let dir = run_dir(runs, p, scenario);
            std::fs::create_dir_all(&dir)?;
            let key = format!("{}/{}", p.source, scenario.shape.name());
            println!("== {key}");
            let ran = match &installed {
                Ok(v) => run_one(ctx, p, scenario, v, &dir),
                Err(e) => Err(std::io::Error::other(e.clone())),
            };
            let (banner, verdict) = match ran {
                Ok(manifest) => (Some(manifest.capture.version), verify::verify(&dir)?),
                Err(e) => (None, Verdict::new(vec![(Label::Blocked, e.to_string())])),
            };
            if verdict.judged() {
                judged.push(dir);
            }
            passed &= verdict.label() == Label::Pass;
            banners.extend(banner.clone());
            entries.push(Entry::new(key, banner, verdict));
        }
        if !judges_source(&judged, p.scenarios.len()) {
            return Ok(entries);
        }
        let key = format!("{} (every scenario)", p.source);
        println!("== {key}");
        let verdict = verify::verify_source(p, &judged)?;
        passed &= verdict.label() == Label::Pass;
        entries.push(Entry::new(key, None, verdict));
        if let (Ok(installed), true, true) = (&installed, args.record, passed) {
            match pin::every_run_reported(installed, p.descriptor().version_hyphen, &banners) {
                Ok(()) => {
                    record(ctx, p, runs)?;
                    if let Some(text) = registry {
                        std::fs::write(pin::registry_path(), text)?;
                    }
                }
                Err(why) => entries.push(Entry::new(
                    format!("{} (recording)", p.source),
                    None,
                    Verdict::new(vec![(Label::Fail, why)]),
                )),
            }
        }
        Ok(entries)
    }

    /// The registry a recording of `source` at `installed` leaves: `None` while
    /// the pin stays. Worked out before any CLI runs, so a refusal writes nothing.
    fn repinned_registry(
        registry: &str,
        source: &str,
        pin: &Version,
        installed: &Version,
    ) -> Result<Option<String>, String> {
        match installed.cmp_precedence(pin) {
            Ordering::Less => Err(format!(
                "{source}: {installed} is below its pin {pin}, so nothing is recorded"
            )),
            Ordering::Equal => Ok(None),
            Ordering::Greater => pin::repin(registry, source, installed).map(Some),
        }
    }

    fn run_dir(runs: &Path, p: &Profile, scenario: &Scenario) -> PathBuf {
        runs.join(p.source).join(scenario.shape.name())
    }

    /// Each of `p`'s runs under `runs`, written into its scenario's fixture by
    /// the recorder: called once every run and the census passed.
    fn record(ctx: &Ctx, p: &Profile, runs: &Path) -> std::io::Result<()> {
        for scenario in p.scenarios {
            let status = Command::new(&ctx.built.recorder)
                .arg(recorder_flag::PROMOTE)
                .arg(run_dir(runs, p, scenario))
                .status()?;
            if !status.success() {
                return Err(std::io::Error::other(format!(
                    "recording {}/{} exited {status}",
                    p.source,
                    scenario.shape.name()
                )));
            }
        }
        Ok(())
    }

    /// Whether `verify_source` takes a source's `judged` runs: only when they are
    /// every one of its `scenarios`, since a subset, one selected or one Blocked,
    /// fails on a field only another carries.
    fn judges_source(judged: &[PathBuf], scenarios: usize) -> bool {
        judged.len() == scenarios
    }

    /// The run's report, beside the runs; its H1 is the tracking issue's title.
    const REPORT: &str = "report.md";

    /// One section of the report: a scenario's run, or a source's runs together.
    struct Entry {
        key: String,
        /// The CLI's `--version`, for a scenario's run.
        version: Option<String>,
        verdict: Verdict,
    }

    impl Entry {
        fn new(key: String, version: Option<String>, verdict: Verdict) -> Self {
            println!("   {:?}", verdict.label());
            Self {
                key,
                version,
                verdict,
            }
        }
    }

    fn report(versions: Versions, entries: &[Entry]) -> String {
        let which = match versions {
            Versions::Pinned => "verified",
            Versions::Latest => "latest",
        };
        let labels = || entries.iter().map(|e| e.verdict.label());
        let title = match labels().max() {
            Some(worst) if labels().any(|l| l != Label::Blocked) => format!("{worst:?}"),
            _ => "NOTHING RAN".to_owned(),
        };
        let mut out = format!("# hermetic, {which} CLIs: {title}\n");
        for e in entries {
            let version = e
                .version
                .as_deref()
                .map_or(String::new(), |v| format!(" ({v})"));
            out += &format!("\n## {}{version}: {:?}\n", e.key, e.verdict.label());
            for (label, finding) in e.verdict.findings() {
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
            // `npm ci` every run: it installs exactly the lock.
            let dir = tier.join("aimock");
            std::fs::create_dir_all(&dir)?;
            for f in ["package.json", "package-lock.json"] {
                std::fs::copy(src.join(f), dir.join(f))?;
            }
            succeed(npm(&dir, &["ci", "--ignore-scripts"]))?;
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
    fn install_once(
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

    fn npm(dir: &Path, args: &[&str]) -> Command {
        let mut c = tool("npm");
        c.args(args)
            .args(["--no-audit", "--no-fund"])
            .current_dir(dir);
        c
    }

    /// Run `c` to success. Its error names the program and arguments alone: a
    /// `Command`'s `Debug` prints its environment, npm's credentials included.
    fn succeed(mut c: Command) -> std::io::Result<()> {
        let status = c.status()?;
        if status.success() {
            return Ok(());
        }
        let args: Vec<_> = c.get_args().collect();
        Err(std::io::Error::other(format!(
            "{} {args:?} exited {status}",
            c.get_program().to_string_lossy()
        )))
    }

    /// `npm install <pinned>` into `dir`. An install script writes where its
    /// CLI's home variables point: grok's extracts the binary into
    /// `$GROK_HOME/bin` and repoints the `grok` there
    /// (`crates/codegen/xai-grok-pager/npm/grok/bin/postinstall.js:26-27`), so
    /// each points into the install, never at the operator's.
    fn npm_install(p: &Profile, dir: &Path, pinned: &str) -> Command {
        let mut c = npm(dir, &["install", pinned]);
        for var in p.home_vars() {
            c.env(var, dir.join("homes").join(var));
        }
        c
    }

    /// The release `versions` names for `p`.
    fn resolve(p: &Profile, versions: Versions) -> Result<Version, String> {
        let Package::Npm(package) = p.package;
        match versions {
            Versions::Pinned => pin::pinned(p.descriptor()),
            Versions::Latest => {
                // A bare name reads npm's `tag` config, which an operator's
                // `.npmrc` can point at another dist-tag (npm/cli lib/commands/view.js).
                let spec = format!("{package}@latest");
                let out = tool("npm")
                    .args(["view", &spec, "version"])
                    .output()
                    .map_err(|e| format!("npm view {spec}: {e}"))?;
                npm_release(&spec, out.status.success(), &out.stdout, &out.stderr)
            }
        }
    }

    /// How much of npm's error a report, a public issue in the nightly, carries:
    /// its code and summary lines, cut.
    const NPM_ERROR_LINES: usize = 2;
    const NPM_ERROR_CHARS: usize = 200;

    /// The release `npm view <spec> version` answered, strict, or why there is
    /// none. An unresolvable tag exits 0 with nothing on stdout.
    fn npm_release(spec: &str, ok: bool, stdout: &[u8], stderr: &[u8]) -> Result<Version, String> {
        if !ok {
            let why: Vec<String> = String::from_utf8_lossy(stderr)
                .lines()
                .filter(|l| l.starts_with("npm error"))
                .take(NPM_ERROR_LINES)
                .map(|l| l.chars().take(NPM_ERROR_CHARS).collect())
                .collect();
            return Err(format!("npm view {spec} failed: {}", why.join("; ")));
        }
        let v = String::from_utf8_lossy(stdout).trim().to_string();
        if v.is_empty() {
            return Err(format!("npm gave no version for {spec}"));
        }
        Version::parse(&v).map_err(|_| format!("npm view {spec} gave {v:?}, not a release"))
    }

    /// The CLI at the version this run tests, installed into its own project the
    /// way users install it. Its own install scripts run, since they link the
    /// binary: npm 12 blocks a dependency's unless the project's `allowScripts`
    /// names it (`npm help install-scripts`), and older npm ignores the field.
    fn install_cli(tier: &Path, p: &Profile, version: &Version) -> std::io::Result<PathBuf> {
        let Package::Npm(package) = p.package;
        let prefix = tier.join("cli").join(format!("{}-{version}", p.source));
        let pinned = format!("{package}@{version}");
        install_once(&prefix, |dir| {
            let project = serde_json::json!({
                "private": true,
                "allowScripts": { &pinned: true },
            });
            std::fs::write(dir.join("package.json"), project.to_string())?;
            succeed(npm_install(p, dir, &pinned))
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
            for var in p.home_vars() {
                env.push((var.into(), at(&format!("homes/{var}"))?.into()));
            }
            let expand = |t: &str| {
                t.replace("{port}", &port.to_string())
                    .replace("{key}", DUMMY_KEY)
            };
            // Every child gets the profile's env, `connect`'s spawned CLI included;
            // the CLI under test also gets it as `--env` (`Sandbox::cli_env`).
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

        /// The profile's pairs as this sandbox holds them, for the recorder to
        /// set on the CLI as `--env`.
        fn cli_env(&self, p: &Profile) -> Vec<String> {
            p.env
                .iter()
                .filter_map(|(k, _)| {
                    self.var_os(k)
                        .map(|v| format!("{k}={}", v.to_string_lossy()))
                })
                .collect()
        }

        fn var(&self, key: &str) -> Option<PathBuf> {
            self.var_os(key).map(PathBuf::from)
        }

        fn var_os(&self, key: &str) -> Option<&OsString> {
            self.env
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
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
    }

    fn run_one(
        ctx: &Ctx,
        p: &Profile,
        scenario: &Scenario,
        version: &Version,
        dir: &Path,
    ) -> std::io::Result<Manifest> {
        let Ctx {
            tier,
            built,
            aimock,
        } = ctx;
        if let Some(found) = outside_config(
            p.system_config,
            p.ancestor_config,
            Path::new(CAPTURE_WORKSPACE),
            std::env::var_os("USER")
                .as_deref()
                .and_then(std::ffi::OsStr::to_str),
        ) {
            return Err(std::io::Error::other(format!(
                "{} configures {} from outside the sandbox",
                found.display(),
                p.source
            )));
        }
        let cli_bin = install_cli(tier, p, version)?;
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
            tool(&aimock.bin)
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
                // Its log colors a file too, and the scene log is read as text.
                .env("NO_COLOR", "1")
                .stdout(log(dir, SCENE_LOG)?)
                .stderr(log(dir, SCENE_LOG)?)
                .spawn()?,
        );
        wait_until(|| sandbox.sock.exists(), "the headless pixtuoid's socket")?;

        let recorder = sandbox
            .command(&built.recorder)
            .args(recorder_args(
                dir,
                &sandbox.sock,
                sandbox.dir.path(),
                &sandbox.cli_env(p),
                p,
                scenario,
            ))
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

    /// Config a CLI would read that no sandbox home redirects: one of
    /// `system_config`'s paths, its [`USER_PLACEHOLDER`] the operator's
    /// (unchecked with none), or one of `ancestor_config` in an ancestor of
    /// `workspace`, which need not exist yet. An ancestor may be a symlink (macOS's `/tmp`), so
    /// the ancestors of the nearest existing one's target count too.
    fn outside_config(
        system_config: &[&str],
        ancestor_config: &[&str],
        workspace: &Path,
        user: Option<&str>,
    ) -> Option<PathBuf> {
        let lexical = workspace.ancestors().skip(1);
        let real = lexical
            .clone()
            .find_map(|a| std::fs::canonicalize(a).ok())
            .into_iter()
            .flat_map(|r| r.ancestors().map(Path::to_path_buf).collect::<Vec<_>>());
        let ancestors: Vec<PathBuf> = lexical.map(Path::to_path_buf).chain(real).collect();
        let inherited = ancestors
            .iter()
            .flat_map(|dir| ancestor_config.iter().map(move |name| dir.join(name)));
        let system = system_config.iter().filter_map(|path| match user {
            _ if !path.contains(USER_PLACEHOLDER) => Some(PathBuf::from(path)),
            Some(name) => Some(PathBuf::from(path.replace(USER_PLACEHOLDER, name))),
            None => None,
        });
        system.chain(inherited).find(|path| path.exists())
    }

    /// The recorder's command line for one run: the raw capture into `dir`,
    /// teed to the headless scene at `sock`, its harvest held inside `within`,
    /// and the profile's env for the CLI.
    fn recorder_args(
        dir: &Path,
        sock: &Path,
        within: &Path,
        cli_env: &[String],
        p: &Profile,
        scenario: &Scenario,
    ) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            recorder_flag::OUT.into(),
            dir.into(),
            recorder_flag::TEE.into(),
            sock.into(),
            recorder_flag::WITHIN.into(),
            within.into(),
        ];
        for kv in cli_env {
            args.extend([recorder_flag::ENV.into(), kv.into()]);
        }
        args.extend([p.source, scenario.shape.name()].map(OsString::from));
        args.extend(p.invocation.iter().map(OsString::from));
        args
    }

    /// The scenario's script with `{workspace}` and [`NOTE_PLACEHOLDER`]
    /// expanded: a CLI that takes an absolute path gets the one the recorder runs
    /// it in, and a reply waits for the note it reads.
    fn render_script(p: &Profile, scenario: &Scenario, into: &Path) -> std::io::Result<PathBuf> {
        let text = std::fs::read_to_string(scenario.script(p.source))?;
        let workspace = Path::new(CAPTURE_WORKSPACE);
        let workspace =
            std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let out = into.join("script.json");
        std::fs::write(
            &out,
            text.replace("{workspace}", &workspace.to_string_lossy())
                .replace(NOTE_PLACEHOLDER, CAPTURE_NOTE),
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
            std::thread::sleep(POLL);
        }
        Err(std::io::Error::other(format!("{what} never came up")))
    }

    /// A bare HTTP/1.0 GET to aimock on loopback: the core crate carries no
    /// HTTP client, and this is the whole of the protocol the tier needs.
    fn http_get(port: u16, path: &str) -> std::io::Result<String> {
        let mut s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(HTTP_READ_TIMEOUT))?;
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
            assert_eq!(a.versions, Versions::Pinned);
            assert!(a.selects("anything", "at-all"));
            assert!(parse(&args(&["--cli"])).is_err());
            assert!(parse(&args(&["--cli", "newest"])).is_err());
            assert!(parse(&args(&["codex", "tool-run", "extra"])).is_err());
        }

        #[test]
        fn the_reports_title_is_the_worst_verdict_or_that_nothing_ran() {
            let pass = Entry::new(
                "codex/tool-run".into(),
                Some("codex-cli 0.155.1".into()),
                Verdict::new(vec![]),
            );
            let blocked = || {
                Entry::new(
                    "opencode/tool-run".into(),
                    None,
                    Verdict::new(vec![(Label::Blocked, "npm view failed".into())]),
                )
            };
            let r = report(Versions::Latest, &[pass, blocked()]);
            assert_eq!(r.lines().next(), Some("# hermetic, latest CLIs: Blocked"));
            assert!(r.contains("## opencode/tool-run: Blocked\n- Blocked: npm view failed"));
            assert!(r.contains("## codex/tool-run (codex-cli 0.155.1): Pass"));
            let r = report(Versions::Pinned, &[blocked()]);
            assert_eq!(
                r.lines().next(),
                Some("# hermetic, verified CLIs: NOTHING RAN")
            );
            // The script lost a CLI that then exited: it ran, and its drift is the title.
            let lost = Entry::new(
                "codex/tool-run".into(),
                None,
                Verdict::new(vec![
                    (Label::Blocked, "exit 1".into()),
                    (Label::Script, "1 unmatched".into()),
                ]),
            );
            let r = report(Versions::Latest, &[lost, blocked()]);
            assert_eq!(r.lines().next(), Some("# hermetic, latest CLIs: Script"));
        }

        #[test]
        fn config_outside_the_sandbox_is_found_through_a_symlinked_ancestor() {
            let d = tempfile::tempdir().expect("tempdir");
            let real = d.path().join("real/sub");
            std::fs::create_dir_all(&real).expect("mkdir");
            std::os::unix::fs::symlink(&real, d.path().join("alias")).expect("symlink");
            // The workspace and its parent don't exist yet, as on a clean runner.
            let workspace = d.path().join("alias/capture/proj");
            let names = &["hermetic-probe.md"];
            assert_eq!(outside_config(&[], names, &workspace, None), None);
            let planted = d.path().join("real/hermetic-probe.md");
            std::fs::write(&planted, "").expect("plant");
            let found = outside_config(&[], names, &workspace, None).expect("found");
            assert_eq!(
                std::fs::canonicalize(found).ok(),
                std::fs::canonicalize(&planted).ok()
            );
        }

        #[test]
        fn a_listed_system_path_is_config_outside_the_sandbox_once_it_exists() {
            let d = tempfile::tempdir().expect("tempdir");
            let workspace = d.path().join("proj");
            let system = d.path().join("etc-cli");
            let listed = system.to_str().expect("utf-8");
            assert_eq!(
                outside_config(&[listed], &[], &workspace, None),
                None,
                "listed but absent"
            );
            std::fs::create_dir(&system).expect("mkdir");
            assert_eq!(
                outside_config(&[listed], &[], &workspace, None),
                Some(PathBuf::from(listed))
            );
        }

        #[test]
        fn a_per_user_system_path_is_the_operators_and_unchecked_with_no_user() {
            let d = tempfile::tempdir().expect("tempdir");
            let workspace = d.path().join("proj");
            let per_user = d.path().join("prefs/alice/managed.plist");
            let pattern = d
                .path()
                .join(format!("prefs/{USER_PLACEHOLDER}/managed.plist"));
            // The placeholder kept literal, and expanded to nothing: what the
            // no-user arm would find if it checked anything.
            for planted in [&per_user, &pattern, &d.path().join("prefs/managed.plist")] {
                std::fs::create_dir_all(planted.parent().expect("parent")).expect("mkdir");
                std::fs::write(planted, "").expect("plant");
            }
            let pattern = pattern.to_str().expect("utf-8");
            assert_eq!(
                outside_config(&[pattern], &[], &workspace, Some("alice")),
                Some(per_user)
            );
            assert_eq!(
                outside_config(&[pattern], &[], &workspace, Some("bob")),
                None
            );
            assert_eq!(
                outside_config(&[pattern], &[], &workspace, None),
                None,
                "no user: unchecked"
            );
        }

        #[test]
        fn the_recorder_is_held_to_the_sandbox_and_handed_the_clis_env() {
            let p = &PROFILES[0];
            let args = recorder_args(
                Path::new("/runs/x"),
                Path::new("/sb/pix.sock"),
                Path::new("/sb"),
                &["A=b".to_string()],
                p,
                &p.scenarios[0],
            );
            let pair =
                |flag: &str, value: &str| args.windows(2).any(|w| w[0] == *flag && w[1] == *value);
            assert!(pair("--within", "/sb"), "{args:?}");
            assert!(pair("--env", "A=b"), "{args:?}");
            assert!(pair("--tee", "/sb/pix.sock"), "{args:?}");
            assert!(pair("--out", "/runs/x"), "{args:?}");
            // The recorder reads options only before its positionals.
            let source = args.iter().position(|a| a == p.source).expect("the source");
            assert!(
                args[..source]
                    .chunks(2)
                    .all(|kv| kv[0].to_str().is_some_and(|f| f.starts_with("--"))),
                "{args:?}"
            );
            let positionals: Vec<OsString> = [p.source, p.scenarios[0].shape.name()]
                .into_iter()
                .chain(p.invocation.iter().copied())
                .map(OsString::from)
                .collect();
            assert_eq!(args[source..], positionals[..], "{args:?}");
        }

        #[test]
        fn a_sources_runs_are_judged_together_only_when_every_scenario_was() {
            let one = [PathBuf::from("/runs/x")];
            assert!(judges_source(&one, 1));
            assert!(!judges_source(&one, 2), "a sibling selected out or Blocked");
        }

        #[test]
        fn every_child_of_a_run_gets_the_profiles_env_expanded() {
            let built = Built {
                pixtuoid: PathBuf::from("/nowhere/pixtuoid"),
                shim: PathBuf::from("/nowhere/pixtuoid-hook"),
                recorder: PathBuf::from("/nowhere/capture_fixture"),
            };
            for p in PROFILES {
                let sandbox =
                    Sandbox::new(p, &built, Path::new("/nowhere/bin"), 4321).expect("sandbox");
                assert_eq!(sandbox.cli_env(p).len(), p.env.len(), "{}", p.source);
                for (k, v) in p.env {
                    let want = v.replace("{port}", "4321").replace("{key}", DUMMY_KEY);
                    assert_eq!(
                        sandbox.var_os(k),
                        Some(&OsString::from(&want)),
                        "{}: {k} misses `connect`'s children",
                        p.source
                    );
                    // The only path past the recorder's `CLAUDE_CODE_*` scrub.
                    assert!(
                        sandbox.cli_env(p).contains(&format!("{k}={want}")),
                        "{}: {k} misses the CLI's `--env`",
                        p.source
                    );
                }
            }
        }

        #[test]
        fn an_install_points_every_home_its_cli_reads_into_the_install() {
            let dir = Path::new("/tier/cli/x-1.0.0");
            for p in PROFILES {
                let install = npm_install(p, dir, "x@1.0.0");
                let homes: Vec<(&std::ffi::OsStr, Option<&std::ffi::OsStr>)> =
                    install.get_envs().collect();
                for var in p.home_vars() {
                    let at = homes
                        .iter()
                        .find(|(k, _)| *k == var)
                        .and_then(|(_, v)| *v)
                        .unwrap_or_else(|| panic!("{}: {var} is the operator's", p.source));
                    assert!(Path::new(at).starts_with(dir), "{}: {var}", p.source);
                }
            }
        }

        #[test]
        fn npm_and_aimock_keep_npm_config_and_lose_provider_keys() {
            temp_env::with_vars(
                [
                    ("ANTHROPIC_API_KEY", Some("sk-host")),
                    ("npm_config_cache", Some("/npm-cache")),
                ],
                || {
                    let ok = tool("sh")
                        .args([
                            "-c",
                            r#"test -z "$ANTHROPIC_API_KEY" && test "$npm_config_cache" = /npm-cache"#,
                        ])
                        .status()
                        .expect("sh runs");
                    assert!(ok.success());
                },
            );
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

        #[test]
        fn record_takes_whole_sources() {
            let a = parse(&args(&["--record", "--cli", "pinned", "dsh"])).expect("parse");
            assert!(a.record);
            assert_eq!(a.versions, Versions::Pinned);
            let check = |v: &[&str]| parse(&args(v)).expect("parse").check();
            assert!(check(&["--record"]).is_ok());
            assert!(check(&["--record", "dsh"]).is_ok());
            assert!(check(&["--cli", "latest", "--record", "dsh"]).is_ok());
            assert!(
                check(&["--cli", "latest", "--record"]).is_err(),
                "a later source's refusal would strand an earlier one's repin"
            );
            assert!(
                check(&["--record", "dsh", "tool-run"]).is_err(),
                "the census needs all"
            );
            assert!(!parse(&args(&["dsh"])).expect("parse").record);
        }

        #[test]
        fn npm_answers_a_release_or_says_why_not() {
            let spec = "@openai/codex@latest";
            assert_eq!(
                npm_release(spec, true, b"0.162.1\n", b""),
                Ok(Version::new(0, 162, 1))
            );
            for bad in [&b"0.162.1\""[..], b"latest", b"", b"  \n"] {
                assert!(npm_release(spec, true, bad, b"").is_err(), "{bad:?}");
            }
            assert!(
                npm_release(spec, true, b"\n", b"").is_err_and(|why| why.contains("no version")),
                "an unresolvable tag exits 0 with nothing on stdout"
            );
            let stderr = b"npm error code E404\nnpm error 404 No match found for version latest\n\
                           npm error 404\nnpm error A complete log of this run can be found in: /Users/op/x.log\n";
            let why = npm_release(spec, false, b"", stderr).expect_err("npm failed");
            assert!(
                why.contains("E404") && why.contains("No match found"),
                "{why}"
            );
            assert!(
                !why.contains("/Users/op"),
                "the code and summary alone: {why}"
            );
        }

        #[test]
        fn a_recording_moves_the_pin_only_up() {
            let registry = "const A: D = D {\n    name: a_cli::SOURCE_NAME,\n    verified_version: \"1.0.0\",\n};\n";
            let pin = Version::new(1, 0, 0);
            assert_eq!(repinned_registry(registry, "a-cli", &pin, &pin), Ok(None));
            assert_eq!(
                repinned_registry(registry, "a-cli", &pin, &Version::new(1, 1, 0)),
                Ok(Some(registry.replace("1.0.0", "1.1.0")))
            );
            assert!(
                repinned_registry(registry, "a-cli", &pin, &Version::new(0, 9, 0)).is_err(),
                "a `latest` tag moved back"
            );
            assert!(
                repinned_registry(registry, "b-cli", &pin, &Version::new(1, 1, 0)).is_err(),
                "no row to move, refused before any CLI runs"
            );
        }
    }
}
