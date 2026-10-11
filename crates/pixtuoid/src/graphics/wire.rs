//! The graphics probe's bytes both ways: the questions it writes, the reader
//! for the terminal's answer, and tmux's passthrough, which kitty's images
//! ride too.

#[cfg(unix)]
use super::CellSize;

/// How an escape reaches the terminal: as written, or inside tmux's
/// passthrough (tmux(1), `allow-passthrough`), which takes the escape's own
/// ESCs doubled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Passthrough {
    pub(crate) open: &'static str,
    pub(crate) esc: &'static str,
    pub(crate) close: &'static str,
}

impl Passthrough {
    /// tmux's when `tmux`, else none.
    pub(crate) const fn new(tmux: bool) -> Self {
        if tmux {
            Self {
                open: "\x1bPtmux;",
                esc: "\x1b\x1b",
                close: "\x1b\\",
            }
        } else {
            Self {
                open: "",
                esc: "\x1b",
                close: "",
            }
        }
    }
}

/// The 1x1 RGB image a [`Question::Kitty`] loads.
#[cfg(unix)]
pub(crate) const PIXEL: [u8; 3] = [0; 3];

/// One question the probe asks, each answered by a reply of its own.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Question<'a> {
    /// kitty's query action: load [`PIXEL`] as image `id` without storing it,
    /// from the escape, or with `shm` from this host's shared-memory object of
    /// that name (kitty's graphics protocol, "Querying support and available
    /// transmission mediums").
    Kitty { id: u32, shm: Option<&'a str> },
    /// The primary device attributes, which list SIXEL as `4` (xterm's
    /// ctlseqs, `CSI Ps c`).
    Attributes,
    /// The cell's size in pixels (xterm's ctlseqs, `CSI 16 t`).
    CellSize,
}

/// The bytes asking `questions` inside `passthrough`, then the device-status
/// request every terminal answers (xterm's ctlseqs, `CSI 5 n`), whose reply
/// ends the answer.
#[cfg(unix)]
pub(crate) fn ask(questions: &[Question<'_>], passthrough: Passthrough) -> String {
    use std::fmt::Write as _;
    let Passthrough { open, esc, close } = passthrough;
    let b64 = |bytes: &[u8]| base64_simd::STANDARD.encode_to_string(bytes);
    let mut out = String::from(open);
    for question in questions {
        let _ = match *question {
            Question::Kitty { id, shm: None } => write!(
                out,
                "{esc}_Gi={id},s=1,v=1,a=q,t=d,f=24;{}{esc}\\",
                b64(&PIXEL)
            ),
            Question::Kitty {
                id,
                shm: Some(name),
            } => write!(
                out,
                "{esc}_Gi={id},s=1,v=1,a=q,t=s,f=24,S={};{}{esc}\\",
                PIXEL.len(),
                b64(name.as_bytes())
            ),
            Question::Attributes => write!(out, "{esc}[c"),
            Question::CellSize => write!(out, "{esc}[16t"),
        };
    }
    let _ = write!(out, "{esc}[5n{close}");
    out
}

/// What the terminal answered, as far as the [`Question`]s go.
#[cfg(unix)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The kitty query answered `OK`.
    pub(crate) kitty: bool,
    /// The device attributes list SIXEL.
    pub(crate) sixel: bool,
    /// The first well-formed cell size reported.
    pub(crate) cell: Option<CellSize>,
}

/// Where the [`Reader`] is in an escape sequence: the DEC parser's states
/// (vt100.net/emu/dec_ansi_parser), except that an APC's text is kept, since
/// kitty answers in one.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    Apc,
    /// An ESC inside an APC: the ST's first byte, or a new escape.
    ApcEscape,
}

#[cfg(unix)]
const ESC: u8 = 0x1b;

/// Reads the terminal's answer a chunk at a time, keeping what it says about
/// the [`Question`]s and skipping the rest: replies to nothing asked, and
/// whatever the user types meanwhile.
#[cfg(unix)]
pub(crate) struct Reader {
    kitty_id: u32,
    state: State,
    /// The current sequence's bytes after its introducer.
    seq: Vec<u8>,
    answer: Answer,
    ended: bool,
}

#[cfg(unix)]
impl Reader {
    /// A reader whose kitty answer is image `kitty_id`'s.
    pub(crate) fn new(kitty_id: u32) -> Self {
        Self {
            kitty_id,
            state: State::Ground,
            seq: Vec::new(),
            answer: Answer::default(),
            ended: false,
        }
    }

