//! Asking the terminal what it can draw: the IO half of [`super`].
//!
//! The query, the parse and the environment's hints are separate here so the
//! decisions — which protocol, which cell — are pure and tested; only
//! [`probe`] touches the terminal.

use ratatui_image::picker::cap_parser::QueryStdioOptions;
#[cfg(any(unix, test))]
use ratatui_image::picker::cap_parser::{Parser, Response};
use ratatui_image::picker::ProtocolType;

use super::{CellSize, Detected, ImageProtocol, Probe};

/// What the environment says about the terminal, read once per probe.
///
/// Every rule on it mirrors ratatui-image 11.0.8's picker, cited per rule: the
/// cutaway draws through that crate's encoders, so what the plan expects a
/// terminal to take must be what those encoders were built for.
#[cfg(any(unix, test))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EnvHints {
    term: Option<String>,
    term_program: Option<String>,
    lc_terminal: Option<String>,
    wezterm: bool,
    konsole: bool,
    iterm_session: bool,
    tmux_client: bool,
}

#[cfg(any(unix, test))]
impl EnvHints {
    #[cfg(unix)]
    fn read() -> Self {
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        Self {
            term: var("TERM"),
            term_program: var("TERM_PROGRAM"),
            lc_terminal: var("LC_TERMINAL"),
            wezterm: var("WEZTERM_EXECUTABLE").is_some(),
            konsole: var("KONSOLE_VERSION").is_some(),
            iterm_session: var("ITERM_SESSION_ID").is_some(),
            tmux_client: var("TMUX").is_some(),
        }
    }

    fn tmux(&self) -> bool {
        in_tmux(self.term.as_deref(), self.term_program.as_deref())
    }

    /// Inside tmux AND a client of it (`$TMUX`): only then does a bare `tmux`
    /// answer for the pane this process runs in. Without `$TMUX` — a tmux `TERM`
    /// carried over ssh — it answers for whatever server this host runs.
    fn our_tmux_pane(&self) -> bool {
        self.tmux() && self.tmux_client
    }

    /// Protocols never asked for under WezTerm or Konsole: neither implements
    /// kitty's placeholders, Konsole's SIXEL is buggy, and WezTerm draws better
    /// through iTerm2 (`picker.rs:110-119`).
    fn blacklist(&self) -> Vec<ProtocolType> {
        if self.wezterm || self.konsole {
            vec![ProtocolType::Kitty, ProtocolType::Sixel]
        } else {
            Vec::new()
        }
    }

    /// iTerm2 inline images, which upstream's query does not ask about
    /// (`cap_parser.rs:107-108`), guessed from the terminal the environment
    /// names: inside tmux, the outer terminal's markers (`picker.rs:336-347`);
    /// anywhere, `TERM_PROGRAM`/`LC_TERMINAL` (`picker.rs:351-369`).
    fn iterm2(&self) -> Option<ImageProtocol> {
        const ITERM2_TERM_PROGRAMS: [&str; 9] = [
            "iTerm",
            "WezTerm",
            "mintty",
            "vscode",
            "Tabby",
            "Hyper",
            "rio",
            "Bobcat",
            "WarpTerminal",
        ];
        let outer = self.tmux() && (self.iterm_session || self.wezterm);
        let named = self
            .term_program
            .as_deref()
            .is_some_and(|p| ITERM2_TERM_PROGRAMS.iter().any(|t| p.contains(t)))
            || self
                .lc_terminal
                .as_deref()
                .is_some_and(|t| t.contains("iTerm"));
        (outer || named).then_some(ImageProtocol::Iterm2)
    }
}

/// Inside tmux: `TERM` starting `tmux`, or `TERM_PROGRAM` = `tmux` — the test
/// upstream applies before wrapping every image in passthrough (ratatui-image
/// 11.0.8 `picker.rs:320-326`), on every platform.
fn in_tmux(term: Option<&str>, term_program: Option<&str>) -> bool {
    term.is_some_and(|t| t.starts_with("tmux")) || term_program == Some("tmux")
}

