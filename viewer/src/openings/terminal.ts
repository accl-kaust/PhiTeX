// Terminal: an old monitor running `phitex watch` on the document, the loading
// itself told as its log: the project read, each package fetched under a
// progress bar of block characters, typesetting with a spinner and the page
// it is on, and "Did you know" as a comment. Page in: the page's time is
// printed, then the monitor switches off, the picture folding to a line and a
// point over the page.

import { html, type Slot } from "../html.ts";
import { layer, ending, crt, crtOff, type Opening } from "./kit.ts";

const SPIN = "|/-\\";
const BAR = 16;

export function terminal(box: HTMLElement, doc = "document.tex"): Opening {
  const el = layer(
    box,
    "phx-term",
    `${crt("phx-term", "radial-gradient(ellipse at 50% 40%, #12201a, #060a08 75%)", "rgb(120 255 170 / 60%)")}
.phx-term .txt { position: absolute; inset: 0; padding: 6cqw 6cqw; font: 2.5cqw/1.5 "DM Mono", ui-monospace, monospace; color: #b8f5c8; display: flex; flex-direction: column; justify-content: flex-end; }
.phx-term .l { white-space: pre-wrap; word-break: break-word; }
.phx-term .p { color: #7dffa0; } .phx-term .ok { color: #7dffa0; } .phx-term .d { color: #79c0ff; } .phx-term .f { color: #ffd27a; } .phx-term .h { color: #6f8f7c; font-style: italic; }
.phx-term .dim { color: #6f8f7c; }
.phx-term .cur { display: inline-block; width: .6em; height: 1.1em; vertical-align: -.2em; background: #7dffa0; animation: phx-term-blink 1s steps(1) infinite; }
@keyframes phx-term-blink { 50% { opacity: 0; } }`,
    html`<div class="scr"><div class="txt"></div><div class="band"></div></div><div class="led"></div><div class="brand">PHITEX</div>`,
  );
  const txt = el.querySelector<HTMLElement>(".txt")!;
  const cursor = document.createElement("span");
  cursor.className = "cur";
  const t0 = performance.now();
  const line = (content: Slot, cls = "") => {
    const l = document.createElement("div");
    l.className = `l ${cls}`;
    l.replaceChildren(html`${content}`);
    txt.append(l);
    l.append(cursor);
    // (the oldest lines scroll off the top)
    while (txt.scrollHeight > txt.clientHeight + 2 && txt.children.length > 1) txt.firstElementChild!.remove();
    return l;
  };
  // (the command typed at the prompt first, a key at a time)
  const cmd = `phitex watch ${doc}`, prompt = line(html`<span class="p">$ </span>`);
  let typed = 0, timer = 0;
  const key = () => {
    prompt.insertBefore(document.createTextNode(cmd[typed++]), cursor);
    if (typed < cmd.length) timer = window.setTimeout(key, 22 + Math.random() * 30);
    else (timer = 0), flush();
  };
  timer = window.setTimeout(key, 420);
  // (steps that come while the command is still typed wait for it)
  let queue: (() => void)[] = [];
  const later = (f: () => void) => (timer || typed < cmd.length ? queue.push(f) : f());
  const flush = () => {
    line(html`<span class="d">PhiTeX</span>`);
    queue.forEach((f) => f());
    queue = [];
  };
  let cur: { what: string; el: HTMLElement; seen: Set<string> } | null = null, spin = 0, spinner = 0;
  const step = (what: string, pct: number | undefined, detail: string) =>
    later(() => {
      if (cur?.what !== what) {
        // (the step before it done: said so)
        if (cur) cur.el.replaceChildren(html`<span class="ok">✓</span> ${cur.what.toLowerCase()}`);
        cur = { what, el: line(""), seen: new Set() };
      }
      const name = what.toLowerCase();
      if (pct !== undefined) {
        // (packages: each one listed as it comes, the bar under them)
        const pkg = detail.split(" · ")[0];
        if (pkg && !cur.seen.has(pkg)) {
          cur.seen.add(pkg);
          cur.el.before(Object.assign(document.createElement("div"), { className: "l dim", textContent: `  + ${pkg}` }));
        }
        const n = Math.round((pct / 100) * BAR);
        cur.el.replaceChildren(html`<span class="f">${name}</span> [${"█".repeat(n)}${"░".repeat(BAR - n)}] ${detail.split(" · ").slice(1).join("  ")}`);
      } else cur.el.replaceChildren(html`<span class="f">${SPIN[spin % SPIN.length]}</span> ${name}  <span class="dim">${detail}</span>`);
      cur.el.append(cursor);
      clearInterval(spinner);
      if (pct === undefined)
        spinner = window.setInterval(() => {
          const s = cur?.el.querySelector(".f");
          if (s) s.textContent = SPIN[++spin % SPIN.length];
        }, 80);
    });
  const hint = (text: string) => later(() => cur?.el.before(Object.assign(document.createElement("div"), { className: "l h", textContent: `# ${text}` })));
  const cleanup = () => (clearTimeout(timer), clearInterval(spinner));
  return ending(
    el,
    () => {
      cleanup();
      if (cur) cur.el.replaceChildren(html`<span class="ok">✓</span> ${cur.what.toLowerCase()}`);
      line(html`<span class="ok">✓ page 1 in ${((performance.now() - t0) / 1000).toFixed(1)} s</span>`);
      // (a beat to read it, then off)
      setTimeout(() => crtOff(el), 180);
      return 700;
    },
    cleanup,
    { step, hint },
  );
}
