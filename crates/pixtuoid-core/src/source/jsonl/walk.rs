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
use super::{ActivityRecency, SessionEndChecker, SourceDecoders, TailActivity, WatchCtx};

/// Oversized-span skip threshold: a pending span past this is never replayed.
pub(super) const MAX_PENDING_BYTES: u64 = 1 << 20;

/// The path form EVERY id derivation runs on. The `normalize_path_key` fold
/// lives HERE at the seam, not inside each deriver, so every lane mints ONE id
/// per file on Windows while the fixture-fed conformance goldens stay
/// platform-invariant. Identity on Unix.
pub(super) fn id_path(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(crate::id::normalize_path_key(&path.to_string_lossy()))
}

/// First-sight decision, shared by EVERY path that can be the first to see a
/// file: seed the cursor at EOF — suppressing SessionStart — when the session
/// is historical (mtime outside `window`) OR already ended. Unifying the gate
/// here is the #85 fix: the post-startup rescan used to bypass it and
/// resurrect an ended/stale session as a phantom live sprite.
///
/// `probe_live` (the liveness vouch) exempts only the RECENCY half — a
/// structural end marker is the source's own first-hand report that no vouch
/// outranks (grok's leader vouch is mtime-derived and outlives its session;
/// omp's fd vouch fires for any bun tool merely READING an old transcript).
async fn should_seed_at_eof(
    meta: &std::fs::Metadata,
    window: Duration,
    path: &Path,
    check_ended: SessionEndChecker,
    activity_recency: ActivityRecency,
    probe_live: bool,
) -> bool {
    let mtime_recent = meta.modified().ok().is_some_and(|m| within(m, window));
    if !mtime_recent && !probe_live {
        return true;
    }
    let Some(tail) = read_tail(path, TAIL_BYTES).await else {
        // An unreadable tail is no evidence that the session is over.
        return false;
    };
    if check_ended(&tail) {
        return true;
    }
    if probe_live {
        return false;
    }
    // Recent-but-unvouched: were those recent bytes the SESSION writing — the
    // only thing mtime ever proxied, and the half CC falsifies?
    match activity_recency(&tail) {
        TailActivity::At(secs) => !within(
            std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            window,
        ),
        TailActivity::SidecarOnly => true,
        TailActivity::Unknown => false,
    }
}

/// `elapsed()` Errs when `t` is in the FUTURE (APFS nanosecond clock jitter, a
/// clock-skewed wire stamp), and a future instant is within any window.
fn within(t: std::time::SystemTime, window: Duration) -> bool {
    t.elapsed().unwrap_or(Duration::ZERO) <= window
}

