use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AgentId(u64);

/// splitmix64 finalizer. FNV-1a (used by [`AgentId::from_parts`]) doesn't
/// avalanche the mid/high bits for short, similar inputs — desk-adjacent ids
/// collide to a couple of buckets — so the personality slicers (`speed_mult`,
/// `pause_ms_for`, dwell jitter) finalize the raw id (xor a per-purpose tag)
/// through this before taking a bit window. Not cryptographic.
pub(crate) fn splitmix64(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// FNV-1a 64-bit constants. Open-coded by [`AgentId::from_parts`] (string-id
/// hashing, with a domain separator) and `WalkableMask::signature` (geometry
/// hashing over sorted rects) — the same primitive over different inputs, so
/// they share the magic numbers, not a streaming hasher.
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
pub(crate) const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl AgentId {
    /// Test/example opaque-id factory — mints a stable, distinct `AgentId`
    /// from a string by calling `from_parts("claude-code",
    /// normalize_path_key(path))`. This is **not** how production CC keying
    /// works anymore: CC now keys on the session UUID (the transcript filename
    /// stem), derived by `cc_id_from_path`, which is cwd-independent.
    /// `from_transcript_path` is kept because the test + snapshot suites lean
    /// on it heavily — `normalize_path_key` makes every expectation they build
    /// platform-consistent by construction. Do not call this in production
    /// decode paths; use `from_parts(source, &cc_id_from_path(path))` instead.
    #[doc(hidden)]
    pub fn from_transcript_path(path: &str) -> Self {
        Self::from_parts(
            "claude-code",
            &crate::source::decoder::normalize_path_key(path),
        )
    }

    /// Source-agnostic factory. `source` is the source's name (matches the
    /// `Source::name()` return value, e.g. `"claude-code"`, `"codex"`,
    /// `"cursor"`); `opaque_id` is whatever the source uses to uniquely
    /// identify a session — a session UUID for CC, a rollout-filename UUID for
    /// Codex, the cwd for a hook-only source. The pair is
    /// hashed so two sources with the same `opaque_id` produce distinct
    /// `AgentId`s (no cross-source collisions).
    pub fn from_parts(source: &str, opaque_id: &str) -> Self {
        let mut hash: u64 = FNV_OFFSET_BASIS;
        for b in source.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        // Domain separator between source and opaque id so e.g. source="a",
        // opaque="bc" doesn't collide with source="ab", opaque="c".
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
        for b in opaque_id.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        AgentId(hash)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_is_deterministic_per_path() {
        let a = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        let b = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        assert_eq!(a, b);
    }

    #[test]
    fn agent_id_differs_per_path() {
        let a = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        let b = AgentId::from_transcript_path("/Users/me/.claude/projects/x/def.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn agent_id_displays_as_hex() {
        let id = AgentId::from_transcript_path("x");
        assert_eq!(format!("{id}").len(), 16);
    }

    #[test]
    fn from_parts_distinguishes_source_and_opaque() {
        // Two sources with the same opaque_id must NOT collide.
        let cc = AgentId::from_parts("claude-code", "session-123");
        let cx = AgentId::from_parts("codex", "session-123");
        assert_ne!(cc, cx);
    }

    #[test]
    fn from_parts_has_domain_separator() {
        // ("a", "bc") must NOT hash the same as ("ab", "c") — proves the
        // domain separator between source and opaque_id is doing its job.
        let a = AgentId::from_parts("a", "bc");
        let b = AgentId::from_parts("ab", "c");
        assert_ne!(a, b);
    }

    #[test]
    fn from_transcript_path_routes_through_from_parts() {
        // For an already-normalized path (lowercase, forward slashes) the shim
        // equals raw from_parts on every platform — the fold only rewrites
        // backslash/uppercase forms (pinned in source::decoder's unit tests).
        let a = AgentId::from_transcript_path("/x.jsonl");
        let b = AgentId::from_parts("claude-code", "/x.jsonl");
        assert_eq!(a, b);
    }
}
