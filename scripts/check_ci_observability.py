#!/usr/bin/env python3
"""Check repository contracts that keep advisory CI failures observable."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class YamlLine:
    index: int
    indent: int
    content: str


@dataclass(frozen=True)
class YamlEntry:
    line: YamlLine
    indent: int
    key: str
    value: str
    parent: int | None


@dataclass(frozen=True)
class YamlItem:
    source_lines: tuple[str, ...]
    entries: tuple[YamlEntry, ...]

    @property
    def start_index(self) -> int:
        return min(entry.line.index for entry in self.entries)

    def direct_values(self, key: str) -> list[str]:
        return [
            entry.value
            for entry in self.entries
            if entry.parent is None and entry.key == key
        ]

    def direct_value(self, key: str) -> str | None:
        values = self.direct_values(key)
        return values[0] if len(values) == 1 else None

    def child_values(self, parent_key: str, key: str) -> list[str]:
        parent_indexes = {
            index
            for index, entry in enumerate(self.entries)
            if entry.parent is None and entry.key == parent_key
        }
        return [
            entry.value
            for entry in self.entries
            if entry.parent in parent_indexes and entry.key == key
        ]

    def child_value(self, parent_key: str, key: str) -> str | None:
        values = self.child_values(parent_key, key)
        return values[0] if len(values) == 1 else None

    def scalar(self, key: str) -> str:
        candidates = [
            entry
            for entry in self.entries
            if entry.parent is None
            and entry.key == key
            and re.fullmatch(r"[|>][0-9+-]*", entry.value)
        ]
        if len(candidates) != 1:
            return ""

        entry = candidates[0]
        content: list[str] = []
        for line in self.source_lines[entry.line.index + 1 :]:
            stripped = line.lstrip(" ")
            if stripped and len(line) - len(stripped) <= entry.indent:
                break
            content.append(line)
        return "\n".join(content)


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


def structural_yaml_lines(text: str) -> list[YamlLine]:
    structural: list[YamlLine] = []
    block_scalar_indent: int | None = None
    for index, line in enumerate(text.splitlines()):
        stripped = line.lstrip(" ")
        indent = len(line) - len(stripped)
        if block_scalar_indent is not None:
            if not stripped or indent > block_scalar_indent:
                continue
            block_scalar_indent = None
        if not stripped or stripped.startswith("#"):
            continue
        yaml_line = YamlLine(index=index, indent=indent, content=stripped)
        structural.append(yaml_line)
        if re.search(r":\s*[|>][0-9+-]*\s*(?:#.*)?$", stripped):
            block_scalar_indent = indent
    return structural


def parse_mapping(content: str) -> tuple[str, str] | None:
    match = re.fullmatch(r"([A-Za-z0-9_-]+)\s*:\s*(.*?)\s*", content)
    if match is None:
        return None
    return match.group(1), match.group(2)


def yaml_items(text: str) -> list[YamlItem]:
    source_lines = tuple(text.splitlines())
    structural = structural_yaml_lines(text)
    items: list[YamlItem] = []
    for position, first_line in enumerate(structural):
        if not first_line.content.startswith("- "):
            continue

        body: list[tuple[YamlLine, int, str]] = [
            (
                first_line,
                first_line.indent + 2,
                first_line.content.removeprefix("- ").strip(),
            )
        ]
        for line in structural[position + 1 :]:
            if line.indent <= first_line.indent:
                break
            if not line.content.startswith("- "):
                body.append((line, line.indent, line.content))

        entries: list[YamlEntry] = []
        ancestors: list[int] = []
        for line, indent, content in body:
            mapping = parse_mapping(content)
            if mapping is None:
                continue
            while ancestors and entries[ancestors[-1]].indent >= indent:
                ancestors.pop()
            parent = ancestors[-1] if ancestors else None
            key, value = mapping
            entries.append(
                YamlEntry(
                    line=line,
                    indent=indent,
                    key=key,
                    value=value,
                    parent=parent,
                )
            )
            ancestors.append(len(entries) - 1)
        items.append(YamlItem(source_lines=source_lines, entries=tuple(entries)))
    return items


def yaml_mapping_entries(text: str) -> list[YamlEntry]:
    entries: list[YamlEntry] = []
    ancestors: list[int] = []
    for line in structural_yaml_lines(text):
        content = line.content
        indent = line.indent
        if content.startswith("- "):
            content = content.removeprefix("- ").strip()
            indent += 2
        mapping = parse_mapping(content)
        if mapping is None:
            continue
        while ancestors and entries[ancestors[-1]].indent >= indent:
            ancestors.pop()
        parent = ancestors[-1] if ancestors else None
        key, value = mapping
        entries.append(
            YamlEntry(
                line=line,
                indent=indent,
                key=key,
                value=value,
                parent=parent,
            )
        )
        ancestors.append(len(entries) - 1)
    return entries


def yaml_values_at(text: str, path: tuple[str, ...]) -> list[str]:
    entries = yaml_mapping_entries(text)
    values: list[str] = []
    for index, entry in enumerate(entries):
        keys: list[str] = []
        cursor: int | None = index
        while cursor is not None:
            keys.append(entries[cursor].key)
            cursor = entries[cursor].parent
        if tuple(reversed(keys)) == path:
            values.append(entry.value)
    return values


def items_using(text: str, action: str) -> list[YamlItem]:
    return [
        item for item in yaml_items(text) if item.direct_value("uses") == action
    ]


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

    action_items = yaml_items(action)
    validation_items = [
        item
        for item in action_items
        if "scripts/validate_ci_report.py" in item.scalar("run")
        and item.direct_value("if") is None
    ]
    if len(validation_items) != 1:
        errors.append(
            f"{action_path} must contain exactly one unconditional report "
            "validation step running `scripts/validate_ci_report.py` "
            f"(found {len(validation_items)})"
        )
        validation_step = None
    else:
        validation_step = validation_items[0]

    upload_items = items_using(action, "codecov/codecov-action@v7")
    if len(upload_items) != 1:
        errors.append(
            f"{action_path} must contain exactly one direct "
            f"`uses: codecov/codecov-action@v7` step (found {len(upload_items)})"
        )
        upload_step = None
    else:
        upload_step = upload_items[0]

    warning_items = [
        item
        for item in action_items
        if item.direct_value("if")
        in (
            "steps.upload.outcome == 'failure'",
            "${{ steps.upload.outcome == 'failure' }}",
        )
    ]
    if len(warning_items) != 1:
        errors.append(
            f"{action_path} must contain exactly one upload-failure warning "
            "step with active `::warning` and `GITHUB_STEP_SUMMARY` output "
            f"(found {len(warning_items)})"
        )
        warning_step = None
    else:
        warning_step = warning_items[0]

    if validation_step is not None:
        validation_run = validation_step.scalar("run")
        if not re.search(r'-s "\$REPORT_FILE"', validation_run):
            errors.append(f'{action_path} must contain active `-s "$REPORT_FILE"`')

    if upload_step is not None:
        expected_upload_values = (
            (
                upload_step.direct_value("continue-on-error"),
                "true",
                "continue-on-error: true",
            ),
            (
                upload_step.child_value("with", "files"),
                "${{ inputs.file }}",
                "files: ${{ inputs.file }}",
            ),
            (
                upload_step.child_value("with", "report_type"),
                "${{ inputs.report_type }}",
                "report_type: ${{ inputs.report_type }}",
            ),
            (
                upload_step.child_value("with", "disable_search"),
                "true",
                "disable_search: true",
            ),
            (
                upload_step.child_value("with", "fail_ci_if_error"),
                "true",
                "fail_ci_if_error: true",
            ),
        )
        for actual, expected, label in expected_upload_values:
            if actual != expected:
                errors.append(f"{action_path} must contain active `{label}`")
        if upload_step.direct_value("if") is not None:
            errors.append(
                f"{action_path}'s Codecov upload step must be unconditional"
            )

    if warning_step is not None:
        warning_run = warning_step.scalar("run")
        for label in ("::warning", "GITHUB_STEP_SUMMARY"):
            if label not in warning_run:
                errors.append(f"{action_path} must contain active `{label}`")

    workflows_dir = root / ".github" / "workflows"
    workflow_files = sorted(
        {*workflows_dir.glob("*.yml"), *workflows_dir.glob("*.yaml")}
    )
    actions_dir = root / ".github" / "actions"
    action_files = sorted(
        {
            *actions_dir.glob("**/action.yml"),
            *actions_dir.glob("**/action.yaml"),
        }
    )
    authority_path = root / action_path
    for path in [*workflow_files, *action_files]:
        if path == authority_path:
            continue
        candidate = path.read_text(encoding="utf-8")
        relative = path.relative_to(root).as_posix()
        for item in yaml_items(candidate):
            uses = item.direct_value("uses")
            if uses is not None and uses.startswith("codecov/codecov-action@"):
                errors.append(
                    f"{relative} Codecov uploads must be centralized through "
                    "./.github/actions/upload-codecov"
                )
            if (
                uses is not None
                and (
                    uses == "./.github/actions/upload-codecov"
                    or uses.startswith("codecov/codecov-action@")
                )
                and item.child_values("with", "report-type")
            ):
                errors.append(
                    f"{relative} uses invalid `report-type`; use `report_type`"
                )

    local_uploads = items_using(
        workflow, "./.github/actions/upload-codecov"
    )
    if len(local_uploads) != 6:
        errors.append(
            f"{workflow_path} must contain 6 centralized Codecov uploads "
            f"(found {len(local_uploads)})"
        )
    for report_type, expected in (("coverage", 3), ("test_results", 3)):
        actual = sum(
            item.child_value("with", "report_type") == report_type
            for item in local_uploads
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

    target = "site/.lighthouseci/"
    upload_items = [
        item
        for item in items_using(workflow, "actions/upload-artifact@v7")
        if item.child_value("with", "path") == target
    ]
    if len(upload_items) != 1:
        return [
            f"{workflow_path} must contain exactly one `path: {target}` "
            f"artifact upload (found {len(upload_items)})"
        ]

    upload_step = upload_items[0]
    expected_values = (
        (
            upload_step.direct_value("if"),
            "${{ !cancelled() }}",
            "if: ${{ !cancelled() }}",
        ),
        (
            upload_step.child_value("with", "include-hidden-files"),
            "true",
            "include-hidden-files: true",
        ),
        (
            upload_step.child_value("with", "if-no-files-found"),
            "error",
            "if-no-files-found: error",
        ),
    )
    for actual, expected, fragment in expected_values:
        if actual != expected:
            errors.append(
                f"{workflow_path}'s Lighthouse artifact block must contain "
                f"active `{fragment}`"
            )
    return errors


def check_codeql_contract(root: Path) -> list[str]:
    workflow_path = ".github/workflows/codeql.yml"
    workflow, errors = read_text(root, workflow_path)
    if errors:
        return errors

    expected_languages = (
        "actions",
        "javascript-typescript",
        "python",
        "rust",
    )
    language_values = yaml_values_at(
        workflow,
        ("jobs", "analyze", "strategy", "matrix", "language"),
    )
    actual_languages: tuple[str, ...] = ()
    if len(language_values) == 1:
        value = language_values[0]
        if value.startswith("[") and value.endswith("]"):
            actual_languages = tuple(
                language.strip().strip("'\"")
                for language in value[1:-1].split(",")
                if language.strip()
            )
    if actual_languages != expected_languages:
        errors.append(
            f"{workflow_path} must explicitly analyze "
            "`[actions, javascript-typescript, python, rust]`"
        )

    expected_paths = (
        (("on", "push", "branches"), "[main]", "push to main"),
        (("on", "pull_request"), "", "pull requests"),
        (("on", "workflow_dispatch"), "", "manual dispatch"),
        (
            ("jobs", "analyze", "strategy", "fail-fast"),
            "false",
            "fail-fast: false",
        ),
        (("jobs", "analyze", "runs-on"), "ubuntu-latest", "ubuntu-latest"),
        (
            ("jobs", "analyze", "timeout-minutes"),
            "30",
            "timeout-minutes: 30",
        ),
        (("permissions", "actions"), "read", "actions: read"),
        (("permissions", "contents"), "read", "contents: read"),
        (("permissions", "packages"), "read", "packages: read"),
        (
            ("permissions", "security-events"),
            "write",
            "security-events: write",
        ),
    )
    for path, expected, label in expected_paths:
        if yaml_values_at(workflow, path) != [expected]:
            errors.append(f"{workflow_path} must configure active `{label}`")
    if not yaml_values_at(workflow, ("on", "schedule", "cron")):
        errors.append(f"{workflow_path} must retain a weekly `schedule`")

    items = yaml_items(workflow)
    checkout_steps = items_using(workflow, "actions/checkout@v7")
    init_steps = items_using(workflow, "github/codeql-action/init@v4")
    analyze_steps = items_using(workflow, "github/codeql-action/analyze@v4")
    rust_steps = [
        item
        for item in items
        if item.direct_value("if")
        in (
            "matrix.language == 'rust'",
            "${{ matrix.language == 'rust' }}",
        )
        and item.scalar("run")
    ]
    expected_counts = (
        (checkout_steps, "actions/checkout@v7"),
        (init_steps, "github/codeql-action/init@v4"),
        (analyze_steps, "github/codeql-action/analyze@v4"),
        (rust_steps, "if: ${{ matrix.language == 'rust' }}"),
    )
    for steps, label in expected_counts:
        if len(steps) != 1:
            errors.append(
                f"{workflow_path} must contain exactly one active `{label}` "
                f"step (found {len(steps)})"
            )

    rust_step = rust_steps[0] if len(rust_steps) == 1 else None
    init_step = init_steps[0] if len(init_steps) == 1 else None
    if rust_step is not None:
        rust_run = rust_step.scalar("run")
        for required in (
            "rustup component add rust-src --toolchain stable",
            'test -s "$rust_source"',
            "CODEQL_EXTRACTOR_RUST_OPTION_CARGO_ALL_TARGETS=true",
        ):
            if required not in rust_run:
                errors.append(
                    f"{workflow_path} must contain active `{required}`"
                )
    if init_step is not None:
        init_values = (
            (
                init_step.child_value("with", "languages"),
                "${{ matrix.language }}",
                "languages: ${{ matrix.language }}",
            ),
            (
                init_step.child_value("with", "build-mode"),
                "none",
                "build-mode: none",
            ),
        )
        for actual, expected, label in init_values:
            if actual != expected:
                errors.append(f"{workflow_path} must contain active `{label}`")
    if rust_step is not None and init_step is not None:
        if rust_step.start_index >= init_step.start_index:
            errors.append(
                f"{workflow_path} must prepare Rust semantic inputs before "
                "initializing CodeQL"
            )

    if len(checkout_steps) == 1 and init_step is not None:
        if checkout_steps[0].start_index >= init_step.start_index:
            errors.append(
                f"{workflow_path} must check out the repository before "
                "initializing CodeQL"
            )
    if len(analyze_steps) == 1:
        category = analyze_steps[0].child_value("with", "category")
        if category != "/language:${{ matrix.language }}":
            errors.append(
                f"{workflow_path} must use active "
                "`category: /language:${{ matrix.language }}`"
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
        *check_codeql_contract(root),
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
