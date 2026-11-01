"""phitex-graphite's corpus: plain XeTeX documents that set text in every
Graphite font (`/GR`), with each of its features' settings and each of
its languages, for XeTeX to shape (the oracle: the XDV's native words).

    python3 -I gencorpus.py OUT TEXTS FONT... [--extra GR2TESTS]
    python3 -I gencorpus.py --split UNIT.tex

FONT is a font file's path (TeX Live's fonts with a Silf table, found by
tools/corpus.sh); TEXTS is corpus/texts. With --extra, graphite2's own
test fonts and texts (its tests/ directory) are added, loaded by path.
Each unit (a text in one font request) is a document OUT/FONT-NNN.tex,
its texts beside it (TeX's special characters made spaces). Every font
request has a size of its own.
"""

import os
import re
import struct
import sys


def tables(data):
    n = struct.unpack('>H', data[4:6])[0]
    out = {}
    for i in range(n):
        tag, _, off, ln = struct.unpack('>4sLLL', data[12 + 16 * i:28 + 16 * i])
        out[tag.decode('latin1')] = data[off:off + ln]
    return out


def names(name):
    """name ID -> the English (Windows, 0x409) string."""
    out = {}
    if not name:
        return out
    _, count, soff = struct.unpack('>HHH', name[:6])
    for i in range(count):
        pid, eid, lid, nid, ln, off = struct.unpack('>6H', name[6 + 12 * i:18 + 12 * i])
        if pid == 3 and eid == 1 and lid == 0x409:
            out[nid] = name[soff + off:soff + off + ln].decode('utf-16-be', 'replace')
    return out


def feats(feat, nm):
    """[(id, label, [(value, label)])] from the Feat table."""
    out = []
    if not feat:
        return out
    ver = struct.unpack('>L', feat[:4])[0]
    nf = struct.unpack('>H', feat[4:6])[0]
    p = 12
    for _ in range(nf):
        if ver >= 0x00020000:
            fid, ns, _, off, _, lab = struct.unpack('>LHHLHH', feat[p:p + 16])
            p += 16
        else:
            fid, ns, off, _, lab = struct.unpack('>HHLHH', feat[p:p + 12])
            p += 12
        sets = []
        for k in range(ns):
            v, sl = struct.unpack('>hH', feat[off + 4 * k:off + 4 * k + 4])
            sets.append((v, nm.get(sl, '')))
        out.append((fid, nm.get(lab, ''), sets))
    return out


def langs(sill):
    """The Sill table's language codes."""
    out = []
    if not sill:
        return out
    n = struct.unpack('>H', sill[4:6])[0]
    for i in range(n):
        code = sill[12 + 8 * i:16 + 8 * i].rstrip(b'\0 ').decode('latin1')
        if code:
            out.append(code)
    return out


SPECIAL = str.maketrans({c: ' ' for c in '\\{}$&#^_%~'})


def clean(src, dst, words=None):
    with open(src, encoding='utf-8', errors='replace') as f:
        text = f.read().translate(SPECIAL)
    if words is not None:
        out, n = [], 0
        for line in text.splitlines():
            w = line.split()
            if not w:
                continue
            out.append(' '.join(w[:words - n]))
            n += len(w)
            if n >= words:
                break
        text = '\n'.join(out) + '\n'
    with open(dst, 'w', encoding='utf-8') as f:
        f.write(text)


HEAD = r"""\hsize=150mm \vsize=230mm \parindent=0pt \parskip=2pt
\tolerance=10000 \hbadness=10000 \vbadness=10000 \hfuzz=\maxdimen
\pretolerance=-1 \emergencystretch=50pt \nopagenumbers
\def\T#1#2#3{\font\x="#1" at #2\relax\x\input #3 \par}
"""


class Doc:
    """The documents of one font: one a text set in one font request (a
    unit), so a word XeTeX cannot shape (it stops: "all shapers failed")
    loses only its unit, which `--split` then runs a line at a time."""

    def __init__(self, out, name):
        self.out, self.name, self.k = out, name, 0

    def text(self, src, words=None):
        """A cleaned copy of the text beside the documents (its name)."""
        base = 't-%s-%s%s.txt' % (self.name, os.path.basename(src)[:-4], '' if words is None else '-%d' % words)
        dst = os.path.join(self.out, base)
        if not os.path.exists(dst):
            clean(src, dst, words)
        return base

    def set(self, spec, text):
        # (a size of its own: 10pt plus k scaled points)
        self.k += 1
        size = 10 * 65536 + 7 * self.k
        unit(os.path.join(self.out, '%s-%03d.tex' % (self.name, self.k)), spec, size, text)

    def write(self):
        pass


def unit(path, spec, size, text):
    with open(path, 'w', encoding='utf-8') as f:
        f.write(HEAD + '\\T{%s}{%dsp}{%s}\n\\bye\n' % (spec, size, text))


def split(path):
    """A unit XeTeX stopped in, as one document a line of its text
    (NAME-lNNNN.tex, the text's lines beside it)."""
    with open(path, encoding='utf-8') as f:
        src = f.read()
    m = re.search(r'\\T\{(.*)\}\{(\d+)sp\}\{(.*)\}\n', src)
    spec, size, text = m.group(1), int(m.group(2)), m.group(3)
    d = os.path.dirname(path)
    with open(os.path.join(d, text), encoding='utf-8') as f:
        lines = [l for l in f.read().splitlines() if l.strip()]
    base = path[:-4]
    for i, l in enumerate(lines):
        t = '%s-l%04d.txt' % (os.path.basename(base), i)
        with open(os.path.join(d, t), 'w', encoding='utf-8') as f:
            f.write(l + '\n')
        unit('%s-l%04d.tex' % (base, i), spec, size, t)
    print(len(lines))


