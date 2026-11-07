// What the openings share (the loading animations but PhiTeX's own, opening.ts):
// a layer on the first page's sheet, its own styles, and an ending. The page
// is under the sheet when the opening ends (the sheet's white is gone, .in),
// so an exit reveals it: the layer goes, the page is there. Sizes are in the
// sheet's width (cqw: the sheet is a container), so it looks the same at any
// zoom. Each layer carries its styles (the panel may be in a shadow root,
// where the document's are not seen).

import type { Opening } from "../opening.ts";

export type { Opening };

export const INK = { light: "#1d3e72", dark: "#e4e6ea" };
export const PAPER = { light: "#ffffff", dark: "#2c2e33" };
/** Text for markup. */
export const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
/** The logo's ribbon (ACCL's stripes). */
export const RIBBON = ["#00a8a4", "#d3c72a", "#f0b225", "#f58f2d"];
export const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

let registered = false;
/** Once: the reveal's radius as an animatable length (a hole growing in the layer's mask; @property is not read in a shadow root, registerProperty is the page's). */
function register() {
  if (registered) return;
  registered = true;
  try {
    CSS.registerProperty({ name: "--phx-o-r", syntax: "<percentage>", inherits: false, initialValue: "0%" });
  } catch {
    /* (registered already, by another panel in this page) */
  }
}

/** The layer's own styles, and the reveals' (hole, wipe). */
const SHARED = `
.phx-o { position: absolute; inset: 0; overflow: hidden; }
.phx-o.hole { --phx-o-x: 50%; --phx-o-y: 50%;
  -webkit-mask: radial-gradient(circle at var(--phx-o-x) var(--phx-o-y), transparent var(--phx-o-r), #000 calc(var(--phx-o-r) + 0.5%));
          mask: radial-gradient(circle at var(--phx-o-x) var(--phx-o-y), transparent var(--phx-o-r), #000 calc(var(--phx-o-r) + 0.5%)); }
.phx-o.hole.open { --phx-o-r: 150%; transition: --phx-o-r var(--phx-o-t, 600ms) cubic-bezier(.55, 0, .3, 1); }
.phx-o.wipe { -webkit-mask: linear-gradient(var(--phx-ongle, 115deg), transparent calc(var(--phx-o-r) - 12%), #000 var(--phx-o-r));
                      mask: linear-gradient(var(--phx-ongle, 115deg), transparent calc(var(--phx-o-r) - 12%), #000 var(--phx-o-r)); }
.phx-o.wipe.open { --phx-o-r: 115%; transition: --phx-o-r var(--phx-o-t, 600ms) cubic-bezier(.6, 0, .25, 1); }`;

/**
 * The layer: `html` in a div of class `cls` over the page in `box`, with
 * `css` (scope it under .cls), in a shadow root of its own: the panel's
 * styles never reach its parts (.bar, .menu, .tip are the panel's names too)
 * and its own never leave it. Removed with gone().
 */
export function layer(box: HTMLElement, cls: string, css: string, html: string): HTMLElement {
  register();
  const host = document.createElement("div");
  host.style.cssText = "position: absolute; inset: 0; pointer-events: none;";
  host.setAttribute("aria-hidden", "true");
  const root = host.attachShadow({ mode: "open" });
  root.innerHTML = `<style>${SHARED}${css}</style><div class="phx-o ${cls}">${html}</div>`;
  box.append(host);
  return root.lastElementChild as HTMLElement;
}

/** The layer and its shadow root's host gone from the sheet. */
export const gone = (el: HTMLElement) => ((el.getRootNode() as ShadowRoot).host ?? el).remove();

/** A hole opening in the layer from (x, y) (percent of the sheet) over `ms`: the page shows through it. */
export function hole(el: HTMLElement, x = 50, y = 50, ms = 600) {
  el.style.setProperty("--phx-o-x", `${x}%`);
  el.style.setProperty("--phx-o-y", `${y}%`);
  el.style.setProperty("--phx-o-t", `${ms}ms`);
  el.classList.add("hole");
  void el.offsetWidth;
  el.classList.add("open");
}

/** A soft diagonal edge sweeping across the layer at `angle` over `ms`, the page behind it. */
export function wipe(el: HTMLElement, angle = 115, ms = 600) {
  el.style.setProperty("--phx-ongle", `${angle}deg`);
  el.style.setProperty("--phx-o-t", `${ms}ms`);
  el.classList.add("wipe");
  void el.offsetWidth;
  el.classList.add("open");
}

