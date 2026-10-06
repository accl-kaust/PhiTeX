#!/usr/bin/env python3
"""Where a plain build's time goes, by engine phase, from a frame-pointer
profile (`perf record -g --call-graph fp`): each sample goes to the
outermost frame that matches a phase, else to expansion and main control.

    perf script -i perf.data -F comm,ip,sym --no-inline | scripts/perf-phases.py
"""
import re, sys, collections
CATS = [  # (name, regex on a frame), checked root-first: the outermost wins
    ("deflate (zlib)", r"deflate|zlib|compress_block|longest_match|crc32|adler"),
    ("shipout + PDF writer", r"ship_out|hlist_out|vlist_out|pdf_ship|write_font|pdf_end|finish_pdf|pdf::|pdfconv|objs::|write_obj|subset"),
    ("image inclusion", r"png|pdfread|jpeg|image"),
    ("font loading", r"read_font_info|tfm::|fontmap|vf::|load_font|find_font"),
    ("line breaking", r"line_break|try_break|hyphenate|post_line_break"),
    ("page builder", r"build_page|fire_up|vsplit|prune_page"),
    ("math lists", r"mlist_to_hlist"),
    ("alignments", r"fin_align|init_align"),
    ("packing (hpack/vpack)", r"\bhpack|\bvpack|::hpack|::vpack|package"),
    ("makeindex/bibtex", r"makeindex|bibtex"),
    ("format load", r"load_fmt|undump|format::"),
]
cre = [(n, re.compile(r)) for n, r in CATS]
counts = collections.Counter(); leaf = collections.Counter(); total = 0
stack = []
def flush():
    global total
    if not stack: return
    total += 1
    cat = "expansion + main control (macros, scanning, assignments, other)"
    for fr in reversed(stack):  # root first
        hit = next((n for n, r in cre if r.search(fr)), None)
        if hit: cat = hit; break
    counts[cat] += 1
    leaf[stack[0]] += 1
for line in sys.stdin:
    line = line.rstrip("\n")
    if not line.strip():
        flush(); stack = []; continue
    if line.startswith((" ", "\t")):
        fr = line.strip().split(" ", 1)[-1]
        fr = re.sub(r"\+0x[0-9a-f]+$", "", fr)
        stack.append(fr)
flush()
print(f"samples {total}")
for n, c in counts.most_common():
    print(f"{100*c/total:6.2f}%  {n}")
print("\ntop leaf symbols:")
for n, c in leaf.most_common(30):
    print(f"{100*c/total:6.2f}%  {n[:120]}")
