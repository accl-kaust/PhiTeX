// The keys' motion (viewer/src/glide.ts): a held scroll key glides at a
// steady speed, eased in on the press and out on the release, whatever its
// auto-repeat does; a tap moves a line, eased; a page's move extends the
// target; an end stops the motion with nothing left to come; a held zoom
// key zooms on as long as it is held. Frames from a fake clock.
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { Glide, HELD_FROM, HELD_RAMP, LINE, OMEGA, SPEED, ZOOM_RATE, heldRamp, heldReach, springStep, zoomAnchor } from "../src/glide.ts";

/** A glide on a fake scroller (0..max px) and clock: frames run by `run(ms)`. */
function rig(max = 1e9, start = 0) {
  let now = 0;
  let pending: ((t: number) => void) | null = null;
  const log: { t: number; y: number; x: number }[] = [];
  const zooms: { t: number; f: number }[] = [];
  const s = { x: 0, y: start };
  const glide = new Glide({
    scroll(dx, dy) {
      const clamp = (v: number) => Math.max(0, Math.min(max, v));
      const mx = clamp(s.x + dx) - s.x, my = clamp(s.y + dy) - s.y;
      s.x += mx;
      s.y += my;
      return [mx, my];
    },
    zoom: (f) => zooms.push({ t: now, f }),
    frame: (cb) => ((pending = cb), 1),
    cancel: () => (pending = null),
    now: () => now,
  });
  /** `ms` of 60 Hz frames (16.67 ms each). */
  const run = (ms: number, frame = 1000 / 60) => {
    const end = now + ms;
    while (now + frame <= end + 1e-9) {
      now += frame;
      const cb = pending;
      pending = null;
      cb?.(now);
      log.push({ t: now, y: s.y, x: s.x });
    }
    now = end;
  };
  return { glide, run, log, zooms, s, at: (t: number) => (now = t), get now() { return now; }, get moving() { return pending !== null; } };
}

test("the spring's closed form: the same path whatever the frame time, its target reached, never passed", () => {
  // a 600 px move: in one 300 ms step and in 300 one-ms steps
  const one = springStep({ x: 0, v: 0 }, 600, 0, 300);
  let a = { x: 0, v: 0 };
  let most = 0;
  for (let i = 0; i < 300; i++) (a = springStep(a, 600, 0, 1)), (most = Math.max(most, a.x));
  assert.ok(Math.abs(one.x - a.x) < 1e-6, `${one.x} vs ${a.x}`);
  assert.ok(most <= 600 + 1e-9, "no overshoot");
  // (following a target at speed u it rests 2u/ω behind, moving at u)
  let b = { x: 0, v: 0 };
  for (let i = 0; i < 100; i++) b = springStep(b, i * 10, 1, 10);
  assert.ok(Math.abs(b.v - 1) < 1e-6);
  assert.ok(Math.abs(1000 - b.x - 2 / OMEGA) < 1e-3);
  // a bad frame time: no move
  assert.deepEqual(springStep({ x: 5, v: 1 }, 0, 0, NaN), { x: 5, v: 1 });
});

test("a held key's target: a line at once, a tap's time later easing up to SPEED, no pause", () => {
  assert.equal(heldReach(0, 1), LINE);
  // (a tap of 60 to 100 ms: about a line, at most a half more)
  assert.ok(heldReach(60, 1) - LINE < 10);
  assert.ok(heldReach(100, 1) - LINE < LINE / 2);
  assert.equal(heldReach(HELD_FROM + HELD_RAMP + 1000, 1), LINE + SPEED * (HELD_RAMP / 2 + 1000));
  assert.ok(Math.abs(heldReach(10, 3) - 3 * LINE) < 0.1);
  // (its speed, the ramp's slope: rising, never above 1)
  let was = 0;
  for (let t = 0; t < 400; t += 5) {
    const v = heldRamp(t + 1) - heldRamp(t);
    assert.ok(v >= was - 1e-9 && v <= 1 + 1e-9, `${t}: ${v}`);
    was = v;
  }
});

test("a tap moves one line, eased: down only, slow at both ends, all of it", () => {
  const r = rig();
  r.glide.press("KeyJ", 1, 1);
  r.run(16);
  r.glide.release("KeyJ");
  r.run(600);
  assert.equal(r.s.y, LINE);
  const steps = r.log.map((p, i) => p.y - (i ? r.log[i - 1].y : 0));
  assert.ok(steps.every((d) => d >= 0), "monotonic");
  // (eased in: the first frame's step smaller than the largest; eased out: the last ones a pixel or less)
  const most = Math.max(...steps);
  assert.ok(steps[0] < most, `first ${steps[0]} of ${most}`);
  assert.ok(most < LINE / 2, `no jump: ${most}`);
  assert.ok(steps.filter((d) => d > 0).slice(-2).every((d) => d <= 1));
  assert.equal(r.moving, false, "at rest, no more frames");
  // a count: 3 lines
  r.glide.press("KeyJ", 1, 1, 3);
  r.glide.release("KeyJ");
  r.run(600);
  assert.equal(r.s.y, 4 * LINE);
});

