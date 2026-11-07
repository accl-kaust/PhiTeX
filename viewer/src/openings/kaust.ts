// ACCL | KAUST: the byline's landing as a whole opening, ACCL's stripes
// shooting in, the lockup, the rule, KAUST's seeds popping up. Under it ACCL's
// four stripes are the progress: one each for reading the project, fetching
// packages (filling as they come), typesetting (running while the page is
// set) and the page itself; the step, its detail and "Did you know" under
// them. Page in: the fourth stripe fills, and the four great stripes behind
// the logo sweep across the sheet one after another, the page coming in
// behind the last.

import { html, svgArt } from "../html.ts";
import { layer, ending, wipe, RIBBON, PAPER, type Opening } from "./kit.ts";
import { LOCKUP, SEEDS } from "../opening-art.ts";

/** A logo's <svg> (the build's art), of class `cls`, hidden from readers. */
const art = (markup: string, cls: string) => {
  const s = svgArt(markup).firstElementChild!;
  s.setAttribute("class", cls);
  s.setAttribute("aria-hidden", "true");
  return s;
};

const STEPS = ["Reading the project", "Fetching packages", "Typesetting"];

export function kaust(box: HTMLElement, dark: boolean): Opening {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ka",
    `.phx-ka { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; color: ${ink0}; }
.phx-ka .row { position: absolute; left: 50%; top: 36%; transform: translate(-50%, -50%); display: flex; align-items: center; gap: 4cqw; transition: opacity .2s; }
.phx-ka .lk { height: 9cqw; width: auto; animation: phx-ka-in .9s cubic-bezier(.2,.8,.2,1) .35s both; }
.phx-ka .rule { width: .5cqw; height: 9cqw; background: #8d96a5; animation: phx-ka-rule .3s ease-out .9s both; }
.phx-ka .sd { height: 10cqw; width: auto; animation: phx-ka-pop .55s cubic-bezier(.3,1.8,.5,1) 1.05s both; }
@keyframes phx-ka-in { from { opacity: 0; transform: translateX(-6cqw); } }
@keyframes phx-ka-rule { from { transform: scaleY(0); } }
@keyframes phx-ka-pop { from { opacity: 0; transform: translateY(3cqw) scale(.2); } }
.phx-ka .st { position: absolute; left: -40%; width: 180%; height: 2.2cqw; transform: rotate(-50.5deg); animation: phx-ka-shoot 1s cubic-bezier(.2,.9,.3,1.05) both, phx-ka-breathe 2.4s ease-in-out 1.2s infinite; opacity: .14; }
.phx-ka .st:nth-of-type(1) { top: 26%; background: ${RIBBON[0]}; animation-delay: 0s, 1.2s; }
.phx-ka .st:nth-of-type(2) { top: 30%; background: ${RIBBON[1]}; animation-delay: .07s, 1.4s; }
.phx-ka .st:nth-of-type(3) { top: 34%; background: ${RIBBON[2]}; animation-delay: .14s, 1.6s; }
.phx-ka .st:nth-of-type(4) { top: 38%; background: ${RIBBON[3]}; animation-delay: .21s, 1.8s; }
@keyframes phx-ka-shoot { from { transform: rotate(-50.5deg) translateX(-120%); } }
@keyframes phx-ka-breathe { 50% { opacity: .26; } }
.phx-ka .prog { position: absolute; left: 50%; top: 52%; transform: translateX(-50%); display: flex; gap: 1.6cqw; transition: opacity .2s; }
.phx-ka .prog i { position: relative; width: 11cqw; height: 1.8cqw; transform: skewX(-38deg); background: ${dark ? "#3d4047" : "#e7e9ee"}; overflow: hidden; }
.phx-ka .prog i b { position: absolute; inset: 0 auto 0 0; width: 0; background: var(--c); transition: width .35s cubic-bezier(.3,.7,.3,1); }
.phx-ka .prog i.run b { width: 100%; background: linear-gradient(90deg, var(--c) 40%, color-mix(in srgb, var(--c), #fff 55%) 50%, var(--c) 60%) 0 0 / 300% 100%; animation: phx-ka-run 1.2s linear infinite; }
@keyframes phx-ka-run { from { background-position: 100% 0; } to { background-position: -50% 0; } }
.phx-ka .cap { position: absolute; left: 10%; right: 10%; top: 58%; text-align: center; transition: opacity .2s; }
.phx-ka .cap .what { font-size: 3.2cqw; font-weight: 600; }
.phx-ka .cap .det { font-size: 2.4cqw; color: ${mute}; margin-top: .6cqw; min-height: 1.4em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-ka .cap .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 4cqw; line-height: 1.45; }
.phx-ka.out .row, .phx-ka.out .prog, .phx-ka.out .cap { opacity: 0; }
.phx-ka.sweep .st { animation: none; opacity: 1; height: 16cqw; transition: transform .62s cubic-bezier(.55,0,.35,1); }
.phx-ka.sweep .st:nth-of-type(2) { transition-delay: .05s; } .phx-ka.sweep .st:nth-of-type(3) { transition-delay: .1s; } .phx-ka.sweep .st:nth-of-type(4) { transition-delay: .15s; }
.phx-ka.sweep.go .st { transform: rotate(-50.5deg) translateY(150cqw); }`,
    html`<div class="st"></div><div class="st"></div><div class="st"></div><div class="st"></div>
     <div class="row">${art(LOCKUP.replace(/#1D3E72/gi, dark ? "#e4e6ea" : "#1D3E72"), "lk")}<div class="rule"></div>${art(SEEDS, "sd")}</div>
     <div class="prog">${RIBBON.map((c) => html`<i style="--c:${c}"><b></b></i>`)}</div>
     <div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const bars = [...el.querySelectorAll<HTMLElement>(".prog i")];
  const fill = (i: number, w: number | "run") => {
    bars[i].classList.toggle("run", w === "run");
    bars[i].querySelector("b")!.style.width = w === "run" ? "" : `${w}%`;
  };
  const step = (what: string, pct: number | undefined, detail: string) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
    const k = Math.max(0, STEPS.indexOf(what));
    // (the stripes before this step full, this one filling or running)
    for (let i = 0; i < k; i++) fill(i, 100);
    fill(k, pct !== undefined ? pct : "run");
  };
  const hint = (text: string) => ($(".hint").textContent = `Did you know? ${text}`);
  return ending(
    el,
    () => {
      bars.forEach((_, i) => fill(i, 100));
      setTimeout(() => {
        el.classList.add("out", "sweep");
        // (the stripes taken to the sheet's top left, then across it, the page behind the last)
        el.querySelectorAll<HTMLElement>(".st").forEach((s) => (s.style.transition = "none", (s.style.transform = "rotate(-50.5deg) translateY(-70cqw)")));
        void el.offsetWidth;
        el.querySelectorAll<HTMLElement>(".st").forEach((s) => ((s.style.transition = ""), (s.style.transform = "")));
        el.classList.add("go");
        setTimeout(() => wipe(el, 129.5, 560), 180);
      }, 200);
      return 1000;
    },
    undefined,
    { step, hint },
  );
}
