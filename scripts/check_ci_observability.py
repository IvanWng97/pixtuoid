#!/usr/bin/env python3
"""Check repository contracts that keep advisory CI failures observable."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


def read_text(root: Path, relative: str) -> tuple[str, list[str]]:
    path = root / relative
    try:
        return path.read_text(encoding="utf-8"), []
    except FileNotFoundError:
        return "", [f"{relative} is missing"]


def active_lines(text: str, comment_prefixes: tuple[str, ...] = ("#",)) -> list[str]:
    return [
        line
        for line in text.splitlines()
        if line.strip()
        and not any(line.lstrip().startswith(prefix) for prefix in comment_prefixes)
    ]


def active_code_lines(text: str) -> list[str]:
    without_block_comments = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return active_lines(without_block_comments, ("//", "#"))


def yaml_steps(text: str) -> list[list[str]]:
    lines = active_lines(text)
    starts = [
        (index, len(line) - len(line.lstrip()))
        for index, line in enumerate(lines)
        if line.lstrip().startswith("- ")
    ]
    if not starts:
        return []
    step_indent = min(indent for _, indent in starts)
    step_starts = [index for index, indent in starts if indent == step_indent]
    return [
        lines[start : step_starts[position + 1]]
        if position + 1 < len(step_starts)
        else lines[start:]
        for position, start in enumerate(step_starts)
    ]


def find_step(steps: list[list[str]], pattern: str) -> list[str]:
    return next(
        (
            step
            for step in steps
            if any(re.search(pattern, line) for line in step)
        ),
        [],
    )


def check_ascii_codecov_config(root: Path) -> list[str]:
    path = root / "codecov.yml"
    try:
        path.read_bytes().decode("ascii")
    except FileNotFoundError:
        return ["codecov.yml is missing"]
    except UnicodeDecodeError as error:
        return [
            "codecov.yml must be ASCII-only for the Windows Codecov CLI "
            f"(non-ASCII byte at offset {error.start})"
        ]
    return []


def check_codecov_upload_contract(root: Path) -> list[str]:
    action_path = ".github/actions/upload-codecov/action.yml"
    workflow_path = ".github/workflows/ci-tests.yml"
    action, errors = read_text(root, action_path)
    workflow, workflow_errors = read_text(root, workflow_path)
    errors.extend(workflow_errors)
    if errors:
        return errors

    action_steps = yaml_steps(action)
    validation_step = find_step(
        action_steps, r"\bscripts/validate_ci_report\.py\b"
    )
    upload_step = find_step(
        action_steps, r"^\s*uses:\s*codecov/codecov-action@v7\s*$"
    )
    warning_step = find_step(
        action_steps, r"steps\.upload\.outcome\s*==\s*'failure'"
    )
    required_action_lines = (
        (
            validation_step,
            '-s "$REPORT_FILE"',
            r'-s "\$REPORT_FILE"',
        ),
        (
            validation_step,
            "scripts/validate_ci_report.py",
            r"\bscripts/validate_ci_report\.py\b",
        ),
        (
            upload_step,
            "continue-on-error: true",
            r"^\s*continue-on-error:\s*true\s*$",
        ),
        (
            upload_step,
            "uses: codecov/codecov-action@v7",
            r"^\s*uses:\s*codecov/codecov-action@v7\s*$",
        ),
        (
            upload_step,
            "files: ${{ inputs.file }}",
            r"^\s*files:\s*\$\{\{\s*inputs\.file\s*\}\}\s*$",
        ),
        (
            upload_step,
            "report_type: ${{ inputs.report_type }}",
            r"^\s*report_type:\s*\$\{\{\s*inputs\.report_type\s*\}\}\s*$",
        ),
        (
            upload_step,
            "disable_search: true",
            r"^\s*disable_search:\s*true\s*$",
        ),
        (
            upload_step,
            "fail_ci_if_error: true",
            r"^\s*fail_ci_if_error:\s*true\s*$",
        ),
        (
            warning_step,
            "steps.upload.outcome == 'failure'",
            r"steps\.upload\.outcome\s*==\s*'failure'",
        ),
        (warning_step, "::warning", r"::warning"),
        (warning_step, "GITHUB_STEP_SUMMARY", r"\bGITHUB_STEP_SUMMARY\b"),
    )
    for step, label, pattern in required_action_lines:
        if not any(re.search(pattern, line) for line in step):
            errors.append(f"{action_path} must contain active `{label}`")

    workflows_dir = root / ".github" / "workflows"
    workflow_files = sorted(
        {*workflows_dir.glob("*.yml"), *workflows_dir.glob("*.yaml")}
    )
    for path in workflow_files:
        candidate = path.read_text(encoding="utf-8")
        candidate_lines = active_lines(candidate)
        relative = path.relative_to(root).as_posix()
        if any("codecov/codecov-action@" in line for line in candidate_lines):
            errors.append(
                f"{relative} Codecov uploads must be centralized through "
                "./.github/actions/upload-codecov"
            )
        if any(re.match(r"^\s*report-type\s*:", line) for line in candidate_lines):
            errors.append(
                f"{relative} uses invalid `report-type`; use `report_type`"
            )

    workflow_lines = active_lines(workflow)
    local_uploads = sum(
        line.strip().removeprefix("- ").strip()
        == "uses: ./.github/actions/upload-codecov"
        for line in workflow_lines
    )
    if local_uploads != 6:
        errors.append(
            f"{workflow_path} must contain 6 centralized Codecov uploads "
            f"(found {local_uploads})"
        )
    for report_type, expected in (("coverage", 3), ("test_results", 3)):
        actual = sum(
            line.strip() == f"report_type: {report_type}"
            for line in workflow_lines
        )
        if actual != expected:
            errors.append(
                f"{workflow_path} must contain {expected} "
                f"`report_type: {report_type}` uploads (found {actual})"
            )
    return errors


def check_lighthouse_artifact_contract(root: Path) -> list[str]:
    workflow_path = ".github/workflows/site.yml"
    workflow, errors = read_text(root, workflow_path)
    if errors:
        return errors

    lines = workflow.splitlines()
    target = "path: site/.lighthouseci/"
    positions = [index for index, line in enumerate(lines) if line.strip() == target]
    if len(positions) != 1:
        return [
            f"{workflow_path} must contain exactly one `{target}` "
            f"(found {len(positions)})"
        ]

    start = positions[0]
    target_indent = len(lines[start]) - len(lines[start].lstrip())
    step_start = next(
        (
            index
            for index in range(start, -1, -1)
            if lines[index].lstrip().startswith("- ")
            and len(lines[index]) - len(lines[index].lstrip()) < target_indent
        ),
        start,
    )
    step_indent = len(lines[step_start]) - len(lines[step_start].lstrip())
    step_end = len(lines)
    for index in range(start + 1, len(lines)):
        stripped = lines[index].lstrip()
        indent = len(lines[index]) - len(stripped)
        if stripped.startswith("- ") and indent <= step_indent:
            step_end = index
            break
    step_lines = active_lines("\n".join(lines[step_start:step_end]))
    stripped_step_lines = {line.strip() for line in step_lines}
    for fragment in (
        "if: ${{ !cancelled() }}",
        "include-hidden-files: true",
        "if-no-files-found: error",
    ):
        if fragment not in stripped_step_lines:
            errors.append(
                f"{workflow_path}'s Lighthouse artifact block must contain "
                f"active `{fragment}`"
            )
    return errors


def check_code_templates_are_parseable(root: Path) -> list[str]:
    errors: list[str] = []
    pairs = (
        (
            "crates/pixtuoid/src/install/opencode_plugin.ts",
            'const HOOK_PATH: string = "{{HOOK_PATH_JSON}}"',
            "crates/pixtuoid/src/install/opencode.rs",
        ),
        (
            "crates/pixtuoid/src/install/openclaw_plugin.js",
            'const HOOK_PATH = "{{HOOK_PATH_JSON}}";',
            "crates/pixtuoid/src/install/openclaw.rs",
        ),
    )
    for template_path, expected_binding, rust_path in pairs:
        template, read_errors = read_text(root, template_path)
        errors.extend(read_errors)
        if read_errors:
            continue
        template_lines = active_code_lines(template)
        if expected_binding not in (line.strip() for line in template_lines):
            errors.append(
                f"{template_path} must keep the exact hook binding inside a string "
                "literal so it is valid source before rendering"
            )

        rust_source, rust_errors = read_text(root, rust_path)
        errors.extend(rust_errors)
        if rust_errors:
            continue
        expected_authority = (
            'const HOOK_PLACEHOLDER: &str = "\\"{{HOOK_PATH_JSON}}\\"";'
        )
        if expected_authority not in (
            line.strip() for line in active_code_lines(rust_source)
        ):
            errors.append(
                f"{rust_path} must keep the quoted placeholder authority paired "
                f"with {template_path}"
            )
    return errors


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", nargs="?", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)

    root = args.root.resolve()
    errors = [
        *check_ascii_codecov_config(root),
        *check_codecov_upload_contract(root),
        *check_lighthouse_artifact_contract(root),
        *check_code_templates_are_parseable(root),
    ]
    if errors:
        for error in errors:
            print(f"ci-observability: {error}", file=sys.stderr)
        return 1
    print("ci-observability: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