    /// Read one chunk; `true` once the device-status reply that ends the
    /// answer has arrived, after which nothing more is read.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> bool {
        for &byte in chunk {
            if self.ended {
                break;
            }
            self.step(byte);
        }
        self.ended
    }

    pub(crate) fn answer(&self) -> Answer {
        self.answer
    }

    fn step(&mut self, byte: u8) {
        if self.state == State::ApcEscape {
            if byte == b'\\' {
                self.apc();
                self.state = State::Ground;
                return;
            }
            self.state = State::Escape;
        }
        self.state = match (self.state, byte) {
            (State::Apc, ESC) => State::ApcEscape,
            (State::Apc, _) => {
                self.seq.push(byte);
                State::Apc
            }
            (_, ESC) => State::Escape,
            (State::Escape, b'[') => {
                self.seq.clear();
                State::Csi
            }
            (State::Escape, b'_') => {
                self.seq.clear();
                State::Apc
            }
            // Parameter and intermediate bytes.
            (State::Csi, 0x20..=0x3f) => {
                self.seq.push(byte);
                State::Csi
            }
            (State::Csi, 0x40..=0x7e) => {
                self.csi(byte);
                State::Ground
            }
            _ => State::Ground,
        };
    }

    fn csi(&mut self, final_byte: u8) {
        match final_byte {
            b'c' => {
                if let Some(attributes) = self.seq.strip_prefix(b"?") {
                    self.answer.sixel |= params(attributes).any(|p| p == b"4");
                }
            }
            // `CSI 6 ; height ; width t`, xterm's reply to `CSI 16 t`.
            b't' => {
                let mut p = params(&self.seq);
                if let (Some(b"6"), Some(h), Some(w), None) =
                    (p.next(), p.next(), p.next(), p.next())
                {
                    let cell = number(h)
                        .zip(number(w))
                        .filter(|&(h, w)| h > 0 && w > 0)
                        .map(|(h, w)| CellSize { w, h });
                    self.answer.cell = self.answer.cell.or(cell);
                }
            }
            b'n' => self.ended |= self.seq == b"0",
            _ => {}
        }
    }

    /// A kitty reply, `G` then its keys, `;` and `OK` or an error.
    fn apc(&mut self) {
        let Some(body) = self.seq.strip_prefix(b"G") else {
            return;
        };
        let mut parts = body.splitn(2, |&b| b == b';');
        let (Some(keys), Some(message)) = (parts.next(), parts.next()) else {
            return;
        };
        let ours = keys
            .split(|&b| b == b',')
            .any(|key| key.strip_prefix(b"i=").and_then(number) == Some(self.kitty_id));
        self.answer.kitty |= ours && message == b"OK";
    }
}

#[cfg(unix)]
fn params(seq: &[u8]) -> impl Iterator<Item = &[u8]> {
    seq.split(|&b| b == b';')
}

