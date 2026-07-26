package main

import rego.v1

codecov_action := "codecov/codecov-action@v7"
codecov_action_name := "codecov/codecov-action"
codecov_authority_path := ".github/actions/upload-codecov/action.yml"
codecov_wrapper := "./.github/actions/upload-codecov"
ci_workflow_path := ".github/workflows/ci.yml"
codecov_workflow_path := ".github/workflows/ci-tests.yml"
codecov_input_file := "${{ inputs.file }}"
codecov_input_flag := "${{ inputs.flag }}"
codecov_input_report_type := "${{ inputs.report_type }}"
junit_report_path := "target/nextest/ci/junit.xml"
lcov_report_path := "lcov.info"
post_test_condition := "${{ !cancelled() }}"
report_presence_step_name := "Require a generated report"
upload_warning_step_name := "Surface advisory upload failure"
release_workflow_path := ".github/workflows/release.yml"
release_concurrency_group := "pixtuoid-release"
actionlint_config_path := ".github/actionlint.yaml"
actionlint_claude_wif_ignore := "input \"anthropic_(federation_rule_id|organization_id|service_account_id)\" is not defined in action \"anthropics/claude-code-action@v1\""
actionlint_release_queue_ignore := "unexpected key \"queue\" for \"concurrency\" section"
zizmor_config_path := ".github/zizmor.yml"
cache_cleanup_workflow_path := ".github/workflows/cache-cleanup.yml"
claude_action := "anthropics/claude-code-action@v1"
json_schema_flag := "--json-schema"
claude_review_workflow_path := ".github/workflows/claude-review.yml"
claude_security_workflow_path := ".github/workflows/claude-security-review.yml"
claude_reusable_workflow_path := ".github/workflows/claude-readonly-review.yml"
claude_reusable_reference := "./.github/workflows/claude-readonly-review.yml"
claude_manual_commands := {
	claude_review_workflow_path: "/claude-review",
	claude_security_workflow_path: "/security-review",
}

claude_automatic_condition := `(github.event_name == 'pull_request_target' && github.actor != 'dependabot[bot]' && github.event.pull_request.draft == false && github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.base.ref == github.event.repository.default_branch)`
claude_trusted_association_condition := `contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), github.event.comment.author_association)`
pr_resolution_step_name := "Resolve pull request"
claude_model_step_name := "Run read-only Claude review"
claude_publish_step_name := "Publish validated Claude review"
trusted_default_ref := "${{ github.event.repository.default_branch }}"
codeql_workflow_path := ".github/workflows/codeql.yml"
rust_setup_step_name := "Prepare Rust semantic analysis"
rust_health_step_name := "Verify Rust extraction health"
rust_matrix_condition := "${{ matrix.language == 'rust' }}"
codeql_analyze_step_id := "analyze"
codeql_sarif_output := "${{ steps.analyze.outputs.sarif-output }}"
rust_diagnostics_metric := "rust/summary/number-of-files-extracted-with-errors"
rust_clean_metric := "rust/summary/number-of-successfully-extracted-files"
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
}

expected_codecov_routes := {
	expected_codecov_route(junit_report_path, "windows", "test_results", post_test_condition),
	expected_codecov_route(junit_report_path, "macos", "test_results", post_test_condition),
	expected_codecov_route(lcov_report_path, "unit", "coverage", ""),
	expected_codecov_route(junit_report_path, "unit", "test_results", post_test_condition),
	expected_codecov_route(lcov_report_path, "windows", "coverage", ""),
	expected_codecov_route(lcov_report_path, "macos", "coverage", ""),
}

expected_actionlint_paths := {
	claude_reusable_workflow_path: {"ignore": [actionlint_claude_wif_ignore]},
	release_workflow_path: {"ignore": [actionlint_release_queue_ignore]},
}

expected_zizmor_rules := {"unpinned-uses": {"config": {"policies": {"*": "ref-pin"}}}}
expected_claude_oauth_fallback := "${{ vars.ANTHROPIC_FEDERATION_RULE_ID == '' && vars.ANTHROPIC_ORGANIZATION_ID == '' && secrets.CLAUDE_CODE_OAUTH_TOKEN || '' }}"

codecov_oidc_job_names := {
	"windows-test",
	"macos-test",
	"coverage",
	"coverage-windows",
	"coverage-macos",
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
	}
}

