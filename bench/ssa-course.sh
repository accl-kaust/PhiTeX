#!/usr/bin/env bash
# bench/ssa-course.sh [--warm N] [COURSE [AUX]]: the course's edits as
# the rebuilds of ONE `PARTEX_SSA=1` process (DESIGN 4.3), measured.
#
# COURSE is the course's sources (default ~/code/tmp/np-course), AUX the
# directory holding the job's .aux, .out and .toc from a full build
# (default COURSE/_out); in an accl job (`scripts/accl/accl run cmd sh -c
# 'bench/ssa-course.sh "$w/course" "$w/aux"'`) they are $w/course and
# $w/aux. A private copy is made afresh in target/ssa-course/run; COURSE
# itself is only read. One process builds the job cold, then rebuilds it
# after each edit of EDITS (default bench/edits/course.txt,
# bench/edits.sh's format), each followed by its revert (the original
# text) and the revert's rebuild; the steps of a `;;` spec are rebuilt
# one by one before the revert. Then `word` and its revert N more times
# (default 20), warm.
# Each rebuild is one pass: a `\label`'s `.aux` is read by the next
# rebuild, here its revert's. A last rebuild with no edit closes the
# count of the one before it and is not reported.
#
# An edit is made as an editor saves: the new text is written beside the
# file and renamed over it. The process runs alone on the machine and
# under the memory cap (scripts/heavy: HEAVY_VMEM_GB, HEAVY_TIMEOUT), in
# the sandbox, inside GNU time for its peak RSS. The format is made by
# the binary (BIN, default target/release/partex, built if missing), once
# per binary, as
# the course's own: `-ini -jobname=pdflatex -translate-file=cp227.tcx
# *pdflatex.ini`, at the jobs' fixed time (it dumps `\time`, which a job
# sets again when it starts). PARTEX_* variables set by the caller reach
# the SSA process (single-line values: the sandbox passes them line by
# line; the rebuild lines are passed inside the sandbox).
#
# Wall-clock times depend on the machine's load; the load-independent
# numbers are the counts (steps, commands, reads checked, readers marked,
# records made), the peak RSS, and the user-space instructions per phase
# (`perf stat -e instructions:u`): the cold build's (counted by a perf
# around the process until the first rebuild line disables it), and each
# rebuild's with its link (a perf attached to the process by the rebuild's
# line, enabled before the rebuild starts and stopped by the next line;
# it includes the next line's shell, about a million). INSN=0, or no
# perf that counts: no instructions.
#
# Output: a Markdown table on stdout and the JSON in OUT (default
# bench/results/<commit>-ssa-course.json): per rebuild, its ms (rebuild,
# link), counts and instructions; the cold build's; the warm `word`
# edits' medians; the peak RSS; the load before and after; and whether
# the final outputs, every edit reverted, are the cold build's byte for
# byte (the log masked as e2e masks it). NOTE=... adds a note to the
# JSON. Exit 1 if the process failed. HEAVY=0 runs it without
# scripts/heavy: for a small document only (COURSE, MAIN, EDITS).
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
n=20
if [ "${1:-}" = --warm ]; then n=$2; shift 2; fi
course=$(realpath "${1:-$HOME/code/tmp/np-course}")
aux=$(realpath "${2:-$course/_out}")
bin=${BIN:-$repo/target/release/partex}
edits=$(realpath "${EDITS:-$repo/bench/edits/course.txt}")
main=${MAIN:-course.tex}
job=${main%.tex}
insn=${INSN:-1}
dir=$repo/target/ssa-course
rev=$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo unknown)
dirty=false
git -C "$repo" diff --quiet HEAD 2>/dev/null || dirty=true
out=${OUT:-$repo/bench/results/$rev-ssa-course.json}

[ -f "$course/$main" ] || { echo "ssa-course: no $course/$main" >&2; exit 2; }
[ -f "$aux/$job.aux" ] || { echo "ssa-course: no $aux/$job.aux" >&2; exit 2; }
if [ ! -x "$bin" ] && [ -z "${BIN:-}" ]; then
  echo "ssa-course: building $bin" >&2
  (cd "$repo" && scripts/sandbox cargo build --release -q -p partex-cli >&2)
