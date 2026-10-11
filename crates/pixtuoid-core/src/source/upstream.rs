//! What a source depends on UPSTREAM, declared beside the names it reads: the
//! documents that declare those names, how to read them, and what a user loses
//! when one goes. pixtuoid's `drift_watch` checks them against upstream.

/// One public repository a source reads names from, which every [`At`]
/// resolves against.
#[derive(Debug)]
pub struct Upstream {
    pub repo: &'static str,
    pub watches: &'static [Watch],
}

/// A document upstream declares names in.
#[derive(Debug)]
pub struct Doc {
    pub at: At,
    /// Matches the declaration that OWNS the names, never one of them: a name
    /// as its own anchor passes vacuously, and a fetch landing on a re-export
    /// facade then reads as a rename (#793).
    pub anchor: &'static str,
    /// What `anchor` matches, as the report names it.
    pub owns: &'static str,
}

/// Where a [`Doc`] is fetched from.
#[derive(Debug)]
pub enum At {
    /// A file of the tree at `branch`.
    Path {
        path: &'static str,
        branch: &'static str,
    },
    /// `member` of the latest release's tarball `asset` (`{version}` = its
    /// tag, `v`-stripped), admitted only when the release's `sums` file vouches
    /// for it.
    ReleaseAsset {
        asset: &'static str,
        member: &'static str,
        sums: &'static str,
    },
}

/// How a document yields upstream's names.
#[derive(Debug)]
pub enum Read {
    /// Each name must match `pattern`, where `{name}` stands for the name,
    /// regex-escaped.
    Each { pattern: &'static str },
    /// The first group of the ONE match of `pattern`: a second declaration
    /// (a `#[cfg(test)]` twin, a macro body) is as unreadable as none.
    Sole { pattern: &'static str },
    /// The string patterns of `fn name`'s match arms whose result matches `arm`.
    RustArms {
        name: &'static str,
        arm: &'static str,
    },
    /// JSON Schema: each member of `def`'s `oneOf`/`anyOf` union, as the
    /// const of its `prop` property, or its own const when `prop` is `None`.
    JsonUnion {
        def: &'static str,
        prop: Option<&'static str>,
    },
    /// JSON Schema: every `properties` key, at any depth.
    JsonKeys,
}

/// Names we READ that upstream must keep declaring.
#[derive(Debug)]
pub struct Watch {
    /// The report's key for this watch; unique across sources.
    pub id: &'static str,
    pub ours: fn() -> Vec<&'static str>,
    /// Read as ONE document, since a name may be declared in any of them: all
    /// must fetch, or none is checked.
    pub docs: &'static [Doc],
    pub read: Read,
    /// What a user loses when upstream stops declaring one of `ours`: a
    /// vanish is breaking, since users are broken now.
    pub vanish: &'static str,
    pub siblings: Option<Siblings>,
}

/// Upstream names beside ours, in a family we already read, that no decoder
/// reads: a sibling (`custom_tool_call` beside `function_call`, #933) is where
/// a missed decode hides, while mere growth is not.
#[derive(Debug)]
pub struct Siblings {
    pub family: Family,
    /// Siblings deliberately not decoded, each with its reason. A row whose
    /// name upstream dropped is reported: it would silently exempt a reuse.
    pub ignored: &'static [(&'static str, &'static str)],
}

/// Which of upstream's names are siblings of ours.
#[derive(Debug)]
pub enum Family {
    /// Starts with this.
    Prefix(&'static str),
    /// Every name the reader yields: it reads only the family already.
    All,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::registry::REGISTRY;

    /// Every watch is one a checker can run: a blank name, or an `Each` pattern
    /// without `{name}`, matches whatever upstream says, a `Sole` read yields
    /// one name, and a sibling sweep needs a reader that yields upstream's
    /// names rather than testing ours.
    #[test]
    fn every_watch_is_one_a_checker_can_run() {
        let mut ids = std::collections::BTreeSet::new();
        for d in REGISTRY {
            for w in d.upstream.iter().flat_map(|up| up.watches) {
                assert!(ids.insert(w.id), "watch id {} is declared twice", w.id);
                let ours = (w.ours)();
                assert!(
                    !ours.is_empty() && ours.iter().all(|n| !n.is_empty()),
                    "{} watches an empty set or a blank name",
                    w.id
                );
                assert!(!w.docs.is_empty(), "{} reads no document", w.id);
                if let Read::Each { pattern } = w.read {
                    assert!(
                        pattern.contains("{name}"),
                        "{}: `{pattern}` names no name",
                        w.id
                    );
                }
                assert!(
                    !matches!(w.read, Read::Sole { .. }) || ours.len() == 1,
                    "{} reads one name off upstream but watches {}",
                    w.id,
                    ours.len()
                );
                assert!(
                    w.siblings.is_none()
                        || !matches!(w.read, Read::Each { .. } | Read::Sole { .. }),
                    "{} sweeps siblings off a reader that yields none",
                    w.id
                );
            }
        }
    }
}
