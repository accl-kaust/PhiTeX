#!/bin/bash
# Single-thread engine speed by two binaries, alternating on one node
# (job.sbatch `cmd` task, run from the checkout; $w is the workspace, $r
# the results directory, the course in $w/course and its aux in $w/aux):
#
#   expand-ab.sh OLD [ROUNDS] [DOCS] [MODES]
#
# OLD is built in a worktree, the checkout is NEW. Each binary makes its
# own formats (pdflatex.fmt, and pdftex.fmt from pdfetex.ini for plain
# TeX). The documents (DOCS, default "pgfsub pgfman course macro"):
# pgfsub (bench/inputs/pgfsub.tex in upstream/pgf's manual), pgfman (the
# whole manual), course ($w/course), macro (bench/inputs/macro.tex, plain
# TeX); the pgf documents' .aux settled by three plain builds first. Each
# is built cold, ROUNDS times (2) per binary, OLD and NEW in turn, in each
# mode (MODES, default "pdflatex plain machine build ssa"):
#   pdflatex  TeX Live's pdflatex (pdftex for plain TeX), once a round
#   plain     phitex --compat=pdftex (no tracking)
#   machine   the same with PARTEX_MACHINE=1
#   build     `phitex build` (the CLI default), a fresh cache each time
#   ssa       PARTEX_SSA=1 PARTEX_SSA_TRIPS=1 (the extension's mode)
# Each run's wall time and peak RSS (GNU time) go to $r/runs.tsv; the
# medians by document, mode and binary to $r/summary.txt.
set -uo pipefail
old=${1:?old commit} n=${2:-2}
docs=${3:-pgfsub pgfman course macro}
modes=${4:-pdflatex plain machine build ssa}
P=$PWD
git worktree add -q --detach "$w/old" "$old" || exit 1
(cd "$w/old" && cargo build --release -p partex-cli -q) || exit 1
cargo build --release -p partex-cli -q || exit 1
newc=$(git rev-parse --short HEAD) oldc=$(git -C "$w/old" rev-parse --short HEAD)
bin() { if [ "$1" = old ]; then echo "$w/old/target/release/phitex"; else echo "$P/target/release/phitex"; fi; }
env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1)
# formats, per binary
for which in old new; do
  F=$w/fmt-$which B=$(bin $which)
  mkdir -p "$F"
  (cd "$F" && ln -sf "$B" pdftex &&
    env "${env0[@]}" ./pdftex -ini -interaction=batchmode -jobname=pdflatex \
      -translate-file=cp227.tcx '*pdflatex.ini' </dev/null >/dev/null 2>&1
    env "${env0[@]}" ./pdftex -ini -interaction=batchmode -jobname=pdftex '*pdfetex.ini' \
      </dev/null >/dev/null 2>&1
    rm -f pdftex)
  [ -f "$F/pdflatex.fmt" ] && [ -f "$F/pdftex.fmt" ] || { echo "FORMAT FAILED ($which)"; exit 1; }
