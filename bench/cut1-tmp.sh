#!/bin/bash
# cut1 bench (mc, inside the CI image via `accl run cmd bash bench/cut1-tmp.sh`):
# release CLIs of this tree (head) and of CONTROL (a commit, optional),
# interleaved in one job: clean builds (wall, peak RSS, md5) of the TikZ
# manual, amsldoc and para4000 at -j1/-j16, amsldoc's edit history at
# -j8/-j16 (gate-edits.py), and the TikZ-picture edits (cut1-edits.py).
#   BENCH="pgf ams para edits tikzedit grail"  (default: all but grail)
#   REPS=2  CONTROL=<commit>  OBS=1 (observe builds too, CK_OPS rows)
set -u
B=${BENCH:-pgf ams para edits tikzedit}
REPS=${REPS:-2}
here=$PWD
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TIME_LIMIT_SECS=600
t0=$(date +%s)
bins=()
build() { # label dir features
  local l=$1 d=$2 f=${3:-}
  ( cd "$d" && cargo build -q --release -p phitex-cli ${f:+--features $f} > "$r/build-$l.txt" 2>&1 ) \
    || { echo "BUILD FAILED $l: $(grep -m3 '^error' $r/build-$l.txt)"; return 1; }
  mkdir -p "$w/bins/$l" && cp "$d/target/release/phitex" "$w/bins/$l/phitex"
  bins+=("$l")
}
if [ -n "${CONTROL:-}" ]; then
  rm -rf "$w/control" && mkdir -p "$w/control" && git archive "$CONTROL" | tar -x -C "$w/control"
  [ -d upstream ] && ln -s "$PWD/upstream" "$w/control/upstream"
  build control "$w/control" || exit 1
fi
build head "$here" || exit 1
if [ "${OBS:-}" = 1 ]; then
  ( cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build-obs.txt" 2>&1 ) && mkdir -p $w/bins/obs && cp target/release/phitex $w/bins/obs/phitex
fi
echo "built in $(( $(date +%s) - t0 )) s: ${bins[*]}"
# (each label's formats warmed in its own cache)
mkdir -p "$w/warm" && printf '%s\n' '\documentclass{article}' '\usepackage{tikz}' '\begin{document}' 'x' '\end{document}' > "$w/warm/warm.tex"
for l in "${bins[@]}" obs; do
  [ -x "$w/bins/$l/phitex" ] || continue
  (cd "$w/warm" && PHITEX_CACHE_DIR=$w/cache-$l timeout 1800 "$w/bins/$l/phitex" build warm.tex > /dev/null 2>&1)
done
measure() { # label doc-dir main j tag [env...]
  local l=$1 d=$2 m=$3 j=$4 tag=$5; shift 5
  rm -rf "$w/run" && cp -a "$d" "$w/run"
  ( cd "$w/run" && env PHITEX_CACHE_DIR=$w/cache-$l "$@" python3 -I -c '
import resource, subprocess, sys, time, hashlib, os
t = time.monotonic()
p = subprocess.run(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=open(os.environ["ERRF"], "w"))
w = time.monotonic() - t
rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss // 1024
pdf = os.path.splitext(sys.argv[-1])[0] + ".pdf"
try: md5 = hashlib.md5(open(pdf, "rb").read()).hexdigest()[:12]
except OSError: md5 = "none"
print(f"{w:8.2f} s {rss:7d} MB md5 {md5} exit {p.returncode}")' \
    "$w/bins/$l/phitex" build --exact-pdf -j "$j" "$m" ) | sed "s|^|$tag $l -j$j: |" | tee -a "$r/rows.txt"
}
export ERRF=$r/last-stderr.txt
pgf=/usr/share/doc/texlive/generic/pgf
for rep in $(seq 1 "$REPS"); do
  for j in 1 16; do
    for l in "${bins[@]}"; do
      case " $B " in *" pgf "*) measure "$l" "$pgf" pgfmanual.tex $j pgf ;; esac
      case " $B " in *" ams "*) measure "$l" bench/amsldoc amsldoc.tex $j ams ;; esac
      case " $B " in *" para "*) mkdir -p $w/para && cp bench/para4000.tex $w/para/ && measure "$l" "$w/para" para4000.tex $j para ;; esac
    done
  done
