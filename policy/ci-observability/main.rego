package main

import rego.v1

codecov_action := "codecov/codecov-action@v7"
codecov_action_name := "codecov/codecov-action"
codecov_authority_path := ".github/actions/upload-codecov/action.yml"
codecov_wrapper := "./.github/actions/upload-codecov"
codecov_workflow_path := ".github/workflows/ci-tests.yml"
codecov_input_file := "${{ inputs.file }}"
codecov_input_flag := "${{ inputs.flag }}"
codecov_input_report_type := "${{ inputs.report_type }}"
codecov_input_token := "${{ inputs.token }}"
codecov_token_secret := "${{ secrets.CODECOV_TOKEN }}"
junit_report_path := "target/nextest/ci/junit.xml"
lcov_report_path := "lcov.info"
post_test_condition := "${{ !cancelled() }}"
report_presence_step_name := "Require a generated report"
upload_warning_step_name := "Surface advisory upload failure"
codeql_workflow_path := ".github/workflows/codeql.yml"
rust_setup_step_name := "Prepare Rust semantic analysis"
rust_health_step_name := "Verify Rust extraction health"
rust_matrix_condition := "${{ matrix.language == 'rust' }}"
codeql_analyze_step_id := "analyze"
codeql_sarif_output := "${{ steps.analyze.outputs.sarif-output }}"
rust_diagnostics_metric := "rust/summary/number-of-files-extracted-with-errors"
rust_clean_metric := "rust/summary/number-of-successfully-extracted-files"
gemini_workflow_path := ".github/workflows/gemini-review.yml"
gemini_review_step_name := "Run read-only Gemini design review"
gemini_failure_step_name := "Record review failure"
lighthouse_workflow_path := ".github/workflows/site.yml"
pages_workflow_path := ".github/workflows/pages.yml"
site_package_path := "site/package.json"
expected_dependency_audit := "npm audit --audit-level=low"
pinned_npm_version := "12.0.1"
expected_package_manager := sprintf("npm@%s", [pinned_npm_version])
expected_npm_engine := ">=12.0.0 <13"
expected_npm_setup := sprintf("npm install --global npm@%s", [pinned_npm_version])
github_workspace := "${{ github.workspace }}"

expected_codecov_route(file, flag, report_type, condition) := {
	"path": codecov_workflow_path,
	"file": file,
	"flag": flag,
	"report_type": report_type,
	"if": condition,
	"token": codecov_token_secret,
}

expected_codecov_routes := {
	expected_codecov_route(junit_report_path, "windows", "test_results", post_test_condition),
	expected_codecov_route(junit_report_path, "macos", "test_results", post_test_condition),
	expected_codecov_route(lcov_report_path, "unit", "coverage", ""),
	expected_codecov_route(junit_report_path, "unit", "test_results", post_test_condition),
	expected_codecov_route(lcov_report_path, "windows", "coverage", ""),
	expected_codecov_route(lcov_report_path, "macos", "coverage", ""),
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
	object.get(entry.value, "name", "") == report_presence_step_name
	run := object.get(entry.value, "run", "")
	is_string(run)
	contains(run, "-s \"$REPORT_FILE\"")
	env := object.get(entry.value, "env", {})
	object.get(env, "REPORT_FILE", "") == codecov_input_file
	object.get(entry.value, "if", null) == null
]

warning_steps := [entry |
	some entry in authority_objects
	object.get(entry.value, "name", "") == upload_warning_step_name
	object.get(entry.value, "if", "") == "${{ steps.upload.outcome == 'failure' }}"
]

effective_working_directory(job, step) := directory if {
	directory := object.get(step, "working-directory", null)
	directory != null
}

effective_working_directory(job, step) := directory if {
	object.get(step, "working-directory", null) == null
	defaults := object.get(job, "defaults", {})
	run_defaults := object.get(defaults, "run", {})
	directory := object.get(run_defaults, "working-directory", "")
}

dependency_audit_steps(path, job_name) := [step |
	workflow := documents[path]
	jobs := object.get(workflow, "jobs", {})
	job := object.get(jobs, job_name, {})
	object.get(job, "if", null) == null
	object.get(job, "continue-on-error", false) == false
	steps := object.get(job, "steps", [])
	some step in steps
	object.get(step, "run", "") == "npm run audit"
	object.get(step, "if", null) == null
	object.get(step, "continue-on-error", false) == false
	effective_working_directory(job, step) == "site"
]

