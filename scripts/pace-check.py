#!/usr/bin/env python3
"""The fluency gate: run the release binary through a weather transition into
a storm, or across dusk, and fail on a frame past the paint interval or a
window whose p99 passes P99_MS.

The binary measures itself: its `frame pacing` summaries (`tui/jank.rs`), a
minute each and one at exit, are what this reads, from the log it writes.
`--live` runs it in this terminal, as a user would, which is the only run
whose writes meet a real terminal's parser; without it, a pseudo-terminal
sized for `--scale` stands in, which catches our side alone.

Usage:
    just pace-check [--live] [--scale classic|4|16] [--graphics kitty|sixel|iterm2]
                    [--run storm|dusk] [--secs N]
"""

import argparse
import fcntl
import os
import pty
import re
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/pixtuoid"
# The launch bar, owner-set: p99 at most this, and no frame past its
# scheduled interval (`over`, which the binary counts).
P99_MS = 20.0
# The pty runs on the owner's office: the bench names its terminal per scale.
SCALES = ["classic", "4", "16"]
# The lead into the storm transition: a boot and some steady frames first.
LEAD_S = 10


def pacing(*args):
    """What `examples/pacing.rs` prints for `args`: the authority on where a
    run starts and which terminal gives the owner's office."""
    out = subprocess.run(
        ["cargo", "run", "-q", "--release", "-p", "pixtuoid", "--example", "pacing", "--", *args],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return out.stdout.split()


def start_at(run):
    """The Unix second the run's clock starts at: today's nightfall, or the
    lead into the clock's next transition into a storm."""
    if run == "dusk":
        return int(pacing("dusk")[0])
    return int(pacing("next-storm")[0]) - LEAD_S


def run_pty(argv, env, geometry, secs):
    cols, rows, cell_w, cell_h = geometry
    pid, fd = pty.fork()
    if pid == 0:
        try:
            os.execve(argv[0], argv, env)
        finally:
            # An exec that fails must not go on running this script as a child.
            os._exit(127)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, cols * cell_w, rows * cell_h))
    os.kill(pid, signal.SIGWINCH)
    # `cat` drains the pty: a reader slower than the binary's writes would
    # block them, and charge this script's speed to its frames.
    drain = subprocess.Popen(["cat"], stdin=fd, stdout=subprocess.DEVNULL)
    time.sleep(secs)
    os.kill(pid, signal.SIGTERM)
    os.waitpid(pid, 0)
    drain.terminate()
    drain.wait()
    os.close(fd)


def run_live(argv, env, secs):
    child = subprocess.Popen(argv, env=env)
    try:
        child.wait(timeout=secs)
    except subprocess.TimeoutExpired:
        child.send_signal(signal.SIGTERM)
        child.wait()


FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')


def events(log, message):
    for line in log.read_text(errors="replace").splitlines():
        if message in line:
            yield {k: v.strip('"') for k, v in FIELD.findall(line)}


def causes(log):
    """The over-interval frames by what they were: Dirty kind, why the
    cutaway repainted whole, a view the terminal had not shown, and a
    strike."""
    counts = {}
    for e in [*events(log, "frame slow"), *events(log, "frame jank")]:
        repaint = e.get("repaint", "None")
        key = (
            e.get("dirty"),
            "repaint" if repaint != "None" else "-",
            "fresh" if e.get("fresh") == "true" else "-",
            "strike" if e.get("strike") == "true" else "-",
        )
        counts[key] = counts.get(key, 0) + 1
    return sorted(counts.items(), key=lambda kv: -kv[1])


def main():
    ap = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    ap.add_argument("--live", action="store_true")
    ap.add_argument("--scale", choices=SCALES, default="16")
    ap.add_argument("--graphics", choices=["kitty", "sixel", "iterm2"], default="kitty")
    ap.add_argument("--run", choices=["storm", "dusk"], default="storm")
    ap.add_argument("--secs", type=float, default=200.0)
    args = ap.parse_args()
    if not os.access(BIN, os.X_OK):
        sys.exit(f"pace-check: no {BIN}: run it as `just pace-check`, which builds it")

    log = Path(tempfile.mkstemp(prefix="pace-check-", suffix=".log")[1])
    env = dict(
        os.environ,
        PIXTUOID_FAKE_NOW=str(start_at(args.run)),
        PIXTUOID_LOG=str(log),
        # `frame slow` is debug: every frame past its interval, with its state.
        RUST_LOG="info,pixtuoid::tui::jank=debug",
    )
    graphics = "off" if args.scale == "classic" else args.graphics
    argv = [str(BIN), "--log-level", "info", "run", "--graphics", graphics]
    if args.live:
        run_live(argv, env, args.secs)
    else:
        # No pty answers the probe: a terminal the environment names plans
        # without one, and `--graphics` then picks the protocol.
        env.update(TERM="xterm-256color", TERM_PROGRAM="WezTerm")
        env.pop("TMUX", None)
        geometry = tuple(int(n) for n in pacing("terminal", "16" if args.scale == "classic" else args.scale))
        run_pty(argv, env, geometry, args.secs)

    if not any(events(log, "the clock starts at PIXTUOID_FAKE_NOW")):
        sys.exit(f"pace-check: the binary ran on the real clock, not the run's: {log}")
    windows = list(events(log, "frame pacing"))
    if not windows:
        sys.exit(f"pace-check: no `frame pacing` summary in {log}")
    w0 = windows[0]
    wanted = ("classic", "1") if args.scale == "classic" else (args.graphics, args.scale)
    if (w0.get("look"), w0.get("scale")) != wanted:
        sys.exit(f"pace-check: ran {w0.get('look')} x{w0.get('scale')}, not {wanted[0]} x{wanted[1]}: {log}")
    over = sum(int(w.get("over", 0)) for w in windows)
    p99 = max(float(w.get("p99", 0)) for w in windows)
    frames = sum(int(w.get("frames", 0)) for w in windows)
    terminal = w0.get("terminal") if args.live else "pty"
    print(
        f"{w0.get('look')} x{w0.get('scale')} tmux={w0.get('tmux')} terminal={terminal} sync={w0.get('sync')} "
        f"run={args.run}: {frames} frames, worst window p99 {p99:.1f} ms, over {over} | log {log}"
    )
    for (dirty, *why), n in causes(log):
        print(f"  over-interval: {n:4} dirty={dirty} {' '.join(why)}")
    failed = over > 0 or p99 > P99_MS
    print("FAIL" if failed else "PASS")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