actual_codecov_routes := {codecov_route(entry) |
	some entry in codecov_uploads
}

ci_workflow := object.get(documents, ci_workflow_path, {})
ci_jobs := object.get(ci_workflow, "jobs", {})
ci_tests_call_job := object.get(ci_jobs, "tests", {})
codecov_workflow := object.get(documents, codecov_workflow_path, {})
codecov_jobs := object.get(codecov_workflow, "jobs", {})

codeql := documents[codeql_workflow_path]
codeql_job := codeql.jobs.analyze
codeql_steps := object.get(codeql_job, "steps", [])

indexed_steps_matching(steps, field, expected) := [entry |
	some index, step in steps
	object.get(step, field, "") == expected
	entry := {"index": index, "value": step}
]

codeql_steps_using(action) := indexed_steps_matching(codeql_steps, "uses", action)

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

claude_trigger_workflow_paths := {
	claude_review_workflow_path,
	claude_security_workflow_path,
}

claude_reusable := object.get(documents, claude_reusable_workflow_path, {})
claude_reusable_jobs := object.get(claude_reusable, "jobs", {})
claude_analyze_job := object.get(claude_reusable_jobs, "analyze", {})
claude_publish_job := object.get(claude_reusable_jobs, "publish", {})
claude_analyze_steps := object.get(claude_analyze_job, "steps", [])
claude_publish_steps := object.get(claude_publish_job, "steps", [])

claude_analyze_steps_using(action) := indexed_steps_matching(claude_analyze_steps, "uses", action)

claude_analyze_named_steps(name) := indexed_steps_matching(claude_analyze_steps, "name", name)

claude_publish_named_steps(name) := indexed_steps_matching(claude_publish_steps, "name", name)

claude_caller_jobs(path) := [job |
	workflow := documents[path]
	jobs := object.get(workflow, "jobs", {})
	some _, job in jobs
	object.get(job, "uses", "") == claude_reusable_reference
]

claude_manual_condition(command) := sprintf(
	`(github.event_name == 'issue_comment' && github.event.issue.pull_request && startsWith(github.event.comment.body, '%s') && %s)`,
	[command, claude_trusted_association_condition],
)

expected_claude_caller_condition(path) := sprintf(
	"%s || %s",
	[claude_automatic_condition, claude_manual_condition(claude_manual_commands[path])],
)

normalized_claude_condition(condition) := trim_space(regex.replace(condition, `\r?\n[ \t]*`, " "))

claude_caller_condition_is_trusted(path) if {
	callers := claude_caller_jobs(path)
	count(callers) == 1
	condition := object.get(callers[0], "if", "")
	is_string(condition)
	normalized_claude_condition(condition) == normalized_claude_condition(expected_claude_caller_condition(path))
}

claude_checkout_is_trusted(step) if {
	params := object.get(step, "with", {})
	object.get(params, "ref", "") == trusted_default_ref
	object.get(params, "fetch-depth", 0) == 1
	object.get(params, "persist-credentials", true) == false
}

claude_model_is_nonpublishing(step) if {
	params := object.get(step, "with", {})
	object.get(params, "track_progress", true) == false
	object.get(params, "show_full_output", true) == false
	object.get(params, "classify_inline_comments", true) == false
}

claude_model_is_read_only(step) if {
	params := object.get(step, "with", {})
	args := object.get(params, "claude_args", "")
	contains(args, "--allowedTools \"Read,Glob,Grep\"")
	contains(args, json_schema_flag)
	not contains(args, "Bash")
	not contains(args, "mcp__github")
}

# `--json-schema` smuggles a JSON document through a YAML string, so nothing
# else in the gate stack reads it: actionlint and zizmor see an opaque scalar,
# and the read-only rule above is satisfied by the flag NAME alone. An
# unbalanced brace therefore survives every local check and only surfaces on the
# runner, where the CLI refuses to start and every review run dies with
# "--json-schema is not valid JSON" — a gate that can never pass. Parse the
# payload here instead, and hold it to being a well-formed JSON *Schema* rather
# than merely well-formed JSON, so `--json-schema '123'` fails too.
claude_args_schema_entries := [entry |
	some candidate in objects
	args := object.get(candidate.value, "claude_args", null)
	is_string(args)
	contains(args, json_schema_flag)
	entry := {"path": candidate.path, "args": args}
]

