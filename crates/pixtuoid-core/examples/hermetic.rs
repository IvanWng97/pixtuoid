//! Run each profiled agent CLI for real against a scripted model (aimock), with
//! a headless pixtuoid on the same run, and judge what pixtuoid made of it.
//! Profiles, scripts and the judgement: `pixtuoid_core::harness::hermetic`.
//!
//! `--cli pinned` (the default) installs each CLI at its registry
//! `verified_version`, the merge queue's end-to-end gate; `--cli latest` installs
//! what npm's `latest` tag points at, the nightly drift run.
//!
//! Exits 0 when every run passed, 1 on any other verdict, and anything else
//! when the tier itself could not run.

fn main() {
    match run::main() {
        Ok(all_passed) => std::process::exit(i32::from(!all_passed)),
        Err(e) => {
            eprintln!("hermetic: the tier could not run: {e}");
            std::process::exit(2);
        }
    }
}

mod run {
    use std::ffi::{OsStr, OsString};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use pixtuoid_core::harness::hermetic::verify::{self, Label, Verdict};
    use pixtuoid_core::harness::hermetic::{
        self, DUMMY_KEY, HostConfig, MANIFEST, Manifest, NOT_HERMETIC, NOTE_PLACEHOLDER, PROFILES,
        Package, PerOs, Profile, SCENE_LOG, Scenario, USER_PLACEHOLDER, WORKSPACE_PLACEHOLDER,
    };
    use pixtuoid_core::harness::tap::Endpoint;
    use pixtuoid_core::harness::{
        CAPTURE_NOTE, CAPTURE_RECORD, CaptureRecord, capture_workspace, recorder_flag,
    };

