use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::source::decoder::{CwdExtractor, SUBAGENTS_DIR};
use crate::source::registry::cwd_extractor_for;
use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

use super::health::FailureLatch;
use super::liveness::{probe_admits, revouch_gated_files};
use super::{SessionEndChecker, SourceDecoders, WatchCtx};

/// Oversized-span skip threshold (#204): a pending span past this is never
/// replayed. Module-scoped (not fn-local) so the boundary TEST imports THIS
/// const instead of a drifting second copy of the literal.
pub(super) const MAX_PENDING_BYTES: u64 = 1 << 20;

/// First-sight decision, shared by EVERY path that can be the first to see a
/// file (the initial seed, the 250ms rescan, the 60s poll, a notify event):
/// seed the cursor at EOF — suppressing SessionStart — when the session is
/// historical (mtime outside `window`) OR already ended (a session_end marker in
/// its tail). Only a recent, not-yet-ended file is read from the top. Unifying
/// the gate here (rather than only in the old `initial_seed_walk`) is the #85
/// fix: the post-startup rescan used to bypass it and resurrect a missed
/// ended/stale session as a phantom live sprite.
/// The path form EVERY id derivation runs on: the same `normalize_path_key`
/// fold the per-line decoders receive (`transcript_path_str` below), so the
/// first-sight / session-end / park lanes and the decoder lane mint ONE id
/// per file on Windows. CC/Codex are invariant to the fold (lowercase-hex
/// ids — pinned by `cc_id_from_path_is_stable_across_path_separators` and
/// its codex twin) and antigravity's default deriver folds internally; omp's
/// case-carrying stem chains (`…T…Z_<uuid>/Alpha`) are why the fold must
/// live HERE at the seam, not inside each deriver — a pure deriver keeps the
/// fixture-fed conformance goldens platform-invariant. Identity on Unix.
fn id_path(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(crate::id::normalize_path_key(&path.to_string_lossy()))
}

async fn should_seed_at_eof(
    meta: &std::fs::Metadata,
    window: Duration,
    path: &Path,
    check_ended: SessionEndChecker,
) -> bool {
    let recent = meta
        .modified()
        .ok()
        .map(|mtime| {
            // elapsed() Errs when mtime is in the future (APFS nanosecond clock
            // jitter); a future mtime is necessarily within any recency window.
            mtime.elapsed().unwrap_or(Duration::ZERO) <= window
        })
        .unwrap_or(false);
    // Historical → seed EOF. Recent-but-ended → seed EOF. Recent & live → read.
    !recent || check_session_ended(path, check_ended).await
}

pub(super) async fn scan_root(
    root: &Path,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
    root_health: &mut FailureLatch,
) {
    revouch_gated_files(decoders, ctx).await;
    match tokio::fs::read_dir(root).await {
        Ok(mut read) => {
            if root_health.on_success() {
                tracing::info!("watched root {} is readable again", root.display());
            }
            while let Ok(Some(entry)) = read.next_entry().await {
                walk_jsonl(&entry.path(), decoders, ctx).await;
            }
        }
        Err(e) => {
            if root_health.on_failure() {
                warn!(
                    "cannot read watched root {} ({e}); new sessions will not be \
                     discovered until it is readable again",
                    root.display()
                );
            }
        }
    }
}

