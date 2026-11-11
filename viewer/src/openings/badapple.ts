// Bad Apple!!: the shadow-art PV played on the sheet, drawn as curves (its
// frames traced to Béziers, badapple-data.ts), sharp at any size, its
// opening (Reimu's fade-in to the cut to white) at the PV's 30 frames a
// second, again and again. The build's steps are its subtitles, under the
// picture as a PV's lyrics are, "Did you know" smaller under them; the
// credit at the foot. Page in, as the PV changes scenes: the picture stops
// and its silhouettes melt into the page's, a threshold over the two
// blurred and mixed (the frame's black, the page's ink), the blur growing
// then shrinking: the shapes run into blobs, the blobs gather into the
// page's paragraphs, the paragraphs sharpen into its lines, and the page
// itself is there.

import { html } from "../html.ts";
import { layer, ending, loop, PAPER, type Opening } from "./kit.ts";
import { FRAMES, W, H, FPS, DATA } from "./badapple-data.ts";
import { keyed } from "../hints.ts";

/** The frames as paths, read once (every sheet after the first plays them at once). */
let frames: Promise<Path2D[]> | null = null;
const read = () =>
  (frames ??= (async () => {
    const z = Uint8Array.from(atob(DATA), (c) => c.charCodeAt(0));
    const b = new Uint8Array(await new Response(new Blob([z]).stream().pipeThrough(new DecompressionStream("deflate-raw"))).arrayBuffer());
    let i = 0;
    const varint = () => {
      let n = 0, s = 0, x: number;
      do (x = b[i++]), (n |= (x & 0x7f) << s), (s += 7);
      while (x & 0x80);
      return n;
    };
    const zz = () => {
      const n = varint();
      return n & 1 ? -((n + 1) >> 1) : n >> 1;
    };
    const out: Path2D[] = [];
    for (let f = 0; f < FRAMES; f++) {
      const p = new Path2D();
      for (let c = varint(); c > 0; c--) {
        const n = varint();
        let x = varint(), y = varint();
        p.moveTo(x, y);
        for (let s = 0; s < n; s++) {
          const x1 = (x += zz()), y1 = (y += zz()), x2 = (x += zz()), y2 = (y += zz()), x3 = (x += zz()), y3 = (y += zz());
          p.bezierCurveTo(x1, y1, x2, y2, x3, y3);
        }
        p.closePath();
      }
      out.push(p);
    }
    return out;
  })());

/** The page in, in ms: the morph, then the page itself taking over from its likeness. */
const MORPH = 1150, SETTLE = 220;
const ease = (k: number) => (k < 0.5 ? 4 * k * k * k : 1 - (-2 * k + 2) ** 3 / 2);
/** The morph's grid, cells across (the sheet's shape down), and its blurs (in cells, each a level). */
const LW = 300, SIGMAS = [0, 1.2, 2.5, 4.5, 7.5, 12];

/** `a` (lw×lh) blurred by about `sigma` cells: three running box sums each way, the edges held. */
function blur(a: Float32Array, lw: number, lh: number, sigma: number): Float32Array {
  const r = Math.max(1, Math.round((Math.sqrt(4 * sigma * sigma + 1) - 1) / 2)), d = 2 * r + 1;
  let src = a.slice(), dst = new Float32Array(a.length);
  for (let pass = 0; pass < 6; pass++) {
    const across = pass % 2 === 0, n = across ? lw : lh, m = across ? lh : lw, step = across ? 1 : lw, line = across ? lw : 1;
    for (let j = 0; j < m; j++) {
      const o = j * line, last = o + (n - 1) * step;
      let sum = src[o] * (r + 1);
      for (let i = 1; i <= r; i++) sum += src[o + Math.min(i, n - 1) * step];
      for (let i = 0, at = o; i < n; i++, at += step) {
        dst[at] = sum / d;
        const add = i + r + 1 < n ? o + (i + r + 1) * step : last, drop = i - r > 0 ? o + (i - r) * step : o;
        sum += src[add] - src[drop];
      }
    }
    [src, dst] = [dst, src];
  }
  return src;
}

/**
 * `a` at SIGMAS[l], made when first asked (a level or two a frame as the blur grows: never all at
 * once); with `norm`, each level's ink stretched so its dense parts are solid (paragraphs as blocks,
 * the text's strokes whole).
 */
