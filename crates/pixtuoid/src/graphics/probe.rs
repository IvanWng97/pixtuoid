//! Asking the terminal what it can draw: the IO half of [`super`].
//!
//! The query, the parse and the environment's hints are separate here so the
//! decisions — which protocol, which cell — are pure and tested; only
//! [`probe`] touches the terminal.

use ratatui_image::picker::ProtocolType;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::cap_parser::{Parser, Response};

use super::{CellSize, Detected, ImageProtocol, Probe, TermEnv, env_set, env_text};

/// The terminal the environment names, as far as the cutaway's protocols go.
///
/// Its rows mirror ratatui-image 11.1.0's picker, cited per row, except
/// where iTerm2 is told apart from what merely speaks its images, and Warp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Iterm2,
    /// Seen live in v0.2026.09.30: it answers kitty's query but has no
    /// Unicode placeholders, draws no SIXEL, and never repaints an iTerm2
    /// image it replaces.
    Warp,
    WezTerm,
    Konsole,
    /// Built on xterm.js, which draws images only through its image addon,
    /// loaded where the user turns it on: VS Code's `enableImages`
    /// (microsoft/vscode `src/vs/workbench/contrib/terminal/browser/xterm/xtermTerminal.ts:1025`),
    /// Tabby's `terminal.sixel` (Eugeny/tabby
    /// `tabby-terminal/src/frontends/xtermFrontend.ts:210`), Hyper's
    /// `imageSupport` (vercel/hyper `lib/components/term.tsx:243`).
    XtermJs,
    /// Another terminal that speaks iTerm2's images and isn't iTerm2
    /// (`picker.rs:372-390`).
    SpeaksIterm2,
    /// Any other, kitty, Ghostty and Alacritty among them.
    Other,
}

/// A protocol in a terminal's row, by what shows the terminal draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    /// kitty's, by its answer to kitty's query.
    Kitty,
    /// SIXEL, listed in its DA1.
    Sixel,
    /// iTerm2's images, which the query never asks about
    /// (`cap_parser.rs:132-133`).
    Iterm2(Iterm2Sign),
}

/// What stands in for an answer about iTerm2's images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Iterm2Sign {
    /// The terminal's name: it always draws them.
    Name,
    /// SIXEL in its DA1, which xterm.js's image addon lists only once loaded
    /// (xtermjs/xterm.js@c58ea36 `addons/addon-image/src/ImageAddon.ts:284-296`;
    /// without it, `src/common/InputHandler.ts:1706-1716`).
    Sixel,
}

impl Pick {
    fn protocol(self) -> ImageProtocol {
        match self {
            Self::Kitty => ImageProtocol::Kitty,
            Self::Sixel => ImageProtocol::Sixel,
            Self::Iterm2(_) => ImageProtocol::Iterm2,
        }
    }

    /// The query part whose answer shows it, if any.
    fn asks(self) -> Option<ProtocolType> {
        match self {
            Self::Kitty => Some(ProtocolType::Kitty),
            Self::Sixel | Self::Iterm2(Iterm2Sign::Sixel) => Some(ProtocolType::Sixel),
            Self::Iterm2(Iterm2Sign::Name) => None,
        }
    }

    fn shown_by(self, responses: &[Response]) -> bool {
        match self {
            Self::Kitty => responses.contains(&Response::Kitty),
            Self::Sixel | Self::Iterm2(Iterm2Sign::Sixel) => responses.contains(&Response::Sixel),
            Self::Iterm2(Iterm2Sign::Name) => true,
        }
    }
}

impl Terminal {
    /// What the cutaway animates with in it, best first.
    fn animates(self) -> &'static [Pick] {
        use Pick::{Iterm2, Kitty, Sixel};
        match self {
            // Its kitty, "except animation" (iterm2.com/downloads.html
            // changelog), is far too slow for 16x frames, and its inline
            // images nearly so; its SIXEL keeps up.
            Self::Iterm2 => &[Sixel, Kitty, Iterm2(Iterm2Sign::Name)],
            Self::Warp => &[],
            // Neither implements kitty's placeholders, Konsole's SIXEL is
            // buggy, and WezTerm draws better through iTerm2
            // (`picker.rs:119-128`).
            Self::WezTerm => &[Iterm2(Iterm2Sign::Name)],
            Self::Konsole => &[],
            // Its kitty answers but parses no Unicode placeholder (no `U` key,
            // xtermjs/xterm.js@c58ea36
            // `addons/addon-image/src/kitty/KittyGraphicsTypes.ts:39-81`).
            Self::XtermJs => &[Iterm2(Iterm2Sign::Sixel)],
            // Kitty over SIXEL where both answer (`picker.rs:544-554`), then
            // the iTerm2 guess (`picker.rs:136-140`).
            Self::SpeaksIterm2 => &[Kitty, Sixel, Iterm2(Iterm2Sign::Name)],
            Self::Other => &[Kitty, Sixel],
        }
    }
}

