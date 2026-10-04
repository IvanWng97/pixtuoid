//! The model names no rasterizer: outside `pixel_painter/` and `cutaway/`, no
//! source file paths into either, but for the edges [`KNOWN_EDGES`] lists.

use std::collections::BTreeMap;
use std::path::Path;

const RASTERIZERS: [&str; 2] = ["pixel_painter", "cutaway"];

/// Each model file or directory still naming a rasterizer, how many lines do,
/// and the PR that removes the edge. A fix deletes its entry; a new edge under a
/// listed path changes its count.
const KNOWN_EDGES: &[(&str, usize, &str)] = &[
    (
        "display/",
        27,
        "pen, text, light and effects move model-side: #1253",
    ),
    (
        "floor/",
        7,
        "render_to_rgb_buffer, BaseFillCache and the both-painters tests: #1244; AgentFrame: #1253",
    ),
    (
        "overlay.rs",
        6,
        "cutaway::text widths and AgentFrame: #1253",
    ),
    ("wall.rs", 1, "cutaway::pen: #1253"),
];

/// Whether `code` names a rasterizer as a path segment; inside a `use`
/// statement every segment is a path, so a bare name counts.
fn names_a_rasterizer(code: &str, in_use: bool) -> bool {
    RASTERIZERS.iter().any(|name| {
        code.match_indices(name).any(|(at, _)| {
            let (before, after) = (&code[..at], &code[at + name.len()..]);
            let bounded = !before.ends_with(|c: char| c.is_alphanumeric() || c == '_')
                && !after.starts_with(|c: char| c.is_alphanumeric() || c == '_');
            bounded && (in_use || before.ends_with("::") || after.starts_with("::"))
        })
    })
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
        let mut in_use = false;
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            let trimmed = code.trim_start();
            in_use |= trimmed.starts_with("use ") || trimmed.contains(" use ");
            let named = names_a_rasterizer(code, in_use);
            in_use &= !code.contains(';');
            if !named {
                continue;
            }
            match KNOWN_EDGES.iter().find(|(at, ..)| rel.starts_with(at)) {
                Some((at, ..)) => *met.entry(*at).or_default() += 1,
                None => stray.push(format!("{rel}:{}: {}", n + 1, line.trim())),
            }
        }
    }
    assert_eq!(stray, Vec::<String>::new(), "the model names a rasterizer");
    let listed: BTreeMap<&str, usize> = KNOWN_EDGES.iter().map(|&(at, n, _)| (at, n)).collect();
    assert_eq!(
        met, listed,
        "a listed edge's line count moved: a fix lowers it (to 0: delete the entry), a new edge needs its own review"
    );
}

#[test]
fn a_rasterizer_path_is_named_but_its_word_is_not() {
    for named in [
        "use crate::cutaway::pen::Pen;",
        "crate::pixel_painter::AgentFrame",
        "use super::super::cutaway::text;",
    ] {
        assert!(names_a_rasterizer(named, false), "{named}");
    }
    assert!(
        names_a_rasterizer("    cutaway,", true),
        "a bare name in a use group"
    );
    for unnamed in [
        "let cutaway = classic;",
        "fn cutaway_snapshot() {}",
        "Plan::Cutaway { tmux }",
    ] {
        assert!(!names_a_rasterizer(unnamed, false), "{unnamed}");
    }
}
