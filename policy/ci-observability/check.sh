#!/usr/bin/env bash
# Runs every contract in contracts.yml against each of its files as committed,
# where it must pass, and against each of its breaks, where it must fail.
# `--selftest` runs it on one sound contract list and on lists that are each
# wrong in one way: a runner that stopped failing on any of them would switch
# every contract off silently.
set -euo pipefail

self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

if [[ ${1:-} == --selftest ]]; then
    scratch=$(mktemp -d)
    trap 'rm -rf "$scratch"' EXIT
    printf 'answer: 42\n' >"$scratch/good.yml"
    printf 'answer: 0\n' >"$scratch/bad.yml"
    printf 'answer: 1\n---\nanswer: 42\n' >"$scratch/two-document-target.yml"
    doc="file: '$scratch/good.yml', why: w"
    # One run per case: a branch that reports a problem without failing the
    # run would otherwise hide behind another case that does fail it.
    reject() {
        local name=$1 output
        printf '%s\n' "$2" >"$scratch/$name.yml"
        if output=$(bash "$self" "$scratch/$name.yml" 2>&1); then
            printf 'error: check.sh selftest: accepted %s\n%s\n' "$name" "$output" >&2
            exit 1
        fi
    }
    reject vacuous "- {id: a, $doc, assert: 'true', break: '.answer = 0'}"
    reject red-as-committed "- {id: a, $doc, assert: '.answer == 0', break: '.answer = 1'}"
    reject unappliable-break "- {id: a, $doc, assert: '.answer == 42', break: '.answer |'}"
    reject one-idle-break "- {id: a, $doc, assert: '.answer == 42', break: ['.answer = 0', '.other = 1']}"
    reject second-file-red "- {id: a, file: ['$scratch/good.yml', '$scratch/bad.yml'], why: w, assert: '.answer == 42', break: '.answer = 0'}"
    reject missing-break "- {id: a, $doc, assert: '.answer == 42'}"
    reject null-break "- {id: a, $doc, assert: '.answer == 42', break: null}"
    reject break-yields-no-document "- {id: a, $doc, assert: '.answer == 42', break: 'null'}"
    reject misspelled-key "- {id: a, $doc, assert: '.answer == 42', break: '.answer = 0', expcted: x}"
    reject duplicate-id "- {id: a, $doc, assert: '.answer == 42', break: '.answer = 0'}
- {id: a, $doc, assert: '.answer == 42', break: '.answer = 0'}"
    reject several-results "- {id: a, $doc, assert: '(.answer == 0), (.answer == 42)', break: '.answer = 0'}"
    reject no-contracts "[]"
    reject missing-file "- {id: a, file: '$scratch/absent.yml', why: w, assert: '.answer == 42', break: '.answer = 0'}"
    reject two-document-file "- {id: a, file: '$scratch/two-document-target.yml', why: w, assert: '.answer == 42', break: 'select(.answer == 42) | .answer = 0'}"
    reject two-document-contracts "- {id: a, $doc, assert: '.answer == 42', break: '.answer = 0'}
---
- {id: b, $doc, assert: '.answer == 42', break: '.answer = 0'}"
    printf '%s\n' "- {id: a, $doc, assert: '.answer == 42', break: '.answer = 0'}" >"$scratch/sound.yml"
    if ! output=$(bash "$self" "$scratch/sound.yml" 2>&1); then
        printf 'error: check.sh selftest: rejected a sound contract\n%s\n' "$output" >&2
        exit 1
    fi
    echo "check.sh selftest: a sound contract passes and each broken one fails the run"
    exit 0
fi

cd "$(git rev-parse --show-toplevel)"
contracts_path=${1:-policy/ci-observability/contracts.yml}
contracts=$(yq -o=json '.' "$contracts_path")
# A second document here would reach the loop's arithmetic as two counts, which
# bash rejects without tripping `set -e`: every contract would go unrun.
if [[ $(jq -s 'length' <<<"$contracts") != 1 ]]; then
    echo "error: $contracts_path must be one YAML document" >&2
    exit 1
fi

# Shape first: an entry with no usable break, a misspelled key or a reused id
# would otherwise pass without proving anything.
shape=$(jq -r '
    def text: type == "string" and length > 0;
    def texts: text or (type == "array" and length > 0 and all(.[]; text));
    if type != "array" or length == 0 then "it holds no contracts"
    else
        (map(.id) | group_by(.)[] | select(length > 1) | "duplicate id \(.[0])"),
        (.[] | (.id // "?") as $id
            | ((keys - ["id", "file", "why", "assert", "break", "expected"])[] | "\($id): unknown key `\(.)`"),
              ((["id", "file", "why", "assert", "break"] - keys)[] | "\($id): missing `\(.)`"),
              (select((.id | text) and (.why | text) and (.assert | text) | not)
                  | "\($id): id, why and assert must be non-empty strings"),
              (select(.file | texts | not) | "\($id): file must be a path or a non-empty list of paths"),
              (select(.break | texts | not) | "\($id): break must be a jq edit or a non-empty list of them"),
              (select(has("expected") and (.expected | type) != "string") | "\($id): expected must be a string"))
    end' <<<"$contracts")
if [[ -n $shape ]]; then
    printf 'error: %s: %s\n' "$contracts_path" "$shape" >&2
    exit 1
fi
count=$(jq 'length' <<<"$contracts")

failed=0
for ((i = 0; i < count; i++)); do
    id=$(jq -r ".[$i].id" <<<"$contracts")
    # Exactly one truthy result: jq -e alone judges only the last of several.
    assertion="[$(jq -r ".[$i].assert" <<<"$contracts")] | length == 1 and .[0] != false and .[0] != null"
    # One JSON string per line: a break written as a YAML block spans lines.
    breaks=$(jq -c ".[$i].break | [.] | flatten | .[]" <<<"$contracts")
    expected=$(jq -r ".[$i].expected // \"\"" <<<"$contracts")
    files=$(jq -r ".[$i].file | [.] | flatten | .[]" <<<"$contracts")
    while IFS= read -r file; do
        # yq's `==` is false for any two maps or arrays, identical ones included,
        # so yq only turns the file into JSON and jq makes every comparison.
        if ! document=$(yq -o=json '.' "$file") || [[ $(jq -s 'length' <<<"$document") != 1 ]]; then
            echo "error: $id cannot read $file as one document" >&2
            failed=1
            continue
        fi
        if ! jq -e --arg expected "$expected" "$assertion" <<<"$document" >/dev/null; then
            printf 'error: %s breaks contract %s\n  %s\n' "$file" "$id" "$(jq -r ".[$i].why" <<<"$contracts")" >&2
            failed=1
            continue
        fi
        while IFS= read -r encoded; do
            breakage=$(jq -r '.' <<<"$encoded")
            if ! broken=$(jq -c "$breakage" <<<"$document") ||
                [[ $(jq -s 'length == 1 and (.[0] | type) == "object"' <<<"$broken") != true ]]; then
                printf "error: %s's break does not yield one document from %s:\n%s\n" "$id" "$file" "$breakage" >&2
                failed=1
            elif jq -e --arg expected "$expected" "$assertion" <<<"$broken" >/dev/null 2>&1; then
                printf 'error: %s still passes on %s after this break, so that clause cannot fire:\n%s\n' "$id" "$file" "$breakage" >&2
                failed=1
            fi
        done <<<"$breaks"
    done <<<"$files"
done

if ((failed)); then
    exit 1
fi
echo "$count CI contracts hold, and each one fires on every break"