pub(super) async fn walk_jsonl(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let WatchCtx {
        source,
        cursors,
        seen,
        tx,
        window,
        // `live` is consumed inside `probe_admits` (off `ctx` directly).
        live: _,
    } = *ctx;
    // `derive_label` / `id_derive` are consumed inside `emit_first_sight` (off
    // `decoders` directly); only the per-line decoder and the end-checker are
    // used directly here.
    let SourceDecoders {
        decode_line,
        check_ended,
        ..
    } = decoders;
    // symlink_metadata (identical to metadata for non-symlinks, so one stat
    // still serves the gate below): symlinked entries are refused wholesale.
    // A directory symlink planted under the watched root would otherwise
    // recurse unboundedly through a loop (each Box::pin level re-walking
    // every transcript until the kernel's ELOOP depth) or walk foreign
    // `.jsonl` trees outside the root into this source's id space; a file
    // symlink pulls in a single foreign transcript the same way. Nothing
    // first-party lays out symlinks under a projects/sessions root (CC/Codex
    // create real dirs/files), and the ROOT itself may still be a symlink —
    // scan_root's read_dir resolves the root path; only entries are checked.
    let meta = match tokio::fs::symlink_metadata(path).await {
        Ok(m) => m,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                // The path is GONE (CC's 30-day cleanup, a user delete) and
                // this walk — typically the notify Remove event — is the last
                // time the watcher hears about it: retire its map entries, or
                // every transcript ever sighted leaks a cursors entry (and a
                // permanent re-vouch stat candidate) for the process
                // lifetime. A recreated same-path file correctly re-enters
                // through the first-sight gate. NotFound ONLY — a transient
                // EACCES must not drop a live session's cursor.
                cursors.lock().await.remove(path);
                seen.lock().await.remove(path);
            }
            return;
        }
    };
    if meta.file_type().is_symlink() {
        // debug!, not warn!: a benign persistent symlink would otherwise
        // repeat the warning on every 250ms walk pass.
        debug!("skipping symlinked entry {}", path.display());
        return;
    }
    if meta.is_dir() {
        if let Ok(mut read) = tokio::fs::read_dir(path).await {
            while let Ok(Some(entry)) = read.next_entry().await {
                Box::pin(walk_jsonl(&entry.path(), decoders, ctx)).await;
            }
        }
        return;
    }
    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
        return;
    }
    // Per-source file filter (default admits all). Antigravity skips the
    // duplicate `transcript_full.jsonl` sibling here so one conversation doesn't
    // double-render — see `PathFilter`.
    if !(decoders.path_filter)(path) {
        return;
    }

    let file_len = meta.len();

    // `known` = already tracked (an earlier pass seeded or read it); `cursor_now`
    // = where to resume (0 if untracked). One lock read for both.
    let (known, cursor_now): (bool, u64) = {
        let cursors_g = cursors.lock().await;
        let entry = cursors_g.get(path).copied();
        (entry.is_some(), entry.unwrap_or(0))
    };
    // First-sight gate (#85): a file we've never tracked is being seen for the
    // first time — by the initial seed, the 250ms rescan, a notify event, or the
    // 60s poll. Run ONE recency + session_end gate regardless of which pass got
    // here first, so a historical or already-ended session is seeded at EOF
    // instead of resurrected with a phantom SessionStart. (A later write makes it
    // `known` with cursor < len, so the documented revive-on-append still fires.)
    // The liveness probe pre-empts the gate: mtime is only a liveness PROXY,
    // and a long-idle / delegating / stuck-in-a-long-tool-call session writes
    // nothing for hours — when the probe has ground truth that the owning
    // process is alive, the file is read from the top (a > MAX_PENDING_BYTES
    // body falls into the oversized first-sight registration below). The
    // bypass deliberately skips the gate's ended tail-scan too, which is safe
    // only because NEITHER probe user persists a structural end marker today:
    // CC (sessions-registry probe) writes none — `cc_session_ended` matches
    // only legacy/structural shapes — and Codex (FD probe) ships the
    // constant-false `codex_session_ended`. There is nothing to scan for; if
    // the upstream drift watch fires (either CLI starts writing one),
    // admission needs an ended-check before bypassing.
    if !known
        && !probe_admits(path, decoders, ctx).await
        && should_seed_at_eof(&meta, window, path, check_ended).await
    {
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        return;
    }
    // Reset-to-0 is the LIVE-session resync (replay the rewritten file from
    // the top on the next pass). The exit-path drains must NOT take this arm
    // — they pre-park a truncated file at its new EOF instead, or the
    // un-claim right behind the drain would turn this reset into a ghost
    // replay (see park_if_truncated_below_cursor).
    if cursor_now > file_len {
        warn!(
            "{} truncated below cursor ({} < {}), resetting cursor",
            path.display(),
            file_len,
            cursor_now
        );
        cursors.lock().await.insert(path.to_path_buf(), 0);
        return;
    }
    if cursor_now == file_len {
        return;
    }
    if file_len - cursor_now > MAX_PENDING_BYTES {
        warn!(
            "{} has > {} pending bytes; skipping backlog to end",
            path.display(),
            MAX_PENDING_BYTES
        );
        // A skipped span may bury a structural session-end marker (the
        // source's check_ended — CC's matches `subtype:"session_end"` /
        // `SessionEnd`; content never counts). Without a tail-scan here the
        // terminator is lost and the slot reaps only via the slow stale-sweep.
        // Checked UNCONDITIONALLY (one bounded 8 KB tail read on a branch
        // already doing head I/O): a KNOWN file's span can end mid-skip, and a
        // !known file lands here too — the liveness probe bypasses the
        // first-sight gate (should_seed_at_eof) INCLUDING its ended tail-scan,
        // so a probe-admitted ENDED transcript must be caught here or the
        // #204 registration below would mint a ghost for a session that is
        // over. (Codex/Antigravity check_ended no-op.) Scan reads the file
        // tail and is independent of the cursor, so compute it before seeding.
        let ended_in_skip = check_session_ended(path, check_ended).await;
        // Seed the cursor to EOF FIRST — before the awaited head-read +
        // registration below — so a concurrent walk_jsonl on this path (250ms
        // rescan / notify) sees `known` on its next read and won't re-enter this
        // branch. Mirrors the normal tail-read path, which also advances the
        // cursor before emitting. (`emit_first_sight` is idempotent via `seen`, so
        // the window only ever cost a redundant head read, never a duplicate
        // SessionStart — but matching the ordering closes it.)
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        if ended_in_skip {
            // A span that itself ENDED stays unregistered and unscanned — a
            // SessionStart or a seeded Task after the SessionEnd just sent
            // would resurrect/animate a ghost. (The if/else is structural on
            // purpose: the ended arm un-claims `seen` below, so a trailing
            // "not ended" conjunct on the scan would be redundant — an
            // untestable equivalent under mutation.)
            let id = AgentId::from_parts(source, &(decoders.id_derive)(&id_path(path)));
            let _ = tx
                .send((
                    Transport::Jsonl,
                    AgentEvent::SessionEnd {
                        agent_id: id,
                        as_child: false,
                    },
                ))
                .await;
            // Un-claim first-sight AFTER forwarding the terminator: the
            // session is over, so a LATER append must re-register through
            // emit_first_sight (the documented revive). Leaving the claim in
            // place pinned the path "registered" forever — a resumed session
            // could never re-appear without a watcher restart.
            seen.lock().await.remove(path);
            return;
        }
        // #204: on the first oversized sight of a recent, live session, still
        // REGISTER the agent. Otherwise a >1 MB transcript stays invisible
        // until its next small append (a long session, or a delegating parent
        // whose subagents then render as flat roots). The giant backlog is NOT
        // replayed; cwd/label come from a BOUNDED head read (CC writes `cwd`
        // on the first line), never the whole 7.4 MB file. Registration keys
        // on `seen` (= "registered"), NOT `!known`: a first-sight-GATED file
        // (cursor seeded at EOF, no SessionStart) is `known`, yet its first
        // >1 MiB append lands here — keying on `!known` left that agent
        // invisible until a later ≤1 MiB append. The `seen` check also spares
        // already-registered files a redundant head read on every oversized
        // append. "Registered" = the claim is HELD (`true`); a
        // child-end-RELEASED claim (`false`, #246) re-registers here like an
        // absent one.
        let registered = seen.lock().await.get(path) == Some(&true);
        if !registered {
            let head_cwd = read_head_cwd(path, MAX_PENDING_BYTES, cwd_extractor_for(source)).await;
            emit_first_sight(path, source, decoders, seen, tx, head_cwd).await;
        }
        // #222: the skipped span may bury an IN-FLIGHT Agent/Task dispatch —
        // tail-scan the last TASK_SCAN_BYTES for unmatched Task starts and
        // re-emit exactly those, restoring subagent-leak suppression + b1
        // (see scan_pending_tasks for the full WHY). Only when the file is
        // registered after the decision above (no slot → JSONL events for an
        // unknown id are reducer no-ops; skip the wasted 256 KiB decode).
        // Runs AFTER emit_first_sight so the registration precedes the
        // synthesized starts on the channel.
        if seen.lock().await.get(path) == Some(&true) {
            scan_pending_tasks(path, decoders, ctx).await;
        }
        return;
    }

    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            warn!("open {} failed: {e}", path.display());
            return;
        }
    };
    if let Err(e) = file.seek(SeekFrom::Start(cursor_now)).await {
        warn!("seek {} failed: {e}", path.display());
        return;
    }
    let mut new_chunk = Vec::with_capacity((file_len - cursor_now) as usize);
    if let Err(e) = file.read_to_end(&mut new_chunk).await {
        warn!("read tail of {} failed: {e}", path.display());
        return;
    }

    let safe_end_relative = match new_chunk.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    };
    if safe_end_relative == 0 {
        return;
    }
    let new_cursor = cursor_now + safe_end_relative as u64;
    {
        let mut cursors_g = cursors.lock().await;
        cursors_g.insert(path.to_path_buf(), new_cursor);
    }

    let new_bytes = &new_chunk[..safe_end_relative];
    // Passed to per-line decoders as the `transcript_path` argument. CC's
    // `decode_cc_line` re-derives the session UUID via `cc_id_from_path` on
    // this string; Codex's decoder extracts the rollout UUID similarly.
    // Antigravity keys on the normalized path directly. Must be normalized
    // (same form as `id_derive` above) so that on Windows the hook key and
    // per-line key agree — an un-normalized path here would land every JSONL
    // event on a phantom id (caught by the PR #160 security review).
    let transcript_path_str = crate::id::normalize_path_key(&path.to_string_lossy());

    // The first-sight cwd normally comes from the read span, but a GATED file
    // revived by an append only reads the tail — and Codex rollouts carry cwd
    // ONLY on the head session_meta line, so the revive would register with an
    // empty cwd (downstream: unknown cwd → the short reap). Fall back to a
    // bounded head read, gated on the `seen` check so an already-registered
    // append pays at most that one map read, never the head I/O. A RELEASED
    // claim (`false`, #246) is about to RE-register below — it needs the head
    // cwd exactly like a never-registered path (the motivating Codex child's
    // turn-N+1 tail has no cwd line).
    let extract = cwd_extractor_for(source);
    let mut first_sight_cwd = extract_cwd(new_bytes, extract);
    if first_sight_cwd.is_none() && seen.lock().await.get(path) != Some(&true) {
        first_sight_cwd = read_head_cwd(path, MAX_PENDING_BYTES, extract).await;
    }
    emit_first_sight(path, source, decoders, seen, tx, first_sight_cwd).await;

    // Used below to recognize a decoded SessionEnd for THIS transcript (the
    // decoder keys events the same way `id_derive` does — pinned by the
    // hook↔watcher coalesce tests).
    let path_agent_id = AgentId::from_parts(source, &(decoders.id_derive)(&id_path(path)));
    let mut session_ended = false;
    for line in new_bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let s = match std::str::from_utf8(line) {
            Ok(s) => s,
            Err(_) => {
                warn!("non-utf8 line in {}", path.display());
                continue;
            }
        };
        let v: serde_json::Value = match serde_json::from_str(s) {
            Ok(v) => v,
            Err(e) => {
                debug!("skip non-json line in {}: {e}", path.display());
                continue;
            }
        };
        match decode_line(&transcript_path_str, source, v) {
            Ok(events) => {
                for ev in events {
                    let ends_this_agent = matches!(
                        &ev,
                        AgentEvent::SessionEnd { agent_id, .. } if *agent_id == path_agent_id
                    );
                    if tx.send((Transport::Jsonl, ev)).await.is_err() {
                        return;
                    }
                    session_ended |= ends_this_agent;
                }
            }
            Err(e) => warn!("decode error in {}: {e}", path.display()),
        }
    }
    if session_ended {
        // Un-claim first-sight: a decoded SessionEnd retires this path's claim
        // so a LATER append re-registers through emit_first_sight (the
        // documented revive) — otherwise `seen` stays claimed forever and the
        // agent can never re-register without a watcher restart. Runs AFTER
        // the whole chunk is forwarded (the terminator precedes any re-claim),
        // and in-pass emit_first_sight idempotence is unaffected: this pass's
        // claim already happened above; the NEXT pass re-emits the pair.
        seen.lock().await.remove(path);
    }
}

