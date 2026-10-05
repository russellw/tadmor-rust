#!/usr/bin/env python3
"""Maintain vendor/, the committed crate sources the build uses.

.cargo/config.toml replaces crates.io with vendor/ and turns the network
off, so this tool is the only thing that talks to crates.io
(docs/stack.md, "Supply-chain posture"):

  tools/vendor.py sync [--update]
                          Resolve Cargo.toml online, keeping Cargo.lock's
                          versions where they still fit (or, with
                          --update, re-resolving everything to the newest
                          versions). Hold back any crate published less
                          than 7 days ago to its newest older compatible
                          version, then replace vendor/ and Cargo.lock and
                          write dependencies.json.
  tools/vendor.py check   Verify vendor/ against Cargo.lock, offline: every
                          locked crate present with the lock's checksum,
                          every vendored file matching its recorded
                          SHA-256, nothing extra, nothing hidden from git
                          by an ignore rule, and dependencies.json listing
                          exactly the crates the build uses.
  tools/vendor.py manifest
                          Write dependencies.json, the manifest tadmor's
                          tools/measure.py reads (tadmor's
                          docs/counterpart-metrics.md): every crate the
                          linux/x64 build uses, its category, and the
                          crates.io owners who can publish it, looked up
                          online. sync runs it.

Cargo.lock lists the crates for every platform. Crates that only other
platforms use (windows-sys and the like) are vendored as stubs: their
Cargo.toml alone, which Cargo still reads to load the lock file, with no
code. They are not built here, so they are not in the manifest, and the
cooldown does not apply to them.

Categories follow the metrics doc. runtime is what links into the server:
the normal dependencies, not looking inside procedural macros. build is
what else runs while building: build scripts' dependencies, proc macros,
and everything only they use. test is what only dev-dependencies add.

sync works in a temporary copy of Cargo.toml and Cargo.lock, with no
vendor configuration, so if it fails the repository is left as it was.

Standard library only.
"""

import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor"
LOCK = ROOT / "Cargo.lock"
MANIFEST = ROOT / "dependencies.json"
# Publication dates never change, so they are cached between runs.
PUBLISHED_CACHE = ROOT / "target" / "vendor-published.json"
TARGET = "x86_64-unknown-linux-gnu"
API = "https://crates.io/api/v1/crates/"
USER_AGENT = "tadmor-rust tools/vendor.py (https://github.com/russellw/tadmor-rust)"
COOLDOWN = datetime.timedelta(days=7)
CHECKSUM_FILE = ".cargo-checksum.json"

_last_call = 0.0


