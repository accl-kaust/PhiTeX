#!/usr/bin/env python3
"""The PDF file a watch writes while a document is edited (update
sections appended, the exact file written when the editor is idle),
checked: a watch session, then N edits at random places, each saved and
built; after each edit's build the file is kept (K.update.pdf, and the
file before the edit, K.before.pdf), then the exact file the idle write
put over it (K.exact.pdf) and the text (K.tex). After the session each
text is built cold; the idle write's md5 must be the cold build's.
PAIRS then holds what `crates/output/pdf/tests/update.rs`'s
`harness_pairs` reads back (PHITEX_UPDATE_PAIRS).

    gate-update.py BIN DIR MAIN J N SEED OUT_JSON PAIRS [--insert] [--burst B] [FLAG ...]

The edits replace one word (letters only, between blanks, on a line that
is no command's) with another of the same length, as gate-edits.py's;
`--insert` makes every other edit insert a paragraph of words instead.
`--burst B`: then B more word edits saved one after another without the
idle wait (each built as its own update section), their walls apart
(`burst_walls_ms`), the file after the last one checked against the
exact file the idle write puts after it. FLAGs go to both `watch` and
`build` (`--program pdftex`).
"""
import hashlib
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import time

WORD = re.compile(r"(?<=[ ])([a-z]{4,12})(?=[ .,;])")


def candidates(text):
    out = []
    off = 0
    for line in text.split("\n"):
        s = line.lstrip()
        if s and not s.startswith(("\\", "%")) and "\\verb" not in line and "{" not in line:
            for m in WORD.finditer(line):
                out.append((off + m.start(1), m.group(1)))
        off += len(line) + 1
    return out


def other(w, rng):
    while True:
        x = "".join(rng.choice("abcdefghijklmnopqrstuvwxyz") for _ in w)
        if x != w:
            return x


def md5(p):
    try:
        return hashlib.md5(open(p, "rb").read()).hexdigest()
    except OSError:
        return None


