#!/usr/bin/env python3
"""Process-boundary tests for the CI observability contract checker."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECKER = ROOT / "scripts" / "check_ci_observability.py"


def good_files() -> dict[str, str]:
    upload_calls = "\n".join(
        textwrap.dedent(
            f"""\
            - uses: ./.github/actions/upload-codecov
              with:
                file: {file}
                flag: {flag}
                report_type: {report_type}
            """
        )
        for file, flag, report_type in (
            ("target/nextest/ci/junit.xml", "windows", "test_results"),
            ("target/nextest/ci/junit.xml", "macos", "test_results"),
            ("lcov.info", "unit", "coverage"),
            ("target/nextest/ci/junit.xml", "unit", "test_results"),
            ("lcov.info", "windows", "coverage"),
            ("lcov.info", "macos", "coverage"),
        )
    )
    return {
        "codecov.yml": "coverage: {}\n",
        ".github/actions/upload-codecov/action.yml": textwrap.dedent(
            """\
            name: upload-codecov
            inputs:
              file:
                required: true
              flag:
                required: true
              report_type:
                required: true
            runs:
              using: composite
              steps:
                - shell: bash
                  run: test -s "$REPORT_FILE"
                - id: upload
                  continue-on-error: true
                  uses: codecov/codecov-action@v7
                  with:
                    files: ${{ inputs.file }}
                    flags: ${{ inputs.flag }}
                    report_type: ${{ inputs.report_type }}
                    disable_search: true
                    fail_ci_if_error: true
                - if: steps.upload.outcome == 'failure'
                  shell: bash
                  run: |
                    echo "::warning title=Codecov upload failed::upload failed"
                    echo "Codecov upload failed" >> "$GITHUB_STEP_SUMMARY"
            """
        ),
        ".github/workflows/ci-tests.yml": upload_calls,
        ".github/workflows/site.yml": textwrap.dedent(
            """\
            - uses: actions/upload-artifact@v7
              if: ${{ !cancelled() }}
              with:
                path: site/.lighthouseci/
                include-hidden-files: true
                if-no-files-found: error
            """
        ),
        "crates/pixtuoid/src/install/opencode_plugin.ts": (
            'const HOOK_PATH = "{{HOOK_PATH_JSON}}"\n'
        ),
        "crates/pixtuoid/src/install/openclaw_plugin.js": (
            'const HOOK_PATH = "{{HOOK_PATH_JSON}}";\n'
        ),
    }


def run_checker(files: dict[str, str]) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for relative, content in files.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        return subprocess.run(
            [sys.executable, str(CHECKER), str(root)],
            check=False,
            capture_output=True,
            text=True,
        )


class CiObservabilityContractTests(unittest.TestCase):
    def test_accepts_complete_contract(self) -> None:
        result = run_checker(good_files())

        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_non_ascii_codecov_config(self) -> None:
        files = good_files()
        files["codecov.yml"] = "# left arrow \u2190\ncoverage: {}\n"
        result = run_checker(files)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ASCII", result.stderr)

    def test_rejects_direct_codecov_use_and_wrong_input_name(self) -> None:
        files = good_files()
        files[".github/workflows/rogue.yml"] = textwrap.dedent(
            """\
            - uses: codecov/codecov-action@v7
              with:
                report-type: test_results
            """
        )
        result = run_checker(files)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("centralized", result.stderr)
        self.assertIn("report-type", result.stderr)

    def test_rejects_wrapper_contract_regressions(self) -> None:
        mutations = (
            ("fail_ci_if_error: true", "fail_ci_if_error: false"),
            ("disable_search: true", "disable_search: false"),
            ("continue-on-error: true", "continue-on-error: false"),
            ('-s "$REPORT_FILE"', '"$REPORT_FILE"'),
            ("GITHUB_STEP_SUMMARY", "GITHUB_OUTPUT"),
        )
        for required, replacement in mutations:
            with self.subTest(required=required):
                files = good_files()
                files[".github/actions/upload-codecov/action.yml"] = files[
                    ".github/actions/upload-codecov/action.yml"
                ].replace(required, replacement)
                result = run_checker(files)

                self.assertNotEqual(result.returncode, 0)
                self.assertIn(required, result.stderr)

    def test_rejects_a_hidden_lighthouse_artifact_that_can_disappear(self) -> None:
        files = good_files()
        files[".github/workflows/site.yml"] = files[
            ".github/workflows/site.yml"
        ].replace("include-hidden-files: true\n", "").replace(
            "if-no-files-found: error", "if-no-files-found: ignore"
        )
        files[".github/workflows/site.yml"] += textwrap.dedent(
            """\
            - uses: actions/upload-artifact@v7
              with:
                path: public/
                include-hidden-files: true
                if-no-files-found: error
            """
        )
        result = run_checker(files)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("include-hidden-files: true", result.stderr)
        self.assertIn("if-no-files-found: error", result.stderr)

    def test_rejects_lighthouse_upload_after_cancellation(self) -> None:
        files = good_files()
        files[".github/workflows/site.yml"] = files[
            ".github/workflows/site.yml"
        ].replace("if: ${{ !cancelled() }}", "if: always()")
        result = run_checker(files)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("if: ${{ !cancelled() }}", result.stderr)

    def test_rejects_code_templates_that_are_invalid_before_rendering(self) -> None:
        files = good_files()
        files["crates/pixtuoid/src/install/opencode_plugin.ts"] = (
            "const HOOK_PATH = {{HOOK_PATH_JSON}}\n"
        )
        result = run_checker(files)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("valid source before rendering", result.stderr)


if __name__ == "__main__":
    unittest.main()
