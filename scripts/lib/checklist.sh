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

auto_preconditions() {
    local cur
    if ! git -C "$REPO" diff --quiet || ! git -C "$REPO" diff --cached --quiet; then
        echo "uncommitted changes — a release must not sweep up edits" >&2
        return 1
    fi
    if [ "$(git -C "$REPO" rev-list --left-right --count HEAD...origin/main | tr -s '\t' ' ')" != "0 0" ]; then
        echo "HEAD and origin/main have diverged — a stale checkout silently poisons a release" >&2
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
    if [ "$wasm" -lt "$engine" ]; then
        echo "site/public/wasm predates the last scene/web change — run just gen-wasm" >&2
        return 1
    fi
}

auto_ci_green() {
    local pr
    pr="$(gh pr list --head "$(git -C "$REPO" branch --show-current)" --json number --jq '.[0].number')"
    if [ -z "$pr" ]; then
        echo "no open PR for this branch" >&2
        return 1
    fi
    gh pr checks "$pr" || return 1
    gh pr view "$pr" --json comments --jq '.comments[-1].body' | grep -q 'Findings: 0' || {
        echo "the review bot has not posted Findings: 0 at HEAD" >&2
        return 1
    }
}

# Both the roster and each root come from the registry — never a bash-side copy.
# Exit 3 means the CLI has never run on this host: a coverage gap to report, not
# a defect. Anything else is a real decode error or panic.
auto_corpus() {
    local cc="$REPO/target/release/examples/corpus_check" s rc=0 status absent=""
    for s in $("$cc" --sources); do
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

auto_release_smoke() {
    local branch id
    branch="$(git -C "$REPO" branch --show-current)"
    gh workflow run release-smoke.yml --ref "$branch" || return 1
    sleep 8
    id="$(gh run list --workflow=release-smoke.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
    gh run watch "$id" --exit-status
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

manual_hook_sources() {
    echo "  hook-only sources whose CLI is present on this host:"
    "$REPO/target/release/pixtuoid" sources --json |
        jq -r '.[] | select(.cli_present) | "    \(.id)  (\(.display_name))"'
    echo "  connect each, run it once, confirm a sprite appears, then disconnect."
    echo "  a source whose CLI is absent is out of reach here — say so in the skip reason."
}

manual_tui_small() {
    echo "  resize a terminal to exactly 80x24, then run: $REPO/target/release/pixtuoid run"
    echo "  confirm the office paints (not blank) and help / Sources / dashboard all open."
}

manual_floating() {
    echo "  run: $REPO/target/release/pixtuoid floating"
    echo "  confirm the window opens and renders, and CPU stays near idle with no agents."
}

auto_release_run() {
    local id
    id="$(gh run list --workflow=release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
    if [ -z "$id" ]; then
        echo "no release.yml run found" >&2
        return 1
    fi
    gh run watch "$id" --exit-status
}

auto_gh_release() {
    local json
    json="$(gh release view "v$VERSION" --json assets,isDraft,isPrerelease)" || return 1
    if [ "$(jq -r .isDraft <<<"$json")" != false ]; then
        echo "v$VERSION is still a draft" >&2
        return 1
    fi
    if [ "$(jq -r .isPrerelease <<<"$json")" != false ]; then
        echo "v$VERSION is marked prerelease" >&2
        return 1
    fi
    echo "  assets: $(jq '.assets | length' <<<"$json")"
}

# The sparse index, not the crates.io JSON — that one's max_version grep can come
# back empty. Every published name here is >=4 chars, so the prefix is name[0:2]/name[2:4].
auto_crates_io() {
    local c path line rc=0
    for c in pixtuoid pixtuoid-core pixtuoid-scene pixtuoid-hook; do
        path="${c:0:2}/${c:2:2}/$c"
        line="$(curl -sf "https://index.crates.io/$path" | tail -1)"
        if [ "$(jq -r .vers <<<"$line")" != "$VERSION" ]; then
            echo "  $c: index tail is $(jq -r .vers <<<"$line"), expected $VERSION" >&2
            rc=1
            continue
        fi
        if [ "$(jq -r .yanked <<<"$line")" != false ]; then
            echo "  $c: $VERSION is YANKED" >&2
            rc=1
            continue
        fi
        echo "  $c $VERSION"
    done
    return "$rc"
}

auto_npm_pub() {
    local got
    got="$(npm view pixtuoid version 2>/dev/null)"
    if [ "$got" != "$VERSION" ]; then
        echo "npm shows $got, expected $VERSION" >&2
        return 1
    fi
}

# autobump lags a tag by design, so "not yet" is a skip-and-resume, not a defect.
auto_homebrew() {
    local json got
    json="$(curl -sf https://formulae.brew.sh/api/formula/pixtuoid.json)" || return 1
    got="$(jq -r .versions.stable <<<"$json")"
    echo "  autobump: $(jq -r .autobump <<<"$json")"
    if [ "$got" != "$VERSION" ]; then
        echo "homebrew-core is $got — autobump lags a tag; skip with a reason and resume later" >&2
        return 1
    fi
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
    echo "  with the bump — their formula builds DEFAULT features on macOS AND Linux,"
    echo "  the one configuration release.yml never builds."
}

manual_upgrade_popup() {
    echo "  from an install of the PREVIOUS version, upgrade to $VERSION and launch."
    echo "  confirm the 'What's new in v$VERSION' popup renders whole and Enter closes it."
    echo "  nothing else in this pipeline verifies the curated notes reach a user."
}

# shellcheck disable=SC2034  # read by the sourcing driver, not here
STEPS=(
    "preconditions|auto|clean tree, synced main, version + notes + tag shape"
    "notes_scope|manual|release notes cover the whole cycle"
    "verify|auto|just verify (preflight + site-check + gen-check)"
    "npm_check|auto|npm generator + OpenClaw plugin contract"
    "wasm_fresh|auto|committed wasm is not stale vs scene/web"
    "ci_green|auto|release PR checks green and review bot Findings: 0"
    "build|auto|release build of this tree"
    "openclaw_hermetic|auto|hermetic OpenClaw daemon tier"
    "replay|auto|captured rollout through the full headless path"
    "corpus|auto|every transcript-bearing source decodes its real corpus"
    "openclaw_multi|auto|N real OpenClaw gateways"
    "openclaw_backend|auto|real gateway + one BILLED model turn"
    "site_e2e|auto|Playwright against the production site build"
    "release_smoke|auto|release-smoke.yml on this ref, all platforms"
    "fresh_setup|manual|fresh-HOME onboarding paints and lists every source"
    "fresh_connect|manual|connect installs hooks, disconnect leaves it clean"
    "live_dogfood|manual|real CC session registers, walks, meters"
    "hook_sources|manual|every hook-only CLI installed here registers a sprite"
    "tui_small|manual|80x24 paints and every modal is reachable"
    "floating|manual|floating window renders with sane CPU"
    "release_run|auto|release.yml green for the tag"
    "gh_release|auto|GitHub release published with its assets"
    "crates_io|auto|all four crates on the sparse index, unyanked"
    "npm_pub|auto|npm shows the new version"
    "homebrew|auto|homebrew-core formula tracks the release"
    "fresh_install|manual|the PUBLISHED artifact installs and runs in a clean HOME"
    "homebrew_pr|manual|the version-bump PR is open with any new depends_on"
    "upgrade_popup|manual|upgrading shows the What's new popup"
)

# shellcheck disable=SC2034  # read by the sourcing driver, not here
PHASE1_LAST=floating
