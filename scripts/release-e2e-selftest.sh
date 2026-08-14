#!/usr/bin/env bash
# Gate for the release-e2e machinery. Every assertion here has been shown to FAIL
# against a broken subject — a selftest that cannot red is decoration.
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
# `stat -f` is BSD-only and `stat -c` is GNU-only; find's -perm is in POSIX, so the
# gate stays runnable on the Linux hygiene job.
eq sandbox-private "$(find "$sb" -maxdepth 0 -perm 700 | wc -l | tr -d ' ')" 1
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
E2E_BLOCK_REASON="$(mktemp)"
export E2E_BLOCK_REASON
# shellcheck source=/dev/null
. "$LIB/checklist.sh"

ids=""
bad=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id phase kind _ <<<"$s"
    ids="$ids $id"
    case "$phase" in
    pre | post) ;;
    *)
        no "record:$id" "unknown phase '$phase'"
        bad=1
        ;;
    esac
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
[ "$bad" -eq 0 ] && ok "every record has its body, a known phase and a known kind"

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

# The gap that let `replay` ship unpassable: the name pins held, and nothing
# checked that a body survives the arguments a full run actually supplies (none).
bad=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id _ kind _ <<<"$s"
    [ "$kind" = auto ] || continue
    case "$id" in
    # These reach the network, a gateway, or a paid model turn.
    verify | build | site_e2e | npm_check | ci_green | release_smoke | published | openclaw_* | corpus | preconditions | wasm_fresh) continue ;;
    esac
    (auto_"$id" >/dev/null 2>&1)
    rc=$?
    # 0 = ran, 2 = BLOCKED (a prerequisite this host lacks — CI has no release
    # build). Anything else means the body cannot be driven by a full run at all,
    # which is how `replay` shipped unpassable.
    if [ "$rc" -ne 0 ] && [ "$rc" -ne 2 ]; then
        no "invocable:$id" "auto_$id exits $rc with the arguments a full run supplies"
        bad=1
    fi
done
[ "$bad" -eq 0 ] && ok "every argument-less auto body is invocable"

tmp="$(e2e_sandbox)"
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --list >/dev/null
eq list-exits-clean "$?" 0
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --only zz_no_such_step >/dev/null 2>&1
eq unknown-only-reds "$?" 2
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --from zz_no_such_step >/dev/null 2>&1
eq unknown-from-reds "$?" 2
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" 0.0.0 --list >/dev/null 2>&1
eq version-mismatch-reds "$?" 2
first_manual="$(printf '%s\n' "${STEPS[@]}" | grep -m1 '|manual|' | cut -d'|' -f1)"
XDG_STATE_HOME="$tmp" "$HERE/release-e2e.sh" --only "$first_manual" </dev/null >/dev/null 2>&1
eq manual-refuses-non-tty "$?" 1
rm -rf "$tmp" "$E2E_BLOCK_REASON"

[ "$FAIL" -eq 0 ] || exit 1
echo "release-e2e selftest: all checks passed"
