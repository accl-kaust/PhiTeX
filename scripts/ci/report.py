#!/usr/bin/env python3
"""The performance history as one self-contained HTML page (docs/ci.md):
for each document, time series across commits (ordered by commit date) of
the cold builds, the edit, the settled pass, peak memory and partex's
ratios to TeX Live, as inline SVG with a hover tooltip (the commit, its
date and subject, the node, the median and range). No external file, no
library. Everything taken from the history is data: HTML-escaped.

    report.py HISTORY.jsonl OUT.html
"""

import html
import json
import math
import sys
from pathlib import Path

SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4"]
SERIES_DARK = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181"]

# (title, unit, log scale, [(label, metric, scale)])
# (title, unit, log scale, [(label, metric, scale, colour slot)]): a slot
# per side (TeX Live 0, plain 1, build 2, machine 3, SSA 4), the same in
# every chart
CHARTS = [
    ("Cold build to the fixpoint", "s", False, [
        ("TeX Live", "texlive.cold_s", 1, 0), ("partex plain", "plain.cold_s", 1, 1),
        ("partex build", "build.cold_s", 1, 2), ("partex machine", "machine.cold_s", 1, 3),
        ("partex SSA", "ssa.wall_s", 1, 4)]),
    ("A word edit, to the fixpoint", "s, log scale", True, [
        ("TeX Live rerun", "texlive.edit_s", 1, 0), ("partex machine, unchanged", "machine.warm_s", 1, 3),
        ("partex SSA rebuild", "ssa.edit_ms", 0.001, 4)]),
    ("One pass over settled files", "s", False, [
        ("TeX Live", "texlive.pass_s", 1, 0), ("partex plain", "plain.pass_s", 1, 1)]),
    ("Peak memory", "MB", False, [
        ("TeX Live", "texlive.cold_rss_kib", 1 / 1024, 0), ("partex plain", "plain.cold_rss_kib", 1 / 1024, 1),
        ("partex machine", "machine.cold_rss_kib", 1 / 1024, 3), ("partex SSA", "ssa.rss_kib", 1 / 1024, 4)]),
    ("partex / TeX Live", "ratio, log scale", True, [
        ("plain cold / TeX Live cold", ("plain.cold_s", "texlive.cold_s"), 1, 1),
        ("plain pass / TeX Live pass", ("plain.pass_s", "texlive.pass_s"), 1, 2),
        ("SSA cold / TeX Live cold", ("ssa.wall_s", "texlive.cold_s"), 1, 3),
        ("SSA edit / TeX Live edit", ("ssa.edit_ms", "texlive.edit_s"), 0.001, 4)]),
]

W, H, PL, PR, PT, PB = 640, 260, 56, 12, 14, 34


def esc(s):
    return html.escape(str(s if s is not None else ""), quote=True)


def value(doc, metric, scale):
    """(median, lo, hi) of `metric` (or a ratio of medians), or None."""
    if isinstance(metric, tuple):
        a, b = doc.get(metric[0]), doc.get(metric[1])
        if not (isinstance(a, dict) and isinstance(b, dict) and a.get("median") and b.get("median")):
            return None
        v = a["median"] * scale / b["median"]
        return v, v, v
    m = doc.get(metric)
    if not isinstance(m, dict) or m.get("median") is None:
        return None
    return m["median"] * scale, m["min"] * scale, m["max"] * scale


def ticks(lo, hi, log):
    if log:
        a, b = math.floor(math.log10(lo)), math.ceil(math.log10(hi))
        return [10 ** k for k in range(a, b + 1)]
    if hi <= lo:
        hi = lo + 1
    step = 10 ** math.floor(math.log10((hi - lo) / 4 or 1))
    for m in (1, 2, 5, 10):
        if (hi - lo) / (step * m) <= 5:
            step *= m
            break
    t = math.floor(lo / step) * step
    out = []
    while t <= hi + step * 0.5:
        out.append(round(t, 10))
        t += step
    return out


def fmt(v):
    if v == 0:
        return "0"
    if abs(v) >= 100:
        return f"{v:.0f}"
    if abs(v) >= 1:
        return f"{v:.3g}"
    return f"{v:.2g}"


