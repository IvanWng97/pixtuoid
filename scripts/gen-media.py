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
         extra=(), gif=None):
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
    # suppress the text preview on stdout; gif progress stays on stderr.
    # Inherits the process env (TZ=UTC, pinned in main) so renders are deterministic.
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", *args], check=True)


# ── per-kind handlers ────────────────────────────────────────────────────────


def run_render(job, out_dirs, work, intermediates):
    # TZ=UTC is pinned process-wide in main() (snapshot reads --now-hour as a
    # chrono::Local wall time, so every epoch-derived effect — the 10-min weather
    # slot, the city-twinkle/lighting phase — must render under one fixed TZ or a
    # dev box and the UTC CI runner produce different frames; the committed art is
    # UTC too). The `reference` baselines are a multi-frame job; the rest single.
    if "frames" in job:
        for f in job["frames"]:
            for d in out_dirs:
                snap(d / f"{f['name']}.png", cols=job["cols"], rows=job["rows"],
                     hour=f["hour"], day=job.get("day"), theme=f.get("theme"),
                     weather=f.get("weather"))
        return

    raw = work / f"{job['id']}_raw.png"
    snap(raw, cols=job["cols"], rows=job["rows"], hour=job["hour"], day=job.get("day"),
         theme=job.get("theme"), weather=job.get("weather"), extra=job.get("extra", ()))
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
    # A crop reads the unscaled render of its `from` job. With --jobs you can
    # filter that prerequisite out — fail with a useful hint, not a bare KeyError.
    src = intermediates.get(job["from"])
    if src is None:
        sys.exit(
            f"gen-media: crop job '{job['id']}' needs its source render '{job['from']}' "
            f"— include it, e.g. --jobs {job['from']},{job['id']}"
        )
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
        # ships a bigger file). These gifsicle params are the established tuning.
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
    # Optional `crop` (ffmpeg "W:H:X:Y", in the unscaled render's px space) frames
    # a close-up on a fixed region — e.g. the meetings clip onto its meeting room
    # so the chitchat reads (a roaming subject like the pets cat uses a smaller
    # cols/rows render instead, never a crop). Prepended to the even-dims scale.
    # NB: this singular clip-level `crop` is unrelated to the separate
    # kind:"crop" job (run_crop), which reads a plural `crops` dict off a `from`
    # render — different mechanism, different key.
    crop = job.get("crop")
    scale = "scale=trunc(iw/2)*2:trunc(ih/2)*2"
    vf = f"crop={crop},{scale}" if crop else scale
    poster_vf = ["-vf", f"crop={crop}"] if crop else []
    for d in out_dirs:
        frames = work / f"frames-{cid}"
        frames.mkdir(exist_ok=True)
        # re-encode from frames so it's a true loop at `fps` (the GIF's own frame
        # delays otherwise confuse ffmpeg into a fast clip).
        ffmpeg("-i", str(gif), str(frames / "f%04d.png"))
        ffmpeg("-framerate", str(fps), "-i", str(frames / "f%04d.png"),
               "-movflags", "+faststart", "-pix_fmt", "yuv420p", "-vf", vf,
               str(d / f"{cid}.mp4"))
        ffmpeg("-framerate", str(fps), "-i", str(frames / "f%04d.png"),
               "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "36", "-row-mt", "1",
               "-pix_fmt", "yuv420p", "-vf", vf, str(d / f"{cid}.webm"))
        # poster frame: `poster` (seconds into the clip) lets a staged clip
        # (e.g. meetings, whose opening seconds are pre-action) poster on the
        # money shot instead of frame 0. Posters are presence-only in --check.
        poster_seek = ["-ss", str(job["poster"])] if "poster" in job else []
        ffmpeg(*poster_seek, "-i", str(gif), *poster_vf, "-vframes", "1",
               str(d / f"{cid}-poster.png"))


def run_wasm_still(job, out_dirs, work, intermediates):
    # The live-office backdrop's poster (#425): a REAL frame of the pixtuoid-web
    # Office — same seed-3 layout, same looped script, same 320x180 buffer a
    # 16:9 viewport's canvas computes — so the poster→canvas crossfade
    # dissolves in place instead of reframing (the old terminal-render poster
    # was ~1.18:1; cover-cropping it dropped ~60% of its height on wide
    # screens). Deterministic per (t0_ms, advance_ms) under the process TZ=UTC
    # pin, so --check pixel-gates it like every other still.
    subprocess.run(
        ["cargo", "build", "--release", "-p", "pixtuoid-web", "--example", "hero_still"],
        check=True,
        cwd=ROOT,
    )
    for d in out_dirs:
        subprocess.run(
            [str(ROOT / "target/release/examples/hero_still"),
             str(d / f"{job['id']}.png"),
             "--width", str(job["w"]), "--height", str(job["h"]),
             "--t0-ms", str(job["t0_ms"]), "--advance-ms", str(job["advance_ms"])],
            check=True,
        )


HANDLERS = {
    "render": run_render,
    "crop": run_crop,
    "composite": run_composite,
    "gif": run_gif,
    "matrix": run_matrix,
    "clip": run_clip,
    "wasm-still": run_wasm_still,
}


# ── drift check ──────────────────────────────────────────────────────────────


def _presence_only_names(manifest, target):
    """Filenames owned by clip/gif jobs for `target` — ffmpeg/gifsicle outputs
    whose bytes aren't stable cross-version, so they're presence-checked, never
    pixel-gated (and --check skips regenerating them: vp9 encoding blew the CI
    timeout for zero gating value). Derived from the MANIFEST, not a name-shape
    rule: a `-poster.png` suffix rule silently exempted a deterministic render
    still that merely kept a poster name (hero-poster.png, the og:image) when
    its job flipped clip→render (#432 review)."""
    return {
        name
        for job in manifest
        if target in job["targets"]
        for name in _expected_presence_outputs(job)
    }