fi
[ -x "$bin" ] || { echo "ssa-course: no binary $bin" >&2; exit 2; }
if [ "$insn" = 1 ] && ! "$repo"/scripts/sandbox perf stat -e instructions:u -x, -o /dev/null \
  -- true >/dev/null 2>&1; then
  echo "ssa-course: no perf that counts instructions:u here; INSN=0" >&2
  insn=0
fi

# The copy, afresh (the edits are made in it).
rm -rf "$dir/run" "$dir/plan" "$dir/time.txt" "$dir/err.txt" "$dir/term.txt"
mkdir -p "$dir/run" "$dir/plan/insn" "$dir/formats"
cp -a "$course/." "$dir/run/"
rm -rf "$dir/run/out" && mkdir "$dir/run/out"
for f in "$aux/$job".*; do cp "$f" "$dir/run/out/"; done

# The format, made by this binary once.
sha=$(sha256sum "$bin" | cut -c1-16)
fmt=$dir/formats/$sha
if [ ! -f "$fmt/pdflatex.fmt" ]; then
  rm -rf "$dir/formats" && mkdir -p "$fmt"
  echo "ssa-course: making the format" >&2
  (cd "$fmt" && "$repo"/scripts/sandbox env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 \
    "$bin" --compat=pdftex -ini -interaction=batchmode -jobname=pdflatex \
    -translate-file=cp227.tcx '*pdflatex.ini' </dev/null >/dev/null 2>&1) || true
  [ -f "$fmt/pdflatex.fmt" ] || { echo "ssa-course: no format made; see $fmt" >&2; exit 1; }
fi
rm -rf "$dir/run/_fmt" && mkdir "$dir/run/_fmt"
cp "$fmt/pdflatex.fmt" "$dir/run/_fmt/"

# The phases' instruction counts (see above), switched by the rebuild
# lines: `end` closes the phase before, `start K PID` opens rebuild K's.
# It never fails: a failed line would stop the rebuilds.
cat >"$dir/plan/phase.sh" <<'EOF'
d=$(dirname "$0")/insn
case $1 in
end)
  if [ -f "$d/perf.pid" ]; then
    k=$(cat "$d/phase")
    kill -INT "$(cat "$d/perf.pid")" 2>/dev/null
    rm -f "$d/perf.pid"
    i=0
    while ! grep -q instructions "$d/$k.csv" 2>/dev/null && [ $i -lt 1000 ]; do
      sleep 0.01
      i=$((i + 1))
    done
  else
    timeout 10 sh -c 'echo disable > "$0" && read -r a < "$1"' "$d/ctl0" "$d/ack0"
  fi ;;
start)
  rm -f "$d/ctl" "$d/ack"
  mkfifo "$d/ctl" "$d/ack"
  perf stat -e instructions:u -x, -o "$d/$2.csv" -p "$3" -D -1 \
    --control "fifo:$d/ctl,$d/ack" </dev/null >/dev/null 2>>"$d/perf.err" &
  echo $! >"$d/perf.pid"
  echo "$2" >"$d/phase"
  timeout 10 sh -c 'echo enable > "$0" && read -r a < "$1"' "$d/ctl" "$d/ack" ;;
esac
exit 0
EOF

# The plan: each rebuild's files (plan/K/PATH) and its line.
"$repo"/scripts/sandbox python3 - "$dir/run" "$edits" "$n" "$dir/plan" "$insn" <<'EOF'
import json, os, shlex, sys
run, edits, n, plan, insn = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5]
specs = []
for line in open(edits, encoding="utf-8"):
    line = line.rstrip("\n")
    if line and not line.startswith("#"):
        name, spec = line.split("\t", 1)
        specs.append((name, spec.replace("<NL>", "\n")))
original = {}
def text(path):
    if path not in original:
        original[path] = open(os.path.join(run, path), "rb").read()
    return original[path]
