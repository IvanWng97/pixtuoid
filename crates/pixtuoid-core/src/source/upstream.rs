//! What a source depends on UPSTREAM, declared beside the names it reads: the
//! documents that declare those names, how to read them, and which change to
//! one is a finding, with what it costs a user or what a maintainer
//! re-verifies. pixtuoid's `drift_watch` checks them against upstream.

/// Whether a source is watched upstream, and through what.
#[derive(Debug)]
pub enum Upstream {
    /// One slice of watches per upstream project the source reads. Empty
    /// means not yet moved off `check_upstream_drift.py`: the
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
    /// A name is declared where `pattern` matches, `{name}` standing for it,
    /// regex-escaped.
    Each { pattern: &'static str },
    /// The first group of the ONE match of `pattern` in the plain text: a
    /// second declaration (a `#[cfg(test)]` twin, a macro body) is as
    /// unreadable as none.
    Sole { pattern: &'static str },
    /// The string literal the ONE Rust `item` evaluates to: a const by name,
    /// or a method as `<Type as Trait>::method`, its body's tail.
    RustStr { item: &'static str },
    /// The string patterns of `fn name`'s match arms whose result matches `arm`.
    RustArms {
        name: &'static str,
        arm: &'static str,
    },
    /// Enum `name`'s serde wire names: each top-level variant's `rename`, else
    /// its ident in snake_case. Read only under `rename_all = "snake_case"`,
    /// the one spelling it derives.
    RustEnum { name: &'static str },
    /// The serde wire names of the ONE `struct name`'s fields.
    RustStruct { name: &'static str },
    /// The top-level fields of the ONE `interface name`.
    TsFields { name: &'static str },
    /// Every first group of `pattern`: `Each` turned round, so the floor can
    /// tell a degraded parse from a rename.
    Matches { pattern: &'static str },
    /// The backticked first-column names of the first Markdown table in the
    /// section of the ONE heading `under` whose header row begins with
    /// `header`; other tables repeat names in other columns.
    MdTable {
        header: &'static str,
        under: &'static str,
    },
    /// The one-word titles of the direct subsections of the ONE heading
    /// `under`; a page's other sections carry one-word headings of their own.
    MdHeadings { under: &'static str },
    /// The string members of the `{…}` opened after the ONE match of `decl`,
    /// comments first.
    PySet { decl: &'static str },
    /// The row headers of the ONE `name! { … }` invocation.
    RustMacroRows { name: &'static str },
    /// The string elements of the ONE `const name = [ … ]`.
    TsConstArray { name: &'static str },
    /// JSON Schema: each member of `def`'s `oneOf`/`anyOf` union, as the
    /// const of its `prop` property, or its own const when `prop` is `None`.
    JsonUnion {
        def: &'static str,
        prop: Option<&'static str>,
    },
    /// JSON Schema: every `properties` key, at any depth.
    JsonKeys,
}

impl Read {
    /// Whether it yields only the family a watch reads, never the rest of
    /// upstream's declaration beside it.
    pub const fn confined_to_family(&self) -> bool {
        matches!(self, Self::RustArms { .. })
    }
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

impl Watch {
    /// Why a checker cannot run this watch, if it cannot: a blank name, or an
    /// `Each` pattern without `{name}`, matches whatever upstream says; a `Sole`
    /// read yields one name; and a sibling sweep needs a vanish watch whose
    /// reader yields upstream's names, all of them unless the family is `All`,
    /// which only a reader confined to the family can mean.
    pub fn unrunnable(&self) -> Option<String> {
        let ours = (self.ours)();
        let why = if ours.is_empty() || ours.iter().any(|n| n.is_empty()) {
            "watches an empty set or a blank name".to_owned()
        } else if self.docs.is_empty() {
            "reads no document".to_owned()
        } else if matches!(self.read, Read::Each { pattern } if !pattern.contains("{name}")) {
            "has an `Each` pattern that names no name".to_owned()
        } else if matches!(self.read, Read::Sole { .. } | Read::RustStr { .. }) && ours.len() != 1 {
            format!("reads one name off upstream but watches {}", ours.len())
        } else if let Some(sib) = &self.siblings {
            if !matches!(self.when, When::Vanishes) {
                "sweeps siblings on a watch that does not fire on a vanish".to_owned()
            } else if matches!(
                self.read,
                Read::Each { .. } | Read::Sole { .. } | Read::RustStr { .. }
            ) {
                "sweeps siblings off a reader that yields none".to_owned()
            } else if matches!(sib.family, Family::All) && !self.read.confined_to_family() {
                "sweeps `Family::All` off a reader that yields more than the family".to_owned()
            } else {
                return None;
            }
        } else {
            return None;
        };
        Some(why)
    }
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

/// How hard a finding lands, with what it costs a user or what a maintainer
/// re-verifies.
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

    /// Each rule refuses the watch it exists for; the roster is held to them by
    /// the binary's `every_declared_watch_is_runnable_and_its_id_unique`.
    #[test]
    fn unrunnable_names_each_watch_a_checker_cannot_run() {
        const OK: Watch = Watch {
            id: "t.ok",
            ours: || vec!["a", "b"],
            docs: &[Doc {
                at: At::Url("https://example.com/x.md"),
                anchor: "x",
                owns: "x",
            }],
            read: Read::Each {
                pattern: r"\b{name}\b",
            },
            when: When::Vanishes,
            finding: Finding::Breaking("x"),
            siblings: None,
        };
        const ALL: Siblings = Siblings {
            family: Family::All,
            ignored: &[],
        };
        let arms = Read::RustArms {
            name: "f",
            arm: "x",
        };
        let cases = [
            (
                Watch {
                    ours: || vec![],
                    ..OK
                },
                "watches an empty set or a blank name",
            ),
            (
                Watch {
                    ours: || vec![""],
                    ..OK
                },
                "watches an empty set or a blank name",
            ),
            (Watch { docs: &[], ..OK }, "reads no document"),
            (
                Watch {
                    read: Read::Each { pattern: "x" },
                    ..OK
                },
                "has an `Each` pattern that names no name",
            ),
            (
                Watch {
                    read: Read::Sole { pattern: "(x)" },
                    ..OK
                },
                "reads one name off upstream but watches 2",
            ),
            (
                Watch {
                    read: arms,
                    when: When::Appears,
                    siblings: Some(ALL),
                    ..OK
                },
                "sweeps siblings on a watch that does not fire on a vanish",
            ),
            (
                Watch {
                    siblings: Some(ALL),
                    ..OK
                },
                "sweeps siblings off a reader that yields none",
            ),
            (
                Watch {
                    read: Read::MdTable {
                        header: "E",
                        under: "S",
                    },
                    siblings: Some(ALL),
                    ..OK
                },
                "sweeps `Family::All` off a reader that yields more than the family",
            ),
        ];
        assert_eq!(OK.unrunnable(), None);
        for (w, why) in cases {
            assert_eq!(w.unrunnable().as_deref(), Some(why));
        }
    }
}
