# Source decode fixtures

Golden fixtures for the per-CLI decode + hook↔JSONL **coalescing** contract,
driven by `tests/sources/conformance.rs`.

**Record these; do not compose them.** `just capture-fixture <source> <scenario>
<cmd...>` runs the CLI invocation you give it and records what its hooks send.

```text
just capture-fixture kimi tool-run kimi -p '{prompt}'
just capture-fixture openclaw lifecycle scripts/lib/capture-openclaw-lifecycle.sh '{prompt}'
```

A hand-written fixture pins whatever its author believed the wire looked like, and that belief is what the fixture then teaches every later reader.
The composed `cursor/tool-run` this replaced carried no `tool_use_id` and
strictly sequential tools; the capture that replaced it shows an id on every
`preToolUse` and tools that INTERLEAVE (#901 discovered the same gap the
expensive way). kimi's composed fixture went further and asserted the decoder's
own wrong field name back at it.

**Every scenario declares its own provenance**, in `provenance.json` beside the
payloads, because nothing IN the bytes separates a capture from a composition — a
redacted cwd and an invented one look alike. `origin` is one of:

| origin | required | means |
| --- | --- | --- |
| `recorded` | `cli`, `version`, `captured`, `command` | real wire bytes; the recorder writes this file itself |
| `composed` | `note` | hand-written, and the note says how you can tell |
| `unknown` | `note` | predates the rule; nobody recorded where it came from |

**A redacted capture says so in its `note`.** The convention is `/Users/dev` for
the home path; the bytes are otherwise verbatim. Without the note a reader cannot
tell an edited capture from an untouched one, which is the distinction the whole
mechanism exists to make.

The table above is not the schema — [`provenance.schema.json`](provenance.schema.json)
is, and the three readers (this table, `every_scenario_declares_its_provenance`,
`fixture-age.py --check-metadata`) all read it. They each carried their own copy
until #929, and the Python one — the only gate over the single-owner trees — was
already missing `command`.

`every_scenario_declares_its_provenance` enforces the schema, and its
`NO_WIRE_EVIDENCE_YET` list — the hook-only sources with no recorded scenario at
all — only shrinks: recording one forces its entry out, and a new hook-only CLI
cannot join by default.

**The recorder is `pixtuoid-core/examples/capture_fixture.rs`** — Rust rather
than the shell script it replaces, for the reasons under "Why the recorder is a
Rust example" below.

**It records at the shim's OUTPUT**, via a `PIXTUOID_SOCKET` listener,
which is the seam that does not care how the payload reached the shim: codewhale
passes identity in `DEEPSEEK_*` env vars and the shim never reads stdin for it, so
recording the INPUT would have captured a file of empty payloads. Two consequences
for the bytes: a capture carries production's own `_shim_ts_ms` / `_pid` stamps
(fixtures older than this seam do not), and the CLI's installed config is never
touched — the hook it already has is the one that runs, so a DISCONNECTED source
is refused before the turn is spent rather than after.

**A gate needs its own sandbox and its own prompt.** `CAPTURE_SEED=<dir>` is
copied into the sandbox workspace before the run — the place a per-CLI ask rule
belongs, since opencode auto-approves a trusted workspace and CC only FIRES
`PermissionRequest` when a project rule makes the tool ask. `CAPTURE_PROMPT` replaces the shared prompt for
a scenario it cannot reach. Neither touches the user's own config.

A composed fixture kept for a reason says so in its note: `opencode/session-run`
is retained because an auto-approving run emits no permission event for `Waiting`
to ride.

**Re-recording an existing scenario needs a hand.** The recorder never
overwrites committed bytes — it writes `<name>.new` beside them, so a re-record
can be diffed rather than trusted. Nothing READS a `.new`: `conformance.rs` walks
`*.jsonl` and opens `provenance.json` by exact name, and `fixture-age.py`
rglobs the same. So a re-record that you do not promote costs a billed turn and
changes nothing while the suite stays green. Diff the pair, then `mv` the `.new`
over the old one or delete it.

