// Mac OS 9: the periwinkle desktop under its menu bar, paper.pdf's icon on
// it, and a Platinum progress window with the build's steps: the step, its
// detail, the bar filling as packages come and the blue barber pole while how
// far is not known; "Did you know" as Balloon Help. Page in: the bar fills,
// the progress window goes, paper.pdf's icon is opened (darkened), its window
// zooms out of it in the Finder's outline rectangles and is there with the
// page in it; its zoom box is clicked and it fills the sheet, the menu bar
// and frame going off the edges.

import { layer, ending, esc, type Opening } from "./kit.ts";

const RESTORED = { left: 8, top: 17, width: 84, height: 66 };
const DESK = "#6a6fb3";

export function mac9(box: HTMLElement, doc = "document.tex"): Opening {
  const pdf = esc(doc.replace(/\.tex$/i, "") + ".pdf");
  const el = layer(
    box,
    "phx-m9",
    `.phx-m9 { background: ${DESK}; font: 600 3cqw/1.3 "Charcoal", "Chicago", "Geneva", system-ui, sans-serif; color: #000; }
.phx-m9 .mb { position: absolute; left: 0; right: 0; top: 0; height: 5cqw; background: #e8e8e8; border-bottom: 1px solid #000; display: flex; align-items: center; gap: 3.4cqw; padding: 0 2.4cqw; z-index: 3; transition: transform .3s cubic-bezier(.5,0,.2,1); }
.phx-m9 .mb .ap { width: 3cqw; height: 3.2cqw; border-radius: 50% 50% 45% 45%; background: linear-gradient(#61bb46 0 17%, #fdb827 17% 33%, #f5821f 33% 50%, #e03a3e 50% 67%, #963d97 67% 83%, #009ddc 83%); }
.phx-m9 .mb .ck { margin-left: auto; }
.phx-m9 .ic { position: absolute; right: 6%; top: 9%; width: 16cqw; text-align: center; font: 400 2.4cqw/1.2 "Geneva", system-ui, sans-serif; color: #fff; }
.phx-m9 .ic i { display: block; margin: 0 auto .8cqw; width: 7cqw; height: 9cqw; background: #fff linear-gradient(225deg, ${DESK} 1.8cqw, #000 1.8cqw 2cqw, #e4e4e4 2cqw 2.6cqw, transparent 2.6cqw); border: 1px solid #000; }
.phx-m9 .ic i::after { content: "PDF"; display: block; margin-top: 4.4cqw; font: 700 1.8cqw/1 system-ui, sans-serif; color: #c22; }
.phx-m9 .ic.open i { filter: brightness(.45); }
.phx-m9 .ic.open span { background: #000; }
.phx-m9 .w { position: absolute; left: 7%; right: 7%; top: 32%; background: #dedede; border: 1px solid #000; box-shadow: inset 1px 1px #fff, inset -1px -1px #999, .5cqw .5cqw 0 rgb(0 0 0 / 35%); }
.phx-m9 .t { height: 4.2cqw; margin: .5cqw; display: flex; align-items: center; justify-content: center; background: repeating-linear-gradient(#dedede 0 .4cqw, #9a9a9a .4cqw .55cqw, #fff .55cqw .7cqw); position: relative; }
.phx-m9 .t b { background: #dedede; padding: 0 1.6cqw; font-weight: 600; white-space: nowrap; }
.phx-m9 .t i { position: absolute; width: 2.7cqw; height: 2.7cqw; background: #dedede; border: 1px solid #555; box-shadow: inset 1px 1px #fff, inset -1px -1px #999; }
.phx-m9 .t .cl { left: 1cqw; } .phx-m9 .t .zm { right: 4.6cqw; } .phx-m9 .t .co { right: 1cqw; }
.phx-m9 .t .zm::after { content: ""; position: absolute; left: 0; top: 0; width: 55%; height: 55%; border-right: 1px solid #555; border-bottom: 1px solid #555; }
.phx-m9 .t .co::after { content: ""; position: absolute; left: 0; right: 0; top: 45%; border-top: 1px solid #555; }
.phx-m9 .t i.down { background: #888; }
.phx-m9 .b { padding: 3cqw 4cqw 3.4cqw; display: grid; grid-template-columns: auto 1fr; gap: 1.4cqw 3cqw; align-items: center; }
.phx-m9 .doc { width: 6cqw; height: 7.8cqw; background: #fff linear-gradient(225deg, #dedede 1.4cqw, #000 1.4cqw 1.6cqw, transparent 1.6cqw); border: 1px solid #000; grid-row: span 3; }
.phx-m9 .det { font-weight: 400; font-family: "Geneva", system-ui, sans-serif; font-size: 2.5cqw; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-height: 1.3em; }
.phx-m9 .bar { height: 2.6cqw; border: 1px solid #000; border-radius: 1.4cqw; background: #fff; overflow: hidden; box-shadow: inset 1px 1px 2px rgb(0 0 0 / 40%); }
.phx-m9 .bar span { display: block; height: 100%; width: 0; border-radius: inherit; background: linear-gradient(#bcd3fb, #5d8ee6 50%, #3f6fd0 51%, #8fb2f2); transition: width .3s; }
.phx-m9 .bar.pole span { width: 100%; background-image: repeating-linear-gradient(-45deg, #4f7fd4 0 1.2cqw, #a9c4f2 1.2cqw 2.4cqw); background-size: 3.4cqw 100%; animation: phx-m9-pole .6s linear infinite; }
@keyframes phx-m9-pole { to { background-position: 3.4cqw 0; } }
.phx-m9 .bal { position: absolute; left: 12%; right: 12%; top: 66%; background: #fff; border: 1px solid #000; border-radius: 2.4cqw; padding: 2cqw 2.6cqw; font: 400 2.5cqw/1.4 "Geneva", system-ui, sans-serif; box-shadow: .5cqw .5cqw 0 rgb(0 0 0 / 30%); visibility: hidden; }
.phx-m9 .bal::before { content: ""; position: absolute; left: 22%; top: -1.6cqw; width: 2.8cqw; height: 2.8cqw; background: #fff; border-left: 1px solid #000; border-top: 1px solid #000; transform: skewY(-28deg) rotate(45deg); transform-origin: 0 100%; }
.phx-m9 .bal b { display: block; font-weight: 700; margin-bottom: .4cqw; }
.phx-m9 .bal.on { visibility: visible; }
.phx-m9 .z { position: absolute; border: 1px dashed #000; display: none; z-index: 4; }
.phx-m9 .pdfw { position: absolute; display: none; left: ${RESTORED.left}%; top: ${RESTORED.top}%; width: ${RESTORED.width}%; height: ${RESTORED.height}%;
  box-shadow: 0 0 0 1px #888, 0 0 0 .5cqw #dedede, 0 0 0 calc(.5cqw + 1px) #000, .9cqw .9cqw 0 calc(.5cqw + 1px) rgb(0 0 0 / 30%), 0 0 0 999px ${DESK}; transition: all .32s cubic-bezier(.5,0,.2,1); }
.phx-m9 .pdfw .t { position: absolute; left: -.5cqw; right: -.5cqw; bottom: 100%; margin: 0; height: 4.8cqw; border: 1px solid #000; border-bottom: none; padding: 0; }
.phx-m9.desk { background: transparent; }
.phx-m9.desk .pdfw { display: block; }
.phx-m9.desk .ic { z-index: 0; }
.phx-m9.max .pdfw { left: 0; top: 0; width: 100%; height: 100%; }
.phx-m9.max .mb { transform: translateY(-110%); }
/* (the window is the page seen through it: the icon behind would show through as it grows) */
.phx-m9.max .ic { opacity: 0; transition: opacity .12s; }`,
    `<div class="mb"><span class="ap"></span><span>File</span><span>Edit</span><span>View</span><span>Special</span><span>Help</span><span class="ck"></span></div>
     <div class="ic"><i></i><span>${pdf}</span></div>
     <div class="w"><div class="t"><i class="cl"></i><b>Typesetting</b><i class="zm"></i><i class="co"></i></div><div class="b"><div class="doc"></div><div class="msg">Starting…</div><div class="det"></div><div class="bar pole"><span></span></div></div></div>
     <div class="bal"><b>Did you know?</b><span></span></div>
     <div class="pdfw"><div class="t"><i class="cl"></i><b>${pdf}</b><i class="zm"></i><i class="co"></i></div></div>
     <div class="z"></div><div class="z"></div><div class="z"></div><div class="z"></div><div class="z"></div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  $(".ck").textContent = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const bar = $(".bar"), fill = $(".bar span"), bal = $(".bal");
  const step = (what: string, pct: number | undefined, detail: string) => {
    $(".msg").textContent = `${what}…`;
    $(".det").textContent = detail;
    bar.classList.toggle("pole", pct === undefined);
    fill.style.width = pct === undefined ? "" : `${pct}%`;
  };
  const hint = (text: string) => {
    bal.querySelector("span")!.textContent = text;
    // (Balloon Help points at what it is about: hung under the progress window, its tail on the window's edge)
    const w = $(".w").getBoundingClientRect(), b = box.getBoundingClientRect();
    bal.style.top = `calc(${w.bottom - b.top}px + 2.6cqw)`;
    bal.classList.add("on");
  };
  const timers: number[] = [];
  const at = (ms: number, f: () => void) => timers.push(window.setTimeout(f, ms));
  return ending(
    el,
    () => {
      step("Done", 100, "");
      $(".msg").textContent = "Done.";
      at(220, () => (($(".w").style.display = "none"), (bal.style.display = "none"), $(".ic").classList.add("open")));
      at(320, () => {
        // (the Finder's zoom: outline rectangles from the icon to the window, one after another)
        const b = box.getBoundingClientRect(), i = $(".ic i").getBoundingClientRect(), W = b.width, H = b.height;
        const from = { left: i.left - b.left, top: i.top - b.top, width: i.width, height: i.height };
        const to = { left: (RESTORED.left / 100) * W, top: (RESTORED.top / 100) * H - $(".pdfw .t").offsetHeight, width: (RESTORED.width / 100) * W, height: (RESTORED.height / 100) * H + $(".pdfw .t").offsetHeight };
        const px = (r: typeof from) => Object.fromEntries(Object.entries(r).map(([k, v]) => [k, `${v}px`]));
        el.querySelectorAll<HTMLElement>(".z").forEach((z, k) => {
          Object.assign(z.style, { display: "block", ...px(from), opacity: "0" });
          z.animate([{ ...px(from), opacity: 1 }, { ...px(to), opacity: 1 }, { ...px(to), opacity: 0 }], { duration: 260, delay: k * 35, easing: "steps(7, end)", fill: "forwards" });
        });
      });
      at(640, () => el.classList.add("desk"));
      at(860, () => el.querySelectorAll(".pdfw .zm").forEach((z) => z.classList.add("down")));
      at(940, () => (el.querySelectorAll(".pdfw .zm").forEach((z) => z.classList.remove("down")), el.classList.add("max")));
      return 1300;
    },
    () => timers.forEach(clearTimeout),
    { step, hint },
  );
}