/// Pre-drain guard for the exit-path drains (`emit_session_exit` and the #246
/// child-end un-claim): a transcript truncated/recreated BELOW its cursor at
/// the moment a drain runs would hit `walk_jsonl`'s truncation arm — cursor
/// reset to 0, return WITHOUT draining — leaving exactly the state the #228
/// drain-before-unclaim discipline forbids: the file's EXISTING bytes
/// "pending" on a path whose claim is about to be retired/released, so the
/// next pass replays the whole file as a first-sight and re-registers the
/// just-ended session as a ghost, with every fast rung already disarmed for
/// it. Park the cursor at the new EOF instead: a dead session's existing
/// bytes are not pending work, and the revive contract is untouched — only
/// genuinely NEW bytes (len > cursor) re-register. The reset-to-0 replay
/// stays the right call on the NORMAL walk path, where a live session's
/// truncate-rewrite resyncs from the top. Accepted residual: a SECOND
/// truncation landing in the await gap between this stat and the drain's own
/// stat re-opens the ghost — two independent truncations bracketing one
/// await is vanishingly rare, and closing it needs a WalkMode flag threaded
/// through the walk (disproportionate to the window).
pub(super) async fn park_if_truncated_below_cursor(path: &Path, ctx: &WatchCtx<'_>) {
    let Ok(meta) = tokio::fs::symlink_metadata(path).await else {
        return;
    };
    let len = meta.len();
    let mut cursors = ctx.cursors.lock().await;
    // Clamp-to-len rather than a `cursor > len` guard: a no-op for an
    // in-bounds cursor either way, and the clamp has no untestable boundary
    // (the guarded form survives a `>`→`>=` mutation as a pure equivalent —
    // it just re-inserts the same value).
    if let Some(c) = cursors.get_mut(path) {
        *c = (*c).min(len);
    }
}

