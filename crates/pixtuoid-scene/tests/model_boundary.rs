//! The model names no rasterizer: outside `pixel_painter/` and `cutaway/`, no
//! source file paths into either, but for the edges [`KNOWN_EDGES`] lists.

use std::collections::BTreeMap;
use std::path::Path;

use syn::visit::Visit;

const RASTERIZERS: [&str; 2] = ["pixel_painter", "cutaway"];

/// Each model file or directory still naming a rasterizer, how many paths do,
/// and the PR that removes the edge. A fix deletes its entry; a new edge under a
/// listed path changes its count.
const KNOWN_EDGES: &[(&str, usize, &str)] = &[
    ("display/", 27, "pen: #1253; text, light and effects: #1270"),
    (
        "floor/",
        7,
        "render_to_rgb_buffer, BaseFillCache and the both-painters tests: #1244; AgentFrame: #1270",
    ),
    (
        "overlay.rs",
        6,
        "cutaway::text widths and AgentFrame: #1270",
    ),
    ("wall.rs", 1, "cutaway::pen: #1253"),
];

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
        use proc_macro2::TokenTree;
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let colon =
            |t: Option<&TokenTree>| matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == ':');
        for (i, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                TokenTree::Ident(id) if is_rasterizer(id) => {
                    let after = colon(trees.get(i + 1));
                    let before = i > 0 && colon(trees.get(i - 1));
                    if after || before {
                        self.0.push(format!("{id} in a macro"));
                    }
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

    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        let named = match tree {
            syn::UseTree::Path(p) => Some(&p.ident),
            syn::UseTree::Name(n) => Some(&n.ident),
            syn::UseTree::Rename(r) => Some(&r.ident),
            syn::UseTree::Glob(_) | syn::UseTree::Group(_) => None,
        };
        if let Some(id) = named.filter(|id| is_rasterizer(id)) {
            self.0.push(format!("use {id}"));
        }
        syn::visit::visit_use_tree(self, tree);
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
            || RASTERIZERS
                .iter()
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
    ] {
        assert!(RasterizerPaths::of(unnamed).is_empty(), "{unnamed}");
    }
}