def font_doc(out, name, path, by_path, full, sample, extra_specs=()):
    """A document: `full` texts with the defaults, `sample` with every
    feature setting and language."""
    with open(path, 'rb') as f:
        t = tables(f.read())
    nm = names(t.get('name'))
    fs = feats(t.get('Feat'), nm)
    ls = langs(t.get('Sill'))
    d = Doc(out, name)
    base = '[%s]' % (path if by_path else os.path.basename(path))
    full = [d.text(x) for x in full]
    smp = [d.text(x, w) for x, w in sample]
    for x in full:
        d.set(base + '/GR', x)
    for spec in extra_specs:
        for x in smp:
            d.set(spec, x)
    # every setting of every feature, by number
    for fid, _, sets in fs:
        vals = [v for v, _ in sets] or [0, 1, 2]
        for v in vals:
            for x in smp:
                d.set('%s/GR:%d=%d' % (base, fid, v), x)
    # by name (the first sample only), where the names can be written
    bad = set(':;,=]"')
    for fid, lab, sets in fs:
        if not lab or bad & set(lab):
            continue
        for v, sl in sets:
            if sl and not bad & set(sl):
                d.set('%s/GR:%s=%s' % (base, lab, sl), smp[0])
    # every language
    for lang in ls:
        for x in smp:
            d.set('%s/GR:language=%s' % (base, lang), x)
    d.write()
    return len(fs), sum(max(1, len(s)) for _, _, s in fs), len(ls)


def main():
    args = sys.argv[1:]
    if args[0] == '--split':
        split(args[1])
        return
    extra = None
    if '--extra' in args:
        i = args.index('--extra')
        extra = args[i + 1]
        del args[i:i + 2]
    out, texts, fonts = args[0], args[1], args[2:]
    os.makedirs(out, exist_ok=True)
    T = lambda n: os.path.join(texts, n)
    arabic = [T('udhr-urd.txt'), T('awami-tests.txt'), T('udhr-skr.txt'), T('udhr-pnb.txt'),
              T('udhr-pes_1.txt'), T('udhr-arb.txt'), T('udhr-uig_arab.txt')]
    latin = [T('ipa.txt'), T('udhr-eng.txt'), T('udhr-yor.txt'), T('udhr-vie.txt'), T('udhr-rus.txt'),
             T('udhr-bul.txt'), T('udhr-ewe.txt'), T('udhr-hau_NG.txt'), T('udhr-lin_tones.txt'),
             T('udhr-nav.txt')]
    for p in fonts:
        b = os.path.basename(p)
        n = os.path.splitext(b)[0].lower().replace('_', '-')
        if b.startswith('AwamiNastaliq'):
            if b == 'AwamiNastaliq-Regular.ttf':
                r = font_doc(out, n, p, False, arabic, [(T('awami-tests.txt'), 400), (T('udhr-urd.txt'), 300)],
                             ['Awami Nastaliq/GR', '[%s]/GR:script=arab' % b, '[%s]/GR:+kern' % b])
            else:
                r = font_doc(out, n, p, False, arabic[:2], [(T('awami-tests.txt'), 150), (T('udhr-urd.txt'), 100)])
        else:
            spec = ['Doulos SIL/GR', '[%s]/GR:script=latn' % b] if b.startswith('Doulos') else []
            r = font_doc(out, n, p, False, latin, [(T('ipa.txt'), None), (T('udhr-vie.txt'), 200),
                                                   (T('udhr-yor.txt'), 200), (T('udhr-rus.txt'), 100)], spec)
        print('%s: %d features (%d settings), %d languages' % (b, *r))
    if extra:
        ef, et = os.path.join(extra, 'fonts'), os.path.join(extra, 'texts')
        X = lambda n: os.path.join(et, n)
        for f, full, sample in [
            ('Padauk.ttf', [X('my_HeadwordSyllables.txt')], [(X('my_HeadwordSyllables.txt'), 300)]),
            ('Scheherazadegr.ttf', [X('udhr_arb.txt'), T('udhr-urd.txt')], [(X('udhr_arb.txt'), 200)]),
            ('Annapurnarc2.ttf', [X('udhr_nep.txt'), X('udhr_hin.txt')], [(X('udhr_nep.txt'), 200)]),
            ('charis_r_gr.ttf', [X('udhr_eng.txt'), X('udhr_yor.txt'), T('ipa.txt'), T('udhr-vie.txt')],
             [(T('ipa.txt'), None), (X('udhr_yor.txt'), 150)]),
            ('MagyarLinLibertineG.ttf', [X('udhr_eng.txt'), T('udhr-bul.txt')], [(X('udhr_eng.txt'), 200)]),
            ('Awami_test.ttf', [X('awami_tests.txt')], [(X('awami_tests.txt'), 150)]),
            ('grtest1gr.ttf', [X('udhr_eng.txt')], [(X('udhr_eng.txt'), 100)]),
        ]:
            p = os.path.join(ef, f)
            if os.path.exists(p):
                r = font_doc(out, 'x-' + os.path.splitext(f)[0].lower().replace('_', '-'), p, True, full, sample)
                print('%s: %d features (%d settings), %d languages' % (f, *r))


if __name__ == "__main__":
    main()
