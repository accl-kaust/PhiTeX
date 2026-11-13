// The loading animations to choose from (Settings › Appearance › Loading
// animation): PhiTeX's own (opening.ts) and these, each played on the first
// page's sheet while the first build runs. The user picks any of them in a
// gallery that plays each (preview); every opening (a document opened, a
// Rebuild) plays one of those picked at random, PhiTeX's the first time ever
// when it is picked. "random", the default, is every one but Bad Apple (a
// video's silhouettes: picked by those who want it); "all" every one; "off"
// none.

import { opening, still, type Opening } from "../opening.ts";
import { win98 } from "./win98.ts";
import { mac9 } from "./mac9.ts";
import { terminal } from "./terminal.ts";
import { dos } from "./dos.ts";
import { typewriter } from "./typewriter.ts";
import { ink } from "./ink.ts";
import { tetris } from "./tetris.ts";
import { plane } from "./plane.ts";
import { glyphs } from "./glyphs.ts";
import { kaust } from "./kaust.ts";
import { badapple, preload as preloadBadApple } from "./badapple.ts";

export interface Look {
  id: string;
  name: string;
  /** Played in `sheet`, on dark pages or not; `doc` is the main file's name (a title bar's, a prompt's). */
  make(sheet: HTMLElement, dark: boolean, doc: string): Opening;
}

export const LOOKS: Look[] = [
  { id: "phitex", name: "PhiTeX", make: (s, dark) => opening(s, dark ? "#e4e6ea" : "#1d3e72", dark ? "#2c2e33" : "#ffffff") },
  { id: "terminal", name: "Terminal", make: (s, _d, doc) => terminal(s, doc) },
  { id: "win98", name: "Windows 98", make: (s, _d, doc) => win98(s, doc) },
  { id: "mac9", name: "Mac OS 9", make: (s, _d, doc) => mac9(s, doc) },
  { id: "dos", name: "MS-DOS", make: (s) => dos(s) },
  { id: "typewriter", name: "Typewriter", make: (s, dark, doc) => typewriter(s, dark, doc) },
  { id: "plane", name: "Paper plane", make: (s, dark) => plane(s, dark) },
  { id: "ink", name: "Ink drop", make: (s, dark) => ink(s, dark) },
  { id: "tetris", name: "Tetris", make: (s, dark) => tetris(s, dark) },
  { id: "glyphs", name: "Glyph rain", make: (s, dark) => glyphs(s, dark) },
  { id: "kaust", name: "ACCL | KAUST", make: (s, dark) => kaust(s, dark) },
  { id: "badapple", name: "Bad Apple!!", make: (s, dark) => badapple(s, dark) },
];

/** The looks of the default, "random": every one but Bad Apple. */
const DEFAULT = LOOKS.filter((l) => l.id !== "badapple").map((l) => l.id);

/**
 * The choice as kept: "random" (the default's, DEFAULT), "all" (every one of LOOKS), "off" (none), or the
 * ones chosen, by id, comma separated ("tetris,badapple"; one id alone is
 * the first choice's, one look). Each opening plays one of them at random.
 */
export type OpeningLook = string;

/** The ids chosen in `look`, in LOOKS' order. */
export function looksIn(look: OpeningLook): string[] {
  if (look === "random") return DEFAULT;
  if (look === "all") return LOOKS.map((l) => l.id);
  if (look === "off") return [];
  const ids = new Set(look.split(",").map((v) => (v.trim() === "retro" ? "win98" : v.trim())));
  return LOOKS.filter((l) => ids.has(l.id)).map((l) => l.id);
}

/** The choice for `ids`: the default's is "random", every one "all", none "off". */
export function lookFrom(ids: Iterable<string>): OpeningLook {
  const set = new Set(ids);
  const chosen = LOOKS.filter((l) => set.has(l.id)).map((l) => l.id);
  if (chosen.join(",") === DEFAULT.join(",")) return "random";
  return chosen.length === LOOKS.length ? "all" : chosen.length ? chosen.join(",") : "off";
}

/** The choice as kept, made canonical: "retro" (the first Windows 98's, retired) is Windows 98's now; one naming none known is the default's. */
export function lookOf(v: unknown): OpeningLook {
  if (typeof v !== "string") return "random";
  if (v === "random" || v === "all" || v === "off") return v;
  const ids = looksIn(v);
  return ids.length ? lookFrom(ids) : "random";
}

