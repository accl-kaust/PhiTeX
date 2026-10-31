"""Where two PDFs first differ: the first page whose content differs, and
the first differing object (the corpus run's "first differing page",
scripts/accl/corpus-run).

Both files are put in qpdf's QDF form (object streams unpacked, streams
decoded, objects renumbered in traversal order, each marked with its
original number), with the /ID pair removed (scripts/pdfcheck's `canon`),
and walked object by object. The answer names the first object whose text
differs: its original number on each side, its /Type (or /Subtype), the
page it belongs to when QDF says (`%% Page N`, `%% Contents for page N`),
and the first differing line of each. When the QDF forms agree the files
differ in their bytes only (compression, packing, numbering).

    python3 pdfdiff.py A.pdf B.pdf     # prints the JSON
"""

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

OBJ = re.compile(rb"^(\d+) 0 obj\n", re.M)
ORIG = re.compile(rb"%% Original object ID: (\d+) (\d+)")
PAGE = re.compile(rb"%% (Page|Contents for page) (\d+)")
TYPE = re.compile(rb"/(?:Type|Subtype) */(\w+)")
ID = re.compile(rb"^( *)/ID \[<[0-9a-fA-F]*> *<[0-9a-fA-F]*>\]", re.M)


def qdf(pdf, out):
    r = subprocess.run(
        ["qpdf", "--qdf", "--object-streams=disable", "--compress-streams=n",
         "--decode-level=generalized", str(pdf), str(out)],
        capture_output=True, timeout=600,
    )
    # (exit 3: warnings, the file was written)
    if r.returncode not in (0, 3):
        raise RuntimeError(r.stderr.decode(errors="replace")[:300])
    return ID.sub(rb"\1/ID []", Path(out).read_bytes())


def objects(text):
    """[(header comment block, object text)] in file order."""
    out = []
    prev_end = 0
    for m in OBJ.finditer(text):
        if m.start() < prev_end:
            continue  # (an "N 0 obj" line inside a stream)
        e = text.find(b"\nendobj", m.end())
        end = len(text) if e < 0 else e + 7
        out.append((text[prev_end:m.start()], text[m.start():end]))
        prev_end = end
    return out


def describe(head, body):
    o = ORIG.search(head)
    p = None
    for m in PAGE.finditer(head):
        p = int(m.group(2))
    t = TYPE.search(body[:2000])
    return {
        "obj": int(o.group(1)) if o else None,
        "type": t.group(1).decode() if t else None,
        "page": p,
    }


def first_line_diff(a, b):
    la, lb = a.split(b"\n"), b.split(b"\n")
    for i, (x, y) in enumerate(zip(la, lb)):
        if x != y:
            return i, x[:160].decode("latin-1"), y[:160].decode("latin-1")
    i = min(len(la), len(lb))
    return i, (la[i] if i < len(la) else b"<end>")[:160].decode("latin-1"), \
        (lb[i] if i < len(lb) else b"<end>")[:160].decode("latin-1")


def pages(text):
    """[content of page 1, page 2, ...]: each page's content streams as
    QDF writes them (`%% Contents for page N`)."""
    out = {}
    for head, body in objects(text):
        for m in re.finditer(rb"%% Page (\d+)\n", head):
            out.setdefault(int(m.group(1)), b"")
        for m in re.finditer(rb"%% Contents for page (\d+)", head):
            n = int(m.group(1))
            # (the stream's data only: object numbers shift after any
            # earlier extra object)
            a = body.find(b"stream\n")
            e = body.rfind(b"endstream")
            out[n] = out.get(n, b"") + (body[a + 7:e] if a >= 0 and e > a else body)
    return [out[k] for k in sorted(out)]


def first_page(qa, qb):
    """The first page (1-based) whose content differs, and the page
    counts; None when every page's content agrees."""
    pa, pb = pages(qa), pages(qb)
    for i, (x, y) in enumerate(zip(pa, pb)):
        if x != y:
            return i + 1, len(pa), len(pb)
    if len(pa) != len(pb):
        return min(len(pa), len(pb)) + 1, len(pa), len(pb)
    return None, len(pa), len(pb)


def diff(a, b):
    a, b = Path(a), Path(b)
    da, db = a.read_bytes(), b.read_bytes()
    if da == db:
        return {"same": True}
    res = {"same": False, "sizes": [len(da), len(db)]}
    res["byte"] = next((i for i, (x, y) in enumerate(zip(da, db)) if x != y), min(len(da), len(db)))
    with tempfile.TemporaryDirectory() as t:
        try:
            qa, qb = qdf(a, Path(t) / "a.qdf"), qdf(b, Path(t) / "b.qdf")
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            res["qdf_error"] = str(e)
            return res
    if qa == qb:
        res["content_same"] = True
        res["summary"] = "bytes differ, content the same (compression, packing or numbering)"
        return res
    fp, na, nb = first_page(qa, qb)
    res.update({"first_page": fp, "pages": [na, nb]})
    oa, ob = objects(qa), objects(qb)
    for i, ((ha, ba), (hb, bb)) in enumerate(zip(oa, ob)):
        if ha + ba != hb + bb:
            line, xa, xb = first_line_diff(ba, bb)
            d = describe(ha, ba)
            res.update({
                "qdf_index": i + 1,
                "oracle_obj": d["obj"],
                "phitex_obj": describe(hb, bb)["obj"],
                "type": d["type"],
                "page": d["page"],
                "line": line,
                "oracle": xa,
                "phitex": xb,
            })
            where = f"page {d['page']}, " if d["page"] else ""
            res["summary"] = (f"page {fp}: " if fp else "") + f"obj {d['obj']} ({where}{d['type'] or 'untyped'}) line {line}"
            return res
    res["summary"] = (f"page {fp}: " if fp else "") + f"object count differs: {len(oa)} vs {len(ob)}"
    return res


if __name__ == "__main__":
    print(json.dumps(diff(sys.argv[1], sys.argv[2]), indent=1))
