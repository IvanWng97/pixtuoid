//! The ONE source→reducer pipeline spine both painters boot through (#714).
//!
//! `run_async` (TUI) and `floating::run` used to hand-mirror this glue line
//! for line — a new channel or spawn had to be added to both files (the
//! presence channel + exit watch historically were). The MECHANISM was
//! already shared (`build_source_set`, `reducer_task`); this module owns the
//! wiring between them. What deliberately STAYS caller-side, because the two
//! painters genuinely diverge there:
//! - **`boot_caps`**: the TUI measures the terminal (footer-subtracting,
//!   cap-clamped, headless arms); floating measures its window pixels — a
//!   documented sharp edge (reusing the TUI math over-seeds and strands
//!   agents on non-existent desks), so the pipeline takes the seed as a
//!   PARAMETER and must never compute it. It is also WHEN they diverge:
//!   floating cannot seed until a real window exists, so it calls this from
//!   `window::resumed` rather than at boot (#803).
//! - **socket resolution + `ConnectedSources`**: both values outlive the
//!   boot (the Sources panel displays the path and mutates the live set), so
//!   the callers own them and hand in a clone.
//!
//! Requires an ambient tokio runtime context — `run_async` executes inside
//! `block_on`, floating holds an `rt.enter()` guard — so the `tokio::spawn`s
//! here land on the caller's runtime either way.
//!
//! Like its two callers this is codecov-excluded async glue (#103): the win
//! is ONE authority for the wiring, not new test surface — behavior stays
//! pinned through the reducer/watcher/transport suites that drive the
//! spawned halves.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use pixtuoid_core::source::daemon;
use pixtuoid_core::source::manager::{SourceDeath, SourceManager};
use pixtuoid_core::state::MAX_FLOORS;
use pixtuoid_core::{AgentEvent, SceneState, Transport};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::driver::{build_source_set, reducer_task};
use super::ConnectedSources;

/// The live pipeline's caller-facing handles. The spawned source tasks are
/// kept alive by the caller's tokio RUNTIME (floating's `rt`, `run_async`'s
/// `block_on`), not by `_source_handles` — dropping a tokio `JoinHandle`
/// DETACHES the task, it doesn't stop it (this fn itself discards the
/// reducer's handle). The field is a harmless anchor that also lets a future
/// caller `.abort()`/join the sources.
pub(crate) struct Pipeline {
    pub(crate) scene_rx: watch::Receiver<Arc<SceneState>>,
    pub(crate) health_rx: watch::Receiver<Vec<SourceDeath>>,
    pub(crate) floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
    pub(crate) _source_handles: Vec<JoinHandle<()>>,
}

/// Wire and spawn the whole live pipeline: presence channel + exit watch →
/// [`build_source_set`] → event/scene/health channels → boot-seeded
/// floor-caps atomics → [`reducer_task`] → `SourceManager::spawn_with_health`.
pub(crate) fn spawn_pipeline(
    socket_path: PathBuf,
    projects_root: Option<PathBuf>,
    codex_sessions_root: Option<PathBuf>,
    connected: ConnectedSources,
    boot_caps: [usize; MAX_FLOORS],
) -> Pipeline {
    // Daemon-presence SIDE channel (invariant #2: NOT the AgentEvent channel).
    // The HookRouter demux decodes daemon payloads into source-tagged presence
    // deltas sent here; the shared exit watch drains gateway-pid deaths into
    // the SAME channel as `(source, PidExited)`; the reducer task merges both
    // into SceneState::daemons.
    let (presence_tx, presence_rx) = mpsc::unbounded_channel();
    let presence_exit_watch = daemon::spawn_presence_exit_watch(presence_tx.clone());
    let sources = build_source_set(
        socket_path,
        projects_root,
        codex_sessions_root,
        Some(presence_tx),
    );

    let (tx, rx) =
        mpsc::channel::<(Transport, AgentEvent)>(pixtuoid_core::source::EVENT_CHANNEL_CAPACITY);
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(boot_caps)));
    let floor_caps: Arc<[AtomicUsize; MAX_FLOORS]> =
        Arc::new(std::array::from_fn(|i| AtomicUsize::new(boot_caps[i])));
    tokio::spawn(reducer_task(
        rx,
        scene_tx,
        Arc::clone(&floor_caps),
        connected,
        presence_rx,
        presence_exit_watch,
    ));

    // Source-health side channel (#157): a fatal source exit must reach the
    // painter's footer. Deliberately NOT an AgentEvent — the one event channel
    // carries agent activity (its Transport tag drives hook-wins dedup), not
    // source lifecycle.
    let (health_tx, health_rx) = watch::channel(Vec::new());
    let mut manager = SourceManager::new();
    for src in sources {
        manager = manager.with_source(src);
    }
    let _source_handles = manager.spawn_with_health(tx, health_tx);

    Pipeline {
        scene_rx,
        health_rx,
        floor_caps,
        _source_handles,
    }
}
