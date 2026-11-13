// The reading aid (Settings, Appearance: "Colour lines"), BeeLine's and
// WaspLine Reader's (github.com/corollari/waspline-reader): the body
// text's ink coloured by a gradient along each line, its direction
// alternating line to line, so the end of one line and the start of the
// next share a colour and the eye finds the next line. Lines in pairs, a
// pair's colour blue, the next pair's red: ink, then blue at a line's
// end; blue at the next one's start, then ink; ink, then red; red, then
// ink; and so on.
//
// Nothing is drawn again: an SVG over the page's bitmap, a band a line
// with the gradient, blended with what is under it. `lighten` on a white
// page (black ink takes the colour, white paper stays white); `darken`
// on Dark PDF's grey page (its light ink takes a light colour, the grey
// stays grey). The colours are print.ts's (the stops' classes), so Dark
// PDF switched on or off needs nothing drawn. Only the body text's lines
// (the page's most used size), not the centred ones (display maths, page
// numbers) nor those with a wide gap (an equation and its number);
// figures are under no band unless text of the body's size runs over
// them.

import { LAYER_LINES, type TextLine } from "./text.ts";

/** A band to colour: its box (points from the page's top left) and its gradient (0: ink to the colour, 1: the colour to ink; its colour `c`, 0 or 1). */
export interface Band {
  x: number;
  y: number;
  w: number;
  h: number;
  dir: 0 | 1;
  c: 0 | 1;
}

/** A line of a column as the eye reads it: its fragments' extent, baseline, size, non-space glyphs' edges. */
type Row = { l: number; r: number; y: number; size: number; edges: [number, number][] };

/**
 * The bands of a page's lines (TextLine: a run of glyphs on a baseline;
 * a subscript, a size change, starts another): the body's runs grouped
 * into the lines of each column, in reading order, each line a band.
 */
export function bands(lines: TextLine[]): Band[] {
  // (the body's size: the one most characters are set in)
  const count = new Map<number, number>();
  for (const l of lines) {
    const s = Math.round(l.size * 10) / 10;
    count.set(s, (count.get(s) ?? 0) + l.text.replace(/\s/g, "").length);
  }
  let body = 0, most = 0;
  for (const [s, n] of count) if (n > most) (most = n), (body = s);
  if (!body) return [];
  const em = body;
  const runs = lines.filter((l) => Math.abs(l.size - body) <= body * 0.12 && l.text.trim());
  if (!runs.length) return [];
  // (the columns: the long runs' spans merged where they overlap; a page
  // with none (slides, a page of maths), every run's)
  const long = runs.filter((l) => l.w > em * 15);
  const spans = (long.length ? long : runs).map((l) => [l.x, l.x + l.w] as [number, number]).sort((a, b) => a[0] - b[0]);
  const cols: [number, number][] = [];
  for (const s of spans) {
    const last = cols[cols.length - 1];
    if (last && s[0] <= last[1]) last[1] = Math.max(last[1], s[1]);
    else cols.push([...s]);
  }
  const colOf = (l: TextLine) => {
    const m = l.x + l.w / 2;
    let best = 0, d = Infinity;
    cols.forEach(([a, b], i) => {
      const e = m < a ? a - m : m > b ? m - b : 0;
      if (e < d) (d = e), (best = i);
    });
    return best;
  };
  const out: Band[] = [];
  let n = 0;
  cols.forEach((col, ci) => {
    const mine = runs.filter((l) => colOf(l) === ci).sort((a, b) => a.y - b.y || a.x - b.x);
    // (a column's runs on one baseline: one line)
    const rows: Row[] = [];
    for (const l of mine) {
      const last = rows[rows.length - 1];
      const edges: [number, number][] = [];
      for (let i = 0; i < l.text.length; i++) if (l.text[i].trim()) edges.push([l.xs?.[i] ?? l.x, l.rs?.[i] ?? l.x + l.w]);
      if (!l.xs) edges.splice(0, edges.length, [l.x, l.x + l.w]);
      if (last && Math.abs(l.y - last.y) < em * 0.3) {
        last.l = Math.min(last.l, l.x);
        last.r = Math.max(last.r, l.x + l.w);
        last.edges.push(...edges);
      } else rows.push({ l: l.x, r: l.x + l.w, y: l.y, size: l.size, edges });
    }
    for (const row of rows) {
      // (centred, or a wide gap in it (an equation and its number, a
      // table's cells far apart): not a line of the body's)
      const inset = row.l > col[0] + em * 3;
      const edges = row.edges.sort((a, b) => a[0] - b[0]);
      let gap = 0, r = edges.length ? edges[0][1] : row.l;
      for (const [a, b] of edges) (gap = Math.max(gap, a - r)), (r = Math.max(r, b));
      if (inset && (row.r < col[1] - em * 3 || gap > em * 3)) continue;
      const pad = em * 0.15;
      const dir = (n % 2) as 0 | 1;
      out.push({ x: row.l - pad, y: row.y - em * 0.82, w: row.r - row.l + 2 * pad, h: em * 1.14, dir, c: (Math.floor(n / 2) % 2) as 0 | 1 });
      n++;
    }
  });
  return out;
}

const SVG = "http://www.w3.org/2000/svg";
let made = 0;

/** The aid over a page (strips.ts's `.phx-page`: its bitmaps and text layer), once; none where the page has no text layer. */
export function waspOn(page: HTMLElement): void {
  if (page.querySelector(":scope > .phx-wasp")) return;
  const text = page.querySelector<SVGElement>(":scope > .phx-text");
  if (!text) return;
  const lines = [...text.children].map((t) => LAYER_LINES.get(t)).filter((l): l is TextLine => !!l);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-wasp");
  svg.setAttribute("viewBox", text.getAttribute("viewBox") ?? "");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("aria-hidden", "true");
  // (four gradients a page, over each band's own box: ink to blue, blue to ink, ink to red, red to ink)
  const id = `phx-wasp${made++}`;
  const defs = document.createElementNS(SVG, "defs");
  for (const c of [0, 1])
    for (const dir of [0, 1]) {
      const g = document.createElementNS(SVG, "linearGradient");
      g.id = `${id}-${c}${dir}`;
      for (const [off, cls] of [["0", dir ? `w${c}` : "wi"], ["1", dir ? "wi" : `w${c}`]]) {
        const s = document.createElementNS(SVG, "stop");
        s.setAttribute("offset", off);
        s.setAttribute("class", cls);
        g.append(s);
      }
      defs.append(g);
    }
  svg.append(defs);
  for (const b of bands(lines)) {
    const r = document.createElementNS(SVG, "rect");
    r.setAttribute("x", b.x.toFixed(2));
    r.setAttribute("y", b.y.toFixed(2));
    r.setAttribute("width", b.w.toFixed(2));
    r.setAttribute("height", b.h.toFixed(2));
    r.setAttribute("fill", `url(#${id}-${b.c}${b.dir})`);
    svg.append(r);
  }
  page.insertBefore(svg, text);
}

/** The aid off a page. */
export function waspOff(page: HTMLElement): void {
  page.querySelector(":scope > .phx-wasp")?.remove();
}
