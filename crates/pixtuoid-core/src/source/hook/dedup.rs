//! Duplicate-DELIVERY suppression for the shared hook socket.
//!
//! A CLI may run one hook command several times for a single event: cursor runs
//! it 4–6× per event with the standard install (measured against a counting
//! wrapper on the shim, with no recorder in the loop, and against an unrelated
//! hook entry in the same config that ran once). Every copy decodes to a real
//! `ActivityStart`, and `apply_activity_start` increments `tool_call_count`
//! unconditionally, so the HUD's tool count came out several times the truth.
//!
//! **What makes this safe is the gate, not the window.** Only a payload carrying
//! the source's own per-call id takes part: two GENUINE calls of the same tool on
//! the same file carry DIFFERENT ids, so they can never collide, while a
//! re-delivery is the same id in a byte-identical payload. Everything id-less —
//! session lifecycle, codewhale's minimal envelope — passes through untouched,
//! and is idempotent in the reducer anyway.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::source::registry;

/// The shim's own stamps, which differ per invocation and must not defeat the
/// fingerprint they would otherwise make unique.
const SHIM_STAMPS: [&str; 2] = ["_shim_ts_ms", "_pid"];

/// How many recent deliveries stay comparable. Bounded by COUNT rather than
/// time: a per-call id names ONE call, so a repeat of it is a duplicate whenever
/// it arrives, and a count needs no clock to test against.
const WINDOW: usize = 256;

/// Shared across connections — one hook invocation is one connection, so
/// per-connection state would see no duplicate at all.
#[derive(Clone, Default)]
pub(crate) struct DeliveryDedup {
    seen: Arc<Mutex<Recent>>,
}

#[derive(Default)]
struct Recent {
    set: HashSet<u64>,
    order: VecDeque<u64>,
}

impl DeliveryDedup {
    /// `false` when this exact payload was already accepted — the caller drops
    /// it. A payload with no per-call id is always accepted.
    pub(crate) fn accept(&self, v: &Value) -> bool {
        let Some(fingerprint) = fingerprint(v) else {
            return true;
        };
        let mut recent = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if !recent.set.insert(fingerprint) {
            return false;
        }
        recent.order.push_back(fingerprint);
        while recent.order.len() > WINDOW {
            if let Some(old) = recent.order.pop_front() {
                recent.set.remove(&old);
            }
        }
        true
    }
}

/// The payload's identity, or `None` when it carries no per-call id and is
/// therefore out of scope.
fn fingerprint(v: &Value) -> Option<u64> {
    let obj = v.as_object()?;
    let source = obj.get("_pixtuoid_source")?.as_str()?;
    let id_key = registry::descriptor_for(source)?.hook()?.tool_id_key;
    obj.get(id_key.wire_name())?
        .as_str()
        .filter(|s| !s.is_empty())?;

    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    for (k, val) in obj {
        if SHIM_STAMPS.contains(&k.as_str()) {
            continue;
        }
        k.hash(&mut h);
        val.to_string().hash(&mut h);
    }
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cursor_tool(id: &str, file: &str) -> Value {
        json!({
            "_pixtuoid_source": "cursor",
            "hook_event_name": "preToolUse",
            "session_id": "s1",
            "tool_name": "Read",
            "tool_use_id": id,
            "tool_input": {"file_path": file},
        })
    }

    #[test]
    fn a_redelivered_payload_is_dropped_once_seen() {
        let d = DeliveryDedup::default();
        let mut p = cursor_tool("t1", "/a");
        assert!(d.accept(&p), "first delivery");
        // A second invocation differs ONLY in the shim's own stamps.
        p["_shim_ts_ms"] = json!(1);
        assert!(
            !d.accept(&p),
            "the same call re-delivered must not count twice"
        );
        p["_shim_ts_ms"] = json!(2);
        p["_pid"] = json!(999);
        assert!(!d.accept(&p), "nor a third");
    }

    /// The gate's whole point: an agent reading the SAME file twice is two calls,
    /// and cursor gives them different ids. Dropping one would UNDERCOUNT, which
    /// is the failure this must never trade for.
    #[test]
    fn two_genuine_calls_on_one_file_both_count() {
        let d = DeliveryDedup::default();
        assert!(d.accept(&cursor_tool("t1", "/same")));
        assert!(d.accept(&cursor_tool("t2", "/same")));
    }

    #[test]
    fn the_matching_end_is_a_different_delivery() {
        let d = DeliveryDedup::default();
        assert!(d.accept(&cursor_tool("t1", "/a")));
        let mut post = cursor_tool("t1", "/a");
        post["hook_event_name"] = json!("postToolUse");
        assert!(post != cursor_tool("t1", "/a"));
        assert!(
            d.accept(&post),
            "pre and post share an id but are two deliveries"
        );
    }

    #[test]
    fn an_id_less_payload_is_never_suppressed() {
        let d = DeliveryDedup::default();
        let session = json!({
            "_pixtuoid_source": "cursor",
            "hook_event_name": "sessionStart",
            "session_id": "s1",
        });
        assert!(d.accept(&session));
        assert!(d.accept(&session), "no id means no claim about identity");
    }

    /// kimi spells the id `tool_call_id`; the fingerprint reads the registry row
    /// rather than a second copy of that name.
    #[test]
    fn the_gate_reads_each_sources_own_id_spelling() {
        let d = DeliveryDedup::default();
        let mut k = json!({
            "_pixtuoid_source": "kimi",
            "hook_event_name": "PreToolUse",
            "session_id": "s",
            "tool_call_id": "call_1",
        });
        assert!(d.accept(&k));
        k["_shim_ts_ms"] = json!(7);
        assert!(!d.accept(&k), "kimi's id must gate it too");
    }

    #[test]
    fn the_window_is_bounded() {
        let d = DeliveryDedup::default();
        for i in 0..WINDOW + 10 {
            assert!(d.accept(&cursor_tool(&format!("t{i}"), "/a")));
        }
        let recent = d.seen.lock().unwrap();
        assert_eq!(recent.order.len(), WINDOW);
        assert_eq!(recent.set.len(), WINDOW);
    }
}
