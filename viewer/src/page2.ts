// A page's draw list v2 (core-partex's pdfdraw: the PDF pdfTeX wrote, read
// back) drawn as SVG: the glyphs from the PDF's own embedded fonts (their
// outlines, in the view's one glyph store, a `<use>` each, as pdf.js draws them), the
// text over them invisible (for selecting and finding), and a `<path>` per
// painted path (TikZ). A font with no outlines (not embedded) is drawn as
// text in Latin Modern (bundled OpenType, the GUST Font License). Redrawn by parts: each element is keyed by its own
// markup, and an update keeps the elements that did not change, so a
// keystroke replaces the line it changed, not the page.

export interface Draws2 {
  v: 2;
  w: number;
  h: number;
  f: string[];
  /** [font, size, y, "x x ...", text, 1 if its glyphs are drawn from outlines];
   * or [-1, size, y, "x x ...", "", outline font, codes]: the glyphs; either
   * with a colour last when not black (a text run's sixth field then 0 or 1);
   * a glyph run's ninth field, [a, b, c, d]: one transformed glyph (rotated,
   * slanted, extended: XeTeX's), drawn by matrix(a b c d x y), colour null if black. */
  t: (([number, number, number, string, string] | [number, number, number, string, string, 0 | 1, (string | null)?, (string | null)?]) | [-1, number, number, string, "", number, number[], (string | null)?, ([number, number, number, number] | null)?, (string | null)?])[];
  /** The outline fonts' names (ids), by the glyph runs' font. */
  F?: string[];
  /** Outlines by "font:code": SVG path data in 1/1000 em, y up. */
  g?: Record<string, string>;
  /** [d, fill, stroke, width, clip?] */
  p: ([string, string | null, string | null, number] | [string, string | null, string | null, number, string | null])[];
  /** Images: [id, a, b, c, d, e, f, clip?], the matrix taking the image's
   * unit square (row 0 at y 0) to the page, in points from its top left. */
  r: ([string, number, number, number, number, number, number] | [string, number, number, number, number, number, number, string | null])[];
  /** Clips by the page's own ids: [SVG path d, 1 if even-odd, the clip it is inside?]. */
  C?: Record<string, [string, 0 | 1, string?]>;
  /** The paint order, as runs [list (0 p, 1 r, 2 t), first, count]; absent: p, then r, then t. */
  o?: [0 | 1 | 2, number, number][];
  /** The images' data URLs by id (each sent once: the view keeps them, see `store`). */
  I?: Record<string, string>;
  /** How many of the page's drawing operators this list leaves out (none: absent). */
  x?: number;
  /** Links: [x0, y0, x1, y1, uri] or [x0, y0, x1, y1, page (from 0), top | null], in points from the page's top left. */
  L?: Link[];
}

/** A link on a page (`Draws2`'s `L`). */
export type Link = [number, number, number, number, string | number, (number | null)?];

const NS = "http://www.w3.org/2000/svg";

/** CSS families by pdfdraw's font keys (a missing glyph falls back to the math font). */
const FAMILY: Record<string, string> = {
  roman: "PhxLMRoman, PhxLMMath, serif",
  bold: "PhxLMBold, PhxLMMath, serif",
  italic: "PhxLMItalic, PhxLMMath, serif",
  bolditalic: "PhxLMBoldItalic, PhxLMMath, serif",
  mono: "PhxLMMono, monospace",
  math: "PhxLMMath, PhxLMRoman, serif",
};

const FILES: [string, string][] = [
  ["PhxLMRoman", "lmroman10-regular.otf"],
  ["PhxLMBold", "lmroman10-bold.otf"],
  ["PhxLMItalic", "lmroman10-italic.otf"],
  ["PhxLMBoldItalic", "lmroman10-bolditalic.otf"],
  ["PhxLMMono", "lmmono10-regular.otf"],
  ["PhxLMMath", "latinmodern-math.otf"],
];

