//! Record a conformance fixture from the bytes a real agent CLI actually sent.
//! Worked examples, the rules, and why this is Rust and not the shell script it
//! replaces: `tests/sources/fixtures/README.md`.
//!
//! ⚠ BILLED — runs one real model turn on that provider's account.
//!
//! It records the SHIM'S OUTPUT, by pointing `PIXTUOID_SOCKET` at a listener of
//! our own — the one seam that does not care how the payload reached the shim.
//! Recording its INPUT cannot cover every source: codewhale is env-mode, so a
//! stdin tee captures a file of empty payloads.

#[cfg(not(unix))]
fn main() {
    // Non-zero like every other "cannot do the job" path here: a caller that
    // scripts this must not read a silent no-capture as a successful one.
    eprintln!("capture-fixture records at the shim's Unix-socket output; run it on macOS or Linux");
    std::process::exit(2);
}

#[cfg(unix)]
fn main() -> std::io::Result<()> {
    recorder::main()
}

/// The whole tool, Unix-gated: it records at a `UnixListener` the shim connects
/// back to, and the Windows shim speaks a named pipe instead.
#[cfg(unix)]
mod recorder {
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use pixtuoid_core::harness::reads::{self, Deidentified};
    use pixtuoid_core::source::{registry, resolved_source_root};

    /// One prompt for every source, so captures stay comparable: reading the SAME
    /// file twice around a list forces both shapes the composed fixtures got wrong —
    /// tools that interleave, and a tool id that repeats. `CAPTURE_PROMPT` overrides
    /// it for a scenario the shared one cannot reach (a permission gate needs a tool
    /// the CLI refuses to run unasked).
    const DEFAULT_PROMPT: &str =
        "Read NOTE.txt, then list this directory, then read NOTE.txt again.";

    use pixtuoid_core::harness::CAPTURE_WORKSPACE as WORKSPACE;
    use pixtuoid_core::harness::recorder_flag::{ENV, OUT, TEE, WITHIN};
    use pixtuoid_core::harness::{CAPTURE_RECORD, CaptureRecord, HOOK_PAYLOADS, PROVENANCE};

    /// Where a driver names the CLI it drives, a command or the path to its
    /// binary, for the recorder to ask its `--version`.
    const CLI_ENV: &str = "PIXTUOID_CAPTURE_CLI";

    /// Where the recorder names the source's registry `verified_version`, for a
    /// driver that installs its CLI: a capture must anchor that version.
    const VERSION_ENV: &str = "PIXTUOID_CAPTURE_VERSION";

    /// A hook can still be in flight when the CLI's own process exits, so the wait
    /// is measured from the LAST PAYLOAD rather than from that exit.
    const SETTLE: Duration = Duration::from_millis(500);
    const QUIET_ROUNDS: u32 = 4;
    const MAX_ROUNDS: u32 = 40;

    /// A hermetic run's overrides: where the capture goes, and what the harvest
    /// and the CLI may see.
    #[derive(Debug, Default, PartialEq)]
    struct Options {
        /// Write the RAW capture here: not stripped, no provenance, no PII scan.
        /// The strip blanks what the decoder under test does not read, which is
        /// exactly where a renamed upstream field lands, so drift evidence must
        /// keep it; these bytes are never committed.
        out: Option<PathBuf>,
        /// Forward each payload, as received, to this socket: a headless pixtuoid
        /// renders the same run the recorder records.
        tee: Option<PathBuf>,
        /// Refuse a harvest root outside this directory, a sandbox the caller
        /// pointed every home into: a resolver that slipped would otherwise
        /// harvest the operator's own transcripts.
        within: Option<PathBuf>,
        /// Set on the CLI after the agent-env scrub, which would strip a
        /// caller's own `CLAUDE_CODE_*` opt-outs with the agent's.
        env: Vec<(String, String)>,
    }

    /// Leading `--out <dir>` / `--tee <socket>` / `--within <dir>` /
    /// `--env KEY=VALUE` pairs, then the positionals.
    fn parse_options(mut args: Vec<String>) -> Result<(Options, Vec<String>), String> {
        let mut options = Options::default();
        while let Some(flag) = args.first().cloned() {
            if !matches!(flag.as_str(), OUT | TEE | WITHIN | ENV) {
                break;
            }
            if args.len() < 2 {
                return Err(format!("{flag} takes a value"));
            }
            let value = args.remove(1);
            args.remove(0);
            match flag.as_str() {
                OUT => options.out = Some(PathBuf::from(value)),
                TEE => options.tee = Some(PathBuf::from(value)),
                WITHIN => options.within = Some(PathBuf::from(value)),
                _ => {
                    let (k, v) = value
                        .split_once('=')
                        .ok_or_else(|| format!("{ENV} takes KEY=VALUE, got {value:?}"))?;
                    options.env.push((k.to_string(), v.to_string()));
                }
            }
        }
        Ok((options, args))
    }

    /// Why `root` may not be harvested from inside `within`, if it may not.
    /// Lexical: every home the caller set is a join under `within`.
    fn harvest_root_refusal(root: Option<&Path>, within: &Path) -> Option<String> {
        let root = root?;
        let escapes = root
            .components()
            .any(|c| c == std::path::Component::ParentDir);
        (escapes || !root.starts_with(within)).then(|| {
            format!(
                "the harvest root {} is outside {}: a home the sandbox did not set",
                root.display(),
                within.display()
            )
        })
    }

