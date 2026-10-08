#!/usr/bin/env python3
"""Summarise a pure SSA tracer report (`PARTEX_PURE=FILE`, DESIGN 3.17).

    scripts/pure-report.py FILE.json [FILE.json ...]

Prints the node counts by kind (commands by their command code's name,
expandable primitives, recorded calls, build_page), the critical paths,
the page-chain numbers and the projected memory.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FUNCS = ["Start", "Step", "Tokenize", "Hpack", "Vpack", "LineBreak", "ShipOut",
         "WriteOut", "PageStep", "Output", "FontFile", "Encoding", "FontDict"]


def command_names():
    """Command codes to names, from partex-engine's web.rs (first name wins,
    the letter/other/spacer codes named as tex.web's)."""
    src = (ROOT / "crates/partex-engine/src/web.rs").read_text()
    vals, names = {}, {}
    src = src[src.index("pub const RELAX: i32"):]
    for m in re.finditer(r"pub const ([A-Z_0-9]+): i32 = ([^;]+);", src):
        name, expr = m.group(1), m.group(2).strip()
        try:
            v = eval(expr, {}, vals)  # noqa: S307 (constants of our own source)
        except Exception:  # noqa: BLE001
            continue
        if not isinstance(v, int):
            continue
        vals[name] = v
        if name == "MAX_COMMAND":
            break
    for name, v in vals.items():
        if name not in ("MAX_NON_PREFIXED_COMMAND", "MIN_INTERNAL", "MAX_INTERNAL"):
            names.setdefault(v, name)
    for name in ["UNDEFINED_CS", "EXPAND_AFTER", "NO_EXPAND", "INPUT", "IF_TEST",
                 "FI_OR_ELSE", "CS_NAME", "CONVERT", "THE", "TOP_BOT_MARK"]:
        m = re.search(rf"pub const {name}: i32 = MAX_COMMAND \+ (\d+);", src)
        if m:
            names[vals["MAX_COMMAND"] + int(m.group(1))] = name
    names[11] = "LETTER"
    names[12] = "OTHER_CHAR"
    names[10] = "SPACER"
    return names


def kind_name(k, names):
    if k == 1023:
        return "setup"
    if k >= 768:
        return "build_page"
    if k >= 512:
        i = k - 512
        return "call " + (FUNCS[i] if i < len(FUNCS) else str(i))
    if k >= 256:
        return "expand " + names.get(k - 256, str(k - 256))
    return names.get(k, str(k))


def mb(b):
    return f"{b / 1e6:,.1f} MB"


def report(path, names):
    r = json.loads(Path(path).read_text())
    n, w = r["nodes"], r["events"]
    print(f"== {path}")
    print(f"nodes {n:,}  events {w:,}  macro calls (steps) {r['macro_calls']:,}  "
          f"conditionals {r['conditionals']:,}  phis {r['phis']:,}")
    print(f"operands {r['ext_reads']:,}  definitions {r['net_writes']:,}  "
          f"appends {r['appends']:,}  whole-list reads {r['list_reads']:,}  "
          f"structural reads/writes {r['struct_reads']:,}/{r['struct_writes']:,}  "
          f"unversioned nodes {r['unversioned_nodes']:,}  anomalies {r['anomalies']}")
    print(f"critical path: {r['critical_nodes']:,} nodes ({n / max(1, r['critical_nodes']):.1f}x), "
          f"{r['critical_events']:,} events ({w / max(1, r['critical_events']):.1f}x)")
    if r.get("setup_nodes"):
        bw, bc = w - r["setup_events"], r["critical_events"] - r["setup_critical_events"]
        print(f"setup (before the first shipout): {r['setup_nodes']:,} nodes, {r['setup_events']:,} events, "
              f"path {r['setup_critical_events']:,} events; after it: work {bw:,} events, path grew by "
              f"{bc:,} ({bw / max(1, bc):.1f}x)")
    print(f"page chain: {r['page_nodes']:,} nodes, {r['page_events']:,} events "
          f"({100 * r['page_events'] / max(1, w):.1f}% of the work); its own path "
          f"{r['page_path_events']:,} events")
    print(f"body nodes {r['body_nodes']:,}: reading a page-chain definition "
          f"{r['body_reading_page']:,} ({100 * r['body_reading_page'] / max(1, r['body_nodes']):.3f}%), "
          f"downstream of the page chain {r['body_after_page']:,} "
          f"({100 * r['body_after_page'] / max(1, r['body_nodes']):.1f}%)")
    if "body_tail_reads" in r:
        print(f"body reads of the main vertical list's tail made by the page chain: "
              f"{r['body_tail_reads']:,} ({'predicted' if r['spec_tail'] else 'operands'})")
    print(f"memory: unfolded {mb(r['bytes_unfolded'])}, folded {mb(r['bytes_folded'])} "
          f"({r['folds']:,} folds, {r['fold_cuts']:,} cuts; {r['nodes'] / max(1, r['folds']):.2f} nodes a fold)")
    print("page-chain definitions body nodes read (address, nodes):")
    for a, c, *_ in r["body_page_addrs"][:20]:
        print(f"  {c:>9,}  {a}")
    print("what made body nodes downstream of the page chain (first cause, nodes):")
    for a, c, *_ in r.get("body_after_via", [])[:15]:
        print(f"  {c:>9,}  {a}")
    if r.get("critical_path_edges"):
        print(f"the critical path's edges ({r['critical_path_walked']:,} walked), by operand:")
        for a, c, *_ in r["critical_path_edges"][:20]:
            print(f"  {c:>9,}  {a}")
    print("nodes by kind (nodes, events, operands, definitions):")
    for k, nodes, ev, rd, wr in r["kinds"][:40]:
        print(f"  {kind_name(k, names):<28} {nodes:>12,} {ev:>14,} {rd:>12,} {wr:>12,}")


def main():
    names = command_names()
    for p in sys.argv[1:]:
        report(p, names)


if __name__ == "__main__":
    main()