#[cfg(unix)]
fn number<T: std::str::FromStr>(digits: &[u8]) -> Option<T> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn read(id: u32, reply: &[u8]) -> (Answer, bool) {
        let mut reader = Reader::new(id);
        let ended = reader.feed(reply);
        (reader.answer(), ended)
    }

    const CAPABILITIES: [Question<'static>; 3] = [
        Question::Kitty { id: 31, shm: None },
        Question::Attributes,
        Question::CellSize,
    ];

    #[test]
    fn the_capability_query_asks_each_question_then_for_the_status() {
        assert_eq!(
            ask(&CAPABILITIES, Passthrough::new(false)),
            "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c\x1b[16t\x1b[5n"
        );
        assert_eq!(ask(&[], Passthrough::new(false)), "\x1b[5n");
    }

    /// Inside tmux the whole query rides one passthrough, its ESCs doubled.
    #[test]
    fn inside_tmux_the_query_rides_one_passthrough() {
        assert_eq!(
            ask(&CAPABILITIES, Passthrough::new(true)),
            "\x1bPtmux;\x1b\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\x1b\\\
             \x1b\x1b[c\x1b\x1b[16t\x1b\x1b[5n\x1b\\"
        );
    }

    #[test]
    fn the_shared_memory_query_names_the_object_and_its_size() {
        assert_eq!(
            ask(
                &[Question::Kitty {
                    id: 7,
                    shm: Some("/pxt1-0")
                }],
                Passthrough::new(false)
            ),
            "\x1b_Gi=7,s=1,v=1,a=q,t=s,f=24,S=3;L3B4dDEtMA==\x1b\\\x1b[5n"
        );
    }

    /// A kitty-and-SIXEL terminal's reply: the kitty acknowledgement, a DA1
    /// with SIXEL, the cell size, and the status reply that ends it.
    const KITTY_SIXEL_REPLY: &[u8] = b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;22c\x1b[6;41;17t\x1b[0n";

    #[test]
    fn a_full_reply_ends_at_the_status_reply_and_keeps_what_came_before() {
        assert_eq!(
            read(31, KITTY_SIXEL_REPLY),
            (
                Answer {
                    kitty: true,
                    sixel: true,
                    cell: Some(CellSize { w: 17, h: 41 }),
                },
                true
            )
        );
    }

    #[test]
    fn a_reply_split_across_reads_ends_on_its_last_chunk() {
        for at in 1..KITTY_SIXEL_REPLY.len() {
            let (head, tail) = KITTY_SIXEL_REPLY.split_at(at);
            let mut reader = Reader::new(31);
            assert!(!reader.feed(head), "split at {at}");
            assert!(reader.feed(tail), "split at {at}");
            assert_eq!(
                reader.answer(),
                read(31, KITTY_SIXEL_REPLY).0,
                "split at {at}"
            );
        }
    }

    /// An APC cut short gives way to the next escape, so the status reply
    /// still ends the answer.
    #[test]
    fn an_unterminated_apc_gives_way_to_the_status_reply() {
        assert_eq!(read(31, b"\x1b_Gi=31;OK\x1b[0n"), (Answer::default(), true));
    }

    /// Nothing after the status reply is read: it answers another query.
    #[test]
    fn nothing_after_the_status_reply_is_read() {
        let mut reader = Reader::new(31);
        assert!(reader.feed(b"\x1b[0n\x1b[?62;4c"));
        assert!(reader.feed(b"\x1b_Gi=31;OK\x1b\\"));
        assert_eq!(reader.answer(), Answer::default());
    }

    /// Only the reader's own image answering `OK` counts: an error, or
    /// another image's `OK`, does not.
    #[test]
    fn only_the_reader_s_own_image_answering_ok_counts() {
        assert!(read(7, b"\x1b_Gi=7;OK\x1b\\\x1b[0n").0.kitty);
        assert!(
            !read(7, b"\x1b_Gi=7;EBADF:Failed to read image\x1b\\\x1b[0n")
                .0
                .kitty
        );
        assert!(!read(7, b"\x1b_Gi=31;OK\x1b\\\x1b[0n").0.kitty);
        assert!(!read(7, b"\x1b[0n").0.kitty);
    }

    /// Typing and stray escapes around the replies are skipped (ratatui-image
    /// 11.1.0's garbage cases, `cap_parser.rs:329-348`).
    #[test]
    fn garbage_around_the_replies_is_skipped() {
        assert_eq!(
            read(31, b"\x1bhonkey\x1btonkey\x1b[42\x1b\\"),
            (Answer::default(), false)
        );
        assert_eq!(
            read(31, b"\x1bgarbage...\x1b[?64;5c\x1b[0n"),
            (Answer::default(), true)
        );
        assert_eq!(
            read(31, b"\x1b[6;7;14t\x1bgarbage...\x1b[?64;5c\x1b[0n"),
            (
                Answer {
                    cell: Some(CellSize { w: 14, h: 7 }),
                    ..Answer::default()
                },
                true
            )
        );
        assert!(read(31, b"typed\x1b_Gi=31;OK\x1b\\").0.kitty);
    }

    /// Only xterm's cell-size reply form is a cell size: a `t` reply led by
    /// any other number (`CSI 4 ; height ; width t` is the window's pixels)
    /// is not, and neither is a zero side, so the window's size stands in.
    #[test]
    fn only_the_cell_size_reply_form_is_a_cell_size() {
        assert_eq!(read(31, b"\x1b[4;800;1200t\x1b[0n").0.cell, None);
        assert_eq!(read(31, b"\x1b[9;41;17t\x1b[0n").0.cell, None);
        assert_eq!(read(31, b"\x1b[6;0;17t\x1b[0n").0.cell, None);
        assert_eq!(read(31, b"\x1b[6;41;0t\x1b[0n").0.cell, None);
        assert_eq!(read(31, b"\x1b[6;41t\x1b[0n").0.cell, None);
    }

    /// A DA1 without the `?` of a primary reply lists nothing, and a status
    /// other than `0` ends nothing.
    #[test]
    fn only_a_primary_attributes_reply_lists_sixel_and_only_status_ok_ends() {
        assert!(!read(31, b"\x1b[>4;1c\x1b[0n").0.sixel);
        assert!(!read(31, b"\x1b[3n").1);
    }

    /// Every byte, after every prefix that leaves the reader mid-sequence,
    /// is read without a panic, and only a status reply ends the answer.
    #[test]
    fn every_byte_in_every_state_is_read() {
        let prefixes: [&[u8]; 7] = [
            b"",
            b"\x1b",
            b"\x1b[",
            b"\x1b[6;41;",
            b"\x1b_G",
            b"\x1b_Gi=31;OK",
            b"\x1b_Gi=31;OK\x1b",
        ];
        for prefix in prefixes {
            for byte in 0..=u8::MAX {
                let mut reader = Reader::new(31);
                assert!(!reader.feed(prefix));
                assert!(!reader.feed(&[byte, byte]), "{prefix:?} {byte}");
            }
        }
    }
}