claude_json_schema_is_wellformed(args) if {
	parts := split(args, sprintf("%s '", [json_schema_flag]))

	# Exactly one occurrence; a second copy makes the effective payload ambiguous.
	count(parts) == 2
	quoted := trim_space(parts[1])
	payload := trim_suffix(quoted, "'")

	# trim_suffix is a no-op when the suffix is absent, so this proves the
	# closing quote was actually there and the payload is not truncated.
	payload != quoted
	json.is_valid(payload)
	json.verify_schema(json.unmarshal(payload))[0]
}

claude_model_auth_is_scoped(step) if {
	params := object.get(step, "with", {})
	object.get(params, "github_token", "") == "${{ github.token }}"
	object.get(params, "anthropic_federation_rule_id", "") == "${{ vars.ANTHROPIC_FEDERATION_RULE_ID }}"
	object.get(params, "anthropic_organization_id", "") == "${{ vars.ANTHROPIC_ORGANIZATION_ID }}"
	object.get(params, "claude_code_oauth_token", "") == expected_claude_oauth_fallback
}

claude_publisher_revalidates_head(step) if {
	run := object.get(step, "run", "")
	contains(run, "jq --exit-status")
	contains(run, ".head.sha")
	contains(run, "EXPECTED_HEAD_SHA")
	contains(run, "gh pr comment")
}

release_concurrency_is_lossless(concurrency) if {
	object.get(concurrency, "group", "") == release_concurrency_group
	object.get(concurrency, "queue", "") == "max"
	object.get(concurrency, "cancel-in-progress", true) == false
}

rust_health_summary_is_quantified(run) if {
	contains(run, "GITHUB_STEP_SUMMARY")
	contains(run, "Rust CodeQL extraction health")
	contains(run, "Files with diagnostics")
	contains(run, "Clean files")
}

cache_cleanup_is_inert if {
	workflow := documents[cache_cleanup_workflow_path]
	workflow.permissions == {"actions": "write"}
	workflow.on == {"pull_request_target": {"types": ["closed"]}}
	jobs := object.get(workflow, "jobs", {})
	count(jobs) == 1
	job := jobs["closed-pr"]
	count(object.get(job, "steps", [])) == 1
	step := job.steps[0]
	object.get(step, "uses", "") == ""
	env := object.get(job, "env", {})
	object.get(env, "PR_REF", "") == "refs/pull/${{ github.event.pull_request.number }}/merge"
	run := object.get(step, "run", "")
	contains(run, "gh cache delete --all --ref \"$PR_REF\" --succeed-on-no-caches")
}

has_weekly_codeql_schedule if {
	some schedule in codeql.on.schedule
	schedule.cron == "29 11 * * 3"
}

deny contains msg if {
	some path in claude_trigger_workflow_paths
	workflow := documents[path]
	triggers := object.get(workflow, "on", {})
	object.get(triggers, "pull_request", "missing") != "missing"
	msg := sprintf("%s must use pull_request_target instead of pull_request", [path])
}

deny contains msg if {
	some path in claude_trigger_workflow_paths
	workflow := documents[path]
	triggers := object.get(workflow, "on", {})
	object.get(triggers, "pull_request_target", "missing") == "missing"
	msg := sprintf("%s must use pull_request_target instead of pull_request", [path])
}

deny contains msg if {
	some path in claude_trigger_workflow_paths
	_ := documents[path]
	count(claude_caller_jobs(path)) != 1
	msg := sprintf("%s must delegate to the canonical read-only Claude reviewer", [path])
}