/// What the environment says about the terminal beyond [`TermEnv`], read once
/// per probe.
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
    /// it, asking only for what the terminal animates.
    fn query(&self) -> String {
        let animates = self.terminal().animates();
        Parser::query(
            self.env.tmux(),
            QueryStdioOptions {
                blacklist_protocols: [ProtocolType::Kitty, ProtocolType::Sixel]
                    .into_iter()
                    .filter(|t| !animates.iter().any(|p| p.asks() == Some(*t)))
                    .collect(),
                ..QueryStdioOptions::default()
            },
        )
    }

    /// The terminal its own `TERM` names, else its `TERM_PROGRAM`, else,
    /// where that is absent (ssh) or tmux's own, its markers. A terminal
    /// started from another's shell inherits the rest, and those that set
    /// no `TERM_PROGRAM` inherit that too.
    fn terminal(&self) -> Terminal {
        // Their defaults: kovidgoyal/kitty `kitty/options/definition.py:3322`,
        // ghostty-org/ghostty `src/config/Config.zig:3928`,
        // alacritty/alacritty `alacritty_terminal/src/tty/mod.rs:104`.
        const OWN_TERMS: [&str; 3] = ["xterm-kitty", "xterm-ghostty", "alacritty"];
        const XTERM_JS: [&str; 3] = ["vscode", "Tabby", "Hyper"];
        const SPEAKS_ITERM2: [&str; 3] = ["mintty", "rio", "Bobcat"];
        if self
            .env
            .term
            .as_deref()
            .is_some_and(|t| OWN_TERMS.contains(&t))
        {
            return Terminal::Other;
        }
        let program = self.env.term_program.as_deref().unwrap_or_default();
        let named = |names: &[&str]| names.iter().any(|n| program.contains(n));
        if named(&["iTerm"]) {
            return Terminal::Iterm2;
        }
        if named(&["WarpTerminal"]) {
            return Terminal::Warp;
        }
        if named(&["WezTerm"]) {
            return Terminal::WezTerm;
        }
        if named(&XTERM_JS) {
            return Terminal::XtermJs;
        }
        if named(&SPEAKS_ITERM2) {
            return Terminal::SpeaksIterm2;
        }
        if !(self.env.term_program.is_none() || self.env.tmux()) {
            return Terminal::Other;
        }
        let tmux = self.env.tmux();
        if tmux && self.warp_client {
            Terminal::Warp
        } else if self
            .lc_terminal
            .as_deref()
            .is_some_and(|v| v.contains("iTerm"))
            || (tmux && self.iterm_session)
        {
            Terminal::Iterm2
        } else if self.wezterm {
            Terminal::WezTerm
        } else if self.konsole {
            Terminal::Konsole
        } else {
            Terminal::Other
        }
    }
}

/// What the terminal's answer and the environment together say: the first
/// protocol the terminal [animates](Terminal::animates) with that its answer
/// shows; the cell from its answer, else from the kernel's window size.
fn detected(responses: &[Response], hints: &EnvHints, window_cell: Option<CellSize>) -> Detected {
    let animates = hints.terminal().animates();
    let pick = animates.iter().copied().find(|p| p.shown_by(responses));
    let answered_cell = responses.iter().find_map(|r| match r {
        Response::CellSize(Some((w, h))) => Some(CellSize { w: *w, h: *h }),
        _ => None,
    });
    Detected {
        protocol: pick.map(Pick::protocol),
        named: pick == Some(Pick::Iterm2(Iterm2Sign::Name)),
        cell: answered_cell.or(window_cell),
        tmux: hints.env.tmux(),
        unanimated: animates.is_empty(),
        shm: false,
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
        Some(true) => {
            let mut d = detected(&responses, &hints, window_cell());
            // Never through tmux: see `resolve`.
            d.shm = super::Route::may_share(d.protocol, d.tmux) && reads_shared_memory();
            Probe::Answered(d)
        }
    }
}

