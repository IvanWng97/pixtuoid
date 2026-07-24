#!/usr/bin/env python3
"""Validate generated coverage and test reports before an advisory upload."""

from __future__ import annotations

import argparse
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


class ReportValidationError(ValueError):
    """A generated report is not structurally valid for its declared type."""


def parse_uint(value: str, field: str, line_number: int) -> int:
    try:
        number = int(value)
    except ValueError as error:
        raise ReportValidationError(
            f"line {line_number}: {field} must be an integer"
        ) from error
    if number < 0:
        raise ReportValidationError(
            f"line {line_number}: {field} must not be negative"
        )
    return number


def validate_lcov(text: str) -> None:
    record_open = False
    source_file: str | None = None
    has_coverage_data = False
    record_count = 0

    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.rstrip("\r")
        if not line:
            continue
        if line == "end_of_record":
            if not record_open:
                raise ReportValidationError(
                    f"line {line_number}: end_of_record has no record"
                )
            if not source_file:
                raise ReportValidationError(
                    f"line {line_number}: record has no source file"
                )
            if not has_coverage_data:
                raise ReportValidationError(
                    f"line {line_number}: record has no coverage data"
                )
            record_open = False
            source_file = None
            has_coverage_data = False
            record_count += 1
            continue

        tag, separator, value = line.partition(":")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", tag):
            raise ReportValidationError(
                f"line {line_number}: expected an LCOV field"
            )
        if not record_open:
            record_open = True

        if tag == "SF":
            if not value:
                raise ReportValidationError(
                    f"line {line_number}: source file is empty"
                )
            if source_file is not None:
                raise ReportValidationError(
                    f"line {line_number}: record has multiple source files"
                )
            source_file = value
        elif tag == "DA":
            fields = value.split(",")
            if len(fields) not in (2, 3):
                raise ReportValidationError(
                    f"line {line_number}: DA requires line,count[,checksum]"
                )
            parse_uint(fields[0], "DA line", line_number)
            parse_uint(fields[1], "DA count", line_number)
            has_coverage_data = True
        elif tag == "FNDA":
            count, separator, name = value.partition(",")
            if not separator or not name:
                raise ReportValidationError(
                    f"line {line_number}: FNDA requires count,name"
                )
            parse_uint(count, "FNDA count", line_number)
            has_coverage_data = True
        elif tag == "BRDA":
            fields = value.split(",")
            if len(fields) != 4:
                raise ReportValidationError(
                    f"line {line_number}: BRDA requires line,block,branch,taken"
                )
            for field, label in zip(fields[:3], ("line", "block", "branch")):
                parse_uint(field, f"BRDA {label}", line_number)
            if fields[3] != "-":
                parse_uint(fields[3], "BRDA taken", line_number)
            has_coverage_data = True
        elif tag in {"FNF", "FNH", "BRF", "BRH", "LF", "LH"}:
            parse_uint(value, tag, line_number)

    if record_open:
        raise ReportValidationError("final LCOV record has no end_of_record")
    if record_count == 0:
        raise ReportValidationError("LCOV report contains no complete records")


def local_name(tag: str) -> str:
    return tag.rsplit("}", maxsplit=1)[-1]


def validate_junit(path: Path) -> None:
    try:
        root = ET.parse(path).getroot()
    except (ET.ParseError, OSError) as error:
        raise ReportValidationError(f"JUnit XML cannot be parsed: {error}") from error

    root_name = local_name(root.tag)
    if root_name not in {"testsuite", "testsuites"}:
        raise ReportValidationError(
            f"JUnit root must be testsuite or testsuites, found {root_name}"
        )
    if root_name == "testsuites" and not any(
        local_name(element.tag) == "testsuite" for element in root.iter()
    ):
        raise ReportValidationError("JUnit testsuites root has no testsuite")
    if not any(local_name(element.tag) == "testcase" for element in root.iter()):
        raise ReportValidationError("JUnit report has no test cases")


def validate_report(report_type: str, path: Path) -> None:
    try:
        size = path.stat().st_size
    except OSError as error:
        raise ReportValidationError(f"report cannot be read: {error}") from error
    if size == 0:
        raise ReportValidationError("report is empty")

    if report_type == "coverage":
        try:
            validate_lcov(path.read_text(encoding="utf-8"))
        except UnicodeDecodeError as error:
            raise ReportValidationError("LCOV report is not UTF-8") from error
    elif report_type == "test_results":
        validate_junit(path)
    else:
        raise ReportValidationError(f"unknown report type {report_type}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("report_type", choices=("coverage", "test_results"))
    parser.add_argument("path", type=Path)
    args = parser.parse_args(argv)

    try:
        validate_report(args.report_type, args.path)
    except ReportValidationError as error:
        print(f"invalid {args.report_type} report: {error}", file=sys.stderr)
        return 1
    print(f"valid {args.report_type} report: {args.path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
