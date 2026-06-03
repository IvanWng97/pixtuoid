use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::source::{AgentEvent, TaggedSender, Transport};
use crate::AgentId;

pub type LineDecoder = fn(&str, &str, serde_json::Value) -> Result<Vec<AgentEvent>>;
pub type LabelDeriver = fn(&Path, &str, &Path) -> String;
pub type SessionEndChecker = fn(&[u8]) -> bool;

/// Derives the opaque session-id string used to build the generic
/// `SessionStart`'s `AgentId`. Default returns the transcript file path
/// (CC/Antigravity coalesce hook↔JSONL on the path). Codex overrides it to
/// the rollout filename's trailing UUID so it matches the hook `session_id`.
pub type IdDeriver = fn(&Path) -> String;

fn default_id_from_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The per-source decode/label/end/id fn-pointers (the invariant-#3 seam)
/// bundled so the seed/scan/walk helpers thread ONE Copy value, not four.
#[derive(Clone, Copy)]
struct SourceDecoders {
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_ended: SessionEndChecker,
    id_derive: IdDeriver,
}

/// Shared per-run watch state, borrowed by the scan/walk helpers.
#[derive(Clone, Copy)]
struct WatchCtx<'a> {
    source: &'a Arc<str>,
    cursors: &'a Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &'a Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &'a TaggedSender,
    /// Recency window for the first-sight gate (a file older than this is
    /// seeded at EOF without a SessionStart). The whole watch shares one window
    /// so every path that can first-see a file gates identically (see #85).
    window: Duration,
}

pub struct JsonlWatcher {
    root: PathBuf,
    initial_window: Duration,
    source_name: String,
    decode_line: LineDecoder,
    derive_label: LabelDeriver,
    check_session_ended: SessionEndChecker,
    id_derive: IdDeriver,
}

const DEFAULT_INITIAL_WINDOW: Duration = Duration::from_secs(3600);

impl JsonlWatcher {
    pub fn new(
        root: PathBuf,
        source: String,
        decode_line: LineDecoder,
        derive_label: LabelDeriver,
        check_session_ended: SessionEndChecker,
    ) -> Self {
        Self {
            root,
            initial_window: DEFAULT_INITIAL_WINDOW,
            source_name: source,
            decode_line,
            derive_label,
            check_session_ended,
            id_derive: default_id_from_path,
        }
    }

    pub fn with_initial_window(mut self, window: Duration) -> Self {
        self.initial_window = window;
        self
    }

    pub fn with_id_deriver(mut self, id_derive: IdDeriver) -> Self {
        self.id_derive = id_derive;
        self
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        let cursors: Arc<Mutex<HashMap<PathBuf, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let seen_sessions: Arc<Mutex<HashMap<PathBuf, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                            let _ = notify_tx.send(path);
                        }
                    }
                }
            })?;
        let _ = tokio::fs::create_dir_all(&self.root).await;
        watcher.watch(&self.root, RecursiveMode::Recursive)?;

        let source_arc: Arc<str> = Arc::from(self.source_name.as_str());
        let decoders = SourceDecoders {
            decode_line: self.decode_line,
            derive_label: self.derive_label,
            check_ended: self.check_session_ended,
            id_derive: self.id_derive,
        };

        // Initial seed: the same `scan_root` → `walk_jsonl` path every later scan
        // uses, so a file is gated identically (recency + session_end) no matter
        // which pass first sees it. (Previously a separate `initial_seed_walk`
        // owned the gate and `walk_jsonl` had none — the divergence behind #85.)
        scan_root(
            &self.root,
            decoders,
            &WatchCtx {
                source: &source_arc,
                cursors: &cursors,
                seen: &seen_sessions,
                tx: &tx,
                window: self.initial_window,
            },
        )
        .await;

        // Re-scan shortly after startup to catch files that APFS read_dir
        // missed during the initial seed walk (metadata propagation race).
        // walk_jsonl is idempotent (cursor == file_len → no-op).
        let mut rescan_done = false;
        let rescan_delay = tokio::time::sleep(Duration::from_millis(250));
        tokio::pin!(rescan_delay);

        loop {
            let source_arc = source_arc.clone();
            let ctx = WatchCtx {
                source: &source_arc,
                cursors: &cursors,
                seen: &seen_sessions,
                tx: &tx,
                window: self.initial_window,
            };
            tokio::select! {
                Some(path) = notify_rx.recv() => {
                    walk_jsonl(&path, decoders, &ctx).await;
                }
                _ = &mut rescan_delay, if !rescan_done => {
                    rescan_done = true;
                    scan_root(&self.root, decoders, &ctx).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    scan_root(&self.root, decoders, &ctx).await;
                }
            }
        }
    }
}

