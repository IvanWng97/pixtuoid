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
import datetime
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
# The launch bar, owner-set: p99 at most this, and no frame past the paint
# interval (`over`, counted by the binary against PAINT_FRAME_MS).
P99_MS = 20.0
# Cells, and each cell's pixels, giving the owner's 214x125 office at each
# scale (`raw_scale_for_cell`: the cell's width, or half its height).
GEOMETRY = {
    "16": (202, 50, 17, 41),
    "4": (214, 64, 4, 8),
    "classic": (202, 50, 17, 41),
}
# Local dusk, when `nightfall` starts (`sky::SUN_SET_H`).
DUSK = datetime.time(20, 0)
# The lead into the storm transition: a boot and some steady frames first.
LEAD_S = 10


def start_at(run):
    """The Unix second the run's clock starts at."""
    if run == "dusk":
        today = datetime.datetime.combine(datetime.date.today(), DUSK)
        return int(today.timestamp())
    storm = subprocess.run(
        ["cargo", "run", "-q", "--release", "-p", "pixtuoid", "--example", "pacing", "--", "next-storm"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return int(storm.stdout.strip()) - LEAD_S


def run_pty(argv, env, geometry, secs):
    cols, rows, cell_w, cell_h = geometry
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(argv[0], argv, env)
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


def summaries(log):
    for line in log.read_text(errors="replace").splitlines():
        if "frame pacing" in line:
            yield {k: v.strip('"') for k, v in FIELD.findall(line)}


def main():
    ap = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    ap.add_argument("--live", action="store_true")
    ap.add_argument("--scale", choices=sorted(GEOMETRY), default="16")
    ap.add_argument("--graphics", choices=["kitty", "sixel", "iterm2"], default="kitty")
    ap.add_argument("--run", choices=["storm", "dusk"], default="storm")
    ap.add_argument("--secs", type=float, default=200.0)
    args = ap.parse_args()

    log = Path(tempfile.mkstemp(prefix="pace-check-", suffix=".log")[1])
    env = dict(os.environ, PIXTUOID_FAKE_NOW=str(start_at(args.run)), PIXTUOID_LOG=str(log))
    graphics = "off" if args.scale == "classic" else args.graphics
    argv = [str(BIN), "--log-level", "info", "run", "--graphics", graphics]
    if args.live:
        run_live(argv, env, args.secs)
    else:
        # No pty answers the probe: a terminal the environment names plans
        # without one, and `--graphics` then picks the protocol.
        env.update(TERM="xterm-256color", TERM_PROGRAM="WezTerm")
        env.pop("TMUX", None)
        run_pty(argv, env, GEOMETRY[args.scale], args.secs)

    windows = list(summaries(log))
    if not windows:
        sys.exit(f"pace-check: no `frame pacing` summary in {log}")
    w0 = windows[0]
    over = sum(int(w.get("over", 0)) for w in windows)
    p99 = max(float(w.get("p99", 0)) for w in windows)
    frames = sum(int(w.get("frames", 0)) for w in windows)
    terminal = w0.get("terminal") if args.live else "pty"
    print(
        f"{w0.get('look')} x{w0.get('scale')} tmux={w0.get('tmux')} terminal={terminal} "
        f"run={args.run}: {frames} frames, worst window p99 {p99:.1f} ms, over {over} | log {log}"
    )
    failed = over > 0 or p99 > P99_MS
    print("FAIL" if failed else "PASS")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
