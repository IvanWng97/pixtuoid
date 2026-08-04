use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::debug;

use crate::AgentId;

use super::walk::{park_if_truncated_below_cursor, walk_jsonl};
use super::{SourceDecoders, WatchCtx};

/// How long an un-drained [`ChildEndUnclaims`] entry survives. Bounds the set:
/// an id no watcher ever matches would otherwise accrue one entry per child end
/// for the process lifetime. A TTL, not a size cap, because staleness — not
/// volume — is the failure mode, and a cap could evict a fresh entry under a
/// burst of foreign ones.
const CHILD_END_UNCLAIM_TTL: Duration = Duration::from_secs(300);

/// The child-end un-claim side-channel: child ids whose hook
/// `SessionEnd { as_child: true }` (a decoded `SubagentStop`) was observed, and
/// whose `seen`-claimed transcript should therefore be RELEASED so the next
/// append re-registers.
///
/// WHY a side-channel: a multi-turn Codex child gets a `SubagentStop` hook at
/// EVERY turn end but no `SessionStart` carrier of any kind at turn N+1 —
/// upstream provides none — and the hook-side end never touched the watcher's
/// `seen` claim, so turn N+1's appends decoded as unknown-id no-ops forever.
///
/// PRODUCER: the tee inside `HookRouter::run`, the ONE shared socket all
/// sources' hook payloads ride. CONSUMERS: each WIRED source's `JsonlWatcher`
/// drains only the ids matching its OWN claimed paths; foreign ids stay pending
/// for the watcher that owns them (`AgentId` is source-namespaced, so there is
/// exactly one owner) until the TTL prunes them.
#[derive(Clone)]
pub struct ChildEndUnclaims {
    /// `(id, pushed-at)`. `Instant` (monotonic) — a wall-clock jump must not
    /// fake or starve the TTL.
    entries: Arc<std::sync::Mutex<Vec<(AgentId, std::time::Instant)>>>,
    ttl: Duration,
}

impl ChildEndUnclaims {
    /// An empty side-channel with the default prune TTL.
    pub fn new() -> Self {
        Self::with_ttl(CHILD_END_UNCLAIM_TTL)
    }

    /// Test-only seam: shrinks the prune TTL so the bounded-growth contract is
    /// testable without a 5-minute wait.
    #[doc(hidden)]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(std::sync::Mutex::new(Vec::new())),
            ttl,
        }
    }

    /// Record a child id whose hook end was decoded. A repeated push (a
    /// multi-turn child ends once per turn) refreshes the entry's TTL clock
    /// rather than duplicating it. A poisoned lock degrades to a dropped push —
    /// the stale-sweep ladder still reaps, so never panic here.
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
    /// holding only TTL-expired entries reads as non-empty — harmless, the full
    /// drain it admits prunes them; "empty" is never wrong.
    fn is_empty(&self) -> bool {
        self.entries.lock().map(|e| e.is_empty()).unwrap_or(true)
    }
}

impl Default for ChildEndUnclaims {
    fn default() -> Self {
        Self::new()
    }
}

/// Consumer half of the un-claim side-channel (see [`ChildEndUnclaims`] for the
/// WHY). For every pending id matching one of THIS watcher's `seen` paths: walk
/// the path's pending bytes to EOF FIRST — a pre-stop straggler that re-entered
/// as a first-sight would resurrect the just-ended child as a ghost — then
/// RELEASE the claim. Two deliberate differences from `emit_session_exit`:
///
/// * **No `SessionEnd` is emitted** — the reducer already ended the slot from
///   the hook `SubagentStop`.
/// * **Release = `seen` → `false`, not removal.** The path must stay KNOWN: the
///   Codex FD probe keeps vouching an in-flight child's still-open rollout, so a
///   removed claim would let `revouch_gated_files` reset the cursor to 0 on the
///   next scan — a full stale-activity replay plus an instant re-registration
///   that negates the SubagentStop end.
///
/// Accepted residual: bytes the child wrote between the hook Stop and this drain
/// are consumed silently, indistinguishable from pre-stop stragglers without
/// per-byte timestamps.
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
    // Snapshot under a short lock so the later walk/release loop is lock-free.
    let claimed: Vec<(PathBuf, AgentId, bool)> = {
        let seen = ctx.seen.lock().await;
        seen.iter()
            .map(|(p, &held)| {
                (
                    p.clone(),
                    // Folded via `walk::id_path` like every other derivation
                    // seam — a raw path computes an id in a different space.
                    AgentId::from_parts(
                        ctx.source,
                        &(decoders.id_derive)(&super::walk::id_path(p)),
                    ),
                    held,
                )
            })
            .collect()
    };
    // Consume ids matching ANY of this watcher's known paths (held or already
    // released — a duplicate stop's work is done either way); foreign ids stay
    // pending for their owning watcher.
    let matched = unclaims.take_matching(|id| claimed.iter().any(|(_, pid, _)| pid == id));
    for id in matched {
        for (path, pid, held) in &claimed {
            if *pid != id || !*held {
                continue;
            }
            // Drain pending bytes to EOF while the claim is still held (a
            // straggler decodes as an already-claimed walk, registering
            // nothing), THEN release. The release only DOWNGRADES an entry that
            // still exists: if the drained chunk decoded this path's own
            // terminator, walk_jsonl fully retired the claim, and a blind
            // insert would resurrect it as merely-released.
            park_if_truncated_below_cursor(path, ctx).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> AgentId {
        AgentId::from_parts("test", n)
    }

    #[test]
    fn push_keeps_fresh_siblings_and_dedupes_on_identity() {
        let unclaims = ChildEndUnclaims::new();
        unclaims.push(id("a"));
        unclaims.push(id("b"));
        unclaims.push(id("a")); // refresh, not a duplicate
        let mut taken = unclaims.take_matching(|_| true);
        taken.sort_by_key(|i| format!("{i:?}"));
        let mut expected = vec![id("a"), id("b")];
        expected.sort_by_key(|i| format!("{i:?}"));
        assert_eq!(taken, expected, "both fresh ids pending, each exactly once");
    }

    #[test]
    fn is_empty_tracks_pending_entries() {
        let unclaims = ChildEndUnclaims::new();
        assert!(unclaims.is_empty());
        unclaims.push(id("a"));
        assert!(!unclaims.is_empty());
        unclaims.take_matching(|_| true);
        assert!(unclaims.is_empty());
    }
}
