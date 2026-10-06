#!/usr/bin/env python3
"""The engine suites of the scoreboard (docs/ci.md), on a checkout of the
commit under test, each case recorded as passing or not:

  trip, trip-machine, etrip, etrip-machine
                   Knuth's and e-TeX's torture tests (`cargo xtask trip`,
                   `cargo xtask etrip`, plain and machine mode)
  e2e, e2e-machine `cargo xtask e2e`, each case
  ssa-edits        `scripts/ssa-edits --brief --fixpoint`, each case
  l3build-latex2e, l3build-latex3
                   l3build's .lvt/.pvt tests run by l3build with TeX
                   Live's pdfTeX (`cargo xtask oracle`, cached per image
                   and upstream pins) and with partex in its place (`--partex`):
                   a test passes when every file l3build made of it (the
                   normalized log, the PDF test's output) is the same
  security         the escape tests (escape.py)

    torture.py --src DIR --bin PARTEX --formats DIR --out DIR [--only S,...]
               [--l3build-cache DIR --l3build-new DIR]
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tarfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

SUITES = ["trip", "etrip", "e2e", "ssa-edits", "l3build", "security"]


def sh(cmd, cwd, log, env=None, timeout=7200):
    e = dict(os.environ)
    if env:
        e.update(env)
    r = common.run(cmd, cwd, e, timeout, out=log)
    return r, Path(log).read_text(errors="replace")


def case(suite, cid, ok, detail=""):
    return {"suite": suite, "id": cid, "status": "pass" if ok else "fail", "detail": detail[:400]}


def tail(text, n=12):
    return "\n".join(text.splitlines()[-n:])


def trip_suites(src, out):
    res = []
    sh(["cargo", "xtask", "oracle", "--engine", "tex", "--filter", "^trip"], src, out / "trip-oracle.log")
    sh(["cargo", "xtask", "etrip", "--oracle"], src, out / "etrip-oracle.log")
    for suite, cmd in (("trip", ["trip"]), ("trip-machine", ["trip", "--machine"]),
                       ("etrip", ["etrip"]), ("etrip-machine", ["etrip", "--machine"])):
        r, text = sh(["cargo", "xtask", *cmd], src, out / f"{suite}.log")
        res.append(case(suite, suite, r["exit"] == 0, "" if r["exit"] == 0 else tail(text)))
    return res


LINE = re.compile(r"^(ok|DIFFERS|FAILED) +([\w./-]+)(?:: (.*))?$", re.M)


def e2e_suites(src, out):
    res = []
    for suite, env in (("e2e", {}), ("e2e-machine", {"PARTEX_MACHINE": "1", "XTASK_E2E_DIR": "target/e2e-machine"})):
        r, text = sh(["cargo", "xtask", "e2e"], src, out / f"{suite}.log", env)
        found = LINE.findall(text)
        for st, name, detail in found:
            res.append(case(suite, name, st == "ok", detail or ""))
        if not found:
            res.append(case(suite, "(run)", False, tail(text)))
    return res


def ssa_suite(src, out):
    work = src / "target" / "ssa-edits-ci"
    r, text = sh(["python3", "scripts/ssa-edits", "--brief", "--fixpoint", "--bin", "target/release/partex",
                  "--work", str(work)], src, out / "ssa-edits.log")
    data = common.read_json(work / "results.json", {})
    res = []
    for c in data.get("cases", []):
        res.append(case("ssa-edits", c["case"], c.get("ok"), c.get("error") or c.get("panic") or ""))
    if not res:
        res.append(case("ssa-edits", "(run)", False, tail(text)))
    # (the stage timings, for perf.py's history)
    common.write_json(out / "ssa-edits-results.json", data)
    return res


def l3build_suite(src, out, cache, new, jobs):
    """l3build tests on pdfTeX: TeX Live's results (cached) against partex's."""
    key = common.read_json(src / "corpus" / "manifest.json", {}).get("upstream", {})
    stamp = "l3build-" + subprocess.run(
        ["sha256sum"], input=json.dumps(key, sort_keys=True).encode()
        + (src / "xtask/src/oracle.rs").read_bytes(), capture_output=True).stdout.decode()[:16]
    refs = src / "refs"
    cached = Path(cache) / stamp / "refs.tar" if cache else None
    if cached and cached.exists():
        with tarfile.open(cached) as t:
            t.extractall(src, filter="data")
    else:
        sh(["cargo", "xtask", "corpus"], src, out / "l3build-corpus.log")
        sh(["cargo", "xtask", "oracle", "--engine", "pdftex", "--filter", "^latex", "--jobs", str(jobs)],
           src, out / "l3build-oracle.log", timeout=4 * 3600)
        if new:
            (Path(new) / stamp).mkdir(parents=True, exist_ok=True)
            with tarfile.open(Path(new) / stamp / "refs.tar", "w") as t:
                t.add(refs, arcname="refs")
    r, text = sh(["cargo", "xtask", "oracle", "--engine", "pdftex", "--filter", "^latex", "--partex",
                  "--jobs", str(jobs)], src, out / "l3build-partex.log", timeout=4 * 3600)
    theirs = common.read_json(refs / "index.json", {}).get("results", {})
    mine = common.read_json(src / "target/partex-refs/index.json", {}).get("results", {})
    res = []
    for tid in sorted(theirs):
        o = theirs[tid].get("pdftex")
        if not o or o["status"] in ("no-oracle", "not-run", "no-output"):
            continue
        suite = "l3build-" + tid.split("/", 1)[0]
        p = mine.get(tid, {}).get("pdftex")
        if not p or p["status"] == "no-output":
            res.append(case(suite, tid, False, "no output"))
            continue
        stem = Path(tid).stem
        differ = []
        for f in sorted(set(o["files"]) | set(p["files"])):
            if f == f"{stem}.log" or f.endswith((".diff", ".fmt")):
                continue  # (the raw log; l3build's normalized one is compared)
            a = refs / "pdftex" / Path(tid).with_suffix("") / f
            b = src / "target/partex-refs/pdftex" / Path(tid).with_suffix("") / f
            if not (a.exists() and b.exists() and a.read_bytes() == b.read_bytes()):
                differ.append(f)
        detail = ", ".join(differ) + f" (TeX Live: {o['status']}, partex: {p['status']})"
        res.append(case(suite, tid, not differ, "" if not differ else detail))
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--bin", required=True)
    ap.add_argument("--formats", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only")
    ap.add_argument("--l3build-cache")
    ap.add_argument("--l3build-new")
    ap.add_argument("--canary")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 8)
    a = ap.parse_args()
    src, out = Path(a.src), Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    only = a.only.split(",") if a.only else SUITES
    res = []
    steps = {
        "trip": lambda: trip_suites(src, out),
        "etrip": lambda: [],  # (with trip)
        "e2e": lambda: e2e_suites(src, out),
        "ssa-edits": lambda: ssa_suite(src, out),
        "l3build": lambda: l3build_suite(src, out, a.l3build_cache, a.l3build_new, a.jobs),
    }
    for s in only:
        if s == "security":
            r = common.run([sys.executable, str(common.HERE / "escape.py"), "--bin", a.bin, "--formats", a.formats,
                            "--out", str(out / "escape.json"), *(["--canary", a.canary] if a.canary else [])],
                           src, dict(os.environ), 3600, out=out / "escape.log")
            data = common.read_json(out / "escape.json", {})
            for p in data.get("probes", []):
                res.append(case("security", f"{p['side']}/{p['probe']}", p["pass"], " ".join(p["made"])))
            for c in data.get("sandbox", []):
                res.append(case("security", "sandbox/" + c["check"], c["pass"], json.dumps(c.get("found", ""))))
            if not data:
                res.append(case("security", "(run)", False, tail((out / "escape.log").read_text(errors="replace"))))
            continue
        try:
            got = steps[s]()
        except Exception as e:  # (a suite that cannot run fails alone)
            got = [case(s, "(run)", False, f"{type(e).__name__}: {e}")]
        res.extend(got)
        print(f"{s}: {sum(c['status'] == 'pass' for c in got)}/{len(got)} pass", flush=True)
        common.write_json(out / "torture.json", {"cases": res})
    common.write_json(out / "torture.json", {"cases": res})


if __name__ == "__main__":
    main()
