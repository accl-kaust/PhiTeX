import sys, re
# editpdf-pick-tmp.py FILE ID -> LINE \t WORD \t WORD' \t SENTENCE-END: a prose line in the body's middle
p, doc = sys.argv[1], sys.argv[2]
if doc == "ams":
    print("374\tvertically\tbertically\tthe equation."); sys.exit(0)
ls = open(p, "rb").read().split(b"\n")
try: b = next(i for i, l in enumerate(ls) if l.startswith(b"\\begin{document}"))
except StopIteration: sys.exit(1)
try: e = max(i for i, l in enumerate(ls) if l.startswith(b"\\end{document}"))
except ValueError: e = len(ls)
mid = (b + e) // 2
def ok(l):
    if b"\\" in l or b"%" in l or b"$" in l or b"&" in l or b"{" in l or b"}" in l or b"~" in l: return None
    m = re.search(rb"([a-z]{6,}) ", l); s = re.search(rb"([a-z]+\.)$", l.rstrip())
    if not (m and s) or not l.strip() or l.startswith(b" ") and False: return None
    return m.group(1), s.group(1)
order = sorted(range(b + 1, e), key=lambda i: abs(i - mid))
for i in order:
    r = ok(ls[i])
    # (a line between prose lines: not in a verbatim or a table)
    if r and i > 0 and ls[i - 1].strip() and not ls[i - 1].lstrip().startswith(b"\\begin"):
        w, s = r
        w2 = (b"b" if w[:1] != b"b" else b"c") + w[1:]
        print("%d\t%s\t%s\t%s" % (i + 1, w.decode(), w2.decode(), s.decode())); sys.exit(0)
sys.exit(1)
