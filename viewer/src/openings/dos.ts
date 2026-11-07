// MS-DOS: a text-mode setup screen on an old monitor, blue with a grey title
// line, the build's own steps in its double-lined box: the step, a bar of
// block characters (a block sliding back and forth while how far is not
// known), and the file; "Did you know" in a single-lined box under it. Page
// in: the bar fills, then the screen clears a text row at a time from the
// top, the page under each row, and the monitor's bezel fades.

import { html } from "../html.ts";
import { layer, ending, crt, type Opening } from "./kit.ts";

const COLS = 26;

export function dos(box: HTMLElement): Opening {
  const el = layer(
    box,
    "phx-dos",
    `${crt("phx-dos", "#0000a8", "rgb(170 190 255 / 55%)")}
.phx-dos .txt { position: absolute; inset: 0; color: #a8a8a8; font: 2.75cqw/1.6 "DM Mono", ui-monospace, monospace; }
.phx-dos .top { background: #a8a8a8; color: #000; padding: 1.5cqw 4cqw 0; text-shadow: none; }
.phx-dos .main { padding: 4cqw 5cqw; }
.phx-dos .main p { margin: 0 0 2cqw; color: #fff; }
.phx-dos .box { border: .8cqw double #fff; padding: 2cqw 3cqw; margin: 4cqw 0 3cqw; }
.phx-dos .now { color: #55ffff; }
.phx-dos .bar { color: #ffff55; white-space: pre; }
.phx-dos .pct { text-align: center; color: #fff; }
.phx-dos .file { color: #fff; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-dos .tip { border: .4cqw solid #a8a8a8; padding: 1.5cqw 3cqw; margin-top: 3cqw; color: #ffff55; visibility: hidden; }
.phx-dos .tip b { color: #fff; font-weight: normal; }
.phx-dos .bot { position: absolute; left: 0; right: 0; bottom: 0; background: #a8a8a8; color: #000; padding: 0 4cqw 1.2cqw; white-space: pre; text-shadow: none; }
.phx-dos.clear { background: transparent; transition: background .3s; }
.phx-dos.clear .led, .phx-dos.clear .brand { opacity: 0; transition: opacity .2s; }
.phx-dos.clear .scr { box-shadow: none; animation: phx-dos-clear .5s steps(14, end) forwards; }
@keyframes phx-dos-clear { from { clip-path: inset(0 0 0 0); } to { clip-path: inset(100% 0 0 0); } }`,
    html`<div class="scr"><div class="txt"><div class="top">PhiTeX Setup</div>
     <div class="main"><p>Please wait while Setup typesets your document.</p>
     <div class="box"><div class="now">Starting…</div><div class="bar"></div><div class="pct">&nbsp;</div><div class="file">&nbsp;</div></div>
     <div class="tip"><b>Did you know?</b> <span></span></div></div>
     <div class="bot">  ENTER=Continue   F1=Help   F3=Exit</div></div><div class="band"></div></div>
     <div class="led"></div><div class="brand">PHITEX</div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const now = $(".now"), bar = $(".bar"), pct = $(".pct"), file = $(".file"), tip = $(".tip");
  let known: number | undefined, pos = 0;
  const draw = () => {
    if (known !== undefined) {
      const n = Math.round((known / 100) * COLS);
      bar.textContent = "█".repeat(n) + "░".repeat(COLS - n);
      pct.textContent = `${Math.round(known)}%`;
    } else {
      // (not known: a block of four sliding to and fro)
      const p = Math.abs((pos++ % ((COLS - 4) * 2)) - (COLS - 4));
      bar.textContent = "░".repeat(p) + "▓▓▓▓" + "░".repeat(COLS - 4 - p);
      pct.textContent = " ";
    }
  };
  draw();
  const tick = window.setInterval(() => known === undefined && draw(), 90);
  const step = (what: string, p: number | undefined, detail: string) => {
    now.textContent = what;
    known = p;
    file.textContent = detail ? (p !== undefined ? `Copying: ${detail.split(" · ")[0].toUpperCase()}` : detail) : " ";
    draw();
  };
  const hint = (text: string) => {
    tip.style.visibility = "visible";
    tip.querySelector("span")!.textContent = text;
  };
  return ending(
    el,
    () => {
      clearInterval(tick);
      step("Setup is complete.", 100, "");
      setTimeout(() => el.classList.add("clear"), 220);
      return 740;
    },
    () => clearInterval(tick),
    { step, hint },
  );
}
