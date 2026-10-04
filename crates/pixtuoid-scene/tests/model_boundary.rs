//! The model names no rasterizer: outside `pixel_painter/`, `cutaway/` and the
//! frame entry, no source file paths into either, but for the edges
//! [`KNOWN_EDGES`] lists.

use std::collections::BTreeMap;
use std::path::Path;

use syn::visit::Visit;

const RASTERIZERS: [&str; 2] = ["pixel_painter", "cutaway"];

/// The frame entry, `look::render`: it steps the model and runs a rasterizer
/// on the result, so it names both by design.
const ENTRY: &str = "look";

/// Each model file or directory still naming a rasterizer, how many paths do,
/// and the PR that removes the edge. A fix deletes its entry; a new edge under a
/// listed path changes its count.
const KNOWN_EDGES: &[(&str, usize, &str)] = &[];

fn is_rasterizer(ident: &syn::Ident) -> bool {
    RASTERIZERS.iter().any(|r| ident == r)
}

/// Every path in a parsed file that names a rasterizer as a segment, as text.
/// Parsing, not scanning, so a comment or a string literal never counts.
#[derive(Default)]
struct RasterizerPaths(Vec<String>);

impl RasterizerPaths {
    fn of(source: &str) -> Vec<String> {
        let file = syn::parse_file(source).expect("source parses");
        let mut found = Self::default();
        found.visit_file(&file);
        found.0
    }

