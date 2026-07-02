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

/// Canonical form of a path STRING before it is used as an identity key
/// (an `AgentId` transcript-path key, or the palette's cwd outfit key).
/// Identity on Unix. On Windows: `\`→`/` + lowercase — CC emits backslash
/// paths in hook payloads but mixes `\`/`/` forms of the same file
/// internally, and NTFS is case-insensitive; without folding, the hook key
/// and the watcher key hash to two different AgentIds and every session
/// renders as TWO sprites. Used directly as an opaque key by **Antigravity**
/// (whose hook keys on the normalized path). **CC** and **Codex** pass the
/// normalized path string to their line decoders only as a routing hint —
/// each decoder then extracts a UUID from the filename stem
/// (`cc_id_from_path` / `codex_id_from_path`), so the fold is inert for them
/// on Unix but still required so `normalize_path_key` is the one entry point
/// for the `walk_jsonl` normalized-path string and `default_id_from_path`
/// (Antigravity's watcher key) — those two paths must always agree.
///
/// Lives HERE (the identity-keying module), not in `source::decoder`: the
/// `pixtuoid-scene` palette deliberately shares this one identity-key
/// definition (Team Palette keys outfits on the normalized cwd), and the
/// render layer must not depend on the decoder layer for it.
///
/// `pub` + `#[doc(hidden)]`: internal cross-crate helper, NOT a stable API —
/// `#[doc(hidden)]` keeps it off `pixtuoid-core`'s semver surface (cf.
/// `claude_config_dir`).
#[doc(hidden)]
pub fn normalize_path_key(s: &str) -> String {
    normalize_key_inner(cfg!(windows), s)
}

/// Pure core, separated so the Windows arm is unit-testable on any platform.
fn normalize_key_inner(windows: bool, s: &str) -> String {
    if !windows {
        return s.to_string();
    }
    // Strip the `\\?\` verbatim / extended-length prefix before folding, so a
    // verbatim-prefixed path (the form `std::fs::canonicalize` returns on Windows)
    // keys the same as its plain form — otherwise `\\?\C:\X` folds to `//?/c:/x`
    // and never coalesces with `C:\X`. Defensive (#197): nothing in-tree
    // canonicalizes today, so neither side currently emits a verbatim prefix; this
    // guards a future regression / an upstream CLI that starts sending one.
    // `\\?\UNC\server\share` denotes `\\server\share`.
    let stripped = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    };
    stripped.replace('\\', "/").to_lowercase()
}

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
        Self::from_parts("claude-code", &normalize_path_key(path))
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
        // backslash/uppercase forms (pinned by the normalize tests below).
        let a = AgentId::from_transcript_path("/x.jsonl");
        let b = AgentId::from_parts("claude-code", "/x.jsonl");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_path_key_is_identity_on_unix() {
        // The unix arm must be byte-identity — every existing AgentId
        // (and golden) depends on it.
        assert_eq!(
            normalize_key_inner(false, "/Users/Me/.claude/projects/X/s.jsonl"),
            "/Users/Me/.claude/projects/X/s.jsonl"
        );
    }

    #[test]
    fn normalize_path_key_folds_separators_and_case_on_windows() {
        // CC mixes \ and / forms of the same path, and NTFS is
        // case-insensitive — both fold to one key (windows arm is pure
        // string code, testable on any platform).
        let a = normalize_key_inner(true, r"C:\Users\Me\.claude\projects\X\s.jsonl");
        assert_eq!(a, "c:/users/me/.claude/projects/x/s.jsonl");
        assert_eq!(
            normalize_key_inner(true, r"C:\Users\Me\x\s.jsonl"),
            normalize_key_inner(true, "C:/users/me/X/s.jsonl")
        );
    }

    #[test]
    fn normalize_path_key_strips_verbatim_prefix_on_windows() {
        // #197: a \\?\-prefixed path (what canonicalize returns) keys the same as
        // its plain form, instead of folding to a never-coalescing //?/c:/… .
        assert_eq!(
            normalize_key_inner(true, r"\\?\C:\Foo\Bar.jsonl"),
            normalize_key_inner(true, r"C:\Foo\Bar.jsonl")
        );
        assert_eq!(normalize_key_inner(true, r"\\?\C:\Foo"), "c:/foo");
        // \\?\UNC\server\share denotes \\server\share — they must coalesce.
        assert_eq!(
            normalize_key_inner(true, r"\\?\UNC\srv\share\s.jsonl"),
            normalize_key_inner(true, r"\\srv\share\s.jsonl")
        );
    }

    #[test]
    fn normalize_path_key_verbatim_prefix_is_inert_on_unix() {
        // On Unix `\\?\` is just ordinary filename bytes — no stripping or folding.
        assert_eq!(normalize_key_inner(false, r"\\?\C:\Foo"), r"\\?\C:\Foo");
    }
}
