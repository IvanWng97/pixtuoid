#!/usr/bin/env python3
"""Record, or check, every input of the committed wasm build (`just gen-wasm`).

A rebuild cannot prove `site/public/wasm/` fresh: its bytes differ across rustc
versions, and CI builds with latest stable. Its inputs do not, so the record
lists them and `check` fails when any has moved since the last `just gen-wasm`.
The toolchain is deliberately not an input: an artifact any stable rustc built
from these inputs is fresh.

`write` takes the input files from the dep-info cargo wrote beside the artifact,
which lists exactly what that build read: every source, `include_*!` asset and
build script of the workspace crates it compiled, and every path a build script
reruns on. Code compiled out by cfg is never read, so never listed. `check`
reads the same roots back from the committed record and needs no compile: a new
module or asset only enters the build through a recorded file (`mod`,
`include_*!`, `#[path]`), a recorded directory, or a new crate target.

Record lines:
  build    the package, target and profile compiled
  profile  that profile's settings along its `inherits` chain
  crate    a compiled target of a workspace crate: kinds, edition, root file
  dep      a resolved package and its features (`cargo tree`) — the lockfile
           and manifests as this build sees them, minus our own crates' version,
           which a release bump moves without touching the wasm
  dir      a directory a build script reruns on, re-listed on every check
  file     the sha256 of a file the build read
"""

import argparse
import difflib
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Build configuration cargo reads but never lists in dep-info. The toolchain file
# is not here: `wasm-build` names its toolchain itself.
CARGO_CONFIG = ".cargo/config.toml"

# The target kinds that link into the artifact; a crate's tests, benches and
# examples never do.
LINKED_KINDS = {"lib", "rlib", "cdylib", "dylib", "staticlib", "proc-macro", "custom-build"}

REGEN = "run 'just gen-wasm' and commit all of site/public/wasm/"


def die(msg: str) -> None:
    sys.exit(f"wasm-inputs: {msg}")


def cargo(*args: str) -> str:
    return subprocess.run(
        ["cargo", *args], cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True
    ).stdout


def rel(path: str) -> str:
    """`path` relative to the repo; absolute when outside it, so CI cannot find it."""
    p = Path(path)
    p = p if p.is_absolute() else ROOT / p
    for root in (ROOT, ROOT.resolve()):
        if p.is_relative_to(root):
            return p.relative_to(root).as_posix()
    return p.as_posix()


def listed(directory: str) -> set[str]:
    """The files of `directory` a commit can carry: a gitignored one never reaches CI."""
    out = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", directory],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout
    # --cached still lists a file deleted from the worktree but not the index.
    return {f for f in out.split("\0") if f and (ROOT / f).is_file()}


def graph(package: str, target: str, meta: dict) -> tuple[list[str], set[str]]:
    """The `dep` lines, and which workspace crates the build compiles."""
    local = {
        p["name"]: f"{p['name']} v{p['version']} ({Path(p['manifest_path']).parent})"
        for p in meta["packages"]
    }
    # Resolved for this package alone, as the build resolves it: a feature or
    # dependency only the native binary enables is not an input.
    tree = cargo(
        "tree", "--locked", "-p", package, "--target", target, "-e", "normal,build",
        "--prefix", "none", "--no-dedupe", "--format", "{p} {f}",
    )
    deps, crates = set(), set()
    for line in tree.splitlines():
        name = line.split(" ", 1)[0]
        if name in local and line.startswith(local[name]):
            line = name + line[len(local[name]):]
            crates.add(name)
        deps.add(f"dep {line.rstrip()}")
    if package not in crates:
        die(f"`cargo tree -p {package}` did not list {package} as a workspace crate")
    return sorted(deps), crates


def profile_line(profile: str) -> str:
    profiles = tomllib.loads((ROOT / "Cargo.toml").read_text()).get("profile", {})
    chain, name = {}, profile
    while name in profiles:
        chain[name] = profiles[name]
        name = profiles[name].get("inherits")
    if profile not in chain:
        die(f"Cargo.toml defines no [profile.{profile}]")
    return f"profile {json.dumps(chain, sort_keys=True)}"


def metadata() -> dict:
    return json.loads(cargo("metadata", "--no-deps", "--format-version", "1", "--locked"))


def derive(files: set[str], dirs: set[str], args: argparse.Namespace, meta: dict) -> str:
    deps, crates = graph(args.package, args.target, meta)
    targets = sorted(
        f"crate {p['name']} {','.join(sorted(kinds))} {t['edition']} {rel(t['src_path'])}"
        for p in meta["packages"]
        if p["name"] in crates
        for t in p["targets"]
        if (kinds := set(t["kind"]) & LINKED_KINDS)
    )
    lines = [f"build {args.package} {args.target} {args.profile}", profile_line(args.profile)]
    lines += targets + deps + [f"dir {d}" for d in sorted(dirs)]
    for f in sorted(files.union(*map(listed, dirs))):
        path = ROOT / f
        digest = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else "missing"
        lines.append(f"file {digest} {f}")
    return "\n".join(lines) + "\n"


def write(args: argparse.Namespace) -> None:
    meta = metadata()
    target_dir = rel(meta["target_directory"])
    files, dirs = set(), set()
    for line in Path(args.dep_info).read_text().splitlines():
        if line.startswith("#") or ": " not in line:
            continue
        # Make-syntax: entries are space-separated, a space inside one is `\ `.
        for entry in re.split(r"(?<!\\) +", line.split(": ", 1)[1].strip()):
            path = rel(entry.replace("\\ ", " "))
            # Build-script output: derived from the build script and the
            # directories it reruns on, both recorded.
            if path == target_dir or path.startswith(target_dir + "/"):
                continue
            if (ROOT / path).is_dir():
                dirs.add(path)
            elif (ROOT / path).is_file():
                files.add(path)
            else:
                die(f"{args.dep_info} lists {entry}, which is neither a file nor a directory")
    if not files:
        die(f"{args.dep_info} lists no input file")
    if (ROOT / CARGO_CONFIG).is_file():
        files.add(CARGO_CONFIG)
    Path(args.record).write_text(derive(files, dirs, args, meta))


def check(args: argparse.Namespace) -> None:
    recorded = Path(args.record).read_text()
    lines = recorded.splitlines()
    dirs = {line.split(" ", 1)[1] for line in lines if line.startswith("dir ")}
    files = {
        f
        for line in lines
        if line.startswith("file ")
        for f in [line.split(" ", 2)[2]]
        if not any(f.startswith(d + "/") for d in dirs)
    }
    # An empty root set would re-derive an equally empty record and pass.
    if not files:
        die(f"{args.record} records no input file — {REGEN}")
    current = derive(files, dirs, args, metadata())
    if current != recorded:
        sys.stdout.writelines(
            difflib.unified_diff(
                recorded.splitlines(keepends=True),
                current.splitlines(keepends=True),
                f"{args.record} (last gen-wasm)",
                "the checkout now",
            )
        )
        die(f"the wasm build's inputs moved since the last gen-wasm — {REGEN}")
    print(f"wasm-inputs: {args.record} matches the checkout ({len(files)} recorded roots)")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--package", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--profile", required=True)
    sub = parser.add_subparsers(dest="cmd", required=True)
    w = sub.add_parser("write", help="derive the record from a fresh build's dep-info")
    w.add_argument("dep_info")
    w.add_argument("record")
    c = sub.add_parser("check", help="fail if the checkout no longer matches the record")
    c.add_argument("record")
    args = parser.parse_args()
    write(args) if args.cmd == "write" else check(args)


if __name__ == "__main__":
    main()
