// A page's text as the viewer keeps it, line by line, and finding in it:
// the lines the renderer gives (the watch's and the core's strips,
// crates/output/render), TeX's accents made one character with their
// letters, and a
// search across the pages' lines that ignores accents and case.

/** A line of a page's text: its start on the baseline, its width, its font size (PDF points, from the page's top left). */
export interface TextLine {
  x: number;
  y: number;
  w: number;
  size: number;
  text: string;
  /** Each character's left edge (none: the line stretched to `w`). */
  xs?: number[];
  /** Each character's right edge. */
  rs?: number[];
}

/**
 * The line each text element of a page's text layer shows (strips.ts's
 * textLayer): the viewer draws a selection from its characters' edges,
 * one block a line, not the browser's box a glyph.
 */
export const LAYER_LINES = new WeakMap<Element, TextLine>();

/**
 * Text layers made but not yet filled (strips.ts's textLayer): an empty SVG
 * (nothing to lay out) until the view rests or something reads its lines
 * (`ensureText`). Filled as each page arrived, a page's thousands of SVG
 * text elements built and laid out in the frame it came into view were the
 * long frames of a scroll and a zoom, by the wheel as by the keys.
 */
const PENDING = new WeakMap<Element, () => void>();

/** `svg`'s lines, built by `fill` when first needed. */
export function deferText(svg: Element, fill: () => void): void {
  PENDING.set(svg, fill);
  svg.setAttribute("data-pending", "");
}

/** Layers still to fill, or held out of the layout (`holdText`). */
const WAITING = ".phx-text[data-pending], .phx-text[data-hold]";

/** Whether `root` (a page, a slot, the pages) has text layers still to fill or show. */
export function textPending(root: ParentNode): boolean {
  return !!root.querySelector(WAITING) || isWaiting(root);
}

const isWaiting = (n: ParentNode): n is Element => "matches" in n && (n as Element).matches(WAITING);

/**
 * The filled text layers in `root` held out of the layout (hidden) until
 * shown again by `ensureText`: at a zoom's end, every page's text laid out
 * again at its new size at once was a freeze of a third of a second on a
 * dense paper; shown a page a frame once the view rests, it is not.
 */
export function holdText(root: ParentNode): void {
  for (const svg of root.querySelectorAll(".phx-text:not([data-pending])")) svg.setAttribute("data-hold", "");
}

/** Every text layer in `root` (or `root` itself) filled and shown now: read before its lines are (a selection, a link's text, the wasp bands). */
export function ensureText(root: ParentNode | null | undefined): void {
  if (!root) return;
  const all = [...(isWaiting(root) ? [root] : []), ...root.querySelectorAll(WAITING)];
  for (const svg of all) {
    svg.removeAttribute("data-hold");
    if (!svg.hasAttribute("data-pending")) continue;
    const fill = PENDING.get(svg);
    PENDING.delete(svg);
    svg.removeAttribute("data-pending");
    fill?.();
  }
}

/** A match of a search: its page (from 0) and its boxes, [x, y top, w, h] in PDF points from the page's top left. */
export interface Hit {
  k: number;
  boxes: [number, number, number, number][];
}

/** A UTF-16 unit as a search compares it: no accents, lower case (`Ö` as `o`). */
export function fold(c: string): string {
  const f = c.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  return f.length === 1 ? f : c.toLowerCase().length === 1 ? c.toLowerCase() : c;
}

/** TeX's accents, set as glyphs of their own over a letter: as combining marks. */
const COMBINING: Record<string, string> = {
  "\u00a8": "\u0308", "\u00b4": "\u0301", "`": "\u0300", "\u02c6": "\u0302", "^": "\u0302", "\u02dc": "\u0303", "~": "\u0303",
  "\u00af": "\u0304", "\u02c9": "\u0304", "\u02d8": "\u0306", "\u02d9": "\u0307", "\u02da": "\u030a", "\u00b8": "\u0327",
  "\u02c7": "\u030c", "\u02dd": "\u030b", "\u02db": "\u0328",
};

/** A character of a line: its text, its left and right edges. */
export type Ch = { c: string; x: number; r: number };

/**
 * A line's characters with each accent that sits on a letter (its middle
 * over the letter: TeX's \accent puts it before, a cedilla's \oalign
 * after) made one character with it, `¨` `O` as `Ö`.
 */
export function compose(cs: Ch[]): Ch[] {
  const out = cs.map((c) => ({ ...c }));
  const over = (a: Ch, b: Ch | undefined) => b !== undefined && !COMBINING[b.c] && b.c.trim() !== "" && (a.x + a.r) / 2 >= b.x - 0.5 && (a.x + a.r) / 2 <= b.r + 0.5;
  for (let k = 0; k < out.length; k++) {
    const a = out[k];
    const mark = COMBINING[a.c];
    if (!mark) continue;
    const on = over(a, out[k + 1]) ? k + 1 : over(a, out[k - 1]) ? k - 1 : -1;
    if (on < 0) continue;
    out[on].c = (out[on].c + mark).normalize("NFC");
    a.c = "";
  }
  return out.filter((c) => c.c !== "");
}

