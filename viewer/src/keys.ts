// Keys for the pages, as PDF viewers have them and vim's (zathura's)
// motions: a host turns them on (the CLI's page does; the extension may
// not, where the editor owns the keys). Counts as in vim: `3j`, `12G`.

import { Glide, LINE, ZOOM_STEP } from "./glide.ts";

/** What the keys move: the viewer's pages, its scroller, the host's zoom. */
export interface KeyTarget {
  /** How many pages, and the one most in view (from 0). */
  readonly pages: number;
  readonly page: number;
  /** Scroll page `k` into view. */
  goTo(k: number): void;
  /** Turn `n` pages from the one at the view's top (back if negative). */
  turn(n: number): void;
  /** The scrolling element. */
  scroller: HTMLElement;
  /** Zoom by `factor`; fit the page's width; fit the whole page. */
  zoom(factor: number): void;
  fitWidth(): void;
  fitPage(): void;
  /** Show or hide the keys' help. */
  help?(show: boolean): void;
  /**
   * Zoom by `factor` as Ctrl+wheel does (the panel's eased gesture, the
   * pages drawn again once it rests), `ease` false at the next frame. A held
   * key calls it each frame. Without it, `zoom` once a key press.
   */
  zoomBy?(factor: number, ease: boolean): void;
  /**
   * Scroll by whole pixels at once, returning what it moved (less at an
   * end): the eased motion's frames. Without it, the scroller's own.
   */
  scrollBy?(dx: number, dy: number): [number, number];
  /** Where `turn(n)` would scroll to from scroll top `from`, without scrolling (a page key's eased move). */
  turnTop?(n: number, from: number): number;
}

/** The keys, for a help overlay: [keys, what they do]. */
export const KEYS: [string, string][] = [
  ["j k  ↓ ↑", "scroll down, up"],
  ["h l", "scroll left, right"],
  ["Ctrl+d  Ctrl+u", "half a screen down, up"],
  ["Ctrl+b", "a screen up"],
  ["Ctrl+f  /", "find in the pages"],
  ["Enter  n / Shift+Enter  N", "next, previous match"],
  ["Space  Shift+Space", "next, previous page"],
  ["→ ←  PgDn PgUp", "next, previous page"],
  ["gg  Home", "first page"],
  ["G  End", "last page"],
  ["12G  12gg", "page 12"],
  ["K J  + −", "zoom in, out"],
  ["Ctrl+wheel  pinch", "zoom at the pointer"],
  ["Ctrl+ +  Ctrl+ −  Ctrl+0", "zoom in, out, fit"],
  ["0  w", "fit the page's width"],
  ["f", "fit the whole page"],
  ["?", "these keys"],
  ["Esc", "close; forget a count"],
];

/**
 * A key typed into a field (an input, a text area, an editor): the field's,
 * no shortcut. The field itself, not `target`: from a shadow root (the
 * panel's search box) the target out here is the root's host element.
 */
export function typing(e: Event): boolean {
  const el = (e.composedPath?.()[0] ?? e.target) as HTMLElement | null;
  return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName ?? ""));
}

/** Whether moves are to be instant (the system's reduced motion). */
const reduced = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/**
 * Listen for the keys on `on` (the page's window or the viewer's root).
 * Scroll and zoom keys move as the wheel does (glide.ts): a held key glides
 * from its press to its release, one frame loop, its auto-repeats ignored; a
 * tap moves a line, eased; a page's key moves on from where the last one was
 * going. Reduced motion: each press (and repeat) moves at once, as before.
 */