let fonts: Promise<void> | undefined;
/** Where the text fonts are (the host's: the extension's `chrome.runtime.getURL("fonts/")`, the CLI page's `fonts/`). */
let fontBase = "fonts/";
/** Set where the text fonts are loaded from (before the first page is drawn). */
export const setFontBase = (url: string) => {
  fontBase = url;
};
/** The fonts, once: as FontFaces from their bytes (a stylesheet's @font-face is not seen in a shadow root, and Overleaf's CSP governs URLs). */
export const loadFonts = () =>
  (fonts ??= Promise.all(
    FILES.map(async ([family, file]) => {
      const r = await fetch(fontBase + file);
      const f = new FontFace(family, await r.arrayBuffer());
      document.fonts.add(await f.load());
    }),
  ).then(() => undefined));

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** FNV-1a, as hex: a clip's id from what it is (a page's own ids are only the page's). */
function fnv(s: string): string {
  let x = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) x = Math.imul(x ^ s.charCodeAt(i), 0x01000193);
  return (x >>> 0).toString(16);
}

/** The page's clips: by its own ids, the view-wide id (from the clip's path, rule and parent) and the `<clipPath>`. */
function clips(d: Draws2): Map<string, { id: string; markup: string }> {
  const out = new Map<string, { id: string; markup: string }>();
  const resolve = (cid: string, depth = 0): { id: string; markup: string } | undefined => {
    const have = out.get(cid);
    if (have) return have;
    const c = d.C?.[cid];
    if (!c || depth > 64) return undefined;
    const [path, evenOdd, parent] = c;
    const up = parent ? resolve(parent, depth + 1) : undefined;
    const id = `k${fnv(`${path}|${evenOdd}|${up?.id ?? ""}`)}`;
    const markup = `<clipPath id="${id}" clipPathUnits="userSpaceOnUse"${up ? ` clip-path="url(#${up.id})"` : ""}><path d="${esc(path)}"${evenOdd ? ' clip-rule="evenodd"' : ""}/></clipPath>`;
    const r = { id, markup };
    out.set(cid, r);
    return r;
  };
  for (const cid of Object.keys(d.C ?? {})) resolve(cid);
  return out;
}

/** The page's elements as markup, one string each (the keys of the update). */
export function elements(d: Draws2): string[] {
  const cl = d.C ? clips(d) : undefined;
  // (a clipped element in a group clipped: the clip in the page's space, not the element's own transform's)
  const clip = (m: string, c: string | null | undefined) => {
    const k = c != null ? cl?.get(c) : undefined;
    return k ? `<g clip-path="url(#${k.id})">${m}</g>` : m;
  };
  const path = (e: Draws2["p"][number]) => {
    const [d_, fill, stroke, w, c] = e as [string, string | null, string | null, number, (string | null)?];
    return clip(`<path d="${d_}" fill="${fill ?? "none"}" stroke="${stroke ?? "none"}" stroke-width="${w}"/>`, c);
  };
  // (images: a `<use>` of the view's store, the pixels parsed once)
  const image = (e: Draws2["r"][number]) => {
    const [id, a, b, c_, d_, e_, f, c] = e as [string, number, number, number, number, number, number, (string | null)?];
    return typeof id === "string" ? clip(`<use href="#i${esc(id)}" transform="matrix(${a} ${b} ${c_} ${d_} ${e_} ${f})"/>`, c) : "";
  };
  // (glyph ids: the font's name and the code, the same on every page; the
  // outlines themselves live in the view's one store, see `store`)
  const gid = (fr: number, c: number) => `g${d.F?.[fr] ?? fr}_${c}`;
  const text = (r: Draws2["t"][number]) => {
    if (r[0] === -1) {
      const [, size, y, xs, , fr, codes, colour, m, c] = r as [-1, number, number, string, "", number, number[], (string | null)?, ([number, number, number, number] | null)?, (string | null)?];
      if (m) return clip(`<g transform="matrix(${m.join(" ")} ${xs} ${y})"${colour ? ` fill="${esc(colour)}"` : ""}><use href="#${gid(fr, codes[0])}"/></g>`, c);
      const k = size / 1000;
      const x = xs.split(" ");
      return clip(`<g transform="translate(0 ${y}) scale(${k} ${-k})"${colour ? ` fill="${esc(colour)}"` : ""}>${codes.map((g, i) => `<use href="#${gid(fr, g)}" x="${(+x[i] / k).toFixed(1)}"/>`).join("")}</g>`, c);
    }
    const [f, size, y, xs, txt, outlined, colour, c] = r as [number, number, number, string, string, (0 | 1)?, (string | null)?, (string | null)?];
    return clip(`<text x="${xs}" y="${y}" font-size="${size}" font-family="${esc(FAMILY[d.f[f]] ?? FAMILY.roman)}"${outlined ? ' fill-opacity="0"' : colour ? ` style="fill:${esc(colour)}"` : ""}>${esc(txt)}</text>`, c);
  };
  const out: string[] = [];
  const lists = [d.p ?? [], d.r ?? [], d.t ?? []] as const;
  const one = (l: 0 | 1 | 2, i: number) => {
    const e = lists[l][i];
    if (e === undefined) return;
    const m = l === 0 ? path(e as Draws2["p"][number]) : l === 1 ? image(e as Draws2["r"][number]) : text(e as Draws2["t"][number]);
    if (m) out.push(m);
  };
  if (d.o) for (const [l, first, n] of d.o) for (let i = first; i < first + n; i++) one(l, i);
  else for (const l of [0, 1, 2] as const) for (let i = 0; i < lists[l].length; i++) one(l, i);
  return out;
}