/** A line's characters (its units and their edges) with TeX's accents made one with their letters (`compose`). */
export function composeLine(l: TextLine): TextLine {
  if (!l.xs || !l.rs) return l;
  const units = Array.from({ length: l.text.length }, (_, i) => l.text[i]);
  const cs = compose(units.map((c, i) => ({ c, x: l.xs![i] ?? l.x, r: l.rs![i] ?? l.x + l.w })));
  const text = cs.map((c) => c.c).join("");
  const xs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.x));
  const rs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.r));
  return { ...l, text, xs, rs };
}

/**
 * Where `q` is on the pages (`pages[k]`: page k's lines), in page order:
 * accents and case ignored, a match running across a line break (the
 * lines joined by a space) boxed on each line; at most `max` matches.
 */
export function search(pages: TextLine[][], q: string, max = 1000): Hit[] {
  const needle = Array.from(q.replace(/\s+/g, " ").trim(), fold).join("");
  if (!needle) return [];
  const out: Hit[] = [];
  for (let k = 0; k < pages.length && out.length < max; k++) {
    const lines = pages[k];
    // (the page as one string, a space between lines; each unit's line)
    let hay = "";
    const at: number[] = [];
    const col: number[] = [];
    lines.forEach((l, i) => {
      if (i) (hay += " "), at.push(-1), col.push(-1);
      for (let j = 0; j < l.text.length; j++) (hay += fold(l.text[j])), at.push(i), col.push(j);
    });
    for (let from = hay.indexOf(needle); from >= 0 && out.length < max; from = hay.indexOf(needle, from + needle.length)) {
      const boxes: Hit["boxes"] = [];
      let cur = -1, x = 0, r = 0;
      const flush = () => {
        if (cur < 0) return;
        const l = lines[cur];
        boxes.push([x, l.y - l.size * 0.78, r - x, l.size * 1.02]);
      };
      for (let u = from; u < from + needle.length; u++) {
        const i = at[u];
        if (i < 0) continue;
        const l = lines[i], xl = l.xs?.[col[u]] ?? l.x, xr = l.rs?.[col[u]] ?? l.x + l.w;
        if (i !== cur) (flush(), (cur = i), (x = xl), (r = xr));
        else (x = Math.min(x, xl)), (r = Math.max(r, xr));
      }
      flush();
      if (boxes.length) out.push({ k, boxes });
    }
  }
  return out;
}

/** A place in a page's text: line `i` (of the lines given), before its unit `off`. */
export interface Caret {
  i: number;
  off: number;
}

/**
 * Where a drag over the pages puts its end, from a point on a page (PDF
 * points from its top left), in its lines; `from`, the line the drag began
 * on (on this page), keeps it in that line's column. Over a line's text: the
 * character there. Beside a line, in its row (a short last line's white
 * space, a column's gap): the end of the line nearest it in the column the
 * drag began in, not a line of the next column. Between rows: the nearest
 * row in that column. The browser's own hit test over empty SVG gave a far
 * text element there, and a drag a little past a line's end selected half
 * the page (two columns: the rest of one, the top of the next).
 */
export function caretAt(lines: TextLine[], x: number, y: number, from?: TextLine): Caret | null {
  if (!lines.length) return null;
  const top = (l: TextLine) => l.y - l.size * 0.8, bottom = (l: TextLine) => l.y + l.size * 0.25;
  const dx = (l: TextLine) => (x < l.x ? l.x - x : x > l.x + l.w ? x - (l.x + l.w) : 0);
  const dy = (l: TextLine) => (y < top(l) ? top(l) - y : y > bottom(l) ? y - bottom(l) : 0);
  // (a column: the lines whose span across the page meets the line the drag began on's)
  const inColumn = (l: TextLine) => !from || (l.x < from.x + from.w && from.x < l.x + l.w);
  const idx = lines.map((_, i) => i);
  const best = (cands: number[], key: (l: TextLine) => number) => cands.reduce((a, b) => (key(lines[b]) < key(lines[a]) ? b : a));
  const row = idx.filter((i) => dy(lines[i]) === 0);
  let i: number;
  // (a row of the drag's column: the next column's rows stand at their own
  // heights, its baselines not the drag's column's)
  const over = row.filter((i) => dx(lines[i]) === 0);
  const rowCol = row.filter((i) => inColumn(lines[i]));
  if (over.length) i = over[0];
  else if (rowCol.length) i = best(rowCol, dx);
  else {
    // (between rows, or above or below every line: the nearest row, in the column if any)
    const col = idx.filter((i) => inColumn(lines[i]) && dx(lines[i]) === 0);
    const near = col.length ? col : idx.filter((i) => inColumn(lines[i]));
    i = best(near.length ? near : idx, (l) => dy(l) * 4 + dx(l));
  }
  const l = lines[i];
  let off = 0;
  if (x >= l.x + l.w) off = l.text.length;
  else if (x > l.x) {
    const n = l.text.length;
    off = n;
    for (let j = 0; j < n; j++) {
      const a = l.xs?.[j] ?? l.x + (l.w * j) / n, b = l.rs?.[j] ?? l.x + (l.w * (j + 1)) / n;
      if (x < (a + b) / 2) {
        off = j;
        break;
      }
    }
  }
  return { i, off };
}
