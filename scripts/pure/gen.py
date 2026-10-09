import random, sys
n = int(sys.argv[1]); r = random.Random(1)
words = "the of and a to in is that it for as with was on be by this are or at from an which have not but its can more one all their has also these other than been into some may such only most between new each time".split()
out = [r"\pdfoutput=1 \pdfcompresslevel=9 \hsize=5in \vsize=7in", ""]
for p in range(n):
    s = []
    for k in range(6):
        w = [r.choice(words) for _ in range(r.randint(8, 16))]
        w[0] = w[0].capitalize()
        s.append(" ".join(w) + ".")
    out.append("\n".join(s[i:i+2] and " ".join(s[i:i+2]) for i in range(0, 6, 2)))
    out.append("")
out.append(r"\bye")
print("\n".join(out))
