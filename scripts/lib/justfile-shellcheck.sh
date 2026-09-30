#!/usr/bin/env bash
# Shellcheck the body of every shebang recipe in the justfile. The justfile is
# not a shell file, so SHELL_SOURCES can't cover the bash it embeds; `just
# --dump` hands each body back as fragments — a string, or an interpolation.
set -euo pipefail

# just expands `{{ … }}` as TEXT before bash runs, so on a line holding one
# these checks describe bash expansion that never happens there: quoting
# (2086, 2206), single-quoted expansion (2016) and constant words (2043, 2194).
INTERP_LINE_EXCLUDES="SC2016,SC2043,SC2086,SC2194,SC2206"

dump=$(just --dump --dump-format json)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail=0
while IFS= read -r name; do
    jq -r --arg n "$name" --arg ex "$INTERP_LINE_EXCLUDES" '.recipes[$n].body[]
        | (if any(type != "string") then "# shellcheck disable=\($ex)\n" else "" end)
          + (map(if type == "string" then . else "${JUST_INTERP}" end) | join(""))' \
        <<<"$dump" >"$tmp/$name.sh"
    if ! shellcheck "$tmp/$name.sh" | sed "s|$tmp/$name.sh|justfile recipe '$name'|"; then
        fail=1
    fi
done < <(jq -r '.recipes[] | select(.shebang) | .name' <<<"$dump")
exit "$fail"
