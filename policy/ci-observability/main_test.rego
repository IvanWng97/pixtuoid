package main

import rego.v1

test_codecov_action_owner_is_case_insensitive if {
	every value in {
		"codecov/codecov-action@v7",
		"Codecov/codecov-action@v7",
		"CODECOV/CODECOV-ACTION@v7",
		"codecov/Codecov-Action@v7",
	} {
		codecov_action_reference(value)
	}
}

test_unrelated_action_is_not_codecov if {
	not codecov_action_reference("actions/upload-artifact@v7")
}

test_case_variants_cannot_bypass_centralization if {
	path := ".github/workflows/rogue.yml"
	every value in {
		"Codecov/codecov-action@v7",
		"CODECOV/CODECOV-ACTION@v7",
		"codecov/Codecov-Action@v7",
	} {
		fixture := {"documents": [{
			"path": path,
			"contents": {"jobs": {"test": {"steps": [{"uses": value}]}}},
		}]}
		violations := deny with input as fixture
		sprintf("%s may reference Codecov only through the canonical %s step in %s", [path, codecov_action, codecov_authority_path]) in violations
	}
}

test_case_variant_inside_authority_is_not_canonical if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{"uses": "CODECOV/CODECOV-ACTION@v7"}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s may reference Codecov only through the canonical %s step in %s", [codecov_authority_path, codecov_action, codecov_authority_path]) in violations
}

test_declared_codecov_route_matrix_is_complete if {
	count(expected_codecov_routes) == 6
	count({route | some route in expected_codecov_routes; route.report_type == "coverage"}) == 3
	count({route | some route in expected_codecov_routes; route.report_type == "test_results"}) == 3
	count({route | some route in expected_codecov_routes; route.if == "${{ !cancelled() }}"}) == 3
}

test_codecov_route_reads_the_structured_step if {
	entry := {
		"path": codecov_workflow_path,
		"value": {
			"if": "${{ !cancelled() }}",
			"with": {
				"file": "target/nextest/ci/junit.xml",
				"flag": "windows",
				"report_type": "test_results",
			},
		},
	}
	codecov_route(entry) == {
		"path": codecov_workflow_path,
		"file": "target/nextest/ci/junit.xml",
		"flag": "windows",
		"report_type": "test_results",
		"if": "${{ !cancelled() }}",
	}
}

test_codeql_step_selection_preserves_order if {
	steps := [
		{"uses": "actions/checkout@v7"},
		{"uses": "github/codeql-action/init@v4"},
		{"uses": "github/codeql-action/analyze@v4"},
	]
	checkout := codeql_steps_using("actions/checkout@v7") with codeql_steps as steps
	checkout[0].index == 0
	initialize := codeql_steps_using("github/codeql-action/init@v4") with codeql_steps as steps
	initialize[0].index == 1
	analyze := codeql_steps_using("github/codeql-action/analyze@v4") with codeql_steps as steps
	analyze[0].index == 2
}

test_indexed_step_selection_supports_named_steps if {
	steps := [
		{"name": "first"},
		{"name": "target"},
		{"name": "target"},
	]
	matches := indexed_steps_matching(steps, "name", "target")
	[count(matches), matches[0].index, matches[1].index] == [2, 1, 2]
}

test_codeql_extraction_health_cannot_be_masked_as_success if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"name": rust_health_step_name,
			"if": rust_matrix_condition,
			"continue-on-error": true,
			"run": "health",
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Rust extraction-health gate must fail the job", [codeql_workflow_path]) in violations
}

test_report_presence_check_is_required if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {
			"using": "composite",
			"steps": [{
				"id": "upload",
				"continue-on-error": true,
				"uses": codecov_action,
			}],
		}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must contain one unconditional non-empty report check", [codecov_authority_path]) in violations
}

