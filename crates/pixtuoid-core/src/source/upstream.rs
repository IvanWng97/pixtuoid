//! What a source depends on UPSTREAM, declared beside the names it reads: the
//! documents that declare those names, how to read them, and what a user loses
//! when one goes. pixtuoid's `drift_watch` checks them against upstream.

/// One upstream project a source reads names from.
#[derive(Debug)]
pub struct Upstream {
    pub project: Project,
    pub watches: &'static [Watch],
}

#[derive(Debug)]
pub enum Project {
    /// A public repository, which every [`At::Path`] resolves against.
    Repo {
        repo: &'static str,
        branch: &'static str,
    },
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

#[derive(Debug)]
pub enum At {
    Path(&'static str),
}

/// How a document yields upstream's names.
#[derive(Debug)]
pub enum Read {
    /// Each name must match `pattern`, where `{name}` stands for the name,
    /// regex-escaped.
    Each { pattern: &'static str },
    /// JSON Schema: each member of `def`'s `oneOf`/`anyOf` union, as the
    /// const of its `prop` property, or its own const when `prop` is `None`.
    JsonUnion {
        def: &'static str,
        prop: Option<&'static str>,
    },
}

/// A finding and how hard it lands.
#[derive(Debug)]
pub enum Finding {
    /// Users are broken now: the decoder needs fixing.
    Breaking(&'static str),
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
    /// What a user loses when upstream stops declaring one of `ours`.
    pub vanish: Finding,
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

#[derive(Debug)]
pub enum Family {
    /// Starts with this.
    Prefix(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::registry::REGISTRY;

    /// Every watch is one a checker can run: a blank name, or an `Each` pattern
    /// without `{name}`, matches whatever upstream says, and a sibling sweep
    /// needs a reader that yields upstream's names rather than testing ours.
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
                    w.siblings.is_none() || !matches!(w.read, Read::Each { .. }),
                    "{} sweeps siblings off a reader that yields none",
                    w.id
                );
            }
        }
    }
}