def api(path):
    """GET a crates.io API path. crates.io's crawler policy asks for at most
    one request a second and a User-Agent that says who is calling."""
    global _last_call
    time.sleep(max(0.0, _last_call + 1.0 - time.monotonic()))
    request = urllib.request.Request(API + path, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as resp:
        body = json.load(resp)
    _last_call = time.monotonic()
    return body


def cargo(cwd, *args):
    result = subprocess.run(["cargo", *args], cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"cargo {' '.join(args)} failed:\n{result.stderr}")
    return result.stdout


TREE_LINE = re.compile(r"(\d+)(\S+) v(\S+)(.*)")


def tree(cwd, edges):
    """(depth, name, version, is_proc_macro) for each line of cargo tree,
    without deduplication, so every path is walked."""
    out = cargo(cwd, "tree", "--locked", "-e", edges, "--target", TARGET,
                "--prefix", "depth", "--no-dedupe", "--format", "{p}")
    rows = []
    for line in out.splitlines():
        m = TREE_LINE.match(line)
        if m:
            rows.append((int(m[1]), m[2], m[3], "(proc-macro)" in m[4]))
    return rows


def categorized(cwd):
    """(name, version) -> category, for every crate the linux/x64 build uses."""
    runtime = set()
    skip_below = None
    for depth, name, version, proc_macro in tree(cwd, "normal"):
        if skip_below is not None and depth > skip_below:
            continue
        skip_below = None
        if proc_macro:
            skip_below = depth
            continue
        if depth > 0:
            runtime.add((name, version))
    built = {(n, v) for d, n, v, _ in tree(cwd, "normal,build") if d > 0}
    everything = {(n, v) for d, n, v, _ in tree(cwd, "normal,build,dev") if d > 0}
    out = {key: "runtime" for key in runtime}
    out.update({key: "build" for key in built - runtime})
    out.update({key: "test" for key in everything - built})
    return out


def locked(lock_path):
    """(name, version) -> checksum of every crates.io package in a Cargo.lock."""
    packages = tomllib.loads(lock_path.read_text()).get("package", [])
    return {(p["name"], p["version"]): p["checksum"] for p in packages
            if p.get("source", "").startswith("registry+")}


def published_dates():
    try:
        return json.loads(PUBLISHED_CACHE.read_text())
    except (OSError, ValueError):
        return {}


def published(name, version, cache):
    key = f"{name} {version}"
    if key not in cache:
        cache[key] = api(f"{name}/{version}")["version"]["created_at"]
    return datetime.datetime.fromisoformat(cache[key])


def compatible(a, b):
    """Whether two versions are semver-compatible in Cargo's sense: the same
    leftmost non-zero component."""
    pa, pb = [x.split("-")[0].split("+")[0].split(".") for x in (a, b)]
    for i in range(3):
        if pa[i] != pb[i]:
            return False
        if pa[i] != "0":
            return True
    return True


def semver_key(version):
    return tuple(int(x) for x in version.split("-")[0].split("+")[0].split("."))


def hold_back(work):
    """Downgrade every built crate younger than the cooldown to its newest
    older, compatible, unyanked, stable version, until none is left."""
    cache = published_dates()
    now = datetime.datetime.now(datetime.timezone.utc)
    try:
        for _ in range(10):
            young = [(n, v) for n, v in sorted(categorized(work))
                     if now - published(n, v, cache) < COOLDOWN]
            if not young:
                return
            for name, version in young:
                versions = api(f"{name}/versions")["versions"]
                for v in versions:
                    cache[f"{name} {v['num']}"] = v["created_at"]
                candidates = [
                    v["num"] for v in versions
                    if not v["yanked"] and "-" not in v["num"]
                    and compatible(v["num"], version) and semver_key(v["num"]) < semver_key(version)
                    and now - datetime.datetime.fromisoformat(v["created_at"]) >= COOLDOWN
                ]
                if not candidates:
                    raise SystemExit(f"{name} {version} is inside the 7-day cooldown and has no older compatible version")
                older = max(candidates, key=semver_key)
                print(f"holding back {name} {version} -> {older} (cooldown)")
                result = subprocess.run(["cargo", "update", f"{name}@{version}", "--precise", older],
                                        cwd=work, capture_output=True, text=True)
                if result.returncode != 0:
                    raise SystemExit(f"cannot hold back {name} {version} to {older}; "
                                     f"something requires the newer version:\n{result.stderr}")
        raise SystemExit("cooldown hold-backs did not settle after 10 rounds")
    finally:
        PUBLISHED_CACHE.parent.mkdir(exist_ok=True)
        PUBLISHED_CACHE.write_text(json.dumps(cache, indent=0, sort_keys=True) + "\n")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stub(crate_dir):
    """Cut a crate that is never built on linux/x64 down to its Cargo.toml
    and an empty library file, which Cargo needs to see a target."""
    package = json.loads((crate_dir / CHECKSUM_FILE).read_text())["package"]
    lib = tomllib.loads((crate_dir / "Cargo.toml").read_text()).get("lib", {}).get("path", "src/lib.rs")
    for entry in crate_dir.iterdir():
        if entry.name not in ("Cargo.toml", CHECKSUM_FILE):
            shutil.rmtree(entry) if entry.is_dir() else entry.unlink()
    (crate_dir / lib).parent.mkdir(parents=True, exist_ok=True)
    (crate_dir / lib).write_text("")
    files = {"Cargo.toml": sha256(crate_dir / "Cargo.toml"), lib: sha256(crate_dir / lib)}
    (crate_dir / CHECKSUM_FILE).write_text(json.dumps({"files": files, "package": package}))


def is_stub(crate_dir, files):
    """Whether a vendored crate is a stub: its Cargo.toml and one empty file."""
    rest = files - {"Cargo.toml"}
    return len(rest) == 1 and (crate_dir / next(iter(rest))).stat().st_size == 0


def sync(update):
    with tempfile.TemporaryDirectory(prefix="tadmor-vendor-") as tmp:
        work = Path(tmp)
        shutil.copyfile(ROOT / "Cargo.toml", work / "Cargo.toml")
        (work / "src").mkdir()
        (work / "src" / "main.rs").write_text("fn main() {}\n")
        if update:
            cargo(work, "generate-lockfile")
        else:
            if LOCK.exists():
                shutil.copyfile(LOCK, work / "Cargo.lock")
            cargo(work, "metadata", "--format-version", "1")  # resolves only what changed
        hold_back(work)

        cargo(work, "vendor", "--locked", "--versioned-dirs", str(work / "vendor"))
        built = categorized(work)
        stubs = 0
        for crate_dir in sorted((work / "vendor").iterdir()):
            if split_dir(crate_dir) not in built:
                stub(crate_dir)
                stubs += 1

        shutil.rmtree(VENDOR, ignore_errors=True)
        shutil.move(work / "vendor", VENDOR)
        shutil.copyfile(work / "Cargo.lock", LOCK)
    print(f"vendored {len(built)} crates, and {stubs} other-platform crates as stubs, into vendor/")
    manifest()
    check()


def split_dir(crate_dir):
    """(name, version) of a --versioned-dirs directory, read from its
    Cargo.toml since both names and versions may contain hyphens."""
    package = tomllib.loads((crate_dir / "Cargo.toml").read_text())["package"]
    return package["name"], package["version"]


def owners(name):
    return sorted(f"crates.io:{u['login']}" for u in api(f"{name}/owners")["users"])


def text_lines(path):
    try:
        return sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line.strip())
    except (UnicodeDecodeError, ValueError):
        return 0


