#!/usr/bin/env python3
# scripts/sandbox python3 scripts/xetex/icu-tables.py > crates/partex-core/src/icu_tables.rs
"""ICU's stateless single-byte converters as tables, by alias, for XeTeX's
\\XeTeXinputencoding: run with the ICU XeTeX uses (`uconv`)."""
import subprocess, sys

def norm(name):
    # ucnv_io_stripASCIIForCompare: letters lowercased, digits kept (a 0
    # not after a digit and before one dropped), all else dropped
    out, after_digit = [], False
    for i, c in enumerate(name):
        if c.isascii() and c.isalpha():
            out.append(c.lower()); after_digit = False
        elif c == '0':
            nxt = name[i + 1] if i + 1 < len(name) else ''
            if not after_digit and nxt.isdigit():
                continue
            out.append(c)
        elif c.isdigit():
            out.append(c); after_digit = True
        else:
            after_digit = False
    return ''.join(out)

def decode(alias, data):
    r = subprocess.run(['uconv', '-f', alias, '-t', 'UTF-32BE', '--from-callback', 'substitute'],
                       input=data, capture_output=True)
    if r.returncode != 0:
        return None
    b = r.stdout
    return [int.from_bytes(b[i:i+4], 'big') for i in range(0, len(b), 4)]

aliases = []
for line in subprocess.run(['uconv', '-l'], capture_output=True, text=True).stdout.splitlines():
    for a in line.split():
        if a not in aliases:
            aliases.append(a)
fwd = bytes(range(256)); rev = bytes(reversed(range(256)))
tables, ids, by_norm = [], {}, {}
utf8 = decode('UTF-8', b'')
for a in aliases:
    n = norm(a)
    if not n or n in by_norm:
        continue
    f = decode(a, fwd)
    r = decode(a, rev)
    if f is not None and r is not None and len(f) == 256 and f == list(reversed(r)) and max(f) < 0x10000:
        t = tuple(f)
        if t not in ids:
            ids[t] = len(tables); tables.append(t)
        by_norm[n] = ids[t]
    else:
        by_norm[n] = None
# UTF-8's names: decoded as UTF-8 (ICU's: U+FFFD per maximal subpart)
for line in subprocess.run(['uconv', '-l'], capture_output=True, text=True).stdout.splitlines():
    names = line.split()
    if names and names[0] == 'UTF-8':
        for a in names:
            by_norm[norm(a)] = 'utf8'
out = sys.stdout
out.write('//! ICU\'s converters by name, for `XeTeX`\'s `\\XeTeXinputencoding` (generated\n')
out.write('//! from `uconv -l` and `uconv -f NAME` by `scripts/xetex/icu-tables.py`: do not edit).\n\n')
out.write('/// The stateless single-byte converters\' tables: byte to UTF-16 unit.\n')
out.write('pub(crate) static SBCS: [[u16; 256]; %d] = [\n' % len(tables))
for t in tables:
    out.write('    [' + ', '.join('0x%04x' % c for c in t) + '],\n')
out.write('];\n\n')
out.write('/// What a name (as `norm_name` leaves it) opens: a table of `SBCS`,\n')
out.write('/// `UTF8`, or `OTHER` (a converter partex does not have).\n')
out.write('pub(crate) const UTF8: u16 = 0xffff;\npub(crate) const OTHER: u16 = 0xfffe;\n\n')
out.write('pub(crate) static NAMES: [(&str, u16); %d] = [\n' % len(by_norm))
for n in sorted(by_norm):
    v = by_norm[n]
    out.write('    ("%s", %s),\n' % (n, 'UTF8' if v == 'utf8' else 'OTHER' if v is None else v))
out.write('];\n')
