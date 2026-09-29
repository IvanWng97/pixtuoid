//! Drop the comments from the bundled pack's sources before `build.rs` embeds
//! them: they document the files in the repository, and every build would
//! otherwise carry them as dead bytes, the web office included, whose size
//! `just gen-wasm-check` caps.

/// `src`, a `.sprite` file or `pack.toml`, without its comments. A line that
/// held only a comment stays, empty, so a parse error's line number is still
/// the source's. A `#` inside a one-line quoted string (a palette colour) is not
/// a comment.
///
/// # Panics
///
/// On a TOML multi-line string: the scan reads one line at a time, so it would
/// cut a `#` inside one.
pub(crate) fn strip_comments(src: &str) -> String {
    assert!(
        !src.contains("\"\"\"") && !src.contains("'''"),
        "strip_comments cannot read a multi-line string"
    );
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        out.push_str(line[..comment_start(line)].trim_end());
        out.push('\n');
    }
    out
}

/// Where `line`'s comment starts, or its length where it has none.
fn comment_start(line: &str) -> usize {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => return i,
            _ => {}
        }
        escaped = false;
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::strip_comments;

    #[test]
    fn comments_go_their_lines_stay_and_quoted_hashes_stay() {
        let src = "# a header\n\n@frame 0\nH H  # the fringe\n\"+\" = \"#2a2e3c\"   # facade\nk = 'a # b'\n";
        assert_eq!(
            strip_comments(src),
            "\n\n@frame 0\nH H\n\"+\" = \"#2a2e3c\"\nk = 'a # b'\n"
        );
    }

    #[test]
    fn only_a_basic_string_escapes_its_quote() {
        assert_eq!(
            strip_comments("k = \"a \\\" # b\" # c\n"),
            "k = \"a \\\" # b\"\n"
        );
        assert_eq!(strip_comments("k = 'a\\' # b\n"), "k = 'a\\'\n");
    }

    #[test]
    #[should_panic(expected = "multi-line string")]
    fn a_multi_line_string_is_refused() {
        strip_comments("k = \"\"\"\na # b\n\"\"\"\n");
    }
}
