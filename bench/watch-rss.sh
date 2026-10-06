#!/usr/bin/env bash
# bench/watch-rss.sh DOCDIR [BIN]: a `phitex watch` session's memory and
# CPU, as a user's: the first build to its fixpoint, then EDITS one-word
# edits each followed by its revert, then SAVES saves with nothing
# changed, then IDLE seconds of nothing. After each step the watch is let
# settle (its rebuild reported, the store's save and every other thread
# done) and its resident set is sampled.
#
# DOCDIR holds MAIN (default course.tex) and `_out/` with the job's .aux,
# .out and .toc from a full build (as bench/edits.sh's). It is copied into
# DOCDIR/runs/watch-rss/ (sources and outputs): the edits are made there.
# The edits are chosen once, in the chapter files (`ch*.tex`, else every
# .tex file), each the longest word of a prose line (no TeX syntax on it)
# with its last letter doubled, written as editors save (a new file renamed
# over the old). Each step's line: what it was, the rebuild's report (none
# for a save with nothing changed), the resident set after it settled, the
# peak so far, and the CPU seconds the step took.
#
# Environment: EDITS (20), SAVES (20), IDLE (60), GAP (seconds after a
# rebuild's report before the next step; unset: wait until the watch
# settles), FORMATS (a format directory to share between runs), PRELOAD
# (a library partex alone runs with, as jemalloc for a heap profile with
# MALLOC_CONF), REV (the revision the last line names), MAIN, and
# any PARTEX_* switch (partex reads them). It runs partex itself: run it
# inside the sandbox, which passes only PARTEX_* through, so the others
# go in by `env` (on the course, only on accl or through scripts/heavy):
#
#   scripts/heavy scripts/sandbox env EDITS=20 bench/watch-rss.sh DOCDIR
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
[ $# -ge 1 ] || { sed -n '2,3p' "$0" >&2; exit 2; }
doc=$(cd "$1" && pwd)
bin=${2:-$repo/target/release/phitex}
main=${MAIN:-course.tex}
job=${main%.tex}
edits=${EDITS:-20}
saves=${SAVES:-20}
idle=${IDLE:-60}
run=$doc/runs/watch-rss
[ -f "$doc/_out/$job.aux" ] || { echo "watch-rss.sh: no _out/$job.aux in $doc" >&2; exit 2; }

rm -rf "$run"
mkdir -p "$run/src" "$run/out" "$run/store" "$run/cache"
(cd "$doc" && tar cf - --exclude=./runs --exclude=./_out --exclude=./_cache .) | (cd "$run/src" && tar xf -)
cp "$doc/_out/$job".* "$run/out/"
formats=${FORMATS:-$run/formats}
mkdir -p "$formats"

# The edits: file, line number, old line, new line (tab-separated).
python3 - "$run/src" "$edits" >"$run/edits.tsv" <<'EOF'
import os, re, sys
src, n = sys.argv[1], int(sys.argv[2])
files = sorted(f for f in os.listdir(src) if re.fullmatch(r"ch\d+\.tex", f))
if not files:
    files = sorted(f for f in os.listdir(src) if f.endswith(".tex"))
code = re.compile(r"\\begin\{(lstlisting|verbatim|minted|tikzpicture|axis|align\*?|equation\*?|tabular|tcblisting)\}")
prose = re.compile(r"[A-Za-z][A-Za-z ,.;:'()-]{48,}")
picked = []
for f in files:
    lines = open(os.path.join(src, f), encoding="utf-8", errors="surrogateescape").read().split("\n")
    depth, ok = 0, []
    for i, l in enumerate(lines):
        if code.search(l):
            depth += 1
        if depth == 0 and prose.fullmatch(l.strip()) and not re.search(r"[\\$%{}&#_^~]", l):
            ok.append(i)
        if re.search(r"\\end\{(lstlisting|verbatim|minted|tikzpicture|axis|align\*?|equation\*?|tabular|tcblisting)\}", l):
            depth = max(0, depth - 1)
    if ok:
        picked.append((f, ok[len(ok) // 2], lines))
k = 0
while len(picked) and k < n:
    f, i, lines = picked[k % len(picked)]
    old = lines[i]
    words = sorted(set(re.findall(r"[A-Za-z]{6,}", old)), key=lambda w: (-len(w), w))
    w = words[(k // len(picked)) % len(words)] if words else None
    if w is None:
        break
    new = re.sub(r"\b%s\b" % w, w + w[-1], old, count=1)
    print(f"{f}\t{i + 1}\t{old}\t{new}")
    k += 1
EOF
[ "$(wc -l <"$run/edits.tsv")" -ge "$edits" ] || { echo "watch-rss.sh: found $(wc -l <"$run/edits.tsv") edits, not $edits" >&2; exit 2; }

# set_line FILE LINE TEXT: line LINE of FILE becomes TEXT, as an editor
# saves (a new file renamed over the old one).
set_line() {
  python3 - "$1" "$2" "$3" <<'EOF'
import os, sys
path, k, text = sys.argv[1], int(sys.argv[2]), sys.argv[3]
lines = open(path, encoding="utf-8", errors="surrogateescape").read().split("\n")
lines[k - 1] = text
tmp = path + ".watch-rss"
with open(tmp, "w", encoding="utf-8", errors="surrogateescape") as f:
    f.write("\n".join(lines))
os.replace(tmp, path)
EOF
}
# resave FILE: the same bytes, saved again (a new modification time).
resave() {
  cp -p "$1" "$1.watch-rss"
  touch "$1.watch-rss"
  mv "$1.watch-rss" "$1"
}

cd "$run/src"
mkfifo "$run/stdin"
sleep 100000 >"$run/stdin" &
holder=$!
env ${PRELOAD:+LD_PRELOAD="$PRELOAD"} SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
  PARTEX_STORE_DIR="$run/store" PARTEX_CACHE_DIR="$run/cache" PARTEX_FORMATS="$formats" \
  "$bin" watch -v -o "$run/out" "$main" <"$run/stdin" >"$run/watch.out" 2>&1 &
top=$!
trap 'kill $holder 2>/dev/null; kill $top 2>/dev/null; true' EXIT

# partex's own process ($top, or under it if a wrapper runs it)
under() {
  local c
  echo "$1"
  for c in $(cat /proc/"$1"/task/*/children 2>/dev/null); do under "$c"; done
}
pid=
for _ in $(seq 600); do
  for p in $(under $top); do
    case $(cat /proc/"$p"/comm 2>/dev/null) in phitex | partex) pid=$p ;; esac
  done
  [ -n "$pid" ] && break
  sleep 0.1
done
[ -n "$pid" ] || { echo "watch-rss.sh: partex did not start" >&2; cat "$run/watch.out" >&2; exit 1; }

kb() { awk -v k="$1:" '$1 == k {print $2}' /proc/"$pid"/status; }
threads() { awk '$1 == "Threads:" {print $2}' /proc/"$pid"/status; }
ticks() { awk '{print $14 + $15}' /proc/"$pid"/stat; }
hz=$(getconf CLK_TCK)
reports() { grep -c "Machine \(built\|rebuilt\|loaded\|nothing\)" "$run/watch.out" || true; }
alive() { [ -d /proc/"$pid" ] || { echo "watch-rss.sh: partex ended" >&2; tail -20 "$run/watch.out" >&2; exit 1; }; }

# settle: until no thread but the watch's two (itself, its input) and
# under a quarter of a core used, for 3 s in a row (the store's save and
# the restart record done; the watch's own looking at its files goes on);
# at most 15 minutes
settle() {
  local calm=0 t0 t1 k=0
  if [ -n "${GAP:-}" ]; then sleep "$GAP"; return; fi
  t0=$(ticks)
  while [ $calm -lt 3 ] && [ $k -lt 900 ]; do
    alive
    sleep 1
    k=$((k + 1))
    t1=$(ticks)
    if [ "$(threads)" -le 2 ] && [ $((t1 - t0)) -le $((hz / 4)) ]; then calm=$((calm + 1)); else calm=0; fi
    t0=$t1
  done
  [ $k -lt 900 ] || echo "watch-rss.sh: did not settle in 15 minutes" >&2
}
# wait_report N: until N reports in all (a rebuild's, after its passes)
wait_report() {
  while [ "$(reports)" -lt "$1" ]; do alive; sleep 0.2; done
}
# step NAME: a line for the step just settled
c_last=0
step() {
  local c rss hwm line
  c=$(ticks)
  rss=$(kb VmRSS)
  hwm=$(kb VmHWM)
  line=$(grep "Machine \(built\|rebuilt\|loaded\)" "$run/watch.out" | tail -1 | sed 's/^ *Machine //' | cut -c1-70)
  [ "${2:-}" = norebuild ] && line="(no rebuild)"
  printf '| %s | %s | %s | %s | %s |\n' "$1" "$line" "$((rss / 1024))" "$((hwm / 1024))" \
    "$(awk -v d="$((c - c_last))" -v h="$hz" 'BEGIN{printf "%.1f", d / h}')"
  c_last=$c
}

printf '| step | report | RSS MB | peak MB | CPU s |\n|---|---|---:|---:|---:|\n'
wait_report 1
settle
step "build"
n=$(reports)
i=0
while IFS=$'\t' read -r f k old new; do
  i=$((i + 1))
  [ $i -le "$edits" ] || break
  set_line "$f" "$k" "$new"
  n=$((n + 1)); wait_report $n; settle
  step "edit $i ($f:$k)"
  set_line "$f" "$k" "$old"
  n=$((n + 1)); wait_report $n; settle
  step "revert $i"
done <"$run/edits.tsv"
for i in $(seq "$saves"); do
  f=$(cut -f1 "$run/edits.tsv" | sed -n "$(( (i - 1) % $(wc -l <"$run/edits.tsv") + 1 ))p")
  resave "$f"
  sleep 2
  settle
  if [ "$(reports)" -ne "$n" ]; then
    n=$(reports)
    step "save $i ($f), REBUILT"
  else
    step "save $i ($f)" norebuild
  fi
done
t0=$(ticks)
sleep "$idle"
t1=$(ticks)
step "idle ${idle} s" norebuild
printf '\nidle CPU: %s%% of a core over %s s\n' \
  "$(awk -v d="$((t1 - t0))" -v h="$hz" -v s="$idle" 'BEGIN{printf "%.2f", d * 100 / h / s}')" "$idle"
echo q >"$run/stdin"
for _ in $(seq 600); do [ -d /proc/"$pid" ] || break; sleep 0.1; done
rev=${REV:-$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo "?")}
printf '%s, %s edits and their reverts, %s saves with nothing changed; %s (repo at %s), %s.\n' \
  "$(basename "$doc")/$main" "$edits" "$saves" "$bin" "$rev" "$(date '+%Y-%m-%d %H:%M')"
