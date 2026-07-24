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
				"token": "${{ secrets.CODECOV_TOKEN }}",
			},
		},
	}
	codecov_route(entry) == {
		"path": codecov_workflow_path,
		"file": "target/nextest/ci/junit.xml",
		"flag": "windows",
		"report_type": "test_results",
		"if": "${{ !cancelled() }}",
		"token": "${{ secrets.CODECOV_TOKEN }}",
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

test_codecov_token_forwarding_is_pinned if {
	fixture := {"documents": [{
		"path": codecov_authority_path,
		"contents": {"runs": {"steps": [{
			"uses": codecov_action,
			"with": {"token": "broken"},
		}]}},
	}]}
	violations := deny with input as fixture
	sprintf("%s Codecov step must pass inputs.token", [codecov_authority_path]) in violations
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
				"token": codecov_token_secret,
			},
		}]}}},
	}]}
	uploads := codecov_uploads with input as fixture
	count(uploads) == 1
	uploads[0].path == path
	"the repository must contain exactly the six declared Codecov routes" in deny with input as fixture
}
