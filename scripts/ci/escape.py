#!/usr/bin/env python3
"""The escape tests (docs/ci.md, "Security"): documents that try to run
commands or write where they must not, built by TeX Live and by partex in
each of its modes under the CI's settings, and checks of the sandbox
itself. Any probe file made, any command run, a host file seen, or the
network reachable fails the suite.

    escape.py --bin PARTEX --formats DIR --out RESULTS.json [--canary PATH]

Run inside the CI container (scripts/accl/ci.sbatch, stage torture) or
inside scripts/sandbox.
"""

import argparse
import os
import shutil
import socket
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

W = Path(os.environ.get("CI_WORK", "/w"))
ROOT = W / "escape"
OUTSIDE = W / "escape-outside"

PRE = "\\documentclass{article}\n\\begin{document}\nx\n"
POST = "\n\\end{document}\n"
# (name, body, kind): `shell` probes must log the command as disabled and
# make no file; `write` probes must be refused (TeX's "I can't write on
# file"); `read` probes print ABSENT or PRESENT
PROBES = [
    ("write18", "\\immediate\\write18{touch PWNED-write18}\n"
                "\\immediate\\write18{bibtex PWNED-allowed}\n"
                "\\immediate\\write18{kpsewhich x; touch PWNED-chain}\n"
                "\\write18{touch PWNED-shipout}\n", "shell"),
    ("shellescape", "\\typeout{SHELLESCAPE=\\the\\pdfshellescape}\n", "shell"),
    ("pipe-out", "\\immediate\\openout5=|\"touch PWNED-pipeout\"\n"
                 "\\immediate\\write5{x}\\immediate\\closeout5\n", "shell"),
    ("pipe-in", "\\immediate\\openin5=|\"touch PWNED-pipein\"\n"
                "\\ifeof5\\typeout{PIPE=ABSENT}\\else\\typeout{PIPE=PRESENT}\\fi\n", "shell"),
    ("input-pipe", "\\input|\"touch PWNED-inputpipe\"\n", "shell"),
    ("dotfile", "\\immediate\\openout5=.PWNED-dot\\immediate\\write5{x}\\immediate\\closeout5\n", "write"),
    ("absolute", f"\\immediate\\openout5={OUTSIDE}/PWNED-abs\\immediate\\write5{{x}}\\immediate\\closeout5\n", "write"),
    ("parent", "\\immediate\\openout5=../PWNED-up\\immediate\\write5{x}\\immediate\\closeout5\n", "write"),
    ("dotdot", "\\immediate\\openout5=..PWNED-dots\\immediate\\write5{x}\\immediate\\closeout5\n", "write"),
]


def sides(binary, formats):
    """(name, command for MAIN, extra env, engine)."""
    def compat(engine, flag=True):
        flavor, fmt = ("xetex", "xelatex") if engine == "xelatex" else ("pdftex", "pdflatex")
        c = [binary, "--compat=tex", f"-engine={flavor}", f"-fmt={fmt}", "-interaction=nonstopmode"]
        return c + (["-no-shell-escape"] if flag else [])
    px = {"PARTEX_CACHE_DIR": str(W / "escape-cache"), "PARTEX_STORE_DIR": str(W / "escape-store"),
          "TEXFORMATS": f"{formats}:", "PARTEX_FORMATS": str(formats), "NO_COLOR": "1"}
    xe = {"FONTCONFIG_FILE": str(common.HERE.parent / "xetex" / "fonts.conf")}
    return [
        ("texlive-pdflatex", ["pdflatex", "-no-shell-escape", "-interaction=nonstopmode"], {}, "pdflatex"),
        ("texlive-pdflatex-cnf", ["pdflatex", "-interaction=nonstopmode"], {}, "pdflatex"),
        ("texlive-xelatex", ["xelatex", "-no-shell-escape", "-interaction=nonstopmode"], xe, "xelatex"),
        ("partex-plain", compat("pdflatex"), px, "pdflatex"),
        ("partex-plain-cnf", compat("pdflatex", False), px, "pdflatex"),
        ("partex-machine", compat("pdflatex"), {**px, "PARTEX_MACHINE": "1"}, "pdflatex"),
        ("partex-ssa", compat("pdflatex"), {**px, "PARTEX_SSA": "1"}, "pdflatex"),
        ("partex-xetex", compat("xelatex"), {**px, **xe, "PARTEX_XETEX": "1"}, "xelatex"),
        ("partex-build", [binary, "build", "--no-shell-escape", "--color", "never"], {**px, "PARTEX_MACHINE": "0"}, "pdflatex"),
        ("partex-build-cnf", [binary, "build", "--color", "never"], px, "pdflatex"),
    ]


