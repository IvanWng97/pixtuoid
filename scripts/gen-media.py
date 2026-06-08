#!/usr/bin/env python3
"""Regenerate every committed office image from a release `snapshot` build.

Single source of truth for BOTH surfaces' render media — replaces the old
scripts/gen-docs-images.py (docs/images/) and site/scripts/gen-demos.sh
(site/public/demos/). Every render job lives in scripts/media.json; this driver
builds the binary once and runs each job, writing to docs/images/ and/or
site/public/demos/ per the job's `targets`. Theme/weather lists are read from
site/src/{themes,weather}.json (`@themes.json` / `@weather.json` refs) so they
are never duplicated.

  just gen-media           # regenerate everything
  just gen-media --only docs   # docs/images/ only
  just gen-check           # → gen-media.py --check (drift gate; see below)

--check renders to a temp dir and pixel-diffs every committed PNG (threshold 0,
via scripts/compare-screenshots.py); video clips (.mp4/.webm) and the animated
demo.gif are presence-checked only (ffmpeg/gifsicle output is not byte-stable
across versions, but the underlying renders are pixel-deterministic). Exits
non-zero on any drift.

Requires the .venv (Pillow) + ffmpeg + gifsicle. Run via `.venv/bin/python3`.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SNAP = ROOT / "target/release/examples/snapshot"
SITE_SRC = ROOT / "site/src"
MANIFEST = ROOT / "scripts/media.json"
COMPARE = ROOT / "scripts/compare-screenshots.py"

TARGET_DIRS = {
    "docs": ROOT / "docs/images",
    "site": ROOT / "site/public/demos",
}
# --check writes pixel-diff overlays here (survives the run for CI artifact upload).
DIFF_DIR = ROOT / "target/gen-check-diff"
# Committed files under docs/images/ that this pipeline does NOT generate
# (a live-agent capture and a hand-made banner) — never compared in --check.
NOT_GENERATED = {"screenshot-real.png", "sprite-banner.png"}


def build_once():
    """cargo no-ops when fresh; a stale binary silently renders outdated art."""
    subprocess.run(
        ["cargo", "build", "--release", "--example", "snapshot"], cwd=ROOT, check=True
    )


def expand_ref(ref):
    """'@themes.json' -> the parsed site/src/themes.json list."""
    return json.loads((SITE_SRC / ref[1:]).read_text())


def snap(out_path, *, cols, rows, hour, day=None, theme=None, weather=None,
         extra=(), gif=None, env=None):
    cmd = [str(SNAP), "--cols", str(cols), "--rows", str(rows), "--now-hour", str(hour)]
    if day is not None:
        cmd += ["--now-day", str(day)]
    if theme is not None:
        cmd += ["--theme", theme]
    if weather is not None:
        cmd += ["--weather", weather]
    if gif is not None:
        cmd += ["--gif", "--gif-duration", str(gif["duration"]), "--gif-fps", str(gif["fps"])]
    cmd += list(extra)
    cmd += [str(out_path)]
    # suppress the text preview on stdout; gif progress stays on stderr
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, env=env)


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", *args], check=True)


# ── per-kind handlers ────────────────────────────────────────────────────────


def run_render(job, out_dirs, work, intermediates):
    env = {**os.environ, "TZ": "UTC"} if job.get("tz") == "UTC" else None

    if "frames" in job:  # the reference baselines: several named frames, one job
        for f in job["frames"]:
            for d in out_dirs:
                snap(d / f"{f['name']}.png", cols=job["cols"], rows=job["rows"],
                     hour=f["hour"], day=job.get("day"), theme=f.get("theme"),
                     weather=f.get("weather"), env=env)
        return

    raw = work / f"{job['id']}_raw.png"
    snap(raw, cols=job["cols"], rows=job["rows"], hour=job["hour"], day=job.get("day"),
         theme=job.get("theme"), weather=job.get("weather"), extra=job.get("extra", ()),
         env=env)
    intermediates[job["id"]] = raw  # crops read the unscaled render

    scale = job.get("scale")
    for d in out_dirs:
        dst = d / f"{job['id']}.png"
        if scale:
            img = Image.open(raw).convert("RGB")
            img.resize((img.width * scale, img.height * scale), Image.NEAREST).save(dst)
        else:
            shutil.copyfile(raw, dst)


def run_crop(job, out_dirs, work, intermediates):
    src = intermediates[job["from"]]
    if "quadrants" in job:  # docs: fractional quadrants → {id}-{key}.png, Pillow upscale
        img = Image.open(src).convert("RGB")
        w, h = img.size
        scale = job.get("scale", 1)
        for name, (x0, y0, x1, y1) in job["quadrants"].items():
            crop = img.crop((int(w * x0), int(h * y0), int(w * x1), int(h * y1)))
            out = crop.resize((crop.width * scale, crop.height * scale), Image.NEAREST)
            for d in out_dirs:
                out.save(d / f"{job['id']}-{name}.png")
    else:  # site: ffmpeg pixel crops → {id}_{key}.png
        for key, spec in job["crops"].items():
            for d in out_dirs:
                ffmpeg("-i", str(src), "-vf", f"crop={spec}", str(d / f"{job['id']}_{key}.png"))


def run_composite(job, out_dirs, work, intermediates):
    themes = [t["id"] for t in expand_ref(job["over"])]
    slant = job["slant"]
    paths = []
    for i, theme in enumerate(themes):
        p = work / f"composite_{i}.png"
        snap(p, cols=job["cols"], rows=job["rows"], hour=job["hour"], day=job.get("day"),
             theme=theme)
        paths.append(p)

    comp = Image.open(paths[0]).convert("RGB")
    w, h = comp.size
    n = len(themes)
    half = h / 2
    far = w + abs(slant) * h + 10

    def boundary(k, y):  # x of the k-th band boundary at row y (centre-anchored)
        return k * w / n + slant * (y - half)

    for i in range(n):
        im = Image.open(paths[i]).convert("RGB")
        lt = -far if i == 0 else boundary(i, 0)
        lb = -far if i == 0 else boundary(i, h)
        rt = far if i == n - 1 else boundary(i + 1, 0)
        rb = far if i == n - 1 else boundary(i + 1, h)
        mask = Image.new("L", (w, h), 0)
        ImageDraw.Draw(mask).polygon([(lt, 0), (rt, 0), (rb, h), (lb, h)], fill=255)
        comp.paste(im, (0, 0), mask)
    for d in out_dirs:
        comp.save(d / "themes-composite.png")


def run_gif(job, out_dirs, work, intermediates):
    if not shutil.which("gifsicle"):
        sys.exit("gifsicle not found — brew install gifsicle")
    for d in out_dirs:
        dst = d / f"{job['id']}.gif"
        snap(dst, cols=job["cols"], rows=job["rows"], hour=job["hour"], day=job.get("day"),
             theme=job.get("theme"), gif={"duration": job["duration"], "fps": job["fps"]})
        # Palette reduction (NOT --lossy: it breaks gifsicle's inter-frame diff and
        # ships a bigger file). Same params as the old gen-docs-images.py.
        subprocess.run(
            ["gifsicle", "-b", "-O3", "--colors", str(job["colors"]), str(dst)], check=True
        )


def run_matrix(job, out_dirs, work, intermediates):
    items = [x["id"] for x in expand_ref(job["over"])]
    axis = job["axis"]  # "theme" | "weather"
    for item in items:
        for d in out_dirs:
            kwargs = {"theme": item} if axis == "theme" else {"weather": item}
            snap(d / f"{axis}_{item}.png", cols=job["cols"], rows=job["rows"],
                 hour=job["hour"], **kwargs)


def run_clip(job, out_dirs, work, intermediates):
    gif = work / f"{job['id']}.gif"
    snap(gif, cols=job["cols"], rows=job["rows"], hour=job["hour"],
         extra=job.get("extra", ()), gif={"duration": job["duration"], "fps": job["fps"]})
    fps = job["fps"]
    cid = job["id"]
    for d in out_dirs:
        frames = work / f"frames-{cid}"
        frames.mkdir(exist_ok=True)
        # re-encode from frames so it's a true loop at `fps` (the GIF's own frame
        # delays otherwise confuse ffmpeg into a fast clip).
        ffmpeg("-i", str(gif), str(frames / "f%04d.png"))
        scale = "scale=trunc(iw/2)*2:trunc(ih/2)*2"
        ffmpeg("-framerate", str(fps), "-i", str(frames / "f%04d.png"),
               "-movflags", "+faststart", "-pix_fmt", "yuv420p", "-vf", scale,
               str(d / f"{cid}.mp4"))
        ffmpeg("-framerate", str(fps), "-i", str(frames / "f%04d.png"),
               "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "36", "-row-mt", "1",
               "-pix_fmt", "yuv420p", "-vf", scale, str(d / f"{cid}.webm"))
        ffmpeg("-i", str(gif), "-vframes", "1", str(d / f"{cid}-poster.png"))


HANDLERS = {
    "render": run_render,
    "crop": run_crop,
    "composite": run_composite,
    "gif": run_gif,
    "matrix": run_matrix,
    "clip": run_clip,
}


# ── drift check ──────────────────────────────────────────────────────────────


def run_check(out_base, work, only=None):
    """Pixel-diff every generated PNG vs committed; presence-check video + gif."""
    failures = []
    DIFF_DIR.mkdir(parents=True, exist_ok=True)
    for target, tdir in out_base.items():
        if only and target != only:
            continue
        committed_dir = TARGET_DIRS[target]
        generated = sorted(p for p in tdir.iterdir() if p.is_file())

        # every committed generated file must have been (re)generated
        committed_expected = {
            p.name for p in committed_dir.iterdir()
            if p.is_file() and p.name not in NOT_GENERATED
        }
        produced = {p.name for p in generated}
        for missing in sorted(committed_expected - produced):
            failures.append(f"NOT REGENERATED: {target}/{missing}")

        for f in generated:
            committed = committed_dir / f.name
            suf = f.suffix.lower()
            if not committed.exists():
                failures.append(f"NEW (uncommitted) output: {target}/{f.name}")
                continue
            if suf in (".mp4", ".webm", ".gif"):
                print(f"  present (not pixel-gated): {target}/{f.name}")  # ffmpeg/gifsicle
                continue
            diff = DIFF_DIR / f"diff-{target}-{f.name}"
            rc = subprocess.run(
                [sys.executable, str(COMPARE), str(committed), str(f), str(diff)]
            ).returncode
            if rc != 0:
                failures.append(f"PIXEL DRIFT: {target}/{f.name} (compare rc={rc})")

    print()
    if failures:
        print(f"\033[31mgen-check FAILED — {len(failures)} issue(s):\033[0m")
        for x in failures:
            print(f"  ✗ {x}")
        return 1
    print("\033[32mgen-check OK — every committed artifact is in sync.\033[0m")
    return 0


# ── driver ───────────────────────────────────────────────────────────────────


def main():
    ap = argparse.ArgumentParser(description="Regenerate office media from scripts/media.json")
    ap.add_argument("--check", action="store_true",
                    help="render to a temp dir and diff vs committed; write nothing")
    ap.add_argument("--only", choices=["docs", "site"], help="restrict to one surface")
    ap.add_argument("--jobs", help="comma-separated job ids to run (default: all)")
    args = ap.parse_args()

    only_jobs = set(args.jobs.split(",")) if args.jobs else None

    build_once()
    manifest = json.loads(MANIFEST.read_text())
    work = Path(tempfile.mkdtemp(prefix="gen-media-"))

    if args.check:
        out_base = {t: work / f"out-{t}" for t in TARGET_DIRS}
    else:
        out_base = dict(TARGET_DIRS)
    for d in out_base.values():
        d.mkdir(parents=True, exist_ok=True)

    intermediates = {}
    for job in manifest:
        if only_jobs and job["id"] not in only_jobs:
            continue
        targets = [t for t in job["targets"] if not args.only or t == args.only]
        if not targets:
            continue
        out_dirs = [out_base[t] for t in targets]
        print(f"· {job['id']} ({job['kind']}) → {', '.join(targets)}")
        HANDLERS[job["kind"]](job, out_dirs, work, intermediates)

    if args.check:
        rc = run_check(out_base, work, only=args.only)
        shutil.rmtree(work, ignore_errors=True)
        sys.exit(rc)

    shutil.rmtree(work, ignore_errors=True)
    print(f"\nwrote media → {', '.join(str(TARGET_DIRS[t]) for t in (([args.only] if args.only else TARGET_DIRS)))}")


if __name__ == "__main__":
    main()
