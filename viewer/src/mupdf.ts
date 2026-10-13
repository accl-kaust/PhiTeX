// The pages drawn by MuPDF (Artifex's WebAssembly build, `mupdf` on npm,
// AGPL-3.0-or-later): a build's PDF opened once, a page drawn to RGBA
// pixels at a scale with its text and links, the outline read as the
// panel's entries. Host-neutral:
// the host loads the `mupdf` module (the CLI's page from the watch's
// `mupdf/`, the Overleaf extension's offscreen document from its `dist/`;
// MuPDF finds its .wasm beside it) and gives it here.

import type { Entry } from "./outline.ts";
import type { Link } from "./types.ts";

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
type Ch = { c: string; x: number; r: number };

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

/** A page drawn: its pixels (`pw` by `ph`, RGBA, white under), its size in PDF points, its links, its text. */
export interface Drawn {
  rgba: Uint8ClampedArray;
  pw: number;
  ph: number;
  w: number;
  h: number;
  links: Link[];
  text: TextLine[];
}

/** MuPDF's outline item (its `loadOutline()`). */
interface Item {
  title?: string;
  page?: number;
  down?: Item[];
}

/** One PDF, open in MuPDF. */
export class MupdfDoc {
  private m: any;
  private doc: any;

  /** `m`: the `mupdf` module, loaded by the host. */
  constructor(m: any, pdf: Uint8Array) {
    this.m = m;
    this.doc = m.Document.openDocument(pdf, "application/pdf");
  }

  get pages(): number {
    return this.doc.countPages();
  }

  /** Page `k` (from 0) drawn at `scale` pixels a point; null: no such page. */
  draw(k: number, scale: number): Drawn | null {
    if (k < 0 || k >= this.pages) return null;
    const page = this.doc.loadPage(k);
    try {
      const [x0, y0, x1, y1] = page.getBounds();
      // (no alpha: drawn on white, as the page is printed)
      const pix = page.toPixmap(this.m.Matrix.scale(scale, scale), this.m.ColorSpace.DeviceRGB, false, true);
      try {
        const pw = pix.getWidth(), ph = pix.getHeight(), stride = pix.getStride();
        const rgb: Uint8ClampedArray = pix.getPixels();
        const rgba = new Uint8ClampedArray(pw * ph * 4);
        for (let y = 0; y < ph; y++) {
          let i = y * stride, o = y * pw * 4;
          for (let x = 0; x < pw; x++, i += 3, o += 4) {
            rgba[o] = rgb[i];
            rgba[o + 1] = rgb[i + 1];
            rgba[o + 2] = rgb[i + 2];
            rgba[o + 3] = 255;
          }
        }
        return { rgba, pw, ph, w: x1 - x0, h: y1 - y0, links: this.links(page, x0, y0), text: this.text(page, x0, y0) };
      } finally {
        pix.destroy();
      }
    } finally {
      page.destroy();
    }
  }

  /** A page's links, from its top left (`x0`, `y0`: its box's corner). */
  private links(page: any, x0: number, y0: number): Link[] {
    const out: Link[] = [];
    for (const l of page.getLinks() as any[]) {
      const [a, b, c, d] = l.getBounds();
      if (l.isExternal()) out.push([a - x0, b - y0, c - x0, d - y0, l.getURI()]);
      else {
        const dest = this.doc.resolveLinkDestination(l);
        if (dest.page >= 0) out.push([a - x0, b - y0, c - x0, d - y0, dest.page, Number.isFinite(dest.y) ? dest.y : null]);
      }
    }
    return out;
  }

  /** A page's text, line by line (MuPDF's structured text), from its top left. */
  private text(page: any, x0: number, y0: number): TextLine[] {
    const out: TextLine[] = [];
    let line: TextLine | null = null;
    let chars: Ch[] = [];
    let right = 0;
    const st = page.toStructuredText("preserve-whitespace");
    try {
      st.walk({
        beginLine: (bbox: number[]) => {
          line = null;
          chars = [];
          right = bbox[2] - x0;
        },
        onChar: (c: string, origin: number[], _font: unknown, size: number, quad: number[]) => {
          line ??= { x: origin[0] - x0, y: origin[1] - y0, w: 0, size, text: "" };
          const x = Math.min(quad[0], quad[4]) - x0;
          chars.push({ c, x, r: Math.max(quad[2], quad[6], x + x0) - x0 });
        },
        endLine: () => {
          if (!line) return;
          const cs = compose(chars);
          const text = cs.map((c) => c.c).join("");
          if (!text.trim()) return;
          // (one position a UTF-16 unit, as the SVG counts them)
          const xs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.x));
          const rs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.r));
          out.push({ ...line, text, xs, rs, w: Math.max(right - line.x, 0) });
        },
      });
    } finally {
      st.destroy();
    }
    return out;
  }

  /** Each page's text, kept for searching (this PDF's: it never changes). */
  private texts = new Map<number, TextLine[]>();

  private textOf(k: number): TextLine[] {
    let t = this.texts.get(k);
    if (!t) {
      const page = this.doc.loadPage(k);
      try {
        const [x0, y0] = page.getBounds();
        t = this.text(page, x0, y0);
      } finally {
        page.destroy();
      }
      this.texts.set(k, t);
    }
    return t;
  }

  /**
   * Where `q` is on the pages, in page order: accents and case ignored, a
   * match running across a line break (the lines joined by a space) boxed
   * on each line; at most `max` matches.
   */
  search(q: string, max = 1000): Hit[] {
    const needle = Array.from(q.replace(/\s+/g, " ").trim(), fold).join("");
    if (!needle) return [];
    const out: Hit[] = [];
    for (let k = 0; k < this.pages && out.length < max; k++) {
      const lines = this.textOf(k);
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

  /** The PDF's outline (its bookmarks) as the panel's entries. */
  outline(): Entry[] {
    const conv = (items: Item[] | null | undefined): Entry[] =>
      (items ?? []).map((i) => ({ t: i.title ?? "", p: i.page ?? null, y: null, k: conv(i.down) }));
    return conv(this.doc.loadOutline());
  }

  destroy(): void {
    this.doc.destroy();
  }
}

const SVG = "http://www.w3.org/2000/svg";

/**
 * A drawn page for a host that shows it in the page: its canvas, and over
 * it its text, transparent, for selecting, copying and finding (an SVG in
 * PDF points: each line stretched to its width, so it scales with the page).
 */
export function toCanvas(d: Drawn): HTMLElement {
  const el = document.createElement("div");
  el.className = "phx-page";
  const c = document.createElement("canvas");
  c.width = d.pw;
  c.height = d.ph;
  c.getContext("2d")!.putImageData(new ImageData(d.rgba as Uint8ClampedArray<ArrayBuffer>, d.pw, d.ph), 0, 0);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-text");
  svg.setAttribute("viewBox", `0 0 ${d.w} ${d.h}`);
  svg.setAttribute("preserveAspectRatio", "none");
  for (const l of d.text) {
    const t = document.createElementNS(SVG, "text");
    // (each character where its glyph is: a stand-in font's widths never move it)
    t.setAttribute("x", l.xs ? l.xs.map((x) => x.toFixed(2)).join(" ") : String(l.x));
    t.setAttribute("y", String(l.y));
    t.setAttribute("font-size", String(l.size));
    if (!l.xs && l.w > 0) {
      t.setAttribute("textLength", String(l.w));
      t.setAttribute("lengthAdjust", "spacingAndGlyphs");
    }
    t.textContent = l.text;
    svg.append(t);
  }
  el.append(c, svg);
  return el;
}
