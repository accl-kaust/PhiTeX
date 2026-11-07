// Bad Apple!!: the shadow-art PV played on the sheet, drawn as curves (its
// frames traced to Béziers, badapple-data.ts), sharp at any size, looping
// its first minute. The build's steps are its subtitles, under the picture
// as a PV's lyrics are, "Did you know" smaller under them; the credit at
// the foot. Page in: the picture opens out to the whole sheet, its white is
// the paper (the page under it), and its black melts away from every edge,
// the silhouettes eroding to nothing, until the page alone is left.

import { html } from "../html.ts";
import { layer, ending, loop, PAPER, type Opening } from "./kit.ts";
import { FRAMES, W, H, FPS, DATA } from "./badapple-data.ts";

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

export function badapple(box: HTMLElement, dark: boolean): Opening {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ba",
    `.phx-ba { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; transition: background .3s ease .25s; }
.phx-ba .scr { position: absolute; left: 8cqw; right: 8cqw; top: 16cqw; aspect-ratio: 4 / 3; background: #fff; box-shadow: 0 0 0 .3cqw #000, 0 1.4cqw 3cqw rgb(0 0 0 / 22%); overflow: hidden; transition: all .38s cubic-bezier(.6,0,.3,1); }
.phx-ba canvas { width: 100%; height: 100%; display: block; }
.phx-ba .sub { position: absolute; left: 8%; right: 8%; top: 86cqw; text-align: center; color: ${ink0}; transition: opacity .2s; }
.phx-ba .sub .what { font-size: 3.6cqw; letter-spacing: .06em; }
.phx-ba .sub .det { font-size: 2.4cqw; color: ${mute}; margin-top: .8cqw; min-height: 1.4em; }
.phx-ba .sub .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 3.6cqw; line-height: 1.45; font-style: italic; }
.phx-ba .cr { position: absolute; left: 0; right: 0; bottom: 3cqw; text-align: center; font-size: 1.7cqw; color: ${mute}; opacity: .8; transition: opacity .2s; }
.phx-ba.out { background: transparent; }
.phx-ba.out .sub, .phx-ba.out .cr { opacity: 0; }
.phx-ba.out .scr { left: 0; right: 0; top: 0; aspect-ratio: auto; height: 100%; box-shadow: none; background: transparent; }`,
    html`<div class="scr"><canvas></canvas></div><div class="sub"><div class="what"></div><div class="det"></div><div class="hint"></div></div>
     <div class="cr">Bad Apple!! · Alstroemeria Records feat. nomico · PV: Anira · original: ZUN</div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const cv = el.querySelector("canvas")!, ctx = cv.getContext("2d")!, dpr = devicePixelRatio || 1;
  let paths: Path2D[] | null = null;
  void read().then((p) => (paths = p));
  const t0 = performance.now();
  let out = -1;
  const MELT = 560;
  const stop = loop(() => {
    const r = cv.getBoundingClientRect();
    if (cv.width !== Math.round(r.width * dpr)) (cv.width = Math.round(r.width * dpr)), (cv.height = Math.round(r.height * dpr));
    if (!paths) return;
    // (the PV's frame now: 15 a second from the start, the first minute again and again)
    const f = Math.floor(((performance.now() - t0) / 1000) * FPS) % FRAMES;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.setTransform(cv.width / W, 0, 0, cv.height / H, 0, 0);
    if (out < 0) {
      ctx.fillStyle = "#fff";
      ctx.fill(paths[f], "evenodd");
      return;
    }
    // (page in: the white cut out, the paper under it; the black eroded from its edges, the frame's too)
    const k = Math.max(0, (performance.now() - out - 160) / MELT), w = Math.min(1, k) ** 1.6 * 70;
    ctx.globalCompositeOperation = "destination-out";
    ctx.fill(paths[f], "evenodd");
    if (w > 0) {
      ctx.lineWidth = w;
      ctx.lineJoin = "round";
      ctx.stroke(paths[f]);
      ctx.strokeRect(0, 0, W, H);
    }
  });
  const step = (what: string, _pct: number | undefined, detail: string) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
  };
  const hint = (text: string) => ($(".hint").textContent = `Did you know? ${text}`);
  return ending(
    el,
    () => {
      out = performance.now();
      el.classList.add("out");
      return 160 + MELT + 60;
    },
    stop,
    { step, hint },
  );
}
