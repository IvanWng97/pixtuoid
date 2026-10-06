//! Frame-time telemetry, after Android's JankStats
//! (<https://developer.android.com/topic/performance/jankstats>): every frame
//! over twice the paint interval is reported with what it was drawn under, and
//! once a minute the frames' spread is.

use std::time::{Duration, Instant};

use pixtuoid_scene::anim::PAINT_FRAME_MS;
use pixtuoid_scene::look::FrameNote;

/// A frame past the paint interval, in the window's microseconds.
const OVER_US: u32 = (PAINT_FRAME_MS * 1000) as u32;

/// How often the spread is reported.
const WINDOW: Duration = Duration::from_secs(60);
/// A window's frames twice over, room for a loop that runs hot: past it the
/// oldest are overwritten.
const RING: usize = 2 * (WINDOW.as_millis() / PAINT_FRAME_MS as u128) as usize;

/// What a frame's image transmits did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameSend {
    /// Whether the scene repainted whole, in rects, or not at all.
    pub(crate) dirty: &'static str,
    /// The tiles whose pixels changed.
    pub(crate) changed: usize,
    /// The tiles written.
    pub(crate) sent: usize,
    /// Their bytes.
    pub(crate) bytes: u64,
    /// Cutting and encoding them.
    pub(crate) encode: Duration,
    /// Writing and flushing them.
    pub(crate) write: Duration,
}

/// The frame times of the current window, and its janks.
#[derive(Debug)]
pub(crate) struct Jank {
    micros: [u32; RING],
    len: usize,
    next: usize,
    janks: u32,
    since: Instant,
    painter: Painter,
}

/// What draws the frames a summary spreads: one matrix cell of a run.
#[derive(Debug, Clone, Default)]
pub(crate) struct Painter {
    /// The image protocol, or `classic` for the half-blocks.
    pub(crate) look: &'static str,
    /// Buffer pixels per layout unit; 1 for the half-blocks.
    pub(crate) scale: u16,
    /// Inside tmux.
    pub(crate) tmux: bool,
    /// `$TERM_PROGRAM`, the terminal's own name for itself.
    pub(crate) terminal: Option<String>,
}

impl Jank {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            micros: [0; RING],
            len: 0,
            next: 0,
            janks: 0,
            since: now,
            painter: Painter::default(),
        }
    }

    /// Name what draws the frames from now on.
    pub(crate) fn painted_by(&mut self, painter: Painter) {
        self.painter = painter;
    }

    /// Count a frame that took `total`, reporting it when it janked and the
    /// window's spread once `now` closes it.
    pub(crate) fn record(
        &mut self,
        total: Duration,
        send: Option<FrameSend>,
        note: Option<FrameNote>,
        now: Instant,
    ) {
        self.micros[self.next] = u32::try_from(total.as_micros()).unwrap_or(u32::MAX);
        self.next = (self.next + 1) % RING;
        self.len = (self.len + 1).min(RING);
        if total > 2 * Duration::from_millis(PAINT_FRAME_MS) {
            self.janks += 1;
            report(total, send.unwrap_or_default(), note);
        }
        if now.duration_since(self.since) >= WINDOW {
            self.summarize();
            self.len = 0;
            self.next = 0;
            self.janks = 0;
            self.since = now;
        }
    }

    fn summarize(&self) {
        let mut sorted = self.micros;
        let window = &mut sorted[..self.len];
        window.sort_unstable();
        let at =
            |q: usize| window.get((window.len() * q / 100).min(window.len().saturating_sub(1)));
        let ms = |us: Option<&u32>| us.map_or(0.0, |&us| f64::from(us) / 1000.0);
        let (p50, p99, max) = (ms(at(50)), ms(at(99)), ms(window.last()));
        let frames = self.len;
        let janks = self.janks;
        let over = window.iter().filter(|&&us| us > OVER_US).count();
        let Painter {
            look,
            scale,
            tmux,
            terminal,
        } = &self.painter;
        if janks > 0 {
            tracing::warn!(look, scale, tmux, terminal = ?terminal, frames, over, janks, p50, p99, max, "frame pacing");
        } else {
            tracing::info!(look, scale, tmux, terminal = ?terminal, frames, over, janks, p50, p99, max, "frame pacing");
        }
    }
}

fn report(total: Duration, send: FrameSend, note: Option<FrameNote>) {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let repaint = note.and_then(|n| n.repaint);
    let (from, to, share) = note.map_or((None, None, 0.0), |n| {
        (Some(n.weather.0), Some(n.weather.1), n.weather.2)
    });
    tracing::warn!(
        total = ms(total),
        produce = ms(total.saturating_sub(send.encode + send.write)),
        encode = ms(send.encode),
        write = ms(send.write),
        dirty = send.dirty,
        changed = send.changed,
        sent = send.sent,
        bytes = send.bytes,
        repaint = ?repaint,
        weather_from = ?from,
        weather_to = ?to,
        weather_share = share,
        strike = note.is_some_and(|n| n.strike),
        "frame jank"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow() -> Duration {
        2 * Duration::from_millis(PAINT_FRAME_MS) + Duration::from_millis(1)
    }

    /// A frame over twice the interval is reported, with its transmits; one
    /// within it is not.
    #[test]
    fn a_frame_over_twice_the_interval_is_reported() {
        let t0 = Instant::now();
        let logged = crate::test_capture::capture(|| {
            let mut jank = Jank::new(t0);
            jank.record(Duration::from_millis(PAINT_FRAME_MS), None, None, t0);
            let send = FrameSend {
                dirty: "all",
                changed: 1275,
                sent: 1275,
                bytes: 1_400_000,
                encode: Duration::from_millis(30),
                write: Duration::from_millis(5),
            };
            jank.record(slow(), Some(send), None, t0);
        });
        assert_eq!(logged.matches("frame jank").count(), 1, "{logged}");
        assert!(logged.contains("dirty=\"all\""), "{logged}");
        assert!(logged.contains("sent=1275"), "{logged}");
    }

    /// A window that closes reports its frames' spread and janks, then starts
    /// afresh.
    #[test]
    fn a_closed_window_reports_its_spread_and_starts_afresh() {
        let t0 = Instant::now();
        let logged = crate::test_capture::capture(|| {
            let mut jank = Jank::new(t0);
            jank.painted_by(Painter {
                look: "kitty",
                scale: 16,
                tmux: true,
                terminal: Some("ghostty".into()),
            });
            for _ in 0..99 {
                jank.record(Duration::from_millis(10), None, None, t0);
            }
            jank.record(slow(), None, None, t0 + WINDOW);
            jank.record(Duration::from_millis(10), None, None, t0 + WINDOW);
        });
        let summaries: Vec<&str> = logged
            .lines()
            .filter(|l| l.contains("frame pacing"))
            .collect();
        assert_eq!(summaries.len(), 1, "{logged}");
        let line = summaries[0];
        assert!(line.contains("frames=100"), "{line}");
        assert!(line.contains("janks=1"), "{line}");
        assert!(line.contains("over=1"), "{line}");
        assert!(line.contains("look=\"kitty\""), "{line}");
        assert!(line.contains("tmux=true"), "{line}");
        assert!(line.contains("p50=10"), "{line}");
        assert!(
            line.contains(" WARN "),
            "a window with a jank warns: {line}"
        );
    }
}
