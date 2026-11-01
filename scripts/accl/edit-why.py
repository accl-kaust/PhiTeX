#!/usr/bin/env python3
"""The edit classes' reruns explained (scripts/accl/edit-why runs it).

    edit-why.py OBS_BIN WORKDIR RESULTS [class...]

Each class of edit-classes.py named (default: label, heading, preamble
def) saved into a watch at -j 8 and reverted; the stderr kept in
RESULTS/why.err. Per class it prints: the wall, the rounds with runs and
wall each, the runs by op, the reruns by op, cause and field (from the
`rerunop:` lines) and the loop-back fields published with a new value
(from the `round-trace:` lines, against the cold build's last value).
"""
import collections
import difflib
import importlib.util
import os
import re
import shutil
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("ec", os.path.join(here, "edit-classes.py"))
ec = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ec)


CAUSES = ["Text", "Anchor", "Changed", "Rebind", "Refuted", "Woken", "Loop", "Environment", "Output", "Claim", "SiblingScan",
          "Range", "Scope", "Other"]


def main():
    binary, work, res = sys.argv[1:4]
    want = sys.argv[4:] or ["label", "heading", "preamble def"]
    ec.CLASSES = [c for c in ec.CLASSES if c[0] in want]
    os.makedirs(work, exist_ok=True)
    ams = os.path.join(os.getcwd(), "bench/amsldoc")
    cache = os.path.join(work, "cache")
    os.makedirs(cache, exist_ok=True)
    d = os.path.join(work, "cls")
    env = dict(ec.fresh(ams, d, cache), PHITEX_TEX_TIME="1", PHITEX_ROUND_ACCT="1", PHITEX_RERUN_OPS="1",
               PHITEX_ROUND_TRACE="1")
    errf = os.path.join(res, "why.err")
    # (each edit's PDF kept, to compare with a cold build of its text)
    put = ec.Watch.put
    pdfs = []

    def keep(self, text):
        if pdfs and pdfs[-1][1] is None:
            pdfs[-1][1] = open(self.path[:-4] + ".pdf", "rb").read()
        pdfs.append([text, None])
        return put(self, text)

    ec.Watch.put = keep
    (cold, ev0), out = ec.classes(binary, d, 8, env, errf, True)
    ec.Watch.put = put
    for ext in ("aux", "out", "toc", "idx", "ind", "log"):
        if os.path.exists(os.path.join(d, "amsldoc." + ext)):
            shutil.copy(os.path.join(d, "amsldoc." + ext), res)
    print(f"cold {cold:.1f} s ok {ev0 and ev0.get('ok')}")
    # (the edits are pdfs[0], [2], ...: each against a cold -j 8 build of
    # its text, the bytes and the text pdftotext gives)
    for (name, _, _), (text, pdf) in zip(ec.CLASSES, pdfs[0::2]):
        cd = os.path.join(work, "cold-" + name.replace(" ", "-"))
        ec.fresh(ams, cd, cache)
        open(os.path.join(cd, "amsldoc.tex"), "w", encoding="latin-1").write(text)
        subprocess.run([binary, "build", "--exact-pdf", "-j", "8", "amsldoc.tex"], cwd=cd,
                       env=dict(ec.ENV, PHITEX_CACHE_DIR=cache), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=1800)
        cold_pdf = os.path.join(cd, "amsldoc.pdf")
        wp = os.path.join(res, f"watch-{name.replace(' ', '-')}.pdf")
        open(wp, "wb").write(pdf or b"")
        shutil.copy(cold_pdf, os.path.join(res, f"cold-{name.replace(' ', '-')}.pdf"))
        same = ec.md5(wp) == ec.md5(cold_pdf)
        print(f"{name}: watch PDF vs cold build of its text: bytes {'same' if same else 'differ'}")
    text = open(errf, encoding="utf-8", errors="replace").read()
    first = text.find("=== CLASS")
    last = {}
    for m in re.finditer(r"^round-trace: (\S+) ([0-9a-f]{16}) node", text[:first], re.M):
        last[m.group(1)] = m.group(2)
    for (name, _, _), (wall, ev, _) in zip(ec.CLASSES, out):
        m = re.search(re.escape(f"=== CLASS {name}\n") + r"(.*?)" + re.escape(f"=== REVERT {name}\n"), text, re.S)
        seg = m.group(1) if m else ""
        print(f"\n## {name}: wall {wall * 1e3:.0f} ms ok {ev and ev.get('ok')}")
        for l in re.findall(r"^acct-edit: wall.*", seg, re.M):
            print("  " + l[:600])
        for l in re.findall(r"^(?:rounds:|round guesses:|round rerun causes:|  reruns by field|  evaluate again).*", seg, re.M):
            print("  " + l[:400])
        by = collections.Counter()
        for line in re.findall(r"^rerunop: .*$", seg, re.M):
            op = line.split("[", 1)[1].split(" ", 1)[0].rstrip("]")
            whys = re.findall(r"\] (" + "|".join(CAUSES) + r")\b", line)
            f = line.rsplit(" ", 1)[-1]
            by[(op, whys[-1] if whys else "?", "" if f.endswith(("}", ")")) or f in CAUSES else f)] += 1
        print("  reruns by op, cause, field:")
        for (op, why, f), n in by.most_common(40):
            print(f"    {n:5} {op} {why} {f}")
        moved = collections.Counter()
        seen = dict(last)
        for f, h in re.findall(r"^round-trace: (\S+) ([0-9a-f]{16}) node", seg, re.M):
            if seen.get(f) != h:
                moved[re.sub(r"[:@][^:]*$", ":*", f) if f.startswith(("wdef:", "wunit:")) else f] += 1
            seen[f] = h
        print(f"  loop-backs published with a new value (vs the build before), by field: {moved.most_common(30)}")
        last = seen


if __name__ == "__main__":
    main()
