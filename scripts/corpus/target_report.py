#!/usr/bin/env python3
"""One line (and the error histogram) from a target run's target.json on stdin."""
import collections
import json
import re
import sys

d = json.load(sys.stdin)
w = d["watch"]
b = (w.get("builds") or [{}])
c = b[0]
p = c.get("parallel") or {}
st = d.get("stats") or {}
sp = st.get("parallel") or {}
errs = c.get("errors") or []
diag = [e for e in errs if re.search(r"undeclared definition|hint regime", e)]
tex = [e for e in errs if e not in diag]
cd = d.get("cold_diff") or {}
print(f"cp {p.get('critical_path')} | realised cp {p.get('realised_critical_path', sp.get('realised_critical_path'))} | "
      f"nodes run {c.get('nodes_run')} | rounds {st.get('rounds')} loop rounds {st.get('loop_rounds')} | "
      f"TeX errors {len(tex)} + {len(diag)} diagnostics | PDF {'written' if c.get('output') else 'NOT written'} | "
      f"pages {c.get('pages')} | cold {w.get('cold_process_s')} s (engine {round((p.get('engine_ms') or 0) / 1e3, 2)} s, busy {round((p.get('busy_ms') or 0) / 1e3, 2)} s), j{p.get('threads')} | "
      f"edit {w.get('edit_process_s')} s ({b[1].get('nodes_run') if len(b) > 1 else '-'} nodes) | "
      f"first byte diff {cd.get('first_byte')}")
for k, n in collections.Counter(e[:110] for e in errs).most_common(15):
    print(f"  {n:4d} {k}")
if cd.get("first_byte") not in (None, -1):
    for k in ("object", "page_comment", "qdf_line", "want", "got"):
        print(f"  {k}: {cd.get(k)}")
