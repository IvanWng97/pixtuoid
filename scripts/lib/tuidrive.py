#!/usr/bin/env python3
"""Drive an interactive agent TUI under a pty: type a prompt, answer the gate, quit.

`just capture-fixture` records what a CLI sends. Some of what we need to record
only EXISTS in the full-screen TUI — every CLI's `-p`/`run` mode either
auto-approves or declines without asking, so a permission event has no other
door. This is that door, and it is blind by nature: everything it sees is logged
raw to `$TUIDRIVE_LOG` (a private temp file otherwise) and every answer
attempt is announced in order.

    scripts/lib/tuidrive.py "<prompt>" <cmd> [args...]
    DENY_FROM=1 scripts/lib/tuidrive.py ...   # refuse the 2nd gate onward
    scripts/lib/tuidrive.py --selftest        # the pure logic, no pty

Four things here were each a lost turn before they were code:

1. **A pty with no window size renders into a 0x0 viewport.** The TUI draws
   nothing, and the run looks like the CLI hung. `TIOCSWINSZ` first.
2. **Text and Enter must go SEPARATELY.** Sent as one write, a TUI reads a
   PASTE and the newline lands inside the buffer instead of submitting.
3. **Enter during startup is swallowed** (codex was loading an MCP server), and
   the prompt then sits in the composer forever. The composer still holding the
   text IS the retry signal.
4. **Answer a gate in ANY state.** Claude Code asks whether the folder is
   trusted BEFORE it will accept a prompt at all, and typing into that menu
   loses the turn.

Per-CLI knowledge that does NOT belong here — which flag, which subcommand,
which mode gates — lives in the caller, because a table of ten CLIs' flags in
one file drifts silently and a drifted row captures the wrong thing while still
looking like evidence. The callers are the `capture-*.sh` siblings here.
"""

import fcntl
import os
import pty
import re
import select
import struct
import sys
import tempfile
import termios
import time

ANSI = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()][A-B0-2]")
# Gate wording seen across these TUIs. Deliberately broad: a miss costs a billed
# turn, a false positive costs one keystroke into a composer.
GATE = re.compile(
    rb"(allow|approve|permission|proceed\?|do you want|y/n|\(y\)|yes.{0,20}no|deny|confirm|trust)",
    re.I,
)
# A numbered menu confirms on Enter (its first option is pre-highlighted); a
# bare y/n gate wants the letter.
# LINE-ANCHORED, and each piece rejects a different thing.
#
# The prefix class admits the selection cursor or box border a real full-screen
# TUI paints (`❯ 1. Yes`, `│ 1. Allow │`) — a bare `\s*` rejected every one of
# those. It EXCLUDES `*#+-` because a plain `[^\w\n]*` then matched ordinary
# prose in the same buffer: `- 1. Fixed a crash`, `* 1. see the changelog`,
# `# 1. Overview`. `\s+` after the separator rejects a version, which a line
# anchor alone did not: `1.2.3` starts a line too.
#
# `>` and `|` stay in, and `> 1. quoted prose` is the accepted residual: both are
# real cursor/border glyphs (`> 1. Yes, proceed` is a measured menu), so no prefix
# class rejects the prose without rejecting the menu. Don't "fix" it by dropping
# them — narrow on something other than the prefix if it ever bites.
#
# GATE's cost asymmetry above does NOT apply here — do not reuse it as a reason to
# widen this. A MENU false positive sends Enter into a real y/n gate, which costs
# the same billed turn a miss does, and worse: on a MENU hit the reply is `\r`
# whatever `next_answer` is, so a persistent one disables the escalation ladder.
MENU = re.compile(rb"(?m)^[^\w\n*#+-]*1\s*[.)]\s+\S", re.I)
ANSWERS = [b"y\r", b"\r", b"1\r", b"\x1b[A\r"]
# One billed turn's ceiling, and how long a TUI gets to paint before we type into it.
RUN_BUDGET_S = 300
SETTLE_S = 3


def answer_for(n: int) -> bytes:
    """Clamped: a fifth answer becomes reachable by adding it to ANSWERS."""
    return ANSWERS[min(n, len(ANSWERS) - 1)]


def squash(b: bytes) -> bytes:
    """A TUI redraws its composer padded and wrapped, so compare without spaces."""
    return b"".join(b.split())


