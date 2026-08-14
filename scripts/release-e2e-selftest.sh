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

[ "$FAIL" -eq 0 ] || exit 1
echo "release-e2e selftest: all checks passed"