/// Whether the terminal reads kitty images from this host's shared memory:
/// `a=q` with `t=s` has it load a 1x1 image from an object we publish without
/// storing it, and answer `OK` or an error (kitty's graphics protocol,
/// "Querying support and available transmission mediums"); a device-status
/// request ends the reply, as in [`probe`].
fn reads_shared_memory() -> bool {
    let Ok(name) = super::shm::publish(&[0; 3], std::time::Instant::now()) else {
        return false;
    };
    let id = super::kitty::process_base();
    let mut reply = Vec::new();
    let answered = crate::term::query_tty(
        shm_query(id, &name).as_bytes(),
        super::GRAPHICS_PROBE_TIMEOUT,
        MAX_REPLY_BYTES,
        |chunk| {
            reply.extend_from_slice(chunk);
            reply.windows(DSR_OK.len()).any(|w| w == DSR_OK)
        },
    );
    // The terminal unlinks what it read; this unlinks what it didn't.
    super::shm::unlink_all();
    answered == Some(true) && shm_ok(id, &reply)
}

/// The device-status reply that ends [`shm_query`]'s answer.
const DSR_OK: &[u8] = b"\x1b[0n";

/// The query for a 1x1 RGB image from the shared-memory object `name`, as
/// image `id`, then a device-status request.
fn shm_query(id: u32, name: &str) -> String {
    format!(
        "\x1b_Gi={id},a=q,t=s,f=24,s=1,v=1,S=3;{}\x1b\\\x1b[5n",
        base64_simd::STANDARD.encode_to_string(name)
    )
}

