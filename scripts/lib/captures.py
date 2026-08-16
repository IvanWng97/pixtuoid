"""THE enumeration of committed captures, for the Python gates.

The twin of `tests/sources/captures.rs`. Both exist because the rules run on
both sides of a language boundary — `fixture-age.py` and `fixture-pii.py` gate
in `lint` and CI, the Rust rules gate in `cargo test` — and neither can call the
other. What must NOT exist is a third walk, or two walks that disagree about the
population: `the_two_capture_walks_agree` (in the Rust suite) fails if they do.

Before this, `fixture-age.py` and `fixture-pii.py` each rolled their own rglob
with different filters, and the Rust side had four more. Every provenance rule
therefore landed on whichever subset its author happened to walk (34 / 42 / 24),
which is one root cause behind the "the fix landed on half the population"
finding that recurred across four review rounds.
"""

from __future__ import annotations

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SOURCES = ROOT / "crates/pixtuoid-core/tests/sources"

# Module dirs whose name is NOT the registered source id. MIRRORS
# `MODULE_TO_SOURCE` in captures.rs; the pair is pinned by the Rust twin test.
MODULE_TO_SOURCE = {"claude": "claude-code"}


class Capture:
    """A committed capture: its dir, the source its LAYOUT names, its record."""

    __slots__ = ("dir", "source", "provenance_path", "provenance", "unusable")

    def __init__(self, prov_path: pathlib.Path):
        self.dir = prov_path.parent
        self.provenance_path = prov_path
        self.source = source_of(self.dir)
        self.unusable: str | None = None
        try:
            doc = json.loads(prov_path.read_text())
        except (OSError, json.JSONDecodeError) as e:
            self.provenance, self.unusable = {}, f"not valid JSON ({e})"
            return
        # Valid JSON that is not an object (`null`, `[]`, a bare number) would
        # otherwise reach the field reads and crash the whole sweep on one file,
        # replacing every other file's diagnostic with a traceback.
        if not isinstance(doc, dict):
            self.provenance = {}
            self.unusable = f"top level is {type(doc).__name__}, not an object"
            return
        self.provenance = doc

    @property
    def origin(self) -> str:
        return str(self.provenance.get("origin", ""))

    @property
    def is_recorded(self) -> bool:
        return self.origin == "recorded"

    def field(self, key: str) -> str:
        return str(self.provenance.get(key, ""))

    @property
    def rel(self) -> str:
        return str(self.provenance_path.relative_to(ROOT))

    def hook_payloads(self) -> pathlib.Path | None:
        p = self.dir / "hook-payloads.jsonl"
        return p if p.is_file() else None

    def transcripts(self) -> list[pathlib.Path]:
        """RECURSIVE: a parent-dir-keyed source nests its transcript one level
        down, and a flat read was blind to exactly those fixtures."""
        return sorted(
            f for f in self.dir.rglob("*.jsonl") if f.name != "hook-payloads.jsonl"
        )

    def wire_files(self) -> list[pathlib.Path]:
        out = self.transcripts()
        if (h := self.hook_payloads()) is not None:
            out.append(h)
        return out


def source_of(capture_dir: pathlib.Path) -> str | None:
    """The registered source a capture dir belongs to, from the LAYOUT alone.

    Three shapes exist: `fixtures/<source>/<scenario>/`, `<module>/fixtures/`,
    and `<module>/fixtures/<sub>/`.
    """
    parts = capture_dir.relative_to(SOURCES).parts
    if len(parts) == 3 and parts[0] == "fixtures":
        raw = parts[1]
    elif len(parts) == 3 and parts[1] == "fixtures":
        raw = parts[2]
    elif len(parts) == 2 and parts[1] == "fixtures":
        raw = parts[0]
    else:
        return None
    return MODULE_TO_SOURCE.get(raw, raw)


def every_capture() -> list[Capture]:
    """THE walk. Every `provenance.json` in the tree, sorted."""
    return [Capture(p) for p in sorted(SOURCES.rglob("provenance.json"))]
