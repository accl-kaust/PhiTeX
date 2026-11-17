// Source ↔ page, from the core's glyph origins (each glyph a page shows, the
// source bytes it came from): a double-click on the page finds the glyph
// nearest it and its source; a place in the source finds the glyphs that
// came from its line, to highlight on the page.

/** A glyph: its origin on the page (PDF points, from the top left), its source. */
export interface Glyph {
  x: number;
  y: number;
  /** A project path, or null: no source (pdfTeX's own spaces, \pdfliteral text). */
  file: string | null;
  start: number;
  end: number;
  /** Made by a macro, a counter, \the: the range is the call's. */
  synth: boolean;
}

/**
 * phitex-live's `origins` answer, the same from the watch and the wasm
 * core: per glyph its file (an index into `files`, 2^32-1 none), its
 * source byte range, its origin (f32 points), as base64 columns of
 * little-endian words, and one synthesized bit a glyph.
 */
export interface Origins {
  files: string[];
  n: number;
  file?: string;
  start?: string;
  end?: string;
  synth?: string;
  x?: string;
  y?: string;
}

/** A page's glyphs from the core's `origins` answer; `path` maps the job's file names to the project's. */
export function glyphs(json: Origins, path: (name: string) => string | null): Glyph[] {
  if (!json?.n || !json.file || !json.start || !json.end || !json.synth || !json.x || !json.y) return [];
  const files = json.files.map(path);
  const words = (b64: string) => {
    const b = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    return new DataView(b.buffer);
  };
  const [file, start, end, synth, x, y] = [json.file, json.start, json.end, json.synth, json.x, json.y].map(words);
  const out: Glyph[] = [];
  for (let i = 0; i < json.n; i++) {
    const f = file.getUint32(4 * i, true);
    const s = 4 * (i >> 5) < synth.byteLength ? (synth.getUint32(4 * (i >> 5), true) >>> (i & 31)) & 1 : 0;
    out.push({ x: x.getFloat32(4 * i, true), y: y.getFloat32(4 * i, true), file: f === 0xffffffff ? null : (files[f] ?? null), start: start.getUint32(4 * i, true), end: end.getUint32(4 * i, true), synth: s === 1 });
  }
  return out;
}

/** The glyph with a source nearest (x, y): a line apart weighs more than a word apart. */
export function nearest(gs: Glyph[], x: number, y: number): Glyph | null {
  let best: Glyph | null = null, d = Infinity;
  for (const g of gs) {
    if (!g.file) continue;
    // (the glyph's body is above its baseline: its middle, about 3 pt up)
    const dy = g.y - 3 - y, dx = g.x + 2.5 - x;
    const e = dx * dx + 9 * dy * dy + (g.synth ? 400 : 0);
    if (e < d) (d = e, best = g);
  }
  return best;
}

/** The glyphs of `file` that came from its bytes [lo, hi) (a source line), not from a macro. */
export function from(gs: Glyph[], file: string, lo: number, hi: number): Glyph[] {
  return gs.filter((g) => g.file === file && !g.synth && g.start < hi && g.end > lo);
}

/** A page's glyphs' widths, by its glyph list: their places never change (an edit maps only their sources), so worked out once a page. */
const widthsOf = new WeakMap<Glyph[], Map<Glyph, number>>();
function widths(all: Glyph[]): Map<Glyph, number> {
  let next = widthsOf.get(all);
  if (next) return next;
  // (a glyph's width: to the next glyph on its baseline, else 5 pt)
  next = new Map<Glyph, number>();
  for (let i = 0; i < all.length; i++) {
    const n = all[i + 1];
    next.set(all[i], n && Math.abs(n.y - all[i].y) < 0.5 && n.x > all[i].x && n.x - all[i].x < 15 ? n.x - all[i].x : 5);
  }
  widthsOf.set(all, next);
  return next;
}

/**
 * A page's printed lines, by its glyph list: each glyph's line, a number
 * (glyphs in the page's order, the order they were set). A line is a run of
 * glyphs on one baseline going right; a sub- or superscript is a run of its
 * own, and the line it left goes on after it. Two columns' lines on one
 * baseline are never one line: the left column's is set whole before the
 * right's, so they are runs apart, and a jump right wider than any space
 * (a column's gap) starts a line too. Worked out once a page.
 */
