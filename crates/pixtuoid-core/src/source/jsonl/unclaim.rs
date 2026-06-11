use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::debug;

use crate::AgentId;

use super::walk::walk_jsonl;
use super::{SourceDecoders, WatchCtx};

/// How long an un-drained [`ChildEndUnclaims`] entry survives. Bounds the
/// set: an id no watcher ever matches (the child's transcript was first-sight
/// GATED so it holds no `seen` claim, or the id belongs to a source whose
/// watcher isn't running) would otherwise accrue one entry per child end for
/// the process lifetime. A TTL (not a size cap) because staleness — not
/// volume — is the failure mode: 5 minutes is several 60s poll-backstop
/// cycles, so the OWNING watcher always gets multiple drain passes before an
/// entry can lapse, while a cap could evict a fresh entry under a burst of
/// foreign ones.
const CHILD_END_UNCLAIM_TTL: Duration = Duration::from_secs(300);

/// The #246 child-end un-claim side-channel: child ids whose hook
/// `SessionEnd { as_child: true }` (a decoded `SubagentStop`) was observed,
/// and whose `seen`-claimed transcript should therefore be RELEASED so the
/// next append re-registers.
///
/// WHY a side-channel: a multi-turn Codex child (parent `send_input`) gets a
/// `SubagentStop` hook at EVERY turn end but no `SessionStart` carrier of any
/// kind at turn N+1 — upstream provides none (codex-rs hook_runtime.rs:
/// `UserPromptSubmit` fires only for direct user input, child-context events
/// carry the ROOT session_id, `SubagentStart` fires only at thread startup) —
/// and the hook-side end never touched the watcher's `seen` claim, so turn
/// N+1's appends decoded as unknown-id no-ops forever. Releasing the claim
/// when the hook end is decoded turns the next append into the JSONL
/// first-sight `SessionStart` the reducer's child ledger (#244/#246) re-links
/// to the remembered parent.
///
/// PRODUCER: the tee inside `ClaudeCodeSource::run` — ALL sources' hook
/// payloads ride the ONE shared socket it owns, so every `SubagentStop`
/// (CC and Codex alike) passes that single seam. (If sockets ever split
/// per-source, each source spawns its own tee — `tee_child_end_unclaims`
/// is source-generic.) CONSUMERS: each WIRED source's
/// `JsonlWatcher` (via [`JsonlWatcher::with_child_end_unclaims`] — CC and
/// Codex today; Antigravity stays unwired, nothing stamps its ends
/// `as_child`) drains only
/// the ids matching its OWN claimed paths; foreign ids stay pending for the
/// watcher that owns them (`AgentId` is source-namespaced, so there is
/// exactly one owner) until the TTL prunes them.
#[derive(Clone)]
pub struct ChildEndUnclaims {
    /// `(id, pushed-at)`. `Instant` (monotonic) — a wall-clock jump must not
    /// fake or starve the TTL. A `Vec`, not a map: entries are at most the
    /// number of in-flight child ends, and dedupe-on-push keeps it that way.
    entries: Arc<std::sync::Mutex<Vec<(AgentId, std::time::Instant)>>>,
    ttl: Duration,
}

impl ChildEndUnclaims {
    pub fn new() -> Self {
        Self::with_ttl(CHILD_END_UNCLAIM_TTL)
    }

    /// Test-only seam (mirrors `with_poll_interval`): shrinks the prune TTL
    /// so the bounded-growth contract is testable without a 5-minute wait.
    #[doc(hidden)]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(std::sync::Mutex::new(Vec::new())),
            ttl,
        }
    }

    /// Record a child id whose hook end was decoded. A repeated push (a
    /// multi-turn child ends once per turn) refreshes the entry's TTL clock
    /// rather than duplicating it. A poisoned lock (a panicking sibling
    /// holder — none exists; the sections below never panic) degrades to a
    /// dropped push: the stale-sweep ladder still reaps, never panic here.
    pub fn push(&self, id: AgentId) {
        let now = std::time::Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.retain(|(_, at)| now.duration_since(*at) < self.ttl);
        match entries.iter_mut().find(|(eid, _)| *eid == id) {
            Some(entry) => entry.1 = now,
            None => entries.push((id, now)),
        }
    }

    /// Remove and return every pending id matching `matched`, pruning
    /// TTL-expired entries first. Non-matching entries STAY — another
    /// source's watcher may be their owner.
    pub fn take_matching(&self, mut matched: impl FnMut(&AgentId) -> bool) -> Vec<AgentId> {
        let now = std::time::Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return Vec::new();
        };
        entries.retain(|(_, at)| now.duration_since(*at) < self.ttl);
        let mut out = Vec::new();
        entries.retain(|(id, _)| {
            if matched(id) {
                out.push(*id);
                false
            } else {
                true
            }
        });
        out
    }

    /// Cheap fast path for the per-pass drain: one lock, no pruning. A set
    /// holding only TTL-expired entries reads as non-empty here — harmless,
    /// the full drain it admits prunes them; "empty" is never wrong.
    fn is_empty(&self) -> bool {
        self.entries.lock().map(|e| e.is_empty()).unwrap_or(true)
    }
}

