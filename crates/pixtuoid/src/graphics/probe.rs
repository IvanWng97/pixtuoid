//! Asking the terminal what it can draw: the IO half of [`super`].
//!
//! The query, the parse and the environment's hints are separate here so the
//! decisions — which protocol, which cell — are pure and tested; only
//! [`probe`] touches the terminal.

use ratatui_image::picker::ProtocolType;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::cap_parser::{Parser, Response};

use super::{CellSize, Detected, ImageProtocol, Probe, TermEnv, env_set, env_text};

/// What the environment says about the terminal beyond [`TermEnv`], read once
/// per probe.
///
/// Its protocol rules mirror ratatui-image 11.1.0's picker, cited per rule,
/// except where iTerm2 is told apart from what merely speaks its images
/// ([`EnvHints::is_iterm2`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EnvHints {
    env: TermEnv,
    lc_terminal: Option<String>,
    wezterm: bool,
    konsole: bool,
    iterm_session: bool,
    /// `WARP_CLIENT_VERSION`, which Warp sets in every shell it starts
    /// (warpdotdev/warp `crates/warp_terminal/src/local_tty/unix.rs`).
    warp_client: bool,
}

impl EnvHints {
    fn read() -> Self {
        Self {
            env: TermEnv::read(),
            lc_terminal: env_text("LC_TERMINAL"),
            wezterm: env_set("WEZTERM_EXECUTABLE"),
            konsole: env_set("KONSOLE_VERSION"),
            iterm_session: env_set("ITERM_SESSION_ID"),
            warp_client: env_set("WARP_CLIENT_VERSION"),
        }
    }

    /// A tmux client (`$TMUX`): only then does a bare `tmux` answer for the
    /// pane this process runs in. Without `$TMUX` — a tmux `TERM` carried over
    /// ssh — it answers for whatever server this host runs.
    fn our_tmux_pane(&self) -> bool {
        self.env.tmux_client
    }

    /// The capability query, wrapped for tmux whenever this process is inside
    /// it.
    fn query(&self) -> String {
        Parser::query(
            self.env.tmux(),
            QueryStdioOptions {
                blacklist_protocols: self.blacklist(),
                ..QueryStdioOptions::default()
            },
        )
    }

    /// Protocols never asked for under WezTerm or Konsole: neither implements
    /// kitty's placeholders, Konsole's SIXEL is buggy, and WezTerm draws better
    /// through iTerm2 (`picker.rs:119-128`).
    fn blacklist(&self) -> Vec<ProtocolType> {
        if self.wezterm || self.konsole {
            vec![ProtocolType::Kitty, ProtocolType::Sixel]
        } else {
            Vec::new()
        }
    }

    /// iTerm2 inline images, which upstream's query does not ask about
    /// (`cap_parser.rs:132-133`), guessed from the terminal the environment
    /// names: iTerm2 itself ([`Self::is_iterm2`]), WezTerm's markers inside
    /// tmux (`picker.rs:357-368`), and the other terminals that speak them
    /// (`picker.rs:372-390`).
    fn iterm2(&self) -> Option<ImageProtocol> {
        const OTHER_TERM_PROGRAMS: [&str; 7] = [
            "WezTerm", "mintty", "vscode", "Tabby", "Hyper", "rio", "Bobcat",
        ];
        let outer = self.env.tmux() && self.wezterm;
        let named = self
            .env
            .term_program
            .as_deref()
            .is_some_and(|p| OTHER_TERM_PROGRAMS.iter().any(|t| p.contains(t)));
        (self.is_iterm2() || outer || named).then_some(ImageProtocol::Iterm2)
    }

    /// Warp, where no protocol the cutaway speaks animates, seen live in
    /// v0.2026.09.30: it answers kitty's query but has no Unicode placeholders,
    /// draws no SIXEL, and never repaints an iTerm2 image it replaces. Inside
    /// tmux, whose own `TERM_PROGRAM` hides Warp's, its client marker names
    /// it.
    fn is_warp(&self) -> bool {
        let named = self
            .env
            .term_program
            .as_deref()
            .is_some_and(|p| p.contains("WarpTerminal"));
        named || (self.env.tmux() && self.warp_client)
    }

