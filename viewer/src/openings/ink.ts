// Ink drop: the build told in ink. A drop falls for each package fetched and
// the blot it lands in grows with how far they have come; while the page is
// set, drops keep falling and rings run out. Under it, the step bleeds into
// the paper in an italic hand, its detail and "Did you know" in smaller ink.
// Page in: one last drop floods the sheet, and the page comes up through the
// middle of the ink.

import { html } from "../html.ts";
import { layer, ending, hole, INK, PAPER, type Opening } from "./kit.ts";

/** Where the drops land: the blot's centre, a third of the way down. */
const AT = 34;

export function ink(box: HTMLElement, dark: boolean): Opening {
  const c = dark ? "#9fb7e0" : INK.light;
  const el = layer(
    box,
    "phx-ink",
    `.phx-ink { background: ${dark ? PAPER.dark : PAPER.light}; color: ${c}; }
.phx-ink .drop { position: absolute; left: 50%; top: -6cqw; width: 3.4cqw; height: 4.6cqw; margin-left: -1.7cqw; background: ${c}; border-radius: 50% 50% 50% 50% / 60% 60% 40% 40%; animation: phx-ink-fall .55s cubic-bezier(.5,0,.9,.6) forwards; }
@keyframes phx-ink-fall { 0% { transform: translateY(0); } 97% { opacity: 1; } 100% { transform: translateY(${AT * 1.294 + 4}cqw); opacity: 0; } }
.phx-ink .blot { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; background: ${c}; transform: scale(var(--s, .08)); transition: transform .5s cubic-bezier(.3,1.6,.5,1); animation: phx-ink-wob 2.6s ease-in-out infinite; }
@keyframes phx-ink-wob { 0%, 100% { border-radius: 52% 48% 50% 50% / 50% 52% 48% 50%; } 33% { border-radius: 58% 42% 55% 45% / 45% 58% 42% 55%; } 66% { border-radius: 44% 56% 47% 53% / 56% 44% 58% 42%; } }
.phx-ink .ring { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; border: .35cqw solid ${c}; border-radius: 50%; animation: phx-ink-ring 1.1s ease-out forwards; }
@keyframes phx-ink-ring { 0% { transform: scale(calc(var(--s, .1) * .9)); opacity: .55; } 100% { transform: scale(calc(var(--s, .1) * 2.2 + .4)); opacity: 0; } }
.phx-ink .cap { position: absolute; left: 10%; right: 10%; top: ${AT + 18}%; text-align: center; font-family: "CMU Serif", Georgia, serif; }
.phx-ink .what { font-size: 4.4cqw; font-style: italic; }
.phx-ink .det { font-size: 2.6cqw; margin-top: 1cqw; opacity: .75; min-height: 1.3em; }
.phx-ink .hint { font-size: 2.5cqw; font-style: italic; margin-top: 5cqw; line-height: 1.4; opacity: .7; }
.phx-ink .bleed { animation: phx-ink-bleed .5s ease-out; }
@keyframes phx-ink-bleed { from { filter: blur(.7cqw); opacity: 0; letter-spacing: .08em; } }
.phx-ink .flood { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; background: ${c}; border-radius: 50%; transform: scale(0); }
.phx-ink.out .flood { animation: phx-ink-flood .38s cubic-bezier(.5,0,.7,1) forwards; }
.phx-ink.out .cap { opacity: 0; transition: opacity .2s; }
@keyframes phx-ink-flood { to { transform: scale(10); } }`,
    html`<div class="blot"></div><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="flood"></div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const blot = $(".blot");
  // (text that changes bleeds in again)
  const write = (e: HTMLElement, s: string) => {
    if (e.textContent === s) return;
    e.textContent = s;
    e.classList.remove("bleed");
    void e.offsetWidth;
    e.classList.add("bleed");
  };
  const size = (s: number) => (el.style.setProperty("--s", String(s)), blot.style.setProperty("--s", String(s)));
  const drop = () => {
    const d = document.createElement("div");
    d.className = "drop";
    el.insertBefore(d, $(".cap"));
    setTimeout(() => {
      d.remove();
      const r = document.createElement("div");
      r.className = "ring";
      el.insertBefore(r, blot);
      setTimeout(() => r.remove(), 1150);
    }, 540);
  };
  let seen = new Set<string>(), rain = 0, what = "";
  const step = (w: string, pct: number | undefined, detail: string) => {
    write($(".what"), w);
    // (the detail as it is: a package a few times a second, bled in each time, was never sharp)
    $(".det").textContent = pct !== undefined ? detail.split(" · ").slice(0, 2).join(" · ") : detail;
    const pkg = pct !== undefined ? detail.split(" · ")[0] : "";
    if (pkg && !seen.has(pkg)) (seen.add(pkg), drop());
    if (w !== what) {
      what = w;
      clearInterval(rain);
      // (typesetting: drops on their own, a slow rain)
      if (pct === undefined && seen.size) rain = window.setInterval(drop, 1100);
    }
    size(pct !== undefined ? 0.25 + 0.6 * (pct / 100) : seen.size ? 0.9 : 0.12);
  };
  const hint = (text: string) => write($(".hint"), `Did you know? ${text}`);
  return ending(
    el,
    () => {
      clearInterval(rain);
      el.classList.add("out");
      setTimeout(() => hole(el, 50, AT, 480), 330);
      return 840;
    },
    () => clearInterval(rain),
    { step, hint },
  );
}