/// Claim first-sight for `path` and, if this is the first pass to see it, emit
/// the synthesized `SessionStart` + `Rename` (the registration pair). Shared by
/// the normal tail-read path and the #204 oversized-first-sight path so the two
/// emit IDENTICAL events from one place. `cwd` is the source-derived working dir
/// (from the tail in the normal path, from a bounded head read in the oversized
/// path); `None`/empty falls back to the project-dir label in `derive_label`.
///
/// Takes the `seen` lock ONLY to claim first-sight, then drops it before the
/// awaited sends — holding it across `tx.send` would block on a slow consumer
/// for no reason (the flag flip is the entire critical section). Mirrors the
/// narrow `cursors` locking in `walk_jsonl`.
async fn emit_first_sight(
    path: &Path,
    source: &Arc<str>,
    decoders: SourceDecoders,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &TaggedSender,
    cwd: Option<PathBuf>,
) {
    // `Some(true)` = the claim is already held this life. `None` (never
    // registered / fully retired by an exit un-claim) and `Some(false)` (a
    // claim RELEASED by the child-end un-claim, #246) both register — a
    // released path's next append IS the revival the release exists for.
    let already_claimed = seen.lock().await.insert(path.to_path_buf(), true) == Some(true);
    if already_claimed {
        return;
    }
    // session_id comes from the SAME deriver as the AgentId — the hook
    // transport's slots carry the bare session UUID (CC/Codex), and
    // `backfill_identity` never heals a non-empty session_id, so a raw
    // file-stem here would leave a JSONL-created slot permanently
    // disagreeing with its hook-created twin (a Codex stem is
    // `rollout-<ts>-<uuid>`: every tooltip disambiguator suffix became the
    // constant `roll`).
    let session_id = (decoders.id_derive)(&id_path(path));
    let id = AgentId::from_parts(source, &session_id);
    // Content-derived cwd wins; the PATH deriver is the fallback for sources
    // whose transcript content carries none (grok's URL-encoded group dir) —
    // an empty cwd here would put the slot on the unknown-cwd short reap.
    let cwd = cwd
        .or_else(|| (decoders.cwd_derive)(path))
        .unwrap_or_default();
    let parent_id = detect_parent_id(path, source);
    let _ = tx
        .send((
            Transport::Jsonl,
            AgentEvent::SessionStart {
                agent_id: id,
                source: source.to_string(),
                session_id,
                cwd: cwd.clone(),
                parent_id,
            },
        ))
        .await;

    let label = (decoders.derive_label)(path, source, &cwd);
    let _ = tx
        .send((
            Transport::Jsonl,
            AgentEvent::Rename {
                agent_id: id,
                label,
            },
        ))
        .await;
}