const linesOf = new WeakMap<Glyph[], Map<Glyph, number>>();
export function lines(all: Glyph[]): Map<Glyph, number> {
  let out = linesOf.get(all);
  if (out) return out;
  out = new Map<Glyph, number>();
  // (the runs met last, the line each goes on: a script's return to its base)
  const recent: { id: number; y: number; x: number }[] = [];
  let cur: { id: number; y: number; x: number } | null = null, next = 0;
  const goesOn = (r: { y: number; x: number }, g: Glyph) => Math.abs(g.y - r.y) < 0.5 && g.x >= r.x - 6 && g.x - r.x < LINE_GAP;
  for (const g of all) {
    if (!cur || !goesOn(cur, g)) {
      if (cur) recent.unshift(cur), recent.length > 4 && recent.pop();
      const back = recent.find((r) => goesOn(r, g));
      if (back) recent.splice(recent.indexOf(back), 1);
      cur = back ?? { id: next++, y: g.y, x: g.x };
    }
    cur.x = Math.max(cur.x, g.x);
    out.set(g, cur.id);
  }
  linesOf.set(all, out);
  return out;
}

/** The widest jump right a line's glyphs make (points): a stretched space is less, a column's gap more. */
const LINE_GAP = 40;

/**
 * Glyphs as boxes to highlight, a box a printed line (`lines`: never across
 * two columns), in the order their lines were set: [x, y top, w, h] in points.
 */
export function boxes(gs: Glyph[], all: Glyph[]): [number, number, number, number][] {
  return lineBoxes(gs, all).boxes;
}

/** `boxes`, and each box's printed line (`lines`' number). */
export function lineBoxes(gs: Glyph[], all: Glyph[]): { boxes: [number, number, number, number][]; ids: number[] } {
  const next = widths(all), line = lines(all);
  const out = new Map<number, [number, number, number]>();
  for (const g of gs) {
    const k = line.get(g) ?? -1 - out.size;
    const r = out.get(k), e = g.x + (next.get(g) ?? 5);
    out.set(k, r ? [Math.min(r[0], g.x), Math.max(r[1], e), r[2]] : [g.x, e, Math.round(g.y * 2) / 2]);
  }
  return { boxes: [...out.values()].map(([a, b, y]) => [a - 1, y - 8, b - a + 2, 11]), ids: [...out.keys()] };
}

/** Of `hits` (glyphs of page `all`), those on the printed line (`lines`) of the glyph nearest byte `at` (a long source line spans many, and two columns). */
export function lineAt(hits: Glyph[], at: number, all: Glyph[]): Glyph[] {
  let best: Glyph | null = null;
  for (const g of hits) if (!best || Math.abs(g.start - at) < Math.abs(best.start - at)) best = g;
  if (!best) return [];
  const line = lines(all), id = line.get(best);
  return hits.filter((g) => line.get(g) === id);
}

/**
 * The glyph of `all` a character of the text layer at (x, y) is (its left
 * edge `x`, `edge` "left"; its right, "right"; y its baseline): on that
 * baseline, the one whose origin is that edge, else the one it is in. Two
 * columns share baselines: the column is the one the edge is in. Null: none
 * on that baseline.
 */
export function glyphAt(all: Glyph[], x: number, y: number, edge: "left" | "right"): number | null {
  let best = -1, d = Infinity;
  for (let i = 0; i < all.length; i++) {
    const g = all[i];
    if (Math.abs(g.y - y) >= 1) continue;
    // (a right edge: the last glyph that starts before it; a left: the one starting nearest it, at or before it first)
    const e = edge === "right" ? (g.x < x - 0.1 ? x - g.x : Infinity) : g.x <= x + 0.5 ? x - g.x : 2 * (g.x - x) + 1;
    if (e < d) (d = e), (best = i);
  }
  return best < 0 ? null : best;
}

/**
 * Text selected on the pages, by page `k`: its boxes ([x, y top, w, h], PDF
 * points from the page's top left); on its first character's page, `from`,
 * that character's left edge on its baseline [x, y]; on its last's, `to`,
 * that one's right edge (the text layer's: viewer.ts selectionEnds).
 */
export interface PageSel {
  k: number;
  rects: [number, number, number, number][];
  from?: [number, number];
  to?: [number, number];
}

/** A place in the pages' glyphs, in the order they were set: page `k`, glyph `i` of its list. */
export interface At {
  k: number;
  i: number;
}

/**
 * The source of the glyphs from `a` to `b` (inclusive) in the pages' order
 * (`pages(k)`: page k's glyphs): the file most of them came from, from its
 * first glyph's start to its last's end (bytes); where that is backwards (a
 * float's text first), from the least start to the greatest end. Null: none
 * of them came from a file.
 */
export function sourceBetween(pages: (k: number) => Glyph[], a: At, b: At): { file: string; start: number; end: number; n: number } | null {
  if (b.k < a.k || (b.k === a.k && b.i < a.i)) [a, b] = [b, a];
  const by = new Map<string, { first: Glyph; last: Glyph; lo: number; hi: number; n: number }>();
  for (let k = a.k; k <= b.k; k++) {
    const all = pages(k);
    for (let i = k === a.k ? a.i : 0; i <= (k === b.k ? b.i : all.length - 1); i++) {
      const g = all[i];
      if (!g?.file || g.synth) continue;
      const r = by.get(g.file);
      if (r) (r.last = g), (r.lo = Math.min(r.lo, g.start)), (r.hi = Math.max(r.hi, g.end)), r.n++;
      else by.set(g.file, { first: g, last: g, lo: g.start, hi: g.end, n: 1 });
    }
  }
  const best = [...by].sort((x, y) => y[1].n - x[1].n)[0];
  if (!best) return null;
  const [file, r] = best;
  return r.last.end > r.first.start ? { file, start: r.first.start, end: r.last.end, n: r.n } : { file, start: r.lo, end: r.hi, n: r.n };
}

