#!/usr/bin/env python3
"""One watch build, then E one-word edits, each timed from the save to the
watch's build event; the watch's stderr (PHITEX_TEX_TIME) kept.

    edit-one.py BIN DIR MAIN J E STDERR_FILE [FLAG ...]
"""
import json
import os
import random
import re
import subprocess
import sys
import time

WORD = re.compile(r"(?<=[ ])([a-z]{4,12})(?=[ .,;])")


def main():
    binary, d, main_, j, e, errf = sys.argv[1:7]
    flags = sys.argv[7:]
    path = os.path.join(d, main_)
    err = open(errf, "w")
    p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", j, main_], cwd=d,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)

    def next_build():
        t = time.monotonic()
        for line in p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") == "build":
                return x, time.monotonic() - t
        return None, time.monotonic() - t

    ev, w = next_build()
    print(f"cold: {w:.3f} s")
    rng = random.Random(7)
    text = open(path, encoding="latin-1").read()
    for _ in range(int(e)):
        ms = list(WORD.finditer(text[len(text) // 3:]))
        m = rng.choice(ms)
        at = len(text) // 3 + m.start(1)
        w0 = m.group(1)
        new = "".join(rng.choice("abcdefghijklmnopqrstuvwxyz") for _ in w0)
        text = text[:at] + new + text[at + len(w0):]
        tmp = path + ".tmp"
        open(tmp, "w", encoding="latin-1").write(text)
        err.write(f"=== edit saved at {time.monotonic():.3f}\n")
        err.flush()
        os.replace(tmp, path)
        ev, w = next_build()
        print(f"edit: {w:.3f} s, event wall {ev and ev.get('wall_ms')}")
    p.stdin.write(b"q\n")
    p.stdin.close()
    p.wait(timeout=120)


if __name__ == "__main__":
    main()