def _expected_presence_outputs(job):
    """Committed filenames a presence-only (clip/gif) job owns — asserted to EXIST,
    since --check skips regenerating them and walking the committed tree alone can't
    notice one that's missing/renamed/never-generated."""
    if job["kind"] == "gif":
        return [f"{job['id']}.gif"]
    if job["kind"] == "clip":
        return [f"{job['id']}.mp4", f"{job['id']}.webm", f"{job['id']}-poster.png"]
    return []


def run_check(out_base, work, manifest, only=None):
    """Pixel-diff every committed STILL against a fresh render; presence-check the
    ffmpeg/gifsicle outputs (clips/gif/posters) without regenerating them."""
    failures = []
    DIFF_DIR.mkdir(parents=True, exist_ok=True)
    for target, tdir in out_base.items():
        if only and target != only:
            continue
        committed_dir = TARGET_DIRS[target]
        produced = {p.name for p in tdir.iterdir() if p.is_file()}  # stills only
        presence_only = _presence_only_names(manifest, target)

        for c in sorted((p for p in committed_dir.iterdir() if p.is_file()), key=lambda p: p.name):
            name = c.name
            if name in NOT_GENERATED:
                continue
            if name in presence_only:
                print(f"  present (not pixel-gated): {target}/{name}")
                continue
            # a rendered still — must have been regenerated AND pixel-match
            if name not in produced:
                failures.append(f"NOT REGENERATED: {target}/{name}")
                continue
            diff = DIFF_DIR / f"diff-{target}-{name}"
            rc = subprocess.run(
                [sys.executable, str(COMPARE), str(c), str(tdir / name), str(diff)]
            ).returncode
            if rc != 0:
                failures.append(f"PIXEL DRIFT: {target}/{name} (compare rc={rc})")

        # a still rendered but not committed = a new/renamed output to commit
        for name in sorted(produced):
            if not (committed_dir / name).exists():
                failures.append(f"NEW (uncommitted) output: {target}/{name}")

    # Presence-only outputs (clips/gif) are skipped in --check, so the committed-dir
    # walk above can't catch one that's MISSING. Assert them from the MANIFEST — the
    # source of truth for what must exist — so a deleted/renamed/orphaned clip fails.
    for job in manifest:
        for t in job["targets"]:
            if only and t != only:
                continue
            for name in _expected_presence_outputs(job):
                if not (TARGET_DIRS[t] / name).exists():
                    failures.append(f"MISSING (per manifest): {t}/{name}")

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

    # run_check walks the FULL committed tree, so every still owned by a
    # filtered-out job would report "NOT REGENERATED" — spurious failures.
    # Rejecting the combination is the honest contract (CI never passes --jobs).
    if args.check and args.jobs:
        ap.error("--check renders everything and walks the full committed tree; "
                 "--jobs cannot be combined with it (run --check without --jobs)")

    # Pin TZ=UTC for EVERY render so the office's epoch-derived weather slot +
    # lighting/twinkle phase (snapshot reads --now-hour as a chrono::Local wall
    # time) are machine-independent — without this a dev box and the UTC CI runner
    # render different frames and gen-check pixel-diffs them as drift. The committed
    # art under docs/images/ + site/public/demos/ is generated UTC too. (Inherited
    # by every snapshot/ffmpeg subprocess via os.environ.)
    os.environ["TZ"] = "UTC"

    only_jobs = set(args.jobs.split(",")) if args.jobs else None

    # Validate --jobs against the manifest BEFORE the release build: an unknown
    # id used to be a silent no-op that still printed "wrote media → …".
    manifest = json.loads(MANIFEST.read_text())
    if only_jobs:
        known = {j["id"] for j in manifest}
        unknown = sorted(only_jobs - known)
        if unknown:
            sys.exit(
                f"gen-media: unknown job id(s): {', '.join(unknown)}\n"
                f"available: {', '.join(sorted(known))}"
            )

    build_once()
    work = Path(tempfile.mkdtemp(prefix="gen-media-"))

    if args.check:
        out_base = {t: work / f"out-{t}" for t in TARGET_DIRS}
    else:
        out_base = dict(TARGET_DIRS)
    for d in out_base.values():
        d.mkdir(parents=True, exist_ok=True)

    intermediates = {}
    try:
        for job in manifest:
            if only_jobs and job["id"] not in only_jobs:
                continue
            targets = [t for t in job["targets"] if not args.only or t == args.only]
            if not targets:
                continue
            # --check pixel-gates stills only; clips/gif are presence-checked from
            # the manifest, so don't waste minutes rendering + vp9-encoding them.
            if args.check and job["kind"] in ("gif", "clip"):
                print(f"· {job['id']} ({job['kind']}) → presence-only, skipped in --check")
                continue
            out_dirs = [out_base[t] for t in targets]
            print(f"· {job['id']} ({job['kind']}) → {', '.join(targets)}")
            HANDLERS[job["kind"]](job, out_dirs, work, intermediates)

        if args.check:
            sys.exit(run_check(out_base, work, manifest, only=args.only))

        surfaces = [args.only] if args.only else list(TARGET_DIRS)
        print(f"\nwrote media → {', '.join(str(TARGET_DIRS[t]) for t in surfaces)}")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
