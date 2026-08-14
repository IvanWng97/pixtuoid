#!/usr/bin/env bash
# Gate for the release-e2e machinery. Every assertion here has been shown to FAIL
# when its subject is broken — a selftest that cannot red is decoration.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB="$HERE/lib"
# shellcheck source=/dev/null
. "$LIB/e2e-common.sh"

FAIL=0
ok() { printf '  PASS %s\n' "$1"; }
no() {
    printf '  FAIL %s — %s\n' "$1" "$2" >&2
    FAIL=1
}
eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else no "$1" "wanted '$3', got '$2'"; fi
}

want_root="$(git -C "$HERE" rev-parse --show-toplevel)"
eq repo-root "$(e2e_repo_root)" "$want_root"

sb="$(e2e_sandbox)"
eq sandbox-exists "$([ -d "$sb" ] && echo yes)" yes
eq sandbox-private "$(stat -f '%Lp' "$sb")" 700
rmdir "$sb"

eq require-bin-message \
    "$(e2e_require_bin /nonexistent/pixtuoid 2>&1)" \
    "missing /nonexistent/pixtuoid — run: just build --release"
(e2e_require_bin /nonexistent/pixtuoid >/dev/null 2>&1)
eq require-bin-status "$?" 2
(e2e_require_bin "$(command -v sh)" >/dev/null 2>&1)
eq require-bin-accepts-executable "$?" 0

REPO="$(e2e_repo_root)"
VERSION=0.0.0
export LIB REPO VERSION
# shellcheck source=/dev/null
. "$LIB/checklist.sh"

ids=""
bad=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id kind _ <<<"$s"
    ids="$ids $id"
    case "$kind" in
    auto | manual) ;;
    *)
        no "record:$id" "unknown kind '$kind'"
        bad=1
        ;;
    esac
    if ! declare -F "${kind}_${id}" >/dev/null; then
        no "record:$id" "no ${kind}_${id} function"
        bad=1
    fi
done
[ "$bad" -eq 0 ] && ok "every record has its body"

bad=0
while read -r fn; do
    bare="${fn#auto_}"
    bare="${bare#manual_}"
    case " $ids " in
    *" $bare "*) ;;
    *)
        no "body:$fn" "function has no STEPS record"
        bad=1
        ;;
    esac
done < <(declare -F | awk '{print $3}' | grep -E '^(auto|manual)_')
[ "$bad" -eq 0 ] && ok "every body has its record"

case " $ids " in
*" $PHASE1_LAST "*) ok "PHASE1_LAST names a real step" ;;
*) no PHASE1_LAST "'$PHASE1_LAST' is not a step id" ;;
esac

tmp="$(e2e_sandbox)"
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --list 0.0.0 >/dev/null
eq list-exits-clean "$?" 0
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --only zz_no_such_step >/dev/null 2>&1
eq unknown-only-reds "$?" 2
# A manual step must not self-approve when nothing can answer the prompt.
if grep -q '|manual|' <(printf '%s\n' "${STEPS[@]}"); then
    first_manual="$(printf '%s\n' "${STEPS[@]}" | grep -m1 '|manual|' | cut -d'|' -f1)"
    XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --only "$first_manual" </dev/null >/dev/null 2>&1
    eq manual-refuses-non-tty "$?" 1
else
    ok "manual-refuses-non-tty (no manual step yet)"
fi
rm -rf "$tmp"

[ "$FAIL" -eq 0 ] || exit 1
echo "release-e2e selftest: all checks passed"