/// First-sight decision, shared by EVERY path that can be the first to see a
/// file (the initial seed, the 250ms rescan, the 60s poll, a notify event):
/// seed the cursor at EOF — suppressing SessionStart — when the session is
/// historical (mtime outside `window`) OR already ended (a session_end marker in
/// its tail). Only a recent, not-yet-ended file is read from the top. Unifying
/// the gate here (rather than only in the old `initial_seed_walk`) is the #85
/// fix: the post-startup rescan used to bypass it and resurrect a missed
/// ended/stale session as a phantom live sprite.
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

async fn scan_root(root: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    if let Ok(mut read) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = read.next_entry().await {
            walk_jsonl(&entry.path(), decoders, ctx).await;
        }
    }
}

async fn walk_jsonl(path: &Path, decoders: SourceDecoders, ctx: &WatchCtx<'_>) {
    let WatchCtx {
        source,
        cursors,
        seen,
        tx,
        window,
    } = *ctx;
    let SourceDecoders {
        decode_line,
        derive_label,
        check_ended,
        id_derive,
    } = decoders;
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return,
    };
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

    let file_len = meta.len();
    const MAX_PENDING_BYTES: u64 = 1 << 20;

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
    if !known && should_seed_at_eof(&meta, window, path, check_ended).await {
        cursors.lock().await.insert(path.to_path_buf(), file_len);
        return;
    }
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
            "{} has > {} pending bytes with no newline; skipping to end",
            path.display(),
            MAX_PENDING_BYTES
        );
        cursors.lock().await.insert(path.to_path_buf(), file_len);
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
    let transcript_path_str = path.to_string_lossy().into_owned();

    // Take the `seen` lock ONLY to claim first-sight, then drop it before the
    // awaited sends — holding it across `tx.send` would block on a slow consumer
    // for no reason (the flag flip is the entire critical section). Mirrors the
    // narrow `cursors` locking above.
    let is_first = seen.lock().await.insert(path.to_path_buf(), true).is_none();
    if is_first {
        let id = AgentId::from_parts(source, &id_derive(path));
        let session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cwd = extract_cwd(new_bytes).unwrap_or_default();
        let parent_id = detect_parent_id(path, source);
        let _ = tx
            .send((
                Transport::Jsonl,
                AgentEvent::SessionStart {
                    agent_id: id,
                    source: source.to_string(),
                    session_id: session_id.clone(),
                    cwd: cwd.clone(),
                    parent_id,
                },
            ))
            .await;

        let label = derive_label(path, source, &cwd);
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
                    if tx.send((Transport::Jsonl, ev)).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => warn!("decode error in {}: {e}", path.display()),
        }
    }
}

/// Read the tail of a file and delegate to the source-specific checker.
async fn check_session_ended(path: &Path, checker: SessionEndChecker) -> bool {
    const TAIL_BYTES: u64 = 8192;
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return false;
    };
    let file_len = meta.len();
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let start = file_len.saturating_sub(TAIL_BYTES);
    if tokio::io::AsyncSeekExt::seek(&mut file, SeekFrom::Start(start))
        .await
        .is_err()
    {
        return false;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES as usize);
    if tokio::io::AsyncReadExt::read_to_end(&mut file, &mut buf)
        .await
        .is_err()
    {
        return false;
    }
    checker(&buf)
}

/// The path segment a CC subagent transcript carries: `<parent>/subagents/
/// agent-*.jsonl`. Slash-bounded so a project dir merely *containing* the word
/// (e.g. `subagents-paper`) is not mistaken for one — single source of truth for
/// both `is_subagent_path` and `detect_parent_id` so they cannot diverge (they
/// did once: see the `bug_004` fix in `cc_derive_label`).
const SUBAGENTS_SEGMENT: &str = "/subagents/";

/// Whether a transcript path is a CC subagent transcript (vs a top-level
/// session). Codex subagents are FLAT (no such segment) — they're linked via the
/// `SubagentStart` hook instead, so this predicate is CC-layout-specific.
pub(crate) fn is_subagent_path(path: &Path) -> bool {
    path.to_string_lossy().contains(SUBAGENTS_SEGMENT)
}

/// Detect if this transcript is a CC subagent by checking for the `/subagents/`
/// path segment. If found, derive the parent's AgentId from the grandparent
/// directory (the parent session's transcript directory). CC-layout-specific —
/// Codex subagent parent links come from the `SubagentStart` hook, not the path.
fn detect_parent_id(path: &Path, source: &str) -> Option<AgentId> {
    let path_str = path.to_string_lossy();
    let idx = path_str.find(SUBAGENTS_SEGMENT)?;
    let parent_dir = &path_str[..idx];
    let parent_jsonl = format!("{parent_dir}.jsonl");
    Some(AgentId::from_parts(source, &parent_jsonl))
}