rebuilds = []  # (what, {path: bytes})
def sequence(name, spec, warm=0):
    steps, now = spec.split(";;"), {}
    for i, step in enumerate(steps, 1):
        for e in step.split("||"):
            path, frm, to = e.split("|", 2)
            t = now.get(path, text(path))
            k = t.count(frm.encode())
            if k != 1:
                sys.exit(f"ssa-course: `{frm}` occurs {k} times in {path} ({name})")
            now[path] = t.replace(frm.encode(), to.encode(), 1)
        rebuilds.append(({"name": name, "step": i, "steps": len(steps), "revert": False,
                          "warm": warm}, dict(now)))
    rebuilds.append(({"name": name, "step": 0, "steps": len(steps), "revert": True,
                      "warm": warm}, {p: text(p) for p in now}))
for name, spec in specs:
    sequence(name, spec)
word = dict(specs).get("word")
if word is None:
    sys.exit("ssa-course: the edits have no `word`")
for w in range(1, n + 1):
    sequence("word", word, w)
q = shlex.quote
phase = f"sh {q(os.path.join(plan, 'phase.sh'))}"
lines = []
for k, (what, files) in enumerate(rebuilds + [(None, {})], 1):
    cmds = [f"{phase} end"] if insn == "1" else []
    if k == 1:  # (the cold build's outputs, for the final comparison)
        cmds.append(f"cp -a out {q(os.path.join(plan, 'cold'))}")
    for path, data in files.items():
        p = os.path.join(plan, str(k), path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "wb").write(data)
        cmds.append(f"cp {q(p)} {q(path)}.ssa-new && mv -f {q(path)}.ssa-new {q(path)}")
    if insn == "1" and what is not None:
        # ($PPID: the line's shell's parent, the SSA process)
        cmds.append(f"{phase} start {k} $PPID")
    lines.append(" && ".join(cmds) or "true")
json.dump([what for what, _ in rebuilds], open(os.path.join(plan, "rebuilds.json"), "w"))
open(os.path.join(plan, "lines.txt"), "w").write("\n".join(lines))
EOF

# The build and its rebuilds: one process, alone, capped.
heavy=("$repo"/scripts/heavy)
[ "${HEAVY:-1}" = 0 ] && heavy=()
count=()
if [ "$insn" = 1 ]; then
  mkfifo "$dir/plan/insn/ctl0" "$dir/plan/insn/ack0"
  count=(perf stat -e instructions:u -x, -o "$dir/plan/insn/cold.csv"
    --control "fifo:$dir/plan/insn/ctl0,$dir/plan/insn/ack0" --)
fi
echo "ssa-course: the cold build and $(($(grep -c '' "$dir/plan/lines.txt") - 1)) rebuilds" \
  "${heavy:+(waiting for the heavy lock)}" >&2
load0=$(cut -d' ' -f1-3 /proc/loadavg)
t0=$(date +%s.%N)
status=0
(cd "$dir/run" && "${heavy[@]}" "$repo"/scripts/sandbox /usr/bin/time -v -o "$dir/time.txt" \
  "${count[@]}" env PARTEX_SSA=1 PARTEX_SSA_REBUILD="$(cat "$dir/plan/lines.txt")" \
  SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 \
  "$bin" --compat=pdftex -fmt=_fmt/pdflatex -interaction=batchmode -output-directory=out "$main" \
  </dev/null >"$dir/term.txt" 2>"$dir/err.txt") || status=$?
t1=$(date +%s.%N)
load1=$(cut -d' ' -f1-3 /proc/loadavg)

"$repo"/scripts/sandbox python3 - "$dir" "$out" "$status" "$rev" "$dirty" "$bin" "$edits" "$n" \
  "$load0" "$load1" "$t0" "$t1" "${NOTE:-}" <<'EOF'
import datetime, json, os, re, socket, statistics, sys
(d, out, status, rev, dirty, binary, edits, n, load0, load1, t0, t1, note) = sys.argv[1:]
err = open(os.path.join(d, "err.txt"), errors="replace").read()
whats = json.load(open(os.path.join(d, "plan", "rebuilds.json")))
def num(pattern, text, conv=int):
    m = re.search(pattern, text) if text is not None else None
    return conv(m.group(1)) if m else None
