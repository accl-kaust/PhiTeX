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

/** Glyphs as boxes to highlight, a box a run on one baseline: [x, y top, w, h] in points. */
export function boxes(gs: Glyph[], all: Glyph[]): [number, number, number, number][] {
  const next = widths(all);
  const lines = new Map<number, [number, number]>();
  for (const g of gs) {
    const k = Math.round(g.y * 2) / 2;
    const r = lines.get(k), e = g.x + (next.get(g) ?? 5);
    lines.set(k, r ? [Math.min(r[0], g.x), Math.max(r[1], e)] : [g.x, e]);
  }
  return [...lines].map(([y, [a, b]]) => [a - 1, y - 8, b - a + 2, 11]);
}

/** Of `hits`, those on the printed line of the glyph nearest byte `at` (a long source line spans many). */
export function lineAt(hits: Glyph[], at: number): Glyph[] {
  let best: Glyph | null = null;
  for (const g of hits) if (!best || Math.abs(g.start - at) < Math.abs(best.start - at)) best = g;
  return best ? hits.filter((g) => Math.abs(g.y - best!.y) < 0.5) : [];
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