    /// A macro's arguments are tokens, not syntax: an identifier counts where
    /// `::` joins it to a neighbour.
    fn scan_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::{Spacing, TokenTree};
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let path_sep = |at: Option<usize>| {
            let punct = |i: usize| match trees.get(i) {
                Some(TokenTree::Punct(p)) if p.as_char() == ':' => Some(p.spacing()),
                _ => None,
            };
            at.is_some_and(|i| punct(i) == Some(Spacing::Joint) && punct(i + 1).is_some())
        };
        for (i, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                TokenTree::Ident(id)
                    if is_rasterizer(id)
                        && (path_sep(Some(i + 1)) || path_sep(i.checked_sub(2))) =>
                {
                    self.0.push(format!("{id} in a macro"));
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for RasterizerPaths {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.iter().any(|s| is_rasterizer(&s.ident)) {
            let text: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            self.0.push(text.join("::"));
        }
        syn::visit::visit_path(self, path);
    }

    /// One edge per item a `use` imports from under a rasterizer, so an item
    /// added to an existing group moves the count.
    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        fn leaves(tree: &syn::UseTree, rooted: bool, out: &mut Vec<String>) {
            match tree {
                syn::UseTree::Path(p) => leaves(&p.tree, rooted || is_rasterizer(&p.ident), out),
                syn::UseTree::Name(syn::UseName { ident })
                | syn::UseTree::Rename(syn::UseRename { ident, .. }) => {
                    if rooted || is_rasterizer(ident) {
                        out.push(format!("use {ident}"));
                    }
                }
                syn::UseTree::Glob(_) => {
                    if rooted {
                        out.push("use *".into());
                    }
                }
                syn::UseTree::Group(g) => {
                    for item in &g.items {
                        leaves(item, rooted, out);
                    }
                }
            }
        }
        leaves(tree, false, &mut self.0);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.scan_tokens(mac.tokens.clone());
        syn::visit::visit_macro(self, mac);
    }
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src reads") {
        let path = entry.expect("entry reads").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_model_names_no_rasterizer() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let (mut met, mut stray) = (BTreeMap::<&str, usize>::new(), Vec::new());
    for file in files {
        let rel: Vec<String> = file
            .strip_prefix(&src)
            .expect("under src")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let rel = rel.join("/");
        // lib.rs declares the modules; a rasterizer may name itself.
        if rel == "lib.rs"
            || rel == format!("{ENTRY}.rs")
            || RASTERIZERS
                .iter()
                .chain([&ENTRY])
                .any(|r| rel.starts_with(&format!("{r}/")))
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("source reads");
        for path in RasterizerPaths::of(&text) {
            match KNOWN_EDGES.iter().find(|(at, ..)| rel.starts_with(at)) {
                Some((at, ..)) => *met.entry(*at).or_default() += 1,
                None => stray.push(format!("{rel}: {path}")),
            }
        }
    }
    assert_eq!(stray, Vec::<String>::new(), "the model names a rasterizer");
    let listed: BTreeMap<&str, usize> = KNOWN_EDGES.iter().map(|&(at, n, _)| (at, n)).collect();
    assert_eq!(
        met, listed,
        "a listed edge's path count moved: a fix lowers it (to 0: delete the entry), a new edge needs its own review"
    );
}

#[test]
fn a_rasterizer_path_is_named_but_its_word_is_not() {
    for named in [
        "use crate::cutaway::pen::Pen;",
        "fn f() -> crate::pixel_painter::AgentFrame { todo!() }",
        "use super::super::cutaway::text;",
        "use crate::{\n    cutaway,\n};",
        "fn f() { assert!(crate::cutaway::text::LINE_H > 0); }",
        "fn f() { m!(S { x: crate::cutaway::pen::Pen }); }",
        r#"fn f() { let _ = ("http://x", crate::pixel_painter::AgentFrame::default()); }"#,
    ] {
        assert!(!RasterizerPaths::of(named).is_empty(), "{named}");
    }
    for unnamed in [
        "fn f() { let cutaway = classic; }",
        "fn cutaway_snapshot() {}",
        "fn f() { Plan::Cutaway { tmux }; }",
        r#"fn f() { let _ = "crate::cutaway::pen"; }"#,
        "/* crate::cutaway::pen */ fn f() {}",
        "/// [`crate::cutaway::pen`]\nfn f() {}",
        "fn f() { m!(S { cutaway: 1 }); }",
    ] {
        assert!(RasterizerPaths::of(unnamed).is_empty(), "{unnamed}");
    }
    assert_eq!(
        RasterizerPaths::of("use crate::pixel_painter::{a, b as c, d::*};").len(),
        3,
        "each item a rasterizer group imports is its own edge"
    );
}

/// What the cutaway resolves through its display list rather than reading:
/// the theme's tones and the layout.
const MODEL_INPUTS: [&str; 2] = ["Theme", "SceneLayout"];

/// The cutaway's frame entries, as `(file, fn)`: each composes a list from the
/// theme and the layout, then paints only that list.
const CUTAWAY_ENTRIES: [(&str, &str); 2] = [("canvas.rs", "frame"), ("paint.rs", "render_cutaway")];

/// Every read of a [`MODEL_INPUTS`] in a parsed file, outside its tests and
/// `entries`: a path through one, or a `theme`/`layout` method or field.
struct ModelReads<'e> {
    entries: &'e [&'e str],
    found: Vec<String>,
}

impl<'e> ModelReads<'e> {
    fn of(source: &str, entries: &'e [&'e str]) -> Vec<String> {
        let file = syn::parse_file(source).expect("source parses");
        let mut reads = Self {
            entries,
            found: Vec::new(),
        };
        reads.visit_file(&file);
        reads.found
    }
}

/// Whether `attrs` hold a `#[cfg(...)]` that needs `test`: one naming it
/// outside a `not(...)`.
fn is_test(attrs: &[syn::Attribute]) -> bool {
    fn needs_test(tokens: proc_macro2::TokenStream) -> bool {
        use proc_macro2::TokenTree;
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        trees.iter().enumerate().any(|(i, t)| match t {
            TokenTree::Ident(id) => id == "test",
            TokenTree::Group(g) => {
                let negated = i
                    .checked_sub(1)
                    .and_then(|j| trees.get(j))
                    .is_some_and(|p| matches!(p, TokenTree::Ident(id) if id == "not"));
                !negated && needs_test(g.stream())
            }
            _ => false,
        })
    }
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.meta
                .require_list()
                .is_ok_and(|l| needs_test(l.tokens.clone()))
    })
}