function levels(a: Float32Array, lw: number, lh: number, norm: boolean): (l: number) => Float32Array {
  const made: Float32Array[] = [];
  return (l) => {
    if (made[l]) return made[l];
    const s = SIGMAS[l], b = s ? blur(a, lw, lh, s) : a.slice();
    if (norm) {
      // (its 90th percentile of what is inked at all, by a histogram)
      const hist = new Uint32Array(256);
      let n = 0;
      for (let i = 0; i < b.length; i++) if (b[i] > 0.03) hist[Math.min(255, (b[i] * 255) | 0)]++, n++;
      let top = 255;
      for (let v = 0, c = 0; v < 256; v++) if ((c += hist[v]) >= n * 0.9) {
        top = v;
        break;
      }
      const k = (s ? 0.8 : 1.15) / Math.max(top / 255, 1e-3);
      for (let i = 0; i < b.length; i++) b[i] = Math.min(1, b[i] * k);
    }
    return (made[l] = b);
  };
}

/** What `draw` puts on an lw×lh canvas, as coverage (0 paper, 1 ink): `ink` the lightness that is ink. */
function field(lw: number, lh: number, ink: "dark" | "light", draw: (c: OffscreenCanvasRenderingContext2D) => void): Float32Array {
  const c = new OffscreenCanvas(lw, lh).getContext("2d", { willReadFrequently: true })!;
  draw(c);
  const px = c.getImageData(0, 0, lw, lh).data, out = new Float32Array(lw * lh);
  for (let i = 0; i < out.length; i++) {
    const l = (0.299 * px[4 * i] + 0.587 * px[4 * i + 1] + 0.114 * px[4 * i + 2]) / 255;
    out[i] = ink === "dark" ? 1 - l : l;
  }
  return out;
}

/** "#rrggbb" as [r, g, b]. */
const rgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));