/// Read at most `limit` bytes from the START of a file and extract `cwd` from
/// the first complete JSONL line (CC writes `cwd` there). Used by the #204
/// oversized-first-sight path so registration never reads the whole multi-MB
/// file — the head is bounded by `MAX_PENDING_BYTES`. Returns `None` when the
/// file can't be opened/read or has no `cwd` in its head (an empty-cwd
/// SessionStart then falls back to the project-dir label).
async fn read_head_cwd(path: &Path, limit: u64, extract: CwdExtractor) -> Option<PathBuf> {
    // Bound the allocation to the SMALLER of `limit` and the real file size
    // (mirrors `read_tail`): a tiny SessionStart line must not zero a full
    // `MAX_PENDING_BYTES` (1 MiB) buffer on every oversized-first-sight read.
    let file_len = tokio::fs::metadata(path).await.ok()?.len();
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut head = vec![0u8; limit.min(file_len) as usize];
    let n = file.read(&mut head).await.ok()?;
    head.truncate(n);
    extract_cwd(&head, extract)
}

/// Read at most `bytes` from the END of a file (clamped to file size).
/// `None` on any I/O error — callers treat that as "nothing to scan" (log +
/// continue, never panic). Shared by `check_session_ended` (8 KiB ended-marker
/// scan) and `scan_pending_tasks` (the #222 Task scan) so the two bounded
/// tail reads can't drift apart.
async fn read_tail(path: &Path, bytes: u64) -> Option<Vec<u8>> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let file_len = meta.len();
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let start = file_len.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).await.ok()?;
    let mut buf = Vec::with_capacity(bytes.min(file_len) as usize);
    file.read_to_end(&mut buf).await.ok()?;
    Some(buf)
}

