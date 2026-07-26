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
	every value in {
		"Codecov/codecov-action@v7",
		"CODECOV/CODECOV-ACTION@v7",
		"codecov/Codecov-Action@v7",
	} {
		path := ".github/workflows/rogue.yml"
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
	initialize := codeql_steps_using("github/codeql-action/init@v4") with codeql_steps as steps
	analyze := codeql_steps_using("github/codeql-action/analyze@v4") with codeql_steps as steps
	checkout[0].index == 0
	initialize[0].index == 1
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

test_codeql_rust_setup_cannot_use_rolling_stable if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"name": rust_setup_step_name,
			"if": rust_matrix_condition,
			"run": "rustup component add rust-src rust-analyzer --toolchain stable",
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must derive one workspace MSRV with cargo metadata", [codeql_workflow_path]) in violations
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

test_codeql_analyze_must_expose_sarif_output if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{"uses": "github/codeql-action/analyze@v4"}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s CodeQL analyze step must have id: analyze", [codeql_workflow_path]) in violations
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

test_missing_codeql_semantic_inputs_are_rejected if {
	fixture := {"documents": [{
		"path": codeql_workflow_path,
		"contents": {"jobs": {"analyze": {"steps": [{
			"name": rust_setup_step_name,
			"if": rust_matrix_condition,
			"run": "rustup component add rust-src --toolchain stable\ntest -s \"$rust_source/std/src/lib.rs\"\n",
		}]}}},
	}]}
	violations := deny with input as fixture
	sprintf("%s must derive one workspace MSRV with cargo metadata", [codeql_workflow_path]) in violations
	sprintf("%s must install the declared MSRV before CodeQL init", [codeql_workflow_path]) in violations
	sprintf("%s must derive CodeQL's sysroot from the declared MSRV", [codeql_workflow_path]) in violations
	sprintf("%s must verify the sysroot proc-macro server before CodeQL init", [codeql_workflow_path]) in violations
	sprintf("%s must pass the verified sysroot to the Rust extractor", [codeql_workflow_path]) in violations
	sprintf("%s must pass the verified rust-src path to the Rust extractor", [codeql_workflow_path]) in violations
	sprintf("%s must pass the verified proc-macro server to the Rust extractor", [codeql_workflow_path]) in violations
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

# The exact payload that shipped in #785: the root "properties" object is never
# closed (10 `{` against 9 `}`), so every `claude review` and
# `claude security review` run aborted with "--json-schema is not valid JSON".
broken_json_schema_args := `--max-turns 60 --allowedTools "Read,Glob,Grep" --json-schema '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]'`

fixed_json_schema_args := `--max-turns 60 --allowedTools "Read,Glob,Grep" --json-schema '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}'`

json_schema_fixture(args) := {"documents": [{
	"path": claude_reusable_workflow_path,
	"contents": {"jobs": {"analyze": {"steps": [{
		"uses": claude_action,
		"with": {"claude_args": args},
	}]}}},
}]}

malformed_json_schema_message := sprintf(
	"%s %s must carry exactly one single-quoted, well-formed JSON Schema payload",
	[claude_reusable_workflow_path, json_schema_flag],
)

test_unbalanced_json_schema_payload_is_denied if {
	violations := deny with input as json_schema_fixture(broken_json_schema_args)
	malformed_json_schema_message in violations
}

test_balanced_json_schema_payload_is_accepted if {
	violations := deny with input as json_schema_fixture(fixed_json_schema_args)
	not malformed_json_schema_message in violations
}

# Each variant is a distinct way the payload can be unreadable to the CLI while
# still satisfying the flag-name-only presence check.
test_unreadable_json_schema_payloads_are_denied if {
	every args in {
		# Unquoted, so quote-removal mangles the payload before the CLI sees it.
		`--json-schema {"type":"object"}`,
		# Opening quote with no closing quote.
		`--json-schema '{"type":"object"}`,
		# Valid JSON, but a scalar is not a schema.
		`--json-schema '123'`,
		# Well-formed JSON that is not a well-formed JSON Schema.
		`--json-schema '{"type":"not-a-type"}'`,
		# Two payloads make the effective schema ambiguous.
		`--json-schema '{"type":"object"}' --json-schema '{"type":"array"}'`,
		# Empty payload.
		`--json-schema ''`,
	} {
		violations := deny with input as json_schema_fixture(args)
		malformed_json_schema_message in violations
	}
}

test_claude_args_without_a_schema_flag_is_not_denied if {
	violations := deny with input as json_schema_fixture(`--max-turns 60 --allowedTools "Read,Glob,Grep"`)
	not malformed_json_schema_message in violations
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

ci_gate_membership_violations(jobs) := [msg |
	some msg in deny with input as ci_gate_fixture(jobs)
	contains(msg, "must gate exactly the non-advisory group jobs")
]

ci_gate_unread_violations(jobs) := [msg |
	some msg in deny with input as ci_gate_fixture(jobs)
	contains(msg, "must read needs.")
]

test_ci_gate_covering_every_group_is_accepted if {
	count(ci_gate_membership_violations(ci_gate_shipped_jobs)) == 0
	count(ci_gate_unread_violations(ci_gate_shipped_jobs)) == 0
}

# The gap: a fifth reusable group lands and nobody adds it to the gate, so the
# single protected context stays green while that whole group can fail.
test_ci_gate_missing_a_new_group_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"security": {"uses": "./.github/workflows/ci-security.yml"}})
	count(ci_gate_membership_violations(jobs)) == 1
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
	count(ci_gate_membership_violations(jobs)) == 1
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
	count(ci_gate_membership_violations(jobs)) == 1
}

# The schema is not required to be the LAST flag in claude_args; an earlier
# draft cut at the end of the string and rejected every valid payload followed
# by another argument.
test_json_schema_before_another_flag_is_accepted if {
	args := sprintf("%s '{\"type\":\"object\"}' --max-turns 60", [json_schema_flag])
	violations := deny with input as json_schema_fixture(args)
	not malformed_json_schema_message in violations
}

test_json_schema_unterminated_before_another_flag_is_denied if {
	args := sprintf("%s '{\"type\":\"object\"} --max-turns 60", [json_schema_flag])
	violations := deny with input as json_schema_fixture(args)
	malformed_json_schema_message in violations
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
	count(ci_gate_membership_violations(jobs)) == 0
	count(ci_gate_unread_violations(jobs)) == 0
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
	count(ci_gate_membership_violations(jobs)) == 1
}

test_cross_repo_group_call_granting_oidc_is_denied if {
	jobs := object.union(ci_gate_shipped_jobs, {"builds": {
		"uses": "someorg/shared/.github/workflows/ci-builds.yml@v1",
		"permissions": {"contents": "read", "id-token": "write"},
	}})
	violations := deny with input as ci_gate_fixture(jobs)
	ci_oidc_call_message("builds") in violations
}