**A capture can only be recorded on Unix.** The recorder listens on a
`UnixListener`; the Windows shim speaks a named pipe, so there is no Windows arm
and `just capture-fixture` exits 2 there. Windows wire evidence therefore cannot
be re-recorded on demand — which is why `copilot/tool-run`, the one fixture
carrying a real Windows `cwd`, stays `unknown` rather than being replaced.

Only ONE edit to a capture is allowed: redact PII. Anything else and it stops
being evidence. PII is not always a field you can drop — cursor's arrives as a `user_email`
key, kimi's as the owner column inside a captured `ls -la` `tool_output`, and
codex's as a `world_state` inventory of every skill installed on the host — so
read a capture before committing it rather than trusting a key-name filter.
The recorder's `warn_on_pii` sees only `$HOME`/`$USER`/`$LOGNAME`, and no size
heuristic can stand in for the read: legitimate copilot lines run to 39 KB, wider
than the 27 KB `world_state` dump that had to go. A line the decoder IGNORES is
the cheap case — drop it and re-run the golden; byte-identical means the
redaction cost no evidence.

**Capture only what pixtuoid's OWN hook registration receives.** A machine can
carry other tools' hooks on the same events — a debug tee, another integration —
and a capture that scoops those up is a recording of somebody else's wire. Scope
it to the source's install-side list (`CURSOR_EVENTS` in `install/cursor.rs`, and
its siblings); an event outside that list never reaches the shim in production, so
a decoder that bails on it is correct and a fixture containing it is fiction.
Learned by getting it wrong: a capture off a 12-event debug tee looked like proof
that the decoder mishandled three events pixtuoid never even subscribes to.

The same trap has a second door: a CLI that SPAWNS another CLI. The OpenClaw
gateway runs its agent turn on a Claude Code backend, which inherits
`PIXTUOID_SOCKET` and sends its own unstamped CC hooks to the recorder — six of
the eight payloads in that capture were Claude Code's. Filter a capture to the
source's own stamp before committing it.

Three lists describe each CLI and they answer different questions — read the
install one before concluding anything about the decode one:

| list | where | question |
| --- | --- | --- |
| `*_EVENTS` | `install/<cli>.rs` | which hooks we register — what the CLI sends us |
| `KNOWN_*` | `source/<cli>.rs` | which shapes we recognise — what counts as drift |
| `*_KNOWN_OMITTED` | `scripts/check_upstream_drift.py` | upstream has it, we deliberately skip it |

An event the decoder handles but `*_EVENTS` never registers is a SHIPPING bug no
test can see, because every test asserts our own belief about the wire: CC's
permission gate is `PermissionRequest`, the decoder had read it for years, and
`install/claude.rs` registered four tool events without it — so a session parked
on a permission prompt rendered as working, indefinitely. Only a capture of a
gated run says which of the two lists is wrong.

Each fixture is a directory:

```
tests/sources/fixtures/<source>/<scenario>/
    <transcript>.jsonl     # JSONL transcript lines, fed to the source's LineDecoder
                           # (JSONL-bearing sources only — a hook-only row,
                           # transcript: None, ships NO transcript)
    hook-payloads.jsonl    # one hook payload per line, fed to decode_hook_payload
    # expected snapshot lives in tests/sources/snapshots/ (insta), generated on first run
```

A scenario ships the transports its source actually has: both files
(CC/Codex), transcript-only (antigravity — no hooks), or hook-payloads-only
(reasonix — hook-only, no watchable JSONL).

**A transcript whose id comes from its PARENT DIR must keep that dir.** grok and
copilot are the two (`path.parent()` in their `*_id_from_path`); everything else
keys on the filename. Flattened, the session id becomes the SCENARIO NAME, and
the hook-vs-JSONL coalesce assertion then compares a real id against a directory
name — `grok/permission-flow` passed for months only because its hooks were
composed to declare `sessionId: "permission-flow"`. So a recorded grok scenario
nests: `<scenario>/<session-uuid>/updates.jsonl`. The recorder decides this by
asking the registry's own `path_filter_for` whether the basename repeats across
sessions, and the harness recurses to find it.

