#!/usr/bin/env bash
# The release checklist, as data. Sourced by scripts/release-e2e.sh, which exports
# LIB and REPO before sourcing and VERSION before the first step runs.
#
# Each record is "id|phase|kind|title"; its body is the function named <kind>_<id>.
# release-e2e-selftest.sh pins the two halves in BOTH directions and smoke-invokes
# every argument-less auto body, so a step that cannot run reds there.
#
# An auto body returns 2 to mean BLOCKED — it could not run here — and writes one
# line of reason to $E2E_BLOCK_REASON. Any other non-zero is a real failure.
#
# Bodies precede the records so the SC2034 directives below stay line-scoped —
# a directive before the file's first command applies to the whole file.

blocked() {
    printf '%s\n' "$1" >"$E2E_BLOCK_REASON"
    return 2
}

# The tiers already exit 2 for a missing prerequisite (e2e_require_bin, no
# `openclaw` on PATH, a busy port), so that convention maps straight onto BLOCKED.
run_tier() {
    local script="$1" rc
    shift
    "$LIB/$script" "$@"
    rc=$?
    [ "$rc" -eq 2 ] && printf 'prerequisite missing — see the tier output above\n' >"$E2E_BLOCK_REASON"
    return "$rc"
}

auto_openclaw_hermetic() { run_tier tier-openclaw-hermetic.sh; }
auto_openclaw_multi() { run_tier tier-openclaw-multi.sh "$@"; }
auto_openclaw_backend() { run_tier tier-openclaw-backend.sh; }

# A full-checklist run forwards no arguments, so the step needs its own fixture —
# without one the tier's `${1:?usage}` made this step unpassable.
auto_replay() {
    local fx
    if [ "$#" -gt 0 ]; then
        run_tier tier-replay.sh "$@"
        return $?
    fi
    fx="$(find "$REPO/crates/pixtuoid-core/tests/sources/fixtures/codex" -name 'rollout-*.jsonl' 2>/dev/null | sort | head -1)"
    [ -n "$fx" ] || blocked "no codex rollout fixture in-tree" || return 2
    run_tier tier-replay.sh "$fx" 1
}

auto_preconditions() {
    local cur behind
    if ! git -C "$REPO" diff --quiet || ! git -C "$REPO" diff --cached --quiet; then
        echo "uncommitted changes — a release must not sweep up edits" >&2
        return 1
    fi
    git -C "$REPO" fetch --quiet origin main 2>/dev/null || true
    # BEHIND is the poison (a stale checkout drafts against old numbers); AHEAD is
    # the normal state of a release branch with an open PR.
    behind="$(git -C "$REPO" rev-list --count HEAD..origin/main)"
    if [ "$behind" != 0 ]; then
        echo "HEAD is $behind commit(s) behind origin/main — a stale checkout poisons a release" >&2
        return 1
    fi
    cur="$(grep -m1 '^version' "$REPO/Cargo.toml" | cut -d'"' -f2)"
    if [ "$cur" != "$VERSION" ]; then
        echo "Cargo.toml is $cur, expected $VERSION" >&2
        return 1
    fi
    (cd "$REPO" && just notes-curated) || return 1
    if git -C "$REPO" rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
        echo "tag v$VERSION already exists locally" >&2
        return 1
    fi
    if [ -n "$(git -C "$REPO" ls-remote --tags origin "v$VERSION")" ]; then
        echo "tag v$VERSION already on origin" >&2
        return 1
    fi
}

auto_verify() { (cd "$REPO" && just verify); }
auto_npm_check() { (cd "$REPO" && just npm-check); }
auto_build() { (cd "$REPO" && just build --release --bins --examples); }
auto_site_e2e() { (cd "$REPO" && just site-e2e); }

