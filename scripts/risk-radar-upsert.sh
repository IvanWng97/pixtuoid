#!/usr/bin/env bash
# Upsert the risk-radar sticky comment on a PR (idempotent across re-pushes).
#
# Finds an existing comment by its MARKER, then:
#   - body file non-empty -> PATCH it (or POST a new one)
#   - body file empty      -> DELETE a stale one (the PR dropped its risk paths)
#
# Soft by design: the caller runs it `continue-on-error`, so a read-only fork
# token that cannot write a comment never fails the job — the radar is also
# written to the step summary, which always works.
#
# Usage: risk-radar-upsert.sh <pr-number> <body-file>
# Env:   GH_TOKEN (required), GH_REPO=owner/repo (optional; else `gh repo view`).
set -euo pipefail

pr="${1:?usage: risk-radar-upsert.sh <pr-number> <body-file>}"
body_file="${2:?usage: risk-radar-upsert.sh <pr-number> <body-file>}"
marker='<!-- risk-radar -->'
repo="${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"

# First existing radar comment id (empty if none). --paginate can emit one id
# per page, so take the first.
existing_id="$(gh api "repos/$repo/issues/$pr/comments" --paginate \
    --jq ".[] | select(.body | startswith(\"$marker\")) | .id" | head -n1 || true)"

if [ -s "$body_file" ]; then
    body="$(cat "$body_file")"
    if [ -n "$existing_id" ]; then
        gh api -X PATCH "repos/$repo/issues/comments/$existing_id" -f body="$body" >/dev/null
        echo "risk-radar: updated comment $existing_id"
    else
        gh api -X POST "repos/$repo/issues/$pr/comments" -f body="$body" >/dev/null
        echo "risk-radar: created comment"
    fi
elif [ -n "$existing_id" ]; then
    gh api -X DELETE "repos/$repo/issues/comments/$existing_id" >/dev/null
    echo "risk-radar: removed stale comment $existing_id (no risk seams)"
else
    echo "risk-radar: no risk seams, nothing to post"
fi