export function badapple(box: HTMLElement, dark: boolean): Opening {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ba",
    `.phx-ba { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; }
.phx-ba canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.phx-ba .scr { position: absolute; left: 8cqw; right: 8cqw; top: 16cqw; aspect-ratio: 4 / 3; box-shadow: 0 0 0 .3cqw #000, 0 1.4cqw 3cqw rgb(0 0 0 / 22%); transition: opacity .25s; }
.phx-ba .sub { position: absolute; left: 8%; right: 8%; top: 86cqw; text-align: center; color: ${ink0}; transition: opacity .25s; }
.phx-ba .sub .what { font-size: 3.6cqw; letter-spacing: .06em; }
.phx-ba .sub .det { font-size: 2.4cqw; color: ${mute}; margin-top: .8cqw; min-height: 1.4em; }
.phx-ba .sub .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 3.6cqw; line-height: 1.45; font-style: italic; }
.phx-ba .cr { position: absolute; left: 0; right: 0; bottom: 3cqw; text-align: center; font-size: 1.7cqw; color: ${mute}; opacity: .8; transition: opacity .25s; }
.phx-ba.out .sub, .phx-ba.out .cr, .phx-ba.out .scr { opacity: 0; }`,
    html`<div class="scr"></div><canvas></canvas><div class="sub"><div class="what"></div><div class="det"></div><div class="hint"></div></div>
     <div class="cr">Bad Apple!! · Alstroemeria Records feat. nomico · PV: Anira · original: ZUN</div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const cv = el.querySelector("canvas")!, ctx = cv.getContext("2d")!, dpr = devicePixelRatio || 1;
  let paths: Path2D[] | null = null;
  void read().then((p) => (paths = p));
  const t0 = performance.now();
  // (the screen's place on the sheet, as .scr's: 8cqw in, 16cqw down, 84cqw wide, 4:3)
  const screen = (width: number) => ({ x: 0.08 * width, y: 0.16 * width, w: 0.84 * width, h: (0.84 * width * H) / W });
  let drawn = -1;
  /** The page in, once begun: its start, its grid, the fields' levels (the frame's, the page's, the sheet around the screen), and its colours. */
  let morph: {
    at: number; lw: number; lh: number;
    a: (l: number) => Float32Array; b: (l: number) => Float32Array; m: (l: number) => Float32Array;
    lo: CanvasRenderingContext2D; img: ImageData; paper: number[]; ink: number[]; sheet: number[];
  } | null = null;
  const begin = (page?: HTMLCanvasElement) => {
    if (!paths) return;
    const f = drawn < 0 ? 0 : drawn, lw = LW, lh = Math.round((LW * cv.height) / Math.max(1, cv.width)), s = screen(lw);
    // (the page as it is shown: white paper, or the dark PDF setting's (print.ts's filter on white and on black))
    const night = !!page && getComputedStyle(page).filter.includes("invert");
    const paper = night ? [40, 40, 40] : [255, 255, 255], ink = night ? [225, 225, 225] : [17, 17, 17];
    // (the frame as ink: on white paper its black, on dark its white; the sheet around the screen paper either way)
    const a = field(lw, lh, night ? "light" : "dark", (c) => {
      c.fillStyle = night ? "#000" : "#fff";
      c.fillRect(0, 0, lw, lh);
      c.fillStyle = "#000";
      c.fillRect(s.x, s.y, s.w, s.h);
      c.setTransform(s.w / W, 0, 0, s.h / H, s.x, s.y);
      c.fillStyle = "#fff";
      c.fill(paths![f], "evenodd");
    });
    // (the page's ink: its bitmap is white paper, whatever the filter shows)
    const b = field(lw, lh, "dark", (c) => {
      c.fillStyle = "#fff";
      c.fillRect(0, 0, lw, lh);
      if (page) (c.imageSmoothingQuality = "high"), c.drawImage(page, 0, 0, lw, lh);
    });
    // (the sheet around the screen, in the theme's colour until it turns the page's paper)
    const m = field(lw, lh, "light", (c) => {
      c.fillStyle = "#fff";
      c.fillRect(0, 0, lw, lh);
      c.fillStyle = "#000";
      c.fillRect(s.x, s.y, s.w, s.h);
    });
    // (the morph drawn small, on a canvas of its own the sheet's size on screen: the compositor scales it)
    const lo = Object.assign(document.createElement("canvas"), { width: lw, height: lh });
    cv.replaceWith(lo);
    const sheet = rgb(dark ? PAPER.dark : PAPER.light);
    morph = { at: performance.now(), lw, lh, a: levels(a, lw, lh, false), b: levels(b, lw, lh, true), m: levels(m, lw, lh, false), lo: lo.getContext("2d")!, img: new ImageData(lw, lh), paper, ink, sheet };
  };
  const stop = loop(() => {
    const now = performance.now();
    // (the canvas the sheet's size, in device pixels: set when the sheet's size changes, never while it moves)
    const bw = Math.round(el.clientWidth * dpr), bh = Math.round(el.clientHeight * dpr);
    if (!morph && (cv.width !== bw || cv.height !== bh)) (cv.width = bw), (cv.height = bh), (drawn = -1);
    if (!paths) return;
    if (!morph) {
      // (the PV's frame now, 30 a second, drawn only when it changes)
      const f = Math.floor(((now - t0) / 1000) * FPS) % FRAMES;
      if (f === drawn) return;
      drawn = f;
      const s = screen(cv.width);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, cv.width, cv.height);
      ctx.fillStyle = "#000";
      ctx.fillRect(s.x, s.y, s.w, s.h);
      ctx.setTransform(s.w / W, 0, 0, s.h / H, s.x, s.y);
      ctx.fillStyle = "#fff";
      ctx.fill(paths[f], "evenodd");
      return;
    }
    // (the morph: the two fields mixed, at a blur that grows then shrinks, cut at the middle grey;
    // its last two fifths the cut softening into the page's own grey edges, then the page itself;
    // the sheet around the screen turning the page's paper over its first half)
    const { lw, lh, a, b, m, lo, img, paper, ink, sheet } = morph, t = Math.min(1, (now - morph.at) / MORPH), e = ease(Math.min(1, Math.max(0, (t - 0.08) / 0.62)));
    const L = Math.sin(Math.PI * t) ** 0.8 * (SIGMAS.length - 1), l0 = Math.floor(L), l1 = Math.min(SIGMAS.length - 1, l0 + 1), u = L - l0;
    const A0 = a(l0), A1 = a(l1), B0 = b(l0), B1 = b(l1), M0 = m(l0), M1 = m(l1), px = img.data;
    const soft = ease(Math.min(1, Math.max(0, (t - 0.6) / 0.4))), dusk = 1 - ease(Math.min(1, t / 0.5));
    for (let i = 0; i < lw * lh; i++) {
      const v = (1 - e) * (A0[i] + (A1[i] - A0[i]) * u) + e * (B0[i] + (B1[i] - B0[i]) * u);
      const c = Math.min(1, Math.max(0, (v - 0.42) / 0.16)), k = c * c * (3 - 2 * c), q = k + (Math.min(1, v) - k) * soft;
      const w = dusk * (M0[i] + (M1[i] - M0[i]) * u) * (1 - q);
      for (let j = 0; j < 3; j++) px[4 * i + j] = paper[j] + (ink[j] - paper[j]) * q + (sheet[j] - paper[j]) * w;
      px[4 * i + 3] = 255;
    }
    lo.putImageData(img, 0, 0);
    el.style.opacity = String(1 - Math.min(1, Math.max(0, (now - morph.at - MORPH) / SETTLE)));
  });
  const step = (what: string, _pct: number | undefined, detail: string) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
  };
  const hint = (text: string) => keyed($(".hint"), text, "Did you know? ");
  return ending(
    el,
    (page) => {
      el.classList.add("out");
      begin(page);
      return MORPH + SETTLE + 40;
    },
    stop,
    { step, hint },
  );
}
