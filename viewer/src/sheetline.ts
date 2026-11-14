// The loading sheet's line (panel.ts step()): the step under the opening and
// the detail under it, said calmly. Several writers say it (the project read,
// the packages fetched, the host's phases, typesetting), each as often as its
// events come: a pack every 70 ms on a cold open. Said as they came, the line
// jumped between "pdfescape · 25 of 26" and "26 of 26" a dozen times a
// second. Here:
//
//   - the line is drawn at most once every LINE_GAP ms, the latest wins
//     (the first at once, a pending one at the end of the gap);
//   - the step goes forward only (the project, then the packages, then
//     typesetting): a writer saying an earlier step is not said, until the
//     opening begins again (a rebuild, another document);
//   - the packages' detail has one shape from its first word to the last:
//     "12 fetched · 2.8 MB · l3kernel", the count and size never going back,
//     the name the last one asked for, kept between two fetches.

/** The least time between two drawings of the line (ms): four a second at most. */
export const LINE_GAP = 250;

/** What the line says: the step, how far (undefined: not known), the detail. */
export interface Said {
  what: string;
  pct: number | undefined;
  detail: string;
}

/** A clock and timers (performance.now and setTimeout; a fake one in the tests). */
export interface Clock {
  now(): number;
  later(f: () => void, ms: number): unknown;
  cancel(h: unknown): void;
}

export const realClock: Clock = {
  now: () => performance.now(),
  later: (f, ms) => setTimeout(f, ms),
  cancel: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

/**
 * `draw` called at most once every `gap` ms with the latest value set (the
 * first at once; one set within the gap drawn at its end, one set again
 * before then replacing it). A value the same as the one drawn is not drawn
 * again.
 */
export class Calm<T> {
  private drawnAt = -Infinity;
  private drawn: string | undefined;
  private next: T | undefined;
  private timer: unknown;
  private draw: (v: T) => void;
  private clock: Clock;
  private gap: number;
  private key: (v: T) => string;
  constructor(draw: (v: T) => void, clock: Clock = realClock, gap = LINE_GAP, key: (v: T) => string = (v) => JSON.stringify(v)) {
    this.draw = draw;
    this.clock = clock;
    this.gap = gap;
    this.key = key;
  }

  set(v: T): void {
    const k = this.key(v);
    // (back to what is shown before the pending one was drawn: nothing to draw)
    if (k === this.drawn) return void this.drop();
    const wait = this.drawnAt + this.gap - this.clock.now();
    if (wait <= 0) return void (this.drop(), this.put(v, k));
    this.next = v;
    this.timer ??= this.clock.later(() => {
      this.timer = undefined;
      const n = this.next;
      this.next = undefined;
      if (n !== undefined) this.put(n, this.key(n));
    }, wait);
  }

  /** The pending value drawn now (none: nothing). */
  flush(): void {
    const n = this.next;
    this.drop();
    if (n !== undefined) this.put(n, this.key(n));
  }

  /** Nothing pending, and the next value drawn at once (a new opening). */
  reset(): void {
    this.drop();
    this.drawnAt = -Infinity;
    this.drawn = undefined;
  }

  /** A value is waiting for the end of the gap. */
  get pending(): boolean {
    return this.next !== undefined;
  }

  private drop(): void {
    if (this.timer !== undefined) this.clock.cancel(this.timer);
    this.timer = undefined;
    this.next = undefined;
  }

  private put(v: T, k: string): void {
    this.drawnAt = this.clock.now();
    this.drawn = k;
    this.draw(v);
  }
}

/**
 * The sheet's line: forward only, drawn calmly. `say` with the step's rank
 * (0 the project and the engine, 1 the packages or the format, 2 typesetting):
 * a rank below the one said is not said (false).
 */
export class SheetLine {
  private rank = 0;
  private calm: Calm<Said>;
  constructor(draw: (s: Said) => void, clock: Clock = realClock, gap = LINE_GAP) {
    this.calm = new Calm(draw, clock, gap);
  }

  say(rank: number, s: Said): boolean {
    if (rank < this.rank) return false;
    this.rank = rank;
    this.calm.set(s);
    return true;
  }

  /** The step now (its rank): a writer composing its detail asks. */
  get at(): number {
    return this.rank;
  }

  /** A new opening (a rebuild, another document): from the first step again, drawn at once. */
  reset(): void {
    this.rank = 0;
    this.calm.reset();
  }

  flush(): void {
    this.calm.flush();
  }
}

/** A size: "36 KB", "4.2 MB" (none: ""). */
export function size(b?: number): string {
  return b ? (b < 1048576 ? `${Math.max(1, Math.round(b / 1024))} KB` : `${(b / 1048576).toFixed(1)} MB`) : "";
}

/** A package as the writer knows it: a Shelf pack's id without its hash ("pdfescape-e885d9813aea": "pdfescape"), a file's name as it is. */
export function packName(id: string): string {
  return id.replace(/-[0-9a-f]{12}$/, "");
}

/** The packages so far, as the line says them: never fewer, the name kept between two fetches. */
export interface Fetched {
  done: number;
  bytes: number;
  missing: number;
  name: string;
}

/**
 * The packages of one opening (or one burst after the first page), from the
 * host's states: the count and the bytes never go back (a host's state that
 * says fewer, an older one, is not said), the name is the newest one asked
 * for and stays until another is.
 */
export class Tally {
  private f: Fetched = { done: 0, bytes: 0, missing: 0, name: "" };
  private base = { done: 0, bytes: 0 };

  update(p: { done?: number; bytes?: number; loading: readonly string[]; missing?: number }): Fetched {
    const f = this.f;
    f.done = Math.max(f.done, (p.done ?? 0) - this.base.done);
    f.bytes = Math.max(f.bytes, (p.bytes ?? 0) - this.base.bytes);
    f.missing = Math.max(f.missing, p.missing ?? 0);
    // (the newest asked for: a pack fetched in a build's order, one after the other)
    if (p.loading.length) f.name = packName(p.loading[p.loading.length - 1]);
    return { ...f };
  }

  get now(): Fetched {
    return { ...this.f };
  }

  /** Counted from none again; `from`: the host's totals so far (a burst after the first page counts its own). */
  reset(from?: { done?: number; bytes?: number }): void {
    this.f = { done: 0, bytes: 0, missing: 0, name: "" };
    this.base = { done: from?.done ?? 0, bytes: from?.bytes ?? 0 };
  }
}

/** The packages' detail, one shape throughout: "12 fetched · 2.8 MB · 1 not found · l3kernel". */
export function fetchedLine(f: Fetched, name = true): string {
  return [`${f.done} fetched`, size(f.bytes), f.missing ? `${f.missing} not found` : "", name ? f.name : ""].filter(Boolean).join(" · ");
}