/// What the terminal's answer and the environment together say: kitty over
/// SIXEL when it answers both (ratatui-image 11.0.8 `picker.rs:523-533`), then
/// the iTerm2 guess (`picker.rs:127-131`); the cell from its answer, else from
/// the kernel's window size.
#[cfg(any(unix, test))]
fn detected(responses: &[Response], env: &EnvHints, window_cell: Option<CellSize>) -> Detected {
    let queried = if responses.contains(&Response::Kitty) {
        Some(ImageProtocol::Kitty)
    } else if responses.contains(&Response::Sixel) {
        Some(ImageProtocol::Sixel)
    } else {
        None
    };
    let answered_cell = responses.iter().find_map(|r| match r {
        Response::CellSize(Some((w, h))) => Some(CellSize { w: *w, h: *h }),
        _ => None,
    });
    Detected {
        protocol: queried.or_else(|| env.iterm2()),
        cell: answered_cell.or(window_cell),
        tmux: env.tmux(),
    }
}

/// A terminal whose reply never completed is still the protocol the
/// environment names, as upstream falls back when its query goes unanswered
/// (ratatui-image 11.0.8 `picker.rs:147-157`); with nothing named, it is
/// [`Probe::NoAnswer`].
#[cfg(any(unix, test))]
fn unanswered(env: &EnvHints, window_cell: Option<CellSize>) -> Probe {
    let d = detected(&[], env, window_cell);
    if d.protocol.is_some() {
        Probe::Detected(d)
    } else {
        Probe::NoAnswer
    }
}

/// Feed one chunk of the reply to `parser`, collecting its responses; `true`
/// once the device-status reply that ends the query has arrived.
#[cfg(any(unix, test))]
fn take_reply(parser: &mut Parser, responses: &mut Vec<Response>, chunk: &[u8]) -> bool {
    for &byte in chunk {
        for response in parser.push(char::from(byte)) {
            if response == Response::Status {
                return true;
            }
            responses.push(response);
        }
    }
    false
}

/// The most reply bytes read before a terminal that never sends the status
/// reply is given up on.
#[cfg(unix)]
const MAX_REPLY_BYTES: usize = 4096;

/// Ask the terminal what it can do, when `ask`.
///
/// A failed query is a [`Probe`] outcome, not an error: every caller's fallback
/// is the classic profile, which is also what a terminal without graphics gets,
/// and a visualiser that refuses to start because it could not ask a question
/// would be worse than one that draws the plain office.
///
/// Through [`crate::term::query_tty`], not upstream's
/// `Picker::from_query_stdio`, whose detached reader outlives its timeout and
/// restores the mode only once a reply lands (ratatui-image 11.0.8
/// `picker.rs:584-622`); and read-only: inside tmux it reads
/// `allow-passthrough` where upstream turns it on (`picker.rs:328-334`).
#[cfg(unix)]
pub(crate) fn probe(ask: bool) -> Probe {
    if !ask {
        return Probe::NotQueried;
    }
    let env = EnvHints::read();
    if env.our_tmux_pane() && tmux_passthrough() == Some(false) {
        return Probe::TmuxPassthroughOff;
    }
    let query = Parser::query(
        env.tmux(),
        QueryStdioOptions {
            blacklist_protocols: env.blacklist(),
            ..QueryStdioOptions::default()
        },
    );
    let mut parser = Parser::new();
    let mut responses = Vec::new();
    match crate::term::query_tty(
        query.as_bytes(),
        super::GRAPHICS_PROBE_TIMEOUT,
        MAX_REPLY_BYTES,
        |chunk| take_reply(&mut parser, &mut responses, chunk),
    ) {
        None => Probe::NotQueried,
        Some(false) => unanswered(&env, window_cell()),
        Some(true) => Probe::Detected(detected(&responses, &env, window_cell())),
    }
}