type Keyed = Element & { __k?: string };

/**
 * The view's glyph and image store: one hidden svg's <defs> in the page's
 * root (the shadow root, where `<use href>` finds ids), each outline or
 * image added once, the first time a page brings it; never parsed again.
 */
function store(el: HTMLElement, d: Draws2): void {
  if (!d.g && !d.I && !d.C) return;
  const root = el.getRootNode() as Document | ShadowRoot;
  let defs: Element | null = root.getElementById?.("phx-glyphs") ?? null;
  if (!defs) {
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("style", "position:absolute;width:0;height:0;overflow:hidden");
    svg.setAttribute("aria-hidden", "true");
    svg.innerHTML = `<defs id="phx-glyphs"></defs>`;
    (root instanceof Document ? root.body : root).appendChild(svg);
    defs = svg.firstElementChild!;
  }
  const have = ((defs as Element & { __ids?: Set<string> }).__ids ??= new Set());
  let fresh = "";
  for (const [id, url] of Object.entries(d.I ?? {})) {
    if (have.has("i" + id)) continue;
    have.add("i" + id);
    fresh += `<image id="i${esc(id)}" width="1" height="1" preserveAspectRatio="none" href="${esc(url)}"/>`;
  }
  for (const { id, markup } of d.C ? clips(d).values() : []) {
    if (have.has(id)) continue;
    have.add(id);
    fresh += markup;
  }
  for (const [k, p] of Object.entries(d.g ?? {})) {
    const [fr, c] = k.split(":");
    const id = `g${d.F?.[+fr] ?? fr}_${c}`;
    if (have.has(id)) continue;
    have.add(id);
    fresh += `<path id="${id}" d="${p}"/>`;
  }
  if (fresh) defs!.insertAdjacentHTML("beforeend", fresh);
}

/**
 * Draw `d` into `el` at `w`×`h` CSS pixels: the svg kept if there is one,
 * its elements updated by key (reused when unchanged, new ones parsed in one
 * go, the gone ones dropped).
 */
export function patch(el: HTMLElement, d: Draws2, w: number, h: number): void {
  void loadFonts();
  let svg = el.querySelector<SVGSVGElement>(":scope > svg.page");
  if (!svg || svg.dataset.v !== "2") {
    el.innerHTML = `<svg class="page" xmlns="${NS}" data-v="2" preserveAspectRatio="none" xml:space="preserve"><rect class="paper"/><g class="c"></g></svg>`;
    svg = el.firstElementChild as SVGSVGElement;
  }
  svg.setAttribute("viewBox", `0 0 ${d.w} ${d.h}`);
  svg.setAttribute("width", String(w));
  svg.setAttribute("height", String(h));
  const paper = svg.querySelector("rect.paper")!;
  paper.setAttribute("width", String(d.w));
  paper.setAttribute("height", String(d.h));
  store(el, d);
  const g = svg.querySelector("g.c")!;
  const keys = elements(d);
  const old = new Map<string, Keyed[]>();
  for (const c of Array.from(g.children) as Keyed[]) {
    const k = c.__k ?? "";
    const l = old.get(k);
    if (l) l.push(c);
    else old.set(k, [c]);
  }
  const fresh: string[] = [];
  const slots: (Keyed | number)[] = keys.map((k) => old.get(k)?.pop() ?? (fresh.push(k), fresh.length - 1));
  if (!fresh.length && slots.length === g.children.length && slots.every((s, i) => s === g.children[i])) return void (el.querySelector(":scope > img.ras") || raster(el, svg));
  const made: Keyed[] = [];
  if (fresh.length) {
    const tmp = document.createElementNS(NS, "g");
    tmp.innerHTML = fresh.join("");
    made.push(...(Array.from(tmp.children) as Keyed[]));
    made.forEach((m, i) => (m.__k = fresh[i]));
  }
  // (only what changed is touched: the gone dropped, the new inserted where
  // they go, the kept left where they are)
  const want = slots.map((s) => (typeof s === "number" ? made[s] : s));
  const keep = new Set(want);
  for (const c of Array.from(g.children)) if (!keep.has(c as Keyed)) c.remove();
  let at = g.firstChild;
  for (const n of want) {
    if (at === n) at = at.nextSibling;
    else g.insertBefore(n, at);
  }
  // (changed: drawn live until it settles, then as a picture again)
  live(el, svg);
  raster(el, svg);
}