# gen-wasm-check only proves the committed pair matches its own manifest.sha256 —
# a stale set is perfectly self-consistent, so compare commit times too.
auto_wasm_fresh() {
    local engine wasm
    (cd "$REPO" && just gen-wasm-check) || return 1
    engine="$(git -C "$REPO" log -1 --format=%ct -- crates/pixtuoid-scene crates/pixtuoid-web)"
    wasm="$(git -C "$REPO" log -1 --format=%ct -- site/public/wasm)"
    if [ -z "$engine" ] || [ -z "$wasm" ]; then
        blocked "no commit history for the scene/wasm paths (shallow clone?)"
        return 2
    fi
    if [ "$wasm" -lt "$engine" ]; then
        echo "site/public/wasm predates the last scene/web change — run just gen-wasm" >&2
        return 1
    fi
}

# The bot's verdict must be bound to the commit it reviewed — a stale `Findings: 0`
# from an earlier push is exactly the #316 re-litigation class.
auto_ci_green() {
    local branch pr head
    branch="$(git -C "$REPO" rev-parse --abbrev-ref HEAD)"
    if [ "$branch" = HEAD ]; then
        blocked "detached HEAD — no branch to resolve a PR from"
        return 2
    fi
    pr="$(gh pr list --head "$branch" --state all --limit 1 --json number --jq '.[0].number // empty')"
    if [ -z "$pr" ]; then
        blocked "no PR found for branch $branch"
        return 2
    fi
    head="$(gh pr view "$pr" --json headRefOid --jq .headRefOid)"
    if [ "$head" != "$(git -C "$REPO" rev-parse HEAD)" ]; then
        blocked "PR #$pr head is $head, local HEAD differs — push first"
        return 2
    fi
    gh pr checks "$pr" || return 1
    gh pr view "$pr" --json comments \
        --jq '[.comments[] | select(.body | test("Findings: *0"))] | length' |
        grep -qv '^0$' || {
        echo "no 'Findings: 0' comment on PR #$pr" >&2
        return 1
    }
}

# Both the roster and each root come from the registry — never a bash-side copy.
auto_corpus() {
    local cc="$REPO/target/release/examples/corpus_check" s rc=0 status absent="" roster
    if [ ! -x "$cc" ]; then
        blocked "corpus_check not built — run: just build --release --examples"
        return 2
    fi
    roster="$("$cc" --sources)" || return 1
    if [ -z "$roster" ]; then
        echo "corpus_check --sources returned an empty roster" >&2
        return 1
    fi
    for s in $roster; do
        echo "  --- $s"
        "$cc" "$s"
        status=$?
        case "$status" in
        0) ;;
        3) absent="$absent $s" ;;
        *) rc=1 ;;
        esac
    done
    [ -n "$absent" ] && echo "  NOT COVERED (no local corpus):$absent"
    return "$rc"
}

