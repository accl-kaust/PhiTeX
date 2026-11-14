// The keys (viewer/src/keys.ts) on a fake window and pages: a held scroll
// key glides, its auto-repeats ignored (the same distance with or without
// them); a zoom key steps once on its press and feeds the zoom gesture each
// frame while held; counts, gg and G still jump; reduced motion moves at once.
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { bindKeys, type KeyTarget } from "../src/keys.ts";
import { LINE } from "../src/glide.ts";

// (frames at 60 Hz from timers; reduced motion as each test sets it)
let reduce = false;
const g = globalThis as Record<string, unknown>;
g.requestAnimationFrame = (cb: (t: number) => void) => setTimeout(() => cb(performance.now()), 16) as unknown as number;
g.cancelAnimationFrame = (id: number) => clearTimeout(id);
g.matchMedia = (q: string) => ({ matches: reduce && q.includes("reduce") });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function rig() {
  const on = new EventTarget();
  const s = { x: 0, y: 5000, calls: 0 };
  const zooms: { f: number; ease: boolean }[] = [];
  const went: number[] = [];
  const t: KeyTarget = {
    pages: 20,
    page: 3,
    goTo: (k) => went.push(k),
    turn: () => {},
    scroller: { clientHeight: 800, scrollTop: 5000 } as unknown as HTMLElement,
    zoom: () => assert.fail("the gesture's, not zoom's"),
    fitWidth: () => {},
    fitPage: () => {},
    zoomBy: (f, ease) => zooms.push({ f, ease }),
    scrollBy: (dx, dy) => ((s.x += dx), (s.y += dy), s.calls++, [dx, dy]),
    turnTop: (n, from) => from + n * 1000,
  };
  const off = bindKeys(on as unknown as Window, t);
  const key = (type: string, key: string, code: string, repeat = false) => {
    const e = Object.assign(new Event(type, { cancelable: true }), { key, code, repeat, ctrlKey: false, altKey: false, metaKey: false, shiftKey: key !== key.toLowerCase() });
    on.dispatchEvent(e);
    return e;
  };
  return { s, zooms, went, key, off };
}

test("a held j glides the same distance whether its auto-repeats come or not", async () => {
  reduce = false;
  const a = rig(), b = rig();
  a.key("keydown", "j", "KeyJ");
  b.key("keydown", "j", "KeyJ");
  for (let i = 0; i < 12; i++) {
    await sleep(33);
    // (b gets the auto-repeats: ignored, their default still prevented)
    assert.equal(b.key("keydown", "j", "KeyJ", true).defaultPrevented, true);
  }
  a.key("keyup", "j", "KeyJ");
  b.key("keyup", "j", "KeyJ");
  await sleep(600);
  const da = a.s.y - 5000, db = b.s.y - 5000;
  assert.ok(da > 2 * LINE, `went on past a line: ${da}`);
  assert.ok(Math.abs(da - db) <= 30, `${da} vs ${db}`);
  assert.ok(a.s.calls > 10, "moved in frames, not in a jump");
  a.off(), b.off();
});

test("a tap of k moves a line up, eased over frames", async () => {
  reduce = false;
  const r = rig();
  r.key("keydown", "k", "KeyK");
  r.key("keyup", "k", "KeyK");
  await sleep(500);
  assert.equal(r.s.y, 5000 - LINE);
  assert.ok(r.s.calls > 3);
  r.off();
});

test("Shift+J held: one step on the press, its repeats ignored, the gesture fed each frame until the keyup; Shift let go first", async () => {
  reduce = false;
  const r = rig();
  r.key("keydown", "J", "KeyJ");
  assert.deepEqual(r.zooms[0], { f: 1 / 1.1, ease: true });
  for (let i = 0; i < 10; i++) {
    await sleep(33);
    r.key("keydown", "J", "KeyJ", true);
  }
  const held = r.zooms.length;
  assert.ok(held > 5, `fed in frames: ${held}`);
  assert.ok(r.zooms.slice(1).every((z) => z.f < 1 && z.ease));
  // (Shift up first: the keyup says "j", the same key)
  r.key("keyup", "j", "KeyJ");
  await sleep(100);
  const n = r.zooms.length;
  await sleep(200);
  assert.equal(r.zooms.length, n, "none after the keyup");
  r.off();
});

test("counts, gg and G jump as before, and stop a glide under way", async () => {
  reduce = false;
  const r = rig();
  r.key("keydown", "j", "KeyJ");
  await sleep(100);
  r.key("keydown", "G", "KeyG");
  assert.deepEqual(r.went, [19]);
  const at = r.s.y;
  await sleep(300);
  r.key("keyup", "j", "KeyJ");
  assert.equal(r.s.y, at, "the glide stopped at the jump");
  r.key("keydown", "1", "Digit1");
  r.key("keydown", "2", "Digit2");
  r.key("keydown", "g", "KeyG");
  r.key("keydown", "g", "KeyG");
  assert.deepEqual(r.went, [19, 11]);
  r.key("keydown", "g", "KeyG");
  r.key("keydown", "g", "KeyG");
  assert.deepEqual(r.went, [19, 11, 0]);
  // 3j: three lines
  r.key("keydown", "3", "Digit3");
  r.key("keydown", "j", "KeyJ");
  r.key("keyup", "j", "KeyJ");
  await sleep(500);
  assert.equal(r.s.y, at + 3 * LINE);
  r.off();
});

test("page keys: eased, a press again on the way turns on from the page it was going to", async () => {
  reduce = false;
  const r = rig();
  r.key("keydown", "PageDown", "PageDown");
  await sleep(40);
  r.key("keydown", "PageDown", "PageDown");
  await sleep(600);
  assert.equal(r.s.y, 5000 + 2000);
  r.off();
});

test("reduced motion: each press and repeat moves at once, as before", async () => {
  reduce = true;
  const r = rig();
  r.key("keydown", "j", "KeyJ");
  r.key("keydown", "j", "KeyJ", true);
  assert.equal(r.s.y, 5000 + 2 * LINE);
  r.key("keyup", "j", "KeyJ");
  r.key("keydown", "K", "KeyK");
  r.key("keydown", "K", "KeyK", true);
  assert.deepEqual(r.zooms, [{ f: 1.1, ease: false }, { f: 1.1, ease: false }]);
  r.key("keyup", "K", "KeyK");
  await sleep(100);
  assert.equal(r.s.y, 5000 + 2 * LINE);
  assert.equal(r.zooms.length, 2);
  r.off();
  reduce = false;
});
