"""Reads a TFM file into the fixture format the tests parse: metrics
scaled to the design size as tex.web §571-§575 does, every lig/kern pair
as the first matching instruction, extensible recipes, parameters."""
import struct


def tfm(path):
    d = open(path, 'rb').read()
    lf, lh, bc, ec, nw, nh, nd, ni, nl, nk, ne, np = struct.unpack('>12H', d[:24])
    w = lambda i: d[4 * i: 4 * i + 4]
    base = 6 + lh
    ci = {c: w(base + c - bc) for c in range(bc, ec + 1)}
    wb = base + ec - bc + 1
    hb, db = wb + nw, wb + nw + nh
    ib, lb = db + nd, db + nd + ni
    kb = lb + nl
    eb = kb + nk
    pb = eb + ne
    design = struct.unpack('>i', d[24 + 4:24 + 8])[0]
    # tex.web §568, §572: at its design size, z is the design size in sp
    z = design // 16
    alpha = 16
    while z >= 0o40000000:
        z //= 2
        alpha += alpha
    beta = 256 // alpha
    alpha *= z
    def sw(b):
        a, b1, c, dd = b
        v = (((dd * z) // 256 + c * z) // 256 + b1 * z) // beta
        if a == 0:
            return v
        assert a == 255
        return v - alpha
    lk = [w(lb + i) for i in range(nl)]
    out = []
    chars = {}
    for c, info in ci.items():
        if info[0] == 0:
            continue
        wd = sw(w(wb + info[0]))
        ht = sw(w(hb + (info[1] >> 4)))
        dp = sw(w(db + (info[1] & 15)))
        it = sw(w(ib + (info[2] >> 2)))
        tag, rem = info[2] & 3, info[3]
        chars[c] = (tag, rem)
        extra = ''
        if tag == 2:
            extra = ' list %d' % rem
        elif tag == 3:
            t, m, b, r = w(eb + rem)
            extra = ' ext %d %d %d %d' % (t, m, b, r)
        out.append('char %d %d %d %d %d%s' % (c, wd, ht, dp, it, extra))
    def program(i, rights):
        res = {}
        for r in rights:
            k = i
            while True:
                s, n, op, rem = lk[k]
                if n == r and s <= 128:
                    if op >= 128:
                        res[r] = 'kern %d' % sw(w(kb + 256 * (op - 128) + rem))
                    else:
                        res[r] = 'lig %d %d' % (op, rem)
                    break
                if s >= 128:
                    break
                k += s + 1
        return res
    for c, (tag, rem) in chars.items():
        if tag != 1:
            continue
        i = rem
        if lk[i][0] > 128:
            i = 256 * lk[i][2] + lk[i][3]
        for r, v in program(i, range(256)).items():
            out.append('lk %d %d %s' % (c, r, v))
    if nl and lk[-1][0] == 255:
        for r, v in program(256 * lk[-1][2] + lk[-1][3], range(256)).items():
            out.append('lk -1 %d %s' % (r, v))
    if nl and lk[0][0] == 255:
        out.append('bchar %d' % lk[0][1])
    for i in range(1, np):  # param 1 (slant) is not scaled like the others
        out.append('param %d %d' % (i + 1, sw(w(pb + i))))
    return out
