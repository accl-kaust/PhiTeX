#!/bin/bash
# (temporary) boxscope (PHITEX_BOXSCOPE) against off on a small file: a clean build's passes
# and a few edits under watch, -j1. Minutes, not the TikZ manual.
set -u
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-prof cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-prof/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1 PHITEX_TIME_LIMIT_SECS=600
d=$w/s; rm -rf "$d"; mkdir -p "$d"
cat > "$d/s.tex" << 'TEX'
\documentclass{article}
\usepackage{tikz}
\usepackage{hyperref}
\begin{document}
\tableofcontents
\section{One}\label{s:one}
See section~\ref{s:two} on page~\pageref{s:two}. Most edits change one paragraph, and this one holds a picture of many circles.
\begin{tikzpicture}\foreach \i in {1,...,40}{\foreach \j in {1,...,60}{\draw (\i*0.3,\j*0.2) circle (0.05);}}\node at (0,-1) {label one};\end{tikzpicture}
And words after the picture.

A paragraph with an inline picture \tikz{\foreach \i in {1,...,300}{\draw (\i*0.01,0) circle (0.05);}\node at (0,-1) {page \thepage};} that reads the page number.

A box of text and a picture: \mbox{before \tikz{\foreach \i in {1,...,300}{\draw (\i*0.01,0) circle (0.04);}} after, section~\ref{s:two}}.
\newpage
\section{Two}\label{s:two}
Another paragraph.
\begin{tikzpicture}
\draw (0,0) -- (1,1);
\draw (0,1) -- (1,0);
\foreach \i in {1,...,800}{\draw (\i*0.01,0) circle (0.05);}
\node at (0,-1) {label two};
\end{tikzpicture}
Last words.
\end{document}
TEX
echo '[["s.tex","label one","label uno",2],["s.tex","label two","label dos",2],["s.tex","Most edits","Many edits",2],["s.tex","\\section{One}","\\section{Zero}Text.\\section{One}",2]]' > "$w/edits.json"
for m in off box; do
  e=(); [ $m = off ] || e=(PHITEX_BOXSCOPE=1 PHITEX_BOXSCOPE_TRACE=1 PHITEX_TEX_TIME=1)
  echo "== $m: clean build -j1"; rm -rf "$w/c" "$w/cache-$m"; cp -a "$d" "$w/c"
  ( cd "$w/c" && /usr/bin/time -f "%e s %M KB" env PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" "$bin" build --exact-pdf -j 1 s.tex > "$r/clean-$m.txt" 2>&1; echo "exit $?"; tail -1 "$r/clean-$m.txt"; md5sum s.pdf; cp s.pdf "$r/clean-$m.pdf" )
  echo "leaves: $(grep -c '^boxscope: node .* leaves' "$r/clean-$m.txt")"
  grep "^boxscope: node .* stays" "$r/clean-$m.txt" | sed 's/node [0-9]* //' | cut -c1-160 | sort | uniq -c | sort -rn | head -12
  echo "== $m: edits under watch -j1"; rm -rf "$w/e" "$w/e-cold" "$r/watch-$m.txt"; cp -a "$d" "$w/e"
  env F52_ERR=$r/watch-$m.txt PHITEX_RUN_TIMES=1 PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" timeout 900 python3 -I bench/f52-edits-tmp.py "$bin" "$w/e" s.tex 1 "$w/edits.json" "$r/edits-$m.json" --exact-pdf 2>&1 | tail -12
  md5sum "$w/e/s.pdf"
  grep "^boxscope: node .* ends a paragraph" "$r/watch-$m.txt" | head -5
done
# (each edit's runs, both modes)
for m in off box; do echo "== $m: runs per edit (op node ms)"; awk '/^=== edit/ {print; on=1; next} on && /^runtime:/ {printf "  %s %s %.1f\n", $2, $3, $5-$4} on && /^boxscope/ {print "  " substr($0,1,140)}' "$r/watch-$m.txt" | head -150; done
for m in off box; do echo "== $m: engine accounting per edit"; awk '/^=== edit/ {print; on=1; next} on && /^acct: wall|^acct-edit|^acct: commits by op|re-ask/ {print "  " substr($0,1,260)}' "$r/watch-$m.txt" | head -80; done