def chart(entries, did, title, unit, log, series):
    pts = []  # (series index, x index, median, lo, hi, entry)
    for xi, e in enumerate(entries):
        doc = (e.get("docs") or {}).get(did) or {}
        for si, (_, metric, scale, _slot) in enumerate(series):
            v = value(doc, metric, scale)
            if v and (not log or v[1] > 0):
                pts.append((si, xi, *v, e))
    if not pts:
        return ""
    lo = min(p[3] for p in pts)
    hi = max(p[4] for p in pts)
    if not log:
        lo = 0
    tk = ticks(lo, hi, log)
    y0, y1 = (math.log10(tk[0]), math.log10(tk[-1])) if log else (tk[0], tk[-1])
    y1 = y1 if y1 > y0 else y0 + 1
    n = max(len(entries) - 1, 1)

    def X(i):
        return PL + (W - PL - PR) * (i / n if len(entries) > 1 else 0.5)

    def Y(v):
        t = math.log10(v) if log else v
        return PT + (H - PT - PB) * (1 - (t - y0) / (y1 - y0))

    s = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="{esc(title)}">']
    for t in tk:
        s.append(f'<line class="grid" x1="{PL}" x2="{W - PR}" y1="{Y(t):.1f}" y2="{Y(t):.1f}"/>'
                 f'<text class="tick" x="{PL - 6}" y="{Y(t) + 4:.1f}" text-anchor="end">{esc(fmt(t))}</text>')
    if log and tk[0] <= 1 <= tk[-1] and "ratio" in unit:
        s.append(f'<line class="ref" x1="{PL}" x2="{W - PR}" y1="{Y(1):.1f}" y2="{Y(1):.1f}"/>')
    every = max(1, len(entries) // 8)
    for xi, e in enumerate(entries):
        if xi % every == 0 or xi == len(entries) - 1:
            anchor = "end" if xi == len(entries) - 1 and xi else "middle"
            s.append(f'<text class="tick" x="{X(xi):.1f}" y="{H - PB + 16}" text-anchor="{anchor}">{esc((e.get("commit") or "")[:7])}</text>')
    for si in range(len(series)):
        cl = series[si][3]
        mine = [p for p in pts if p[0] == si]
        if not mine:
            continue
        if len(mine) > 1:
            d = " ".join(f"{'M' if k == 0 else 'L'}{X(p[1]):.1f},{Y(p[2]):.1f}" for k, p in enumerate(mine))
            s.append(f'<path class="line s{cl}" d="{d}"/>')
        for p in mine:
            e = p[5]
            if p[4] > p[3]:
                s.append(f'<line class="range s{cl}" x1="{X(p[1]):.1f}" x2="{X(p[1]):.1f}" y1="{Y(p[3]):.1f}" y2="{Y(p[4]):.1f}"/>')
            tip = (f"{series[si][0]}: {fmt(p[2])} {unit.split(',')[0]}"
                   + (f" (range {fmt(p[3])}–{fmt(p[4])})" if p[4] > p[3] else "")
                   + f"\n{(e.get('commit') or '')[:12]} · {(e.get('date') or '')[:10]} · {e.get('node') or '?'}"
                   + (" · backfill" if e.get("backfill") else "") + f"\n{e.get('subject') or ''}"[:300])
            s.append(f'<circle class="dot s{cl}" cx="{X(p[1]):.1f}" cy="{Y(p[2]):.1f}" r="4" data-tip="{esc(tip)}"/>')
    s.append("</svg>")
    legend = "".join(f'<span class="key"><i class="sw s{cl}"></i>{esc(lbl)}</span>'
                     for si, (lbl, _, _, cl) in enumerate(series) if any(p[0] == si for p in pts))
    return (f'<figure><figcaption><b>{esc(title)}</b> <span class="unit">{esc(unit)}</span></figcaption>'
            f'<div class="legend">{legend}</div>{"".join(s)}</figure>')


def table(entries, did):
    rows = []
    for e in reversed(entries[-12:]):
        doc = (e.get("docs") or {}).get(did) or {}

        def cell(m, scale=1):
            v = value(doc, m, scale)
            return fmt(v[0]) if v else "–"
        rows.append(f"<tr><td><code>{esc((e.get('commit') or '')[:10])}</code></td><td>{esc((e.get('date') or '')[:10])}</td>"
                    f"<td>{esc(e.get('node'))}</td><td>{cell('texlive.cold_s')}</td><td>{cell('plain.cold_s')}</td>"
                    f"<td>{cell('machine.cold_s')}</td><td>{cell('ssa.wall_s')}</td><td>{cell('ssa.edit_ms')}</td>"
                    f"<td>{cell('texlive.edit_s')}</td><td>{cell('plain.cold_rss_kib', 1 / 1024)}</td></tr>")
    return ("<details><summary>Table (last 12 runs)</summary><table><thead><tr><th>commit</th><th>date</th><th>node</th>"
            "<th>TL cold s</th><th>plain cold s</th><th>machine cold s</th><th>SSA cold s</th><th>SSA edit ms</th>"
            "<th>TL edit s</th><th>plain RSS MB</th></tr></thead><tbody>" + "".join(rows) + "</tbody></table></details>")


CSS = """
:root{color-scheme:light;--bg:#fcfcfb;--fg:#0b0b0b;--muted:#52514e;--grid:#e4e3df;--card:#ffffff;
""" + "".join(f"--s{i}:{c};" for i, c in enumerate(SERIES)) + """}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){color-scheme:dark;--bg:#1a1a19;--fg:#fff;--muted:#c3c2b7;--grid:#383835;--card:#222221;
""" + "".join(f"--s{i}:{c};" for i, c in enumerate(SERIES_DARK)) + """}}
:root[data-theme="dark"]{color-scheme:dark;--bg:#1a1a19;--fg:#fff;--muted:#c3c2b7;--grid:#383835;--card:#222221;
""" + "".join(f"--s{i}:{c};" for i, c in enumerate(SERIES_DARK)) + """}
body{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif}
main{max-width:1360px;margin:0 auto}h1{font-size:22px;margin:0 0 4px}h2{font-size:17px;margin:28px 0 8px}
p.note{color:var(--muted);margin:4px 0 16px;max-width:900px}
.grid2{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,600px),1fr));gap:16px}
figure{margin:0;background:var(--card);border:1px solid var(--grid);border-radius:8px;padding:12px}
figcaption{margin-bottom:4px}.unit{color:var(--muted);font-size:12px}
svg{width:100%;height:auto;display:block}.grid{stroke:var(--grid);stroke-width:1}.ref{stroke:var(--muted);stroke-dasharray:4 4}
.tick{fill:var(--muted);font-size:11px}.line{fill:none;stroke-width:2}.range{stroke-width:2;opacity:.45}
.dot{stroke:var(--card);stroke-width:2;cursor:default}
""" + "".join(f".line.s{i},.range.s{i}{{stroke:var(--s{i})}}.dot.s{i},.sw.s{i}{{fill:var(--s{i});background:var(--s{i})}}" for i in range(len(SERIES))) + """
.legend{display:flex;flex-wrap:wrap;gap:12px;font-size:12px;color:var(--muted);margin-bottom:4px}
.sw{display:inline-block;width:10px;height:10px;border-radius:5px;margin-right:5px;vertical-align:-1px}
#tip{position:fixed;pointer-events:none;background:var(--card);color:var(--fg);border:1px solid var(--grid);border-radius:6px;
padding:6px 8px;font-size:12px;white-space:pre-line;max-width:420px;box-shadow:0 2px 8px #0003;display:none}
table{border-collapse:collapse;font-size:12px;margin-top:8px;display:block;overflow-x:auto}td,th{padding:3px 8px;border-bottom:1px solid var(--grid);text-align:right}
td:first-child,th:first-child{text-align:left}details{margin-top:8px}
"""

JS = """
const tip=document.getElementById('tip');
document.addEventListener('mousemove',e=>{const t=e.target.closest('[data-tip]');
if(!t){tip.style.display='none';return}tip.textContent=t.dataset.tip;tip.style.display='block';
const x=Math.min(e.clientX+14,innerWidth-tip.offsetWidth-8);tip.style.left=x+'px';tip.style.top=(e.clientY+14)+'px'});
"""


def write(history, out):
    entries = []
    for line in Path(history).read_text().splitlines():
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if e.get("docs"):
            entries.append(e)
    # (one point per commit: the main runs, the latest run of a commit)
    by = {}
    for e in sorted(entries, key=lambda e: (e.get("date") or "", e.get("run") or "")):
        if e.get("main") or e.get("backfill"):
            by[e.get("commit")] = e
    entries = sorted(by.values(), key=lambda e: e.get("date") or "")
    docs = []
    for e in entries:
        for d in e.get("docs") or {}:
            if d not in docs:
                docs.append(d)
    parts = [f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
             f"<title>partex performance history</title><style>{CSS}</style></head><body><main>"
             "<h1>partex performance history</h1>"
             f"<p class=note>{len(entries)} commits of main, by commit date; each point is the median of the run's "
             "repetitions on one node, the vertical bar its range. TeX Live's engine is timed on the same node, to the same "
             "fixpoint (every pass and tool run counted). Hover a point for the commit. Generated by scripts/ci/report.py "
             "from the CI's history.</p>"]
    for d in docs:
        parts.append(f"<h2>{esc(d)}</h2><div class=grid2>")
        for title, unit, log, series in CHARTS:
            parts.append(chart(entries, d, title, unit, log, series))
        parts.append("</div>" + table(entries, d))
    ssa = [e for e in entries if e.get("ssa_edits")]
    if ssa:
        cases = sorted({c for e in ssa for c in e["ssa_edits"]} - {"(all cases)"})
        fake = [{**e, "docs": {"ssa-edits": {c: {"median": v["median_ms"], "min": v["median_ms"], "max": v["median_ms"]}
                                              for c, v in e["ssa_edits"].items()}}} for e in ssa]
        series = [("all cases (median of medians)", "(all cases)", 1, 0)] + [(c, c, 1, i + 1) for i, c in enumerate(cases[:4])]
        parts.append("<h2>ssa-edits: rebuild time per case (median, ms)</h2><div class=grid2>"
                     + chart(fake, "ssa-edits", "SSA rebuilds of the edit harness", "ms, log scale", True, series) + "</div>")
    parts.append(f"<div id=tip></div><script>{JS}</script></main></body></html>")
    Path(out).write_text("".join(parts))


if __name__ == "__main__":
    write(sys.argv[1], sys.argv[2])
