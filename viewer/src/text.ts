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