# Binds to the run this step dispatched: anything else watches whatever was newest,
# and a stale green run returns 0 immediately having observed nothing.
auto_release_smoke() {
    local branch t0 id tries=0
    branch="$(git -C "$REPO" rev-parse --abbrev-ref HEAD)"
    if [ -z "$(git -C "$REPO" ls-remote --heads origin "$branch")" ]; then
        blocked "branch $branch is not on origin — push it before dispatching CI"
        return 2
    fi
    t0="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    gh workflow run release-smoke.yml --ref "$branch" || return 1
    while [ "$tries" -lt 30 ]; do
        id="$(gh run list --workflow=release-smoke.yml --branch "$branch" --event workflow_dispatch \
            --json databaseId,createdAt --jq "[.[] | select(.createdAt > \"$t0\")] | .[0].databaseId // empty")"
        [ -n "$id" ] && break
        tries=$((tries + 1))
        sleep 4
    done
    if [ -z "$id" ]; then
        blocked "dispatched run never appeared for $branch"
        return 2
    fi
    gh run watch "$id" --exit-status
}

auto_tag_ready() {
    printf '  tag this exact commit:\n    git tag -a v%s %s -m "pixtuoid %s"\n' \
        "$VERSION" "$(git -C "$REPO" rev-parse HEAD)" "$VERSION"
    printf '  then push it yourself — that is the irreversible step.\n'
}

# One step for every registry: after release.yml is green the individual polls add
# no information, and "not propagated yet" is a BLOCKED to re-run, not a defect.
published_crates() {
    (cd "$REPO" && cargo metadata --no-deps --format-version 1 |
        jq -r '.packages[] | select(.publish != []) | .name')
}

auto_published() {
    local missing="" c path line json n
    for c in $(published_crates); do
        path="${c:0:2}/${c:2:2}/$c"
        line="$(curl -sf "https://index.crates.io/$path" | tail -1)"
        if [ "$(jq -r .vers <<<"$line" 2>/dev/null)" = "$VERSION" ] &&
            [ "$(jq -r .yanked <<<"$line" 2>/dev/null)" = false ]; then
            echo "  crates.io $c $VERSION"
        else
            missing="$missing crates.io:$c"
        fi
    done
    if [ "$(npm view pixtuoid version 2>/dev/null)" = "$VERSION" ]; then
        echo "  npm pixtuoid $VERSION"
    else
        missing="$missing npm"
    fi
    if [ "$(curl -sf https://formulae.brew.sh/api/formula/pixtuoid.json | jq -r .versions.stable)" = "$VERSION" ]; then
        echo "  homebrew-core $VERSION"
    else
        missing="$missing homebrew"
    fi
    json="$(gh release view "v$VERSION" --json assets,isDraft,isPrerelease 2>/dev/null)"
    if [ -z "$json" ]; then
        missing="$missing gh-release"
    else
        n="$(jq '.assets | length' <<<"$json")"
        if [ "$(jq -r .isDraft <<<"$json")" != false ] || [ "$(jq -r .isPrerelease <<<"$json")" != false ]; then
            echo "v$VERSION is a draft or prerelease" >&2
            return 1
        fi
        if [ "$n" -lt 1 ]; then
            echo "the GitHub release has no assets" >&2
            return 1
        fi
        echo "  gh release v$VERSION ($n assets)"
    fi
    if [ -n "$missing" ]; then
        blocked "not propagated yet:$missing"
        return 2
    fi
}

manual_notes_scope() {
    local last
    last="$(git -C "$REPO" describe --tags --abbrev=0)"
    echo "  $(git -C "$REPO" log --oneline --no-merges "$last..HEAD" | wc -l | tr -d ' ') commits since $last"
    echo "  current bullets:"
    sed -n "/\"$VERSION\" => Some/,/\]),/p" "$REPO/crates/pixtuoid/src/version.rs" | sed 's/^/    /'
    echo "  Do these cover the whole cycle? A mid-cycle bump leaves them stale by construction."
}

manual_fresh_setup() {
    local sb
    sb="$(mktemp -d)"
    echo "  sandbox: $sb"
    echo "  run in another terminal:"
    echo "    HOME=$sb XDG_CONFIG_HOME=$sb/.config XDG_STATE_HOME=$sb/.state \\"
    echo "      PIXTUOID_SOCKET=$sb/hook.sock $REPO/target/release/pixtuoid setup"
    echo "  expected roster:"
    "$REPO/target/release/pixtuoid" sources --json | jq -r '.[].id' | paste -sd' ' - | sed 's/^/    /'
    echo "  confirm onboarding paints, lists exactly that roster, and wrote nothing outside $sb"
}

manual_fresh_connect() {
    echo "  in the same sandbox HOME: connect a source (Sources panel, s), then disconnect."
    echo "  confirm the hooks land in the SANDBOX's .claude/settings.json,"
    echo "  and that the file returns to its original content after disconnect."
}

manual_live_dogfood() {
    echo "  run: $REPO/target/release/pixtuoid run --projects-root ~/.claude/projects"
    echo "  start a real Claude Code session; confirm the sprite registers, walks, and its token meter grows."
}

# Hook-only is the COMPLEMENT of corpus_check's transcript-bearing roster — the
# sources --json contract carries no such field (additionalProperties: false).
manual_hook_sources() {
    local cc="$REPO/target/release/examples/corpus_check" transcript
    transcript="$("$cc" --sources 2>/dev/null | paste -sd'|' -)"
    echo "  hook-only sources whose CLI is present on this host:"
    "$REPO/target/release/pixtuoid" sources --json |
        jq -r --arg t "$transcript" '.[] | select(.cli_present) | select(.id | test("^(" + $t + ")$") | not) | "    \(.id)  (\(.display_name))"'
    echo "  connect each, run it once, confirm a sprite appears, then disconnect."
    echo "  a source whose CLI is absent is out of reach here — say so in the blocked reason."
}

manual_tui_small() {
    echo "  resize a terminal to exactly 80x24, then run: $REPO/target/release/pixtuoid run"
    echo "  confirm the office paints (not blank) and help / Sources / dashboard all open."
}

manual_floating() {
    echo "  run: $REPO/target/release/pixtuoid floating"
    echo "  confirm the window opens and renders, and CPU stays near idle with no agents."
}

manual_fresh_install() {
    local sb
    sb="$(mktemp -d)"
    echo "  sandbox HOME: $sb"
    echo "  install the PUBLISHED artifact one of three ways:"
    echo "    npm i -g pixtuoid@$VERSION   |   cargo install pixtuoid@$VERSION   |   brew install pixtuoid"
    echo "  then, against that clean HOME:"
    echo "    HOME=$sb XDG_CONFIG_HOME=$sb/.config pixtuoid --version   # expect $VERSION"
    echo "    HOME=$sb XDG_CONFIG_HOME=$sb/.config pixtuoid doctor"
    echo "    HOME=$sb XDG_CONFIG_HOME=$sb/.config pixtuoid run"
    echo "  confirm the version matches, doctor is clean, and the office starts."
}

manual_homebrew_pr() {
    echo "  check https://github.com/Homebrew/homebrew-core/pulls?q=pixtuoid"
    echo "  BrewTestBot autobumps on its own; preempt it if a new depends_on must ship"
    echo "  with the bump — their formula builds DEFAULT features on macOS AND Linux."
}

manual_upgrade_popup() {
    echo "  from an install of the PREVIOUS version, upgrade to $VERSION and launch."
    echo "  confirm the 'What's new in v$VERSION' popup renders whole and Enter closes it."
    echo "  nothing else in this pipeline verifies the curated notes reach a user."
}

# shellcheck disable=SC2034  # read by the sourcing driver, not here
STEPS=(
    "preconditions|pre|auto|clean tree, not behind main, version + notes + tag shape"
    "notes_scope|pre|manual|release notes cover the whole cycle"
    "verify|pre|auto|just verify (preflight + site-check + gen-check)"
    "npm_check|pre|auto|npm generator + OpenClaw plugin contract"
    "wasm_fresh|pre|auto|committed wasm is not stale vs scene/web"
    "ci_green|pre|auto|PR checks green and the bot's Findings: 0 bound to HEAD"
    "build|pre|auto|release build of this tree"
    "openclaw_hermetic|pre|auto|hermetic OpenClaw daemon tier"
    "replay|pre|auto|captured rollout through the full headless path"
    "corpus|pre|auto|every transcript-bearing source decodes its real corpus"
    "openclaw_multi|pre|auto|N real OpenClaw gateways"
    "openclaw_backend|pre|auto|real gateway + one BILLED model turn"
    "site_e2e|pre|auto|Playwright against the production site build"
    "release_smoke|pre|auto|release-smoke.yml on this ref, all platforms"
    "fresh_setup|pre|manual|fresh-HOME onboarding paints and lists every source"
    "fresh_connect|pre|manual|connect installs hooks, disconnect leaves it clean"
    "live_dogfood|pre|manual|real CC session registers, walks, meters"
    "hook_sources|pre|manual|every hook-only CLI installed here registers a sprite"
    "tui_small|pre|manual|80x24 paints and every modal is reachable"
    "floating|pre|manual|floating window renders with sane CPU"
    "tag_ready|pre|auto|print the tag command for the verified commit"
    "published|post|auto|every registry and the GitHub release carry this version"
    "fresh_install|post|manual|the PUBLISHED artifact installs and runs in a clean HOME"
    "homebrew_pr|post|manual|the version-bump PR is open with any new depends_on"
    "upgrade_popup|post|manual|upgrading shows the What's new popup"
)