export function bindKeys(on: Window | HTMLElement, t: KeyTarget): () => void {
  let count = "";
  let g = false;
  let helping = false;
  const times = () => Math.max(1, Number(count) || 1);
  const scroll = (dx: number, dy: number): [number, number] => {
    if (t.scrollBy) return t.scrollBy(dx, dy);
    const s = t.scroller, x = s.scrollLeft, y = s.scrollTop;
    s.scrollBy({ left: dx, top: dy, behavior: "auto" });
    // (what it moved: all of it, unless the scroller stopped at an end)
    const end = (d: number, was: number, now: number) => (d && Math.round(now - was) !== d ? Math.round(now - was) : d);
    return [end(dx, x, s.scrollLeft), end(dy, y, s.scrollTop)];
  };
  const glide = new Glide({ scroll, zoom: (f) => t.zoomBy?.(f, true) });
  /** A distance at once (reduced motion), or eased on from where the motion is going. */
  const by = (dx: number, dy: number) => (reduced() ? (glide.stop(), void scroll(dx, dy)) : glide.by(dx, dy));
  /** Page keys: from where the motion is going, the page `n` on. */
  const page = (n: number) => {
    if (reduced() || !t.turnTop) return glide.stop(), t.turn(n);
    const from = t.scroller.scrollTop + glide.ahead()[1];
    glide.by(0, t.turnTop(n, from) - from);
  };
  /** A jump (a page by number, the ends, a fit): no motion left to land after it. */
  const jump = (f: () => void) => (glide.stop(), f());
  /** A scroll key pressed: held, it glides (axis 0 across, 1 down) until its keyup. */
  const line = (e: KeyboardEvent, axis: 0 | 1, dir: number, n: number) => {
    if (reduced()) return void scroll(axis ? 0 : dir * n * LINE, axis ? dir * n * LINE : 0);
    // (a repeat of a key held since before the keys were ours: its press is this one)
    if (!e.repeat || !glide.holding(e.code || e.key)) glide.press(e.code || e.key, axis, dir, n);
  };
  /** A zoom key: a step (the wheel's notch) on its press; held, on at ZOOM_RATE until its keyup. */
  const zoom = (e: KeyboardEvent, dir: number, n: number) => {
    if (!t.zoomBy) return t.zoom(ZOOM_STEP ** (dir * n));
    const instant = reduced();
    if (e.repeat && !instant && glide.holding(e.code || e.key)) return;
    t.zoomBy(ZOOM_STEP ** (dir * n), !instant);
    if (!instant) glide.press(e.code || e.key, 2, dir, n);
  };
  const up = (ev: Event) => {
    const e = ev as KeyboardEvent;
    glide.release(e.code || e.key);
    // (Shift let go before J: its keyup says "j" and the same code; with no code, either case)
    if (!e.code) glide.release(e.key === e.key.toLowerCase() ? e.key.toUpperCase() : e.key.toLowerCase());
  };
  const lost = () => glide.releaseAll();
  const handler = (ev: Event) => {
    const e = ev as KeyboardEvent;
    // (typing in a field, or a key with Alt or Meta: not ours)
    if (typing(e)) return;
    if (e.altKey || e.metaKey) return;
    const k = e.key;
    if (e.ctrlKey) {
      const h = t.scroller.clientHeight;
      const n = times();
      const act: Record<string, () => void> = {
        d: () => by(0, (n * h) / 2),
        u: () => by(0, (-n * h) / 2),
        b: () => by(0, -n * h),
      };
      if (!act[k]) return;
      e.preventDefault();
      act[k]();
      count = "";
      return;
    }
    if (/^[0-9]$/.test(k) && (count || k !== "0")) {
      count += k;
      e.preventDefault();
      return;
    }
    const n = times();
    const goToCount = (last: boolean) => jump(() => t.goTo(count ? Math.min(t.pages, Number(count)) - 1 : last ? t.pages - 1 : 0));
    let done = true;
    if (g) {
      g = false;
      if (k === "g") goToCount(false);
      else done = false;
    } else
      switch (k) {
        case "j":
        case "ArrowDown":
          line(e, 1, 1, n);
          break;
        case "k":
        case "ArrowUp":
          line(e, 1, -1, n);
          break;
        case "h":
          line(e, 0, -1, n);
          break;
        case "l":
          line(e, 0, 1, n);
          break;
        case " ":
          page(e.shiftKey ? -n : n);
          break;
        case "ArrowRight":
        case "PageDown":
          page(n);
          break;
        case "ArrowLeft":
        case "PageUp":
          page(-n);
          break;
        case "Home":
          jump(() => t.goTo(0));
          break;
        case "End":
          jump(() => t.goTo(t.pages - 1));
          break;
        case "g":
          g = true;
          e.preventDefault();
          return;
        case "G":
          goToCount(true);
          break;
        case "K":
        case "+":
        case "=":
          zoom(e, 1, n);
          break;
        case "J":
        case "-":
          zoom(e, -1, n);
          break;
        case "0":
        case "w":
          jump(() => t.fitWidth());
          break;
        case "f":
          jump(() => t.fitPage());
          break;
        case "?":
          helping = !helping;
          t.help?.(helping);
          break;
        case "Escape":
          if (helping) {
            helping = false;
            t.help?.(false);
          }
          break;
        default:
          done = false;
      }
    count = "";
    if (done) e.preventDefault();
  };
  on.addEventListener("keydown", handler);
  on.addEventListener("keyup", up);
  // (the keys gone elsewhere: no keyup will come for those held)
  const win = typeof window === "undefined" ? null : window;
  win?.addEventListener("blur", lost);
  return () => {
    on.removeEventListener("keydown", handler);
    on.removeEventListener("keyup", up);
    win?.removeEventListener("blur", lost);
    glide.stop();
  };
}