fn extract_cwd(bytes: &[u8]) -> Option<PathBuf> {
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
        if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
            return Some(PathBuf::from(cwd));
        }
        if let Some(cwd) = v
            .get("payload")
            .and_then(|p| p.get("cwd"))
            .and_then(|c| c.as_str())
        {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_id_from_path_returns_full_path_string() {
        let p = Path::new("/Users/me/.claude/projects/x/abc.jsonl");
        assert_eq!(
            default_id_from_path(p),
            "/Users/me/.claude/projects/x/abc.jsonl"
        );
    }

    #[test]
    fn extract_cwd_reads_top_level_and_nested_payload() {
        // CC/AG shape: top-level cwd.
        let top = br#"{"cwd":"/repo/a"}"#;
        assert_eq!(extract_cwd(top), Some(PathBuf::from("/repo/a")));
        // Codex shape: cwd nested under payload (session_meta).
        let nested = br#"{"type":"session_meta","payload":{"cwd":"/repo/b","id":"u"}}"#;
        assert_eq!(extract_cwd(nested), Some(PathBuf::from("/repo/b")));
    }

    fn t_decode(_t: &str, _s: &str, _v: serde_json::Value) -> Result<Vec<AgentEvent>> {
        Ok(vec![])
    }
    fn t_label(_p: &Path, _s: &str, _c: &Path) -> String {
        "t".to_string()
    }
    fn t_ended(buf: &[u8]) -> bool {
        std::str::from_utf8(buf).is_ok_and(|s| s.contains("session_end"))
    }

    /// Drive `walk_jsonl` once over a fresh (never-seeded) file — the
    /// deterministic, timing-free repro of the #85 race. When the watcher's
    /// `walk_jsonl` (rescan / 60s poll / notify) is the FIRST to see a file,
    /// does it gate (ended/stale) or resurrect it? Returns the emitted events +
    /// the cursor it left.
    async fn first_sight_walk(
        path: &Path,
        window: Duration,
        check_ended: SessionEndChecker,
    ) -> (Vec<(Transport, AgentEvent)>, Option<u64>) {
        let cursors = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Transport, AgentEvent)>(32);
        let source: Arc<str> = Arc::from("test");
        let decoders = SourceDecoders {
            decode_line: t_decode,
            derive_label: t_label,
            check_ended,
            id_derive: default_id_from_path,
        };
        let ctx = WatchCtx {
            source: &source,
            cursors: &cursors,
            seen: &seen,
            tx: &tx,
            window,
        };
        walk_jsonl(path, decoders, &ctx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let cursor = cursors.lock().await.get(path).copied();
        (events, cursor)
    }

    #[tokio::test]
    async fn walk_jsonl_gates_a_first_sight_ended_file() {
        // #85: an ENDED session the initial read_dir missed must NOT be
        // resurrected when the rescan's walk_jsonl is the first to see it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ended.jsonl");
        let content = "{\"type\":\"system\",\"subtype\":\"session_start\"}\n\
                       {\"type\":\"system\",\"subtype\":\"session_end\"}\n";
        tokio::fs::write(&path, content).await.unwrap();
        let len = tokio::fs::metadata(&path).await.unwrap().len();

        let (events, cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
        assert!(
            events.is_empty(),
            "a never-seeded ENDED file must not emit SessionStart, got {events:?}"
        );
        assert_eq!(cursor, Some(len), "ended file must be seeded at EOF");
    }

    #[tokio::test]
    async fn walk_jsonl_gates_a_first_sight_stale_file() {
        // The stale-on-startup flake's root: an OLD file the initial read_dir
        // missed must be seeded at EOF by the rescan, not read from the top.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
            .await
            .unwrap();
        filetime::set_file_mtime(
            &path,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - Duration::from_secs(3600),
            ),
        )
        .unwrap();
        let len = tokio::fs::metadata(&path).await.unwrap().len();

        let (events, cursor) = first_sight_walk(&path, Duration::from_secs(60), t_ended).await;
        assert!(
            events.is_empty(),
            "a never-seeded STALE file must not emit SessionStart, got {events:?}"
        );
        assert_eq!(cursor, Some(len), "stale file must be seeded at EOF");
    }

    #[tokio::test]
    async fn walk_jsonl_emits_for_a_first_sight_recent_live_file() {
        // The gate must NOT over-suppress: a recent, not-ended file seen first by
        // any path is a live session and must still get its SessionStart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.jsonl");
        tokio::fs::write(&path, "{\"type\":\"assistant\",\"cwd\":\"/r\"}\n")
            .await
            .unwrap();

        let (events, _cursor) = first_sight_walk(&path, Duration::from_secs(3600), t_ended).await;
        assert!(
            events
                .iter()
                .any(|(_, e)| matches!(e, AgentEvent::SessionStart { .. })),
            "a recent, not-ended file seen first must still emit SessionStart, got {events:?}"
        );
    }
}