impl Default for ChildEndUnclaims {
    fn default() -> Self {
        Self::new()
    }
}

/// Consumer half of the #246 un-claim side-channel (see [`ChildEndUnclaims`]
/// for the WHY). For every pending id that matches one of THIS watcher's
/// `seen` paths: walk the path's pending bytes to EOF FIRST (the #228
/// drain-before-unclaim discipline `emit_session_exit` pinned — a pre-stop
/// straggler that re-entered as a first-sight would resurrect the just-ended
/// child as a ghost), then RELEASE the claim. Two deliberate differences from
/// `emit_session_exit`:
///
/// * **No `SessionEnd` is emitted** — the reducer already ended the slot from
///   the hook `SubagentStop`; a duplicate here would be a no-op at best.
/// * **Release = `seen` → `false`, not removal.** The path must stay KNOWN:
///   the Codex FD probe keeps vouching an in-flight child's still-open
///   rollout, so a removed claim would let `revouch_gated_files` reset the
///   cursor to 0 on the next scan — a full stale-activity replay plus an
///   instant re-registration that negates the SubagentStop end. A released
///   claim is skipped by the re-vouch sweep (it keys on `contains_key`) and
///   revives only on genuinely NEW bytes through `emit_first_sight`.
///
/// Accepted residuals: bytes the child wrote between the hook Stop and this
/// drain are consumed silently (indistinguishable from pre-stop stragglers
/// without per-byte timestamps) — the next append re-registers, and an active
/// turn appends continuously; and a revival start landing inside the slot's
/// 4.5s exit grace is swallowed by the root-gated resurrect pin (documented
/// reducer edge) — the turn N+2 stop re-arms this same path.
pub(super) async fn drain_child_end_unclaims(
    unclaims: Option<&ChildEndUnclaims>,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) {
    let Some(unclaims) = unclaims else {
        return;
    };
    if unclaims.is_empty() {
        return;
    }
    // Snapshot path → (id, held) under a short lock (the id derivation is an
    // allocation per path — never hold the lock across it... it's sync, but
    // the snapshot also keeps the later walk/release loop lock-free).
    let claimed: Vec<(PathBuf, AgentId, bool)> = {
        let seen = ctx.seen.lock().await;
        seen.iter()
            .map(|(p, &held)| {
                (
                    p.clone(),
                    AgentId::from_parts(ctx.source, &(decoders.id_derive)(p)),
                    held,
                )
            })
            .collect()
    };
    // Consume ids matching ANY of this watcher's known paths (held or already
    // released — a duplicate stop's work is done either way); foreign ids
    // stay pending for their owning watcher.
    let matched = unclaims.take_matching(|id| claimed.iter().any(|(_, pid, _)| pid == id));
    for id in matched {
        for (path, pid, held) in &claimed {
            if *pid != id || !*held {
                continue;
            }
            // #228: drain pending bytes to EOF while the claim is still held
            // (a straggler decodes as an already-claimed walk, registering
            // nothing), THEN release. The release only DOWNGRADES an entry
            // that still exists: if the drained chunk decoded this path's own
            // terminator, walk_jsonl fully retired the claim (removed) — a
            // blind insert would resurrect it as merely-released.
            walk_jsonl(path, decoders, ctx).await;
            {
                let mut seen = ctx.seen.lock().await;
                if seen.contains_key(path) {
                    seen.insert(path.clone(), false);
                }
            }
            debug!(
                "child-end un-claim: released first-sight claim on {} — the \
                 next append re-registers (#246)",
                path.display()
            );
        }
    }
}
