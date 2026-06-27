#!/usr/bin/env python3
"""Self-test for check_dod.py — pins EVERY pure parser/check on both sides of its
threshold so a regression can't silently make the gate find nothing ("DoD met"
by absence — the #283/#384 silent-monitor-death class). Pure, no network. Run:
`python3 scripts/check_dod_selftest.py` (exit 0 = pass) — also CI-gated in the
hygiene job AND as a prerequisite of the definition-of-done job."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_dod as d  # noqa: E402


def run() -> int:
    fails: list = []

    def check(name: str, cond: bool) -> None:
        if not cond:
            fails.append(name)

    def n_of(check_name: str, viols: list) -> int:
        return sum(1 for v in viols if v.check == check_name)

    # --- classify_changed_files -------------------------------------------- #
    docs = d.classify_changed_files("M\tREADME.md\nM\tdocs/ARCHITECTURE.md")
    check("class docs_only", docs.docs_only and not docs.code_touched)
    code = d.classify_changed_files("M\tcrates/pixtuoid/src/main.rs")
    check("class code_touched", code.code_touched and not code.docs_only)
    pub = d.classify_changed_files("M\tcrates/pixtuoid/src/cli.rs")
    check("class public_surface", pub.public_surface)
    feat = d.classify_changed_files(
        "M\tcrates/a/src/x.rs\nM\tcrates/a/src/y.rs\nM\tcrates/a/src/z.rs"
    )
    check("class feature_shaped (>=3)", feat.feature_shaped)
    ren = d.classify_changed_files("R100\tcrates/a/src/old.rs\tcrates/a/src/new.rs")
    check("class rename takes dest", "crates/a/src/new.rs" in ren.files)
    spr = d.classify_changed_files("M\tcrates/pixtuoid-scene/sprites/default/x.sprite")
    check("class sprite", spr.sprite)

    # --- added_lines ------------------------------------------------------- #
    diff = (
        "diff --git a/crates/p/src/f.rs b/crates/p/src/f.rs\n"
        "--- a/crates/p/src/f.rs\n+++ b/crates/p/src/f.rs\n"
        "@@ -1,1 +1,2 @@\n fn x() {}\n+let z = 1;\n"
    )
    al = d.added_lines(diff)
    check("added_lines path+text", al.get("crates/p/src/f.rs") == [(2, "let z = 1;")])

    # --- prod_print_violations (BLOCKING) ---------------------------------- #
    prod = {"crates/pixtuoid/src/foo.rs": [(10, '    println!("hi");')]}
    check("print: prod println flagged", n_of("prod-print", d.prod_print_violations(prod)) == 1)
    pragma = {"crates/pixtuoid/src/foo.rs": [(10, 'println!("hi"); // dod:allow(println) — banner')]}
    check("print: pragma exempts", d.prod_print_violations(pragma) == [])
    test = {"crates/pixtuoid/tests/foo.rs": [(10, '    println!("hi");')]}
    check("print: test path exempt", d.prod_print_violations(test) == [])
    exa = {"crates/pixtuoid/examples/snap.rs": [(10, '    println!("hi");')]}
    check("print: examples exempt", d.prod_print_violations(exa) == [])
    ep = {"crates/pixtuoid/src/foo.rs": [(3, '    eprintln!("e");')]}
    check("print: eprintln flagged", n_of("prod-print", d.prod_print_violations(ep)) == 1)
    cli = {"crates/pixtuoid/src/validate.rs": [(8, '    println!("OK");')]}
    check("print: CLI-presenter exempt", d.prod_print_violations(cli) == [])

    # --- settings_write_violations (BLOCKING) ------------------------------ #
    bad = {"crates/pixtuoid/src/cli.rs": [(5, 'fs::write("settings.json", x)?;')]}
    check("settings: write outside io.rs flagged", n_of("settings-write", d.settings_write_violations(bad)) == 1)
    okio = {d.SETTINGS_WRITE_ALLOWED: [(5, 'fs::write("settings.json", x)?;')]}
    check("settings: io.rs allowed", d.settings_write_violations(okio) == [])
    tst = {"crates/pixtuoid/tests/x.rs": [(5, 'fs::write("settings.json", x)?;')]}
    check("settings: test exempt", d.settings_write_violations(tst) == [])
    tmp = {"crates/pixtuoid/src/cli.rs": [(5, 'fs::write(tmp.join("settings.json"), x)?;')]}
    check("settings: tempdir exempt", d.settings_write_violations(tmp) == [])

    # --- no-verify (BLOCKING) ---------------------------------------------- #
    check("no-verify cmd: --no-verify", d.command_has_no_verify("git commit --no-verify -m x"))
    check("no-verify cmd: SKIP_PREFLIGHT", d.command_has_no_verify("SKIP_PREFLIGHT=1 git push"))
    check("no-verify cmd: commit -n", d.command_has_no_verify("git commit -n -m x"))
    check("no-verify cmd: clean push ok", not d.command_has_no_verify("git push origin main"))
    nv = {"scripts/x.sh": [(2, "git commit --no-verify")]}
    check("no-verify: committed flagged", n_of("no-verify", d.no_verify_violations(nv)) == 1)
    nvdoc = {".githooks/pre-push": [(2, "# never pass --no-verify here")]}
    check("no-verify: doc comment exempt", d.no_verify_violations(nvdoc) == [])

    # --- public_surface / docs-currency (WARN) ----------------------------- #
    cls = d.classify_changed_files("M\tcrates/pixtuoid/src/cli.rs")
    newarm = {"crates/pixtuoid/src/cli.rs": [(40, "    Doctor {")]}
    pv = d.public_surface_violations(cls, newarm)
    check("docs-currency: new arm flagged (warn)", n_of("docs-currency", pv) == 1 and pv[0].severity == "warn")
    withdocs = {
        "crates/pixtuoid/src/cli.rs": [(40, "    Doctor {")],
        "README.md": [(5, "## doctor")],
        "crates/pixtuoid/CLAUDE.md": [(5, "doctor note")],
    }
    check("docs-currency: docs present clears", d.public_surface_violations(cls, withdocs) == [])

    # --- path_sep (WARN) --------------------------------------------------- #
    ps = {"crates/p/src/x.rs": [(9, 'assert_eq!(p.to_string_lossy(), "/home/u/c");')]}
    check("path-sep flagged", n_of("path-sep", d.path_sep_warnings(ps)) == 1)
    psok = {"crates/p/src/x.rs": [(9, "assert_eq!(p, PathBuf::from(expected));")]}
    check("path-sep: structural ok", d.path_sep_warnings(psok) == [])

    # --- two-lens ---------------------------------------------------------- #
    body2 = ("Two-lens-review:\n- correctness: APPROVE (conf 85)\n"
             "- design/blast-radius: REQUEST-CHANGES (conf 70)\n")
    check("two-lens: 2 distinct lenses ok", d.two_lens_ok(body2))
    body1 = "Two-lens-review:\n- correctness: APPROVE (conf 85)\n"
    check("two-lens: 1 lens fails", not d.two_lens_ok(body1))
    bodydup = ("Two-lens-review:\n- correctness: APPROVE (conf 85)\n"
               "- correctness: APPROVE (conf 80)\n")
    check("two-lens: duplicate label fails", not d.two_lens_ok(bodydup))
    check("two-lens: no block fails", not d.two_lens_ok("just a normal PR body"))

    # --- closes / deferrals ------------------------------------------------ #
    check("closes_keywords", d.closes_keywords("Closes #12. Also fixes #34 and resolves #7") == [7, 12, 34])
    check("deferral no issue flagged", len(d.deferrals_have_issue("- defer the perf work")) == 1)
    check("deferral with issue ok", d.deferrals_have_issue("- deferred to #5 (perf)") == [])

    # --- attestation / bypass ---------------------------------------------- #
    feat_priv = d.classify_changed_files(
        "M\tcrates/a/src/x.rs\nM\tcrates/a/src/y.rs\nM\tcrates/a/src/z.rs"
    )
    gaps = d.attestation_gaps(feat_priv, "")
    check("attestation: feature requires 4 boxes", len(gaps) == 4)
    full = ("- [x] TDD: yes\n- [x] Self-review: yes\n- [x] Design: yes\n"
            "- [x] Impl-plan: yes\n")
    check("attestation: all ticked clears", d.attestation_gaps(feat_priv, full) == [])
    check("attestation: docs-only none", d.attestation_gaps(docs, "") == [])
    check("bypass: env", d.bypass_reason("", {"DOD_BYPASS": "hotfix"}) == "hotfix")
    check("bypass: md line", d.bypass_reason("DOD-BYPASS: ci is down\n", {}) == "ci is down")
    check("bypass: none", d.bypass_reason("nothing here", {}) is None)

    # --- two-lens: confidence is OPTIONAL (review #I) ---------------------- #
    body_optconf = ("Two-lens-review:\n- correctness: APPROVE (conf 85)\n"
                    "- design/blast-radius: REQUEST-CHANGES\n")
    check("two-lens: confidence optional", d.two_lens_ok(body_optconf))
    body_badconf = ("Two-lens-review:\n- correctness: APPROVE (conf 999)\n"
                    "- design: REQUEST-CHANGES (conf 999)\n")
    check("two-lens: garbage >100 confidence doesn't count", not d.two_lens_ok(body_badconf))
    body_4digit = ("Two-lens-review:\n- correctness: APPROVE (conf 9999)\n"
                   "- design: REQUEST-CHANGES (conf 9999)\n")
    check("two-lens: 4-digit out-of-range conf also excluded", not d.two_lens_ok(body_4digit))
    body_parens = ("Two-lens-review:\n- correctness (grounding): APPROVE\n"
                   "- design/blast-radius: REQUEST-CHANGES\n")
    check("two-lens: label with parens still parses", d.two_lens_ok(body_parens))

    # --- no-verify: scoping + quote/flag precision (review HIGH-1, C) ------- #
    # The gate must NOT scan source files (else it flags its own regexes/strings).
    rs_token = {"scripts/check_dod.py": [(1, '_NOVERIFY = re.compile(r"--no-verify")')]}
    check("no-verify: source file out of scope (no self-trip)", d.no_verify_violations(rs_token) == [])
    rs_real = {"crates/p/src/x.rs": [(1, 'Command::new("git").arg("--no-verify");')]}
    check("no-verify: .rs out of scope (PreToolUse covers transient)", d.no_verify_violations(rs_real) == [])
    for fp in ["echo 'never pass --no-verify'", "git commit -m 'fix the -n flag bug'",
               "grep -rn SKIP_PREFLIGHT=1 .", "git -C /repo push"]:
        check(f"no-verify cmd FP: {fp}", not d.command_has_no_verify(fp))
    for fn in ["git commit -nm 'msg'", "git commit --no-verify", "SKIP_PREFLIGHT=1 just x",
               "git -c core.hooksPath=/dev/null commit -n",  # global opt before commit
               "export SKIP_PREFLIGHT=1; git push"]:          # export-assignment
        check(f"no-verify cmd hit: {fn}", d.command_has_no_verify(fn))

    # --- is_push tolerates leading git global options (review F) ------------ #
    for c in ["git push", "git -C /repo push", "git -c http.x=y push", 'git -c x="a b" push']:
        check(f"is_push: {c}", d._has_push(c))
    check("is_push: 'git config push.default' not a push", not d._has_push("git config push.default x"))
    check("is_push: 'git log' not a push", not d._has_push("git log --oneline"))
    # CHAINED commands — the agent's canonical forms. The tokenizer must see a push /
    # commit -n in ANY git invocation, not just the first (regression vs the old
    # re.search-anywhere; review R0626-DOD-17).
    for c in ["git add . && git push", "git add -A && git commit -m x && git push",
              "git status; git push", "git fetch && git push --force"]:
        check(f"is_push chained: {c}", d._has_push(c))
    for c in ["git add . && git commit -nm x", "git status; git commit -n -m msg",
              "git add -A && git commit -nm 'wip'"]:
        check(f"no-verify chained: {c}", d.command_has_no_verify(c))
    # but a `;`/`&&` INSIDE a quoted message is not a real separator
    check("no-verify: separator inside quoted msg ignored",
          not d._has_push("git commit -m 'fix; then push later'"))
    # py/redos regression: must not backtrack on a pathological `-- -- --` run.
    import time as _t
    _t0 = _t.time()
    check("is_push: pathological '-- ' run terminates fast",
          d._has_push("git " + "-- " * 8000 + "push") and (_t.time() - _t0) < 0.5)
    _t1 = _t.time()
    check("has_no_verify: pathological run terminates fast",
          (d.command_has_no_verify("git " + "-- " * 8000 + "commit -n") is True) and (_t.time() - _t1) < 0.5)
    check("merge: gh pr merge detected", bool(d._MERGE_RE.search("gh pr merge 5 --squash")))

    # --- prod_print / settings: syntax-blind FP fixes (review D, E) --------- #
    for text in ["/// use println! for debugging", "// TODO: replace println! with tracing",
                 '    let s = "call println! here";']:
        fp = {"crates/p/src/x.rs": [(1, text)]}
        check(f"prod_print FP fixed: {text[:24]}", d.prod_print_violations(fp) == [])
    real_print = {"crates/p/src/x.rs": [(1, '    println!("real");')]}
    check("prod_print still has teeth", len(d.prod_print_violations(real_print)) == 1)
    swc = {"crates/p/src/x.rs": [(1, "// never fs::write to settings.json directly")]}
    check("settings_write: comment not flagged", d.settings_write_violations(swc) == [])

    # --- added_lines: a +++ content line is not a header (review H) --------- #
    diff_plusplus = (
        "diff --git a/crates/p/src/x.rs b/crates/p/src/x.rs\n"
        "--- a/crates/p/src/x.rs\n+++ b/crates/p/src/x.rs\n"
        "@@ -0,0 +1,2 @@\n+++ banner separator\n+    println!(\"leaked\");\n"
    )
    al2 = d.added_lines(diff_plusplus)
    check("added_lines keeps file after +++ content line",
          any("println" in t for _, t in al2.get("crates/p/src/x.rs", [])))

    # --- name_status_from_diff: real A/M/D (review K) ----------------------- #
    nsd = (
        "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+x\n"
        "diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-y\n"
        "diff --git a/mod.rs b/mod.rs\n--- a/mod.rs\n+++ b/mod.rs\n@@ -1 +1 @@\n-a\n+b\n"
    )
    ns = d.name_status_from_diff(nsd)
    check("name_status: added file => A", "A\tnew.rs" in ns)
    check("name_status: deleted file => D", "D\tgone.rs" in ns)
    check("name_status: modified file => M", "M\tmod.rs" in ns)

    # --- _DOC_RE: a .rs file named like a doc is code, not docs (review L) -- #
    rdme = d.classify_changed_files("M\tcrates/p/src/readme_gen.rs")
    check("doc-name anchored: readme_gen.rs is code not docs", rdme.code_touched and not rdme.docs_only)
    # match-arm idents don't read as new subcommands (warn noise)
    arms = {"crates/pixtuoid/src/cli.rs": [(1, "    Some(x) => y,"), (2, "    Ok(v) => v,")]}
    cls_cli = d.classify_changed_files("M\tcrates/pixtuoid/src/cli.rs")
    check("public_surface: Some/Ok arms not flagged", d.public_surface_violations(cls_cli, arms) == [])

    # --- sanitizer / verdict ----------------------------------------------- #
    check("strip_control", d._strip_control("a\x1b[31mb\x07c") == "a[31mbc")
    blk = [d.Violation("x", "blocking", "w", "d")]
    wrn = [d.Violation("y", "warn", "w", "d")]
    check("verdict: blocking => 1", d.verdict(blk) == 1)
    check("verdict: warn => 0", d.verdict(wrn) == 0)
    check("verdict: empty => 0", d.verdict([]) == 0)

    if fails:
        print("check_dod selftest FAILED:")
        for f in fails:
            print(f"  ✗ {f}")
        return 1
    print("check_dod selftest: all assertions passed.")
    return 0


if __name__ == "__main__":
    sys.exit(run())
