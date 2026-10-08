//! Every character the office can write is one the scene's pixel font draws.
//! Read from the literals of everything that renders text, parsed so a
//! comment, a doc, a pattern or a test never counts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// The source trees whose literals reach the screen, core's decoded labels
/// and details among them.
const RENDERERS: [&str; 4] = [
    "../pixtuoid-core/src",
    "../pixtuoid-scene/src",
    "src/tui",
    "src/floating",
];

/// The characters of a file's string and char literals outside tests,
/// attributes and patterns, and the test modules it declares out of line.
#[derive(Default)]
struct Written {
    chars: Vec<char>,
    test_mods: Vec<String>,
}

impl Written {
    fn of(source: &str) -> Self {
        let file = syn::parse_file(source).expect("source parses");
        let mut found = Self::default();
        found.visit_file(&file);
        found
    }

    /// A macro's arguments are tokens, not syntax: each literal among them
    /// counts.
    fn scan_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::TokenTree;
        for tree in tokens {
            match tree {
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                TokenTree::Literal(lit) => {
                    let text = lit.to_string();
                    if let Ok(s) = syn::parse_str::<syn::LitStr>(&text) {
                        self.chars.extend(s.value().chars());
                    } else if let Ok(c) = syn::parse_str::<syn::LitChar>(&text) {
                        self.chars.push(c.value());
                    }
                }
                _ => {}
            }
        }
    }
}

/// Whether an item compiles only under test: `#[test]`, or a `cfg` that holds
/// only when `test` does (`test`, or `all(…)` naming it).
fn is_test(attrs: &[syn::Attribute]) -> bool {
    fn needs_test(meta: &syn::Meta) -> bool {
        match meta {
            syn::Meta::Path(p) => p.is_ident("test"),
            syn::Meta::List(l) if l.path.is_ident("all") => l
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .is_ok_and(|all| all.iter().any(needs_test)),
            _ => false,
        }
    }
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && a.parse_args::<syn::Meta>().is_ok_and(|m| needs_test(&m)))
    })
}

impl<'ast> Visit<'ast> for Written {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if !is_test(&m.attrs) {
            syn::visit::visit_item_mod(self, m);
        } else if m.content.is_none() {
            self.test_mods.push(m.ident.to_string());
        }
    }

    fn visit_pat(&mut self, _: &'ast syn::Pat) {}

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !is_test(&f.attrs) {
            syn::visit::visit_item_fn(self, f);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if !is_test(&i.attrs) {
            syn::visit::visit_item_impl(self, i);
        }
    }

    fn visit_lit_str(&mut self, s: &'ast syn::LitStr) {
        self.chars.extend(s.value().chars());
    }

    fn visit_lit_char(&mut self, c: &'ast syn::LitChar) {
        self.chars.push(c.value());
    }

    /// A `matches!`'s pattern is no text: only its scrutinee counts.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.is_ident("matches") {
            let scrutinee = mac.tokens.clone().into_iter().take_while(
                |t| !matches!(t, proc_macro2::TokenTree::Punct(p) if p.as_char() == ','),
            );
            self.scan_tokens(scrutinee.collect());
        } else {
            self.scan_tokens(mac.tokens.clone());
        }
    }
}

/// Where `file`'s out-of-line `mod name;` lives, as rustc resolves it: beside
/// a `mod.rs` or `lib.rs`, else in the directory named for the file.
fn module_dir(file: &Path) -> PathBuf {
    let dir = file.parent().expect("a file has a directory");
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("mod" | "lib" | "main") => dir.to_path_buf(),
        Some(stem) => dir.join(stem),
        None => dir.to_path_buf(),
    }
}

/// Whether a written `c` reaches a glyph: one that takes no cell (a control,
/// a bidi mark) draws nothing, and a private-use code point means nothing
/// outside a private agreement (Unicode §23.5), here a painter's in-band
/// marker.
fn drawn(c: char) -> bool {
    let private_use = matches!(c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{10ffff}');
    pixtuoid_scene::display::text::cells(c.encode_utf8(&mut [0; 4])) > 0 && !private_use
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
fn the_pixel_font_draws_every_character_the_office_writes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for tree in RENDERERS {
        rust_files(&root.join(tree), &mut files);
    }
    let written: Vec<(PathBuf, Written)> = files
        .into_iter()
        .map(|file| {
            let source = std::fs::read_to_string(&file).expect("source reads");
            let found = Written::of(&source);
            (file, found)
        })
        .collect();
    let tests: Vec<PathBuf> = written
        .iter()
        .flat_map(|(file, found)| {
            let dir = module_dir(file);
            found.test_mods.iter().map(move |name| dir.join(name))
        })
        .collect();
    let in_tests = |file: &Path| {
        tests
            .iter()
            .any(|t| file.with_extension("") == *t || file.starts_with(t))
    };
    let mut tofu = BTreeMap::<char, Vec<String>>::new();
    for (file, found) in &written {
        if in_tests(file) {
            continue;
        }
        for &c in &found.chars {
            if drawn(c) && !pixtuoid_scene::cutaway::draws(c) {
                let at = file
                    .strip_prefix(root)
                    .unwrap_or(file)
                    .display()
                    .to_string();
                let seen = tofu.entry(c).or_default();
                if !seen.contains(&at) {
                    seen.push(at);
                }
            }
        }
    }
    assert!(
        tofu.is_empty(),
        "the pixel font draws these as tofu: {:#?}",
        tofu.iter()
            .map(|(c, at)| (format!("{c} U+{:04X}", u32::from(*c)), at))
            .collect::<Vec<_>>()
    );
}