def manifest():
    """Writes dependencies.json (tadmor's docs/counterpart-metrics.md)."""
    cats = categorized(ROOT)
    packages = []
    for (name, version), category in sorted(cats.items(), key=lambda kv: (kv[1], kv[0])):
        packages.append({
            "ecosystem": "crates.io", "name": name, "version": version, "category": category,
            "identities": owners(name), "evidence": f"{API}{name}/owners",
        })
    sources = []
    for category in ("runtime", "build", "test"):
        files = [f for (n, v), c in cats.items() if c == category
                 for f in (VENDOR / f"{n}-{v}").rglob("*") if f.is_file() and f.name != CHECKSUM_FILE]
        if files:
            sources.append({"label": f"crates.io crates ({category}, vendored)",
                            "bytes": sum(f.stat().st_size for f in files),
                            "lines": sum(text_lines(f) for f in files)})
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=True).stdout.split()[1]
    doc = {
        "format": "tadmor-dependencies/1",
        "generator": "tools/vendor.py manifest (tadmor-rust)",
        "platform": "linux/x64",
        "toolchains": [f"Rust {rustc} and Cargo (Ubuntu's rustc and cargo packages)"],
        "packages": packages,
        "sources": sources,
    }
    MANIFEST.write_text(json.dumps(doc, indent=1) + "\n")
    print(f"wrote {MANIFEST.relative_to(ROOT)}: {len(packages)} crates")


def check():
    problems = []
    lock = locked(LOCK)
    built = categorized(ROOT)
    dirs = {}
    for crate_dir in sorted(VENDOR.iterdir()):
        if not crate_dir.is_dir():
            problems.append(f"vendor/{crate_dir.name} is not a crate directory")
            continue
        dirs[split_dir(crate_dir)] = crate_dir
    for key in sorted(dirs.keys() - lock.keys()):
        problems.append(f"{dirs[key].relative_to(ROOT)} is not in Cargo.lock")
    for key in sorted(lock.keys() - dirs.keys()):
        problems.append(f"{key[0]} {key[1]} is in Cargo.lock but not vendored")

    for key, crate_dir in sorted(dirs.items()):
        if key not in lock:
            continue
        rel = crate_dir.relative_to(ROOT)
        recorded = json.loads((crate_dir / CHECKSUM_FILE).read_text())
        if recorded["package"] != lock[key]:
            problems.append(f"{rel} has a different package checksum from Cargo.lock")
        present = {str(f.relative_to(crate_dir)) for f in crate_dir.rglob("*")
                   if f.is_file() and f.name != CHECKSUM_FILE}
        for f in sorted(present - recorded["files"].keys()):
            problems.append(f"{rel}/{f} is not in its {CHECKSUM_FILE}")
        for f, digest in sorted(recorded["files"].items()):
            if f not in present:
                problems.append(f"{rel}/{f} is missing")
            elif sha256(crate_dir / f) != digest:
                problems.append(f"{rel}/{f} does not match its recorded SHA-256")
        stubbed = is_stub(crate_dir, present)
        if key in built and stubbed:
            problems.append(f"{rel} is a stub but the linux/x64 build uses it; run tools/vendor.py sync")
        if key not in built and not stubbed:
            problems.append(f"{rel} is vendored in full but only other platforms use it; run tools/vendor.py sync")

    ignored = subprocess.run(["git", "ls-files", "--others", "--ignored", "--exclude-standard", "--", "vendor"],
                             cwd=ROOT, capture_output=True, text=True).stdout.split()
    for f in ignored:
        problems.append(f"{f} is hidden from git by an ignore rule, so a clean clone would not have it")

    if not MANIFEST.exists():
        problems.append("dependencies.json is missing; run tools/vendor.py manifest")
    else:
        listed = {(p["name"], p["version"]): p["category"] for p in json.loads(MANIFEST.read_text())["packages"]}
        if listed != built:
            problems.append("dependencies.json does not list the crates the build uses; run tools/vendor.py manifest")
    if problems:
        raise SystemExit("\n".join(problems))
    print(f"vendor/ matches Cargo.lock ({len(built)} crates built on linux/x64, "
          f"{len(dirs) - len(built)} other-platform stubs)")


def main():
    args = sys.argv[1:]
    if args in (["sync"], ["sync", "--update"]):
        sync(update=len(args) == 2)
    elif args == ["check"]:
        check()
    elif args == ["manifest"]:
        manifest()
    else:
        raise SystemExit("usage: tools/vendor.py sync [--update] | check | manifest")


if __name__ == "__main__":
    main()