def selftest() -> int:
    """The pure halves, which are the ones that go wrong silently.

    `assert` is compiled OUT under `PYTHONOPTIMIZE`/`-O`, which would make this
    REQUIRED gate print "ok" having checked nothing — so the first thing it does
    is prove assertions are live.
    """
    try:
        assert False  # noqa: B011
    except AssertionError:
        pass
    else:
        print(
            "tuidrive selftest: assertions are disabled (PYTHONOPTIMIZE/-O), so this "
            "gate would pass without checking anything",
            file=sys.stderr,
        )
        return 1
    assert ANSI.sub(b"", b"\x1b[1;32mhi\x1b[0m") == b"hi"
    assert ANSI.sub(b"", b"\x1b]0;title\x07go") == b"go"
    assert squash(b"de lete  NO\nTE") == b"deleteNOTE"
    for wording in [b"Do you want to proceed?", b"Allow this command?", b"(y/n)", b"trust"]:
        assert GATE.search(wording), wording
    assert not GATE.search(b"reading NOTE.txt")
    # Every case carries a PREFIX shape: an `\s*`-anchored form passed the two
    # bare ones while missing every cursor/box form a real TUI paints.
    for menu in [
        b"  1. Yes, continue",
        b"1) Approve",
        "❯ 1. Yes".encode(),
        b"> 1. Yes, proceed",
        "│ 1. Yes, allow      │".encode(),
        "│ ❯ 1. Yes  │".encode(),
        "• 1) Approve".encode(),
    ]:
        assert MENU.search(menu), menu
    assert not MENU.search(b"press enter")
    # DISCRIMINATING cases: `rb"1[.]"` passed the menus above while matching these,
    # which sends Enter into a y/n gate — the documented silent-failure mode. The
    # last three start the line, so a line anchor alone does not reject them.
    for prose in [
        b"version 1.2.3",
        b"step 1. first",
        b"took 1.5s",
        b"1.2.3",
        b"1.0.0 released",
        b"1.x",
        # Ordinary prose in the same 4 KB buffer. A `[^\w\n]*` prefix admitted
        # every one of these, and a MENU hit pins the reply to Enter.
        b"- 1. Fixed a crash in the watcher",
        b"* 1. see the changelog",
        b"  - 1. step one",
        b"# 1. Overview",
        b"+ 1. added",
    ]:
        assert not MENU.search(prose), prose

    # The ladder BY VALUE. Asserting `answer_for(0) == ANSWERS[0]` is
    # self-referential: three of the four rungs could be deleted and this stayed
    # green, while the escalation each rung exists for silently stopped happening.
    assert [answer_for(i) for i in range(len(ANSWERS))] == [
        b"y\r",
        b"\r",
        b"1\r",
        b"\x1b[A\r",
    ], ANSWERS
    assert answer_for(99) == b"\x1b[A\r", "past the last rung must clamp, not wrap"
    print("tuidrive selftest: ok")
    return 0


def main() -> int:
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        return selftest()
    if len(sys.argv) < 3:
        sys.exit('usage: tuidrive.py "<prompt>" <cmd> [args...]  |  --selftest')
    prompt, cmd = sys.argv[1], sys.argv[2:]
    deny_from = int(os.environ["DENY_FROM"]) if os.environ.get("DENY_FROM") else None
    # The recorder points this into its private 0700 dir; a fixed shared-temp
    # name is both clobberable and symlink-followable.
    log_path = os.environ.get("TUIDRIVE_LOG")
    if log_path:
        log = open(log_path, "wb", buffering=0)
    else:
        fd, log_path = tempfile.mkstemp(prefix="tuidrive-", suffix=".log")
        log = os.fdopen(fd, "wb", buffering=0)
    print(f"driver transcript: {log_path}", flush=True)

    def note(msg: str) -> None:
        log.write(f"\n<<< {msg} >>>\n".encode())
        print(msg, flush=True)

    pid, fd = pty.fork()
    if pid == 0:
        os.environ.update(TERM="xterm-256color", COLUMNS="120", LINES="40")
        os.execvp(cmd[0], cmd)

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

    seen = b""  # rolling window: a dialog's text arrives split across reads
    state = "settling"
    deadline = time.time() + RUN_BUDGET_S
    settle_until = time.time() + SETTLE_S
    next_answer = last_gate = typed_at = last_submit = submits = 0

    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.5)
        now = time.time()
        if ready:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            log.write(chunk)
            seen = (seen + ANSI.sub(b"", chunk))[-4096:]
            if GATE.search(seen) and now - last_gate > 2.0:
                if deny_from is not None and next_answer >= deny_from:
                    reply = b"\x1b"
                else:
                    reply = b"\r" if MENU.search(seen) else answer_for(next_answer)
                time.sleep(0.8)
                note(f"gate seen, sending {reply!r}")
                os.write(fd, reply)
                next_answer += 1
                last_gate = now
                seen = b""
                if state == "settling":
                    settle_until = now + SETTLE_S
        if state == "settling" and now > settle_until:
            note("typing the prompt")
            os.write(fd, prompt.encode())
            time.sleep(1.0)
            os.write(fd, b"\r")
            state, typed_at, last_submit = "working", now, now
            seen = b""  # the trust dialog must not re-trigger the gate arm
        elif (
            state == "working"
            and submits < 3
            and now - last_submit > 8
            and squash(prompt.encode()) in squash(seen)
        ):
            note("prompt still in the composer; pressing enter again")
            os.write(fd, b"\r")
            submits += 1
            last_submit = now
        elif state == "working" and now - typed_at > 90:
            note("turn window elapsed; quitting")
            # `/exit` FIRST: a SIGINT kill skips the CLI's own session_end, which
            # is the whole shape a clean-exit scenario exists to record.
            for keys in (b"/exit\r", b"\x03", b"\x03", b"\x04"):
                os.write(fd, keys)
                time.sleep(0.7)
            state = "quitting"
        elif state == "quitting" and now - typed_at > 108:
            break

    os.close(fd)
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        pass
    note(f"answers attempted: {next_answer}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
