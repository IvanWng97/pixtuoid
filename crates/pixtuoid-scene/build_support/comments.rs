//! Drop the comments from the bundled pack's sources before `build.rs` embeds
//! them: they document the files in the repository, and every build, the size-
//! capped web office included, would otherwise carry them as dead bytes.

/// `src`, a `.sprite` file or `pack.toml`, without its comments and blank
/// lines. A `#` inside a quoted string (a palette colour) is not a comment.
pub(crate) fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let code = line[..comment_start(line)].trim_end();
        if !code.trim_start().is_empty() {
            out.push_str(code);
            out.push('\n');
        }
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
    fn comments_and_blank_lines_go_and_quoted_hashes_stay() {
        let src = "# a header\n\n@frame 0\nH H  # the fringe\n\"+\" = \"#2a2e3c\"   # facade\nk = 'a # b'\n";
        assert_eq!(
            strip_comments(src),
            "@frame 0\nH H\n\"+\" = \"#2a2e3c\"\nk = 'a # b'\n"
        );
    }

    #[test]
    fn an_escaped_quote_does_not_end_a_string() {
        assert_eq!(
            strip_comments("k = \"a \\\" # b\" # c\n"),
            "k = \"a \\\" # b\"\n"
        );
    }
}