pinned_npm_setup_steps(path, job_name) := [step |
	workflow := documents[path]
	jobs := object.get(workflow, "jobs", {})
	job := object.get(jobs, job_name, {})
	object.get(job, "if", null) == null
	object.get(job, "continue-on-error", false) == false
	steps := object.get(job, "steps", [])
	some step in steps
	object.get(step, "run", "") == expected_npm_setup
	object.get(step, "working-directory", "") == github_workspace
	object.get(step, "if", null) == null
	object.get(step, "continue-on-error", false) == false
]

codecov_uploads := [entry |
	some entry in uses_entries
	entry.uses == codecov_wrapper
]

codecov_route(entry) := route if {
	params := object.get(entry.value, "with", {})
	is_object(params)
	route := {
		"path": entry.path,
		"file": object.get(params, "file", ""),
		"flag": object.get(params, "flag", ""),
		"report_type": object.get(params, "report_type", ""),
		"if": object.get(entry.value, "if", ""),
		"token": object.get(params, "token", ""),
	}
}

actual_codecov_routes := {codecov_route(entry) |
	some entry in codecov_uploads
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
	object.get(step, "name", "") == rust_setup_step_name
	object.get(step, "if", "") == rust_matrix_condition
	run := object.get(step, "run", "")
	is_string(run)
	entry := {"index": index, "value": step, "run": run}
]

rust_health_steps := [entry |
	some index, step in codeql_steps
	object.get(step, "name", "") == rust_health_step_name
	object.get(step, "if", "") == rust_matrix_condition
	run := object.get(step, "run", "")
	is_string(run)
	entry := {"index": index, "value": step, "run": run}
]

gemini := documents[gemini_workflow_path]
gemini_job := gemini.jobs["design-review"]
gemini_steps := object.get(gemini_job, "steps", [])
gemini_review_steps := [step |
	some step in gemini_steps
	object.get(step, "name", "") == gemini_review_step_name
]

gemini_failure_steps := [step |
	some step in gemini_steps
	object.get(step, "name", "") == gemini_failure_step_name
]

has_weekly_codeql_schedule if {
	some schedule in codeql.on.schedule
	schedule.cron == "29 11 * * 3"
}

deny contains msg if {
	count(gemini_review_steps) != 1
	msg := sprintf("%s must contain exactly one Gemini review step", [gemini_workflow_path])
}

deny contains msg if {
	count(gemini_review_steps) == 1
	object.get(gemini_review_steps[0], "continue-on-error", false) != false
	msg := sprintf("%s must fail when Gemini produces no review", [gemini_workflow_path])
}

deny contains msg if {
	count(gemini_failure_steps) != 1
	msg := sprintf("%s must contain exactly one Gemini failure notice", [gemini_workflow_path])
}

deny contains msg if {
	count(gemini_failure_steps) == 1
	condition := object.get(gemini_failure_steps[0], "if", "")
	not contains(condition, "failure()")
	msg := sprintf("%s Gemini failure notice must run after a failed review step", [gemini_workflow_path])
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
	object.get(params, "files", "") != codecov_input_file
	msg := sprintf("%s Codecov step must pass only inputs.file", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "flags", "") != codecov_input_flag
	msg := sprintf("%s Codecov step must pass inputs.flag", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "report_type", "") != codecov_input_report_type
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
	object.get(params, "plugins", "") != "noop"
	msg := sprintf("%s Codecov step must disable plugin autodiscovery", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "fail_ci_if_error", false) != true
	msg := sprintf("%s Codecov step must keep fail_ci_if_error: true", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "token", "") != codecov_input_token
	msg := sprintf("%s Codecov step must pass inputs.token", [codecov_authority_path])
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
	count(warning_steps) == 1
	env := object.get(warning_steps[0].value, "env", {})
	object.get(env, "REPORT_FILE", "") != codecov_input_file
	msg := sprintf("%s failure step must identify inputs.file", [codecov_authority_path])
}

deny contains msg if {
	count(warning_steps) == 1
	env := object.get(warning_steps[0].value, "env", {})
	object.get(env, "REPORT_FLAG", "") != codecov_input_flag
	msg := sprintf("%s failure step must identify inputs.flag", [codecov_authority_path])
}

deny contains msg if {
	count(warning_steps) == 1
	env := object.get(warning_steps[0].value, "env", {})
	object.get(env, "REPORT_TYPE", "") != codecov_input_report_type
	msg := sprintf("%s failure step must identify inputs.report_type", [codecov_authority_path])
}

deny contains msg if {
	some entry in uses_entries
	entry.uses == codecov_wrapper
	params := object.get(entry.value, "with", {})
	object.get(params, "report-type", null) != null
	msg := sprintf("%s must use report_type, not report-type", [entry.path])
}

deny contains msg if {
	count(codecov_uploads) != count(expected_codecov_routes)
	msg := "the repository must contain exactly the six declared Codecov routes"
}

