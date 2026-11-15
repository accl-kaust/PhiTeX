// The keys' motion (keys.ts): a scroll that follows a target as the wheel's
// does, eased in and out, one frame loop from a key's press to its release.
//
// Each axis's place chases its target as a critically damped spring (no
// overshoot, no bounce). A tap moves the target a LINE at once; a key held
// moves it on at a steady SPEED after that: a held key glides at that
// speed, eased in on the press and out on the release, whatever the key's
// auto-repeat does (repeats are not ours). The target's own speed eases in
// from the time a tap would be up (heldRamp), so a tap is a line and a key
// held a line and then a steady glide, with no pause between. A page's move (Space, Ctrl+d) adds
// its distance to the target: pressed again before it lands, the target
// moves on from where the last one was going, the motion not restarted.
//
// A held zoom key is the same in log terms (a step of 1.1, then twice a second), handed to
// the host's zoom gesture as a factor a frame (its easing is the gesture's).

/** A line's scroll, in CSS pixels: a tap's. */
export const LINE = 60;
/** A held key's scroll, CSS pixels a millisecond (1,500 a second). */
export const SPEED = 1.5;
/** The spring's rate (per ms): most of a tap's line in 100 ms, all of it in 300 ms. */
export const OMEGA = 1 / 50;
/** A held zoom key's rate: the zoom doubled each second (natural log a ms). */
export const ZOOM_RATE = Math.LN2 / 1000;
/** A tap's zoom step, as the wheel's notch. */
export const ZOOM_STEP = 1.1;

/** One axis of the motion: its place and speed (px, px/ms), and the target it chases. */
export interface Axis {
  x: number;
  v: number;
}

/**
 * The spring `dt` ms on, its target at `t0` then moving at `u` px/ms: exact
 * (the closed form of a critically damped spring after a ramp), so any frame
 * time gives the same path, a long frame no overshoot. Returns the new place
 * and speed.
 */
export function springStep(a: Axis, t0: number, u: number, dt: number, w = OMEGA): Axis {
  if (!(dt > 0)) return { x: a.x, v: a.v };
  // (in the target's frame the spring rests 2u/w behind it; f is the distance from that rest)
  const rest = (-2 * u) / w;
  const f = a.x - t0 - rest;
  const g = a.v - u;
  const E = Math.exp(-w * dt);
  const c = g + w * f;
  return { x: t0 + u * dt + rest + (f + c * dt) * E, v: u + (g - w * c * dt) * E };
}

/**
 * A held key's speed-up: from HELD_FROM ms after its press, full speed HELD_RAMP
 * ms later. Its start slow (smoothstep's) is what keeps a tap a line: begun
 * later, the spring's ease-in from the line's step and the ramp's met in a
 * dip of the speed (a hesitation); begun at once and steeper, a tap was two
 * lines. These have no dip and a tap of 60 to 100 ms 1.1 to 1.5 lines.
 */
export const HELD_FROM = 0;
export const HELD_RAMP = 200;

/**
 * How far a key held `held` ms has gone at speed 1 a ms: its speed eased from
 * 0 at HELD_FROM to 1 over HELD_RAMP (smoothstep, integrated): a key held
 * goes a line, then on, at full speed after 200 ms, with no pause between.
 */
export function heldRamp(held: number): number {
  const x = (held - HELD_FROM) / HELD_RAMP;
  if (!(x > 0)) return 0;
  if (x >= 1) return HELD_RAMP / 2 + (held - HELD_FROM - HELD_RAMP);
  return HELD_RAMP * (x * x * x - (x * x * x * x) / 2);
}

/** How far a held key has moved its target `held` ms after its press: `n` lines at once, then on at `rate` a ms. */
export function heldReach(held: number, n: number, line = LINE, rate = SPEED): number {
  return n * line + rate * heldRamp(held);
}

/** A key held: its axis, its direction, its count, since when. */
interface Hold {
  axis: 0 | 1 | 2;
  dir: number;
  n: number;
  t0: number;
}

/** What the motion moves: its host (the viewer's scroller, the panel's zoom). */
export interface GlideHost {
  /** Scroll by whole pixels; returns what it moved (less at an end). */
  scroll(dx: number, dy: number): [number, number];
  /** The zoom by `f` a frame, at the gesture's place, eased by the gesture. */
  zoom?(f: number): void;
  /** The frame clock (requestAnimationFrame, performance.now); a test's own. */
  frame?(cb: (t: number) => void): number;
  cancel?(id: number): void;
  now?(): number;
}

/**
 * The keys' motion: scroll axes 0 (x) and 1 (y), zoom axis 2 (log of the
 * factor). The target is `base` (taps, pages, finished holds) plus what the
 * keys still held add.
 */
export class Glide {
  private base = [0, 0, 0];
  private holds = new Map<string, Hold>();
  private ax: Axis[] = [{ x: 0, v: 0 }, { x: 0, v: 0 }];
  /** The target at the last frame (steps since: a tap's, a page's, a press's, added at once). */
  private prev = [0, 0, 0];
  /** What was scrolled so far, in whole pixels (the place rounded: the sum of the moves is exact). */
  private done = [0, 0];
  private last = 0;
  private id = 0;
  private host: GlideHost;
  constructor(host: GlideHost) {
    this.host = host;
  }

  private now(): number {
    return this.host.now?.() ?? performance.now();
  }

  private target(i: number, t: number): number {
    let s = this.base[i];
    for (const h of this.holds.values()) if (h.axis === i) s += h.dir * (i === 2 ? heldReach(t - h.t0, h.n, Math.log(ZOOM_STEP), ZOOM_RATE) : heldReach(t - h.t0, h.n));
    return s;
  }

