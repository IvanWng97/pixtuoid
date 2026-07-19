#!/usr/bin/env bash
# PreToolUse(Bash) guard for the repo's hardest, prose-only prohibitions
# (CLAUDE.md "Things NOT to do"): make the irreversible ones a deterministic
# prompt instead of an honor-system rule. NOT the removed check_dod class
# (self-attestation of a judgment call) — a zero-ceremony mechanical guard,
# the "real teeth live in automated checks" philosophy.
#
# Match a git/gh INVOCATION, not a mention: every pattern is anchored to a
# command-start boundary (line start, a shell separator, or trailing `VAR=val`
# env-assignments / a wrapper like the user's `rtk` rewrite of `git push` ->
# `rtk git push`), so a `push` inside a quoted commit message or an
# `rg -- "--no-verify"` search does NOT fire. All three decisions are `ask`
# (never a hard block): a human OKs the op, and any residual false-positive is
# one keystroke, not a wedged workflow.
#
# Fail-OPEN on any parse error or a missing jq — a guard must never block a
# legitimate command; the prose rule + the human stay the backstop. Binds
# Claude-on-Unix only: Windows and non-Claude tools (Codex / openclaw read
# AGENTS.md, never these hooks) fall back to the prose rule. Residual known gap:
# the `git commit -n` short form of --no-verify is not matched (bare `-n` is
# overloaded — `git push -n` is a harmless --dry-run — so it is left to prose).

command -v jq >/dev/null 2>&1 || exit 0

input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -n "$cmd" ] || exit 0

emit() {
	# $1 = permissionDecision (ask|deny), $2 = reason shown to the human.
	printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"%s","permissionDecisionReason":"%s"}}\n' "$1" "$2"
	exit 0
}

# A command-start boundary: line start, a shell separator (`;` `&` `|` `(`),
# then optional `VAR=val ` env-assignments and optional wrappers (rtk/sudo/…).
# This is what tells a real invocation apart from the phrase inside a quote.
# Concatenate "$lead" (expanded) with a single-quoted tail so `$` stays a literal
# end-anchor for grep, never a shell expansion.
lead='(^|[;&|(])[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*((rtk|sudo|env|command|time|nice|xargs)[[:space:]]+)*'

# Absolute repo rule: never skip the git hooks (preflight is the pre-push gate).
# Requires a git invocation AND `--no-verify` as a whitespace-delimited token, so
# `rg -- "--no-verify"` (no git) and `-m "…--no-verify"` (quote-hugged) don't fire.
if printf '%s' "$cmd" | grep -Eq -- "${lead}"'git[[:space:]].*[[:space:]]--no-verify([[:space:]]|$)'; then
	emit ask "Repo rule (CLAUDE.md 'Things NOT to do'): no --no-verify / hook-skipping on git ops — confirm."
fi

# Irreversible / outward-facing: a human OKs every push and merge. Flags between
# git and the push subcommand — incl. `-C <dir>` / `-c <cfg>` that take a
# separate-word arg — are skipped; `commit` etc. is not `push`, so a `push`
# inside a message is not the subcommand and does not match.
if printf '%s' "$cmd" | grep -Eq -- "${lead}"'git[[:space:]]+((-[Cc][[:space:]]+[^[:space:]]+|-{1,2}[^[:space:]]+)[[:space:]]+)*push([[:space:]]|$)'; then
	emit ask "Repo rule: never 'git push' (incl. --force) without explicit human confirmation."
fi
if printf '%s' "$cmd" | grep -Eq -- "${lead}"'gh[[:space:]]+pr[[:space:]]+merge'; then
	emit ask "Repo rule: a human merges — confirm 'gh pr merge' explicitly."
fi

exit 0
