"""Shared pieces of the CI harness (docs/ci.md): the pinned environment
every engine run gets, a run with limits, tree hashes.

Everything here runs inside the CI container (scripts/accl/ci.sbatch) or
inside scripts/sandbox: documents are untrusted code, so every engine and
tool run gets shell escape off, kpathsea's paranoid file access, a CPU
and wall-clock limit and no core files, on top of the container (no
network, no home, only the job's scratch writable).
"""

import hashlib
import json
import os
import resource
import shutil
import signal
import subprocess
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent

# One fixed time and locale for both sides (as xtask/src/e2e.rs pins it),
# and the security settings of docs/ci.md: kpathsea reads these variables
# before texmf.cnf, in TeX Live's engines, BibTeX, makeindex and partex.
PINNED = {
    "SOURCE_DATE_EPOCH": "1758800000",
    "FORCE_SOURCE_DATE": "1",
    "TZ": "UTC",
    "LANG": "C.UTF-8",
    "LC_ALL": "C.UTF-8",
    "shell_escape": "f",
    "openout_any": "p",
    "openin_any": "p",
}

# What a run may inherit from the container's environment: nothing else
# (no tokens, no agent sockets, no git credentials).
INHERIT = ("PATH", "HOME", "TMPDIR", "RUSTUP_HOME", "TEXMFVAR", "TEXMFCONFIG")

# Files that are not compared: transcripts (the .log need not match:
# AGENTS.md), tool logs, and the engines' bookkeeping.
NOT_COMPARED = (
    ".log", ".blg", ".ilg", ".glg", ".alg", ".slg", ".fls", ".fdb_latexmk",
    ".synctex.gz", ".synctex", ".fmt", ".xdv",
)


def env(extra=None):
    e = {k: os.environ[k] for k in INHERIT if k in os.environ}
    e.update(PINNED)
    if extra:
        e.update(extra)
    return e


def _limits(cpu_s, as_bytes):
    def apply():
        os.setsid()
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        if cpu_s:
            resource.setrlimit(resource.RLIMIT_CPU, (cpu_s, cpu_s + 5))
        if as_bytes:
            resource.setrlimit(resource.RLIMIT_AS, (as_bytes, as_bytes))
        # (a runaway \write can't fill the scratch disk)
        resource.setrlimit(resource.RLIMIT_FSIZE, (8 << 30, 8 << 30))
    return apply


def run(cmd, cwd, environ, timeout, out=None, cpu_s=None, as_bytes=None, stdin=None):
    """Run `cmd` in `cwd` with limits: wall `timeout` seconds (the whole
    process group killed), CPU `cpu_s` seconds, address space `as_bytes`.
    Its terminal output goes to `out` (a path) or is dropped. Returns
    {exit, wall_s, rss_kib, timeout}."""
    t0 = time.monotonic()
    sink = open(out, "wb") if out else subprocess.DEVNULL
    try:
        p = subprocess.Popen(
            [str(c) for c in cmd], cwd=cwd, env=environ, stdout=sink,
            stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL if stdin is None else stdin,
            preexec_fn=_limits(cpu_s or 8 * timeout, as_bytes),
        )
        timed_out = False
        deadline = t0 + timeout
        while True:
            pid, status, ru = os.wait4(p.pid, os.WNOHANG)
            if pid:
                break
            if time.monotonic() > deadline:
                timed_out = True
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                pid, status, ru = os.wait4(p.pid, 0)
                break
            time.sleep(0.02)
    finally:
        if out:
            sink.close()
    code = os.waitstatus_to_exitcode(status)
    return {
        "exit": code,
        "wall_s": round(time.monotonic() - t0, 3),
        "rss_kib": ru.ru_maxrss,
        "timeout": timed_out,
    }


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def tree_files(root):
    """Every regular file under `root`, relative, sorted (symlinks as
    their targets' contents: TeX reads through them)."""
    root = Path(root)
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for n in sorted(filenames):
            p = Path(dirpath) / n
            if p.is_file():
                out.append(p.relative_to(root).as_posix())
    return out


def tree_hash(root):
    """sha256 over (path, sha256 of contents) of every file under `root`."""
    h = hashlib.sha256()
    for rel in tree_files(root):
        h.update(rel.encode() + b"\0" + sha256_file(Path(root) / rel).encode() + b"\n")
    return h.hexdigest()


def snapshot(root, base=None):
    """{relpath: sha256} of the files under `root` that the job made or
    changed (not in `base`, or with other contents), the compared ones."""
    snap = {}
    for rel in tree_files(root):
        if rel.endswith(NOT_COMPARED) or rel.startswith((".partex", "term")):
            continue
        d = sha256_file(Path(root) / rel)
        if base is None or base.get(rel) != d:
            snap[rel] = d
    return snap


def fresh_copy(src, dst):
    if Path(dst).exists():
        shutil.rmtree(dst)
    # (times kept: \pdffilemoddate and friends see the tree's own)
    shutil.copytree(src, dst, symlinks=False)


def write_json(path, obj):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, indent=1, sort_keys=True) + "\n")
    tmp.replace(path)


def read_json(path, default=None):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default