/** What the looks in `look` read ahead, so that each draws on its first frame: Bad Apple's frames. */
export function ready(look: OpeningLook): void {
  if (looksIn(look).includes("badapple")) preloadBadApple();
}

/** The opening to play for `look`: `first` (none played ever) gets PhiTeX's when it is chosen; another than the last one played, when there is another. */
export function play(look: OpeningLook, first: boolean, last: string | null, sheet: HTMLElement, dark: boolean, doc: string): { id: string; anim: Opening } {
  const ids = looksIn(look);
  if (!ids.length) return { id: "off", anim: still() };
  const others = ids.filter((id) => id !== last);
  const pool = others.length ? others : ids;
  const id = first && ids.includes("phitex") ? "phitex" : pool[Math.floor(Math.random() * pool.length)];
  return { id, anim: LOOKS.find((l) => l.id === id)!.make(sheet, dark, doc) };
}

/**
 * The look `id` played over and over in `tile` (Settings' gallery): on a
 * sheet over a made-up first page, the build's steps as a cold build says
 * them (`steps`: reading, fetching 24 packages, typesetting page 1), `hint`
 * its "Did you know", then the page-in onto that page; again 1.4 s on.
 * Stopped by what it returns.
 */
export function preview(id: string, tile: HTMLElement, dark: boolean, doc: string, steps: readonly [string, string, string], hint: string): () => void {
  const look = LOOKS.find((l) => l.id === id);
  if (!look) return () => {};
  const page = document.createElement("canvas"), sheet = document.createElement("div");
  page.className = "pg";
  sheet.className = "sheet";
  tile.replaceChildren(page, sheet);
  let anim: Opening | null = null, timers: ReturnType<typeof setTimeout>[] = [], alive = true;
  const at = (ms: number, f: () => void) => timers.push(setTimeout(() => alive && f(), ms));
  const round = () => {
    timers.forEach(clearTimeout);
    timers = [];
    anim?.stop();
    sheet.replaceChildren();
    sheet.classList.remove("in");
    firstPage(page, dark);
    const a = (anim = look.make(sheet, dark, doc));
    a.step?.(steps[0], undefined, "");
    at(900, () => a.hint?.(hint));
    for (let k = 1; k <= 24; k++) at(1100 + k * 90, () => a.step?.(steps[1], Math.round((100 * k) / 24), `${k} of 24 · ${(k * 0.21).toFixed(1)} MB`));
    at(3500, () => a.step?.(steps[2], undefined, "Page 1"));
    at(5800, () => {
      sheet.classList.add("in");
      let next = false;
      const again = () => !next && ((next = true), a.stop(), sheet.replaceChildren(), at(1400, round));
      void a.done(page).then(again);
      at(1600, again);
    });
  };
  round();
  return () => {
    alive = false;
    timers.forEach(clearTimeout);
    anim?.stop();
    tile.replaceChildren();
  };
}

/** A first page drawn in `cv` at its size: a title, an author, paragraphs as grey lines. */
function firstPage(cv: HTMLCanvasElement, dark: boolean): void {
  const dpr = devicePixelRatio || 1, w = Math.max(1, Math.round(cv.clientWidth * dpr)), h = Math.max(1, Math.round(cv.clientHeight * dpr));
  cv.width = w;
  cv.height = h;
  const g = cv.getContext("2d")!;
  g.fillStyle = dark ? "#2c2e33" : "#ffffff";
  g.fillRect(0, 0, w, h);
  g.fillStyle = dark ? "#c9ccd2" : "#3a3f48";
  const m = w * 0.14, line = h * 0.0125, lead = h * 0.024;
  g.fillRect(w * 0.27, h * 0.1, w * 0.46, line * 1.7);
  g.fillRect(w * 0.38, h * 0.1 + lead * 1.6, w * 0.24, line);
  let y = h * 0.21;
  for (let p = 0; y < h * 0.88; p++) {
    const n = 4 + ((p * 5) % 4);
    for (let k = 0; k < n && y < h * 0.88; k++, y += lead) {
      const x0 = k === 0 ? m + w * 0.04 : m, x1 = k === n - 1 ? m + (w - 2 * m) * (0.35 + ((p * 37) % 50) / 100) : w - m;
      g.fillRect(x0, y, x1 - x0, line);
    }
    y += lead * 0.6;
  }
}