done
if [ "${OBS:-}" = 1 ] && [ -x $w/bins/obs/phitex ]; then
  for ops in all none pic exec,resume; do
    e=(); [ $ops = all ] || e=(PHITEX_CK_OPS=$ops)
    case " $B " in *" pgf "*) measure obs "$pgf" pgfmanual.tex 1 "pgf-ckops-$ops" "${e[@]}" ;; esac
  done
fi
for l in "${bins[@]}"; do
  case " $B " in *" edits "*)
    for j in 8 16; do
      rm -rf "$w/amsh" "$w/amsh-cold" && cp -a bench/amsldoc "$w/amsh"
      PHITEX_CACHE_DIR=$w/cache-$l timeout 2400 python3 -I scripts/accl/gate-edits.py "$w/bins/$l/phitex" "$w/amsh" amsldoc.tex $j 20 $j "$r/amsedit-$l-j$j.json" --exact-pdf > /dev/null 2>&1
      python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1])); x = sorted(r["edit_walls_s"])
v = r["verdict"]
print(f"amsedit {sys.argv[2]} -j{sys.argv[3]}: {v} median {1000*x[len(x)//2]:.1f} ms p90 {1000*x[int(len(x)*0.9)]:.1f} ms n {len(x)}")' "$r/amsedit-$l-j$j.json" $l $j | tee -a "$r/rows.txt"
    done ;;
  esac
  case " $B " in *" tikzedit "*)
    rm -rf "$w/tz" && mkdir -p "$w/tz"
    cat > "$w/tz/tz.tex" << 'EOF'
\documentclass{article}
\usepackage{tikz}
\begin{document}
Most edits change one paragraph, and this one holds a picture of many circles.
\begin{tikzpicture}\foreach \i in {1,...,40}{\foreach \j in {1,...,60}{\draw (\i*0.3,\j*0.2) circle (0.05);}}\node at (0,-1) {label one};\end{tikzpicture}
And words after the picture.

Another paragraph.
\begin{tikzpicture}
\draw (0,0) -- (1,1);
\draw (0,1) -- (1,0);
\foreach \i in {1,...,800}{\draw (\i*0.01,0) circle (0.05);}
\node at (0,-1) {label two};
\end{tikzpicture}
Last words.
\end{document}
EOF
    echo '[["tz.tex","Most edits","Many edits",4],["tz.tex","label one","label uno",2],["tz.tex","label two","label dos",2],["tz.tex","Last words","Final words",2]]' > "$w/tz/edits.json"
    for j in 1 16; do
      rm -rf "$w/tzr" "$w/tzr-cold" && cp -a "$w/tz" "$w/tzr"
      PHITEX_CACHE_DIR=$w/cache-$l timeout 1800 python3 -I bench/cut1-edits.py "$w/bins/$l/phitex" "$w/tzr" tz.tex $j "$w/tzr/edits.json" "$r/tzedit-$l-j$j.json" --exact-pdf 2>&1 | sed "s|^|tzedit $l -j$j: |" | tee -a "$r/rows.txt"
    done ;;
  esac
  case " $B " in *" grail "*)
    if [ -d "$w/bin/grail-src" ]; then
      ln=${EDIT_LINE:-260}
      for j in 1 16; do
        rm -rf "$w/gr" "$w/gr-cold" && cp -a "$w/bin/grail-src" "$w/gr"
        python3 -I -c '
import json, sys
t = open(sys.argv[1], encoding="latin-1").read().split("\n")
k = int(sys.argv[2]) - 1
while " the " not in t[k] or t[k].lstrip().startswith(("%", "\\")): k += 1
s = t[k]; i = s.index(" the ")
a = s[max(0, i - 12): i + 12]
print(json.dumps([["grail.tex", a, a.replace(" the ", " the typed ", 1), 4]]))' "$w/gr/grail.tex" $ln > "$w/gr-edits.json"
        PHITEX_CACHE_DIR=$w/cache-$l timeout 3000 python3 -I bench/cut1-edits.py "$w/bins/$l/phitex" "$w/gr" grail.tex $j "$w/gr-edits.json" "$r/grail-$l-j$j.json" --exact-pdf 2>&1 | sed "s|^|grail $l -j$j: |" | tee -a "$r/rows.txt"
      done
    fi ;;
  esac
done
echo "bench wall $(( $(date +%s) - t0 )) s"