done
# the documents: source directory, main file, settled aux directory
D=$w/pgf
cp -a upstream/pgf/doc/generic/pgf "$D" && cp bench/inputs/pgfsub.tex "$D/"
mkdir -p "$w/macro" && cp bench/inputs/macro.tex "$w/macro/"
src() { case $1 in pgfsub | pgfman) echo "$D" ;; course) echo "$w/course" ;; macro) echo "$w/macro" ;; esac; }
main() { case $1 in pgfsub) echo pgfsub.tex ;; pgfman) echo pgfmanual.tex ;; course) echo course.tex ;; macro) echo macro.tex ;; esac; }
fmt() { case $1 in macro) echo pdftex ;; *) echo pdflatex ;; esac; }
aux() { case $1 in course) echo "$w/aux" ;; macro) echo "" ;; *) echo "$w/aux-$1" ;; esac; }
for d in $docs; do
  case $d in pgfsub | pgfman)
    A=$(aux $d) && mkdir -p "$A"
    for i in 1 2 3; do
      (cd "$(src $d)" && env "${env0[@]}" "$(bin new)" --compat=pdftex -fmt="$w/fmt-new/pdflatex" \
        -interaction=batchmode -output-directory="$A" "$(main $d)" </dev/null >/dev/null 2>&1)
    done
    rm -f "$A"/*.pdf "$A"/*.log ;;
  esac
done
# one run: doc mode which round
run() {
  local d=$1 m=$2 which=$3 i=$4 B F O S M e t
  B=$(bin "$which") F=$w/fmt-$which O=$w/out/$d-$m-$which S=$(src "$d") M=$(main "$d")
  rm -rf "$O" && mkdir -p "$O"
  [ -n "$(aux "$d")" ] && cp -a "$(aux "$d")"/. "$O"/
  local cmd=("$B" --compat=pdftex -fmt="$F/$(fmt "$d")" -interaction=batchmode -output-directory="$O" "$M")
  local ev=("${env0[@]}")
  case $m in
    pdflatex) if [ "$(fmt "$d")" = pdftex ]; then cmd[0]=pdftex; else cmd[0]=pdflatex; fi
      unset 'cmd[1]' 'cmd[2]' ;;
    machine) ev+=(PARTEX_MACHINE=1) ;;
    ssa) ev+=(PARTEX_SSA=1 PARTEX_SSA_TRIPS=1) ;;
    build) rm -rf "$w/cache-$which"
      ev+=(PARTEX_CACHE_DIR="$w/cache-$which" PARTEX_STORE_DIR="$w/cache-$which/store"
        PARTEX_FORMATS="$w/modern-fmt-$which" NO_COLOR=1)
      # (the engine named: pgfsub.tex has no \documentclass of its own)
      cmd=("$B" build -q --engine "$(fmt "$d")" -o "$O" "$M") ;;
  esac
  (cd "$S" && /usr/bin/time -f '%e %M' -o "$O/.time" timeout -s KILL 7200 env "${ev[@]}" "${cmd[@]}" \
    </dev/null >"$O/.term" 2>"$O/.err")
  e=$?
  t=$(tail -1 "$O/.time")
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$d" "$m" "$which" "$i" "$e" "$t" | tee -a "$r/runs.tsv"
}
# (`build` makes its formats once per binary, untimed)
for which in old new; do
  case " $modes " in *" build "*)
    for d in $docs; do run "$d" build "$which" 0 >/dev/null; break; done ;;
  esac
done
printf 'doc\tmode\tbinary\tround\texit\twall_s rss_kb\n' >"$r/runs.tsv"
for i in $(seq "$n"); do
  for d in $docs; do
    for m in $modes; do
      if [ "$m" = pdflatex ]; then run "$d" pdflatex new "$i"; continue; fi
      for which in old new; do run "$d" "$m" "$which" "$i"; done
    done
  done
done
# medians
{
  echo "old $oldc, new $newc, $n rounds, node $(hostname)"
  python3 - "$r/runs.tsv" <<'EOF'
import sys, statistics, collections
rows = [l.rstrip('\n').split('\t') for l in open(sys.argv[1])][1:]
t = collections.defaultdict(list)
for d, m, b, i, e, x in rows:
    w = x.split()[0] if x else 'nan'
    t[(d, m, b)].append((float(w), e))
docs = list(dict.fromkeys(r[0] for r in rows))
modes = list(dict.fromkeys(r[1] for r in rows))
print(f"{'doc':8} {'mode':9} {'old s':>8} {'new s':>8} {'new/old':>8}  exits")
for d in docs:
    for m in modes:
        o, n = t.get((d, m, 'old'), []), t.get((d, m, 'new'), [])
        mo = statistics.median(x for x, _ in o) if o else float('nan')
        mn = statistics.median(x for x, _ in n) if n else float('nan')
        ex = ','.join(e for _, e in o + n)
        print(f"{d:8} {m:9} {mo:8.2f} {mn:8.2f} {mn / mo if o else float('nan'):8.3f}  {ex}")
EOF
} | tee -a "$r/summary.txt"