test_wrong_report_type_key_is_rejected if {
	path := ".github/workflows/wrong-input.yml"
	fixture := {"documents": [{
		"path": path,
		"contents": {"jobs": {"test": {"steps": [{
			"uses": codecov_wrapper,
			"with": {"report-type": "coverage"},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must use report_type, not report-type", [path]) in violations
}

test_hidden_lighthouse_artifact_is_required if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"lighthouse": {"steps": [{
			"uses": "actions/upload-artifact@v7",
			"with": {"path": "site/.lighthouseci/"},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Lighthouse upload must include hidden files", [lighthouse_workflow_path]) in violations
}

test_site_dependency_audit_is_required if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"check": {"steps": [{"run": "npm ci"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path]) in violations
}

test_pages_dependency_audit_is_required if {
	path := ".github/workflows/pages.yml"
	fixture := {"documents": [{
		"path": path,
		"contents": {"jobs": {"build": {"steps": [{"run": "npm ci"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [path]) in violations
}

test_echoed_dependency_audit_is_rejected if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"check": {"steps": [{"run": "echo npm run audit"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path]) in violations
}

test_conditioned_dependency_audit_is_rejected if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"check": {
			"defaults": {"run": {"working-directory": "site"}},
			"steps": [{"run": "npm run audit", "if": "${{ false }}"}],
		}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path]) in violations
}

test_continue_on_error_dependency_audit_is_rejected if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"check": {
			"defaults": {"run": {"working-directory": "site"}},
			"steps": [{"run": "npm run audit", "continue-on-error": true}],
		}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path]) in violations
}

test_dependency_audit_in_the_wrong_job_is_rejected if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"other": {
			"defaults": {"run": {"working-directory": "site"}},
			"steps": [{"run": "npm run audit"}],
		}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must run the site dependency audit exactly once", [lighthouse_workflow_path]) in violations
}

test_dependency_audit_script_is_pinned if {
	fixture := {"documents": [{
		"path": site_package_path,
		"contents": {"scripts": {"audit": "echo clean"}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must keep scripts.audit at %q", [site_package_path, expected_dependency_audit]) in violations
}

test_site_workflow_installs_the_pinned_npm if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"check": {"steps": [{"run": "npm install --global npm@latest"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must install %s exactly once", [lighthouse_workflow_path, expected_package_manager]) in violations
}

test_pages_workflow_installs_the_pinned_npm if {
	fixture := {"documents": [{
		"path": pages_workflow_path,
		"contents": {"jobs": {"build": {"steps": [{"run": "npm install --global npm@latest"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must install %s exactly once", [pages_workflow_path, expected_package_manager]) in violations
}

test_site_manifest_pins_the_npm_generation if {
	fixture := {"documents": [{
		"path": site_package_path,
		"contents": {
			"packageManager": "npm@11.17.0",
			"engines": {"npm": ">=11"},
		},
	}]}
	violations := deny with input as fixture
	sprintf("%s must pin packageManager to %s", [site_package_path, expected_package_manager]) in violations
	sprintf("%s must require npm %s", [site_package_path, expected_npm_engine]) in violations
}

test_codeql_language_set_is_exact if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"strategy": {"matrix": {"language": [
			"actions",
			"javascript-typescript",
			"python",
		]}}}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must analyze actions, JavaScript/TypeScript, Python, and Rust", [codeql_workflow_path]) in violations
}

test_codecov_flags_forwarding_is_pinned if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"uses": codecov_action,
			"with": {"flags": "broken"},
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Codecov step must pass inputs.flag", [codecov_authority_path]) in violations
}

test_codecov_plugin_autodiscovery_is_rejected if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"uses": codecov_action,
			"with": {"plugins": "gcov"},
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Codecov step must disable plugin autodiscovery", [codecov_authority_path]) in violations
}

test_codecov_token_forwarding_is_pinned if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"uses": codecov_action,
			"with": {"token": "broken"},
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must not declare or forward a Codecov upload token", [codecov_authority_path]) in violations
}

test_conditioned_codecov_upload_is_denied if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"uses": codecov_action,
			"if": post_test_condition,
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Codecov step must remain unconditional", [codecov_authority_path]) in violations
}

# The authority action has its own token rule; this one covers every CALLER of
# the wrapper, where a re-introduced token would otherwise pass unseen.
test_wrapper_caller_forwarding_a_token_is_denied if {
	path := ".github/workflows/extra.yml"
	fixture := {"documents": [{
		"path": path,
		"contents": {"jobs": {"test": {"steps": [{
			"uses": codecov_wrapper,
			"with": {"token": "${{ secrets.UPLOAD_TOKEN }}"},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must not forward a Codecov upload token", [path]) in violations
}

test_retired_codecov_token_secret_is_denied if {
	every path in {ci_workflow_path, codecov_workflow_path, codecov_authority_path} {
		fixture := {"documents": [{
			"path": path,
			"contents": {"jobs": {"test": {"env": {"TOKEN": "${{ secrets.CODECOV_TOKEN }}"}}}},
		}]}
		violations := deny with input as fixture
		sprintf("%s must not reference the retired CODECOV_TOKEN secret", [path]) in violations
	}
}

codecov_warning_step(env) := {
	"path": codecov_authority_path,
	"contents": {"runs": {"steps": [{
		"name": upload_warning_step_name,
		"if": "${{ steps.upload.outcome == 'failure' }}",
		"env": env,
		"run": "echo warning",
	}]}},
}

# An advisory upload failure that does not name which report/flag/type failed is
# a warning nobody can act on, so all three env pins are separate rules.
test_codecov_warning_step_must_identify_the_failed_upload if {
	fixture := {"documents": [codecov_warning_step({})]}
	violations := deny with input as fixture
	every message in {
		"failure step must identify inputs.file",
		"failure step must identify inputs.flag",
		"failure step must identify inputs.report_type",
	} {
		sprintf("%s %s", [codecov_authority_path, message]) in violations
	}
}

test_codecov_warning_step_naming_every_input_is_accepted if {
	fixture := {"documents": [codecov_warning_step({
		"REPORT_FILE": codecov_input_file,
		"REPORT_FLAG": codecov_input_flag,
		"REPORT_TYPE": codecov_input_report_type,
	})]}
	violations := deny with input as fixture
	every message in {
		"failure step must identify inputs.file",
		"failure step must identify inputs.flag",
		"failure step must identify inputs.report_type",
	} {
		not sprintf("%s %s", [codecov_authority_path, message]) in violations
	}
}

# `queue` is a release-only compatibility field carried by an actionlint ignore;
# anywhere else it is an unrecognized key that silently serializes nothing.
test_release_queue_field_outside_release_is_denied if {
	fixture := {"documents": [{
		"path": ci_workflow_path,
		"contents": {"concurrency": {"queue": true}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must not use the release-only queue compatibility field", [ci_workflow_path]) in violations
}

test_report_presence_check_reads_inputs_file if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"name": report_presence_step_name,
			"env": {"REPORT_FILE": "README.md"},
			"run": "test -s \"$REPORT_FILE\"",
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must contain one unconditional non-empty report check", [codecov_authority_path]) in violations
}

test_wrapper_collection_covers_every_workflow if {
	path := ".github/workflows/extra.yml"
	fixture := {"documents": [{
		"path": path,
		"contents": {"jobs": {"test": {"steps": [{
			"uses": codecov_wrapper,
			"with": {
				"file": lcov_report_path,
				"flag": "extra",
				"report_type": "coverage",
			},
		}]}}},
	}]}
	uploads := codecov_uploads with input as fixture
	count(uploads) == 1
	uploads[0].path == path
	"the repository must contain exactly the six declared Codecov routes" in deny with input as fixture
}

test_codecov_oidc_replaces_the_repository_token if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {
			"inputs": {"token": {"required": false}},
			"runs": {"steps": [{
				"uses": codecov_action,
				"with": {
					"files": codecov_input_file,
					"flags": codecov_input_flag,
					"report_type": codecov_input_report_type,
					"token": "${{ inputs.token }}",
				},
			}]},
		},
	}]}
	violations := deny with input as fixture
	sprintf("%s Codecov step must authenticate with GitHub OIDC", [codecov_authority_path]) in violations
	sprintf("%s must not declare or forward a Codecov upload token", [codecov_authority_path]) in violations
}

codecov_scope_message(name) := sprintf(
	"%s job %s must declare its own permissions and must not receive the Codecov OIDC permission",
	[codecov_workflow_path, name],
)

test_codecov_oidc_permission_is_job_scoped if {
	fixture := {"documents": [{
		"path": codecov_workflow_path,
		"contents": {"jobs": {
			"coverage": {"permissions": {"contents": "read"}},
			"snapshots": {"permissions": {"id-token": "write"}},
		}},
	}]}
	violations := deny with input as fixture
	sprintf("%s job coverage must receive the Codecov OIDC permission", [codecov_workflow_path]) in violations
	codecov_scope_message("snapshots") in violations
}

# The regression that shipped: snapshots/smoke/required carried no `permissions:`
# block at all, so they silently inherited the caller's id-token:write while the
# old rule — which only read the declared block — stayed quiet.
test_codecov_job_omitting_permissions_is_denied if {
	fixture := {"documents": [{
		"path": codecov_workflow_path,
		"contents": {"jobs": {"snapshots": {"runs-on": "ubuntu-latest"}}},
	}]}
	violations := deny with input as fixture
	codecov_scope_message("snapshots") in violations
}

test_codecov_job_declaring_its_own_scope_is_accepted if {
	fixture := {"documents": [{
		"path": codecov_workflow_path,
		"contents": {"jobs": {"snapshots": {"permissions": {"contents": "read"}}}},
	}]}
	violations := deny with input as fixture
	not codecov_scope_message("snapshots") in violations
}

test_release_concurrency_must_serialize_different_tags if {
	fixture := {"documents": [{
		"path": release_workflow_path,
		"contents": {"concurrency": {
			"group": "release-${{ github.ref }}",
			"cancel-in-progress": false,
		}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must serialize every release tag through one concurrency group", [release_workflow_path]) in violations
}

test_actionlint_compatibility_ignores_cannot_be_broadened if {
	fixture := {"documents": [{
		"path": actionlint_config_path,
		"contents": {"paths": {".github/workflows/**": {"ignore": [".*"]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must keep only the two path-specific upstream compatibility ignores", [actionlint_config_path]) in violations
}

test_zizmor_pin_policy_cannot_be_tightened_or_disabled_silently if {
	fixture := {"documents": [{
		"path": zizmor_config_path,
		"contents": {"rules": {"unpinned-uses": {"disable": true}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must require every action to use a symbolic ref or SHA", [zizmor_config_path]) in violations
}

zizmor_fixture(contents) := {"documents": [{"path": lint_workflow_path, "contents": contents}]}

zizmor_token_message(job_name) := sprintf(
	"%s job %q must give `%s` a non-empty %s — GitHub layers step env over job env over workflow env, so declaring it at any one of the three is enough; tokenless, zizmor drops to offline and silently skips impostor-commit, known-vulnerable-actions, ref-confusion and stale-action-refs, which run nowhere else",
	[lint_workflow_path, job_name, zizmor_recipe, github_token_env],
)

zizmor_presence_message(found) := sprintf(
	"%s must run `%s` in exactly one step that nothing skips or softens — no `if:` or continue-on-error on either the step or its job — found %d; restore that step, or retarget this policy's zizmor_recipe if the recipe was genuinely renamed, because zizmor's four online audits run nowhere else",
	[lint_workflow_path, zizmor_recipe, found],
)

test_zizmor_online_audits_cannot_be_retired_by_dropping_the_token if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {"steps": [{"run": "just zizmor"}]}}})
	violations := deny with input as fixture
	zizmor_token_message("hygiene") in violations
}

test_zizmor_step_carrying_the_token_is_silent if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {"steps": [{
		"run": "just zizmor",
		"env": {"GH_TOKEN": "${{ github.token }}"},
	}]}}})
	violations := deny with input as fixture
	not zizmor_token_message("hygiene") in violations
}

# The legitimate variant nobody writes a test for: hoisting the token to the job
# once a second step in that job needs it changes nothing about what zizmor
# sees, so a rule that denies it would order the maintainer to undo a valid
# refactor while wearing a required gate's authority.
test_zizmor_step_inheriting_a_job_level_token_is_silent if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {
		"env": {"GH_TOKEN": "${{ github.token }}"},
		"steps": [{"run": "just zizmor"}],
	}}})
	violations := deny with input as fixture
	not zizmor_token_message("hygiene") in violations
}

test_zizmor_step_inheriting_a_workflow_level_token_is_silent if {
	fixture := zizmor_fixture({
		"env": {"GH_TOKEN": "${{ github.token }}"},
		"jobs": {"hygiene": {"steps": [{"run": "just zizmor"}]}},
	})
	violations := deny with input as fixture
	not zizmor_token_message("hygiene") in violations
}

# The nearest DECLARATION wins even when it is empty, so accepting inheritance
# must not degrade into "a token exists somewhere": a step-level blank shadows
# the job's token and puts zizmor back offline.
test_zizmor_step_blanking_an_inherited_token_still_fires if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {
		"env": {"GH_TOKEN": "${{ github.token }}"},
		"steps": [{"run": "just zizmor", "env": {"GH_TOKEN": ""}}],
	}}})
	violations := deny with input as fixture
	zizmor_token_message("hygiene") in violations
}

# The existence half. Without it the token rule is keyed on a step it can no
# longer find, so it passes by matching nothing at all.
test_zizmor_cannot_be_retired_by_renaming_the_recipe if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {"steps": [{
		"run": "just zizmor --fix",
		"env": {"GH_TOKEN": "${{ github.token }}"},
	}]}}})
	violations := deny with input as fixture
	zizmor_presence_message(0) in violations
}

# A skipped or soft-failing step reaches green without auditing anything, and a
# guard on the JOB retires it just as completely as one on the step — so all
# four positions have to count, not just the two on the step.
test_zizmor_cannot_be_retired_by_guarding_or_softening_the_step_or_its_job if {
	tokened_step := {"run": "just zizmor", "env": {"GH_TOKEN": "${{ github.token }}"}}
	guard := "${{ github.event_name == 'schedule' }}"

	retirements := [
		{"jobs": {"hygiene": {"steps": [object.union(tokened_step, {"if": guard})]}}},
		{"jobs": {"hygiene": {"steps": [object.union(tokened_step, {"continue-on-error": true})]}}},
		{"jobs": {"hygiene": {"if": guard, "steps": [tokened_step]}}},
		{"jobs": {"hygiene": {"continue-on-error": true, "steps": [tokened_step]}}},
	]

	every contents in retirements {
		zizmor_presence_message(0) in deny with input as zizmor_fixture(contents)
	}
}

test_zizmor_running_in_two_jobs_is_denied if {
	fixture := zizmor_fixture({"jobs": {
		"hygiene": {"steps": [{"run": "just zizmor", "env": {"GH_TOKEN": "${{ github.token }}"}}]},
		"security": {"steps": [{"run": "just zizmor", "env": {"GH_TOKEN": "${{ github.token }}"}}]},
	}})
	violations := deny with input as fixture
	zizmor_presence_message(2) in violations
}

# A `run: |` block scalar is the same command with the newline yq preserves.
# Matching the raw string would report a rename that never happened AND blind
# the token half, so the existence rule must stay silent here.
test_zizmor_block_scalar_invocation_is_silent if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {"steps": [{
		"run": "just zizmor\n",
		"env": {"GH_TOKEN": "${{ github.token }}"},
	}]}}})
	violations := deny with input as fixture
	not zizmor_presence_message(0) in violations
	not zizmor_token_message("hygiene") in violations
}

test_zizmor_block_scalar_invocation_still_needs_the_token if {
	fixture := zizmor_fixture({"jobs": {"hygiene": {"steps": [{"run": "just zizmor\n"}]}}})
	violations := deny with input as fixture
	zizmor_token_message("hygiene") in violations
}

test_cache_cleanup_cannot_execute_pull_request_code if {
	fixture := {"documents": [{
		"path": cache_cleanup_workflow_path,
		"contents": {
			"on": {"pull_request_target": {"types": ["closed"]}},
			"permissions": {"actions": "write"},
			"jobs": {"closed-pr": {
				"env": {"PR_REF": "refs/pull/${{ github.event.pull_request.number }}/merge"},
				"steps": [{"uses": "actions/checkout@v7"}],
			}},
		},
	}]}
	violations := deny with input as fixture
	sprintf("%s pull_request_target job must remain cache-only and checkout-free", [cache_cleanup_workflow_path]) in violations
}

test_claude_review_trigger_must_load_from_the_trusted_base if {
	fixture := {"documents": [{
		"path": claude_review_workflow_path,
		"contents": {
			"on": {"pull_request": {"types": ["opened"]}},
			"jobs": {"review": {"steps": [{"uses": claude_action}]}},
		},
	}]}
	violations := deny with input as fixture
	sprintf("%s must use pull_request_target instead of pull_request", [claude_review_workflow_path]) in violations
	sprintf("%s must delegate to the canonical read-only Claude reviewer", [claude_review_workflow_path]) in violations
}

test_claude_caller_trust_guards_cannot_be_weakened if {
	review_condition := expected_claude_caller_condition(claude_review_workflow_path)
	security_condition := expected_claude_caller_condition(claude_security_workflow_path)
	elevated_association := replace(
		review_condition,
		claude_trusted_association_condition,
		"true",
	)
	mutations := {
		{
			"path": claude_review_workflow_path,
			"condition": "${{ true }}",
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"github.actor != 'dependabot[bot]'",
				"true",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"'dependabot[bot]'",
				"'dependabot[bot] '",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"github.event.pull_request.draft == false",
				"true",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"github.event.pull_request.head.repo.full_name == github.repository",
				"true",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"github.event.pull_request.base.ref == github.event.repository.default_branch",
				"true",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(review_condition, "'/claude-review'", "'/review'"),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"'/claude-review'",
				"'/claude-review '",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				"github.event.issue.pull_request",
				"true",
			),
		},
		{
			"path": claude_review_workflow_path,
			"condition": elevated_association,
		},
		{
			"path": claude_review_workflow_path,
			"condition": replace(
				review_condition,
				`"COLLABORATOR"`,
				`"COLLABORATOR "`,
			),
		},
		{
			"path": claude_security_workflow_path,
			"condition": replace(security_condition, "'/security-review'", "'/review'"),
		},
	}
	every mutation in mutations {
		fixture := {"documents": [{
			"path": mutation.path,
			"contents": {
				"on": {
					"pull_request_target": {"types": ["opened"]},
					"issue_comment": {"types": ["created"]},
				},
				"jobs": {"review": {
					"if": mutation.condition,
					"uses": claude_reusable_reference,
				}},
			},
		}]}
		violations := deny with input as fixture
		sprintf("%s must preserve the trusted automatic and manual review guards", [mutation.path]) in violations
	}
}

test_claude_resolver_is_required if {
	fixture := {"documents": [{
		"path": claude_reusable_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": []}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must resolve one open internal default-branch pull request", [claude_reusable_workflow_path]) in violations
}

claude_absence_missing_message := sprintf("%s must report an absent review in exactly one job, conditioned on BOTH `%s` and `%s`", [claude_reusable_workflow_path, claude_absence_condition, claude_decline_condition])

claude_absence_status_message := sprintf("%s absent-review job's `if:` needs a status function (one of %v) — without one an implicit `success()` skips it exactly when analyze fails", [claude_reusable_workflow_path, claude_status_functions])

claude_absence_reusable(jobs) := {"documents": [{
	"path": claude_reusable_workflow_path,
	"contents": {"jobs": jobs},
}]}

# Absence and a clean review render identically without this job — see the
# rule in main.rego. #819
test_claude_absent_review_must_be_reported if {
	violations := deny with input as claude_absence_reusable({"analyze": {"steps": []}})
	claude_absence_missing_message in violations
}

test_claude_absent_review_reporter_is_accepted if {
	violations := deny with input as claude_absence_reusable({
		"analyze": {"steps": []},
		"report_absence": {
			"if": sprintf("always() && (%s || %s)", [claude_absence_condition, claude_decline_condition]),
			"permissions": {"pull-requests": "write"},
			"steps": [],
		},
	})
	not claude_absence_missing_message in violations
}

# The decline arm is the shape with no red job behind it, so nothing else would
# notice its removal.
test_claude_absent_review_without_the_decline_arm_is_denied if {
	violations := deny with input as claude_absence_reusable({
		"analyze": {"steps": []},
		"report_absence": {
			"if": sprintf("!cancelled() && %s", [claude_absence_condition]),
			"steps": [],
		},
	})
	claude_absence_missing_message in violations
}

# The inert variant, and the one that would land as a cleanup — see
# `claude_status_functions` in main.rego.
test_claude_absent_review_without_a_status_function_is_denied if {
	violations := deny with input as claude_absence_reusable({
		"analyze": {"steps": []},
		"report_absence": {
			"if": sprintf("%s || %s", [claude_absence_condition, claude_decline_condition]),
			"permissions": {"pull-requests": "write"},
			"steps": [],
		},
	})
	claude_absence_status_message in violations
}

test_claude_oauth_fallback_requires_all_wif_authority_fields_to_be_absent if {
	fixture := {"documents": [{
		"path": claude_reusable_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"name": claude_model_step_name,
			"uses": claude_action,
			"with": {
				"github_token": "${{ github.token }}",
				"anthropic_federation_rule_id": "${{ vars.ANTHROPIC_FEDERATION_RULE_ID }}",
				"anthropic_organization_id": "${{ vars.ANTHROPIC_ORGANIZATION_ID }}",
				"claude_code_oauth_token": "${{ vars.ANTHROPIC_FEDERATION_RULE_ID == '' && secrets.CLAUDE_CODE_OAUTH_TOKEN || '' }}",
			},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Claude step must use WIF-first authentication with the scoped job token", [claude_reusable_workflow_path]) in violations
}

test_claude_model_and_publisher_permissions_are_separated if {
	fixture := {"documents": [{
		"path": claude_reusable_workflow_path,
		"contents": {"jobs": {
			"analyze": {
				"permissions": {
					"contents": "write",
					"pull-requests": "write",
					"id-token": "write",
				},
				"steps": [
					{
						"uses": "actions/checkout@v7",
						"with": {
							"ref": "${{ github.event.pull_request.head.sha }}",
							"persist-credentials": true,
						},
					},
					{
						"name": claude_model_step_name,
						"uses": claude_action,
						"with": {
							"track_progress": true,
							"show_full_output": true,
							"claude_args": "--allowedTools Bash,mcp__github_inline_comment__create_inline_comment",
						},
					},
				],
			},
			"publish": {
				"permissions": {
					"contents": "write",
					"pull-requests": "write",
				},
				"steps": [{"uses": "actions/checkout@v7"}],
			},
		}},
	}]}
	violations := deny with input as fixture
	sprintf("%s analyze job must remain read-only except for OIDC", [claude_reusable_workflow_path]) in violations
	sprintf("%s must check out only the trusted default branch without persisted credentials", [claude_reusable_workflow_path]) in violations
	sprintf("%s Claude step must disable progress comments and full output", [claude_reusable_workflow_path]) in violations
	sprintf("%s Claude step must expose only read tools and structured output", [claude_reusable_workflow_path]) in violations
	sprintf("%s publish job must have comment-only permissions and no checkout", [claude_reusable_workflow_path]) in violations
}

test_claude_publisher_must_revalidate_the_exact_head if {
	fixture := {"documents": [{
		"path": claude_reusable_workflow_path,
		"contents": {"jobs": {"publish": {"steps": [{
			"name": claude_publish_step_name,
			"run": "gh pr comment \"$PR_NUMBER\"",
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s publisher must validate structured output and recheck the exact PR head", [claude_reusable_workflow_path]) in violations
}

test_codeql_health_metrics_must_be_visible_in_the_job_summary if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"name": rust_health_step_name,
			"if": rust_matrix_condition,
			"run": sprintf("read %s %s", [rust_diagnostics_metric, rust_clean_metric]),
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Rust extraction-health gate must write a quantified job summary", [codeql_workflow_path]) in violations
}

schema_reference_message := sprintf(
	"%s must pass %s via the committed schema file (%s), not an inline literal",
	[claude_reusable_workflow_path, json_schema_flag, review_schema_reference],
)

json_schema_fixture(args) := {"documents": [{
	"path": claude_reusable_workflow_path,
	"contents": {"jobs": {"analyze": {"steps": [{
		"uses": claude_action,
		"with": {"claude_args": args},
	}]}}},
}]}

test_committed_schema_reference_is_accepted if {
	args := sprintf("--max-turns 60 %s", [review_schema_reference])
	violations := deny with input as json_schema_fixture(args)
	not schema_reference_message in violations
}

# The regression that took both bots down for 31h: an inline literal is opaque
# to actionlint and zizmor, so a single unbalanced brace reached the runner.
# Any inline payload is now refused outright, balanced or not.
test_inline_schema_payloads_are_denied if {
	every args in {
		# The exact malformed payload from #785 — ten `{` against nine `}`.
		`--json-schema '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]'`,
		# Balanced, and still refused: inline is the shape being retired.
		`--json-schema '{"type":"object"}'`,
		`--json-schema {"type":"object"}`,
	} {
		violations := deny with input as json_schema_fixture(args)
		schema_reference_message in violations
	}
}

test_claude_args_without_a_schema_flag_is_not_denied if {
	violations := deny with input as json_schema_fixture(`--max-turns 60 --allowedTools "Read,Glob,Grep"`)
	not schema_reference_message in violations
}

ci_gate_fixture(jobs) := {"documents": [{"path": ci_workflow_path, "contents": {"jobs": jobs}}]}

ci_gate_shipped_jobs := {
	"lint": {"uses": "./.github/workflows/ci-lint.yml"},
	"builds": {"uses": "./.github/workflows/ci-builds.yml"},
	"tests": {"uses": "./.github/workflows/ci-tests.yml"},
	"supplemental": {"uses": "./.github/workflows/ci-supplemental.yml"},
	"gate": {
		"needs": ["lint", "builds", "tests"],
		"steps": [{"env": {
			"LINT_RESULT": "${{ needs.lint.result }}",
			"BUILDS_RESULT": "${{ needs.builds.result }}",
			"TESTS_RESULT": "${{ needs.tests.result }}",
		}}],
	},
}

ci_gate_membership_violations(violations) := [msg |
	some msg in violations
	contains(msg, "must gate exactly the non-advisory group jobs")
]

ci_gate_unread_violations(violations) := [msg |
	some msg in violations
	contains(msg, "must read needs.")
]

test_ci_gate_covering_every_group_is_accepted if {
	shipped_violations := deny with input as ci_gate_fixture(ci_gate_shipped_jobs)
	count(ci_gate_membership_violations(shipped_violations)) == 0
	count(ci_gate_unread_violations(shipped_violations)) == 0
}

# The gap: a fifth reusable group lands and nobody adds it to the gate, so the
# single protected context stays green while that whole group can fail.
test_ci_gate_missing_a_new_group_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"security": {"uses": "./.github/workflows/ci-security.yml"}})
	violations := deny with input as ci_gate_fixture(jobs)
	count(ci_gate_membership_violations(violations)) == 1
}

# Consistently dropping a group — from `needs` AND from the env the shell reads —
# is the edit actionlint cannot catch, because nothing dangles.
test_ci_gate_dropping_a_group_entirely_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"gate": {
		"needs": ["lint", "tests"],
		"steps": [{"env": {
			"LINT_RESULT": "${{ needs.lint.result }}",
			"TESTS_RESULT": "${{ needs.tests.result }}",
		}}],
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	count(ci_gate_membership_violations(violations)) == 1
	sprintf("%s %s must read needs.builds.result", [ci_workflow_path, ci_gate_job_key]) in deny with input as ci_gate_fixture(jobs)
}

# In `needs` but never consulted by the verdict shell.
test_ci_gate_listing_a_group_it_never_reads_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"gate": {
		"needs": ["lint", "builds", "tests"],
		"steps": [{"env": {
			"LINT_RESULT": "${{ needs.lint.result }}",
			"TESTS_RESULT": "${{ needs.tests.result }}",
		}}],
	}})
	sprintf("%s %s must read needs.builds.result", [ci_workflow_path, ci_gate_job_key]) in deny with input as ci_gate_fixture(jobs)
}

test_ci_gate_need_on_the_advisory_group_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"gate": {
		"needs": ["lint", "builds", "tests", "supplemental"],
		"steps": [{"env": {
			"LINT_RESULT": "${{ needs.lint.result }}",
			"BUILDS_RESULT": "${{ needs.builds.result }}",
			"TESTS_RESULT": "${{ needs.tests.result }}",
		}}],
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	count(ci_gate_membership_violations(violations)) == 1
}

ci_oidc_call_message(name) := sprintf(
	"%s %s call must not pass id-token: write down to jobs that declare no permissions of their own",
	[ci_workflow_path, name],
)

test_non_tests_group_call_granting_oidc_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"lint": {
		"uses": "./.github/workflows/ci-lint.yml",
		"permissions": {"contents": "read", "id-token": "write"},
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	ci_oidc_call_message("lint") in violations
}

test_tests_call_granting_oidc_is_accepted if {
	jobs := object.union(ci_gate_shipped_jobs, {"tests": {
		"uses": "./.github/workflows/ci-tests.yml",
		"permissions": {"contents": "read", "id-token": "write"},
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	not ci_oidc_call_message("tests") in violations
}

# A group converted to a cross-repo reusable workflow must stay in the gate set.
# Prefix-matching `./.github/workflows/` dropped it instead, and the membership
# rule then told the maintainer to REMOVE that group from the merge gate.
test_cross_repo_group_call_still_counts_as_a_group if {
	jobs := object.union(ci_gate_shipped_jobs, {"builds": {"uses": "someorg/shared/.github/workflows/ci-builds.yml@v1"}})
	violations := deny with input as ci_gate_fixture(jobs)
	count(ci_gate_membership_violations(violations)) == 0
	count(ci_gate_unread_violations(violations)) == 0
}

test_cross_repo_group_call_ungated_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {
		"builds": {"uses": "someorg/shared/.github/workflows/ci-builds.yml@v1"},
		"gate": {
			"needs": ["lint", "tests"],
			"steps": [{"env": {
				"LINT_RESULT": "${{ needs.lint.result }}",
				"TESTS_RESULT": "${{ needs.tests.result }}",
			}}],
		},
	})
	violations := deny with input as ci_gate_fixture(jobs)
	count(ci_gate_membership_violations(violations)) == 1
}

test_cross_repo_group_call_granting_oidc_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"builds": {
		"uses": "someorg/shared/.github/workflows/ci-builds.yml@v1",
		"permissions": {"contents": "read", "id-token": "write"},
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	ci_oidc_call_message("builds") in violations
}

nested_manifest_message(path, expected, actual) := sprintf(
	"%s %s job must need exactly %v, not %v",
	[path, required_manifest_job_key, expected, actual],
)

nested_manifest_fixture(jobs) := {"documents": [{
	"path": ".github/workflows/ci-lint.yml",
	"contents": {"jobs": jobs},
}]}

test_nested_manifest_covering_every_job_is_accepted if {
	jobs := {
		"fmt": {},
		"clippy": {},
		"required": {"needs": ["fmt", "clippy"]},
	}
	violations := deny with input as nested_manifest_fixture(jobs)
	not nested_manifest_message(".github/workflows/ci-lint.yml", {"clippy", "fmt"}, {"clippy", "fmt"}) in violations
}

# The gap: a job is added to the workflow but not to the manifest, so it sits
# outside the single protected ci-gate with nothing to say so.
test_nested_manifest_missing_a_new_job_is_denied if {
	jobs := {
		"fmt": {},
		"clippy": {},
		"newcheck": {},
		"required": {"needs": ["fmt", "clippy"]},
	}
	violations := deny with input as nested_manifest_fixture(jobs)
	nested_manifest_message(
		".github/workflows/ci-lint.yml",
		{"clippy", "fmt", "newcheck"},
		{"clippy", "fmt"},
	) in violations
}

test_nested_manifest_needing_a_deleted_job_is_denied if {
	jobs := {
		"fmt": {},
		"required": {"needs": ["fmt", "clippy"]},
	}
	violations := deny with input as nested_manifest_fixture(jobs)
	nested_manifest_message(".github/workflows/ci-lint.yml", {"fmt"}, {"clippy", "fmt"}) in violations
}

# Uploading before the health gate publishes a security tab that reads cleaner
# than reality, because a degraded extraction yields fewer alerts.
test_codeql_analyze_uploading_rust_inline_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"uses": "github/codeql-action/analyze@v4",
			"with": {"category": "/language:${{ matrix.language }}"},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s analyze must defer the Rust upload until extraction health passes", [codeql_workflow_path]) in violations
}

test_codeql_missing_deferred_upload_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"uses": "github/codeql-action/analyze@v4",
			"with": {"upload": codeql_rust_upload_gate},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must upload the Rust SARIF after the extraction-health gate", [codeql_workflow_path]) in violations
}

# Health BEFORE upload is the ordering that fails SILENTLY when broken — the
# inverse (gate ahead of analyze) dies loudly on an empty SARIF_DIR.
test_codeql_upload_before_health_gate_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [
			{"name": codeql_upload_step_name},
			{"name": rust_health_step_name},
		]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must verify Rust extraction health before uploading the SARIF", [codeql_workflow_path]) in violations
}

test_codeql_health_gate_before_upload_is_accepted if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [
			{"name": rust_health_step_name},
			{"name": codeql_upload_step_name},
		]}}},
	}]}
	violations := deny with input as fixture
	not sprintf("%s must verify Rust extraction health before uploading the SARIF", [codeql_workflow_path]) in violations
}

claude_tag_head_guard_message(event) := sprintf("%s %s arm must require `%s`", [claude_tag_workflow_path, event, claude_same_repo_head_condition])

claude_tag_fork_refusal_message := sprintf("%s must refuse fork pull requests in a step scoped to `issue_comment`, before `%s` runs", [claude_tag_workflow_path, claude_action])

claude_tag_fork_refusal_step := {
	"if": "github.event_name == 'issue_comment' && github.event.issue.pull_request",
	"run": sprintf("head_repo=\"$(gh api \"repos/$REPOSITORY/pulls/$PR_NUMBER\" --jq '.%s')\"", [claude_fork_refusal_marker]),
}

claude_tag_single_job_message := sprintf("%s must run `%s` in exactly one job — the fork-head guard is keyed to that job's condition", [claude_tag_workflow_path, claude_action])

claude_tag_triggers := {
	"issues": {"types": ["opened"]},
	"issue_comment": {"types": ["created"]},
	"pull_request_review": {"types": ["submitted"]},
	"pull_request_review_comment": {"types": ["created"]},
}

claude_tag_workflow(job_name, job) := {"documents": [{
	"path": claude_tag_workflow_path,
	"contents": {
		"on": claude_tag_triggers,
		"jobs": {job_name: job},
	},
}]}

claude_tag_condition_text(review_arm_suffix) := sprintf(
	"(github.event_name == 'issues' && trusted) || (github.event_name == 'issue_comment' && trusted) || (github.event_name == 'pull_request_review'%s && trusted) || (github.event_name == 'pull_request_review_comment'%s && trusted)",
	[review_arm_suffix, review_arm_suffix],
)

claude_tag_fixture(review_arm_suffix) := claude_tag_workflow("claude", {
	"if": claude_tag_condition_text(review_arm_suffix),
	"steps": [{"uses": claude_action}],
})

test_claude_tag_pull_request_arms_without_the_head_guard_are_denied if {
	violations := deny with input as claude_tag_fixture("")
	every event in claude_pull_request_event_names {
		claude_tag_head_guard_message(event) in violations
	}
}

# The worst shape, and the one an arm-keyed rule missed: with no condition at
# all every arm is present-and-ungated, not absent.
test_claude_tag_job_without_a_condition_is_denied if {
	violations := deny with input as claude_tag_workflow("claude", {"steps": [{"uses": claude_action}]})
	every event in claude_pull_request_event_names {
		claude_tag_head_guard_message(event) in violations
	}
}

# The sequence spelling of `on:` reaches the job just as the mapping does.
test_claude_tag_sequence_trigger_form_is_denied if {
	fixture := {"documents": [{
		"path": claude_tag_workflow_path,
		"contents": {
			"on": ["issues", "pull_request_review", "pull_request_review_comment"],
			"jobs": {"claude": {"steps": [{"uses": claude_action}]}},
		},
	}]}
	violations := deny with input as fixture
	every event in claude_pull_request_event_names {
		claude_tag_head_guard_message(event) in violations
	}
}

# Renaming the job away from `claude` used to silence the guard entirely.
test_claude_tag_renamed_job_is_still_guarded if {
	violations := deny with input as claude_tag_workflow("respond", {
		"if": claude_tag_condition_text(""),
		"steps": [{"uses": claude_action}],
	})
	every event in claude_pull_request_event_names {
		claude_tag_head_guard_message(event) in violations
	}
	not claude_tag_single_job_message in violations
}

test_claude_tag_workflow_without_the_action_is_denied if {
	violations := deny with input as claude_tag_workflow("claude", {"steps": [{"uses": "actions/checkout@v7"}]})
	claude_tag_single_job_message in violations
}

# The issues/issue_comment arms carry no pull_request object, so demanding the
# guard of them would deny a workflow that is not exposed in the first place.
test_claude_tag_guarded_pull_request_arms_are_accepted if {
	violations := deny with input as claude_tag_fixture(sprintf(" && %s", [claude_same_repo_head_condition]))
	every event in claude_pull_request_event_names {
		not claude_tag_head_guard_message(event) in violations
	}
	not claude_tag_single_job_message in violations
}

# Dropping BOTH the arm and its `on:` entry retires the exposure, so the rule
# keyed off `on:` must stay silent — the guard tracks reachability, not arms.
test_claude_tag_dropping_a_pull_request_trigger_is_accepted if {
	fixture := {"documents": [{
		"path": claude_tag_workflow_path,
		"contents": {
			"on": {"issues": {"types": ["opened"]}},
			"jobs": {"claude": {
				"if": "(github.event_name == 'issues' && trusted)",
				"steps": [{"uses": claude_action}],
			}},
		},
	}]}
	violations := deny with input as fixture
	every violation in violations {
		not contains(violation, "arm must require")
	}
}

# The `if:` guard the other arms use is inexpressible here — see the rule in
# main.rego. #799
test_claude_tag_issue_comment_without_a_fork_refusal_step_is_denied if {
	violations := deny with input as claude_tag_fixture(sprintf(" && %s", [claude_same_repo_head_condition]))
	claude_tag_fork_refusal_message in violations
}

# ORDER is the whole point — a refusal that runs after the action has already
# staged the fork tree is decoration.
test_claude_tag_fork_refusal_after_the_action_is_denied if {
	violations := deny with input as claude_tag_workflow("claude", {
		"if": claude_tag_condition_text(sprintf(" && %s", [claude_same_repo_head_condition])),
		"steps": [{"uses": claude_action}, claude_tag_fork_refusal_step],
	})
	claude_tag_fork_refusal_message in violations
}

# Narrowing the step's own condition reopens #799 in full while leaving the
# `run:` body — and so a run-only rule — untouched.
test_claude_tag_fork_refusal_not_scoped_to_issue_comment_is_denied if {
	violations := deny with input as claude_tag_workflow("claude", {
		"if": claude_tag_condition_text(sprintf(" && %s", [claude_same_repo_head_condition])),
		"steps": [
			object.union(claude_tag_fork_refusal_step, {"if": "github.event_name == 'pull_request_review'"}),
			{"uses": claude_action},
		],
	})
	claude_tag_fork_refusal_message in violations
}

test_claude_tag_fork_refusal_before_the_action_is_accepted if {
	violations := deny with input as claude_tag_workflow("claude", {
		"if": claude_tag_condition_text(sprintf(" && %s", [claude_same_repo_head_condition])),
		"steps": [claude_tag_fork_refusal_step, {"uses": claude_action}],
	})
	not claude_tag_fork_refusal_message in violations
}

# Retiring the trigger retires the requirement, same as the head-guard rule.
test_claude_tag_without_the_issue_comment_trigger_needs_no_fork_refusal if {
	fixture := {"documents": [{
		"path": claude_tag_workflow_path,
		"contents": {
			"on": {"issues": {"types": ["opened"]}},
			"jobs": {"claude": {
				"if": "(github.event_name == 'issues' && trusted)",
				"steps": [{"uses": claude_action}],
			}},
		},
	}]}
	violations := deny with input as fixture
	not claude_tag_fork_refusal_message in violations
}

test_codeql_init_hardcoding_a_language_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"uses": "github/codeql-action/init@v4",
			"with": {"languages": "rust"},
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s CodeQL init must consume matrix.language", [codeql_workflow_path]) in violations
}

# init snapshots the workspace, so anything staged after it is invisible to the
# extractor while the job still reports a successful analysis.
codeql_pull_request_message := sprintf("%s must analyze every pull request: keep on.pull_request present and unfiltered", [codeql_workflow_path])

test_codeql_dropping_the_pull_request_trigger_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"on": {"push": {"branches": ["main"]}}},
	}]}
	violations := deny with input as fixture
	codeql_pull_request_message in violations
}

# A filtered trigger still "runs on pull requests", so the old wording accused
# the maintainer of the opposite of what they did.
test_codeql_filtered_pull_request_trigger_is_denied if {
	every trigger in {
		{"types": ["opened", "synchronize", "reopened"]},
		{"branches": ["main"]},
		{"paths-ignore": ["docs/**"]},
	} {
		fixture := {"documents": [{
			"path": codeql_workflow_path,
			"contents": {"on": {"pull_request": trigger}},
		}]}
		violations := deny with input as fixture
		codeql_pull_request_message in violations
	}
}

test_bare_codeql_pull_request_trigger_is_accepted if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"on": {"pull_request": null}},
	}]}
	violations := deny with input as fixture
	not codeql_pull_request_message in violations
}

# Reaching through `codeql.on` made the whole rule undefined — silently green —
# when the trigger block itself was gone, the one shape that disables every arm.
test_codeql_losing_its_trigger_block_is_denied if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": []}}},
	}]}
	violations := deny with input as fixture
	codeql_pull_request_message in violations
}