def made(routines):  # (the records a build made: the sum over its routines)
    return sum(int(x) for x in re.findall(r"records (\d+)", routines)) if routines else None
def instructions(name):  # (perf's CSV: count,unit,event,...)
    try:
        for line in open(os.path.join(d, "plan", "insn", f"{name}.csv")):
            fields = line.split(",")
            if len(fields) > 2 and fields[2].startswith("instructions") and fields[0].isdigit():
                return int(fields[0])
    except OSError:
        pass
    return None
cold = re.search(r"^partex: ssa build 0: ([\d.]+) ms, (.*)$", err, re.M)
cold_rest = cold.group(2) if cold else None
cold_routines = re.search(r"^partex: ssa build 0 routines: (.*)$", err, re.M)
result = {
    "commit": rev, "dirty": dirty == "true",
    "date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
    "host": socket.gethostname(), "cpus": os.cpu_count(),
    "load": [load0, load1], "binary": binary,
    "document": "the course (295 pages), its edits as the rebuilds of one PARTEX_SSA=1 "
                "process, each followed by its revert; one pass per rebuild",
    "edits": os.path.relpath(edits), "warm_repetitions": int(n), "note": note,
    "exit": int(status), "wall_s": round(float(t1) - float(t0), 1),
    "cold": {
        "ms": float(cold.group(1)) if cold else None,
        "link_ms": num(r"^partex: ssa build 0: link ([\d.]+) ms", err, float),
        "commands": num(r"\bcommands (\d+)", cold_rest),
        "records": num(r"\brecords (\d+)", cold_rest),
        "records_made": made(cold_routines.group(1) if cold_routines else None),
        "instructions": instructions("cold"),
    },
    "rebuilds": [],
}
time_v = open(os.path.join(d, "time.txt"), errors="replace").read() \
    if os.path.exists(os.path.join(d, "time.txt")) else ""
result["peak_rss_kb"] = num(r"Maximum resident set size \(kbytes\): (\d+)", time_v)
for k, what in enumerate(whats, 1):
    m = re.search(rf"^partex: ssa rebuild {k}: ([\d.]+) ms \(the rebuild ([\d.]+) ms, "
                  rf"the link ([\d.]+) ms\)(.*)$", err, re.M)
    routines = re.search(rf"^partex: ssa rebuild {k} routines: (.*)$", err, re.M)
    row = {"n": k, **what}
    if m:
        rest = m.group(4)
        row.update(ms=float(m.group(1)), rebuild_ms=float(m.group(2)),
                   link_ms=float(m.group(3)), steps_run=num(r"\bsteps run (\d+)", rest),
                   commands=num(r"\bcommands (\d+)", rest),
                   reads_checked=num(r"\breads checked (\d+)", rest),
                   readers_marked=num(r"\breaders marked (\d+)", rest),
                   definitions_changed=num(r"\bdefinitions changed (\d+)", rest),
                   records_made=made(routines.group(1) if routines else None),
                   instructions=instructions(str(k)))
    stopped = re.search(rf"^partex: ssa rebuild {k}: stopped: (.*)$", err, re.M)
    if stopped:
        row["stopped"] = stopped.group(1)
    result["rebuilds"].append(row)
panic = next((l for l in err.splitlines() if "panicked at" in l), None)
if panic:
    result["panic"] = panic
warm = [r for r in result["rebuilds"] if r["warm"] and "ms" in r]
for kind, rows in (("edit", [r for r in warm if not r["revert"]]),
                   ("revert", [r for r in warm if r["revert"]])):
    if rows:
        insn = [r["instructions"] for r in rows if r.get("instructions") is not None]
        result[f"warm_word_{kind}"] = {
            "ms": [r["ms"] for r in rows],
            "median_ms": statistics.median(r["ms"] for r in rows),
            "median_rebuild_ms": statistics.median(r["rebuild_ms"] for r in rows),
            "median_link_ms": statistics.median(r["link_ms"] for r in rows),
            "instructions": insn,
            "median_instructions": statistics.median(insn) if insn else None,
        }