/**
 * Where a span [start, end) of `file` chosen on the pages (a note's) is
 * drawn: every glyph from its first to its last in the pages' order, as a
 * selection between them is, not every glyph from its bytes (a float, a
 * footnote's text, a paragraph's lines in another column are bytes of a
 * span and not of the text selected). Its first: the glyph at byte
 * `start` (on page `page` where it is set there too), else the one from
 * the span with the least start; its last: the first glyph after that at
 * byte `end - 1`, else the one from the span with the greatest end. The
 * glyphs by page, in order (`i`: the first's place in its page's); []
 * where none of the span is set.
 */
export function extent(pages: Glyph[][], file: string, start: number, end: number, page?: number): { k: number; gs: Glyph[]; i: number }[] {
  end = Math.max(end, start + 1);
  const mine = (g: Glyph) => g.file === file && !g.synth;
  const order = page !== undefined && page < pages.length ? [page, ...pages.keys()] : [...pages.keys()];
  let a: At | null = null;
  for (const k of order) {
    const i = pages[k].findIndex((g) => mine(g) && g.start <= start && g.end > start);
    if (i >= 0) {
      a = { k, i };
      break;
    }
  }
  const inSpan = (g: Glyph) => mine(g) && g.start < end && g.end > start;
  if (!a) {
    let lo = Infinity;
    for (let k = 0; k < pages.length; k++)
      for (let i = 0; i < pages[k].length; i++) {
        const g = pages[k][i];
        if (inSpan(g) && g.start < lo) (lo = g.start), (a = { k, i });
      }
  }
  if (!a) return [];
  const from: At = a;
  // (its last: the first at byte end - 1 from there on; else the greatest end from there on)
  let b: At | null = null, far: At = from, farEnd = -Infinity;
  for (let k = from.k; k < pages.length && !b; k++) {
    const gs = pages[k];
    for (let i = k === from.k ? from.i : 0; i < gs.length; i++) {
      const g = gs[i];
      if (!inSpan(g)) continue;
      if (g.end >= end) {
        b = { k, i };
        break;
      }
      if (g.end > farEnd) (farEnd = g.end), (far = { k, i });
    }
  }
  b ??= far;
  const out: { k: number; gs: Glyph[]; i: number }[] = [];
  for (let k = from.k; k <= b.k; k++) {
    const i = k === from.k ? from.i : 0;
    const gs = pages[k].slice(i, k === b.k ? b.i + 1 : undefined);
    if (gs.length) out.push({ k, gs, i });
  }
  return out;
}

/**
 * The word the cursor at byte `at` follows, on the line [lo, hi) of `text`
 * (bytes): the one it is in, else, in spaces, the word before it on the
 * line, else the one after; null on a line with no word. The server's
 * `word_near` (phitex-live lsp.rs) takes the same.
 */
export function wordNear(text: Uint8Array, at: number, lo: number, hi: number): [number, number] | null {
  const here = wordBytes(text, at);
  if (here) return here;
  const w = (c: number) => c >= 0x80 || (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 39 || c === 45;
  for (let i = Math.min(at, text.length) - 1; i >= lo; i--) if (w(text[i])) return wordBytes(text, i);
  for (let i = at; i < Math.min(hi, text.length); i++) if (w(text[i])) return wordBytes(text, i);
  return null;
}

/** Of `hits` (one source line's glyphs), the glyph nearest byte `at`: the last at or before it, else the first after. */
export function glyphNear(hits: Glyph[], at: number): Glyph | null {
  let before: Glyph | null = null, after: Glyph | null = null;
  for (const g of hits) {
    if (g.start <= at) { if (!before || g.start > before.start) before = g; }
    else if (!after || g.start < after.start) after = g;
  }
  return before ?? after;
}

/** The source word around byte `at` of `text` ([lo, hi) in bytes): letters, digits, a command's name; null in spaces. */
export function wordBytes(text: Uint8Array, at: number): [number, number] | null {
  const w = (c: number) => c >= 0x80 || (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 39 || c === 45;
  let lo = at, hi = at;
  while (lo > 0 && w(text[lo - 1])) lo--;
  while (hi < text.length && w(text[hi])) hi++;
  return lo < hi ? [lo, hi] : null;
}
