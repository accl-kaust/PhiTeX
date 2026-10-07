// Keys for the pages, as PDF viewers have them and vim's (zathura's)
// motions: a host turns them on (the CLI's page does; the extension may
// not, where the editor owns the keys). Counts as in vim: `3j`, `12G`.

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
}

/** The keys, for a help overlay: [keys, what they do]. */
export const KEYS: [string, string][] = [
  ["j k  ↓ ↑", "scroll down, up"],
  ["h l", "scroll left, right"],
  ["Ctrl+d  Ctrl+u", "half a screen down, up"],
  ["Ctrl+f  Ctrl+b", "a screen down, up"],
  ["Space  Shift+Space", "next, previous page"],
  ["→ ←  PgDn PgUp", "next, previous page"],
  ["gg  Home", "first page"],
  ["G  End", "last page"],
  ["12G  12gg", "page 12"],
  ["K J  + −", "zoom in, out"],
  ["0  w", "fit the page's width"],
  ["f", "fit the whole page"],
  ["?", "these keys"],
  ["Esc", "close; forget a count"],
];

/** A line's scroll, in CSS pixels. */
const LINE = 60;

/** Listen for the keys on `on` (the page's window or the viewer's root). */
export function bindKeys(on: Window | HTMLElement, t: KeyTarget): () => void {
  let count = "";
  let g = false;
  let helping = false;
  const times = () => Math.max(1, Number(count) || 1);
  const page = (delta: number) => t.turn(delta);
  const by = (dx: number, dy: number) => t.scroller.scrollBy({ left: dx, top: dy, behavior: "auto" });
  const handler = (ev: Event) => {
    const e = ev as KeyboardEvent;
    // (typing in a field, or a key with Alt or Meta: not ours)
    const el = e.target as HTMLElement | null;
    if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
    if (e.altKey || e.metaKey) return;
    const k = e.key;
    if (e.ctrlKey) {
      const h = t.scroller.clientHeight;
      const n = times();
      const act: Record<string, () => void> = {
        d: () => by(0, (n * h) / 2),
        u: () => by(0, (-n * h) / 2),
        f: () => by(0, n * h),
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
    const goToCount = (last: boolean) => t.goTo(count ? Math.min(t.pages, Number(count)) - 1 : last ? t.pages - 1 : 0);
    let done = true;
    if (g) {
      g = false;
      if (k === "g") goToCount(false);
      else done = false;
    } else
      switch (k) {
        case "j":
        case "ArrowDown":
          by(0, n * LINE);
          break;
        case "k":
        case "ArrowUp":
          by(0, -n * LINE);
          break;
        case "h":
          by(-n * LINE, 0);
          break;
        case "l":
          by(n * LINE, 0);
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
          t.goTo(0);
          break;
        case "End":
          t.goTo(t.pages - 1);
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
          t.zoom(1.1 ** n);
          break;
        case "J":
        case "-":
          t.zoom(1.1 ** -n);
          break;
        case "0":
        case "w":
          t.fitWidth();
          break;
        case "f":
          t.fitPage();
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
  return () => on.removeEventListener("keydown", handler);
}