/**
 * Scrolling: a page's drawing (glyphs, paths, images, clips) as one picture,
 * an <img> of the page as a standalone SVG, which the browser rasterizes
 * once and then only moves; the live SVG keeps its text (selection, and
 * text in the bundled fonts, which an SVG image cannot use) over it. A page
 * that changes is drawn live (the keystroke's path) and becomes a picture
 * again once it has not changed for a moment. Thousands of <use> glyphs
 * painted live cost a 29-page document 14 fps scrolling, 90% busy; as
 * pictures 55 fps, 17% (scripts/scrollbench.mjs).
 */
const RASTER_MS = 250;
type Rastered = HTMLElement & { __ras?: ReturnType<typeof setTimeout>; __gen?: number };

/** The live drawing shown again, the picture (now stale) dropped. */
function live(el: Rastered, svg: SVGSVGElement): void {
  svg.classList.remove("ras");
  const img = el.querySelector<HTMLImageElement>(":scope > img.ras");
  if (img) (URL.revokeObjectURL(img.src), img.remove());
}

function raster(el: Rastered, svg: SVGSVGElement): void {
  clearTimeout(el.__ras);
  const gen = (el.__gen = (el.__gen ?? 0) + 1);
  el.__ras = setTimeout(() => void picture(el, svg, gen), RASTER_MS);
}

async function picture(el: Rastered, svg: SVGSVGElement, gen: number): Promise<void> {
  if (!svg.isConnected || el.__gen !== gen) return;
  const root = el.getRootNode() as Document | ShadowRoot;
  const defs = root.getElementById?.("phx-glyphs");
  const g = svg.querySelector("g.c");
  if (!g) return;
  // (the drawing's markup, without the text; and what it refers to in the store: glyphs, images, clips and their parents)
  const parts: string[] = [];
  const refs = new Set<string>();
  const ref = (m: string) => {
    for (const r of m.matchAll(/(?:href="#|url\(#)([^")]+)/g)) refs.add(r[1]);
  };
  for (const c of Array.from(g.children)) {
    if (c.tagName === "text") continue;
    const m = c.outerHTML;
    parts.push(m);
    ref(m);
  }
  if (!parts.length) return;
  const out: string[] = [];
  const seen = new Set<string>();
  for (let todo = [...refs]; todo.length; ) {
    const id = todo.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const e = defs?.querySelector(`#${CSS.escape(id)}`);
    if (!e) continue;
    const m = e.outerHTML;
    out.push(m);
    for (const r of m.matchAll(/url\(#([^)]+)\)/g)) todo.push(r[1]);
  }
  const vb = svg.getAttribute("viewBox") ?? "";
  const [, , vw, vh] = vb.split(" ");
  const src = `<svg xmlns="${NS}" viewBox="${vb}" width="${vw}" height="${vh}" preserveAspectRatio="none"><defs>${out.join("")}</defs>${parts.join("")}</svg>`;
  const img = new Image();
  img.className = "ras";
  img.alt = "";
  img.src = URL.createObjectURL(new Blob([src], { type: "image/svg+xml" }));
  try {
    await img.decode();
  } catch {
    URL.revokeObjectURL(img.src);
    return;
  }
  // (a change while it decoded: that one's picture follows)
  if (!svg.isConnected || el.__gen !== gen) return void URL.revokeObjectURL(img.src);
  live(el, svg);
  el.insertBefore(img, svg);
  svg.classList.add("ras");
}
