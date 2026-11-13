// Colour lines, a reading aid (Settings, its own tab), after the scheme of
// WaspLine Reader (github.com/corollari/waspline-reader): the body
// text's ink coloured by a gradient along each line, its direction
// alternating line to line, so the end of one line and the start of the
// next share a colour and the eye finds the next line. Lines in pairs, a
// pair's colour blue, the next pair's red: ink, then blue at a line's
// end; blue at the next one's start, then ink; ink, then red; red, then
// ink; and so on (Classic's; the tab has a few more pairs, the user's
// own, how far the fade goes, and whether only the body's lines).
//
// Nothing is drawn again: an SVG over the page's bitmap, a band a line
// with the gradient, blended with what is under it. `lighten` on a white
// page (black ink takes the colour, white paper stays white); `darken`
// on Dark PDF's grey page (its light ink takes a light colour, the grey
// stays grey). The colours are CSS variables on the panel (the stops'
// classes, print.ts), so Dark PDF flipped, a colour or the strength
// changed, needs nothing drawn. Only the body text's lines
// (the page's most used size; All text: every size), not the centred ones (display maths, page
// numbers) nor those with a wide gap (an equation and its number);
// figures are under no band unless text of the body's size runs over
// them.

import { LAYER_LINES, type TextLine } from "./text.ts";

/** The colours (Settings, Colour lines): a preset's or the user's two. */
export type WaspColours = "classic" | "blues" | "gray" | "warm" | "custom";

/**
 * The presets: their names, each one's two colours on a white page and on
 * Dark PDF's (lighter: blended with darken over light ink), at strength
 * 100; and at 200, `vivid` (pure blue and red for Classic) and
 * `vividDark` (bright, saturated tints, still above Dark PDF's grey page).
 */
export const WASP_PRESETS: { id: Exclude<WaspColours, "custom">; name: string; light: [string, string]; dark: [string, string]; vivid: [string, string]; vividDark: [string, string] }[] = [
  { id: "classic", name: "Classic", light: ["#1747d6", "#d0202a"], dark: ["#6e98ff", "#ff7b72"], vivid: ["#0000ff", "#ff0000"], vividDark: ["#4f86ff", "#ff4646"] },
  { id: "blues", name: "Blues", light: ["#1a3ccf", "#0a8fc7"], dark: ["#7d93ff", "#5cc8ff"], vivid: ["#0028ff", "#00a2ff"], vividDark: ["#5a74ff", "#1fbcff"] },
  { id: "gray", name: "Gray", light: ["#6e6e6e", "#8c8c8c"], dark: ["#8c8c8c", "#a6a6a6"], vivid: ["#9c9c9c", "#b8b8b8"], vividDark: ["#5e5e5e", "#787878"] },
  { id: "warm", name: "Warm", light: ["#d8620a", "#8a2bbf"], dark: ["#ffa05a", "#c590ff"], vivid: ["#ff7000", "#a000ff"], vividDark: ["#ff8526", "#b060ff"] },
];

/** `c` (#rrggbb) more saturated: pushed away from its grey, `t` of itself again (clamped). */
export function saturate(c: string, t: number): string {
  const p = [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16) || 0);
  const g = (p[0] + p[1] + p[2]) / 3;
  return "#" + p.map((v) => Math.round(Math.min(255, Math.max(0, v + (v - g) * t))).toString(16).padStart(2, "0")).join("");
}

/** The user's choices for the aid: on, its colours (`custom` the two of Custom, for a white page), how far a line's fade goes (0 to 1), which text. */
export interface WaspLook {
  on: boolean;
  colours: WaspColours;
  custom: [string, string];
  /** 0 to 2: 1 the colours as they are (the default), less mixed toward the ink, more toward their vivid ones. */
  strength: number;
  /** 0 to 2: 1 the whole line a fade (the default), less the ink held from its other end that much longer, more the colour held flat from its end. */
  length: number;
  text: "body" | "all";
}

/** `a` and `b` (#rrggbb) mixed, `t` of the way to `b`. */
export function mix(a: string, b: string, t: number): string {
  const p = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) || 0);
  const [x, y] = [p(a), p(b)];
  return "#" + x.map((v, i) => Math.round(v + (y[i] - v) * t).toString(16).padStart(2, "0")).join("");
}

/**
 * The two colours a line fades to, on a white page and on Dark PDF's, at
 * the look's strength: up to 1, each mixed with what leaves the ink as it
 * is (black under lighten, white under darken); past 1, toward its vivid
 * one (Custom's: its own saturated). Custom's dark pair its own colours
 * lightened, half way to white (light enough over Dark PDF's light ink,
 * and never below its grey page).
 */