    pub(crate) fn main() -> std::io::Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("--strip-corpus") {
            return strip_corpus(&sources_root());
        }
        let (options, args) = match parse_options(args) {
            Ok(parsed) => parsed,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        if args.len() < 3 {
            eprintln!(
                "usage: capture_fixture [--out <dir>] [--tee <socket>] [--within <dir>] [--env K=V]... \
                 <source-id> <scenario> <cmd...>   \
                 ('{{prompt}}' expands)\n       \
                 capture_fixture --strip-corpus   (strip every committed capture in place)"
            );
            std::process::exit(2);
        }
        let (source, scenario) = (args[0].clone(), args[1].clone());
        // Set-but-empty is unset: `CAPTURE_PROMPT=` would otherwise expand
        // `{prompt}` to nothing and record `"prompt": ""` as if it were a choice.
        let prompt = nonempty_env("CAPTURE_PROMPT").unwrap_or_else(|| DEFAULT_PROMPT.to_string());
        let cmd: Vec<String> = args[2..]
            .iter()
            .map(|a| a.replace("{prompt}", &prompt))
            .collect();

        let Some(descriptor) = registry::descriptor_for(&source) else {
            let known: Vec<&str> = registry::registered_source_names().collect();
            eprintln!("no source {source:?} — registered: {}", known.join(", "));
            std::process::exit(2);
        };

        let dest = options
            .out
            .clone()
            .unwrap_or_else(|| scenario_dest(&sources_root(), &source, &scenario));
        let root = resolved_source_root(&source);
        if let Some(why) = options
            .within
            .as_deref()
            .and_then(|w| harvest_root_refusal(root.as_deref(), w))
        {
            eprintln!("{why}");
            std::process::exit(2);
        }

        let workspace = prepare_workspace()?;
        let sandbox = tempfile::tempdir()?;
        let sock = sandbox.path().join("capture.sock");
        let listener = Listener::bind(&sock, options.tee.clone())?;

        println!("capturing {source}/{scenario} — one real model turn");
        let started = SystemTime::now();
        let status = run_cli(
            &cmd,
            &workspace,
            &sock,
            descriptor.verified_version,
            &options.env,
        )?;
        listener.settle();
        let payloads = listener.take();

        std::fs::create_dir_all(&dest)?;
        let mut wrote: Vec<PathBuf> = Vec::new();

        // A source can be BOTH: CC's tool run is in the transcript while its
        // permission gate is a hook event, so the two are harvested together.
        if let Some(root) = root.as_deref() {
            let candidates = pixtuoid_core::harness::transcripts_under(&source, root);
            if let Some(fresh) = newest_born_after(&candidates, started) {
                wrote.push(place_transcript(&fresh, &candidates, &dest)?);
            }
        }
        if !payloads.is_empty() {
            wrote.push(write_payloads(&payloads, &dest)?);
        }

        let named = std::fs::read_to_string(named_cli(&sock)).ok();
        let (cli, version) = identity(&cmd[0], named.as_deref(), probe_version);
        if options.out.is_some() {
            // An empty raw capture is still recorded: a hook that never fired
            // is evidence about the run, which a missing record would hide.
            let record = CaptureRecord {
                workspace: std::fs::canonicalize(&workspace).unwrap_or(workspace),
                cli,
                version,
                exit_code: status.code(),
            };
            std::fs::write(
                dest.join(CAPTURE_RECORD),
                format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;
            for p in &wrote {
                println!("wrote raw {} ({} lines)", p.display(), count_lines(p));
            }
            return Ok(());
        }
        if wrote.is_empty() {
            eprintln!(
                "captured nothing — no hook fired and no transcript was created{}; did the turn run?",
                root.map(|r| format!(" under {}", r.display()))
                    .unwrap_or_default()
            );
            std::process::exit(1);
        }

        let deidentified = match reads::exemption(&dest.join(PROVENANCE)) {
            Some(why) => Deidentified::Exempt(why),
            None => {
                let strip = reads::strip_unread(&source, &wrote)?;
                report_not_json(&strip);
                Deidentified::Stripped(strip.blanked)
            }
        };
        write_provenance(
            &dest,
            (&cli, &version),
            &args[2..],
            &deidentified,
            &[
                ("prompt", nonempty_env("CAPTURE_PROMPT")),
                ("seed", nonempty_env("CAPTURE_SEED")),
            ],
        )?;
        for p in &wrote {
            println!("wrote {} ({} lines)", p.display(), count_lines(p));
        }
        match &deidentified {
            Deidentified::Stripped(n) => println!("blanked {n} subtrees no decoder reads"),
            Deidentified::Exempt(why) => println!("not stripped, as the record declares: {why}"),
        }
        refuse_on_pii(&wrote)?;
        if !status.success() {
            eprintln!("WARNING: the CLI exited {status} — this capture may be truncated");
        }
        println!("next: just test conformance   then   cargo insta review");
        Ok(())
    }

    // ── the decisions ────────────────────────────────────────────────────────────

    /// The agent's own env reaches the CLI under test: a nested `claude` inherited
    /// `CLAUDE_CODE_CHILD_SESSION` and turned its transcript saving OFF, so the run
    /// left nothing to harvest. Scrub the whole namespace, not the one that bit.
    fn agent_env_names<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
        names
            .into_iter()
            .filter(|k| k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_"))
            .collect()
    }

    /// The transcript this run CREATED: BIRTH time, not mtime. A live agent session
    /// is appended to forever and is therefore always newest by mtime — that once
    /// harvested the developer's own live session.
    fn newest_born_after(candidates: &[PathBuf], after: SystemTime) -> Option<PathBuf> {
        candidates
            .iter()
            .filter_map(|p| {
                let born = std::fs::metadata(p).ok()?.created().ok()?;
                (born >= after).then_some((born, p.clone()))
            })
            .max_by_key(|(born, _)| *born)
            .map(|(_, p)| p)
    }

    /// A source whose transcripts all share ONE basename keys its session on the
    /// PARENT dir, so flattening would rename the session after the scenario. Asked
    /// of the source's own file list, never a table of which sources those are.
    fn basename_repeats(name: &std::ffi::OsStr, candidates: &[PathBuf]) -> bool {
        candidates
            .iter()
            .filter(|p| p.file_name() == Some(name))
            .count()
            > 1
    }

    fn place_transcript(
        fresh: &Path,
        candidates: &[PathBuf],
        dest: &Path,
    ) -> std::io::Result<PathBuf> {
        let name = fresh.file_name().expect("a walked file has a name");
        let out = if basename_repeats(name, candidates) {
            let parent = fresh
                .parent()
                .and_then(Path::file_name)
                .expect("a transcript has a parent dir");
            let d = dest.join(parent);
            std::fs::create_dir_all(&d)?;
            d.join(name)
        } else {
            dest.join(name)
        };
        let out = no_clobber(out);
        std::fs::copy(fresh, &out)?;
        Ok(out)
    }

    /// A re-record of an existing scenario is drift EVIDENCE, so it lands beside the
    /// original to be diffed rather than overwriting a committed, redacted capture.
    fn no_clobber(p: PathBuf) -> PathBuf {
        if p.exists() {
            let mut s = p.into_os_string();
            s.push(".new");
            PathBuf::from(s)
        } else {
            p
        }
    }

    // ── plumbing ─────────────────────────────────────────────────────────────────

    /// One connection per event (`transport::send_line` connects, writes one line,
    /// closes), so a whole payload arrives per accept and cannot interleave.
    /// THREADED, and not as an optimization: the shim write-times-out under a
    /// watchdog and a CLI whose hook then fails RETRIES it, so a serial listener
    /// would add copies on top of the re-delivery a CLI already does.
    struct Listener {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl Listener {
        fn bind(sock: &Path, tee: Option<PathBuf>) -> std::io::Result<Self> {
            let listener = UnixListener::bind(sock)?;
            let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&lines);
            let tee = Arc::new(tee);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let sink = Arc::clone(&sink);
                    let tee = Arc::clone(&tee);
                    std::thread::spawn(move || {
                        let mut buf = String::new();
                        if stream.read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
                            if let Some(to) = tee.as_deref() {
                                forward(to, &buf);
                            }
                            sink.lock().expect("listener sink").push(buf);
                        }
                    });
                }
            });
            Ok(Self { lines })
        }

        fn len(&self) -> usize {
            self.lines.lock().expect("listener sink").len()
        }

        fn settle(&self) {
            let (mut prev, mut quiet, mut round) = (usize::MAX, 0, 0);
            while quiet < QUIET_ROUNDS && round < MAX_ROUNDS {
                let n = self.len();
                quiet = if n == prev { quiet + 1 } else { 0 };
                prev = n;
                round += 1;
                std::thread::sleep(SETTLE);
            }
        }

        fn take(&self) -> Vec<String> {
            self.lines.lock().expect("listener sink").clone()
        }
    }

    /// One connection per payload, as the shim itself sends. A tee that cannot
    /// connect is the headless renderer's failure, left in its scene log; the
    /// recording goes on.
    fn forward(to: &Path, payload: &str) {
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(to) {
            let _ = s.write_all(payload.as_bytes());
        }
    }

    fn prepare_workspace() -> std::io::Result<PathBuf> {
        let ws = PathBuf::from(WORKSPACE);
        let base = ws.parent().expect("WORKSPACE has a parent");
        // A fixed name in shared temp is pre-plantable; a foreign owner is refused, not
        // removed — under the sticky bit the remove fails and the capture lands in theirs.
        if base.exists() && !owned_by_us(base) {
            eprintln!(
                "{} exists and is not yours — remove it or run as its owner",
                base.display()
            );
            std::process::exit(2);
        }
        let _ = std::fs::remove_dir_all(base);
        std::fs::create_dir_all(&ws)?;
        std::fs::write(ws.join("NOTE.txt"), "pong\n")?;
        if let Some(seed) = std::env::var_os("CAPTURE_SEED") {
            copy_tree(Path::new(&seed), &ws)?;
        }
        init_repo(&ws)?;
        Ok(ws)
    }

    /// Make `ws` its own repository, every step of which must succeed: the
    /// workspace being one is what stops every CLI's project-root walk there,
    /// so a failed `git init` would let a repository above it in. The commit
    /// does not depend on the operator's signing config.
    fn init_repo(ws: &Path) -> std::io::Result<()> {
        for cmd in [
            vec!["init", "-q"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.email=fixture@pixtuoid",
                "-c",
                "user.name=fixture",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "init",
            ],
        ] {
            git_in(ws, &cmd)?;
        }
        Ok(())
    }

    fn git_in(ws: &Path, args: &[&str]) -> std::io::Result<()> {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(ws)
            .args(args)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "git {args:?} exited {status}"
            )))
        }
    }

    /// A per-CLI ask rule (CC's `.claude/settings.json`, opencode's
    /// `opencode.json`) is what makes a gate fire at all, so it seeds the
    /// sandbox rather than the user's own config.
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let dest = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                std::fs::create_dir_all(&dest)?;
                copy_tree(&entry.path(), &dest)?;
            } else {
                std::fs::copy(entry.path(), &dest)?;
            }
        }
        Ok(())
    }

    /// `symlink_metadata`, not `metadata`: the squat this refuses IS a planted
    /// entry, and following it would stat the target the attacker chose.
    #[cfg(unix)]
    fn owned_by_us(p: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(p)
            .map(|m| m.uid() == rustix::process::getuid().as_raw())
            .unwrap_or(false)
    }

    fn run_cli(
        cmd: &[String],
        ws: &Path,
        sock: &Path,
        verified: &str,
        env: &[(String, String)],
    ) -> std::io::Result<std::process::ExitStatus> {
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..])
            .current_dir(ws)
            .env("PIXTUOID_SOCKET", sock);
        // Transcripts go beside the socket in this run's private sandbox: a fixed
        // shared-temp name is symlink-followable and two concurrent captures interleave.
        if let Some(dir) = sock.parent() {
            c.env("TUIDRIVE_LOG", dir.join("tuidrive.log"));
        }
        c.env(CLI_ENV, named_cli(sock)).env(VERSION_ENV, verified);
        for k in agent_env_names(std::env::vars_os().filter_map(|(k, _)| k.into_string().ok())) {
            c.env_remove(k);
        }
        c.envs(env.iter().map(|(k, v)| (k, v)));
        c.status()
    }

    fn write_payloads(payloads: &[String], dest: &Path) -> std::io::Result<PathBuf> {
        let out = no_clobber(dest.join(HOOK_PAYLOADS));
        let mut f = std::fs::File::create(&out)?;
        for p in payloads {
            // The shim already stamped `_pixtuoid_source`.
            let v: serde_json::Value = serde_json::from_str(p.trim())
                .unwrap_or_else(|e| panic!("a captured payload is not JSON: {e}: {p:?}"));
            writeln!(f, "{}", serde_json::to_string(&v)?)?;
        }
        Ok(out)
    }

    /// Beside the socket, in this run's private sandbox.
    fn named_cli(sock: &Path) -> PathBuf {
        sock.with_file_name("cli")
    }

    /// The CLI a capture ran, and its version, asked of that CLI alone: a
    /// command named by a path is a driver, which `--version` would run whole,
    /// billed and outside the sandbox, so only the CLI it names at [`CLI_ENV`]
    /// is asked.
    fn identity(
        cmd0: &str,
        named: Option<&str>,
        probe: impl FnOnce(&str) -> Option<String>,
    ) -> (String, String) {
        let cli = named
            .and_then(|n| n.lines().next())
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .or_else(|| (!cmd0.contains('/')).then_some(cmd0));
        let name = Path::new(cli.unwrap_or(cmd0))
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_owned();
        let version = cli.and_then(probe).unwrap_or_else(|| "unknown".into());
        (name, version)
    }

    fn probe_version(cli: &str) -> Option<String> {
        std::process::Command::new(cli)
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.trim().to_owned()))
    }

    fn write_provenance(
        dest: &Path,
        (cli, version): (&str, &str),
        raw: &[String],
        deidentified: &Deidentified,
        overrides: &[(&str, Option<String>)],
    ) -> std::io::Result<()> {
        let mut prov = serde_json::json!({
            "origin": "recorded",
            "cli": cli,
            "version": version,
            "captured": today(),
            "command": raw.join(" "),
            "deidentified": deidentified.record(),
        });
        // `command` is the UN-expanded argv, so an override-driven scenario records a
        // `{prompt}` placeholder; written only when set, so committed records stay schema-clean.
        if let Some(map) = prov.as_object_mut() {
            for (key, value) in overrides {
                if let Some(v) = value {
                    map.insert((*key).into(), serde_json::Value::String(v.clone()));
                }
            }
        }
        let out = no_clobber(dest.join(PROVENANCE));
        std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&prov)?))?;
        // Loud, because the record just written is unusable.
        if version == "unknown" {
            eprintln!(
                "WARNING: {} records version \"unknown\" — `{cli} --version` did not \
                 answer, or a driver named no CLI at ${CLI_ENV}. Fix `cli` and `version` \
                 by hand before committing, or the anchor gate skips this source.",
                out.display()
            );
        }
        Ok(())
    }

    fn nonempty_env(key: &str) -> Option<String> {
        pixtuoid_core::platform::text_env(key).filter(|s| !s.trim().is_empty())
    }

    fn today() -> String {
        let secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let days = secs / 86_400;
        let (mut y, mut d) = (1970i64, days as i64);
        loop {
            let len = if leap(y) { 366 } else { 365 };
            if d < len {
                break;
            }
            d -= len;
            y += 1;
        }
        let months = [
            31,
            if leap(y) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut m = 0;
        while d >= months[m] {
            d -= months[m];
            m += 1;
        }
        format!("{y:04}-{:02}-{:02}", m + 1, d + 1)
    }

    fn leap(y: i64) -> bool {
        (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
    }

    /// Strings this machine would leak into a capture, the git identity included:
    /// a CLI that renders `git status` embeds the committer's real name, which no
    /// value shape can match (`.gitleaks-identity.toml` carries the committed-tree half).
    fn identity_needles(git: impl Fn(&str) -> Option<String>) -> BTreeSet<String> {
        let mut needles: BTreeSet<String> = BTreeSet::new();
        for var in ["HOME", "USER", "LOGNAME"] {
            if let Some(v) = pixtuoid_core::platform::text_env(var)
                && !v.is_empty()
            {
                needles.insert(v);
            }
        }
        for key in ["user.name", "user.email"] {
            if let Some(v) = git(key).filter(|v| !v.is_empty()) {
                needles.insert(v);
            }
        }
        needles
    }

    fn git_config(key: &str) -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["config", "--get", key])
            .output()
            .ok()?;
        parse_git_config(&out.stdout)
    }

    /// `git config` answers with a trailing newline, and an unset key answers with
    /// nothing — an untrimmed value never matches the bytes a CLI embedded, and an
    /// empty one would make every capture match the needle.
    fn parse_git_config(stdout: &[u8]) -> Option<String> {
        let v = String::from_utf8_lossy(stdout).trim().to_string();
        (!v.is_empty()).then_some(v)
    }

    /// Every reason `body` cannot be committed, or empty: the operator's own
    /// identity, the one set gitleaks cannot know. Everything with a value shape
    /// is the tree scan's; everything no decoder reads is already gone.
    fn pii_hits(body: &str, needles: &BTreeSet<String>) -> Vec<String> {
        needles
            .iter()
            .filter(|n| body.contains(n.as_str()))
            .map(String::to_string)
            .collect()
    }

    fn scan_for_pii(files: &[PathBuf]) -> Vec<String> {
        let needles = identity_needles(git_config);
        let mut found = Vec::new();
        for f in files {
            let Ok(body) = std::fs::read_to_string(f) else {
                continue;
            };
            let hits = pii_hits(&body, &needles);
            if !hits.is_empty() {
                found.push(format!("{}: {}", f.display(), hits.join(", ")));
            }
        }
        found
    }

    /// A REFUSAL, not a warning. The old form printed to stderr and left the exit
    /// code at 0 on a run that had already written into the repo tree, so the
    /// capturer had to notice a line scrolling past — and twice did not.
    fn refuse_on_pii(files: &[PathBuf]) -> std::io::Result<()> {
        let found = scan_for_pii(files);
        if found.is_empty() {
            return Ok(());
        }
        for line in &found {
            eprintln!("PII: {line}");
        }
        Err(std::io::Error::other(
            "the capture embeds the recorder's own identity — redact those files, \
             then re-run. The bytes are on disk; nothing was discarded.",
        ))
    }

    fn count_lines(p: &Path) -> usize {
        std::fs::read_to_string(p)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    fn sources_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources")
    }

    /// The library strips and records; printing the outcome is the CLI's job.
    fn strip_corpus(root: &Path) -> std::io::Result<()> {
        reads::strip_corpus(root, |capture, strip| {
            report_not_json(strip);
            println!(
                "{}: blanked {}",
                capture
                    .dir
                    .strip_prefix(root)
                    .unwrap_or(&capture.dir)
                    .display(),
                strip.blanked
            );
        })
    }

    fn report_not_json(strip: &reads::Strip) {
        for (file, error) in &strip.not_json {
            eprintln!("{}", not_json_notice(file, error));
        }
    }

    fn not_json_notice(file: &Path, error: &str) -> String {
        format!(
            "not stripping {}: a line is not JSON: {error}",
            file.display()
        )
    }

    /// Where this scenario's bytes ALREADY live, else the conformance root: a module
    /// keeps its own rounds out of conformance's reach (omp's hook keys fold on
    /// Windows), and a re-record sent to the root instead lands as a NEW directory
    /// `conformance.rs` auto-scans, with no committed bytes for `.new` to protect.
    /// Keyed on the SOURCE, never a search across modules: scenario names repeat
    /// (`approval-recorded` is hermes' and omp's), so a search finds the wrong
    /// module's single match and a billed capture lands as `.new` files inside it.
    /// `claude/` owning `claude-code` is the one name mismatch, and it holds no
    /// scenario subdirectory, so it cannot reach here.
    fn scenario_dest(sources: &Path, source: &str, scenario: &str) -> PathBuf {
        let owned = sources.join(source).join("fixtures").join(scenario);
        if owned.join(PROVENANCE).is_file() {
            return owned;
        }
        sources.join("fixtures").join(source).join(scenario)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_workspace_becomes_its_own_repository_or_fails() {
            let d = tempfile::tempdir().expect("tempdir");
            let ws = d.path().join("ws");
            std::fs::create_dir(&ws).expect("mkdir");
            std::fs::write(ws.join("NOTE.txt"), "pong\n").expect("write");
            // An operator config that signs every commit with a failing program.
            let config = d.path().join("gitconfig");
            std::fs::write(
                &config,
                "[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = false\n",
            )
            .expect("config");
            let made = temp_env::with_vars(
                [
                    ("GIT_CONFIG_GLOBAL", Some(config.as_os_str())),
                    ("GIT_CONFIG_NOSYSTEM", Some("1".as_ref())),
                ],
                || init_repo(&ws),
            );
            assert!(made.is_ok(), "{made:?}");
            assert!(git_in(&ws, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok());
            let file = d.path().join("a-file");
            std::fs::write(&file, "").expect("write");
            assert!(init_repo(&file).is_err(), "a failed step fails it");
        }

        #[test]
        fn leading_option_pairs_are_options_and_the_rest_is_positional() {
            let args = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
            let (o, rest) = parse_options(args(&[
                "--tee", "/s", "--out", "/o", "--within", "/w", "--env", "A=b=c", "codex",
                "tool-run", "codex",
            ]))
            .expect("parse");
            assert_eq!(o.out, Some(PathBuf::from("/o")));
            assert_eq!(o.tee, Some(PathBuf::from("/s")));
            assert_eq!(o.within, Some(PathBuf::from("/w")));
            assert_eq!(o.env, [("A".to_string(), "b=c".to_string())]);
            assert_eq!(rest, args(&["codex", "tool-run", "codex"]));
            assert!(
                parse_options(args(&["--env", "NOEQUALS", "codex"])).is_err(),
                "an env pair without its value"
            );
            let (o, rest) = parse_options(args(&["codex", "tool-run", "--out"])).expect("parse");
            assert_eq!(
                o,
                Options::default(),
                "a flag after the positionals is the CLI's"
            );
            assert_eq!(rest.len(), 3);
            assert!(
                parse_options(args(&["--out"])).is_err(),
                "a flag without its path"
            );
        }

        #[test]
        fn a_harvest_root_outside_the_sandbox_is_refused() {
            let within = Path::new("/sandbox");
            assert_eq!(harvest_root_refusal(None, within), None);
            assert_eq!(
                harvest_root_refusal(
                    Some(Path::new("/sandbox/homes/CODEX_HOME/sessions")),
                    within
                ),
                None
            );
            assert!(
                harvest_root_refusal(Some(Path::new("/Users/me/.claude/projects")), within)
                    .is_some()
            );
            assert!(
                harvest_root_refusal(Some(Path::new("/sandbox/../Users/me/.codex")), within)
                    .is_some(),
                "a lexical prefix that climbs out"
            );
            assert!(
                harvest_root_refusal(Some(Path::new("/sandboxed")), within).is_some(),
                "a sibling sharing the prefix's text"
            );
        }

        #[test]
        fn the_listener_forwards_each_payload_to_the_tee_as_received() {
            let d = tempfile::tempdir().expect("tempdir");
            let tee = d.path().join("tee.sock");
            let downstream = UnixListener::bind(&tee).expect("bind tee");
            let sock = d.path().join("capture.sock");
            let listener = Listener::bind(&sock, Some(tee)).expect("bind");
            let payload = "{\"hook_event_name\":\"SessionStart\"}\n";
            std::os::unix::net::UnixStream::connect(&sock)
                .expect("connect")
                .write_all(payload.as_bytes())
                .expect("send");
            let (mut got, _) = downstream.accept().expect("forwarded");
            let mut buf = String::new();
            got.read_to_string(&mut buf).expect("read");
            assert_eq!(buf, payload);
            listener.settle();
            assert_eq!(
                listener.take(),
                vec![payload.to_string()],
                "recorded as well"
            );
        }

        #[test]
        fn the_not_json_notice_names_the_file_and_the_parse_error() {
            let notice = not_json_notice(
                Path::new("/x/hook-payloads.jsonl"),
                "expected value at line 2",
            );
            assert_eq!(
                notice,
                "not stripping /x/hook-payloads.jsonl: a line is not JSON: expected value at line 2"
            );
        }

        #[test]
        fn a_module_owned_scenario_is_re_recorded_where_it_lives() {
            let d = tempfile::tempdir().expect("tempdir");
            let root = d.path();
            let owned = root.join("omp/fixtures/bridge-run-recorded");
            std::fs::create_dir_all(&owned).expect("mkdir");
            std::fs::write(owned.join("provenance.json"), "{}").expect("write");

            assert_eq!(scenario_dest(root, "omp", "bridge-run-recorded"), owned);
            assert_eq!(
                scenario_dest(root, "omp", "tool-run-recorded"),
                root.join("fixtures/omp/tool-run-recorded")
            );
            assert_eq!(
                scenario_dest(root, "kimi", "tool-run"),
                root.join("fixtures/kimi/tool-run")
            );
            std::fs::create_dir_all(root.join("kimi/fixtures/tool-run")).expect("mkdir");
            assert_eq!(
                scenario_dest(root, "kimi", "tool-run"),
                root.join("fixtures/kimi/tool-run")
            );

            let mine = root.join("omp/fixtures/approval-recorded");
            std::fs::create_dir_all(&mine).expect("mkdir");
            std::fs::write(mine.join("provenance.json"), "{}").expect("write");
            assert_eq!(scenario_dest(root, "omp", "approval-recorded"), mine);
            assert_eq!(
                scenario_dest(root, "hermes", "approval-recorded"),
                root.join("fixtures/hermes/approval-recorded")
            );
        }

        #[test]
        fn a_git_config_answer_is_trimmed_and_an_unset_key_is_none() {
            assert_eq!(
                parse_git_config(b"Ada Lovelace\n").as_deref(),
                Some("Ada Lovelace")
            );
            assert_eq!(
                parse_git_config(b"  ada@example.org  ").as_deref(),
                Some("ada@example.org")
            );
            assert_eq!(parse_git_config(b""), None);
            assert_eq!(parse_git_config(b"\n"), None);
        }

        /// Injected rather than read from the machine: on a CI runner `git config
        /// user.name` is unset, so a test asserting the real one passes VACUOUSLY
        /// exactly where it is most needed.
        #[test]
        fn the_git_identity_joins_the_needles() {
            let needles = identity_needles(|k| match k {
                "user.name" => Some("Ada Lovelace".to_string()),
                "user.email" => Some("ada@example.org".to_string()),
                _ => None,
            });
            assert!(needles.contains("Ada Lovelace"), "{needles:?}");
            assert!(needles.contains("ada@example.org"), "{needles:?}");
            assert!(!identity_needles(|_| None).contains("Ada Lovelace"));
        }

        /// A name has no shape to match, so it is refused where the recorder knows
        /// it — this machine's own git identity — and nowhere else. The tree half is
        /// `.gitleaks-identity.toml`'s label-anchored rule.
        #[test]
        fn the_operators_own_git_identity_is_refused_wherever_it_is_rendered() {
            let needles = BTreeSet::from(["Ada Lovelace".to_string()]);
            let body = r#"{"gitStatus":"Current branch: main\nGit user: Ada Lovelace"}"#;
            assert_eq!(pii_hits(body, &needles), vec!["Ada Lovelace".to_string()]);
        }

        #[test]
        fn a_capture_carrying_nothing_of_the_operator_passes() {
            let needles = BTreeSet::from(["Ada Lovelace".to_string(), "/Users/ada".to_string()]);
            let body = r#"{"cwd":"/tmp/pixtuoid-capture/proj","tool_use_id":"t1"}"#;
            assert!(pii_hits(body, &needles).is_empty(), "{body}");
        }

        #[test]
        fn the_env_scrub_names_the_agent_namespace_and_nothing_else() {
            let got = agent_env_names(
                [
                    "CLAUDECODE",
                    "CLAUDE_CODE_CHILD_SESSION",
                    "CLAUDE_CONFIG_DIR",
                    "PATH",
                    "HOME",
                ]
                .map(String::from),
            );
            assert_eq!(got, vec!["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION"]);
        }

        #[test]
        fn birth_time_selection_ignores_a_file_that_predates_the_run() {
            let d = tempfile::tempdir().expect("tempdir");
            let (old, new) = (d.path().join("old.jsonl"), d.path().join("new.jsonl"));
            std::fs::write(&old, "{}").expect("write");
            let started = SystemTime::now();
            std::thread::sleep(Duration::from_millis(1100));
            std::fs::write(&new, "{}").expect("write");
            let candidates = vec![old.clone(), new.clone()];
            assert_eq!(newest_born_after(&candidates, started), Some(new));
            // The live-session trap: touching the old file must NOT make it win,
            // which is the whole reason this reads birth time and not mtime.
            std::fs::write(&old, "{}{}").expect("touch");
            assert_ne!(newest_born_after(&candidates, started), Some(old));
        }

        #[test]
        fn no_candidate_born_after_the_run_is_none_not_a_panic() {
            let d = tempfile::tempdir().expect("tempdir");
            let old = d.path().join("old.jsonl");
            std::fs::write(&old, "{}").expect("write");
            std::thread::sleep(Duration::from_millis(1100));
            assert_eq!(newest_born_after(&[old], SystemTime::now()), None);
        }

        #[test]
        fn a_repeated_basename_means_the_parent_dir_is_the_identity() {
            let repeated = [
                PathBuf::from("a/updates.jsonl"),
                PathBuf::from("b/updates.jsonl"),
            ];
            let unique = [PathBuf::from("a/one.jsonl"), PathBuf::from("b/two.jsonl")];
            let name = std::ffi::OsStr::new("updates.jsonl");
            assert!(basename_repeats(name, &repeated));
            assert!(!basename_repeats(
                std::ffi::OsStr::new("one.jsonl"),
                &unique
            ));
        }

        #[test]
        fn a_re_record_lands_beside_the_committed_one_instead_of_over_it() {
            let d = tempfile::tempdir().expect("tempdir");
            let p = d.path().join("hook-payloads.jsonl");
            assert_eq!(no_clobber(p.clone()), p, "a fresh path is used as-is");
            std::fs::write(&p, "{}").expect("write");
            assert!(no_clobber(p).to_string_lossy().ends_with(".new"));
        }

        #[test]
        fn birth_time_selection_scales_past_the_size_that_broke_the_shell_version() {
            // A pin on the SHAPE, not on a buffer size.
            let d = tempfile::tempdir().expect("tempdir");
            let mut all: Vec<PathBuf> = (0..1200)
                .map(|i| {
                    let p = d.path().join(format!("f{i}.jsonl"));
                    std::fs::write(&p, "{}").expect("write");
                    p
                })
                .collect();
            // Created last, and placed FIRST, so returning `candidates.first()`
            // or swapping max for min are both caught.
            std::thread::sleep(std::time::Duration::from_millis(20));
            let newest = d.path().join("zzz-newest.jsonl");
            std::fs::write(&newest, "{}").expect("write");
            all.insert(0, newest.clone());
            assert_eq!(
                newest_born_after(&all, SystemTime::UNIX_EPOCH),
                Some(newest)
            );
        }

        /// A link PLANTED by another user cannot be created in a unit test, so this
        /// pins the discriminator instead: a link WE own pointing at a root-owned dir
        /// reads as ours — `fs::metadata` would stat the target and read as root's,
        /// which is how a planted link would have slipped past the refusal.
        #[test]
        fn ownership_is_judged_on_the_link_itself_not_on_what_it_points_at() {
            let d = tempfile::tempdir().expect("tempdir");
            let link = d.path().join("to-root-owned");
            std::os::unix::fs::symlink("/usr", &link).expect("symlink");
            assert!(
                owned_by_us(&link),
                "the link is ours; only its target belongs to root"
            );
        }

        #[test]
        fn provenance_records_an_override_only_when_it_was_actually_set() {
            // Both directions: committed records predate these fields and must stay
            // schema-clean, so an unset override writes nothing at all.
            let read = |d: &Path| -> serde_json::Value {
                serde_json::from_str(
                    &std::fs::read_to_string(d.join("provenance.json")).expect("read"),
                )
                .expect("json")
            };
            let argv = ["true".to_string()];

            let bare = tempfile::tempdir().expect("tempdir");
            write_provenance(
                bare.path(),
                ("true", "1.0"),
                &argv,
                &Deidentified::Stripped(0),
                &[("prompt", None)],
            )
            .expect("write");
            assert!(read(bare.path()).get("prompt").is_none());

            let set = tempfile::tempdir().expect("tempdir");
            write_provenance(
                set.path(),
                ("true", "1.0"),
                &argv,
                &Deidentified::Stripped(0),
                &[("prompt", Some("read NOTE.txt".into()))],
            )
            .expect("write");
            assert_eq!(read(set.path())["prompt"], "read NOTE.txt");
        }

        #[test]
        fn the_seed_lands_in_the_sandbox_including_a_dotted_config_dir() {
            let src = tempfile::tempdir().expect("src");
            let dst = tempfile::tempdir().expect("dst");
            std::fs::create_dir_all(src.path().join(".claude")).expect("mkdir");
            std::fs::write(src.path().join(".claude/settings.json"), "{}").expect("write");
            std::fs::write(src.path().join("opencode.json"), "{}").expect("write");

            copy_tree(src.path(), dst.path()).expect("copy");

            assert!(dst.path().join(".claude/settings.json").is_file());
            assert!(dst.path().join("opencode.json").is_file());
        }

        #[test]
        fn only_the_cli_is_asked_its_version_never_its_driver() {
            let asked = std::cell::RefCell::new(Vec::new());
            let probe = |c: &str| {
                asked.borrow_mut().push(c.to_owned());
                Some(format!("{c} 1.0"))
            };
            let driver = "scripts/lib/capture-x.sh";
            assert_eq!(
                identity(driver, Some("dsh\n"), probe),
                ("dsh".into(), "dsh 1.0".into())
            );
            assert_eq!(
                identity(driver, Some("/tmp/oc/package/bin/opencode\n"), probe),
                ("opencode".into(), "/tmp/oc/package/bin/opencode 1.0".into())
            );
            assert_eq!(
                identity(driver, None, probe),
                ("capture-x.sh".into(), "unknown".into())
            );
            assert_eq!(
                identity("claude", None, probe),
                ("claude".into(), "claude 1.0".into())
            );
            assert_eq!(
                identity("grok", None, |_| None),
                ("grok".into(), "unknown".into())
            );
            assert_eq!(
                *asked.borrow(),
                ["dsh", "/tmp/oc/package/bin/opencode", "claude"]
            );
        }

        #[test]
        fn a_driver_gets_the_verified_version_and_its_callers_env_and_names_its_cli() {
            let sandbox = tempfile::tempdir().expect("sandbox");
            let ws = tempfile::tempdir().expect("workspace");
            let sock = sandbox.path().join("capture.sock");
            // An inherited agent var is scrubbed; a caller's `CLAUDE_CODE_*`
            // opt-out outlives the scrub.
            let script = format!(
                r#"test "${VERSION_ENV}" = 1.0.0 && test -z "$CLAUDE_CODE_CHILD_SESSION" \
                   && test "$CLAUDE_CODE_PROBE" = 1 && echo dsh >"${CLI_ENV}""#
            );
            let cmd = ["sh", "-c", &script].map(String::from);
            let env = [("CLAUDE_CODE_PROBE".to_string(), "1".to_string())];
            // The host holds the probe too, so only scrub-then-set yields its 1.
            let ran = temp_env::with_vars(
                [
                    ("CLAUDE_CODE_CHILD_SESSION", Some("1")),
                    ("CLAUDE_CODE_PROBE", Some("0")),
                ],
                || run_cli(&cmd, ws.path(), &sock, "1.0.0", &env).expect("runs"),
            );
            assert!(ran.success());
            assert_eq!(
                std::fs::read_to_string(named_cli(&sock)).ok().as_deref(),
                Some("dsh\n")
            );
        }

        /// The recorder's env name crosses into every driver, each a copy a rename
        /// would strand: its capture would then record version "unknown".
        #[test]
        fn every_driver_names_its_cli_where_the_recorder_reads_it() {
            let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib");
            let mut drivers = 0;
            for entry in std::fs::read_dir(&lib).expect("scripts/lib") {
                let path = entry.expect("an entry").path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.starts_with("capture-") || name == "tuidrive.py" {
                    drivers += 1;
                    let text = std::fs::read_to_string(&path).expect("a driver");
                    assert!(text.contains(CLI_ENV), "{name} names no CLI at ${CLI_ENV}");
                }
            }
            assert!(drivers > 0, "scripts/lib holds the drivers");
        }

        #[test]
        fn today_is_an_iso_date_the_provenance_gate_can_parse() {
            let t = today();
            assert_eq!(t.len(), 10, "{t}");
            let parts: Vec<&str> = t.split('-').collect();
            assert_eq!(parts.len(), 3);
            assert!(parts[0].parse::<u32>().expect("year") >= 2026);
            assert!((1..=12).contains(&parts[1].parse::<u32>().expect("month")));
            assert!((1..=31).contains(&parts[2].parse::<u32>().expect("day")));
        }
    }
}
