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

    use pixtuoid_core::source::{registry, resolved_source_root};

    /// One prompt for every source, so captures stay comparable: reading the SAME
    /// file twice around a list forces both shapes the composed fixtures got wrong —
    /// tools that interleave, and a tool id that repeats. `CAPTURE_PROMPT` overrides
    /// it for a scenario the shared one cannot reach (a permission gate needs a tool
    /// the CLI refuses to run unasked).
    const DEFAULT_PROMPT: &str =
        "Read NOTE.txt, then list this directory, then read NOTE.txt again.";

    /// The workspace path is FIXED and generic on purpose: every payload embeds its
    /// own cwd and transcript_path, so capturing somewhere already generic is what
    /// lets the bytes ship unedited. A random sandbox would bake a per-run path into
    /// them.
    const WORKSPACE: &str = "/tmp/pixtuoid-capture/proj";

    /// A hook can still be in flight when the CLI's own process exits, so the wait
    /// is measured from the LAST PAYLOAD rather than from that exit.
    const SETTLE: Duration = Duration::from_millis(500);
    const QUIET_ROUNDS: u32 = 4;
    const MAX_ROUNDS: u32 = 40;

    pub(crate) fn main() -> std::io::Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.len() < 3 {
            eprintln!(
                "usage: capture_fixture <source-id> <scenario> <cmd...>   ('{{prompt}}' expands)"
            );
            std::process::exit(2);
        }
        let (source, scenario) = (args[0].clone(), args[1].clone());
        let prompt = std::env::var("CAPTURE_PROMPT").unwrap_or_else(|_| DEFAULT_PROMPT.to_string());
        let cmd: Vec<String> = args[2..]
            .iter()
            .map(|a| a.replace("{prompt}", &prompt))
            .collect();

        if registry::descriptor_for(&source).is_none() {
            let known: Vec<&str> = registry::registered_source_names().collect();
            eprintln!("no source {source:?} — registered: {}", known.join(", "));
            std::process::exit(2);
        }

        let dest = fixtures_root().join(&source).join(&scenario);
        let root = resolved_source_root(&source);

        let workspace = prepare_workspace()?;
        let sandbox = tempfile::tempdir()?;
        let sock = sandbox.path().join("capture.sock");
        let listener = Listener::bind(&sock)?;

        println!("capturing {source}/{scenario} — one real model turn");
        let started = SystemTime::now();
        let status = run_cli(&cmd, &workspace, &sock)?;
        listener.settle();
        let payloads = listener.take();

        std::fs::create_dir_all(&dest)?;
        let mut wrote: Vec<PathBuf> = Vec::new();

        if let Some(root) = root.as_deref() {
            // A source can be BOTH: CC's tool run is in the transcript while its
            // permission gate is a hook event, so the two are harvested together.
            let candidates = pixtuoid_core::harness::transcripts_under(&source, root);
            match newest_born_after(&candidates, started) {
                Some(fresh) => wrote.push(place_transcript(&fresh, &candidates, &dest)?),
                None if payloads.is_empty() => {
                    eprintln!(
                        "captured nothing — no transcript created under {} and no hook fired; \
                         did the turn run?",
                        root.display()
                    );
                    std::process::exit(1);
                }
                None => {}
            }
        }
        if !payloads.is_empty() {
            wrote.push(write_payloads(&payloads, &dest)?);
        }
        if wrote.is_empty() {
            eprintln!("captured nothing — the CLI fired no hook and wrote no transcript");
            std::process::exit(1);
        }

        write_provenance(
            &dest,
            &cmd,
            &args[2..],
            &[
                ("prompt", std::env::var("CAPTURE_PROMPT").ok()),
                ("seed", std::env::var("CAPTURE_SEED").ok()),
            ],
        )?;
        for p in &wrote {
            println!("wrote {} ({} lines)", p.display(), count_lines(p));
        }
        warn_on_pii(&wrote);
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
        fn bind(sock: &Path) -> std::io::Result<Self> {
            let listener = UnixListener::bind(sock)?;
            let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&lines);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let sink = Arc::clone(&sink);
                    std::thread::spawn(move || {
                        let mut buf = String::new();
                        if stream.read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
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

    fn prepare_workspace() -> std::io::Result<PathBuf> {
        let ws = PathBuf::from(WORKSPACE);
        let base = ws.parent().expect("WORKSPACE has a parent");
        // A fixed name in shared temp is pre-plantable, so a foreign owner is
        // REFUSED rather than removed: under /tmp's sticky bit the remove would fail
        // and the capture would land inside their directory.
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
        for cmd in [
            vec!["init", "-q"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.email=fixture@pixtuoid",
                "-c",
                "user.name=fixture",
                "commit",
                "-qm",
                "init",
            ],
        ] {
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&ws)
                .args(cmd)
                .status();
        }
        Ok(ws)
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
    ) -> std::io::Result<std::process::ExitStatus> {
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..])
            .current_dir(ws)
            .env("PIXTUOID_SOCKET", sock);
        // The driver and the per-CLI cycle scripts write their transcripts beside the
        // socket, in this run's private sandbox — a fixed shared-temp name is
        // symlink-followable and two concurrent captures would interleave into it.
        if let Some(dir) = sock.parent() {
            c.env("TUIDRIVE_LOG", dir.join("tuidrive.log"));
        }
        for k in agent_env_names(std::env::vars().map(|(k, _)| k)) {
            c.env_remove(k);
        }
        c.status()
    }

    fn write_payloads(payloads: &[String], dest: &Path) -> std::io::Result<PathBuf> {
        let out = no_clobber(dest.join("hook-payloads.jsonl"));
        let mut f = std::fs::File::create(&out)?;
        for p in payloads {
            // The shim already stamped `_pixtuoid_source`; nothing here edits the bytes.
            let v: serde_json::Value = serde_json::from_str(p.trim())
                .unwrap_or_else(|e| panic!("a captured payload is not JSON: {e}: {p:?}"));
            writeln!(f, "{}", serde_json::to_string(&v)?)?;
        }
        Ok(out)
    }

    fn write_provenance(
        dest: &Path,
        cmd: &[String],
        raw: &[String],
        overrides: &[(&str, Option<String>)],
    ) -> std::io::Result<()> {
        let cli = Path::new(&cmd[0])
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let version = std::process::Command::new(&cmd[0])
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        let mut prov = serde_json::json!({
            "origin": "recorded",
            "cli": cli,
            "version": version.trim(),
            "captured": today(),
            "command": raw.join(" "),
        });
        // `command` is the UN-expanded argv, so a scenario driven by an override
        // records a `{prompt}` placeholder and nothing else says what ran. Only
        // written when set, so the already-committed records stay schema-clean.
        if let Some(map) = prov.as_object_mut() {
            for (key, value) in overrides {
                if let Some(v) = value {
                    map.insert((*key).into(), serde_json::Value::String(v.clone()));
                }
            }
        }
        let out = no_clobber(dest.join("provenance.json"));
        std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&prov)?))?;
        if cli.ends_with("sh") {
            eprintln!(
                "NOTE: {} names {cli} — a driver script cannot probe the CLI; fix cli/version by hand",
                out.display()
            );
        }
        Ok(())
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

    /// PII is not always a key you can drop — kimi's arrived as the owner column
    /// inside a captured `ls -la`, and a CLI's ACCOUNT identity is a different
    /// namespace from the host's (`user_email` slipped past a `$HOME|$USER` grep).
    fn warn_on_pii(files: &[PathBuf]) {
        let mut needles: BTreeSet<String> = BTreeSet::new();
        for var in ["HOME", "USER", "LOGNAME"] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    needles.insert(v);
                }
            }
        }
        for f in files {
            let Ok(body) = std::fs::read_to_string(f) else {
                continue;
            };
            let mut hits: Vec<&str> = needles
                .iter()
                .filter(|n| body.contains(n.as_str()))
                .map(String::as_str)
                .collect();
            if body.contains("@") && body.contains("\"user_email\"") {
                hits.push("an account email");
            }
            if !hits.is_empty() {
                eprintln!(
                    "WARNING: {} embeds {} — redact before committing",
                    f.display(),
                    hits.join(", ")
                );
            }
        }
    }

    fn count_lines(p: &Path) -> usize {
        std::fs::read_to_string(p)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    fn fixtures_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources/fixtures")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

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

        #[test]
        fn ownership_is_judged_on_the_link_itself_not_on_what_it_points_at() {
            // The squat this refuses is a link PLANTED by another user, which a
            // unit test cannot create — so pin the discriminator instead: a link
            // WE own pointing at a root-owned dir must read as ours. Under
            // `fs::metadata` it would stat the target and read as root's, which
            // is how a planted link would have slipped past the refusal.
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
            // Both directions: the already-committed records predate these
            // fields and must stay schema-clean, so an unset override writes
            // nothing at all.
            let read = |d: &Path| -> serde_json::Value {
                serde_json::from_str(
                    &std::fs::read_to_string(d.join("provenance.json")).expect("read"),
                )
                .expect("json")
            };
            let argv = ["true".to_string()];

            let bare = tempfile::tempdir().expect("tempdir");
            write_provenance(bare.path(), &argv, &argv, &[("prompt", None)]).expect("write");
            assert!(read(bare.path()).get("prompt").is_none());

            let set = tempfile::tempdir().expect("tempdir");
            write_provenance(
                set.path(),
                &argv,
                &argv,
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