/// How far back from EOF the first-sight predicates read. ONE value for both
/// the end-marker scan and the activity-recency probe — a split would let a
/// transcript be "ended" and "active" over different bytes.
pub(super) const TAIL_BYTES: u64 = 8192;

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
        live: _,
    } = *ctx;
    let SourceDecoders {
        decode_line,
        check_ended,
        ..
    } = decoders;
    // symlink_metadata, not metadata: symlinked entries are refused wholesale.
    // A directory symlink under the watched root would otherwise recurse
    // unboundedly, or walk foreign `.jsonl` trees into this source's id space.
    // The ROOT itself may still be a symlink — only entries are checked.
    let meta = match tokio::fs::symlink_metadata(path).await {
        Ok(m) => m,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                // The path is GONE and this walk is the last time the watcher
                // hears about it: retire its map entries, or every transcript
                // ever sighted leaks a cursors entry for the process lifetime.
                // NotFound ONLY — a transient EACCES must not drop a live
                // session's cursor.
                cursors.lock().await.remove(path);
                seen.lock().await.remove(path);
            }
            return;
        }
    };
    if meta.file_type().is_symlink() {
        // debug!, not warn!: a persistent symlink would re-warn every 250ms.
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
    // Per-source file filter: antigravity skips the duplicate
    // `transcript_full.jsonl` sibling so one conversation doesn't double-render.
    if !(decoders.path_filter)(path) {
        return;
    }

    let file_len = meta.len();

    let (known, cursor_now): (bool, u64) = {
        let cursors_g = cursors.lock().await;
        let entry = cursors_g.get(path).copied();
        (entry.is_some(), entry.unwrap_or(0))
    };
    if !known
        && should_seed_at_eof(
            &meta,
            window,
            path,
            check_ended,
            decoders.activity_recency,
            probe_admits(path, decoders, ctx).await,
        )
        .await
    {
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        return;
    }
    // Reset-to-0 is the LIVE-session resync. The exit-path drains must NOT take
    // this arm — they pre-park a truncated file at its new EOF instead, or the
    // un-claim right behind the drain turns this reset into a ghost replay (see
    // park_if_truncated_below_cursor).
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
        // A skipped span may bury a structural session-end marker; without this
        // tail-scan the terminator is lost and the slot reaps only via the slow
        // stale-sweep. Unconditional because a KNOWN file's span can end
        // mid-skip. Independent of the cursor, so compute it before seeding.
        let ended_in_skip = check_session_ended(path, check_ended).await;
        // Seed the cursor to EOF BEFORE the awaited head-read + registration
        // below, so a concurrent walk_jsonl on this path sees `known` on its
        // next read and won't re-enter this branch.
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        if ended_in_skip {
            // A span that itself ENDED stays unregistered and unscanned — a
            // SessionStart or a seeded Task after the SessionEnd just sent
            // would resurrect/animate a ghost.
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
            // Un-claim first-sight AFTER forwarding the terminator, so a LATER
            // append re-registers through emit_first_sight. Leaving the claim in
            // place pinned the path "registered" forever — a resumed session
            // could never re-appear without a watcher restart.
            seen.lock().await.remove(path);
            return;
        }
        // #204: on the first oversized sight of a recent, live session, still
        // REGISTER the agent — otherwise a >1 MB transcript stays invisible
        // until its next small append. The giant backlog is NOT replayed;
        // cwd/label come from a BOUNDED head read. Keys on `seen` (=
        // registered), NOT `!known`: a first-sight-GATED file is `known` yet its
        // first >1 MiB append lands here, and keying on `!known` left that agent
        // invisible until a later ≤1 MiB append.
        let registered = seen.lock().await.get(path) == Some(&true);
        // The span is too big to hold, so the tail is the instrument — a
        // metadata run always lands at the end.
        let metadata_only = matches!(
            read_tail(path, TAIL_BYTES)
                .await
                .as_deref()
                .map(decoders.activity_recency),
            Some(super::TailActivity::SidecarOnly)
        );
        if !registered && !metadata_only {
            let head_cwd = read_head_cwd(path, MAX_PENDING_BYTES, cwd_extractor_for(source)).await;
            emit_first_sight(path, source, decoders, seen, tx, head_cwd).await;
        }
        // #222: the skipped span may bury an IN-FLIGHT Task dispatch — tail-scan
        // for unmatched Task starts and re-emit exactly those (see
        // scan_pending_tasks). Only when registered: JSONL events for an unknown
        // id are reducer no-ops, so an unregistered path skips the decode.
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
    // Must be normalized (the same form as `id_derive` above) so that on
    // Windows the hook key and the per-line key agree — an un-normalized path
    // here would land every JSONL event on a phantom id.
    let transcript_path_str = crate::id::normalize_path_key(&path.to_string_lossy());

    // A GATED file revived by an append only reads the tail — and Codex
    // rollouts carry cwd ONLY on the head session_meta line, so the revive
    // would register with an empty cwd (→ the unknown-cwd short reap). Fall
    // back to a bounded head read.
    let extract = cwd_extractor_for(source);
    let mut first_sight_cwd = extract_cwd(new_bytes, extract);
    if first_sight_cwd.is_none() && seen.lock().await.get(path) != Some(&true) {
        first_sight_cwd = read_head_cwd(path, MAX_PENDING_BYTES, extract).await;
    }
    if !matches!(
        (decoders.activity_recency)(new_bytes),
        super::TailActivity::SidecarOnly
    ) {
        emit_first_sight(path, source, decoders, seen, tx, first_sight_cwd).await;
    }

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
        // Un-claim first-sight so a LATER append re-registers through
        // emit_first_sight. RELEASED (`false`), not removed: `emit_first_sight`
        // treats `Some(false)` exactly like an absent entry, while
        // `revouch_gated_files` (which keys on `contains_key`) can no longer
        // reset a still-vouched path's cursor to 0 and replay the whole
        // transcript once per scan pass.
        seen.lock().await.insert(path.to_path_buf(), false);
    }
}

/// Pre-drain guard for the exit-path drains: a transcript truncated/recreated
/// BELOW its cursor at the moment a drain runs would hit `walk_jsonl`'s
/// truncation arm — cursor reset to 0, return WITHOUT draining — so the next
/// pass replays the whole file as a first-sight and re-registers the just-ended
/// session as a ghost. Park the cursor at the new EOF instead; only genuinely
/// NEW bytes (len > cursor) re-register. The reset-to-0 replay stays the right
/// call on the NORMAL walk path, where a live session's truncate-rewrite
/// resyncs from the top.
pub(super) async fn park_if_truncated_below_cursor(path: &Path, ctx: &WatchCtx<'_>) {
    let Ok(meta) = tokio::fs::symlink_metadata(path).await else {
        return;
    };
    let len = meta.len();
    let mut cursors = ctx.cursors.lock().await;
    if let Some(c) = cursors.get_mut(path) {
        *c = (*c).min(len);
    }
}

