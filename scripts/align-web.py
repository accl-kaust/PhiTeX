#!/usr/bin/env python3
# Align pdftex-merged.web with tex-merged.web section by section (run from
# the repository root after scripts/merge-web.sh). Writes
# target/web/pdftex-align.tsv: same|changed|added|removed, tex §, pdftex §.
import re,sys,difflib,collections
def sections(path):
    secs=[];cur=[]
    for l in open(path,errors='replace').read().split('\n'):
        if re.match(r"^@( |\*|$|\t)",l):
            secs.append('\n'.join(cur)); cur=[]
        cur.append(l)
    secs.append('\n'.join(cur))
    return secs[1:]  # drop limbo
def norm(s): return re.sub(r'\s+',' ',s).strip()
tex=sections('target/web/tex-merged.web')
pdf=sections('target/web/pdftex-merged.web')
idx={}
for i,s in enumerate(tex): idx.setdefault(norm(s),i+1)
# sequence alignment via SequenceMatcher on normalized sections
a=[norm(s) for s in tex]; b=[norm(s) for s in pdf]
sm=difflib.SequenceMatcher(None,a,b,autojunk=False)
same=changed=added=removed=0
rows=[]
for tag,i1,i2,j1,j2 in sm.get_opcodes():
    if tag=='equal': same+=i2-i1; rows+=[('same',i+1,j+1) for i,j in zip(range(i1,i2),range(j1,j2))]
    elif tag=='replace':
        n=min(i2-i1,j2-j1); changed+=n
        rows+=[('changed',i1+k+1,j1+k+1) for k in range(n)]
        if j2-j1>n: added+=j2-j1-n; rows+=[('added',None,j+1) for j in range(j1+n,j2)]
        if i2-i1>n: removed+=i2-i1-n; rows+=[('removed',i+1,None) for i in range(i1+n,i2)]
    elif tag=='insert': added+=j2-j1; rows+=[('added',None,j+1) for j in range(j1,j2)]
    elif tag=='delete': removed+=i2-i1; rows+=[('removed',i+1,None) for i in range(i1,i2)]
print(f"tex sections {len(tex)}, pdftex sections {len(pdf)}")
print(f"same {same}, changed {changed}, added {added}, removed {removed}")
with open('target/web/pdftex-align.tsv','w') as f:
    for r in rows: f.write(f"{r[0]}\t{r[1]}\t{r[2]}\n")