    /// iTerm2 itself, not a terminal that speaks its images: its
    /// `TERM_PROGRAM`, its session inside tmux, or its `LC_TERMINAL` where
    /// `TERM_PROGRAM` is absent (ssh) or tmux's own. A terminal started from
    /// iTerm2's shell names itself and inherits that `LC_TERMINAL`.
    fn is_iterm2(&self) -> bool {
        let named = |v: &Option<String>| v.as_deref().is_some_and(|v| v.contains("iTerm"));
        named(&self.env.term_program)
            || ((self.env.term_program.is_none() || self.env.tmux()) && named(&self.lc_terminal))
            || (self.env.tmux() && self.iterm_session)
    }
}

/// What the terminal's answer and the environment together say: SIXEL for
/// iTerm2 when it lists it, else kitty over SIXEL when it answers both
/// (ratatui-image 11.1.0 `picker.rs:544-554`), then the iTerm2 guess
/// (`picker.rs:136-140`); the cell from its answer, else from the kernel's
/// window size.
fn detected(responses: &[Response], hints: &EnvHints, window_cell: Option<CellSize>) -> Detected {
    // iTerm2 answers kitty's query, but its kitty, "except animation"
    // (iterm2.com/downloads.html changelog), is far too slow for 16x frames,
    // and its inline images nearly so; its SIXEL keeps up.
    let queried = if hints.is_iterm2() && responses.contains(&Response::Sixel) {
        Some(ImageProtocol::Sixel)
    } else if responses.contains(&Response::Kitty) {
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
    let unanimated = hints.is_warp();
    Detected {
        protocol: queried.or_else(|| hints.iterm2()).filter(|_| !unanimated),
        cell: answered_cell.or(window_cell),
        tmux: hints.env.tmux(),
        unanimated,
    }
}

/// A terminal whose reply never completed is still the protocol the
/// environment names, as upstream falls back when its query goes unanswered
/// (ratatui-image 11.1.0 `picker.rs:156-166`); with nothing named, it is
/// [`Probe::NoAnswer`].
fn unanswered(hints: &EnvHints, window_cell: Option<CellSize>) -> Probe {
    let d = detected(&[], hints, window_cell);
    if d.protocol.is_some() {
        Probe::Answered(d)
    } else {
        Probe::NoAnswer
    }
}

/// Feed one chunk of the reply to `parser`, collecting its responses; `true`
/// once the device-status reply that ends the query has arrived.
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
/// restores the mode only once a reply lands (ratatui-image 11.1.0
/// `picker.rs:606-644`); and read-only: inside tmux it reads
/// `allow-passthrough` where upstream turns it on (`picker.rs:349-355`).
pub(crate) fn probe(ask: bool) -> Probe {
    if !ask {
        return Probe::NotQueried;
    }
    let hints = EnvHints::read();
    if hints.our_tmux_pane() && tmux_passthrough() == Some(false) {
        return Probe::TmuxPassthroughOff;
    }
    let query = hints.query();
    let mut parser = Parser::new();
    let mut responses = Vec::new();
    match crate::term::query_tty(
        query.as_bytes(),
        super::GRAPHICS_PROBE_TIMEOUT,
        MAX_REPLY_BYTES,
        |chunk| take_reply(&mut parser, &mut responses, chunk),
    ) {
        None => Probe::NotQueried,
        Some(false) => unanswered(&hints, window_cell()),
        Some(true) => Probe::Answered(detected(&responses, &hints, window_cell())),
    }
}

/// Our pane's `allow-passthrough`, inherited value included (tmux(1)
/// `show-options -A`); `None` when tmux cannot say within
/// [`GRAPHICS_PROBE_TIMEOUT`](super::GRAPHICS_PROBE_TIMEOUT). Asked only where
/// [`EnvHints::our_tmux_pane`] holds.
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
fn window_cell() -> Option<CellSize> {
    let size = crossterm::terminal::window_size().ok()?;
    CellSize::of_window(ratatui::backend::WindowSize {
        columns_rows: ratatui::layout::Size::new(size.columns, size.rows),
        pixels: ratatui::layout::Size::new(size.width, size.height),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hints(term: &str, term_program: &str) -> EnvHints {
        EnvHints {
            env: TermEnv {
                term: Some(term.to_string()).filter(|s| !s.is_empty()),
                term_program: Some(term_program.to_string()).filter(|s| !s.is_empty()),
                ..TermEnv::default()
            },
            ..EnvHints::default()
        }
    }

    fn client(of: EnvHints) -> EnvHints {
        EnvHints {
            env: TermEnv {
                tmux_client: true,
                ..of.env
            },
            ..of
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
            detected(&[], &hints("xterm-256color", "iTerm.app"), None).protocol,
            Some(ImageProtocol::Iterm2)
        );
        assert_eq!(
            detected(
                &[Response::Kitty],
                &hints("xterm-256color", "WezTerm"),
                None
            )
            .protocol,
            Some(ImageProtocol::Kitty)
        );
        let lc = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..EnvHints::default()
        };
        assert_eq!(lc.iterm2(), Some(ImageProtocol::Iterm2));
        assert_eq!(hints("xterm-ghostty", "ghostty").iterm2(), None);
        let leaked = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..hints("xterm-256color", "Apple_Terminal")
        };
        assert_eq!(leaked.iterm2(), None, "started from iTerm2's shell");
    }

    /// iTerm2 answers kitty's query and lists SIXEL in its DA1 (3.7.3, live:
    /// `ESC [ ? 64;1;2;4;6;17;18;21;22;52 c` and `ESC _ G i=31;OK`), and only
    /// its SIXEL keeps up with a 16x frame: it takes SIXEL. A terminal that
    /// lists no SIXEL keeps its kitty, whatever the environment names.
    #[test]
    fn iterm2_takes_its_sixel_over_its_kitty() {
        let both = [Response::Kitty, Response::Sixel];
        let iterm2 = hints("xterm-256color", "iTerm.app");
        assert_eq!(
            detected(&both, &iterm2, None).protocol,
            Some(ImageProtocol::Sixel)
        );
        let in_tmux = client(EnvHints {
            iterm_session: true,
            ..hints("tmux-256color", "tmux")
        });
        assert_eq!(
            detected(&both, &in_tmux, None).protocol,
            Some(ImageProtocol::Sixel)
        );
        assert_eq!(
            detected(&[Response::Kitty], &iterm2, None).protocol,
            Some(ImageProtocol::Kitty),
            "no SIXEL listed"
        );
        let leaked = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..hints("xterm-ghostty", "ghostty")
        };
        assert_eq!(
            detected(&[Response::Kitty], &leaked, None).protocol,
            Some(ImageProtocol::Kitty),
            "a terminal started from iTerm2's shell"
        );
        let leaked_both = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..hints("xterm-256color", "WezTerm")
        };
        assert_eq!(
            detected(&both, &leaked_both, None).protocol,
            Some(ImageProtocol::Kitty),
            "a terminal started from iTerm2's shell that lists SIXEL too"
        );
        let over_ssh = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..hints("xterm-256color", "")
        };
        assert_eq!(
            detected(&both, &over_ssh, None).protocol,
            Some(ImageProtocol::Sixel),
            "over ssh only LC_TERMINAL arrives"
        );
        let tmux_over_ssh = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..client(hints("tmux-256color", "tmux"))
        };
        assert_eq!(
            detected(&both, &tmux_over_ssh, None).protocol,
            Some(ImageProtocol::Sixel),
            "tmux names itself, not the terminal"
        );
        // A pane's environment is the tmux server's: a server iTerm2 started
        // still reads as iTerm2 when another terminal attaches, which then
        // paints SIXEL if it lists it. Slower there, never frozen.
        let started_in_iterm2 = EnvHints {
            iterm_session: true,
            ..tmux_over_ssh.clone()
        };
        assert_eq!(
            detected(&both, &started_in_iterm2, None).protocol,
            Some(ImageProtocol::Sixel),
            "attached from another terminal: the server's iTerm2 environment"
        );
        assert_eq!(
            detected(&both, &hints("xterm-kitty", "kitty"), None).protocol,
            Some(ImageProtocol::Kitty)
        );
    }

    /// Warp's kitty answer is taken as no protocol the cutaway animates with,
    /// its cell kept for a `--graphics` that forces one; it is never guessed
    /// iTerm2.
    #[test]
    fn warp_answers_but_animates_no_protocol() {
        let warp = hints("xterm-256color", "WarpTerminal");
        let cell = CellSize { w: 8, h: 18 };
        let d = detected(&[Response::Kitty], &warp, Some(cell));
        assert_eq!((d.protocol, d.cell, d.unanimated), (None, Some(cell), true));
        assert_eq!(warp.iterm2(), None);
        assert!(!hints("xterm-256color", "iTerm.app").is_warp());
    }

    /// Inside tmux Warp is named by its client marker, since tmux's own
    /// `TERM_PROGRAM` hides Warp's; outside tmux the marker alone names
    /// nothing: a terminal started from a Warp shell names itself.
    #[test]
    fn warp_inside_tmux_is_named_by_its_client_marker() {
        let tmux = EnvHints {
            warp_client: true,
            ..client(hints("tmux-256color", "tmux"))
        };
        assert!(tmux.is_warp());
        let d = detected(&[Response::Kitty], &tmux, Some(CellSize { w: 8, h: 18 }));
        assert_eq!((d.protocol, d.unanimated), (None, true));
        let started_from_warp = EnvHints {
            warp_client: true,
            ..hints("xterm-ghostty", "ghostty")
        };
        assert!(!started_from_warp.is_warp());
    }

    /// Inside tmux the outer terminal's markers name iTerm2; outside they do
    /// not count.
    #[test]
    fn inside_tmux_the_outer_terminals_markers_name_iterm2() {
        let outer = EnvHints {
            iterm_session: true,
            ..hints("tmux-256color", "tmux")
        };
        assert_eq!(outer.iterm2(), Some(ImageProtocol::Iterm2));
        let not_tmux = EnvHints {
            iterm_session: true,
            ..hints("xterm-ghostty", "ghostty")
        };
        assert_eq!(not_tmux.iterm2(), None);
    }

    /// A silent terminal is still the protocol the environment names; with
    /// nothing named, the silence is the answer.
    #[test]
    fn a_silent_terminal_falls_back_to_the_protocol_the_environment_names() {
        let window = Some(CellSize { w: 9, h: 18 });
        assert_eq!(
            unanswered(&hints("xterm-256color", "iTerm.app"), window),
            Probe::Answered(Detected {
                protocol: Some(ImageProtocol::Iterm2),
                cell: window,
                tmux: false,
                unanimated: false,
            })
        );
        assert_eq!(
            unanswered(&hints("xterm-ghostty", "ghostty"), window),
            Probe::NoAnswer
        );
    }

    /// Only a tmux client's own pane is asked about: a tmux `TERM` without
    /// `$TMUX` (carried over ssh) names no server this process runs in.
    #[test]
    fn passthrough_is_read_only_for_a_tmux_clients_own_pane() {
        let carried = hints("tmux-256color", "");
        assert!(carried.env.tmux() && !carried.our_tmux_pane());
        assert!(client(carried).our_tmux_pane());
    }

    /// Under a tmux whose `default-terminal` is not tmux's own, `$TMUX` alone
    /// puts this process inside tmux: the image is wrapped, the link is
    /// tmux's, our pane is asked about passthrough, and the outer terminal's
    /// markers count.
    #[test]
    fn a_tmux_client_is_inside_tmux_whatever_its_term() {
        let pane = client(hints("xterm-256color", ""));
        assert!(detected(&[Response::Kitty], &pane, None).tmux);
        assert!(pane.env.link().tmux);
        assert!(pane.our_tmux_pane());
        assert!(pane.query().starts_with("\x1bPtmux;"), "{:?}", pane.query());
        let outer = EnvHints {
            iterm_session: true,
            ..pane
        };
        assert_eq!(outer.iterm2(), Some(ImageProtocol::Iterm2));
    }

    /// The same `TERM`/`TERM_PROGRAM` test ratatui-image applies.
    #[test]
    fn tmux_is_named_by_term_or_term_program() {
        assert!(hints("tmux-256color", "").env.tmux());
        assert!(hints("xterm-ghostty", "tmux").env.tmux());
        assert!(!hints("screen-256color", "ghostty").env.tmux());
        assert!(!EnvHints::default().env.tmux());
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