/// Our pane's `allow-passthrough`, inherited value included (tmux(1)
/// `show-options -A`); `None` when tmux cannot say within
/// [`GRAPHICS_PROBE_TIMEOUT`](super::GRAPHICS_PROBE_TIMEOUT). Asked only where
/// [`EnvHints::our_tmux_pane`] holds.
#[cfg(unix)]
fn tmux_passthrough() -> Option<bool> {
    use std::process::{Command, Stdio};
    let out = crate::output_within(
        Command::new("tmux")
            .args(["show-options", "-Apv", "allow-passthrough"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
        super::GRAPHICS_PROBE_TIMEOUT,
    )?;
    if !out.status.success() {
        return None;
    }
    Some(matches!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "on" | "all"
    ))
}

/// A cell's size from the kernel's window size, for a terminal that reports no
/// cell size of its own.
#[cfg(unix)]
fn window_cell() -> Option<CellSize> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some(CellSize {
        w: size.width / size.columns,
        h: size.height / size.rows,
    })
}

/// Off Unix there is no controlling-terminal primitive yet, so the probe is
/// upstream's, detached reader and tmux write included. Upstream answers a
/// timeout with its fallback picker (ratatui-image 11.0.8 `picker.rs:147-157`),
/// which without a window size — never one on Windows (`picker.rs:453-456`) —
/// is halfblocks whatever the environment names, so a terminal that never
/// answers arrives as [`Probe::Detected`] with no protocol.
#[cfg(not(unix))]
pub(crate) fn probe(ask: bool) -> Probe {
    use ratatui_image::picker::Picker;

    if !ask {
        return Probe::NotQueried;
    }
    let options = QueryStdioOptions {
        timeout: super::GRAPHICS_PROBE_TIMEOUT,
        ..QueryStdioOptions::default()
    };
    let Ok(picker) = Picker::from_query_stdio_with_options(options) else {
        return Probe::NotQueried;
    };
    let font = picker.font_size();
    Probe::Detected(Detected {
        protocol: match picker.protocol_type() {
            ProtocolType::Kitty => Some(ImageProtocol::Kitty),
            ProtocolType::Sixel => Some(ImageProtocol::Sixel),
            ProtocolType::Iterm2 => Some(ImageProtocol::Iterm2),
            ProtocolType::Halfblocks => None,
        },
        cell: Some(CellSize {
            w: font.width,
            h: font.height,
        }),
        tmux: in_tmux(
            std::env::var("TERM").ok().as_deref(),
            std::env::var("TERM_PROGRAM").ok().as_deref(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(term: &str, term_program: &str) -> EnvHints {
        EnvHints {
            term: Some(term.to_string()).filter(|s| !s.is_empty()),
            term_program: Some(term_program.to_string()).filter(|s| !s.is_empty()),
            ..EnvHints::default()
        }
    }

    /// A reply as a kitty-and-SIXEL terminal sends it, run through upstream's
    /// own parser: the kitty acknowledgement, a DA1 with SIXEL (`4`), the cell
    /// size (`CSI 6 ; h ; w t`), and the device-status reply that ends it.
    const KITTY_SIXEL_REPLY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;22c\x1b[6;41;17t\x1b[0n";

    #[test]
    fn a_full_reply_ends_at_the_status_reply_and_keeps_what_came_before() {
        let (mut parser, mut responses) = (Parser::new(), Vec::new());
        assert!(take_reply(&mut parser, &mut responses, KITTY_SIXEL_REPLY));
        let d = detected(&responses, &EnvHints::default(), None);
        assert_eq!(d.protocol, Some(ImageProtocol::Kitty), "kitty over SIXEL");
        assert_eq!(d.cell, Some(CellSize { w: 17, h: 41 }));
    }

    /// A reply split across reads completes on the chunk that carries the
    /// status reply, not before.
    #[test]
    fn a_reply_split_across_reads_completes_on_its_last_chunk() {
        let (head, tail) = KITTY_SIXEL_REPLY.split_at(KITTY_SIXEL_REPLY.len() - 3);
        let (mut parser, mut responses) = (Parser::new(), Vec::new());
        assert!(!take_reply(&mut parser, &mut responses, head));
        assert!(take_reply(&mut parser, &mut responses, tail));
    }

    #[test]
    fn a_terminal_that_answers_without_a_cell_size_falls_back_to_the_window() {
        let window = Some(CellSize { w: 9, h: 18 });
        let d = detected(&[Response::Sixel], &EnvHints::default(), window);
        assert_eq!(d.protocol, Some(ImageProtocol::Sixel));
        assert_eq!(d.cell, window);
        assert_eq!(
            detected(&[Response::Sixel], &EnvHints::default(), None).cell,
            None
        );
    }

    /// iTerm2's images are guessed from the environment, never over an
    /// answered protocol.
    #[test]
    fn iterm2_is_guessed_from_the_terminal_the_environment_names() {
        assert_eq!(
            detected(&[], &env("xterm-256color", "iTerm.app"), None).protocol,
            Some(ImageProtocol::Iterm2)
        );
        assert_eq!(
            detected(&[Response::Kitty], &env("xterm-256color", "WezTerm"), None).protocol,
            Some(ImageProtocol::Kitty)
        );
        let lc = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..EnvHints::default()
        };
        assert_eq!(lc.iterm2(), Some(ImageProtocol::Iterm2));
        assert_eq!(env("xterm-ghostty", "ghostty").iterm2(), None);
    }

    /// Inside tmux the outer terminal's markers name iTerm2; outside they do
    /// not count.
    #[test]
    fn inside_tmux_the_outer_terminals_markers_name_iterm2() {
        let outer = EnvHints {
            iterm_session: true,
            ..env("tmux-256color", "tmux")
        };
        assert_eq!(outer.iterm2(), Some(ImageProtocol::Iterm2));
        let not_tmux = EnvHints {
            iterm_session: true,
            ..env("xterm-ghostty", "ghostty")
        };
        assert_eq!(not_tmux.iterm2(), None);
    }

    /// A silent terminal is still the protocol the environment names; with
    /// nothing named, the silence is the answer.
    #[test]
    fn a_silent_terminal_falls_back_to_the_protocol_the_environment_names() {
        let window = Some(CellSize { w: 9, h: 18 });
        assert_eq!(
            unanswered(&env("xterm-256color", "iTerm.app"), window),
            Probe::Detected(Detected {
                protocol: Some(ImageProtocol::Iterm2),
                cell: window,
                tmux: false,
            })
        );
        assert_eq!(
            unanswered(&env("xterm-ghostty", "ghostty"), window),
            Probe::NoAnswer
        );
    }

    /// Only a tmux client's own pane is asked about: a tmux `TERM` without
    /// `$TMUX` (carried over ssh) names no server this process runs in.
    #[test]
    fn passthrough_is_read_only_for_a_tmux_clients_own_pane() {
        let carried = env("tmux-256color", "");
        assert!(carried.tmux() && !carried.our_tmux_pane());
        let client = EnvHints {
            tmux_client: true,
            ..carried
        };
        assert!(client.our_tmux_pane());
        let outside = EnvHints {
            tmux_client: true,
            ..env("xterm-ghostty", "ghostty")
        };
        assert!(!outside.our_tmux_pane());
    }

    /// The same `TERM`/`TERM_PROGRAM` test ratatui-image applies.
    #[test]
    fn tmux_is_named_by_term_or_term_program() {
        assert!(env("tmux-256color", "").tmux());
        assert!(env("xterm-ghostty", "tmux").tmux());
        assert!(!env("screen-256color", "ghostty").tmux());
        assert!(!EnvHints::default().tmux());
    }

    #[test]
    fn wezterm_and_konsole_are_never_asked_for_kitty_or_sixel() {
        for hints in [
            EnvHints {
                wezterm: true,
                ..EnvHints::default()
            },
            EnvHints {
                konsole: true,
                ..EnvHints::default()
            },
        ] {
            assert_eq!(
                hints.blacklist(),
                vec![ProtocolType::Kitty, ProtocolType::Sixel]
            );
        }
        assert!(EnvHints::default().blacklist().is_empty());
    }
}
