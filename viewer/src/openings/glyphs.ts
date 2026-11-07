// Glyph rain: TeX's glyphs fall down the sheet at three depths, the near ones
// larger, darker and faster. The build is said in the middle, set in the
// paper's own serif (the step, its detail, "Did you know" in italic), and each
// package fetched falls as its name onto a shelf at the bottom (Shelf, where
// packages come from), stacking there. Page in: each glyph drops into a line
// of the page, the lines of glyphs settling where the text will be, the words
// fade with them.

import { html } from "../html.ts";
import { layer, loop, ending, PAPER, INK, RIBBON, type Opening } from "./kit.ts";

const GLYPHS = [..."∑∫∂∇πφΦαβγλμσθ√∞≈≠≤≥§¶{}[]\\$&^_#%ﬁﬂ"];

export function glyphs(box: HTMLElement, dark: boolean): Opening {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-gl",
    `.phx-gl { background: ${dark ? PAPER.dark : PAPER.light}; transition: background .45s ease .2s; } .phx-gl canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; } .phx-gl.out { background: transparent; }
.phx-gl .cap { position: absolute; left: 8%; right: 8%; top: 34%; text-align: center; font-family: "CMU Serif", Georgia, serif; color: ${ink0}; transition: opacity .25s; }
.phx-gl .cap .what { font-size: 5.2cqw; text-shadow: 0 0 1.2cqw ${dark ? PAPER.dark : "#fff"}, 0 0 .4cqw ${dark ? PAPER.dark : "#fff"}; }
.phx-gl .cap .det { font-size: 2.7cqw; color: ${mute}; margin-top: 1cqw; min-height: 1.3em; }
.phx-gl .cap .hint { font-size: 2.6cqw; font-style: italic; color: ${mute}; margin-top: 5cqw; line-height: 1.4; }
.phx-gl .shelf { position: absolute; left: 8%; right: 8%; bottom: 9%; height: 3.8cqw; border-bottom: .5cqw solid ${dark ? "#4a4e57" : "#c9ced8"}; padding-bottom: 1.4cqw; box-sizing: content-box; transition: opacity .25s; }
.phx-gl .shelf span { position: absolute; bottom: 1.4cqw; white-space: nowrap; font: 2.4cqw/1.4 "DM Mono", ui-monospace, monospace; padding: .2cqw 1.2cqw; border-radius: .6cqw; color: #fff; transition: left .32s cubic-bezier(.3,.7,.3,1) .2s; }
.phx-gl .shelf span.in { animation: phx-gl-land .52s cubic-bezier(.5,0,.8,.45) both, phx-gl-bounce .32s cubic-bezier(.3,1.8,.5,1) .52s; }
.phx-gl .shelf span.gone { animation: phx-gl-drop 1s cubic-bezier(.5,0,1,.6) .2s forwards; }
.phx-gl .shelf .more { left: 0; color: ${mute}; padding-left: 0; }
@keyframes phx-gl-land { from { transform: translateY(-110cqw) rotate(var(--r)); } to { transform: none; } }
@keyframes phx-gl-bounce { 0% { transform: scaleY(.7) translateY(.6cqw); } 100% { transform: none; } }
@keyframes phx-gl-drop { 0% { transform: none; } 14% { transform: translate(-.6cqw, -.8cqw) rotate(-10deg); } 85% { opacity: 1; } to { transform: translate(-4cqw, 24cqw) rotate(-40deg); opacity: 0; } }
.phx-gl.out .cap, .phx-gl.out .shelf { opacity: 0; }`,
    html`<canvas></canvas><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="shelf"></div>`,
  );
  const cv = el.querySelector("canvas")!, ctx = cv.getContext("2d")!, dpr = devicePixelRatio || 1;
  const W = (cv.width = box.clientWidth * dpr), H = (cv.height = box.clientHeight * dpr);
  const ink = dark ? "#c9d4ea" : INK.light;
  type G = { x: number; y: number; z: number; v: number; ch: string; rot: number; spin: number; c: string; tx?: number; ty?: number };
  const make = (y = Math.random() * H): G => {
    const z = [0.45, 0.7, 1][Math.floor(Math.random() * 3)];
    return { x: Math.random() * W, y, z, v: (0.04 + Math.random() * 0.03) * z * H, ch: GLYPHS[Math.floor(Math.random() * GLYPHS.length)], rot: Math.random() * 6, spin: (Math.random() - 0.5) * 1.5, c: Math.random() < 0.12 ? RIBBON[Math.floor(Math.random() * 4)] : ink };
  };
  const gs = Array.from({ length: 60 }, () => make());
  let out = -1;
  // (the words' places, in the canvas's pixels, a margin round each: glyphs passing them fade, so the words stay clear)
  let clear: DOMRect[] = [], clearAt = -1e9;
  const places = () => {
    const b = box.getBoundingClientRect(), m = 2.4 * (b.width / 100);
    clear = [...el.querySelectorAll<HTMLElement>(".cap > div, .shelf")]
      .map((e) => e.getBoundingClientRect())
      .filter((r) => r.height > 0 && r.width > 0)
      .map((r) => {
        // (a line's text, not its whole box: the caption's lines are as wide as the sheet)
        return new DOMRect((r.left - b.left - m) * dpr, (r.top - b.top - m) * dpr, (r.width + 2 * m) * dpr, (r.height + 2 * m) * dpr);
      });
  };
  const fade = (x: number, y: number) => {
    let a = 1;
    for (const r of clear) {
      const dx = Math.max(r.left - x, 0, x - r.right), dy = Math.max(r.top - y, 0, y - r.bottom), d = Math.hypot(dx, dy);
      a = Math.min(a, d / (14 * dpr));
    }
    return Math.max(0, Math.min(1, a));
  };
  const stopLoop = loop((t, dt) => {
    ctx.clearRect(0, 0, W, H);
    if (t - clearAt > 250) (places(), (clearAt = t));
    const s = dt / 1000;
    for (const g of gs) {
      let alpha = 0.25 + 0.6 * g.z, rot = g.rot;
      if (out < 0) {
        g.y += g.v * s;
        g.rot += g.spin * s;
        if (g.y > H + 30 * dpr) Object.assign(g, make(-30 * dpr));
      } else {
        // (into the lines: each glyph to a place on one, upright, small as the text)
        const k = Math.min(1, (t - out) / 380), e = 1 - (1 - k) ** 3;
        g.x += ((g.tx ?? g.x) - g.x) * e * 0.35;
        g.y += ((g.ty ?? g.y) - g.y) * e * 0.35;
        rot = g.rot * (1 - e);
        alpha *= 1 - Math.max(0, (t - out - 200) / 300);
      }
      ctx.save();
      ctx.globalAlpha = Math.max(0, alpha * (out < 0 ? fade(g.x, g.y) : 1));
      ctx.translate(g.x, g.y);
      ctx.rotate(rot);
      const px = (out < 0 ? 40 * g.z : 40 * g.z * (1 - 0.55 * Math.min(1, (t - out) / 380))) * (W / 520);
      ctx.font = `${px}px "CMU Serif", Georgia, serif`;
      ctx.fillStyle = g.c;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(g.ch, 0, 0);
      ctx.restore();
    }
  });
  const t0 = performance.now();
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const seen = new Set<string>();
  const step = (what: string, pct: number | undefined, detail: string) => {
    $(".what").textContent = what;
    $(".det").textContent = pct !== undefined ? detail.split(" · ").slice(1).join(" · ") : detail;
    const pkg = pct !== undefined ? detail.split(" · ")[0] : "";
    if (pkg && !seen.has(pkg)) seen.add(pkg), (pending = pkg);
  };
  // (a name lands at most every 0.4 s: packages come faster than that, the ones between
  // counted. One row: the new name falls into its right end, the row slides left as it
  // lands, and those pushed past the left end (the count's) tip off and fall off the sheet)
  let pending: string | null = null, shown = 0;
  const row: HTMLElement[] = [];
  const shelf = $(".shelf");
  const more = document.createElement("span");
  more.className = "more";
  shelf.append(more);
  const place = () => {
    const W = shelf.clientWidth, gap = box.clientWidth * 0.014, from = (more.textContent ? more.offsetWidth : 0) + gap;
    let x = W;
    for (let i = row.length - 1; i >= 0; i--) {
      x -= row[i].offsetWidth;
      if (x < from) {
        for (const e of row.splice(0, i + 1)) e.classList.add("gone"), setTimeout(() => e.remove(), 1300);
        break;
      }
      row[i].style.left = `${x}px`;
      x -= gap;
    }
  };
  const land = () => {
    if (!pending) return;
    const pkg = pending;
    pending = null;
    shown++;
    const w = document.createElement("span");
    w.textContent = pkg;
    w.style.background = [INK.light, ...RIBBON][shown % 5];
    w.style.setProperty("--r", `${(Math.random() - 0.5) * 40}deg`);
    // (its place at once, the others' after a moment: it falls into the room they make)
    w.style.transition = "none";
    shelf.append(w);
    w.style.left = `${shelf.clientWidth - w.offsetWidth}px`;
    w.classList.add("in");
    row.push(w);
    place();
    more.textContent = seen.size > row.length ? `+${seen.size - row.length}` : "";
    requestAnimationFrame(() => (w.style.transition = ""));
  };
  const lander = window.setInterval(land, 400);
  const hint = (text: string) => ($(".hint").textContent = `Did you know? ${text}`);
  return ending(el, () => {
    out = performance.now() - t0;
    // (the page's text block: lines from a fifth of the way down, a line's height apart)
    const lh = W * 0.0162 * 1.25 * 1.6;
    gs.forEach((g) => ((g.tx = W * (0.115 + 0.77 * Math.random())), (g.ty = H * 0.2 + Math.round((Math.random() * H * 0.65) / lh) * lh)));
    el.classList.add("out");
    return 620;
  }, () => (stopLoop(), clearInterval(lander)), { step, hint });
}