/** A frame loop until the layer goes: frame(t, dt) in ms. */
export function loop(frame: (t: number, dt: number) => void) {
  const t0 = performance.now();
  let raf = 0, last = t0, on = true;
  const tick = (now: number) => {
    if (!on) return;
    frame(now - t0, Math.min(50, now - last));
    last = now;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return () => ((on = false), cancelAnimationFrame(raf));
}

/**
 * The Opening for a layer: done() runs `exit` (it starts the layer's way out
 * and says how long it takes, in ms), then the layer is removed; stop() at once.
 * `cleanup` runs either way (timers, frame loops).
 */
export function ending(el: HTMLElement, exit: () => number, cleanup: () => void = () => {}, says?: Pick<Opening, "step" | "hint">): Opening {
  let over = false, ending: Promise<void> | null = null;
  const stop = () => {
    if (over) return;
    over = true;
    cleanup();
    gone(el);
  };
  return {
    ...says,
    done: () =>
      (ending ??= new Promise((ok) => {
        if (over) return ok();
        const ms = reduced() ? 0 : exit();
        setTimeout(() => (stop(), ok()), ms);
      })),
    stop,
  };
}

/**
 * An old monitor, for a layer whose screen is `.scr` (cls: the layer's class):
 * a plastic bezel around a rounded screen, scanlines, the glass's vignette and
 * glare, a slow rolling band, flicker, glowing text with a touch of colour
 * fringing, and the tube warming up at the start. Its switching off is crtOff.
 */
export function crt(cls: string, screen: string, glow = "currentColor"): string {
  const c = `.${cls}`;
  return `${c} { background: linear-gradient(160deg, #3a3b3f, #1c1d20 60%, #151618); }
${c} .scr { position: absolute; inset: 3.4cqw 3.4cqw 5.6cqw; border-radius: 4cqw / 5cqw; background: ${screen}; overflow: hidden;
  box-shadow: 0 0 0 .5cqw #0b0b0c, 0 .3cqw .6cqw .5cqw rgb(255 255 255 / 6%); transform-origin: 50% 50%;
  animation: phx-crt-on .5s cubic-bezier(.2,.8,.3,1) both, phx-crt-flicker 3.2s steps(1) .5s infinite; }
${c} .scr .txt { text-shadow: -.12cqw 0 rgb(255 40 90 / 35%), .12cqw 0 rgb(40 170 255 / 35%), 0 0 .7cqw ${glow}; }
${c} .scr::before { content: ""; position: absolute; inset: 0; z-index: 3; pointer-events: none;
  background: repeating-linear-gradient(rgb(0 0 0 / 0) 0 .25cqw, rgb(0 0 0 / 32%) .25cqw .5cqw); mix-blend-mode: multiply; }
${c} .scr::after { content: ""; position: absolute; inset: 0; z-index: 4; pointer-events: none; border-radius: inherit;
  background: radial-gradient(120% 90% at 30% 15%, rgb(255 255 255 / 9%), transparent 40%), radial-gradient(ellipse at center, transparent 55%, rgb(0 0 0 / 55%) 100%);
  box-shadow: inset 0 0 5cqw rgb(0 0 0 / 70%); }
${c} .band { position: absolute; left: 0; right: 0; height: 18%; z-index: 2; pointer-events: none; background: linear-gradient(transparent, rgb(255 255 255 / 4%), transparent); animation: phx-crt-band 7s linear infinite; }
${c} .led { position: absolute; right: 6cqw; bottom: 2.2cqw; width: 1.2cqw; height: 1.2cqw; border-radius: 50%; background: #5dff8a; box-shadow: 0 0 1cqw #5dff8a; }
${c} .brand { position: absolute; left: 0; right: 0; bottom: 1.7cqw; text-align: center; font: 600 1.6cqw/1 system-ui, sans-serif; letter-spacing: .3em; color: #6b6d72; }
@keyframes phx-crt-on { 0% { transform: scale(1, .004); filter: brightness(4); } 40% { transform: scale(1, .004); filter: brightness(4); } 100% { transform: scale(1, 1); filter: none; } }
@keyframes phx-crt-flicker { 0% { opacity: 1; } 31% { opacity: .96; } 32% { opacity: 1; } 67% { opacity: .98; } 68% { opacity: 1; } }
@keyframes phx-crt-band { from { top: -20%; } to { top: 110%; } }
${c}.off { background: transparent; transition: background .35s ease .1s; }
${c}.off .led, ${c}.off .brand { opacity: 0; transition: opacity .2s; }
${c}.off .scr { box-shadow: none; animation: phx-crt-off .5s cubic-bezier(.7,0,.6,1) forwards; }
${c}.off .scr > * { animation: phx-crt-white .5s forwards; }
@keyframes phx-crt-off { 0% { transform: scale(1, 1); filter: brightness(1); }
  45% { transform: scale(1, .006); filter: brightness(5); border-radius: 0; }
  75% { transform: scale(.02, .006); filter: brightness(6); opacity: 1; }
  100% { transform: scale(0, 0); filter: brightness(6); opacity: 0; } }
@keyframes phx-crt-white { 30%, 100% { background: #fff; color: #fff; } }`;
}

/** The monitor switched off (crt): the picture folds to a bright line, a point, and is gone; the bezel fades off the page. */
export function crtOff(el: HTMLElement): number {
  el.classList.add("off");
  return 520;
}
