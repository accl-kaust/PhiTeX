// The pages drawn as strips (crates/output/render, docs/spec-display-list.md),
// as every host shows them: a page is its bands, each a bitmap the core
// (the watch, phitex-live's Viewer::frame and Viewer::strips) drew once
// and this page keeps by its key. A build's page asks for its frame (its
// bands' keys and places, its text and links), fetches only the bands
// whose keys it has not got, and composes the page from the bitmaps it
// keeps. The host gives the two calls (`StripSource`): the CLI's over
// HTTP from the watch, the frontends' cores over their wasm calls.

import { composeLine, type TextLine } from "./text.ts";
import type { Link } from "./types.ts";

/** A page as the core gives it (Viewer::frame): sizes, bands `[key, top, height]`, text, links, what drawing it took. */
export type Frame = {
  pw: number;
  ph: number;
  w: number;
  h: number;
  bands: [string, number, number][];
  text: TextLine[];
  links: Link[];
  drawn: number;
  kept: number;
  us: { open: number; probe: number; draw: number; all: number };
};

/**
 * The host's two calls: page `k`'s frame at `scale` pixels a point, and
 * the strips of `keys` (each: width and height as little-endian u32, then
 * its RGBA pixels, in the order asked; 0 by 0 for one the core dropped).
 */
export interface StripSource {
  frame(k: number, scale: number): Promise<Frame | null>;
  strips(keys: string[]): Promise<ArrayBuffer | null>;
  /**
   * Optional, one round trip: the frame and the strips of its bands not in
   * `have` (phitex-live's Viewer::frame_with: the JSON's length as a
   * little-endian u32, the JSON with `sent`, then those strips).
   */
  frameWith?(k: number, scale: number, have: string[]): Promise<ArrayBuffer | null>;
}

export class Strips {
  /** The bitmaps kept, by key (the oldest dropped past `max`). */
  private kept = new Map<string, ImageBitmap>();
  private max = 4000;
  private src: StripSource;
  /** Where the last frame's numbers are shown (none: not shown). */
  private chip: HTMLElement | null;

  constructor(src: StripSource, chip: HTMLElement | null = null) {
    this.src = src;
    this.chip = chip;
  }

  /** Each page's bands' keys as last shown (what a frame of it most likely keeps). */
  private shown = new Map<number, string[]>();

  /** Page `k` at `scale` pixels a point: its element (canvas and text), size and links. */
  async page(k: number, scale: number): Promise<{ el: HTMLElement; w: number; h: number; links: Link[] } | null> {
    const t0 = performance.now();
    let f: Frame | null = null;
    let t1 = t0;
    if (this.src.frameWith) {
      const have = (this.shown.get(k) ?? []).filter((key) => this.kept.has(key));
      const buf = await this.src.frameWith(k, scale, have);
      if (!buf || buf.byteLength < 4) return null;
      const n = new DataView(buf).getUint32(0, true);
      f = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, n))) as Frame & { sent?: string[] };
      t1 = performance.now();
      await this.take((f as Frame & { sent?: string[] }).sent ?? [], buf, 4 + n);
    } else {
      f = await this.src.frame(k, scale);
      if (!f) return null;
      t1 = performance.now();
    }
    if (!f) return null;
    this.shown.set(k, f.bands.map((b) => b[0]));
    const want = [...new Set(f.bands.map((b) => b[0]).filter((key) => !this.kept.has(key)))];
    if (want.length) await this.fetch(want);
    const t2 = performance.now();
    const c = document.createElement("canvas");
    c.width = f.pw;
    c.height = f.ph;
    const g = c.getContext("2d")!;
    g.fillStyle = "#fff";
    g.fillRect(0, 0, f.pw, f.ph);
    for (const [key, y] of f.bands) {
      const b = this.kept.get(key);
      if (b) g.drawImage(b, 0, y);
    }
    const t3 = performance.now();
    if (this.chip)
      this.chip.textContent =
      `p${k + 1}: ${f.drawn} drawn, ${f.kept} kept · watch ${(f.us.all / 1000).toFixed(1)} ms ` +
      `(probe ${(f.us.probe / 1000).toFixed(1)}, draw ${(f.us.draw / 1000).toFixed(1)}) · ` +
      `fetch ${(t1 - t0).toFixed(1)}+${(t2 - t1).toFixed(1)} · compose ${(t3 - t2).toFixed(1)} ms`;
    const el = document.createElement("div");
    el.className = "phx-page";
    el.append(c, textLayer(f.text.map(composeLine), f.w, f.h));
    return { el, w: f.w, h: f.h, links: f.links };
  }

  /** The strips of `keys`, made bitmaps and kept. */
  private async fetch(keys: string[]): Promise<void> {
    const buf = await this.src.strips(keys);
    if (buf) await this.take(keys, buf, 0);
  }

  /** The strips of `keys` in `buf` from byte `at` (each its size, then its pixels), made bitmaps and kept. */
  private async take(keys: string[], buf: ArrayBuffer, at: number): Promise<void> {
    const dv = new DataView(buf);
    const made: Promise<void>[] = [];
    for (const key of keys) {
      if (at + 8 > buf.byteLength) break;
      const w = dv.getUint32(at, true), h = dv.getUint32(at + 4, true);
      at += 8;
      if (w && h) {
        const px = new Uint8ClampedArray(buf, at, w * h * 4);
        made.push(
          createImageBitmap(new ImageData(px, w, h)).then((b) => {
            this.kept.set(key, b);
          }),
        );
      }
      at += w * h * 4;
    }
    await Promise.all(made);
    for (const k of this.kept.keys()) {
      if (this.kept.size <= this.max) break;
      this.kept.get(k)?.close();
      this.kept.delete(k);
    }
  }
}

const SVG = "http://www.w3.org/2000/svg";

/** The page's text over its canvas, transparent, for selecting and copying. */
export function textLayer(lines: TextLine[], w: number, h: number): SVGElement {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-text");
  svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  svg.setAttribute("preserveAspectRatio", "none");
  for (const l of lines) {
    const t = document.createElementNS(SVG, "text");
    t.setAttribute("x", l.xs ? l.xs.map((x) => x.toFixed(2)).join(" ") : String(l.x));
    t.setAttribute("y", String(l.y));
    t.setAttribute("font-size", String(l.size));
    t.textContent = l.text;
    svg.append(t);
  }
  return svg;
}