# Dependabot pins a floating major to an exact release. That is a STRICTER pin
# and must not be rejected — #786 failed with "must analyze with CodeQL v4
# exactly once" against a step that was v4.37.1.
test_exact_release_pin_matches_the_major if {
	action_matches("github/codeql-action/analyze@v4.37.1", "github/codeql-action/analyze@v4")
	action_matches("anthropics/claude-code-action@v1.0.178", claude_action)
	action_matches("codecov/codecov-action@v7.1.2", codecov_action)
}

# A real major bump must STILL fail — that is what the pin is for.
test_major_bump_does_not_match if {
	not action_matches("github/codeql-action/analyze@v5.0.0", "github/codeql-action/analyze@v4")
	not action_matches("anthropics/claude-code-action@v2", claude_action)
}

# A different action that merely shares a version must not match.
test_unrelated_action_does_not_match if {
	not action_matches("evil/codeql-action/analyze@v4.37.1", "github/codeql-action/analyze@v4")
	not action_matches("github/codeql-action/init@v4.37.1", "github/codeql-action/analyze@v4")
}

# A ref that only PREFIXES the major is not beneath it (v40 is not v4.x).
test_prefix_lookalike_does_not_match if {
	not action_matches("github/codeql-action/analyze@v40.1.0", "github/codeql-action/analyze@v4")
}