impl<'ast> Visit<'ast> for ModelReads<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = match item {
            syn::Item::Fn(f) if self.entries.iter().any(|e| f.sig.ident == e) => return,
            syn::Item::Fn(f) => &f.attrs,
            syn::Item::Mod(m) => &m.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Use(u) => &u.attrs,
            syn::Item::Struct(s) => &s.attrs,
            syn::Item::Enum(e) => &e.attrs,
            syn::Item::Const(c) => &c.attrs,
            syn::Item::Static(s) => &s.attrs,
            _ => return syn::visit::visit_item(self, item),
        };
        if !is_test(attrs) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_test(&f.attrs) && !self.entries.iter().any(|e| f.sig.ident == e) {
            syn::visit::visit_impl_item_fn(self, f);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let named = path.segments.iter().any(|s| {
            MODEL_INPUTS.iter().any(|m| s.ident == m)
                || (s.ident == "theme" && path.segments.len() > 1)
        });
        if named {
            let text: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            self.found.push(text.join("::"));
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        match tree {
            syn::UseTree::Name(syn::UseName { ident })
            | syn::UseTree::Rename(syn::UseRename { ident, .. })
            | syn::UseTree::Path(syn::UsePath { ident, .. })
                if MODEL_INPUTS.iter().any(|m| ident == m) || ident == "theme" =>
            {
                self.found.push(format!("use {ident}"));
            }
            _ => syn::visit::visit_use_tree(self, tree),
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "theme" || call.method == "layout" {
            self.found.push(format!(".{}()", call.method));
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_field(&mut self, field: &'ast syn::ExprField) {
        if let syn::Member::Named(name) = &field.member
            && (name == "theme" || name == "layout")
        {
            self.found.push(format!(".{name}"));
        }
        syn::visit::visit_expr_field(self, field);
    }
}

/// The cutaway draws only its display list: outside its tests and its frame
/// entries ([`CUTAWAY_ENTRIES`]), nothing under `cutaway/` reads the theme or
/// the layout, which the list has already resolved.
#[test]
fn the_cutaway_draws_only_its_list() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cutaway");
    let mut files = Vec::new();
    rust_files(&dir, &mut files);
    assert!(!files.is_empty(), "the cutaway has sources");
    let mut reads = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .expect("a file")
            .to_string_lossy()
            .into_owned();
        let entries: Vec<&str> = CUTAWAY_ENTRIES
            .iter()
            .filter(|(f, _)| *f == name)
            .map(|&(_, e)| e)
            .collect();
        let text = std::fs::read_to_string(&file).expect("source reads");
        reads.extend(
            ModelReads::of(&text, &entries)
                .into_iter()
                .map(|r| format!("{name}: {r}")),
        );
    }
    assert_eq!(reads, Vec::<String>::new(), "the cutaway reads the model");
}

#[test]
fn a_model_read_is_named_but_a_test_or_an_entry_is_not() {
    for read in [
        "fn f(t: &crate::theme::Theme) {}",
        "use crate::theme::Theme;",
        "fn f(l: &SceneLayout) {}",
        "fn f() { let c = crate::theme::NORMAL.surface.wall; }",
        "fn f(list: &L) { let t = list.theme(); }",
        "fn f(o: &O) { let w = o.layout.buf_w; }",
        "impl P { fn g(&self, t: &Theme) {} }",
        "#[cfg(not(test))]\nfn f(t: &Theme) {}",
    ] {
        assert!(!ModelReads::of(read, &[]).is_empty(), "{read}");
    }
    for unread in [
        "#[cfg(test)]\nmod tests { use crate::theme::Theme; }",
        "#[cfg(test)]\nfn f(t: &Theme) {}",
        "#[cfg(all(test, feature = \"x\"))]\nfn f(t: &Theme) {}",
        "fn entry(t: &Theme) {}",
        "impl P { fn entry(&self, l: &SceneLayout) {} }",
        "fn f() { let theme_free = 1; }",
        "/// [`crate::theme::Theme`]\nfn f() {}",
    ] {
        assert!(ModelReads::of(unread, &["entry"]).is_empty(), "{unread}");
    }
}