    /// How long a server gets to come up before the run gives up.
    const BOOT: Duration = Duration::from_secs(60);
    /// How often a server that is coming up is asked whether it is ready.
    const POLL: Duration = Duration::from_millis(200);
    /// How long one read from aimock may hang.
    const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);
    /// How long the headless scene runs on after the recorder returns, to
    /// decode the last payloads and log any breadcrumb they provoke.
    const SCENE_SETTLE: Duration = Duration::from_secs(4);
    /// The variables a CLI may inherit from the host, with each platform's own
    /// in [`PLATFORM_INHERITED`]: everything else is cleared, so an operator's
    /// provider keys can never reach a CLI whose base-URL wiring slipped and bill
    /// a real provider.
    const INHERITED: &[&str] = &["PATH"];
    const PLATFORM_INHERITED: PerOs<&[&str]> = PerOs {
        unix: &[
            "LANG", "LC_ALL", "TERM", "TMPDIR", "SHELL", "USER", "LOGNAME",
        ],
        windows: &[
            // `connect` finds a CLI's `.cmd` through `which`, which reads it
            // (`which-8.0.6/src/finder.rs:277`).
            "PATHEXT",
            // "In order to run a side-by-side assembly the specified env must
            // include a valid %SystemRoot%" (docs.python.org, subprocess).
            "SystemRoot",
            // A CLI's shell: Node falls back to `cmd.exe` without it
            // (nodejs.org/api/child_process.html, "Default Windows shell").
            "ComSpec",
            // TMPDIR's twins: GetTempPath reads TMP, then TEMP, then
            // USERPROFILE, a sandbox home (learn.microsoft.com, GetTempPath2W).
            "TMP",
            "TEMP",
        ],
    };
    /// What npm and aimock inherit beyond [`INHERITED`]: npm's own config, cache
    /// and network settings. A CLI's install scripts and the npm-installed aimock
    /// are untrusted code, so no provider key reaches them either.
    const TOOL_INHERITED: &[&str] = &[
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "NODE_EXTRA_CA_CERTS",
    ];
    /// npm's home, whose `cache` is `~/.npm`, or `%LocalAppData%\npm-cache` on
    /// Windows (docs.npmjs.com/cli/v11/using-npm/config).
    const PLATFORM_TOOL_INHERITED: PerOs<&[&str]> = PerOs {
        unix: &["HOME"],
        windows: &["USERPROFILE", "APPDATA", "LOCALAPPDATA"],
    };

    /// npm, which Windows installs as a batch file: std runs one with its
    /// arguments escaped "according to `cmd.exe` rules", refusing at spawn one it
    /// cannot (std::process, "Batch file special handling"), and an npm name has
    /// no character that needs it (docs.npmjs.com, package.json `name`).
    const NPM: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

    /// Whether `name` is on `list`, as `windows` compares variable names: there
    /// they ignore case, and a host may spell `PATH` as `Path`.
    fn listed(windows: bool, list: &[&str], name: &str) -> bool {
        list.iter().any(|k| {
            if windows {
                k.eq_ignore_ascii_case(name)
            } else {
                *k == name
            }
        })
    }

    /// Every variable a CLI inherits on this platform.
    fn inherited() -> impl Iterator<Item = &'static str> {
        INHERITED
            .iter()
            .chain(PLATFORM_INHERITED.here().iter())
            .copied()
    }

    /// `program` with the host env cleared to what [`inherited`],
    /// [`TOOL_INHERITED`] and npm's `npm_config_*` settings keep.
    fn tool(program: impl AsRef<OsStr>) -> Command {
        let mut c = Command::new(program);
        c.env_clear().stdin(Stdio::null());
        let keeps: Vec<&str> = inherited()
            .chain(TOOL_INHERITED.iter().copied())
            .chain(PLATFORM_TOOL_INHERITED.here().iter().copied())
            .collect();
        for (k, v) in std::env::vars_os() {
            let keep = k.to_str().is_some_and(|k| {
                listed(cfg!(windows), &keeps, k)
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
            if self.record && !cfg!(unix) {
                return Err(
                    "--record writes the committed corpus, which pins Unix paths: record on macOS or Linux"
                        .into(),
                );
            }
            if self.record && self.versions == Versions::Latest {
                return Err("--record writes fixtures of the pinned CLIs only".into());
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
            versions: args.versions,
        };

        let mut entries = Vec::new();
        for p in PROFILES {
            let mut judged = Vec::new();
            let mut passed = true;
            for scenario in p.scenarios {
                let name = scenario.shape.name();
                if !args.selects(p.source, name) {
                    continue;
                }
                let dir = run_dir(&runs, p, scenario);
                std::fs::create_dir_all(&dir)?;
                let key = format!("{}/{name}", p.source);
                println!("== {key}");
                let (version, verdict) = match run_one(&ctx, p, scenario, &dir) {
                    Ok(manifest) => (Some(manifest.capture.version), verify::verify(&dir)?),
                    Err(e) => (None, Verdict::new(vec![(Label::Blocked, e.to_string())])),
                };
                if verdict.judged() {
                    judged.push(dir);
                }
                passed &= verdict.label() == Label::Pass;
                entries.push(Entry::new(key, version, verdict));
            }
            if judges_source(&judged, p.scenarios.len()) {
                let key = format!("{} (every scenario)", p.source);
                println!("== {key}");
                let verdict = verify::verify_source(p, &judged)?;
                passed &= verdict.label() == Label::Pass;
                entries.push(Entry::new(key, None, verdict));
                if args.record && passed {
                    record(&ctx, p, &runs)?;
                }
            }
        }
        let report = report(args.versions, &host_tools(), &entries);
        std::fs::write(runs.join(REPORT), &report)?;
        println!("\n{report}");
        Ok(entries.iter().all(|e| e.verdict.label() == Label::Pass))
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

    /// The host's own tools a CLI runs, which the runner image supplies unpinned.
    const HOST_TOOLS: PerOs<&[&str]> = PerOs {
        unix: &["git"],
        windows: &["git", "pwsh"],
    };

    /// Each of [`HOST_TOOLS`]' `--version`, or why it has none.
    fn host_tools() -> Vec<String> {
        HOST_TOOLS
            .here()
            .iter()
            .map(|t| {
                tool(t)
                    .arg("--version")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .and_then(|v| v.lines().next().map(str::trim).map(str::to_owned))
                    .unwrap_or_else(|| format!("{t}: none"))
            })
            .collect()
    }

    fn report(versions: Versions, host: &[String], entries: &[Entry]) -> String {
        let which = match versions {
            Versions::Pinned => "verified",
            Versions::Latest => "latest",
        };
        let labels = || entries.iter().map(|e| e.verdict.label());
        let title = match labels().max() {
            Some(worst) if labels().any(|l| l != Label::Blocked) => format!("{worst:?}"),
            _ => "NOTHING RAN".to_owned(),
        };
        let os = std::env::consts::OS;
        let mut out = format!(
            "# hermetic, {which} CLIs on {os}: {title}\n\nHost: {}\n",
            host.join(", ")
        );
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
            let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
            let found = Self {
                pixtuoid: profile_dir.join(exe("pixtuoid")),
                shim: profile_dir.join(exe("pixtuoid-hook")),
                recorder: me.with_file_name(exe("capture_fixture")),
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
        launch: Launch,
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
            npm(&dir, &["ci", "--ignore-scripts"])?;
            let modules = dir.join("node_modules");
            Ok(Self {
                launch: Launch::of(
                    &modules.join("@copilotkit/aimock"),
                    &modules.join(".bin"),
                    "llmock",
                )?,
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

    fn npm(dir: &Path, args: &[&str]) -> std::io::Result<()> {
        let status = tool(NPM)
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
    fn install_cli(tier: &Path, p: &Profile, versions: Versions) -> std::io::Result<Installed> {
        let Package::Npm(package) = p.package;
        let version = match versions {
            Versions::Pinned => p.descriptor().verified_version.to_string(),
            Versions::Latest => {
                let out = tool(NPM).args(["view", package, "version"]).output()?;
                if !out.status.success() {
                    return Err(std::io::Error::other(format!("npm view {package} failed")));
                }
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            }
        };
        let prefix = tier.join("cli").join(format!("{}-{version}", p.source));
        let pinned = format!("{package}@{version}");
        install_once(&prefix, |dir| {
            let project = serde_json::json!({
                "private": true,
                "allowScripts": { &pinned: true },
            });
            std::fs::write(dir.join("package.json"), project.to_string())?;
            npm(dir, &["install", &pinned])
        })?;
        let modules = prefix.join("node_modules");
        let bin = modules.join(".bin");
        let name = p.invocation[0];
        Ok(Installed {
            launch: Launch::of(&modules.join(package), &bin, name)?,
            bin,
        })
    }

    /// An installed CLI: the directory npm links its commands into, and how to
    /// start it.
    struct Installed {
        bin: PathBuf,
        launch: Launch,
    }

    /// How to start an npm-installed command.
    #[derive(Debug, PartialEq, Eq)]
    struct Launch {
        program: PathBuf,
        args: Vec<String>,
        /// What the entry's shebang sets.
        env: Vec<(String, String)>,
    }

    impl Launch {
        /// `name` from the package at `package` npm linked into `bin`: on Unix
        /// the link itself, which the kernel runs by its shebang; on Windows
        /// what npm's shim there runs (`Launch::from_bin_field`).
        fn of(package: &Path, bin: &Path, name: &str) -> std::io::Result<Self> {
            if cfg!(windows) {
                Self::from_bin_field(package, bin, name)
            } else {
                Ok(Self {
                    program: bin.join(name),
                    args: Vec::new(),
                    env: Vec::new(),
                })
            }
        }

        /// What npm's Windows shim runs for `name`, as cmd-shim writes it
        /// (`npm/cmd-shim` `lib/index.js:24,50-58,73-119` @v9.0.2): a `bin`
        /// entry with a shebang runs under its program (the `.exe` beside the
        /// shim if there is one) with its arguments and variables, and one
        /// without (a PE) runs itself.
        fn from_bin_field(package: &Path, bin: &Path, name: &str) -> std::io::Result<Self> {
            let manifest: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(package.join("package.json"))?)?;
            let rel = match &manifest["bin"] {
                serde_json::Value::String(only) => Some(only.as_str()),
                bins => bins[name].as_str(),
            }
            .ok_or_else(|| {
                std::io::Error::other(format!("{} has no bin {name}", package.display()))
            })?;
            let entry = package.join(rel);
            let mut head = Vec::new();
            std::fs::File::open(&entry)?
                .take(SHEBANG_READ)
                .read_to_end(&mut head)?;
            let first = String::from_utf8_lossy(&head);
            let first = first.trim().lines().next().unwrap_or_default();
            Ok(match Shebang::parse(first) {
                None => Self {
                    program: entry,
                    args: Vec::new(),
                    env: Vec::new(),
                },
                Some(sb) => {
                    let beside = bin.join(format!("{}.exe", sb.program));
                    let mut args = sb.args;
                    args.push(entry.to_string_lossy().into_owned());
                    Self {
                        program: if beside.is_file() {
                            beside
                        } else {
                            PathBuf::from(sb.program)
                        },
                        args,
                        env: sb.env,
                    }
                }
            })
        }

        /// The command line, program first.
        fn argv(&self) -> Vec<OsString> {
            std::iter::once(self.program.clone().into_os_string())
                .chain(self.args.iter().map(OsString::from))
                .collect()
        }
    }

    /// How much of an entry is read for its first line: past any shebang, and
    /// short of reading a native CLI's whole binary.
    const SHEBANG_READ: u64 = 4096;

    /// A `#!` line as cmd-shim's `shebangExpr` reads it: `/usr/bin/env`, an
    /// optional `-S`, leading `VAR=value` words, then the program and its
    /// arguments.
    #[derive(Debug, PartialEq, Eq)]
    struct Shebang {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    }

    impl Shebang {
        fn parse(line: &str) -> Option<Self> {
            let rest = line.strip_prefix("#!")?.trim_start();
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            let mut env = Vec::new();
            if words.first() == Some(&"/usr/bin/env") {
                words.remove(0);
                if words.first() == Some(&"-S") {
                    words.remove(0);
                }
                while let Some((k, v)) = words.first().and_then(|w| w.split_once('=')) {
                    if k.is_empty() || v.is_empty() || v.contains('=') {
                        break;
                    }
                    env.push((k.to_owned(), v.to_owned()));
                    words.remove(0);
                }
            }
            let (program, args) = words.split_first()?;
            Some(Self {
                program: (*program).to_owned(),
                args: args.iter().map(|a| (*a).to_owned()).collect(),
                env,
            })
        }
    }

    /// One sandbox: every home a CLI or pixtuoid resolves points inside it, so
    /// nothing reads or writes the operator's.
    struct Sandbox {
        dir: tempfile::TempDir,
        env: Vec<(String, OsString)>,
        /// Where the headless pixtuoid listens and the recorder tees to.
        scene: Endpoint,
    }

    /// Every home a CLI or pixtuoid may resolve, each a directory of the
    /// sandbox. All are set on every platform: a variable no reader there reads
    /// is inert.
    const SANDBOX_HOMES: &[(&str, &str)] = &[
        ("HOME", "home"),
        ("USERPROFILE", "home"),
        ("APPDATA", "home/AppData/Roaming"),
        ("LOCALAPPDATA", "home/AppData/Local"),
        ("XDG_CONFIG_HOME", "xdg/config"),
        ("XDG_DATA_HOME", "xdg/data"),
        ("XDG_STATE_HOME", "xdg/state"),
        ("XDG_CACHE_HOME", "xdg/cache"),
    ];

    impl Sandbox {
        fn new(p: &Profile, built: &Built, cli_bin: &Path, port: u16) -> std::io::Result<Self> {
            let dir = tempfile::tempdir()?;
            let at = |rel: &str| -> std::io::Result<PathBuf> {
                let d = dir.path().join(rel);
                std::fs::create_dir_all(&d)?;
                Ok(d)
            };
            let bin = at("bin")?;
            // A copy: a Windows symlink needs privilege, and a hard link cannot
            // cross from the build's volume to the temp one.
            std::fs::copy(
                &built.shim,
                bin.join(format!("pixtuoid-hook{}", std::env::consts::EXE_SUFFIX)),
            )?;
            let mut env: Vec<(String, OsString)> = inherited()
                .filter(|k| *k != "PATH")
                .filter_map(|k| std::env::var_os(k).map(|v| (k.to_string(), v)))
                .collect();
            let host_path = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                [bin, cli_bin.to_path_buf()]
                    .into_iter()
                    .chain(std::env::split_paths(&host_path)),
            )
            .map_err(std::io::Error::other)?;
            env.push(("PATH".into(), path));
            for (var, rel) in SANDBOX_HOMES {
                env.push(((*var).into(), at(rel)?.into()));
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
            // Every child gets the profile's env, `connect`'s spawned CLI included;
            // the CLI under test also gets it as `--env` (`Sandbox::cli_env`).
            for (k, v) in p.env {
                env.push(((*k).to_string(), expand(v).into()));
            }
            let sandbox = Self {
                scene: Endpoint::in_dir(dir.path(), "scene"),
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

        /// The profile's pairs as this sandbox holds them, then what the CLI's
        /// launch sets, for the recorder to set on the CLI as `--env`.
        fn cli_env(&self, p: &Profile, launch: &Launch) -> Vec<String> {
            p.env
                .iter()
                .filter_map(|(k, _)| {
                    self.var_os(k)
                        .map(|v| format!("{k}={}", v.to_string_lossy()))
                })
                .chain(launch.env.iter().map(|(k, v)| format!("{k}={v}")))
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
        fn command(&self, program: impl AsRef<OsStr>) -> Command {
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
        let workspace = capture_workspace()?;
        if let Some(found) = outside_config(
            p.system_config.here(),
            p.ancestor_config,
            &workspace,
            std::env::var_os("USER").as_deref().and_then(OsStr::to_str),
        ) {
            return Err(std::io::Error::other(format!(
                "{found} configures {} from outside the sandbox",
                p.source
            )));
        }
        let cli = install_cli(tier, p, *versions)?;
        let port = free_port()?;
        let sandbox = Sandbox::new(p, built, &cli.bin, port)?;

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

        let script = render_script(p, scenario, &workspace, sandbox.dir.path())?;
        let mut model = tool(&aimock.launch.program);
        model
            .args(&aimock.launch.args)
            .envs(aimock.launch.env.iter().map(|(k, v)| (k, v)));
        let model = Reaped(
            model
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
                .arg(sandbox.scene.path())
                // Its log colors a file too, and the scene log is read as text.
                .env("NO_COLOR", "1")
                .stdout(log(dir, SCENE_LOG)?)
                .stderr(log(dir, SCENE_LOG)?)
                .spawn()?,
        );
        wait_until(
            || sandbox.scene.accepting(),
            "the headless pixtuoid's endpoint",
        )?;

        let recorder = sandbox
            .command(&built.recorder)
            .args(recorder_args(
                &RecorderRun {
                    out: dir,
                    scene: sandbox.scene.path(),
                    within: sandbox.dir.path(),
                    workspace: &workspace,
                    cli_env: &sandbox.cli_env(p, &cli.launch),
                    cli_version: &cli_version(&sandbox, p, &cli.launch)?,
                    launch: &cli.launch,
                },
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
    /// `system_config`'s places, its [`USER_PLACEHOLDER`] the operator's
    /// (unchecked with none), or one of `ancestor_config` in an ancestor of
    /// `workspace`, which need not exist yet. An ancestor may be a symlink (macOS's `/tmp`), so
    /// the ancestors of the nearest existing one's target count too.
    fn outside_config(
        system_config: &[HostConfig],
        ancestor_config: &[&str],
        workspace: &Path,
        user: Option<&str>,
    ) -> Option<String> {
        let lexical = workspace.ancestors().skip(1);
        let real = lexical
            .clone()
            .find_map(|a| std::fs::canonicalize(a).ok())
            .into_iter()
            .flat_map(|r| r.ancestors().map(Path::to_path_buf).collect::<Vec<_>>());
        let ancestors: Vec<PathBuf> = lexical.map(Path::to_path_buf).chain(real).collect();
        let inherited = ancestors
            .iter()
            .flat_map(|dir| ancestor_config.iter().map(move |name| dir.join(name)))
            .find(|path| path.exists())
            .map(|path| path.display().to_string());
        system_config
            .iter()
            .find_map(|row| host_config_found(row, user))
            .or(inherited)
    }

    /// `row`, as found on this host.
    fn host_config_found(row: &HostConfig, user: Option<&str>) -> Option<String> {
        match row {
            HostConfig::Path(path) => {
                let path = match user {
                    _ if !path.contains(USER_PLACEHOLDER) => (*path).to_owned(),
                    Some(name) => path.replace(USER_PLACEHOLDER, name),
                    None => return None,
                };
                let path = known_folders_expanded(&path)?;
                path.exists().then(|| path.display().to_string())
            }
            #[cfg(windows)]
            HostConfig::Registry(key) => {
                pixtuoid_core::harness::host::registry_key_exists(key).then(|| (*key).to_owned())
            }
            #[cfg(not(windows))]
            HostConfig::Registry(_) => None,
        }
    }

    /// `path` with a leading known-folder placeholder resolved, or `None` when
    /// the shell has no such folder.
    fn known_folders_expanded(path: &str) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            use pixtuoid_core::harness::host::{KnownFolder, known_folder};
            for folder in KnownFolder::ALL {
                if let Some(rest) = path.strip_prefix(folder.placeholder()) {
                    let base = known_folder(folder).ok()?;
                    return Some(base.join(rest.trim_start_matches('\\')));
                }
            }
        }
        Some(PathBuf::from(path))
    }

    /// The CLI's `--version`, asked of it alone in the sandbox the run uses:
    /// the recorder cannot repeat a launch through an interpreter.
    fn cli_version(sandbox: &Sandbox, p: &Profile, launch: &Launch) -> std::io::Result<String> {
        let probe = p
            .descriptor()
            .version_probe
            .ok_or_else(|| std::io::Error::other(format!("{} has no version probe", p.source)))?;
        let out = sandbox
            .command(&launch.program)
            .args(&launch.args)
            .args(&probe[1..])
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .output()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .map(|l| l.trim().to_owned())
            .filter(|v| out.status.success() && !v.is_empty())
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "{:?} --version answered nothing (exit {})",
                    launch.program, out.status
                ))
            })
    }

    /// What the tier decides about one recorder run.
    struct RecorderRun<'a> {
        /// The raw capture goes here.
        out: &'a Path,
        /// Teed to the headless scene listening here.
        scene: &'a Path,
        /// Its harvest held inside this sandbox.
        within: &'a Path,
        /// The workspace the CLI runs in, as this tier resolved it.
        workspace: &'a Path,
        /// Set on the CLI after the recorder's scrub.
        cli_env: &'a [String],
        cli_version: &'a str,
        launch: &'a Launch,
    }

    /// The recorder's command line for one run: its options, then the source,
    /// the scenario and the CLI's own launch and invocation.
    fn recorder_args(run: &RecorderRun<'_>, p: &Profile, scenario: &Scenario) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            recorder_flag::OUT.into(),
            run.out.into(),
            recorder_flag::TEE.into(),
            run.scene.into(),
            recorder_flag::WITHIN.into(),
            run.within.into(),
            recorder_flag::CLI_VERSION.into(),
            run.cli_version.into(),
            recorder_flag::WORKSPACE.into(),
            run.workspace.into(),
        ];
        for kv in run.cli_env {
            args.extend([recorder_flag::ENV.into(), kv.into()]);
        }
        args.extend([p.source, scenario.shape.name()].map(OsString::from));
        args.extend(run.launch.argv());
        args.extend(p.invocation[1..].iter().map(OsString::from));
        args
    }

    /// The scenario's script with its placeholders expanded in every string,
    /// written into `into`: a CLI that takes an absolute path gets the one the
    /// recorder runs it in, as its own cwd reads it, and a reply waits for the
    /// note the tool read.
    fn render_script(
        p: &Profile,
        scenario: &Scenario,
        workspace: &Path,
        into: &Path,
    ) -> std::io::Result<PathBuf> {
        let mut script: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(scenario.script(p.source))?)?;
        expand_strings(&mut script, &script_values(p, workspace));
        let out = into.join("script.json");
        std::fs::write(&out, serde_json::to_string_pretty(&script)?)?;
        Ok(out)
    }

    /// Each placeholder's value here. Unix's realpath is what a CLI's `getcwd`
    /// reports (macOS's `/tmp` is `/private/tmp`); Windows' cwd reads back as
    /// it was set, and canonicalizing would resolve a junction it never sees.
    fn script_values(p: &Profile, workspace: &Path) -> Vec<(String, String)> {
        let workspace = if cfg!(windows) {
            workspace.to_path_buf()
        } else {
            std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf())
        };
        [
            (
                WORKSPACE_PLACEHOLDER.to_owned(),
                workspace.to_string_lossy().into_owned(),
            ),
            (NOTE_PLACEHOLDER.to_owned(), CAPTURE_NOTE.to_owned()),
        ]
        .into_iter()
        .chain(
            p.script_vars
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v.here()).to_owned())),
        )
        .collect()
    }

    /// Replace every placeholder inside `v`'s strings: a raw `C:\…` spliced
    /// into JSON text would be an invalid escape.
    fn expand_strings(v: &mut serde_json::Value, values: &[(String, String)]) {
        match v {
            serde_json::Value::String(text) => {
                for (k, value) in values {
                    *text = text.replace(k.as_str(), value);
                }
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(|i| expand_strings(i, values));
            }
            serde_json::Value::Object(map) => {
                map.values_mut().for_each(|i| expand_strings(i, values));
            }
            _ => {}
        }
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
            let os = std::env::consts::OS;
            let host = ["git version 2.55.0".to_string()];
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
            let r = report(Versions::Latest, &host, &[pass, blocked()]);
            assert_eq!(
                r.lines().next(),
                Some(format!("# hermetic, latest CLIs on {os}: Blocked").as_str())
            );
            assert!(r.contains("Host: git version 2.55.0"), "{r}");
            assert!(r.contains("## opencode/tool-run: Blocked\n- Blocked: npm view failed"));
            assert!(r.contains("## codex/tool-run (codex-cli 0.155.1): Pass"));
            let r = report(Versions::Pinned, &host, &[blocked()]);
            assert_eq!(
                r.lines().next(),
                Some(format!("# hermetic, verified CLIs on {os}: NOTHING RAN").as_str())
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
            let r = report(Versions::Latest, &host, &[lost, blocked()]);
            assert_eq!(
                r.lines().next(),
                Some(format!("# hermetic, latest CLIs on {os}: Script").as_str())
            );
        }

        #[cfg(unix)]
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

        /// A profile's rows are `'static`; a test's temp path lives for the run.
        fn path_row(p: &Path) -> HostConfig {
            HostConfig::Path(Box::leak(p.to_string_lossy().into_owned().into_boxed_str()))
        }

        #[test]
        fn a_listed_system_path_is_config_outside_the_sandbox_once_it_exists() {
            let d = tempfile::tempdir().expect("tempdir");
            let workspace = d.path().join("proj");
            let system = d.path().join("etc-cli");
            let listed = [path_row(&system)];
            assert_eq!(
                outside_config(&listed, &[], &workspace, None),
                None,
                "listed but absent"
            );
            std::fs::create_dir(&system).expect("mkdir");
            assert_eq!(
                outside_config(&listed, &[], &workspace, None),
                Some(system.display().to_string())
            );
        }

        #[test]
        fn a_per_user_system_path_is_the_operators_and_unchecked_with_no_user() {
            let d = tempfile::tempdir().expect("tempdir");
            let workspace = d.path().join("proj");
            let per_user = d.path().join("prefs").join("alice").join("managed.plist");
            let pattern = d
                .path()
                .join("prefs")
                .join(USER_PLACEHOLDER)
                .join("managed.plist");
            // The placeholder kept literal, and expanded to nothing: what the
            // no-user arm would find if it checked anything.
            for planted in [&per_user, &pattern, &d.path().join("prefs/managed.plist")] {
                std::fs::create_dir_all(planted.parent().expect("parent")).expect("mkdir");
                std::fs::write(planted, "").expect("plant");
            }
            let rows = [path_row(&pattern)];
            assert_eq!(
                outside_config(&rows, &[], &workspace, Some("alice")),
                Some(per_user.display().to_string())
            );
            assert_eq!(outside_config(&rows, &[], &workspace, Some("bob")), None);
            assert_eq!(
                outside_config(&rows, &[], &workspace, None),
                None,
                "no user: unchecked"
            );
        }

        #[test]
        fn the_recorder_is_held_to_the_sandbox_and_handed_the_clis_env() {
            let p = &PROFILES[0];
            let launch = Launch {
                program: PathBuf::from("/cli/node"),
                args: vec!["/cli/entry.js".into()],
                env: Vec::new(),
            };
            let args = recorder_args(
                &RecorderRun {
                    out: Path::new("/runs/x"),
                    scene: Path::new("/sb/scene.sock"),
                    within: Path::new("/sb"),
                    workspace: Path::new("/ws/proj"),
                    cli_env: &["A=b".to_string()],
                    cli_version: "1.2.3",
                    launch: &launch,
                },
                p,
                &p.scenarios[0],
            );
            let pair =
                |flag: &str, value: &str| args.windows(2).any(|w| w[0] == *flag && w[1] == *value);
            assert!(pair("--within", "/sb"), "{args:?}");
            assert!(pair("--env", "A=b"), "{args:?}");
            assert!(pair("--tee", "/sb/scene.sock"), "{args:?}");
            assert!(pair("--out", "/runs/x"), "{args:?}");
            assert!(pair("--cli-version", "1.2.3"), "{args:?}");
            assert!(pair("--workspace", "/ws/proj"), "{args:?}");
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
                .map(OsString::from)
                .chain(launch.argv())
                .chain(p.invocation[1..].iter().map(OsString::from))
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
            let d = tempfile::tempdir().expect("tempdir");
            let shim = d.path().join("pixtuoid-hook");
            std::fs::write(&shim, "").expect("a shim");
            let built = Built {
                pixtuoid: PathBuf::from("/nowhere/pixtuoid"),
                shim,
                recorder: PathBuf::from("/nowhere/capture_fixture"),
            };
            let launch = Launch::of(d.path(), d.path(), "cli").unwrap_or(Launch {
                program: PathBuf::from("cli"),
                args: Vec::new(),
                env: Vec::new(),
            });
            for p in PROFILES {
                let sandbox =
                    Sandbox::new(p, &built, Path::new("/nowhere/bin"), 4321).expect("sandbox");
                assert_eq!(
                    sandbox.cli_env(p, &launch).len(),
                    p.env.len() + launch.env.len(),
                    "{}",
                    p.source
                );
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
                        sandbox.cli_env(p, &launch).contains(&format!("{k}={want}")),
                        "{}: {k} misses the CLI's `--env`",
                        p.source
                    );
                }
                for (var, _) in SANDBOX_HOMES {
                    let home = sandbox.var(var).expect("set");
                    assert!(home.starts_with(sandbox.dir.path()), "{var} escapes");
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
                    let c = tool("anything");
                    let envs: Vec<(String, Option<String>)> = c
                        .get_envs()
                        .map(|(k, v)| {
                            (
                                k.to_string_lossy().into_owned(),
                                v.map(|v| v.to_string_lossy().into_owned()),
                            )
                        })
                        .collect();
                    assert!(
                        !envs
                            .iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("ANTHROPIC_API_KEY")),
                        "{envs:?}"
                    );
                    assert!(
                        envs.iter()
                            .any(|(k, v)| k.eq_ignore_ascii_case("npm_config_cache")
                                && v.as_deref() == Some("/npm-cache")),
                        "{envs:?}"
                    );
                },
            );
        }

        #[test]
        fn a_variable_is_kept_as_its_platform_spells_names() {
            let list = ["PATH", "SystemRoot"];
            assert!(listed(true, &list, "Path"), "Windows ignores case");
            assert!(listed(true, &list, "SYSTEMROOT"));
            assert!(!listed(false, &list, "Path"), "Unix does not");
            assert!(listed(false, &list, "PATH"));
            assert!(!listed(true, &list, "PATHEXT"));
        }

        #[test]
        fn a_shebang_reads_as_cmd_shim_reads_it() {
            let sb = |l: &str| Shebang::parse(l).expect("a shebang");
            assert_eq!(
                sb("#!/usr/bin/env node"),
                Shebang {
                    program: "node".into(),
                    args: vec![],
                    env: vec![]
                }
            );
            assert_eq!(
                sb("#!/usr/bin/env node --no-warnings --stack-size=8"),
                Shebang {
                    program: "node".into(),
                    args: vec!["--no-warnings".into(), "--stack-size=8".into()],
                    env: vec![]
                }
            );
            assert_eq!(
                sb("#!/usr/bin/env -S NODE_OPTIONS=--x DEBUG=1 node --flag"),
                Shebang {
                    program: "node".into(),
                    args: vec!["--flag".into()],
                    env: vec![
                        ("NODE_OPTIONS".into(), "--x".into()),
                        ("DEBUG".into(), "1".into())
                    ]
                }
            );
            assert_eq!(sb("#!/bin/sh").program, "/bin/sh");
            assert_eq!(Shebang::parse("MZ\u{90}\0"), None, "a PE has none");
        }

        /// A package as npm lays it out, its `bin` entry holding `head`.
        fn package(d: &Path, bin: serde_json::Value, entry: &str, head: &[u8]) -> PathBuf {
            let pkg = d.join("node_modules/pkg");
            std::fs::create_dir_all(pkg.join("bin")).expect("mkdir");
            std::fs::write(
                pkg.join("package.json"),
                serde_json::json!({ "bin": bin }).to_string(),
            )
            .expect("manifest");
            std::fs::write(pkg.join(entry), head).expect("entry");
            pkg
        }

        #[test]
        fn a_windows_launch_runs_what_npm_s_shim_runs() {
            let d = tempfile::tempdir().expect("tempdir");
            let bin = d.path().join("node_modules/.bin");
            std::fs::create_dir_all(&bin).expect("mkdir");
            let node = package(
                d.path(),
                serde_json::json!({ "cli": "bin/cli.js" }),
                "bin/cli.js",
                b"#!/usr/bin/env node --no-warnings\n",
            );
            let l = Launch::from_bin_field(&node, &bin, "cli").expect("launch");
            assert_eq!(l.program, PathBuf::from("node"), "node on PATH");
            assert_eq!(
                l.args,
                [
                    "--no-warnings".to_string(),
                    node.join("bin/cli.js").to_string_lossy().into_owned()
                ]
            );
            std::fs::write(bin.join("node.exe"), "").expect("a node beside the shim");
            let l = Launch::from_bin_field(&node, &bin, "cli").expect("launch");
            assert_eq!(
                l.program,
                bin.join("node.exe"),
                "the shim prefers its sibling"
            );
            let pe = package(
                d.path(),
                serde_json::json!("bin/cli.exe"),
                "bin/cli.exe",
                b"MZ\x90\x00",
            );
            let l = Launch::from_bin_field(&pe, &bin, "cli").expect("launch");
            assert_eq!(
                l,
                Launch {
                    program: pe.join("bin/cli.exe"),
                    args: vec![],
                    env: vec![]
                },
                "a PE runs itself"
            );
            assert!(
                Launch::from_bin_field(&pe, &bin, "other").is_ok(),
                "a string bin is every name's"
            );
        }

        /// A bare name would leave aimock, whose link no run's PATH holds, unfound.
        #[cfg(unix)]
        #[test]
        fn a_unix_launch_is_the_link_npm_made() {
            let l =
                Launch::of(Path::new("/m/pkg"), Path::new("/m/.bin"), "llmock").expect("launch");
            assert_eq!(l.program, PathBuf::from("/m/.bin/llmock"));
            assert!(l.args.is_empty() && l.env.is_empty());
        }

        #[test]
        fn a_script_expands_inside_its_strings_whatever_the_path_holds() {
            let mut script = serde_json::json!({
                "fixtures": [{ "response": { "toolCalls": [{
                    "name": "{shell_tool}",
                    "arguments": { "file_path": "{workspace}/NOTE.txt" }
                }]}}]
            });
            let values = [
                (
                    "{workspace}".to_string(),
                    r"C:\ProgramData\pixtuoid-capture\proj".to_string(),
                ),
                ("{shell_tool}".to_string(), "pwsh".to_string()),
            ];
            expand_strings(&mut script, &values);
            let text = serde_json::to_string(&script).expect("serializes");
            let back: serde_json::Value = serde_json::from_str(&text).expect("still JSON");
            let call = &back["fixtures"][0]["response"]["toolCalls"][0];
            assert_eq!(call["name"], "pwsh");
            assert_eq!(
                call["arguments"]["file_path"],
                r"C:\ProgramData\pixtuoid-capture\proj/NOTE.txt"
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

        #[cfg(not(unix))]
        #[test]
        fn record_is_refused_off_unix() {
            let check = |v: &[&str]| parse(&args(v)).expect("parse").check();
            let refused = check(&["--record", "dsh"]).expect_err("the corpus pins Unix paths");
            assert!(refused.contains("Unix"), "{refused}");
        }

        #[cfg(unix)]
        #[test]
        fn record_takes_whole_pinned_sources() {
            let a = parse(&args(&["--record", "--cli", "pinned", "dsh"])).expect("parse");
            assert!(a.record);
            assert_eq!(a.versions, Versions::Pinned);
            let check = |v: &[&str]| parse(&args(v)).expect("parse").check();
            assert!(check(&["--record"]).is_ok());
            assert!(check(&["--record", "dsh"]).is_ok());
            assert!(
                check(&["--cli", "latest", "--record"]).is_err(),
                "fixtures track the pin"
            );
            assert!(
                check(&["--record", "dsh", "tool-run"]).is_err(),
                "the census needs all"
            );
            assert!(!parse(&args(&["dsh"])).expect("parse").record);
        }
    }
}