# The Lighthouse rules matched `actions/upload-artifact@v7` literally, so an
# exact pin would have emptied their entry list and spuriously fired all four
# "must upload site/.lighthouseci/" denials — #786's failure, relocated.
test_lighthouse_rules_tolerate_an_exact_upload_artifact_pin if {
	fixture := {"documents": [{
		"path": lighthouse_workflow_path,
		"contents": {"jobs": {"lighthouse": {"steps": [{
			"uses": "actions/upload-artifact@v7.1.2",
			"if": "${{ !cancelled() }}",
			"with": {
				"path": "site/.lighthouseci/",
				"include-hidden-files": true,
				"if-no-files-found": "error",
			},
		}]}}},
	}]}
	violations := deny with input as fixture
	every message in {
		"must upload site/.lighthouseci/ exactly once",
		"Lighthouse upload must run under !cancelled()",
		"Lighthouse upload must include hidden files",
		"Lighthouse upload must fail when reports are absent",
	} {
		every violation in violations {
			not contains(violation, message)
		}
	}
}

composite_pin_fixture(dependabot_contents) := {"documents": [
	{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{"uses": codecov_action}]}},
	},
	{"path": dependabot_config_path, "contents": dependabot_contents},
]}

uncovered_composite_message := sprintf(
	"%s must list a github-actions directory covering /%s/upload-codecov: %s is otherwise invisible to Dependabot",
	[dependabot_config_path, composite_action_root, codecov_action],
)