export function waspColours(l: WaspLook): { light: [string, string]; dark: [string, string] } {
  const p = WASP_PRESETS.find((x) => x.id === l.colours);
  const light = p ? p.light : l.custom;
  const dark = p ? p.dark : (l.custom.map((c) => mix(c, "#ffffff", 0.45)) as [string, string]);
  const vivid = p ? p.vivid : (l.custom.map((c) => saturate(c, 0.8)) as [string, string]);
  const vividDark = p ? p.vividDark : (l.custom.map((c) => mix(saturate(c, 0.8), "#ffffff", 0.3)) as [string, string]);
  const s = Math.min(2, Math.max(0, l.strength));
  const at = (base: [string, string], none: string, more: [string, string]) =>
    base.map((c, i) => (s <= 1 ? mix(none, c, s) : mix(c, more[i], s - 1))) as [string, string];
  return { light: at(light, "#000000", vivid), dark: at(dark, "#ffffff", vividDark) };
}

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
export function bands(lines: TextLine[], all = false): Band[] {
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
  // (all: headings, captions, footnotes too, each line at its own size)
  const runs = lines.filter((l) => (all || Math.abs(l.size - body) <= body * 0.12) && l.text.trim());
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
      if (last && Math.abs(l.y - last.y) < Math.max(last.size, l.size, em) * 0.3) {
        last.l = Math.min(last.l, l.x);
        last.r = Math.max(last.r, l.x + l.w);
        last.edges.push(...edges);
        // (a line's size its largest run's, its baseline that run's: a heading's, not its subscript's)
        if (l.size > last.size) (last.size = l.size), (last.y = l.y);
      } else rows.push({ l: l.x, r: l.x + l.w, y: l.y, size: l.size, edges });
    }
    for (const row of rows) {
      // (centred, or a wide gap in it (an equation and its number, a
      // table's cells far apart): not a line of the body's)
      const em = all ? row.size : body;
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

/** How much of a line the colour holds flat at length 2 (the rest a short fade to ink). */
const HOLD = 0.85;

/**
 * Where a gradient's stops are (ink's, the colour's, fractions of the
 * line) for `length`: up to 1, ink to the colour holds the ink until
 * 1 - length of the way and fades to the colour at the end; past 1, it
 * fades from the start and the colour reaches 1 - HOLD × (length - 1) of
 * the way, flat from there. The colour to ink mirrored.
 */
export function stopsAt(dir: 0 | 1, length: number): [ink: number, colour: number] {
  const l = Math.min(2, Math.max(0, length));
  const ink = Math.max(0, 1 - l), colour = l > 1 ? 1 - HOLD * (l - 1) : 1;
  return dir ? [1 - ink, 1 - colour] : [ink, colour];
}

/** The fade's length on a page's bands (or every page's under `root`): only their gradients' stops moved, nothing drawn again. */
export function waspLength(root: ParentNode, length: number): void {
  for (const s of root.querySelectorAll<SVGStopElement>(".phx-wasp stop")) s.setAttribute("offset", stopsAt(s.dataset.dir === "1" ? 1 : 0, length)[s.classList.contains("wi") ? 0 : 1].toFixed(3));
}

/** The aid over a page (strips.ts's `.phx-page`: its bitmaps and text layer), once (`all`: every line, not only the body's; `length`: how much of a line fades); none where the page has no text layer. */
export function waspOn(page: HTMLElement, all = false, length = 1): void {
  const had = page.querySelector<SVGElement>(":scope > .phx-wasp");
  if (had?.dataset.all === String(all)) return waspLength(page, length);
  had?.remove();
  const text = page.querySelector<SVGElement>(":scope > .phx-text");
  if (!text) return;
  const lines = [...text.children].map((t) => LAYER_LINES.get(t)).filter((l): l is TextLine => !!l);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-wasp");
  svg.setAttribute("viewBox", text.getAttribute("viewBox") ?? "");
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("aria-hidden", "true");
  svg.dataset.all = String(all);
  // (four gradients a page, over each band's own box: ink to blue, blue to ink, ink to red, red to ink)
  const id = `phx-wasp${made++}`;
  const defs = document.createElementNS(SVG, "defs");
  for (const c of [0, 1])
    for (const dir of [0, 1]) {
      const g = document.createElementNS(SVG, "linearGradient");
      g.id = `${id}-${c}${dir}`;
      // (the stops where the fade's length puts them, waspLength)
      const [ink, col] = stopsAt(dir as 0 | 1, length).map((x) => x.toFixed(3));
      for (const [off, cls] of [dir ? [col, `w${c}`] : [ink, "wi"], dir ? [ink, "wi"] : [col, `w${c}`]]) {
        const s = document.createElementNS(SVG, "stop");
        s.setAttribute("offset", off);
        s.setAttribute("class", cls);
        s.dataset.dir = String(dir);
        g.append(s);
      }
      defs.append(g);
    }
  svg.append(defs);
  for (const b of bands(lines, all)) {
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