  /** Whether a frame is coming (a motion under way). */
  get moving(): boolean {
    return this.id !== 0;
  }

  /** Where the scroll is still going: its target less its place, per axis (px). */
  ahead(): [number, number] {
    const t = this.now();
    return [this.target(0, t) - this.done[0], this.target(1, t) - this.done[1]];
  }

  /**
   * A key pressed and held: `id` its own (its code), released by `release(id)`.
   * Axis 0, 1: it moves `n` lines at once, then on at SPEED. Axis 2 (zoom):
   * the tap's step is the caller's; held, the zoom goes on from there.
   */
  press(id: string, axis: 0 | 1 | 2, dir: number, n = 1): void {
    this.release(id);
    const t0 = this.now();
    const h: Hold = { axis, dir, n, t0 };
    // (the line, or the zoom's first step, at once: a step in the target, not part of the last frame's ramp)
    const step = axis === 2 ? dir * n * Math.log(ZOOM_STEP) : dir * n * LINE;
    this.holds.set(id, h);
    // (the zoom's first step is the caller's tap: only the ramp from it is the frames')
    this.prev[axis] += step;
    this.run();
  }

  /** The key let go: its target kept where it reached (the spring eases out to it). */
  release(id: string): void {
    const h = this.holds.get(id);
    if (!h) return;
    const t = this.now();
    const before = this.target(h.axis, t);
    this.holds.delete(id);
    this.base[h.axis] += before - this.target(h.axis, t);
  }

  /** Whether the key `id` is held (pressed, its keyup not yet come). */
  holding(id: string): boolean {
    return this.holds.has(id);
  }

  /** Every key let go (the window lost the keys: no keyup will come). */
  releaseAll(): void {
    for (const id of [...this.holds.keys()]) this.release(id);
  }

  /** Scroll by (`dx`, `dy`) eased: the target moved on from where it was going. */
  by(dx: number, dy: number): void {
    this.base[0] += dx;
    this.base[1] += dy;
    this.prev[0] += dx;
    this.prev[1] += dy;
    this.run();
  }

  /** Stopped where it is (a jump: Home, a page by number): nothing still to come. */
  stop(): void {
    this.holds.clear();
    if (this.id && this.host.cancel) this.host.cancel(this.id);
    else if (this.id) cancelAnimationFrame(this.id);
    this.id = 0;
    for (const i of [0, 1] as const) {
      this.base[i] = this.prev[i] = this.ax[i].x = this.done[i];
      this.ax[i].v = 0;
    }
    this.base[2] = this.prev[2] = 0;
  }

  private run(): void {
    if (this.id) return;
    this.last = 0;
    const frame = this.host.frame ?? ((cb: (t: number) => void) => requestAnimationFrame(cb));
    const step = (t: number) => {
      this.id = 0;
      // (the first frame: as if one before it, at 60 Hz)
      const dt = this.last ? Math.min(100, Math.max(0, t - this.last)) : 16;
      this.last = t;
      if (this.tick(t, dt)) this.id = frame(step);
    };
    this.id = frame(step);
  }

  /** One frame at time `t`, `dt` ms after the last: false once at rest. */
  tick(t: number, dt: number): boolean {
    const want: [number, number] = [0, 0];
    let busy = false;
    for (const i of [0, 1] as const) {
      const to = this.target(i, t);
      const u = dt > 0 ? (to - this.prev[i]) / dt : 0;
      const a = springStep(this.ax[i], this.prev[i], u, dt);
      this.prev[i] = to;
      // (at rest: within a pixel's tenth, slower than 3 px a second, no key held)
      const held = [...this.holds.values()].some((h) => h.axis === i);
      if (!held && Math.abs(to - a.x) < 0.1 && Math.abs(a.v) < 0.003) (a.x = to), (a.v = 0);
      else busy = true;
      this.ax[i] = a;
      want[i] = Math.round(a.x) - this.done[i];
    }
    if (want[0] || want[1]) {
      const got = this.host.scroll(want[0], want[1]);
      for (const i of [0, 1] as const) {
        this.done[i] += got[i];
        // (at an end: the motion's still to come dropped there, no ghost of it after)
        if (got[i] !== want[i]) {
          this.ax[i] = { x: this.done[i], v: 0 };
          // (a key still held at the end: its target pinned there too, from now)
          for (const h of this.holds.values()) if (h.axis === i) (h.t0 = t), (h.n = 0);
          this.base[i] += this.done[i] - this.target(i, t);
          this.prev[i] = this.done[i];
        }
      }
    }
    // (the zoom: the target's change since the last frame, as a factor)
    const z = this.target(2, t);
    if (z !== this.prev[2]) this.host.zoom?.(Math.exp(z - this.prev[2]));
    this.prev[2] = z;
    if ([...this.holds.values()].some((h) => h.axis === 2)) busy = true;
    return busy;
  }
}

/**
 * Where a zoom key zooms, in the stage's own pixels: at the pointer when it is
 * over the stage (`r`, client pixels), else the stage's centre.
 */
export function zoomAnchor(r: { left: number; top: number; width: number; height: number }, p: { x: number; y: number } | null): [number, number] {
  const over = !!p && p.x >= r.left && p.x < r.left + r.width && p.y >= r.top && p.y < r.top + r.height;
  return over ? [p.x - r.left, p.y - r.top] : [r.width / 2, r.height / 2];
}
