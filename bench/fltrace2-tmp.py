"""fltrace2: the wakes of text nodes (exec, resume) by field and by the waking node's effect and may-define."""
import re, sys, collections
c = collections.Counter(); tot = 0
for l in open(sys.argv[1], errors='replace'):
    m = re.match(r'wake: node \d+ op .*? field (\S*) (\w+)[ {(].*? by (-|\d+ \w+ \w+ \w+)\twake\t(\w+)\t', l)
    if not m or m.group(4) not in ('exec', 'resume'): continue
    f = re.sub(r'[0-9:-]+$', '', m.group(1)); by = m.group(3).split(' ')[1:] if m.group(3) != '-' else ['-']
    c[(f or m.group(2), ' '.join(by))] += 1; tot += 1
print(f"text wakes {tot}")
for k, v in c.most_common(25): print(f"  {v} {k[0]} by {k[1]}")
