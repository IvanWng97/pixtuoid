#!/usr/bin/env bash
# The release checklist, as data. Sourced by scripts/release-e2e.sh, which
# exports LIB, REPO and VERSION before sourcing.
#
# Each record is "id|kind|title"; its body is the function named <kind>_<id>.
# release-e2e-selftest.sh pins the two halves in BOTH directions, so a renamed
# step reds instead of silently vanishing from the run.
#
# Bodies precede the records so the SC2034 directives below stay line-scoped —
# a directive before the file's first command applies to the whole file.

auto_openclaw_hermetic() { "$LIB/tier-openclaw-hermetic.sh"; }
auto_openclaw_multi() { "$LIB/tier-openclaw-multi.sh" "$@"; }
auto_openclaw_backend() { "$LIB/tier-openclaw-backend.sh"; }
auto_replay() { "$LIB/tier-replay.sh" "$@"; }

# shellcheck disable=SC2034  # read by the sourcing driver, not here
STEPS=(
    "openclaw_hermetic|auto|hermetic OpenClaw daemon tier"
    "replay|auto|captured rollout through the full headless path"
    "openclaw_multi|auto|N real OpenClaw gateways"
    "openclaw_backend|auto|real gateway + one BILLED model turn"
)

# shellcheck disable=SC2034  # read by the sourcing driver, not here
PHASE1_LAST=openclaw_backend