/// Read the tail of a file and delegate to the source-specific checker.
async fn check_session_ended(path: &Path, checker: SessionEndChecker) -> bool {
    const TAIL_BYTES: u64 = 8192;
    match read_tail(path, TAIL_BYTES).await {
        Some(buf) => checker(&buf),
        None => false,
    }
}

/// How far back from EOF the oversized-skip Task scan looks (#222). Bounds
/// both the I/O and the decode work; survivors are at most the parallel-
/// dispatch ceiling in practice, so no further cap is needed.
pub(super) const TASK_SCAN_BYTES: u64 = 256 * 1024;

/// #222: tail-scan an oversized skipped span for IN-FLIGHT Task dispatches
/// and re-emit exactly their `ActivityStart`s. Mid-attach to a delegating
/// session whose backlog exceeds `MAX_PENDING_BYTES` seeds the cursor at EOF,
/// so the in-flight `Agent` dispatch tool_use line is never decoded — and its
/// PreToolUse hook predates attach — leaving the reducer's `active_tasks`
/// empty: subagent-leak suppression stays OFF (the parent animates the
/// subagent's misattributed hook tools instead of showing Delegating) and the
/// b1 completion cascade never arms (the finished subagent lingers Idle up to
/// the 30-min stale sweep). Re-sending the unmatched Task starts restores
/// both: `track_active_tasks` seeds `active_tasks` from any transport's Task
/// ActivityStart, so the reducer needs no change.
///
/// Tail-window geometry guarantees no false leak: a completion is always
/// LATER in the file than its start, so any windowed start's completion (if
/// one exists) is also in the window — a synthesized start is only ever a
/// genuinely in-flight dispatch OR one whose completion raced in beyond the
/// `file_len` snapshot (the next walk re-decodes that span; `active_tasks` is
/// a HashSet, so the duplicate insert is idempotent). A dispatch buried
/// deeper than `TASK_SCAN_BYTES` of subsequent traffic keeps the pre-#222
/// skip behavior — bounded, documented residual.
///
/// `decode_line` also emits OTHER events from these lines (Rename, plain
/// ActivityStarts, SessionStart…) — everything except the unmatched Task
/// starts is DISCARDED. This is a Task-seeding scan, not a replay: replaying
/// 256 KiB of activity would animate a burst of stale tools.
///
/// Hook-wins dedup (#150): the synthesized events are Jsonl-tagged. On
/// mid-attach no hook record for these tuids exists (the hooks predate the
/// listener), so they pass the dedup. A mid-RUN oversized skip (> 1 MiB
/// appended between scans on an attached session) can race a recent hook
/// record — the dedup eating the synthesized start then is CORRECT: the hook
/// copy already seeded `active_tasks`.
///
/// Codex/Antigravity rollouts produce no Task ActivityStarts from line
/// decode (Codex subagents wire via the SubagentStart/Stop hooks), so the
/// scan is a structural no-op for them.
async fn scan_pending_tasks(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let Some(buf) = read_tail(path, TASK_SCAN_BYTES).await else {
        return;
    };
    // Same per-line keying as the walk loop: the decoder re-derives the agent
    // id from this normalized path string (see `transcript_path_str` there).
    let transcript_path_str = crate::id::normalize_path_key(&path.to_string_lossy());
    let mut lines = buf.split(|b| *b == b'\n');
    // The window ALWAYS starts mid-file (the only caller is the oversized
    // branch, so file_len > MAX_PENDING_BYTES > TASK_SCAN_BYTES by
    // construction), so its first chunk is almost always a partial line —
    // skip through the first newline unconditionally rather than decode a
    // fragment (which could even parse as JSON by accident). The former
    // `file_len > TASK_SCAN_BYTES` guard was provably constant-true here
    // and its false arm untestable dead code.
    let _ = lines.next();
    // Unmatched Task dispatches in file order. A Vec keeps the order; a
    // duplicate ActivityStart for a seen tuid is skipped (HashSet semantics)
    // and a completion removes its start wherever it sits.
    let mut pending: Vec<(String, AgentEvent)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        let events = match (decoders.decode_line)(&transcript_path_str, ctx.source, v) {
            Ok(events) => events,
            Err(e) => {
                debug!("task-scan decode error in {}: {e}", path.display());
                continue;
            }
        };
        for ev in events {
            match &ev {
                AgentEvent::ActivityStart {
                    tool_use_id: Some(tuid),
                    detail: Some(d),
                    ..
                } if d.is_task() => {
                    if !pending.iter().any(|(t, _)| t == tuid) {
                        pending.push((tuid.clone(), ev));
                    }
                }
                AgentEvent::ActivityEnd {
                    tool_use_id: Some(tuid),
                    ..
                } => {
                    pending.retain(|(t, _)| t != tuid);
                }
                _ => {}
            }
        }
    }
    for (tuid, ev) in pending {
        debug!(
            "re-emitting in-flight Task dispatch {tuid} from the oversized tail of {}",
            path.display()
        );
        if ctx.tx.send((Transport::Jsonl, ev)).await.is_err() {
            return;
        }
    }
}

