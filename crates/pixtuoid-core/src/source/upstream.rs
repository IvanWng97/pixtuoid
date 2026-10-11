//! What a source depends on UPSTREAM, declared beside the names it reads: the
//! documents that declare those names, how to read them, and which change to
//! one is a finding, with what it costs a user or what a maintainer
//! re-verifies. pixtuoid's `drift_watch` checks them against upstream.

/// Whether a source is watched upstream, and through what.
#[derive(Debug)]
pub enum Upstream {
    /// One slice of watches per upstream project the source reads, its own
    /// first. Empty means not yet moved off `check_upstream_drift.py`: the
    /// census that every row is watched or closed lands with the last move.
    Watched(&'static [&'static [Watch]]),
    /// Nothing upstream publishes declares what the source reads.
    Closed { why: &'static str },
}

impl Upstream {
    /// Every watch the source declares; none when it is closed.
    pub fn watches(&self) -> impl Iterator<Item = &'static Watch> + use<> {
        let projects: &'static [&'static [Watch]] = match *self {
            Self::Watched(projects) => projects,
            Self::Closed { .. } => &[],
        };
        projects.iter().flat_map(|watches| watches.iter())
    }
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
    /// A file of public repository `repo`'s tree at `branch`.
    Path {
        repo: &'static str,
        branch: &'static str,
        path: &'static str,
    },
    /// `member` of `repo`'s latest release tarball `asset` (`{version}` = its
    /// tag, `v`-stripped), admitted only when the release's `sums` file vouches
    /// for it.
    ReleaseAsset {
        repo: &'static str,
        asset: &'static str,
        member: &'static str,
        sums: &'static str,
    },
    /// A page a closed-source CLI publishes as its contract.
    Url(&'static str),
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
    /// Enum `name`'s serde wire names: each top-level variant's `rename`, else
    /// its ident in snake_case. Read only under `rename_all = "snake_case"`,
    /// the one spelling it derives.
    RustEnum { name: &'static str },
    /// The fields of the ONE `export interface name`.
    TsFields { name: &'static str },
    /// JSON Schema: each member of `def`'s `oneOf`/`anyOf` union, as the
    /// const of its `prop` property, or its own const when `prop` is `None`.
    JsonUnion {
        def: &'static str,
        prop: Option<&'static str>,
    },
    /// JSON Schema: every `properties` key, at any depth.
    JsonKeys,
}

/// Names we read, or wait on, whose vanishing or appearing upstream is a
/// finding.
#[derive(Debug)]
pub struct Watch {
    /// The report's key for this watch; unique across sources.
    pub id: &'static str,
    pub ours: fn() -> Vec<&'static str>,
    /// Read as ONE document, since a name may be declared in any of them: all
    /// must fetch, or none is checked.
    pub docs: &'static [Doc],
    pub read: Read,
    pub when: When,
    pub finding: Finding,
    pub siblings: Option<Siblings>,
}

/// Which change to one of a watch's names is the finding.
#[derive(Debug)]
pub enum When {
    /// Upstream stops declaring it.
    Vanishes,
    /// Upstream starts declaring it: a premise we rely on broke, or a surface
    /// we wait for landed.
    Appears,
}

/// How hard a finding lands, with what it costs a user.
#[derive(Debug)]
pub enum Finding {
    /// Users are broken now.
    Breaking(&'static str),
    /// Nothing is known broken; a maintainer re-verifies.
    Review(&'static str),
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
    /// Shares its word before the first `_` with one of ours that has one.
    FirstWord,
    /// Shares its word after the last `_` with one of ours that has one.
    LastWord,
    /// Every name the reader yields, which reads only the family already.
    All,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::registry::REGISTRY;

    /// A closed source says why, since nothing upstream will.
    #[test]
    fn every_closed_source_says_why() {
        for d in REGISTRY {
            if let Upstream::Closed { why } = d.upstream {
                assert!(
                    !why.trim().is_empty(),
                    "{} is closed with no reason",
                    d.name
                );
            }
        }
    }

    /// Every watch is one a checker can run: a blank name, or an `Each` pattern
    /// without `{name}`, matches whatever upstream says; a `Sole` read yields
    /// one name; and a sibling sweep needs a vanish watch whose reader yields
    /// upstream's names, all of them unless the family is `All`, which only a
    /// reader confined to the family can mean.
    #[test]
    fn every_watch_is_one_a_checker_can_run() {
        let mut ids = std::collections::BTreeSet::new();
        for d in REGISTRY {
            for w in d.upstream.watches() {
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
                if let Some(sib) = &w.siblings {
                    assert!(
                        matches!(w.when, When::Vanishes),
                        "{} sweeps siblings on a watch that does not fire on a vanish",
                        w.id
                    );
                    assert!(
                        !matches!(w.read, Read::Each { .. } | Read::Sole { .. }),
                        "{} sweeps siblings off a reader that yields none",
                        w.id
                    );
                    assert!(
                        !matches!(sib.family, Family::All)
                            || matches!(w.read, Read::RustArms { .. }),
                        "{}: `Family::All` off a reader that yields more than the family",
                        w.id
                    );
                }
            }
        }
    }
}
