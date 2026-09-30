#!/usr/bin/env bash
# Shellcheck the body of every shebang recipe in the justfile. The justfile is
# not a shell file, so SHELL_SOURCES can't cover the bash it embeds; `just
# --dump` hands each body back as fragments — a string, or an interpolation.
# (Linewise recipes are single commands per line, run as `set shell` gives them.)
set -euo pipefail

# just inserts `{{ … }}` as literal TEXT, so an interpolation becomes a literal
# word: quoting checks never fire on it, exactly as at run time. What a literal
# word does trigger is the constant-word checks — a loop over one word (2043), a
# case on a constant (2194) — and those can't mask a `$var` bug.
LITERAL_WORD_EXCLUDES="SC2043,SC2194"

dump=$(just --dump --dump-format json)
names=$(jq -r '.recipes[] | select(.shebang) | .name' <<<"$dump")
# A reshaped dump would empty this list and pass having checked nothing.
[ -n "$names" ] || {
    echo "error: no shebang recipes found in \`just --dump\`" >&2
    exit 1
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail=0
checked=0
while IFS= read -r name; do
    jq -r --arg n "$name" '.recipes[$n].body[]
        | map(if type == "string" then . else "__JUST__" end) | join("")' \
        <<<"$dump" >"$tmp/$name.sh"
    # Body line N sits N lines below the recipe's header line in the justfile.
    header=$(grep -n -m1 -E "^${name}( |:)" justfile | cut -d: -f1)
    if ! shellcheck -f gcc -e "$LITERAL_WORD_EXCLUDES" "$tmp/$name.sh" |
        awk -F: -v off="$header" -v n="$name" '{ $1 = "justfile"; $2 += off; print $0 " (recipe " n ")" }' OFS=:; then
        fail=1
    fi
    checked=$((checked + 1))
done <<<"$names"
echo "$checked shebang recipe bodies shellchecked"
exit "$fail"
