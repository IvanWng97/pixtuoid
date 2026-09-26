#!/usr/bin/env bash
# Runs every contract in contracts.yml twice: against the file as committed,
# where it must pass, and against its `break`, where it must fail.
# `--selftest` feeds it contracts that are each wrong in one way it must report
# — a runner that stopped reporting would switch every contract off silently.
set -euo pipefail

self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

if [[ ${1:-} == --selftest ]]; then
    scratch=$(mktemp -d)
    trap 'rm -rf "$scratch"' EXIT
    printf 'answer: 42\n' >"$scratch/doc.yml"
    cat >"$scratch/contracts.yml" <<EOF
- {id: sound-contract, file: '$scratch/doc.yml', why: w, assert: '.answer == 42', break: '.answer = 0'}
- {id: vacuous-contract, file: '$scratch/doc.yml', why: w, assert: 'true', break: '.answer = 0'}
- {id: red-as-committed, file: '$scratch/doc.yml', why: w, assert: '.answer == 0', break: '.answer = 1'}
- {id: unappliable-break, file: '$scratch/doc.yml', why: w, assert: '.answer == 42', break: '.answer |'}
- {id: one-idle-break, file: '$scratch/doc.yml', why: w, assert: '.answer == 42', break: ['.answer = 0', '.other = 1']}
EOF
    if output=$(bash "$self" "$scratch/contracts.yml" 2>&1); then
        echo "error: check.sh selftest: the runner accepted contracts it must reject" >&2
        exit 1
    fi
    for id in vacuous-contract red-as-committed unappliable-break one-idle-break; do
        if ! grep -q -- "$id" <<<"$output"; then
            printf 'error: check.sh selftest: %s went unreported\n%s\n' "$id" "$output" >&2
            exit 1
        fi
    done
    if grep -q -- sound-contract <<<"$output"; then
        printf 'error: check.sh selftest: a sound contract was reported\n%s\n' "$output" >&2
        exit 1
    fi
    echo "check.sh selftest: each broken contract is reported"
    exit 0
fi

cd "$(git rev-parse --show-toplevel)"
contracts_path=${1:-policy/ci-observability/contracts.yml}
contracts=$(yq -o=json '.' "$contracts_path")
count=$(jq 'length' <<<"$contracts")
if ((count == 0)); then
    echo "error: $contracts_path holds no contracts" >&2
    exit 1
fi

failed=0
for ((i = 0; i < count; i++)); do
    id=$(jq -r ".[$i].id" <<<"$contracts")
    assertion=$(jq -r ".[$i].assert" <<<"$contracts")
    # One JSON string per line: a break written as a YAML block spans lines.
    breaks=$(jq -c ".[$i].break | [.] | flatten | .[]" <<<"$contracts")
    expected=$(jq -r ".[$i].expected // \"\"" <<<"$contracts")
    files=$(jq -r ".[$i].file | [.] | flatten | .[]" <<<"$contracts")
    while IFS= read -r file; do
        # yq's `==` is false for any two maps or arrays, identical ones included,
        # so yq only turns the file into JSON and jq makes every comparison.
        if ! document=$(yq -o=json '.' "$file"); then
            echo "error: $id cannot parse $file" >&2
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
            if ! broken=$(jq "$breakage" <<<"$document"); then
                printf "error: %s's break does not apply to %s:\n%s\n" "$id" "$file" "$breakage" >&2
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