deny contains msg if {
	actual_codecov_routes != expected_codecov_routes
	msg := "Codecov routes must match the declared paths, files, flags, types, conditions, and token"
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
	count(dependency_audit_steps(lighthouse_workflow_path, "check")) != 1
	msg := sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path])
}

deny contains msg if {
	count(dependency_audit_steps(pages_workflow_path, "build")) != 1
	msg := sprintf("%s must run the site dependency audit exactly once", [pages_workflow_path])
}

deny contains msg if {
	manifest := documents[site_package_path]
	scripts := object.get(manifest, "scripts", {})
	object.get(scripts, "audit", "") != expected_dependency_audit
	msg := sprintf("%s must keep scripts.audit at %q", [site_package_path, expected_dependency_audit])
}

deny contains msg if {
	count(pinned_npm_setup_steps(lighthouse_workflow_path, "check")) != 1
	msg := sprintf("%s must install %s exactly once", [lighthouse_workflow_path, expected_package_manager])
}

deny contains msg if {
	count(pinned_npm_setup_steps(pages_workflow_path, "build")) != 1
	msg := sprintf("%s must install %s exactly once", [pages_workflow_path, expected_package_manager])
}

deny contains msg if {
	manifest := documents[site_package_path]
	object.get(manifest, "packageManager", "") != expected_package_manager
	msg := sprintf("%s must pin packageManager to %s", [site_package_path, expected_package_manager])
}

deny contains msg if {
	manifest := documents[site_package_path]
	engines := object.get(manifest, "engines", {})
	object.get(engines, "npm", "") != expected_npm_engine
	msg := sprintf("%s must require npm %s", [site_package_path, expected_npm_engine])
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
	not contains(run, "cargo metadata --no-deps --format-version 1")
	msg := sprintf("%s must derive one workspace MSRV with cargo metadata", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "rustup toolchain install \"$workspace_msrv\"")
	msg := sprintf("%s must install the declared MSRV before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "rustup run \"$workspace_msrv\" rustc --print sysroot")
	msg := sprintf("%s must derive CodeQL's sysroot from the declared MSRV", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "test -s \"$rust_source/std/src/lib.rs\"")
	msg := sprintf("%s must verify rust-src before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "test -x \"$proc_macro_server\"")
	msg := sprintf("%s must verify the sysroot proc-macro server before CodeQL init", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "CODEQL_EXTRACTOR_RUST_OPTION_SYSROOT=$rust_sysroot")
	msg := sprintf("%s must pass the verified sysroot to the Rust extractor", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "CODEQL_EXTRACTOR_RUST_OPTION_SYSROOT_SRC=$rust_source")
	msg := sprintf("%s must pass the verified rust-src path to the Rust extractor", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_setup_steps) == 1
	run := rust_setup_steps[0].run
	not contains(run, "CODEQL_EXTRACTOR_RUST_OPTION_PROC_MACRO_SERVER=$proc_macro_server")
	msg := sprintf("%s must pass the verified proc-macro server to the Rust extractor", [codeql_workflow_path])
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
	analyze_steps := codeql_steps_using("github/codeql-action/analyze@v4")
	count(analyze_steps) == 1
	object.get(analyze_steps[0].value, "id", "") != codeql_analyze_step_id
	msg := sprintf("%s CodeQL analyze step must have id: %s", [codeql_workflow_path, codeql_analyze_step_id])
}

deny contains msg if {
	count(rust_health_steps) != 1
	msg := sprintf("%s must contain one Rust extraction-health gate", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_health_steps) == 1
	object.get(rust_health_steps[0].value, "continue-on-error", false) != false
	msg := sprintf("%s Rust extraction-health gate must fail the job", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_health_steps) == 1
	env := object.get(rust_health_steps[0].value, "env", {})
	object.get(env, "SARIF_DIR", "") != codeql_sarif_output
	msg := sprintf("%s Rust extraction-health gate must read CodeQL's SARIF output", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_health_steps) == 1
	not contains(rust_health_steps[0].run, rust_diagnostics_metric)
	msg := sprintf("%s Rust extraction-health gate must read the diagnostics metric", [codeql_workflow_path])
}

deny contains msg if {
	count(rust_health_steps) == 1
	not contains(rust_health_steps[0].run, rust_clean_metric)
	msg := sprintf("%s Rust extraction-health gate must read the clean-files metric", [codeql_workflow_path])
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

deny contains msg if {
	analyze_steps := codeql_steps_using("github/codeql-action/analyze@v4")
	count(analyze_steps) == 1
	count(rust_health_steps) == 1
	analyze_steps[0].index >= rust_health_steps[0].index
	msg := sprintf("%s must verify Rust extraction health after CodeQL analyze", [codeql_workflow_path])
}
