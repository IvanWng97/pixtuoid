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

/// Shared per-run watch state, borrowed by the seed/scan/walk helpers.
#[derive(Clone, Copy)]
struct WatchCtx<'a> {
    source: &'a Arc<str>,
    cursors: &'a Arc<Mutex<HashMap<PathBuf, u64>>>,
    seen: &'a Arc<Mutex<HashMap<PathBuf, bool>>>,
    tx: &'a TaggedSender,
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

        initial_seed_root(
            &self.root,
            self.initial_window,
            decoders,
            &WatchCtx {
                source: &source_arc,
                cursors: &cursors,
                seen: &seen_sessions,
                tx: &tx,
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

async fn initial_seed_root(
    root: &Path,
    window: Duration,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) {
    if let Ok(mut read) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = read.next_entry().await {
            initial_seed_walk(&entry.path(), window, decoders, ctx).await;
        }
    }
}

async fn initial_seed_walk(
    path: &Path,
    window: Duration,
    decoders: SourceDecoders,
    ctx: &WatchCtx<'_>,
) {
    let WatchCtx { cursors, .. } = *ctx;
    let SourceDecoders { check_ended, .. } = decoders;
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if meta.is_dir() {
        if let Ok(mut read) = tokio::fs::read_dir(path).await {
            while let Ok(Some(entry)) = read.next_entry().await {
                Box::pin(initial_seed_walk(&entry.path(), window, decoders, ctx)).await;
            }
        }
        return;
    }
    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
        return;
    }

    let recent = meta
        .modified()
        .ok()
        .map(|mtime| {
            // elapsed() returns Err when mtime is in the future (clock jitter
            // on APFS nanosecond-precision filesystems). A future mtime is
            // necessarily within any recency window.
            let elapsed = mtime.elapsed().unwrap_or(Duration::ZERO);
            elapsed <= window
        })
        .unwrap_or(false);

    if recent {
        let ended = check_session_ended(path, check_ended).await;
        if ended {
            cursors.lock().await.insert(path.to_path_buf(), meta.len());
        } else {
            walk_jsonl(path, decoders, ctx).await;
        }
    } else {
        cursors.lock().await.insert(path.to_path_buf(), meta.len());
    }
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
    } = *ctx;
    let SourceDecoders {
        decode_line,
        derive_label,
        id_derive,
        ..
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

    let cursor_now: u64 = {
        let cursors_g = cursors.lock().await;
        *cursors_g.get(path).unwrap_or(&0)
    };
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
}
