"""Whether TeX itself ends on a TRIP reproduction's texts.

    flow-fuzz-tex.py TRIPDIR REPRO...

For each reproduction of the `trip` document: its text before the edit
and after it, each TRIP's session (pass 1, `tex -ini` with trip1.in, on
the first job; pass 2, `tex` with trip2.in, on the second), run by the
system's `tex` in a scratch directory with a limit of TEX_LIMIT seconds
(env, 60). A line a reproduction: `ends`, `loops` or `failed` (tex
did not write what the pass writes: trip.fmt, trip.log; a full disk or
a missing file) for each of the four runs. A hang of ours on a text where TeX loops too is TeX's, not
ours: TRIP edited at random can loop. Run it under scripts/sandbox.
"""
import os
import shutil
import subprocess
import sys
import tempfile

SEP = '\n\x01'
JOB = ''
LIMIT = int(os.environ.get('TEX_LIMIT', '60'))
trip = sys.argv[1]


def texts(path):
    parts = open(path, encoding='utf-8').read().split(SEP)
    head = parts[0].split('\n')
    _, efile, at, dl, _ = next(l for l in head if l.startswith('edit ')).split(' ', 4)
    ins = next(p for p in parts if p.startswith('ins\n'))[4:]
    text = next(p for p in parts if p.startswith('text\n'))[5:]
    b = text.encode('utf-8')
    after = text
    if efile == '-':
        after = (b[:int(at)] + ins.encode('utf-8') + b[int(at) + int(dl):]).decode('utf-8')
    return text, after


def session(text):
    jobs = text.split(JOB)[1:]
    out = []
    d = tempfile.mkdtemp()
    try:
        for f in ('trip.tfm', 'trip1.in', 'trip2.in'):
            shutil.copy(os.path.join(trip, f), d)
        env = dict(os.environ, TFMFONTS='.', TEXFORMATS='.', TEXINPUTS='.')
        for args, stdin, job in ((['-ini'], 'trip1.in', jobs[0]), ([], 'trip2.in', jobs[1] if len(jobs) > 1 else '')):
            open(os.path.join(d, 'trip.tex'), 'wb').write(job.encode('latin-1', 'replace'))
            with open(os.path.join(d, stdin), 'rb') as i:
                want = os.path.join(d, 'trip.fmt' if args else 'trip.log')
                if os.path.exists(want):
                    os.remove(want)
                try:
                    subprocess.run(['tex', *args, '-interaction=batchmode'], cwd=d, env=env, stdin=i,
                                   capture_output=True, timeout=LIMIT)
                    out.append('ends' if os.path.exists(want) else 'failed')
                except subprocess.TimeoutExpired:
                    out.append('loops')
    finally:
        shutil.rmtree(d, ignore_errors=True)
    return out


for path in sys.argv[2:]:
    before, after = texts(path)
    b, a = session(before), session(after)
    print(f'{path}\tbefore pass1 {b[0]} pass2 {b[1]}\tafter pass1 {a[0]} pass2 {a[1]}', flush=True)
