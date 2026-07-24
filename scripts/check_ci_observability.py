#!/usr/bin/env python3
"""Check repository contracts that keep advisory CI failures observable."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def read_text(root: Path, relative: str) -> tuple[str, list[str]]:
    path = root / relative
    try:
        return path.read_text(encoding="utf-8"), []
    except FileNotFoundError:
        return "", [f"{relative} is missing"]


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

    required_action_fragments = (
        '-s "$REPORT_FILE"',
        "continue-on-error: true",
        "uses: codecov/codecov-action@v7",
        "report_type: ${{ inputs.report_type }}",
        "disable_search: true",
        "fail_ci_if_error: true",
        "steps.upload.outcome == 'failure'",
        "::warning",
        "GITHUB_STEP_SUMMARY",
    )
    for fragment in required_action_fragments:
        if fragment not in action:
            errors.append(f"{action_path} must contain `{fragment}`")

    workflows_dir = root / ".github" / "workflows"
    workflow_files = sorted(
        {*workflows_dir.glob("*.yml"), *workflows_dir.glob("*.yaml")}
    )
    for path in workflow_files:
        candidate = path.read_text(encoding="utf-8")
        relative = path.relative_to(root).as_posix()
        if "codecov/codecov-action@" in candidate:
            errors.append(
                f"{relative} Codecov uploads must be centralized through "
                "./.github/actions/upload-codecov"
            )
        if "report-type:" in candidate:
            errors.append(
                f"{relative} uses invalid `report-type`; use `report_type`"
            )

    local_uploads = workflow.count("uses: ./.github/actions/upload-codecov")
    if local_uploads != 6:
        errors.append(
            f"{workflow_path} must contain 6 centralized Codecov uploads "
            f"(found {local_uploads})"
        )
    for report_type, expected in (("coverage", 3), ("test_results", 3)):
        actual = workflow.count(f"report_type: {report_type}")
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
    step_text = "\n".join(line.strip() for line in lines[step_start:step_end])
    for fragment in (
        "if: ${{ !cancelled() }}",
        "include-hidden-files: true",
        "if-no-files-found: error",
    ):
        if fragment not in step_text:
            errors.append(
                f"{workflow_path}'s Lighthouse artifact block must contain "
                f"`{fragment}`"
            )
    return errors


def check_code_templates_are_parseable(root: Path) -> list[str]:
    errors: list[str] = []
    for relative in (
        "crates/pixtuoid/src/install/opencode_plugin.ts",
        "crates/pixtuoid/src/install/openclaw_plugin.js",
    ):
        template, read_errors = read_text(root, relative)
        errors.extend(read_errors)
        if read_errors:
            continue
        if '"{{HOOK_PATH_JSON}}"' not in template:
            errors.append(
                f"{relative} must keep the hook placeholder inside a string "
                "literal so it is valid source before rendering"
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
