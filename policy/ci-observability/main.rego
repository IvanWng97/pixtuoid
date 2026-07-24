package main

import rego.v1

codecov_action := "codecov/codecov-action@v7"
codecov_action_name := "codecov/codecov-action"
codecov_authority_path := ".github/actions/upload-codecov/action.yml"
codecov_wrapper := "./.github/actions/upload-codecov"
codecov_workflow_path := ".github/workflows/ci-tests.yml"
codeql_workflow_path := ".github/workflows/codeql.yml"
lighthouse_workflow_path := ".github/workflows/site.yml"

expected_codecov_routes := {
	{
		"file": "target/nextest/ci/junit.xml",
		"flag": "windows",
		"report_type": "test_results",
		"if": "${{ !cancelled() }}",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
	{
		"file": "target/nextest/ci/junit.xml",
		"flag": "macos",
		"report_type": "test_results",
		"if": "${{ !cancelled() }}",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
	{
		"file": "lcov.info",
		"flag": "unit",
		"report_type": "coverage",
		"if": "",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
	{
		"file": "target/nextest/ci/junit.xml",
		"flag": "unit",
		"report_type": "test_results",
		"if": "${{ !cancelled() }}",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
	{
		"file": "lcov.info",
		"flag": "windows",
		"report_type": "coverage",
		"if": "",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
	{
		"file": "lcov.info",
		"flag": "macos",
		"report_type": "coverage",
		"if": "",
		"token": "${{ secrets.CODECOV_TOKEN }}",
	},
}

documents := {document.path: document.contents |
	some document in input.documents
}

objects := [entry |
	some document in input.documents
	some _, value in walk(document.contents)
	is_object(value)
	entry := {"path": document.path, "value": value}
]

uses_entries := [entry |
	some candidate in objects
	uses := object.get(candidate.value, "uses", null)
	is_string(uses)
	entry := {
		"path": candidate.path,
		"value": candidate.value,
		"uses": uses,
	}
]

entries_using(action) := [entry |
	some entry in uses_entries
	entry.uses == action
]

codecov_action_reference(value) if {
	parts := split(value, "@")
	count(parts) == 2
	lower(parts[0]) == codecov_action_name
}

rogue_codecov_entries := [entry |
	some entry in uses_entries
	codecov_action_reference(entry.uses)
	not canonical_codecov_entry(entry)
]

canonical_codecov_entry(entry) if {
	entry.path == codecov_authority_path
	entry.uses == codecov_action
}

authority_uploads := [entry |
	some entry in uses_entries
	entry.path == codecov_authority_path
	entry.uses == codecov_action
]

authority_objects := [entry |
	some entry in objects
	entry.path == codecov_authority_path
]

validation_steps := [entry |
	some entry in authority_objects
	run := object.get(entry.value, "run", "")
	is_string(run)
	contains(run, "-s \"$REPORT_FILE\"")
	object.get(entry.value, "if", null) == null
]

warning_steps := [entry |
	some entry in authority_objects
	object.get(entry.value, "if", "") == "${{ steps.upload.outcome == 'failure' }}"
]

ci_uploads := [entry |
	some entry in uses_entries
	entry.path == codecov_workflow_path
	entry.uses == codecov_wrapper
]

codecov_route(entry) := route if {
	params := object.get(entry.value, "with", {})
	is_object(params)
	route := {
		"file": object.get(params, "file", ""),
		"flag": object.get(params, "flag", ""),
		"report_type": object.get(params, "report_type", ""),
		"if": object.get(entry.value, "if", ""),
		"token": object.get(params, "token", ""),
	}
}

actual_codecov_routes := {codecov_route(entry) |
	some entry in ci_uploads
}

codeql := documents[codeql_workflow_path]
codeql_job := codeql.jobs.analyze
codeql_steps := object.get(codeql_job, "steps", [])

codeql_steps_using(action) := [entry |
	some index, step in codeql_steps
	object.get(step, "uses", "") == action
	entry := {"index": index, "value": step}
]

rust_setup_steps := [entry |
	some index, step in codeql_steps
	object.get(step, "if", "") == "${{ matrix.language == 'rust' }}"
	run := object.get(step, "run", "")
	is_string(run)
	entry := {"index": index, "value": step, "run": run}
]

has_weekly_codeql_schedule if {
	some schedule in codeql.on.schedule
	schedule.cron == "29 11 * * 3"
}

deny contains msg if {
	some entry in rogue_codecov_entries
	msg := sprintf("%s may reference Codecov only through the canonical %s step in %s", [entry.path, codecov_action, codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) != 1
	msg := sprintf("%s must contain exactly one canonical %s step", [codecov_authority_path, codecov_action])
}

deny contains msg if {
	count(authority_uploads) == 1
	upload := authority_uploads[0].value
	object.get(upload, "id", "") != "upload"
	msg := sprintf("%s Codecov step must declare id: upload", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	upload := authority_uploads[0].value
	object.get(upload, "continue-on-error", false) != true
	msg := sprintf("%s Codecov step must keep continue-on-error: true", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	upload := authority_uploads[0].value
	object.get(upload, "if", null) != null
	msg := sprintf("%s Codecov step must remain unconditional", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "files", "") != "${{ inputs.file }}"
	msg := sprintf("%s Codecov step must pass only inputs.file", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "report_type", "") != "${{ inputs.report_type }}"
	msg := sprintf("%s Codecov step must pass inputs.report_type", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "disable_search", false) != true
	msg := sprintf("%s Codecov step must keep disable_search: true", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "fail_ci_if_error", false) != true
	msg := sprintf("%s Codecov step must keep fail_ci_if_error: true", [codecov_authority_path])
}

deny contains msg if {
	count(validation_steps) != 1
	msg := sprintf("%s must contain one unconditional non-empty report check", [codecov_authority_path])
}

deny contains msg if {
	count(warning_steps) != 1
	msg := sprintf("%s must contain one upload-failure warning step", [codecov_authority_path])
}

deny contains msg if {
	count(warning_steps) == 1
	run := object.get(warning_steps[0].value, "run", "")
	not contains(run, "::warning")
	msg := sprintf("%s failure step must emit a workflow warning", [codecov_authority_path])
}

deny contains msg if {
	count(warning_steps) == 1
	run := object.get(warning_steps[0].value, "run", "")
	not contains(run, "GITHUB_STEP_SUMMARY")
	msg := sprintf("%s failure step must write the job summary", [codecov_authority_path])
}

deny contains msg if {
	some entry in uses_entries
	entry.uses == codecov_wrapper
	params := object.get(entry.value, "with", {})
	object.get(params, "report-type", null) != null
	msg := sprintf("%s must use report_type, not report-type", [entry.path])
}

deny contains msg if {
	count(ci_uploads) != count(expected_codecov_routes)
	msg := sprintf("%s must contain the six declared Codecov routes", [codecov_workflow_path])
}

deny contains msg if {
	actual_codecov_routes != expected_codecov_routes
	msg := sprintf("%s Codecov routes must match the declared files, flags, types, conditions, and token", [codecov_workflow_path])
}

deny contains msg if {
	uploads := [entry |
		some entry in uses_entries
		entry.path == lighthouse_workflow_path
		entry.uses == "actions/upload-artifact@v7"
		params := object.get(entry.value, "with", {})
		object.get(params, "path", "") == "site/.lighthouseci/"
	]
	count(uploads) != 1
	msg := sprintf("%s must upload site/.lighthouseci/ exactly once", [lighthouse_workflow_path])
}

deny contains msg if {
	some entry in uses_entries
	entry.path == lighthouse_workflow_path
	entry.uses == "actions/upload-artifact@v7"
	params := object.get(entry.value, "with", {})
	object.get(params, "path", "") == "site/.lighthouseci/"
	object.get(entry.value, "if", "") != "${{ !cancelled() }}"
	msg := sprintf("%s Lighthouse upload must run under !cancelled()", [lighthouse_workflow_path])
}

deny contains msg if {
	some entry in uses_entries
	entry.path == lighthouse_workflow_path
	entry.uses == "actions/upload-artifact@v7"
	params := object.get(entry.value, "with", {})
	object.get(params, "path", "") == "site/.lighthouseci/"
	object.get(params, "include-hidden-files", false) != true
	msg := sprintf("%s Lighthouse upload must include hidden files", [lighthouse_workflow_path])
}

deny contains msg if {
	some entry in uses_entries
	entry.path == lighthouse_workflow_path
	entry.uses == "actions/upload-artifact@v7"
	params := object.get(entry.value, "with", {})
	object.get(params, "path", "") == "site/.lighthouseci/"
	object.get(params, "if-no-files-found", "") != "error"
	msg := sprintf("%s Lighthouse upload must fail when reports are absent", [lighthouse_workflow_path])
}

deny contains msg if {
	codeql.on.push.branches != ["main"]
	msg := sprintf("%s must run on pushes to main", [codeql_workflow_path])
}

deny contains msg if {
	object.get(codeql.on, "pull_request", "missing") != null
	msg := sprintf("%s must run on pull requests", [codeql_workflow_path])
}

deny contains msg if {
	object.get(codeql.on, "workflow_dispatch", "missing") != null
	msg := sprintf("%s must support manual dispatch", [codeql_workflow_path])
}

deny contains msg if {
	not has_weekly_codeql_schedule
	msg := sprintf("%s must retain its weekly schedule", [codeql_workflow_path])
}

deny contains msg if {
	codeql.permissions.actions != "read"
	msg := sprintf("%s must grant actions: read", [codeql_workflow_path])
}

deny contains msg if {
	codeql.permissions.contents != "read"
	msg := sprintf("%s must grant contents: read", [codeql_workflow_path])
}

deny contains msg if {
	codeql.permissions.packages != "read"
	msg := sprintf("%s must grant packages: read", [codeql_workflow_path])
}

deny contains msg if {
	codeql.permissions["security-events"] != "write"
	msg := sprintf("%s must grant security-events: write", [codeql_workflow_path])
}

deny contains msg if {
	codeql.concurrency.group != "codeql-${{ github.ref }}"
	msg := sprintf("%s must group concurrency by ref", [codeql_workflow_path])
}

deny contains msg if {
	codeql.concurrency["cancel-in-progress"] != "${{ github.event_name == 'pull_request' }}"
	msg := sprintf("%s must cancel only superseded pull-request runs", [codeql_workflow_path])
}

deny contains msg if {
	codeql_job["runs-on"] != "ubuntu-latest"
	msg := sprintf("%s analyze job must use ubuntu-latest", [codeql_workflow_path])
}

deny contains msg if {
	codeql_job["timeout-minutes"] != 30
	msg := sprintf("%s analyze job must keep timeout-minutes: 30", [codeql_workflow_path])
}

deny contains msg if {
	codeql_job.strategy["fail-fast"] != false
	msg := sprintf("%s analyze matrix must keep fail-fast: false", [codeql_workflow_path])
}

deny contains msg if {
	codeql_job.strategy.matrix.language != ["actions", "javascript-typescript", "python", "rust"]
	msg := sprintf("%s must analyze actions, JavaScript/TypeScript, Python, and Rust", [codeql_workflow_path])
}

deny contains msg if {
	count(codeql_steps_using("actions/checkout@v7")) != 1
	msg := sprintf("%s must check out the repository exactly once", [codeql_workflow_path])
}

deny contains msg if {
	count(codeql_steps_using("github/codeql-action/init@v4")) != 1
	msg := sprintf("%s must initialize CodeQL v4 exactly once", [codeql_workflow_path])
}

deny contains msg if {
	count(codeql_steps_using("github/codeql-action/analyze@v4")) != 1
	msg := sprintf("%s must analyze with CodeQL v4 exactly once", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) != 1
	msg := sprintf("%s must contain one Rust semantic-input setup step", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "rustup component add rust-src --toolchain stable")
	msg := sprintf("%s must install rust-src before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "test -s \"$rust_source\"")
	msg := sprintf("%s must verify rust-src before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "CODEQL_EXTRACTOR_RUST_OPTION_CARGO_ALL_TARGETS=true")
	msg := sprintf("%s must enable CodeQL cargo_all_targets", [codeql_workflow_path])
}

deny contains msg if {
	init_steps := codeql_steps_using("github/codeql-action/init@v4")
	count(init_steps) == 1
	params := object.get(init_steps[0].value, "with", {})
	object.get(params, "languages", "") != "${{ matrix.language }}"
	msg := sprintf("%s CodeQL init must consume matrix.language", [codeql_workflow_path])
}

deny contains msg if {
	init_steps := codeql_steps_using("github/codeql-action/init@v4")
	count(init_steps) == 1
	params := object.get(init_steps[0].value, "with", {})
	object.get(params, "build-mode", "") != "none"
	msg := sprintf("%s CodeQL init must use build-mode: none", [codeql_workflow_path])
}

deny contains msg if {
	analyze_steps := codeql_steps_using("github/codeql-action/analyze@v4")
	count(analyze_steps) == 1
	params := object.get(analyze_steps[0].value, "with", {})
	object.get(params, "category", "") != "/language:${{ matrix.language }}"
	msg := sprintf("%s CodeQL analyze must use a per-language category", [codeql_workflow_path])
}

deny contains msg if {
	checkout_steps := codeql_steps_using("actions/checkout@v7")
	init_steps := codeql_steps_using("github/codeql-action/init@v4")
	count(checkout_steps) == 1
	count(init_steps) == 1
	checkout_steps[0].index >= init_steps[0].index
	msg := sprintf("%s must check out before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	init_steps := codeql_steps_using("github/codeql-action/init@v4")
	count(rust_setup_steps) == 1
	count(init_steps) == 1
	rust_setup_steps[0].index >= init_steps[0].index
	msg := sprintf("%s must prepare Rust before CodeQL init", [codeql_workflow_path])
}
