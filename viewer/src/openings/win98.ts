// Windows 98, finished: the viewer's setup dialog on the teal desktop with a
// taskbar under it, the build's steps in the dialog (the step, its detail,
// the blocky bar filling as packages come and marching while how far is not
// known) and "Did you know" as a balloon over the tray. Page in: the bar runs
// to the end, "Setup is complete.", the dialog closes, and the PDF opens as
// a program: its title bar flies up from its taskbar button (as Windows
// animates a window opening), the window is there with the page in it, its
// Maximize button is pressed and it fills the sheet, its frame and the
// taskbar going off the edges, the page alone left.

import { html } from "../html.ts";
import { layer, ending, type Opening } from "./kit.ts";
import { keyed } from "../hints.ts";

/** The PDF's window before it is maximized, in percent of the sheet: its page area (the frame and bars around it). */
const RESTORED = { left: 9, top: 15, width: 82, height: 66 };

export function win98(box: HTMLElement, doc = "document.tex"): Opening {
  const pdf = doc.replace(/\.tex$/i, "") + ".pdf";
  const el = layer(
    box,
    "phx-w98",
    `.phx-w98 { background: #008080; font: 3.1cqw/1.3 "MS Sans Serif", Tahoma, "Segoe UI", sans-serif; color: #000; }
.phx-w98 .bev { background: #c0c0c0; border: 2px solid; border-color: #fff #000 #000 #fff; box-shadow: inset -1px -1px #808080, inset 1px 1px #dfdfdf; }
.phx-w98 .w { position: absolute; left: 9%; right: 9%; top: 26%; padding: 3px; }
.phx-w98 .t, .phx-w98 .fly { display: flex; align-items: center; gap: 2px; height: 5cqw; padding: 0 .8cqw; color: #fff; font-weight: bold; background: linear-gradient(90deg, #000080, #1084d0); white-space: nowrap; overflow: hidden; }
.phx-w98 .t span { flex: 1; overflow: hidden; text-overflow: ellipsis; }
.phx-w98 .t i { flex: none; font-style: normal; width: 4.2cqw; height: 3.7cqw; line-height: 3.1cqw; text-align: center; color: #000; background: #c0c0c0; border: 1px solid; border-color: #fff #000 #000 #fff; font-size: 2.8cqw; font-weight: bold; }
.phx-w98 .t i.down { border-color: #000 #fff #fff #000; }
.phx-w98 .b { padding: 3.4cqw 3.4cqw 3cqw; }
.phx-w98 .fl { position: relative; height: 11cqw; margin-bottom: 2.4cqw; }
.phx-w98 .fold { position: absolute; top: 3cqw; width: 9cqw; height: 6.6cqw; background: #e8d36a; border: 1px solid #806000; }
.phx-w98 .fold::before { content: ""; position: absolute; left: -1px; top: -1.8cqw; width: 3.6cqw; height: 1.5cqw; background: #e8d36a; border: 1px solid #806000; border-bottom: none; }
.phx-w98 .f1 { left: 6%; } .phx-w98 .f2 { right: 6%; }
.phx-w98 .pg { position: absolute; top: 1.2cqw; left: 12%; width: 4.2cqw; height: 5.4cqw; background: #fff; border: 1px solid #000; animation: phx-w98-fly 1.6s linear infinite; }
@keyframes phx-w98-fly { 0% { transform: translate(0, 2.4cqw) rotate(-20deg); } 50% { transform: translate(20cqw, -1.8cqw) rotate(0); } 100% { transform: translate(40cqw, 2.4cqw) rotate(20deg); } }
.phx-w98 .msg { margin: 0; font-weight: bold; }
.phx-w98 .det { margin: .6cqw 0 2cqw; min-height: 1.3em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-w98 .bar { height: 4.6cqw; border: 1px solid; border-color: #808080 #fff #fff #808080; padding: 2px; position: relative; overflow: hidden; }
.phx-w98 .bar span { position: absolute; top: 2px; bottom: 2px; left: 2px; width: 0; background: repeating-linear-gradient(90deg, #000080 0 2.2cqw, transparent 2.2cqw 2.8cqw); }
.phx-w98 .bar.march span { width: 30%; animation: phx-w98-march 1.6s steps(10) infinite; }
@keyframes phx-w98-march { from { left: -30%; } to { left: 100%; } }
.phx-w98 .task { position: absolute; left: 0; right: 0; bottom: 0; height: 6.6cqw; background: #c0c0c0; border-top: 2px solid #fff; box-shadow: 0 -1px #dfdfdf; display: flex; align-items: center; gap: 1cqw; padding: 0 .8cqw; transition: transform .3s cubic-bezier(.5,0,.2,1); }
.phx-w98 .start, .phx-w98 .btn { height: 4.9cqw; display: flex; align-items: center; padding: 0 1.4cqw; border: 2px solid; border-color: #fff #000 #000 #fff; white-space: nowrap; overflow: hidden; }
.phx-w98 .start { font-weight: bold; }
.phx-w98 .btn { width: 34%; }
.phx-w98 .btn.on { border-color: #000 #fff #fff #000; background: repeating-conic-gradient(#c0c0c0 0 25%, #fff 0 50%) 0 0 / 2px 2px; font-weight: bold; }
.phx-w98 .clock { margin-left: auto; height: 4.9cqw; display: flex; align-items: center; padding: 0 1.4cqw; border: 1px solid; border-color: #808080 #fff #fff #808080; }
.phx-w98 .tip { position: absolute; right: 2cqw; bottom: 9.4cqw; width: 58%; background: #ffffe1; border: 1px solid #000; border-radius: 1.6cqw; padding: 1.6cqw 2cqw; font-size: 2.6cqw; line-height: 1.35; box-shadow: .4cqw .4cqw 0 rgb(0 0 0 / 25%); visibility: hidden; }
.phx-w98 .tip::after { content: ""; position: absolute; right: 7cqw; bottom: -2.2cqw; border: 1.1cqw solid transparent; border-top: 1.1cqw solid #000; border-right-color: #000; }
.phx-w98 .tip b { display: block; margin-bottom: .6cqw; }
.phx-w98 .tip.on { visibility: visible; animation: phx-w98-tip .2s steps(3); }
@keyframes phx-w98-tip { from { opacity: 0; } }
.phx-w98 .fly { position: absolute; display: none; z-index: 5; }
.phx-w98 .pdfw { position: absolute; display: none; left: ${RESTORED.left}%; top: ${RESTORED.top}%; width: ${RESTORED.width}%; height: ${RESTORED.height}%;
  box-shadow: 0 0 0 1px #808080, 0 0 0 3px #c0c0c0, 0 0 0 4px #dfdfdf, 0 0 0 5px #000, 0 0 0 999px #008080; transition: all .3s cubic-bezier(.5,0,.2,1); }
.phx-w98 .pdfw .chrome { position: absolute; left: -3px; right: -3px; bottom: 100%; background: #c0c0c0; padding: 3px 3px 0; }
.phx-w98 .pdfw .menu { display: flex; gap: 2.6cqw; padding: .5cqw 1.4cqw; font-size: 2.8cqw; border-bottom: 1px solid #808080; }
.phx-w98 .pdfw .menu u { text-decoration: underline; }
.phx-w98.desk { background: transparent; }
.phx-w98.desk .pdfw { display: block; }
.phx-w98.max .pdfw { left: 0; top: 0; width: 100%; height: 100%; }
.phx-w98.max .task { transform: translateY(110%); }`,
    html`<div class="w bev"><div class="t"><span>PhiTeX Setup</span><i>_</i><i>□</i><i>×</i></div><div class="b"><div class="fl"><div class="fold f1"></div><div class="pg"></div><div class="fold f2"></div></div>
       <p class="msg">Starting…</p><p class="det"></p><div class="bar march"><span></span></div></div></div>
     <div class="pdfw"><div class="chrome"><div class="t"><span>${pdf} - PhiTeX</span><i>_</i><i class="mx">□</i><i>×</i></div><div class="menu"><span><u>F</u>ile</span><span><u>E</u>dit</span><span><u>V</u>iew</span><span><u>W</u>indow</span><span><u>H</u>elp</span></div></div></div>
     <div class="task"><div class="start">⊞ Start</div><div class="btn on">PhiTeX Setup</div><div class="clock"></div></div>
     <div class="tip"><b>Did you know?</b><span></span></div>
     <div class="fly">${pdf} - PhiTeX</div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const w = $(".w"), fly = $(".fly"), bar = $(".bar"), fill = $(".bar span"), btn = $(".btn"), tip = $(".tip");
  $(".clock").textContent = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const step = (what: string, pct: number | undefined, detail: string) => {
    $(".msg").textContent = `${what}…`;
    $(".det").textContent = detail;
    bar.classList.toggle("march", pct === undefined);
    // (in blocks, as Windows fills it)
    fill.style.width = pct === undefined ? "" : `calc(${Math.round(pct / 5) * 5}% - 4px)`;
  };
  const hint = (text: string) => {
    keyed(tip.querySelector("span")!, text);
    tip.classList.remove("on");
    void tip.offsetWidth;
    tip.classList.add("on");
  };
  const timers: number[] = [];
  const at = (ms: number, f: () => void) => timers.push(window.setTimeout(f, ms));
  return ending(
    el,
    () => {
      step("Setup is complete", 100, "");
      $(".msg").textContent = "Setup is complete.";
      at(260, () => {
        // (the dialog closed; the PDF's button in its place, its title bar flying up from it)
        w.style.display = "none";
        tip.style.display = "none";
        btn.textContent = `${doc.replace(/\.tex$/i, "")}.pdf - PhiTeX`;
        const b = box.getBoundingClientRect(), r = btn.getBoundingClientRect(), H = b.height, W = b.width;
        const to = { left: (RESTORED.left / 100) * W - 3, top: (RESTORED.top / 100) * H - $(".pdfw .chrome").offsetHeight, width: (RESTORED.width / 100) * W + 6 };
        fly.style.cssText = `display: flex; left: ${r.left - b.left}px; top: ${r.top - b.top}px; width: ${r.width}px;`;
        fly.animate([{}, { left: `${to.left}px`, top: `${to.top}px`, width: `${to.width}px` }], { duration: 230, easing: "ease-out", fill: "forwards" });
      });
      at(500, () => ((fly.style.display = "none"), el.classList.add("desk")));
      at(760, () => $(".mx").classList.add("down"));
      at(840, () => ($(".mx").classList.remove("down"), el.classList.add("max")));
      return 1180;
    },
    () => timers.forEach(clearTimeout),
    { step, hint },
  );
}