# #784/#785 moved four third-party pins into composites; `directory: /` searches
# only `.github/workflows` and a root `action.yml`, so all four left coverage.
test_composite_pin_outside_dependabot_coverage_is_rejected if {
	fixture := composite_pin_fixture({"updates": [{
		"package-ecosystem": "github-actions",
		"directory": "/",
	}]})
	violations := deny with input as fixture
	uncovered_composite_message in violations
}

test_missing_dependabot_config_leaves_composite_pins_uncovered if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{"uses": codecov_action}]}},
	}]}
	violations := deny with input as fixture
	uncovered_composite_message in violations
}

# Copying the glob into the SINGULAR key is the likeliest way to get this wrong,
# and GitHub does not glob `directory` — Dependabot would resolve nothing.
test_globbed_singular_directory_does_not_cover_the_pin if {
	fixture := composite_pin_fixture({"updates": [{
		"package-ecosystem": "github-actions",
		"directory": "/.github/actions/*",
	}]})
	violations := deny with input as fixture
	uncovered_composite_message in violations
}

# The glob form dependabot-core#6704 confirms works must stay silent.
test_composite_directory_glob_covers_the_pin if {
	fixture := composite_pin_fixture({"updates": [{
		"package-ecosystem": "github-actions",
		"directories": ["/", "/.github/actions/*"],
	}]})
	violations := deny with input as fixture
	not uncovered_composite_message in violations
}