test("a held key glides at SPEED: eased in, never faster, steady, no steps at a key-repeat's rate", () => {
  const r = rig();
  r.glide.press("KeyJ", 1, 1);
  r.run(2000);
  const v = r.log.map((p, i) => (i ? (p.y - r.log[i - 1].y) / (p.t - r.log[i - 1].t) : 0));
  // (rising from the press, never above SPEED, never falling)
  for (let i = 2; i < v.length; i++) assert.ok(v[i] >= v[i - 1] - 0.07, `frame ${i}: ${v[i]} after ${v[i - 1]}`);
  assert.ok(Math.max(...v) <= SPEED + 0.07, `${Math.max(...v)}`);
  // after 500 ms: a frame's step within a pixel of SPEED's (whole pixels), no frame without a move
  const late = r.log.filter((p) => p.t > 500);
  for (let i = 1; i < late.length; i++) {
    const d = late[i].y - late[i - 1].y;
    assert.ok(Math.abs(d - SPEED * (1000 / 60)) <= 1, `step ${d}`);
  }
  // let go: eased out, a lag's distance on (2 SPEED/ω), then at rest
  const at = r.s.y;
  r.glide.release("KeyJ");
  r.run(800);
  assert.ok(Math.abs(r.s.y - at - (2 * SPEED) / OMEGA) <= 2 + SPEED * 17, `coasted ${r.s.y - at}`);
  assert.equal(r.moving, false);
});

test("two keys held at once cancel; one let go, the other goes on", () => {
  const r = rig(1e9, 5000);
  r.glide.press("KeyJ", 1, 1);
  r.glide.press("KeyK", 1, -1);
  r.run(500);
  assert.ok(Math.abs(r.s.y - 5000) <= 1);
  r.glide.release("KeyK");
  r.run(500);
  assert.ok(r.s.y > 5000 + 300);
});

test("a page's move extends the target: pressed again on the way, it goes on from where it was going", () => {
  const r = rig();
  r.glide.by(0, 800);
  r.run(50);
  const mid = r.s.y;
  assert.ok(mid > 0 && mid < 800);
  r.glide.by(0, 800);
  assert.deepEqual(r.glide.ahead(), [0, 1600 - mid]);
  r.run(1000);
  assert.equal(r.s.y, 1600);
  const steps = r.log.map((p, i) => p.y - (i ? r.log[i - 1].y : 0));
  assert.ok(steps.every((d) => d >= 0), "never back");
});

test("at an end the motion stops there: nothing left to come after it", () => {
  const r = rig(1000, 900);
  r.glide.press("KeyJ", 1, 1);
  r.run(1000);
  assert.equal(r.s.y, 1000);
  r.glide.release("KeyJ");
  r.run(500);
  // back up: the full line, not less what piled up past the end
  r.glide.press("KeyK", 1, -1);
  r.glide.release("KeyK");
  r.run(600);
  assert.equal(r.s.y, 1000 - LINE);
});

test("a scroll made elsewhere (the wheel) is kept: the motion moves by its own, not to a place", () => {
  const r = rig();
  r.glide.by(0, 300);
  r.run(50);
  r.s.y += 1000;
  r.run(1000);
  assert.equal(r.s.y, 1300);
});

test("stop: the motion dropped where it is (a jump)", () => {
  const r = rig();
  r.glide.press("KeyJ", 1, 1);
  r.run(500);
  r.glide.stop();
  const at = r.s.y;
  r.run(500);
  assert.equal(r.s.y, at);
  assert.deepEqual(r.glide.ahead(), [0, 0]);
});

test("across: h and l on axis 0", () => {
  const r = rig();
  r.glide.press("KeyL", 0, 1);
  r.glide.release("KeyL");
  r.run(600);
  assert.equal(r.s.x, LINE);
  assert.equal(r.s.y, 0);
});

test("a held zoom key: its tap's step the caller's, then on, eased up to ZOOM_RATE; none after the release", () => {
  const r = rig();
  r.glide.press("KeyK", 2, 1);
  r.run(100);
  const early = r.zooms.reduce((p, z) => p * z.f, 1);
  assert.ok(early < 1.02, `a tap's time: hardly more than the caller's step (${early})`);
  r.run(900);
  const total = r.zooms.reduce((p, z) => p * z.f, 1);
  assert.ok(Math.abs(total - Math.exp(ZOOM_RATE * heldRamp(1000))) < 0.01, `${total}`);
  assert.ok(total > 1.5, "held 1 s: half again at least");
  assert.ok(r.zooms.every((z) => z.f > 1 && z.f < 1.02), "small steps, one a frame");
  r.glide.release("KeyK");
  // (the zoom up to the release, at the next frame; then none)
  r.run(17);
  const n = r.zooms.length;
  r.run(500);
  assert.equal(r.zooms.length, n);
  assert.equal(r.moving, false);
  // a tap: the frames' zoom hardly any
  r.glide.press("KeyJ", 2, -1);
  r.run(50);
  r.glide.release("KeyJ");
  r.run(500);
  assert.ok(r.zooms.slice(n).reduce((p, z) => p * z.f, 1) > 0.998);
});

test("the zoom keys zoom at the pointer over the pages, else the view's centre", () => {
  const r = { left: 100, top: 50, width: 800, height: 600 };
  assert.deepEqual(zoomAnchor(r, { x: 150, y: 70 }), [50, 20]);
  assert.deepEqual(zoomAnchor(r, null), [400, 300]);
  assert.deepEqual(zoomAnchor(r, { x: 950, y: 70 }), [400, 300]);
  assert.deepEqual(zoomAnchor(r, { x: 150, y: 10 }), [400, 300]);
});
