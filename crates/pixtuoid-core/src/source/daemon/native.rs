//! The `native`-only runtime half of the daemon presence layer: the tokio
//! presence side channel + the shared gateway-pid exit watcher. The pure
//! state machine (`apply_presence`, the sweeps, the vocabulary) stays in the
//! always-compiled parent module; this whole file sits behind the parent's
//! ONE `#[cfg(feature = "native")] mod native;` gate and is re-exported
//! there, so public paths don't move.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{DaemonPresenceUpdate, PresenceMsg};

/// The daemon-presence SIDE channel (invariant #2: NOT the one `AgentEvent`
/// channel). Unbounded — presence deltas are tiny + rare.
pub type PresenceSender = tokio::sync::mpsc::UnboundedSender<PresenceMsg>;

/// A handle to arm gateway-pid exit watches across ALL daemons. A dying gateway
/// pid converts to a source-tagged `PidExited` presence delta — the instant
/// abrupt-down rung — reusing the AGNOSTIC `ExitWatch` (pid → channel, no
/// `AgentId` coupling), NOT `HookPidWatch` (which emits an AgentSlot-shaped
/// `SessionEnd` the non-slot mascot can't consume). One watcher multiplexes
/// every daemon's pid; the `pid → source` binding routes the death back.
pub struct PresenceExitWatch {
    inner: crate::source::exit_watch::ExitWatch,
    /// pid → owning daemon source, so a death emits `(source, PidExited)`.
    pids: Arc<Mutex<HashMap<i32, String>>>,
}

impl PresenceExitWatch {
    /// Watch a daemon's gateway pid; its death emits `(source, PidExited)`.
    /// Idempotent per pid (a re-arm just refreshes the binding).
    pub fn watch(&self, source: &str, pid: i32) {
        self.pids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid, source.to_string());
        self.inner.watch(pid);
    }
}

/// Spawn the shared gateway-pid exit watcher: pid deaths drain into source-tagged
/// `PidExited` on `presence_tx`. `None` where the platform has no exit-watch
/// backend (then the `presence_ttl_ms` sweep is the only abrupt-down signal).
/// Call in a tokio runtime.
pub fn spawn_presence_exit_watch(presence_tx: PresenceSender) -> Option<PresenceExitWatch> {
    let pids: Arc<Mutex<HashMap<i32, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let (pid_tx, mut pid_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();
    let inner = crate::source::exit_watch::ExitWatch::spawn(pid_tx)?;
    let pids_drain = Arc::clone(&pids);
    tokio::spawn(async move {
        while let Some(pid) = pid_rx.recv().await {
            // A pid with no binding is a stale receipt (already routed) — skip.
            let source = pids_drain
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&pid);
            if let Some(source) = source {
                if presence_tx
                    .send(PresenceMsg {
                        source,
                        delta: DaemonPresenceUpdate::PidExited { pid },
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    Some(PresenceExitWatch { inner, pids })
}