def pwned():
    return sorted(str(p.relative_to(W)) for p in W.rglob("*")
                  if p.name.startswith(("PWNED", ".PWNED", "..PWNED")))


def probe_run(side, probe):
    name, cmd, extra, engine = side
    pname, body, kind = probe
    if engine == "xelatex" and pname == "shellescape":
        body = "\\typeout{SHELLESCAPE=\\the\\shellescape}\n"
    d = ROOT / name / pname / "job"
    shutil.rmtree(ROOT / name / pname, ignore_errors=True)
    shutil.rmtree(OUTSIDE, ignore_errors=True)
    d.mkdir(parents=True)
    OUTSIDE.mkdir(parents=True)
    (d / "probe.tex").write_text(PRE + body + POST)
    r = common.run([*cmd, "probe.tex"], d, common.env({"HOME": str(W / "home"), **extra}), 300,
                   out=d / "term.txt")
    text = b""
    for f in ("probe.log", "term.txt"):
        try:
            text += (d / f).read_bytes()
        except OSError:
            pass
    made = pwned()
    out = {"side": name, "probe": pname, "exit": r["exit"], "made": made}
    ok = not made and not r["timeout"]
    if kind == "shell" and pname == "write18":
        # (every \write18 logged as not run)
        ok = ok and text.count(b"runsystem(") >= 3 and b")...executed" not in text
    if kind == "shell" and pname == "shellescape":
        ok = ok and b"SHELLESCAPE=0" in text
    if kind == "write":
        ok = ok and b"I can't write on file" in text
    out["pass"] = ok
    return out


def sandbox_checks(canary):
    checks = []
    bad = [k for k in os.environ if k == "SSH_AUTH_SOCK" or any(
        s in k.upper() for s in ("TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "API_KEY")) or k.startswith(("GIT_", "GH_", "AWS_"))]
    checks.append({"check": "no secrets in the environment", "pass": not bad, "found": bad})
    try:
        socket.create_connection(("1.1.1.1", 53), timeout=3).close()
        net = True
    except OSError:
        net = False
    try:
        socket.getaddrinfo("github.com", 443)
        dns = True
    except OSError:
        dns = False
    checks.append({"check": "no network", "pass": not net and not dns})
    if canary:
        checks.append({"check": "the host's files are not visible", "pass": not Path(canary).exists()})
    try:
        (Path("/usr/share") / "partex-ci-write-test").write_text("x")
        ro = False
    except OSError:
        ro = True
    checks.append({"check": "the image is read-only", "pass": ro})
    return checks


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--formats", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--canary")
    a = ap.parse_args()
    results = []
    for side in sides(a.bin, a.formats):
        for probe in PROBES:
            r = probe_run(side, probe)
            results.append(r)
            print(f"{'ok  ' if r['pass'] else 'FAIL'} {r['side']:22} {r['probe']:12} exit {r['exit']} {r['made'] or ''}", flush=True)
            for p in r["made"]:
                (W / p).unlink(missing_ok=True)
    checks = sandbox_checks(a.canary)
    for c in checks:
        print(f"{'ok  ' if c['pass'] else 'FAIL'} sandbox: {c['check']}")
    common.write_json(a.out, {"probes": results, "sandbox": checks})
    sys.exit(0 if all(r["pass"] for r in results + checks) else 1)


if __name__ == "__main__":
    main()
