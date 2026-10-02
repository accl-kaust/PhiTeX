#!/usr/bin/env python3
"""Queries over an event log (`PARTEX_EVENTS=DIR partex ...`, the CLI's
`eventlog.rs`): a table of every definition of eqtb and the hash, and of
every command, as raw little-endian columns.

    scripts/events.py DIR summary      counts, lifetimes, the busiest cells
    scripts/events.py DIR setup        the build before its first \\shipout
    scripts/events.py DIR grain        definitions and crossing reads kept
                                       at each grain of span (a command, a
                                       window of N commands, a step between
                                       outer clean points, the setup as one
                                       span)

A definition is *internal* to a span if the cell's next definition is
made in the same span: no read outside the span can reach it, so an index
kept at that grain need not hold it. A definition read after its span
ends has a *crossing* read (at least one edge between spans).
"""
import os
import sys

import numpy as np

NONE = 0xFFFFFFFF


def load(d):
    col = lambda n, t: np.fromfile(os.path.join(d, n), dtype=t)
    L = {
        "kind": col("defs_kind.u8", np.uint8),
        "cell": col("defs_cell.u32", "<u4"),
        "t": col("defs_t.u32", "<u4"),
        "next": col("defs_next.u32", "<u4"),
        "last": col("defs_last.u32", "<u4"),
        "nread": col("defs_nread.u32", "<u4"),
        "level": col("defs_level.u8", np.uint8),
        "cmd_outer": col("cmd_outer.u8", np.uint8),
        "cmd_level": col("cmd_level.u8", np.uint8),
        "ships": col("ships.u32", "<u4"),
    }
    L["kinds"] = open(os.path.join(d, "kinds.txt")).read().split()
    L["names"] = {}
    for line in open(os.path.join(d, "names.txt"), errors="replace"):
        c, _, n = line.rstrip("\n").partition("\t")
        if c.isdigit():
            L["names"][int(c)] = n
    L["commands"] = len(L["cmd_outer"])
    return L


def pct(a, b):
    return f"{100.0 * a / max(b, 1):.1f}%"


def summary(L):
    t, nxt, kind = L["t"], L["next"], L["kind"]
    made = t > 0
    print(f"commands {L['commands']:,}; definitions {int(made.sum()):,} made by commands, "
          f"{int((~made).sum()):,} held before (the format's)")
    for k, name in enumerate(L["kinds"]):
        n = int((made & (kind == k)).sum())
        if n:
            print(f"  {name}: {n:,}")
    # lifetimes: commands until the cell's next definition
    live = made & (nxt != NONE)
    life = (nxt[live] - t[live]).astype(np.int64)
    edges = [1, 2, 4, 16, 64, 256, 1024, 4096, 16384, 65536, 1 << 20, 1 << 32]
    h, _ = np.histogram(life, bins=edges)
    print("lifetime (commands to the next definition):")
    acc = 0
    for lo, hi, n in zip(edges, edges[1:], h):
        acc += n
        print(f"  [{lo:>7}, {hi:>10}): {n:>11,}  cumulative {pct(acc, len(life))}")
    print(f"  never redefined: {int((made & (nxt == NONE)).sum()):,}")
    unread = made & (L["nread"] == 0)
    print(f"never read: {int(unread.sum()):,} ({pct(int(unread.sum()), int(made.sum()))})")
    # the busiest eqtb cells
    eq = made & (kind == 0)
    cells, counts = np.unique(L["cell"][eq], return_counts=True)
    top = np.argsort(counts)[::-1][:20]
    print("busiest eqtb cells: " + ", ".join(
        f"{L['names'].get(int(cells[i]), cells[i])}={int(counts[i]):,}" for i in top))


def setup(L):
    first = int(L["ships"][0]) if len(L["ships"]) else L["commands"]
    t, nxt = L["t"], L["next"]
    made = t > 0
    before = made & (t < first)
    kept = before & ((nxt == NONE) | (nxt >= first))
    print(f"first \\shipout at command {first:,} of {L['commands']:,} ({pct(first, L['commands'])})")
    print(f"definitions made before it {int(before.sum()):,} ({pct(int(before.sum()), int(made.sum()))}); "
          f"its net definitions (reaching the body) {int(kept.sum()):,}")
    body_reads = before & (L["last"] >= first)
    print(f"setup definitions the body reads: {int(body_reads.sum()):,}")


def spans_of(starts, x):
    return np.searchsorted(starts, x, side="right") - 1


def grain(L):
    t, nxt, last = L["t"].astype(np.int64), L["next"].astype(np.int64), L["last"].astype(np.int64)
    made = t > 0
    total = int(made.sum())
    n = L["commands"]
    first = int(L["ships"][0]) if len(L["ships"]) else n
    outer = np.flatnonzero(L["cmd_outer"]) + 1  # command numbers from 1
    grains = [("command", np.arange(1, n + 1))]
    for w in (16, 256, 4096, 65536):
        grains.append((f"window {w}", np.arange(1, n + 1, w)))
    grains.append(("step (outer clean points)", np.unique(np.concatenate(([1], outer)))))
    setup_steps = np.unique(np.concatenate(([1], outer[outer >= first])))
    grains.append(("setup + steps", setup_steps))
    print(f"definitions made by commands: {total:,}")
    print(f"{'grain':<28}{'spans':>12}{'kept':>14}{'kept %':>9}{'crossing reads':>16}")
    tm, nm, lm = t[made], nxt[made], last[made]
    for name, starts in grains:
        s0 = spans_of(starts, tm)
        redefined = nm != NONE
        internal = redefined & (spans_of(starts, np.where(redefined, nm, 0)) == s0)
        kept = int((~internal).sum())
        read = L["nread"][made] > 0
        crossing = int((read & (spans_of(starts, lm) != s0)).sum())
        print(f"{name:<28}{len(starts):>12,}{kept:>14,}{pct(kept, total):>9}{crossing:>16,}")


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    L = load(sys.argv[1])
    for q in sys.argv[2:]:
        {"summary": summary, "setup": setup, "grain": grain}[q](L)
        print()


if __name__ == "__main__":
    main()
