#!/usr/bin/env python3
"""The cold watch build's time by op class (gate-fast's ams shard; the
same command for the head and the control, as bench/ams-cold-tmp.sh runs
amsldoc: `phitex watch --no-view --json -j J amsldoc.tex`, cold, then q).

    gate-time.py run BIN DIR MAIN J OUT_JSON ERR_LOG   one cold watch build
    gate-time.py show A.json [B.json ...]              the side-by-side table

From the build event (wall, nodes run, pages, the notes: live nodes) and
the PHITEX_TEX_TIME lines on stderr: rounds (their count and walls), the
runs by op, node-runs / live nodes, the hint pass (`incr walk`: the
analyses' busy time), the evaluation's wall (the rounds' walls summed),
and the engine's share (evaluation wall minus the op bodies' busy time)
where the build prints the op bodies' busy time (op class lines:
`busy by op`); a build that does not print it shows '-'
(docs/spec-redesign.md section 7, item 10).
"""
import json
import re
import subprocess
import sys
import time

UNIT = {"s": 1.0, "ms": 1e-3, "µs": 1e-6, "us": 1e-6, "ns": 1e-9}


def secs(s):
    m = re.match(r"([0-9.]+)(s|ms|µs|us|ns)$", s.strip())
    return float(m.group(1)) * UNIT[m.group(2)] if m else None


def run(binary, d, main, j, out, err):
    t0 = time.monotonic()
    with open(err, "w") as e:
        p = subprocess.Popen([binary, "watch", "--no-view", "--json", "-j", j, main], cwd=d,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=e)
        ev = None
        for l in p.stdout:
            try:
                x = json.loads(l)
            except ValueError:
                continue
            if x.get("event") == "build":
                ev = x
                break
        try:
            p.stdin.write(b"q\n")
            p.stdin.close()
        except OSError:
            pass
        try:
            p.wait(timeout=120)
        except subprocess.TimeoutExpired:
            p.kill()
    res = {"j": int(j), "process_s": round(time.monotonic() - t0, 3), "event": ev}
    res.update(parse(open(err, encoding="utf-8", errors="replace").read(), ev or {}))
    json.dump(res, open(out, "w"), indent=1)


def parse(t, ev):
    r = {}
    rounds = [m.group(1) for m in re.finditer(r"^round: ([^,]+), (\d+) nodes run, (\d+) runs", t, re.M)]
    runs = [int(m.group(3)) for m in re.finditer(r"^round: ([^,]+), (\d+) nodes run, (\d+) runs", t, re.M)]
    r["rounds"] = len(rounds)
    r["round_walls_s"] = [round(secs(x) or 0, 3) for x in rounds]
    r["eval_s"] = round(sum(r["round_walls_s"]), 3)
    r["runs"] = sum(runs)
    byop = {}
    for m in re.finditer(r"^round: .*by op (.*)$", t, re.M):
        for k, v in re.findall(r'\("?(\w+)"?, (\d+)\)', m.group(1)):
            byop[k] = byop.get(k, 0) + int(v)
    r["runs_by_op"] = byop
    live = None
    notes = " ".join(ev.get("notes") or []) if isinstance(ev.get("notes"), list) else str(ev.get("notes") or "")
    for src in (notes, t):
        m = list(re.finditer(r"(\d+) nodes run of (\d+) live", src))
        if m:
            live = int(m[-1].group(2))
            break
    r["live"] = live
    r["runs_per_live"] = round(r["runs"] / live, 3) if live and r["runs"] else None
    hp = [secs(m.group(1)) for m in re.finditer(r"^incr walk: .*\(busy ([^)]+)\)", t, re.M)]
    r["hint_s"] = round(sum(x for x in hp if x), 3) if hp else None
    # op bodies' busy time by class, when the build prints it
    busy = {}
    for m in re.finditer(r"busy by op:? (.*)$", t, re.M):
        for k, v in re.findall(r'"?(\w+)"?[:,] ?([0-9.]+(?:s|ms|µs|us|ns))', m.group(1)):
            busy[k] = busy.get(k, 0) + (secs(v) or 0)
    r["busy_by_op_s"] = {k: round(v, 3) for k, v in busy.items()} or None
    r["engine_s"] = round(r["eval_s"] - sum(busy.values()), 3) if busy else None
    r["wall_s"] = round((ev.get("wall_ms") or 0) / 1000, 3) if ev.get("wall_ms") is not None else None
    r["pages"] = ev.get("pages")
    r["ok"] = ev.get("ok")
    r["nodes_run"] = ev.get("nodes_run")
    return r


def show(paths):
    rows = [(p, json.load(open(p))) for p in paths]
    print(f"  {'build':<26} {'j':>3} {'wall_s':>7} {'eval_s':>7} {'rounds':>6} {'round walls':<22} {'runs/live':>12} {'hint_s':>7} {'engine_s':>8}  runs by op / busy by op")
    for p, r in rows:
        name = p.rsplit("/", 1)[-1].rsplit(".", 1)[0]
        rl = f"{r['runs']}/{r['live']}={r['runs_per_live']}" if r.get("runs_per_live") else f"{r.get('runs')}/{r.get('live')}"
        print(f"  {name:<26} {r['j']:>3} {str(r.get('wall_s')):>7} {r['eval_s']:>7} {r['rounds']:>6} "
              f"{','.join(str(x) for x in r['round_walls_s'])[:22]:<22} {rl:>12} {str(r.get('hint_s') or '-'):>7} "
              f"{str(r.get('engine_s') or '-'):>8}  {r['runs_by_op']} / {r.get('busy_by_op_s') or '- (not printed)'}"
              + ("" if r.get("ok") else "  NOT OK"))


if __name__ == "__main__":
    if sys.argv[1] == "run":
        run(*sys.argv[2:8])
    else:
        show(sys.argv[2:])