# the final outputs against the cold build's (every edit reverted)
masks = [re.compile(p, re.M) for p in (
    rb"^ \d+ strings? out of \d+$", rb"^ \d+ string characters out of \d+$",
    rb"^ \d+ multiletter control sequences out of \d+\+\d+$",
    rb"^ \d+ words of font info for \d+ fonts?, out of \d+ for \d+$",
    rb"^ \d+ hyphenation exceptions? out of \d+$",
    rb"^ \d+i,\d+n,\d+p,\d+b,\d+s stack positions out of .*$",
    rb"\d+ words of memory out of \d+",
    rb"^ \d+ words of extra memory for PDF output out of \d+ \(max\. \d+\)$")]
def masked(b):
    b = b[b.find(b"\n") + 1:]
    for p in masks:
        b = p.sub(b"MASKED", b)
    return b
cold_dir, final_dir = os.path.join(d, "plan", "cold"), os.path.join(d, "run", "out")
if os.path.isdir(cold_dir) and all("ms" in r for r in result["rebuilds"]):
    differ = []
    for f in sorted(set(os.listdir(cold_dir)) | set(os.listdir(final_dir))):
        a, b = os.path.join(cold_dir, f), os.path.join(final_dir, f)
        if not (os.path.isfile(a) and os.path.isfile(b)):
            differ.append(f)
            continue
        x, y = open(a, "rb").read(), open(b, "rb").read()
        if f.endswith(".log"):
            x, y = masked(x), masked(y)
        if x != y:
            differ.append(f)
    result["final_differs_from_cold"] = differ
os.makedirs(os.path.dirname(out), exist_ok=True)
json.dump(result, open(out, "w"), indent=1)
# the table
def f(x, p=1):
    return "-" if x is None else f"{x:.{p}f}" if isinstance(x, float) else str(x)
def g(x):  # (instructions, in millions)
    return "-" if x is None else f"{x / 1e6:,.1f}"
c = result["cold"]
print(f"cold build: {f(c['ms'] and c['ms'] / 1000)} s (link {f(c['link_ms'])} ms), "
      f"{f(c['commands'])} commands, {f(c['records'])} records, "
      f"{g(c['instructions'])} M instructions:u; under load {load0.split()[0]}")
print()
print("| # | edit | ms | rebuild ms | link ms | steps | commands | reads checked "
      "| readers marked | records made | M instructions |")
print("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
for r in result["rebuilds"]:
    if r["warm"]:
        continue
    what = r["name"] + (" (revert)" if r["revert"] else
                        f" #{r['step']}" if r["steps"] > 1 else "")
    print(f"| {r['n']} | {what} | {f(r.get('ms'))} | {f(r.get('rebuild_ms'))} | "
          f"{f(r.get('link_ms'))} | {f(r.get('steps_run'))} | {f(r.get('commands'))} | "
          f"{f(r.get('reads_checked'))} | {f(r.get('readers_marked'))} | "
          f"{f(r.get('records_made'))} | {g(r.get('instructions'))} |"
          + (f" stopped: {r['stopped']}" if "stopped" in r else ""))
for kind in ("edit", "revert"):
    w = result.get(f"warm_word_{kind}")
    if w:
        print(f"| | word {kind}, {len(w['ms'])} warm, median | {f(w['median_ms'])} | "
              f"{f(w['median_rebuild_ms'])} | {f(w['median_link_ms'])} | | | | | | "
              f"{g(w['median_instructions'])} |")
print()
rss = result["peak_rss_kb"]
print(f"on {result['host']}: peak RSS {f(rss and rss / 1048576, 2)} GB; exit {status}; "
      f"wall {result['wall_s']} s; "
      f"load {load0} -> {load1}; final outputs = cold build's: "
      f"{'yes' if result.get('final_differs_from_cold') == [] else result.get('final_differs_from_cold', 'not compared')}"
      + (f"; PANIC: {panic}" if panic else ""))
print(f"JSON: {out}")
failed = int(status) != 0 or panic or any("ms" not in r or "stopped" in r
                                          for r in result["rebuilds"])
sys.exit(1 if failed else 0)
EOF