/// Detect a CC subagent by the `subagents` path component and link it to its
/// parent via the parent's session UUID — the directory component immediately
/// before `subagents` (`<parent-uuid>`). That UUID equals the parent's own id
/// (`cc_id_from_path` of the parent transcript), so the link resolves even when
/// the subagent transcript lands under a DIFFERENT project dir than the parent
/// (a git-worktree cwd-split): only the cwd-derived project-dir prefix differs;
/// the `<parent-uuid>` component is identical. CC-layout-specific — Codex links
/// subagents via the SubagentStart hook instead.
pub(super) fn detect_parent_id(path: &Path, source: &str) -> Option<AgentId> {
    let mut prev: Option<&str> = None;
    for c in path.components() {
        if c.as_os_str() == SUBAGENTS_DIR {
            return prev.map(|uuid| AgentId::from_parts(source, uuid));
        }
        prev = c.as_os_str().to_str();
    }
    None
}

/// Scan a byte span line-by-line and return the first cwd the SCANNED
/// source's own extractor finds. This fn owns only the shared line iteration
/// (skip empty / non-UTF-8 / non-JSON lines, never short-circuit on them);
/// the per-source shape knowledge lives in the source's registry row
/// (invariant #3 — the old accreting if-chain here tried every source's shape
/// against every transcript, so a foreign-shaped line, e.g. a codex-style
/// `payload.cwd` inside a CC transcript, could label a session with a
/// foreign, identity-bearing cwd).
pub(super) fn extract_cwd(bytes: &[u8], extract: CwdExtractor) -> Option<PathBuf> {
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        if let Some(cwd) = extract(&v) {
            return Some(cwd);
        }
    }
    None
}
