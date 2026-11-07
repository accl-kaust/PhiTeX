// Typewriter: the build typed out as it happens, on a cream sheet in a black
// ribbon: the project read, each package's name as it is fetched, the page
// being set, and "Did you know" in the ribbon's red half, a bell at each long
// line's end and the paper feeding up. Page in: the typed letters blur and
// dissolve into the typeset page under them (typewritten → typeset).

import { layer, ending, PAPER, type Opening } from "./kit.ts";

export function typewriter(box: HTMLElement, dark: boolean, doc = "document.tex"): Opening {
  const el = layer(
    box,
    "phx-tw",
    `.phx-tw { background: ${dark ? PAPER.dark : "#fbf8f1"}; color: ${dark ? "#d8d4ca" : "#2a2622"}; transition: background .5s ease; }
.phx-tw .roll { position: absolute; left: 9cqw; right: 8cqw; top: 12cqw; font: 3.1cqw/1.55 "DM Mono", ui-monospace, monospace; white-space: pre-wrap; transition: transform .25s cubic-bezier(.3,.6,.3,1); }
.phx-tw .roll > div { min-height: 1.55em; }
.phx-tw .red { color: ${dark ? "#e07a66" : "#b0352a"}; }
.phx-tw .k { animation: phx-tw-strike .09s ease-out; }
@keyframes phx-tw-strike { from { opacity: .35; } }
.phx-tw .cur { display: inline-block; width: .6em; height: 1.05em; vertical-align: -.18em; background: currentColor; opacity: .5; animation: phx-tw-blink 1s steps(1) infinite; }
@keyframes phx-tw-blink { 50% { opacity: 0; } }
.phx-tw .ding { position: absolute; right: 3cqw; font: 2.2cqw "DM Mono", monospace; opacity: 0; color: #b0352a; }
.phx-tw .ding.on { animation: phx-tw-ding .5s ease-out; }
@keyframes phx-tw-ding { 0% { opacity: 1; transform: translateY(0); } 100% { opacity: 0; transform: translateY(-3cqw); } }
.phx-tw.out { background: transparent; }
.phx-tw.out .roll { filter: blur(.35cqw); opacity: 0; transition: filter .45s ease-in, opacity .5s ease-in; }
.phx-tw.out .cur, .phx-tw.out .ding { display: none; }`,
    `<div class="roll"></div><div class="ding">ding</div>`,
  );
  const roll = el.querySelector<HTMLElement>(".roll")!, ding = el.querySelector<HTMLElement>(".ding")!;
  const cursor = document.createElement("span");
  cursor.className = "cur";
  // (what is still to type: characters, a colour, or a new line)
  const queue: (string | { red: boolean } | "\n")[] = [];
  let line: HTMLElement, run: HTMLElement, red = false, timer = 0;
  const feed = () => {
    // (the paper feeds up once the lines pass three fifths of the sheet)
    const over = roll.offsetTop + roll.scrollHeight - box.clientHeight * 0.72;
    roll.style.transform = `translateY(${-Math.max(0, over)}px)`;
  };
  const newLine = () => {
    if (line && line.textContent!.length > 30) {
      ding.style.top = `${line.getBoundingClientRect().top - box.getBoundingClientRect().top}px`;
      ding.classList.remove("on");
      void ding.offsetWidth;
      ding.classList.add("on");
    }
    line = document.createElement("div");
    roll.append(line);
    newRun();
    feed();
  };
  const newRun = () => {
    run = document.createElement("span");
    if (red) run.className = "red";
    line.append(run);
    line.append(cursor);
  };
  newLine();
  const type = (...parts: typeof queue) => {
    queue.push(...parts);
    if (!timer) timer = window.setTimeout(key, 60);
  };
  const key = () => {
    timer = 0;
    const next = queue.shift();
    if (next === undefined) return;
    if (next === "\n") newLine();
    else if (typeof next === "object") (red = next.red), newRun();
    else {
      const k = document.createElement("span");
      k.className = "k";
      k.textContent = next;
      run.append(k);
      if (line.offsetHeight > 0) feed();
    }
    // (a typist's pace, quicker when there is a lot to catch up on)
    const pace = queue.length > 40 ? 6 : 18 + Math.random() * 30;
    timer = window.setTimeout(key, next === "\n" ? 200 : next === " " ? pace + 20 : pace);
  };
  const chars = (s: string) => [...s];
  type(...chars(doc), "\n", ...chars("─".repeat(24)), "\n");
  let what = "", pkgs = new Set<string>(), page = "";
  const step = (w: string, pct: number | undefined, detail: string) => {
    if (w !== what) {
      if (what) type("\n");
      what = w;
      type(...chars(`${w}…`));
      if (pct === undefined && detail && !/^Page /.test(detail)) type(...chars(` ${detail}`));
    }
    const pkg = pct !== undefined ? detail.split(" · ")[0] : "";
    if (pkg && !pkgs.has(pkg)) (pkgs.add(pkg), type(...chars(`${pkgs.size > 1 ? "," : ""} ${pkg}`)));
    if (/^Page \d+/.test(detail) && detail !== page) (page = detail), type(...chars(` p.${detail.slice(5)}`));
  };
  const hint = (text: string) => type("\n", "\n", { red: true }, ...chars(`Did you know? ${text}`), { red: false }, "\n");
  return ending(el, () => (clearTimeout(timer), el.classList.add("out"), 520), () => clearTimeout(timer), { step, hint });
}