(This tree is **conformance-scanned ONLY** — `conformance.rs` asserts every dir
here is a registered source. Single-owner fixtures read by one module — decode's
`sources/decode/fixtures/`, codex's `sources/codex/fixtures/`, render's
`render/fixtures/` — live with their module, NOT here. See
[`tests/CLAUDE.md`](../../CLAUDE.md) for the governing principle.)

The harness, for each fixture dir:
1. decodes the transcript lines (via the source's `LineDecoder`) and the hook
   payloads (via `decode_hook_payload`),
2. snapshots the full decoded `AgentEvent` sequence (`insta`),
3. **asserts every decoded event shares ONE `AgentId`** — the coalescing
   contract. This is the bug class that keeps biting (hook and JSONL keying a
   session differently → two sprites).

`{{TRANSCRIPT_PATH}}` in a hook payload's `transcript_path` is replaced at
runtime with the fixture's transcript file path (for a hook-only scenario: the
scenario dir's relative path), so a CC hook (which coalesces on
`transcript_path`) lines up with its JSONL file. Codex carries it too — to
prove Codex *ignores* it and still coalesces on `session_id`.

**Adding a CLI:** drop a new `fixtures/<source>/<scenario>/` dir — the decoder
comes from the source's `SourceDescriptor` row in `source/registry.rs` (a
hook-only row, `transcript: None`, ships only `hook-payloads.jsonl` instead
of a transcript). Run `cargo insta review` to accept the generated snapshot.
No harness edit, no other test code.

## Where a scenario's own story lives

Each scenario declares itself in its `provenance.json` `note` — that is the
single place, and it sits beside the bytes it describes. This section used to
carry a per-scenario catalogue plus one blanket claim that everything here was
"derived from real sessions, then sanitized"; both are retired. The blanket
claim was the thing the provenance rule replaced (it could not distinguish a
capture from a composition, which is the whole problem), and the catalogue
described five scenarios that no longer exist — a list of directories rots the
moment one is re-recorded under a better name.

Two scenarios carry a fact BEYOND their own decode, so they are named here where
someone changing the tree will see it:

- **`claude-code/proof-session/`** is read at RENDER time by the `snapshot
  --proof` site-media renderer (`scripts/media.json` job `proof`) — its
  timestamps ARE the clip's beat timings, so retiming this fixture re-times the
  committed `proof.webm`. Its visible strings are pinned disjoint from the
  statusline ticker's own FALLBACK corpus by `tests/proof_fixture_disjointness.rs`.
- **`copilot/tool-run/`** is the only fixture carrying a real Windows `cwd` (a
  path with a space and parens), which makes it the Windows arm of
  `registry_cwd_extractor_matches_each_sources_real_head_shape`. A macOS capture
  cannot reproduce it, which is why it stays `unknown` rather than being
  re-recorded.

## Why the recorder is a Rust example

`just capture-fixture` drives `pixtuoid-core/examples/capture_fixture.rs`. It
was a shell script first, and every defect that version accumulated was bash
semantics rather than recording logic: an empty array under `set -u`
(which stopped the CLI launching for anyone outside an agent session),
`pipefail` swallowing a lister's exit 3 after the turn was billed, `head -1`
taking SIGPIPE once a corpus passed ~700 files (same failure, same place, found
later), `stat -f %B` being macOS-only, and three world-writable temp paths.

None of them could fail in a selftest small enough to run in CI — which is the
argument, not the count. In Rust they are absent by construction: no arrays, no
pipelines, `Metadata::created()` for birth time, `TempDir` for a private 0700
sandbox — and the decisions are ordinary `#[test]`s that `just test` already
runs, rather than a bespoke `--selftest` wired into two gates.

That last clause is only true because `Cargo.toml` declares `[[example]]
test = true`. Without it an example is not a test target: the tests compile and
never run, which is how they shipped at first.