def main():
    binary, d, main_, j, n, seed, out, pairs = sys.argv[1:9]
    flags = sys.argv[9:]
    insert = "--insert" in flags
    flags = [f for f in flags if f != "--insert"]
    burst = 0
    if "--burst" in flags:
        i = flags.index("--burst")
        burst = int(flags[i + 1])
        del flags[i:i + 2]
    n, rng = int(n), random.Random(int(seed))
    path = os.path.join(d, main_)
    stem = os.path.splitext(main_)[0]
    pdf = os.path.join(d, stem + ".pdf")
    shutil.rmtree(pairs, ignore_errors=True)
    os.makedirs(pairs)
    res = {"j": int(j), "edits": 0, "walls_ms": [], "section_bytes": [], "kinds": [],
           "burst_walls_ms": [], "burst_section_bytes": []}
    err = open(os.environ["GATE_EDIT_STDERR"], "w") if os.environ.get("GATE_EDIT_STDERR") else subprocess.DEVNULL
    p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", j, main_], cwd=d,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
    pending = []

    def next_event(kind, timeout, build=0):
        # (a write's event names the build it wrote: one of an earlier
        # build is not the one waited for)
        t = time.monotonic()
        for x in list(pending):
            if x.get("event") == kind:
                pending.remove(x)
                if x.get("build", build) >= build:
                    return x
        while time.monotonic() - t < timeout:
            line = p.stdout.readline()
            if not line:
                return None
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") == kind and x.get("build", build) >= build:
                return x
            if x.get("event") == "build":
                pending.append(x)
        return None

    ev = next_event("build", 900)
    text = open(path, encoding="latin-1").read()
    # (the watch's file is exact after its cold build: no idle write)
    time.sleep(0.5)

    def save(t):
        tmp = path + ".tmp"
        open(tmp, "w", encoding="latin-1").write(t)
        os.replace(tmp, path)

    def edit(k, ins):
        nonlocal text
        if ins:
            # (a paragraph of words inserted after a blank line: the
            # pages after it move)
            spots = [m.end() for m in re.finditer(r"\n\n", text)]
            spots = spots[len(spots) // 4: 3 * len(spots) // 4] or spots
            at = rng.choice(spots)
            words = " ".join("".join(rng.choice("abcdefghij") for _ in range(rng.randint(3, 9)))
                             for _ in range(rng.randint(40, 120)))
            text = text[:at] + words + ".\n\n" + text[at:]
        else:
            at, w = rng.choice(candidates(text))
            text = text[:at] + other(w, rng) + text[at + len(w):]
        save(text)

    for k in range(1, n + 1):
        if ev is None:
            break
        ins = insert and k % 2 == 0
        shutil.copyfile(pdf, os.path.join(pairs, f"{k}.before.pdf"))
        before = os.path.getsize(pdf)
        edit(k, ins)
        ev = next_event("build", 900)
        if ev is None:
            break
        shutil.copyfile(pdf, os.path.join(pairs, f"{k}.update.pdf"))
        res["walls_ms"].append(round(ev.get("wall_ms", 0), 3))
        res["section_bytes"].append(os.path.getsize(pdf) - before)
        res["kinds"].append("insert" if ins else "word")
        open(os.path.join(pairs, f"{k}.tex"), "w", encoding="latin-1").write(text)
        w = next_event("written", 30, ev.get("build", 0))
        if w is None and os.path.getsize(pdf) != before:
            res["no_idle_write"] = k
            break
        shutil.copyfile(pdf, os.path.join(pairs, f"{k}.exact.pdf"))
        res["edits"] += 1
    # the burst: edits one after another, then the idle write
    if burst and ev is not None:
        k = n + 1
        shutil.copyfile(pdf, os.path.join(pairs, f"{k}.before.pdf"))
        for _ in range(burst):
            before = os.path.getsize(pdf)
            edit(k, False)
            ev = next_event("build", 900)
            if ev is None:
                break
            res["burst_walls_ms"].append(round(ev.get("wall_ms", 0), 3))
            res["burst_section_bytes"].append(os.path.getsize(pdf) - before)
        if ev is not None:
            shutil.copyfile(pdf, os.path.join(pairs, f"{k}.update.pdf"))
            open(os.path.join(pairs, f"{k}.tex"), "w", encoding="latin-1").write(text)
            w = next_event("written", 30, ev.get("build", 0))
            shutil.copyfile(pdf, os.path.join(pairs, f"{k}.exact.pdf"))
            res["burst_written"] = w is not None
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    try:
        p.wait(timeout=120)
    except subprocess.TimeoutExpired:
        p.kill()
    # each edit's text built cold: its md5 against the idle write's
    cold = d.rstrip("/") + "-cold"
    res["md5"] = []
    ok = res["edits"] == n
    for f in sorted(os.listdir(pairs), key=lambda f: (len(f), f)):
        if not f.endswith(".tex"):
            continue
        k = f[:-4]
        shutil.rmtree(cold, ignore_errors=True)
        os.makedirs(cold)
        shutil.copyfile(os.path.join(pairs, f), os.path.join(cold, main_))
        subprocess.run([binary, "build", *flags, "-j", j, main_], cwd=cold,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800)
        c = md5(os.path.join(cold, stem + ".pdf"))
        x = md5(os.path.join(pairs, f"{k}.exact.pdf"))
        res["md5"].append({"edit": int(k), "idle_write": x, "cold": c, "same": x == c})
        ok = ok and x == c
    words = [w for w, kd in zip(res["walls_ms"], res["kinds"]) if kd == "word"]
    res["word_median_ms"] = statistics.median(words) if words else None
    res["burst_median_ms"] = statistics.median(res["burst_walls_ms"]) if res["burst_walls_ms"] else None
    res["verdict"] = "same" if ok else "DIFFERS" if res["edits"] == n else f"stopped after {res['edits']}"
    json.dump(res, open(out, "w"), indent=1)
    print(f"update file -j {j}: {res['edits']} of {n} edits{' (insert every other)' if insert else ''}, "
          f"idle writes against cold builds {res['verdict']}; word edit median {res['word_median_ms']} ms "
          f"(walls {' '.join(str(w) for w in res['walls_ms'])}); sections {' '.join(map(str, res['section_bytes']))} bytes"
          + (f"; burst of {burst}: median {res['burst_median_ms']} ms, sections "
             f"{' '.join(map(str, res['burst_section_bytes']))} bytes" if burst else ""))


if __name__ == "__main__":
    main()
