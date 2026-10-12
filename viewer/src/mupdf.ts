// The pages drawn by MuPDF (Artifex's WebAssembly build, `mupdf` on npm,
// AGPL-3.0-or-later): a build's PDF opened once, a page drawn to RGBA
// pixels at a scale, the outline read as the panel's entries. Host-neutral:
// the host loads the `mupdf` module (the CLI's page from the watch's
// `mupdf/`, the Overleaf extension's offscreen document from its `dist/`;
// MuPDF finds its .wasm beside it) and gives it here.

import type { Entry } from "./outline.ts";
import type { Link } from "./types.ts";

/** A page drawn: its pixels (`pw` by `ph`, RGBA, white under), its size in PDF points, its links. */
export interface Drawn {
  rgba: Uint8ClampedArray;
  pw: number;
  ph: number;
  w: number;
  h: number;
  links: Link[];
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
        return { rgba, pw, ph, w: x1 - x0, h: y1 - y0, links: this.links(page, x0, y0) };
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

/** A drawn page on a canvas (for a host that shows it in the page). */
export function toCanvas(d: Drawn): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = d.pw;
  c.height = d.ph;
  c.getContext("2d")!.putImageData(new ImageData(d.rgba as Uint8ClampedArray<ArrayBuffer>, d.pw, d.ph), 0, 0);
  return c;
}
