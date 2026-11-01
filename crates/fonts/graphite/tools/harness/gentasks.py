"""Tasks for the unit check (gr2dump.c and examples/gr-dump.rs): the
corpus texts' words and lines in each Graphite font, by default and with
each feature setting and language.

    python3 -I gentasks.py OUT TEXTS FONT... [--extra GR2TESTS] [--max N]
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
import gencorpus as gc  # noqa: E402

SPECIAL = gc.SPECIAL


def rtl(w):
    return any(0x0590 <= ord(c) <= 0x08FF or 0xFB1D <= ord(c) <= 0xFEFC for c in w)


def words(path, limit):
    out = []
    with open(path, encoding='utf-8', errors='replace') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            out.append(line[:400])
            out.extend(line.split())
            if len(out) > limit:
                break
    return out[:limit]


def main():
    args = sys.argv[1:]
    extra = None
    mx = 4000
    if '--extra' in args:
        i = args.index('--extra'); extra = args[i + 1]; del args[i:i + 2]
    if '--max' in args:
        i = args.index('--max'); mx = int(args[i + 1]); del args[i:i + 2]
    out, texts, fonts = args[0], args[1], args[2:]
    T = lambda n: os.path.join(texts, n)
    arabic = [T('awami-tests.txt'), T('udhr-urd.txt'), T('udhr-skr.txt'), T('udhr-arb.txt'), T('udhr-pes_1.txt')]
    latin = [T('ipa.txt'), T('udhr-vie.txt'), T('udhr-yor.txt'), T('udhr-rus.txt'), T('udhr-eng.txt'), T('udhr-lin_tones.txt')]
    plan = []
    for p in fonts:
        b = os.path.basename(p)
        plan.append((p, arabic if b.startswith('Awami') else latin))
    if extra:
        X = lambda n: os.path.join(extra, 'texts', n)
        F = lambda n: os.path.join(extra, 'fonts', n)
        plan += [(F('Padauk.ttf'), [X('my_HeadwordSyllables.txt')]),
                 (F('Scheherazadegr.ttf'), [X('udhr_arb.txt'), T('udhr-urd.txt')]),
                 (F('Annapurnarc2.ttf'), [X('udhr_nep.txt'), X('udhr_hin.txt')]),
                 (F('charis_r_gr.ttf'), [X('udhr_eng.txt'), X('udhr_yor.txt'), T('ipa.txt')]),
                 (F('MagyarLinLibertineG.ttf'), [X('udhr_eng.txt')]),
                 (F('Awami_test.ttf'), [X('awami_tests.txt')]),
                 (F('AwamiNastaliq-Regular.ttf'), [X('awami_tests.txt')]),
                 (F('grtest1gr.ttf'), [X('udhr_eng.txt')])]
    n = 0
    with open(out, 'w', encoding='utf-8') as o:
        def task(font, lang, feats, w):
            nonlocal n
            d = 2 | (1 if rtl(w) else 0)
            o.write('%s\t%s\t%s\t%d\t%s\n' % (font, lang, feats, d, w))
            n += 1
        for font, txts in plan:
            if not os.path.exists(font):
                continue
            with open(font, 'rb') as f:
                t = gc.tables(f.read())
            fs = gc.feats(t.get('Feat'), gc.names(t.get('name')))
            ls = gc.langs(t.get('Sill'))
            ws = []
            for x in txts:
                if os.path.exists(x):
                    ws += words(x, mx)
            for w in ws:
                task(font, '-', '-', w)
            sample = ws[: max(40, mx // 20)]
            for fid, _, sets in fs:
                for v, _ in sets or [(1, ''), (2, '')]:
                    for w in sample:
                        task(font, '-', '%d=%d' % (fid, v & 0xffff), w)
            for lang in ls:
                for w in sample:
                    task(font, lang, '-', w)
    print('tasks: %d' % n)


if __name__ == "__main__":
    main()