/// Claim first-sight for `path` and, if this is the first pass to see it, emit
/// the synthesized `SessionStart` + `Rename`. Shared by the normal tail-read
/// path and the oversized-first-sight path so the two emit IDENTICAL events
/// from one place. A `None`/empty `cwd` falls back to the project-dir label in
/// `derive_label`.
async fn emit_first_sight(
    path: &Path,
    source: &Arc<str>,
    decoders: SourceDecoders,
    seen: &Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &TaggedSender,
    cwd: Option<PathBuf>,
) {
    // `Some(false)` — a claim RELEASED by the child-end un-claim (#246) —
    // registers like an absent entry: a released path's next append IS the
    // revival the release exists for.
    let already_claimed = seen.lock().await.insert(path.to_path_buf(), true) == Some(true);
    if already_claimed {
        return;
    }
    // session_id comes from the SAME deriver as the AgentId: the hook
    // transport's slots carry the bare session UUID and `backfill_identity`
    // never heals a non-empty session_id, so a raw file-stem here would leave a
    // JSONL-created slot permanently disagreeing with its hook-created twin.
    let session_id = (decoders.id_derive)(&id_path(path));
    let id = AgentId::from_parts(source, &session_id);
    // Content-derived cwd wins; the PATH deriver is the fallback for sources
    // whose content carries none — an empty cwd would put the slot on the
    // unknown-cwd short reap.
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
/// the first complete JSONL line (CC writes `cwd` there), so registration never
/// reads a whole multi-MB transcript.
async fn read_head_cwd(path: &Path, limit: u64, extract: CwdExtractor) -> Option<PathBuf> {
    let file_len = tokio::fs::metadata(path).await.ok()?.len();
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let mut head = vec![0u8; limit.min(file_len) as usize];
    let n = file.read(&mut head).await.ok()?;
    head.truncate(n);
    extract_cwd(&head, extract)
}

/// Read at most `bytes` from the END of a file (clamped to file size). `None`
/// on any I/O error — callers treat that as "nothing to scan".
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

pub(super) async fn check_session_ended(path: &Path, checker: SessionEndChecker) -> bool {
    match read_tail(path, TAIL_BYTES).await {
        Some(buf) => checker(&buf),
        None => false,
    }
}

/// How far back from EOF the oversized-skip Task scan looks.
pub(super) const TASK_SCAN_BYTES: u64 = 256 * 1024;

/// #222: tail-scan an oversized skipped span for IN-FLIGHT Task dispatches and
/// re-emit exactly their `ActivityStart`s. Mid-attach to a delegating session
/// whose backlog exceeds `MAX_PENDING_BYTES` never decodes the in-flight
/// dispatch line, leaving the reducer's `active_tasks` empty: subagent-leak
/// suppression stays OFF and the b1 completion cascade never arms.
///
/// Tail-window geometry guarantees no false leak: a completion is always LATER
/// in the file than its start, so any windowed start's completion is also in
/// the window. A dispatch buried deeper than `TASK_SCAN_BYTES` keeps the
/// pre-#222 skip behavior — bounded, documented residual.
///
/// Everything `decode_line` emits EXCEPT the unmatched Task starts is
/// DISCARDED: this is a Task-seeding scan, not a replay — replaying 256 KiB of
/// activity would animate a burst of stale tools.
async fn scan_pending_tasks(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let Some(buf) = read_tail(path, TASK_SCAN_BYTES).await else {
        return;
    };
    let transcript_path_str = crate::id::normalize_path_key(&path.to_string_lossy());
    let mut lines = buf.split(|b| *b == b'\n');
    // The window ALWAYS starts mid-file (the only caller is the oversized
    // branch), so its first chunk is almost always a partial line — skip it
    // rather than decode a fragment that could parse as JSON by accident.
    let _ = lines.next();
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
/// parent via the directory component immediately before it (`<parent-uuid>`).
/// That UUID equals the parent's own id, so the link resolves even when the
/// subagent transcript lands under a DIFFERENT project dir than the parent (a
/// git-worktree cwd-split). CC-layout-specific.
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

/// Scan a byte span line-by-line and return the first cwd the SCANNED source's
/// own extractor finds. The per-source shape knowledge stays in the registry
/// row (invariant #3): an if-chain trying every source's shape against every
/// transcript lets a foreign-shaped line label a session with a foreign,
/// identity-bearing cwd.
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
