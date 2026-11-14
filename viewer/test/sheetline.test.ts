// The loading sheet's line (viewer/src/sheetline.ts): drawn at most every
// LINE_GAP ms with the latest said, the step forward only, the packages'
// detail one shape with a count that never goes back. A fake clock; the
// states are a cold open's as the Overleaf extension's session said them,
// one pack after the other, 70 ms apart.
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { Calm, LINE_GAP, SheetLine, Tally, fetchedLine, packName, size, type Clock, type Said } from "../src/sheetline.ts";

/** A fake clock: `run(ms)` moves it on, firing the timers due. */
function clock() {
  let now = 0, n = 0;
  const timers = new Map<number, { at: number; f: () => void }>();
  const c: Clock = {
    now: () => now,
    later: (f, ms) => (timers.set(++n, { at: now + ms, f }), n),
    cancel: (h) => void timers.delete(h as number),
  };
  const run = (ms: number) => {
    const end = now + ms;
    for (;;) {
      const due = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      timers.delete(due[0]);
      now = due[1].at;
      due[1].f();
    }
    now = end;
  };
  return { c, run, get now() { return now; } };
}

test("the first is drawn at once, then at most one every LINE_GAP ms, the latest", () => {
  const k = clock();
  const drawn: { t: number; v: number }[] = [];
  const calm = new Calm<number>((v) => drawn.push({ t: k.now, v }), k.c);
  calm.set(1);
  for (let i = 2; i <= 40; i++) (k.run(20), calm.set(i));
  k.run(1000);
  assert.deepEqual(drawn[0], { t: 0, v: 1 });
  for (let i = 1; i < drawn.length; i++) assert.ok(drawn[i].t - drawn[i - 1].t >= LINE_GAP, `${drawn[i].t} after ${drawn[i - 1].t}`);
  // (the last said is the last drawn)
  assert.equal(drawn.at(-1)!.v, 40);
  assert.ok(drawn.length <= 1 + Math.ceil((39 * 20) / LINE_GAP) + 1);
});

test("the same value is not drawn again; back to what is shown, the pending one is dropped", () => {
  const k = clock();
  const drawn: string[] = [];
  const calm = new Calm<string>((v) => drawn.push(v), k.c);
  calm.set("a");
  k.run(100);
  calm.set("a");
  // (within the gap: "b" waits, then "a" again, as shown)
  calm.set("b");
  calm.set("a");
  k.run(1000);
  assert.deepEqual(drawn, ["a"]);
});

test("reset: the next drawn at once, nothing pending", () => {
  const k = clock();
  const drawn: string[] = [];
  const calm = new Calm<string>((v) => drawn.push(v), k.c);
  calm.set("a");
  calm.set("b");
  assert.ok(calm.pending);
  calm.reset();
  assert.ok(!calm.pending);
  calm.set("c");
  k.run(1000);
  assert.deepEqual(drawn, ["a", "c"]);
});

test("the step goes forward only, until the opening begins again", () => {
  const k = clock();
  const drawn: Said[] = [];
  const line = new SheetLine((s) => drawn.push(s), k.c);
  const say = (rank: number, what: string, detail = "") => (k.run(300), line.say(rank, { what, pct: undefined, detail }));
  assert.ok(say(0, "Reading the project"));
  assert.ok(say(1, "Fetching packages", "3 fetched"));
  assert.ok(say(2, "Typesetting", "Page 1"));
  // (a late reading or fetching state: not said)
  assert.ok(!say(0, "Project read · checking packages…"));
  assert.ok(!say(1, "Fetching packages", "4 fetched"));
  k.run(1000);
  assert.deepEqual(drawn.map((s) => s.what), ["Reading the project", "Fetching packages", "Typesetting"]);
  line.reset();
  assert.ok(say(0, "Reading the project"));
  assert.equal(drawn.at(-1)!.what, "Reading the project");
});

test("a cold open's packs, one after the other: one shape, the count never back, four a second at most", () => {
  const k = clock();
  const drawn: { t: number; s: Said }[] = [];
  const line = new SheetLine((s) => drawn.push({ t: k.now, s }), k.c);
  const tally = new Tally();
  const ids = Array.from({ length: 40 }, (_, i) => `pack${i}-${(0x100000000000 + i).toString(16)}`);
  const done: string[] = [];
  let bytes = 0;
  // (as the session says them: the pack loading, then none (the gap), then the next)
  const tell = (loading: string[]) => {
    const f = tally.update({ done: done.length, bytes, loading, missing: 0 });
    line.say(1, { what: "Fetching packages", pct: undefined, detail: fetchedLine(f) });
  };
  for (const id of ids) {
    tell([id]);
    k.run(60);
    done.push(id);
    bytes += 4096;
    tell([]);
    k.run(10);
  }
  k.run(1000);
  const shape = (d: string) => d.replace(/\d+(\.\d+)?/g, "#").replace(/pack#/, "NAME");
  const shapes = new Set(drawn.slice(1).map((d) => shape(d.s.detail)));
  assert.deepEqual([...shapes], ["# fetched · # KB · NAME"]);
  let last = -1;
  for (const d of drawn) {
    const n = +/^(\d+) fetched/.exec(d.s.detail)![1];
    assert.ok(n >= last, `${n} after ${last}`);
    last = n;
  }
  assert.equal(last, 40);
  for (let i = 1; i < drawn.length; i++) assert.ok(drawn[i].t - drawn[i - 1].t >= LINE_GAP);
  // (2.8 s of packs: a dozen drawings, not 80)
  assert.ok(drawn.length <= 14, `${drawn.length} drawn`);
  // (the name never dropped between two fetches)
  assert.ok(drawn.every((d) => / · pack\d+$/.test(d.s.detail)));
});

test("Tally: never fewer, the name kept, a burst counted from its start", () => {
  const t = new Tally();
  assert.deepEqual(t.update({ done: 3, bytes: 100, loading: ["a.sty"] }), { done: 3, bytes: 100, missing: 0, name: "a.sty" });
  // (an older state after a newer one: not fewer)
  assert.deepEqual(t.update({ done: 2, bytes: 50, loading: [] }), { done: 3, bytes: 100, missing: 0, name: "a.sty" });
  t.reset({ done: 10, bytes: 1000 });
  assert.deepEqual(t.update({ done: 11, bytes: 1500, loading: ["tikz-cd-0123456789ab"] }), { done: 1, bytes: 500, missing: 0, name: "tikz-cd" });
});

test("the words: sizes, pack names, the line", () => {
  assert.equal(size(0), "");
  assert.equal(size(300), "1 KB");
  assert.equal(size(4.2 * 1048576), "4.2 MB");
  assert.equal(packName("pdfescape-e885d9813aea"), "pdfescape");
  assert.equal(packName("pgf.graphdrawing-lua-pgf-gd-lib-084bd2cedd36"), "pgf.graphdrawing-lua-pgf-gd-lib");
  assert.equal(packName("tikz.sty"), "tikz.sty");
  assert.equal(fetchedLine({ done: 12, bytes: 2.8 * 1048576, missing: 1, name: "l3kernel" }), "12 fetched · 2.8 MB · 1 not found · l3kernel");
  assert.equal(fetchedLine({ done: 12, bytes: 0, missing: 0, name: "l3kernel" }, false), "12 fetched");
});
