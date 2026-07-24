#!/usr/bin/env python3
"""Check repository contracts that keep advisory CI failures observable."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

import yaml
from yaml.constructor import ConstructorError
from yaml.nodes import MappingNode
from yaml.resolver import BaseResolver


@dataclass(frozen=True)
class YamlItem:
    mapping: dict[str, object]
    start_index: int

    def direct_values(self, key: str) -> list[str]:
        value = self.mapping.get(key)
        return [value] if isinstance(value, str) else []

    def direct_value(self, key: str) -> str | None:
        values = self.direct_values(key)
        return values[0] if len(values) == 1 else None

    def child_values(self, parent_key: str, key: str) -> list[str]:
        parent = self.mapping.get(parent_key)
        if not isinstance(parent, dict):
            return []
        value = parent.get(key)
        return [value] if isinstance(value, str) else []

    def child_value(self, parent_key: str, key: str) -> str | None:
        values = self.child_values(parent_key, key)
        return values[0] if len(values) == 1 else None

    def scalar(self, key: str) -> str:
        value = self.mapping.get(key)
        return value if isinstance(value, str) else ""


class UniqueKeyBaseLoader(yaml.BaseLoader):
    """String-only loader that also rejects ambiguous duplicate mapping keys.

    BaseLoader keeps GitHub's `on` key and scalar settings as strings and
    cannot construct Python objects from repository-controlled YAML tags.
    """


def construct_unique_mapping(
    loader: UniqueKeyBaseLoader, node: MappingNode, deep: bool = False
) -> dict[str, object]:
    mapping: dict[str, object] = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if not isinstance(key, str):
            raise ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                "mapping keys must be strings",
                key_node.start_mark,
            )
        if key in mapping:
            raise ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                f"duplicate mapping key {key!r}",
                key_node.start_mark,
            )
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


UniqueKeyBaseLoader.add_constructor(
    BaseResolver.DEFAULT_MAPPING_TAG,
    construct_unique_mapping,
)


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


def load_yaml(
    text: str, relative: str
) -> tuple[object | None, list[str]]:
    try:
        return yaml.load(text, Loader=UniqueKeyBaseLoader), []
    except yaml.YAMLError as error:
        problem = getattr(error, "problem", None) or str(error)
        return None, [f"{relative} must be valid, unambiguous YAML: {problem}"]


def yaml_items(document: object) -> list[YamlItem]:
    items: list[YamlItem] = []
    active_containers: set[int] = set()

    def visit(value: object) -> None:
        if not isinstance(value, (dict, list)):
            return
        identity = id(value)
        if identity in active_containers:
            return
        active_containers.add(identity)
        if isinstance(value, dict):
            items.append(YamlItem(mapping=value, start_index=len(items)))
            for child in value.values():
                visit(child)
        else:
            for child in value:
                visit(child)
        active_containers.remove(identity)

    visit(document)
    return items


def yaml_values_at(document: object, path: tuple[str, ...]) -> list[object]:
    value = document
    for key in path:
        if not isinstance(value, dict) or key not in value:
            return []
        value = value[key]
    return [value]


def items_using(document: object, action: str) -> list[YamlItem]:
    return [
        item
        for item in yaml_items(document)
        if item.direct_value("uses") == action
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

    action_document, action_yaml_errors = load_yaml(action, action_path)
    workflow_document, workflow_yaml_errors = load_yaml(
        workflow, workflow_path
    )
    errors.extend(action_yaml_errors)
    errors.extend(workflow_yaml_errors)
    if errors:
        return errors

    action_items = yaml_items(action_document)
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

    upload_items = items_using(
        action_document, "codecov/codecov-action@v7"
    )
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
        candidate = path.read_text(encoding="utf-8")
        relative = path.relative_to(root).as_posix()
        candidate_document, candidate_errors = load_yaml(candidate, relative)
        errors.extend(candidate_errors)
        if candidate_errors:
            continue
        if path == authority_path:
            continue
        for item in yaml_items(candidate_document):
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
        workflow_document, "./.github/actions/upload-codecov"
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
    workflow_document, yaml_errors = load_yaml(workflow, workflow_path)
    errors.extend(yaml_errors)
    if errors:
        return errors

    target = "site/.lighthouseci/"
    upload_items = [
        item
        for item in items_using(
            workflow_document, "actions/upload-artifact@v7"
        )
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
    workflow_document, yaml_errors = load_yaml(workflow, workflow_path)
    errors.extend(yaml_errors)
    if errors:
        return errors

    expected_languages = (
        "actions",
        "javascript-typescript",
        "python",
        "rust",
    )
    language_values = yaml_values_at(
        workflow_document,
        ("jobs", "analyze", "strategy", "matrix", "language"),
    )
    actual_languages: tuple[str, ...] = ()
    if len(language_values) == 1:
        value = language_values[0]
        if isinstance(value, list) and all(
            isinstance(language, str) for language in value
        ):
            actual_languages = tuple(value)
    if actual_languages != expected_languages:
        errors.append(
            f"{workflow_path} must explicitly analyze "
            "`[actions, javascript-typescript, python, rust]`"
        )

    expected_paths = (
        (("on", "push", "branches"), ["main"], "push to main"),
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
        if yaml_values_at(workflow_document, path) != [expected]:
            errors.append(f"{workflow_path} must configure active `{label}`")
    schedule = yaml_values_at(workflow_document, ("on", "schedule"))
    if (
        len(schedule) != 1
        or not isinstance(schedule[0], list)
        or not any(
            isinstance(item, dict) and isinstance(item.get("cron"), str)
            for item in schedule[0]
        )
    ):
        errors.append(f"{workflow_path} must retain a weekly `schedule`")

    items = yaml_items(workflow_document)
    checkout_steps = items_using(workflow_document, "actions/checkout@v7")
    init_steps = items_using(
        workflow_document, "github/codeql-action/init@v4"
    )
    analyze_steps = items_using(
        workflow_document, "github/codeql-action/analyze@v4"
    )
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