deny contains msg if {
	some path in claude_trigger_workflow_paths
	_ := documents[path]
	count(claude_caller_jobs(path)) == 1
	not claude_caller_condition_is_trusted(path)
	msg := sprintf("%s must preserve the trusted automatic and manual review guards", [path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	expected := {
		"contents": "read",
		"id-token": "write",
		"pull-requests": "read",
	}
	object.get(claude_analyze_job, "permissions", {}) != expected
	msg := sprintf("%s analyze job must remain read-only except for OIDC", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	steps := claude_analyze_named_steps(pr_resolution_step_name)
	count(steps) != 1
	msg := sprintf("%s must resolve one open internal default-branch pull request", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	checkouts := claude_analyze_steps_using("actions/checkout@v7")
	count(checkouts) != 1
	msg := sprintf("%s must check out only the trusted default branch without persisted credentials", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	checkouts := claude_analyze_steps_using("actions/checkout@v7")
	count(checkouts) == 1
	not claude_checkout_is_trusted(checkouts[0].value)
	msg := sprintf("%s must check out only the trusted default branch without persisted credentials", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	model_steps := claude_analyze_steps_using(claude_action)
	count(model_steps) != 1
	msg := sprintf("%s must contain exactly one read-only Claude step", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	model_steps := claude_analyze_steps_using(claude_action)
	count(model_steps) == 1
	not claude_model_is_nonpublishing(model_steps[0].value)
	msg := sprintf("%s Claude step must disable progress comments and full output", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	model_steps := claude_analyze_steps_using(claude_action)
	count(model_steps) == 1
	not claude_model_is_read_only(model_steps[0].value)
	msg := sprintf("%s Claude step must expose only read tools and structured output", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	model_steps := claude_analyze_steps_using(claude_action)
	count(model_steps) == 1
	not claude_model_auth_is_scoped(model_steps[0].value)
	msg := sprintf("%s Claude step must use WIF-first authentication with the scoped job token", [claude_reusable_workflow_path])
}

# Deliberately walks every document rather than the reusable workflow alone: the
# payload is unreadable to actionlint wherever it appears, so any future caller
# that grows its own `--json-schema` inherits the same silent-red failure mode.
deny contains msg if {
	some entry in claude_args_schema_entries
	not claude_json_schema_is_wellformed(entry.args)
	msg := sprintf(
		"%s %s must carry exactly one single-quoted, well-formed JSON Schema payload",
		[entry.path, json_schema_flag],
	)
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	expected := {
		"issues": "write",
		"pull-requests": "write",
	}
	object.get(claude_publish_job, "permissions", {}) != expected
	msg := sprintf("%s publish job must have comment-only permissions and no checkout", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	some step in claude_publish_steps
	object.get(step, "uses", "") != ""
	msg := sprintf("%s publish job must have comment-only permissions and no checkout", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	steps := claude_publish_named_steps(claude_publish_step_name)
	count(steps) != 1
	msg := sprintf("%s publisher must validate structured output and recheck the exact PR head", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[claude_reusable_workflow_path]
	steps := claude_publish_named_steps(claude_publish_step_name)
	count(steps) == 1
	not claude_publisher_revalidates_head(steps[0].value)
	msg := sprintf("%s publisher must validate structured output and recheck the exact PR head", [claude_reusable_workflow_path])
}

deny contains msg if {
	_ := documents[cache_cleanup_workflow_path]
	not cache_cleanup_is_inert
	msg := sprintf("%s pull_request_target job must remain cache-only and checkout-free", [cache_cleanup_workflow_path])
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
	object.get(params, "use_oidc", false) != true
	msg := sprintf("%s Codecov step must authenticate with GitHub OIDC", [codecov_authority_path])
}

deny contains msg if {
	authority := documents[codecov_authority_path]
	inputs := object.get(authority, "inputs", {})
	object.get(inputs, "token", null) != null
	msg := sprintf("%s must not declare or forward a Codecov upload token", [codecov_authority_path])
}

deny contains msg if {
	count(authority_uploads) == 1
	params := object.get(authority_uploads[0].value, "with", {})
	object.get(params, "token", null) != null
	msg := sprintf("%s must not declare or forward a Codecov upload token", [codecov_authority_path])
}

deny contains msg if {
	some entry in codecov_uploads
	params := object.get(entry.value, "with", {})
	object.get(params, "token", null) != null
	msg := sprintf("%s must not forward a Codecov upload token", [entry.path])
}

deny contains msg if {
	_ := documents[ci_workflow_path]
	expected := {
		"contents": "read",
		"id-token": "write",
	}
	object.get(ci_tests_call_job, "permissions", {}) != expected
	msg := sprintf("%s tests call must grant only contents:read and id-token:write", [ci_workflow_path])
}

deny contains msg if {
	_ := documents[codecov_workflow_path]
	some name in codecov_oidc_job_names
	job := object.get(codecov_jobs, name, {})
	expected := {
		"contents": "read",
		"id-token": "write",
	}
	object.get(job, "permissions", {}) != expected
	msg := sprintf("%s job %s must receive the Codecov OIDC permission", [codecov_workflow_path, name])
}

# A job in a CALLED workflow that omits `permissions:` runs with exactly the set
# the caller handed down, and ci.yml grants this workflow id-token:write for the
# Codecov OIDC upload. So an omitted block is not "no permissions" — it IS the
# grant, and a rule that only inspects the declared block cannot see the very
# jobs that are exposed. Require every non-Codecov job to declare its own scope.
codecov_job_scope_is_self_declared(job) if {
	permissions := object.get(job, "permissions", null)
	is_object(permissions)
	object.get(permissions, "id-token", "") != "write"
}

deny contains msg if {
	_ := documents[codecov_workflow_path]
	some name, job in codecov_jobs
	not name in codecov_oidc_job_names
	not codecov_job_scope_is_self_declared(job)
	msg := sprintf("%s job %s must declare its own permissions and must not receive the Codecov OIDC permission", [codecov_workflow_path, name])
}

deny contains msg if {
	path := {ci_workflow_path, codecov_workflow_path, codecov_authority_path}[_]
	document := documents[path]
	contains(json.marshal(document), "CODECOV_TOKEN")
	msg := sprintf("%s must not reference the retired CODECOV_TOKEN secret", [path])
}

# `ci-gate` is the ONLY context in branch protection, so its `needs` list plus
# the results it reads ARE the merge gate. Each reusable workflow already pins
# its own nested job membership; nothing pinned the level above, where adding a
# group and forgetting to gate it leaves the single required check green while
# that group is free to fail. `supplemental` is advisory by design.
ci_gate_job_key := "gate"

ci_advisory_job_keys := {"supplemental"}

ci_jobs := object.get(ci_workflow, "jobs", {})

ci_gate_job := object.get(ci_jobs, ci_gate_job_key, {})

ci_group_job_keys := {name |
	some name, job in ci_jobs
	startswith(object.get(job, "uses", ""), "./.github/workflows/")
	not name in ci_advisory_job_keys
}

ci_gate_needs := {name | some name in object.get(ci_gate_job, "needs", [])}

deny contains msg if {
	_ := documents[ci_workflow_path]
	ci_gate_needs != ci_group_job_keys
	msg := sprintf(
		"%s %s must gate exactly the non-advisory group jobs %v, not %v",
		[ci_workflow_path, ci_gate_job_key, ci_group_job_keys, ci_gate_needs],
	)
}

# Membership alone is not enough: a job can sit in `needs` and still be ignored
# by the shell that decides the verdict.
deny contains msg if {
	_ := documents[ci_workflow_path]
	some name in ci_group_job_keys
	not contains(json.marshal(ci_gate_job), sprintf("needs.%s.result", [name]))
	msg := sprintf("%s %s must read needs.%s.result", [ci_workflow_path, ci_gate_job_key, name])
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
	msg := "Codecov routes must match the declared paths, files, flags, types, and conditions"
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
	release := documents[release_workflow_path]
	concurrency := object.get(release, "concurrency", {})
	not release_concurrency_is_lossless(concurrency)
	msg := sprintf("%s must serialize every release tag through one concurrency group", [release_workflow_path])
}

deny contains msg if {
	some document in input.documents
	document.path != release_workflow_path
	concurrency := object.get(document.contents, "concurrency", {})
	object.get(concurrency, "queue", null) != null
	msg := sprintf("%s must not use the release-only queue compatibility field", [document.path])
}

deny contains msg if {
	config := documents[actionlint_config_path]
	object.get(config, "paths", {}) != expected_actionlint_paths
	msg := sprintf("%s must keep only the two path-specific upstream compatibility ignores", [actionlint_config_path])
}

deny contains msg if {
	config := documents[zizmor_config_path]
	object.get(config, "rules", {}) != expected_zizmor_rules
	msg := sprintf("%s must require every action to use a symbolic ref or SHA", [zizmor_config_path])
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
	count(rust_health_steps) == 1
	run := rust_health_steps[0].run
	not rust_health_summary_is_quantified(run)
	msg := sprintf("%s Rust extraction-health gate must write a quantified job summary", [codeql_workflow_path])
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