# So must the upstream one-entry-per-composite workaround, which predates
# `directories` and uses the singular key.
test_per_composite_directory_entry_covers_the_pin if {
	fixture := composite_pin_fixture({"updates": [
		{"package-ecosystem": "github-actions", "directory": "/"},
		{
			"package-ecosystem": "github-actions",
			"directory": "/.github/actions/upload-codecov",
		},
	]})
	violations := deny with input as fixture
	not uncovered_composite_message in violations
}

# A composite that only calls sibling actions has nothing for Dependabot to
# bump, so an uncovered directory is not a finding.
test_local_composite_reference_needs_no_dependabot_coverage if {
	fixture := {"documents": [
		{
			"path": ".github/actions/packaging-build/action.yml",
			"contents": {"runs": {"steps": [{"uses": codecov_wrapper}]}},
		},
		{
			"path": dependabot_config_path,
			"contents": {"updates": [{
				"package-ecosystem": "github-actions",
				"directory": "/",
			}]},
		},
	]}
	violations := deny with input as fixture
	every violation in violations {
		not contains(violation, "invisible to Dependabot")
	}
}

# A non-actions ecosystem's directory list must not launder the coverage.
test_other_ecosystem_directories_do_not_cover_composites if {
	fixture := composite_pin_fixture({"updates": [
		{"package-ecosystem": "github-actions", "directory": "/"},
		{"package-ecosystem": "npm", "directories": ["/.github/actions/*"]},
	]})
	violations := deny with input as fixture
	uncovered_composite_message in violations
}