/// Whether `reply` holds image `id`'s `OK`.
fn shm_ok(id: u32, reply: &[u8]) -> bool {
    let ok = format!("\x1b_Gi={id};OK\x1b\\");
    reply.windows(ok.len()).any(|w| w == ok.as_bytes())
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

    /// Only the query's own id answering `OK` reads shared memory: an error,
    /// or another image's `OK`, does not.
    #[test]
    fn shared_memory_is_read_only_where_the_query_s_own_id_answers_ok() {
        assert!(shm_ok(7, b"\x1b_Gi=7;OK\x1b\\\x1b[0n"));
        assert!(!shm_ok(
            7,
            b"\x1b_Gi=7;EBADF:Failed to read image\x1b\\\x1b[0n"
        ));
        assert!(!shm_ok(7, b"\x1b_Gi=31;OK\x1b\\\x1b[0n"));
        assert!(!shm_ok(7, b"\x1b[0n"));
        let query = shm_query(7, "/pxt1-0");
        assert!(
            query.starts_with("\x1b_Gi=7,a=q,t=s,f=24,s=1,v=1,S=3;"),
            "{query:?}"
        );
        assert!(query.ends_with("\x1b\\\x1b[5n"));
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
            detected(&[Response::Kitty], &hints("xterm-256color", "mintty"), None).protocol,
            Some(ImageProtocol::Kitty)
        );
        assert_eq!(
            detected(&[], &hints("xterm-256color", "mintty"), None).protocol,
            Some(ImageProtocol::Iterm2)
        );
        let lc = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..EnvHints::default()
        };
        assert_eq!(lc.terminal(), Terminal::Iterm2);
        assert_eq!(
            hints("xterm-ghostty", "ghostty").terminal(),
            Terminal::Other
        );
        let leaked = EnvHints {
            lc_terminal: Some("iTerm2".into()),
            ..hints("xterm-256color", "Apple_Terminal")
        };
        assert_eq!(
            leaked.terminal(),
            Terminal::Other,
            "started from iTerm2's shell"
        );
    }

    /// An xterm.js terminal draws images only with its image addon loaded,
    /// which its user turns on, and the addon's DA1 then lists SIXEL: its name
    /// alone picks nothing, and its kitty, without Unicode placeholders, never.
    #[test]
    fn an_xterm_js_terminal_draws_iterm2_only_with_its_image_addon_loaded() {
        for program in ["vscode", "Tabby", "Hyper"] {
            let hints = hints("xterm-256color", program);
            assert_eq!(detected(&[], &hints, None).protocol, None, "{program}");
            assert_eq!(unanswered(&hints, None), Probe::NoAnswer, "{program}");
            let loaded = detected(&[Response::Kitty, Response::Sixel], &hints, None);
            assert_eq!(loaded.protocol, Some(ImageProtocol::Iterm2), "{program}");
            assert!(!loaded.named, "{program}: answered");
        }
    }

    /// A protocol picked on the terminal's name alone says so, for the
    /// doctor; one it answered does not.
    #[test]
    fn a_protocol_the_name_alone_picks_is_marked_named() {
        let iterm2 = hints("xterm-256color", "iTerm.app");
        assert!(detected(&[], &iterm2, None).named);
        assert!(!detected(&[Response::Sixel], &iterm2, None).named);
        assert!(!detected(&[Response::Kitty], &EnvHints::default(), None).named);
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
            ..hints("xterm-ghostty", "ghostty")
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
        assert_eq!(warp.terminal(), Terminal::Warp);
        assert!(!warp.query().contains("\x1b_G"), "kitty is never asked");
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
        assert_eq!(tmux.terminal(), Terminal::Warp);
        let d = detected(&[Response::Kitty], &tmux, Some(CellSize { w: 8, h: 18 }));
        assert_eq!((d.protocol, d.unanimated), (None, true));
        let started_from_warp = EnvHints {
            warp_client: true,
            ..hints("xterm-ghostty", "ghostty")
        };
        assert_eq!(started_from_warp.terminal(), Terminal::Other);
    }

    /// Inside tmux the outer terminal's markers name iTerm2; outside they do
    /// not count.
    #[test]
    fn inside_tmux_the_outer_terminals_markers_name_iterm2() {
        let outer = EnvHints {
            iterm_session: true,
            ..hints("tmux-256color", "tmux")
        };
        assert_eq!(outer.terminal(), Terminal::Iterm2);
        let not_tmux = EnvHints {
            iterm_session: true,
            ..hints("xterm-ghostty", "ghostty")
        };
        assert_eq!(not_tmux.terminal(), Terminal::Other);
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
                named: true,
                shm: false,
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
        assert_eq!(outer.terminal(), Terminal::Iterm2);
    }

    /// The same `TERM`/`TERM_PROGRAM` test ratatui-image applies.
    #[test]
    fn tmux_is_named_by_term_or_term_program() {
        assert!(hints("tmux-256color", "").env.tmux());
        assert!(hints("xterm-ghostty", "tmux").env.tmux());
        assert!(!hints("screen-256color", "ghostty").env.tmux());
        assert!(!EnvHints::default().env.tmux());
    }

    /// WezTerm and Konsole are never asked for kitty or SIXEL: WezTerm draws
    /// through iTerm2's images, Konsole with none. Their markers name them
    /// only where `TERM_PROGRAM` doesn't name another terminal.
    #[test]
    fn wezterm_and_konsole_are_never_asked_for_kitty_or_sixel() {
        let wezterm = EnvHints {
            wezterm: true,
            ..hints("xterm-256color", "WezTerm")
        };
        let konsole = EnvHints {
            konsole: true,
            ..hints("xterm-256color", "")
        };
        for (hints, want) in [(&wezterm, Some(ImageProtocol::Iterm2)), (&konsole, None)] {
            let query = hints.query();
            assert!(
                !query.contains("\x1b_G") && !query.contains("\x1b[c"),
                "{query:?}"
            );
            let d = detected(&[Response::Kitty, Response::Sixel], hints, None);
            assert_eq!(d.protocol, want);
        }
        assert!(EnvHints::default().query().contains("\x1b_G"));
        let started_from_wezterm = EnvHints {
            wezterm: true,
            ..hints("xterm-ghostty", "ghostty")
        };
        assert_eq!(started_from_wezterm.terminal(), Terminal::Other);
    }

    /// kitty and Alacritty set `TERM` but no `TERM_PROGRAM`: started from
    /// another terminal's shell, their own `TERM` names them over what they
    /// inherited.
    #[test]
    fn a_terminals_own_term_names_it_over_what_it_inherited() {
        let kitty_from_wezterm = EnvHints {
            wezterm: true,
            ..hints("xterm-kitty", "WezTerm")
        };
        assert_eq!(
            detected(&[Response::Kitty], &kitty_from_wezterm, None).protocol,
            Some(ImageProtocol::Kitty)
        );
        let kitty_from_konsole = EnvHints {
            konsole: true,
            ..hints("xterm-kitty", "")
        };
        assert_eq!(
            detected(&[Response::Kitty], &kitty_from_konsole, None).protocol,
            Some(ImageProtocol::Kitty)
        );
        let alacritty_from_vscode = hints("alacritty", "vscode");
        assert_eq!(detected(&[], &alacritty_from_vscode, None).protocol, None);
    }
}
