#!/usr/bin/env python3
"""First difference of two PDFs (oracle A, ours B): byte offset, the
object holding it (number, offset in the object), the writer part (from the
object's dictionary), and for a stream the first difference of the decoded
data (into an object stream: the inner object)."""
import re
import sys
import zlib

OBJ = re.compile(rb"(?:^|[\r\n])(\d+) (\d+) obj\b")


def objects(d):
    """[(start, num)] of the top-level objects, in file order."""
    return [(m.start(1), int(m.group(1))) for m in OBJ.finditer(d)]


def obj_at(objs, k):
    best = None
    for s, n in objs:
        if s <= k:
            best = (s, n)
        else:
            break
    return best


def body(d, start):
    e = d.find(b"endobj", start)
    return d[start : e if e >= 0 else len(d)]


def find_obj(d, num):
    m = re.search(rb"(?:^|[\r\n])(%d 0 obj)\b" % num, d)
    return body(d, m.start(1)) if m else None


def part(b):
    h = b[:600]
    def has(x):
        return x in h
    if has(b"/ObjStm"):
        return "object stream"
    if has(b"/XRef"):
        return "xref stream"
    if has(b"/FontFile") or has(b"/Subtype/Type1C") or has(b"/Subtype /Type1C") or has(b"/Subtype/CIDFontType0C") or has(b"/Subtype/OpenType"):
        return "font program"
    if has(b"/Length1") or has(b"/Length2"):
        return "font program"
    if has(b"/FontDescriptor") and not has(b"/Type/Font") and not has(b"/Type /Font"):
        return "font descriptor"
    if has(b"/Type/FontDescriptor") or has(b"/Type /FontDescriptor"):
        return "font descriptor"
    if has(b"/CIDSystemInfo") and has(b"/W"):
        return "CID font dict (widths)"
    if has(b"/Type/Font") or has(b"/Type /Font"):
        return "font dict"
    if has(b"/CMapName") or b"begincmap" in b:
        return "CMap (ToUnicode)"
    if has(b"/Type/Page") and not has(b"/Type/Pages"):
        return "page dict"
    if has(b"/Type /Page") and not has(b"/Type /Pages"):
        return "page dict"
    if has(b"/Type/Pages") or has(b"/Type /Pages"):
        return "pages tree"
    if has(b"/Type/Catalog") or has(b"/Type /Catalog"):
        return "catalog"
    if has(b"/Producer") or has(b"/CreationDate"):
        return "info"
    if has(b"/Subtype/Image") or has(b"/Subtype /Image"):
        return "image XObject"
    if has(b"/Subtype/Form") or has(b"/Subtype /Form"):
        return "form XObject"
    if has(b"/Annot") or has(b"/Subtype/Link") or has(b"/Subtype /Link"):
        return "annotation"
    if has(b"/Dests") or has(b"/Names"):
        return "names"
    if has(b"/Outlines") or has(b"/First") and has(b"/Count"):
        return "outlines"
    if has(b"/Differences"):
        return "encoding"
    if has(b"stream"):
        return "stream (content?)"
    return "other"


def stream_data(b):
    s = b.find(b"stream")
    if s < 0:
        return None
    s += 6
    if b[s:s+2] == b"\r\n":
        s += 2
    elif b[s:s+1] in (b"\n", b"\r"):
        s += 1
    e = b.rfind(b"endstream")
    raw = b[s:e]
    if b"/FlateDecode" in b[:s]:
        try:
            return zlib.decompressobj().decompress(raw)
        except zlib.error as e:
            return b"<does not inflate: %s>" % str(e).encode()
    return raw


def first_diff(a, b):
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i
    return n if len(a) != len(b) else None


def show(x, k, w=60):
    s = x[max(0, k - w) : k + w]
    return repr(s)[:260]


def main(fa, fb):
    a = open(fa, "rb").read()
    try:
        b = open(fb, "rb").read()
    except OSError:
        b = b""
    if not b:
        print("  ours: empty output")
        return
    k = first_diff(a, b)
    if k is None:
        print("  identical")
        return
    print(f"  sizes {len(a)} vs ours {len(b)}; first byte diff at {k}")
    tail = a.rfind(b"startxref")
    xr = a.rfind(b"\nxref")
    oa = obj_at(objects(a), k)
    if k < 20:
        print(f"  header: A {show(a, k)} B {show(b, k)}")
        return
    if xr >= 0 and k >= xr:
        print(f"  part: xref table / trailer (A {show(a, k, 40)} B {show(b, k, 40)})")
        return
    if k >= tail >= 0:
        print(f"  part: startxref (A {show(a, k, 30)} B {show(b, k, 30)})")
        return
    if not oa:
        print(f"  no object at {k}: A {show(a,k)} B {show(b,k)}")
        return
    s, num = oa
    ba = body(a, s)
    bb = find_obj(b, num)
    p = part(ba)
    print(f"  object {num} (at {s}, +{k - s}), part: {p}")
    print(f"    A dict: {repr(ba[:200])[:220]}")
    if bb is None:
        print("    ours has no such object")
        return
    print(f"    B dict: {repr(bb[:200])[:220]}")
    da, db = stream_data(ba), stream_data(bb)
    if da is not None and db is not None:
        j = first_diff(da, db)
        if j is None:
            print("    decoded streams identical (compression/dict differs)")
        else:
            print(f"    decoded sizes {len(da)} vs {len(db)}; first decoded diff at {j}")
            if p == "object stream":
                # header: pairs "num off" then objects from /First
                m = re.search(rb"/First\s+(\d+)", ba[:600])
                first = int(m.group(1)) if m else 0
                hdr = list(map(int, da[:first].split()))
                pairs = list(zip(hdr[0::2], hdr[1::2]))
                inner = None
                for i, (on, off) in enumerate(pairs):
                    if first + off <= j:
                        end = first + pairs[i + 1][1] if i + 1 < len(pairs) else len(da)
                        inner = (on, first + off, end)
                if j < first:
                    print("    in the object stream's header (object numbers/offsets differ)")
                elif inner:
                    on, st, end = inner
                    ib = da[st:end]
                    print(f"    inner object {on} (+{j - st}), part: {part(ib)}")
                    print(f"      A {repr(da[st:st+160])[:200]}")
            print(f"      A ...{show(da, j)}")
            print(f"      B ...{show(db, j)}")
    else:
        print(f"    A ...{show(a, k)}")
        print(f"    B ...{show(b, k)}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
