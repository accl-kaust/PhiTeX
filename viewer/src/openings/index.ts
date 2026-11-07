// The loading animations to choose from (Settings › Appearance › Loading
// animation): PhiTeX's own (opening.ts) and these, each played on the first
// page's sheet while the first build runs. "random", the default, plays
// PhiTeX's the first time ever and one of them at random at every opening
// after (a document opened, a Rebuild); "off" plays none.

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
];

/** The choice: one of LOOKS, at random, or none. */
export type OpeningLook = "random" | "off" | (typeof LOOKS)[number]["id"];

/** The choice as kept: "retro" (the first Windows 98's, retired) is Windows 98's now; one not known is the default's. */
export function lookOf(v: unknown): OpeningLook {
  if (v === "retro") return "win98";
  return v === "random" || v === "off" || LOOKS.some((l) => l.id === v) ? (v as OpeningLook) : "random";
}

/** The opening to play for `look`: `first` (none played ever) gets PhiTeX's; random avoids the last one played. */
export function play(look: OpeningLook, first: boolean, last: string | null, sheet: HTMLElement, dark: boolean, doc: string): { id: string; anim: Opening } {
  if (look === "off") return { id: "off", anim: still() };
  let l = LOOKS.find((x) => x.id === look);
  if (!l) {
    const pool = LOOKS.filter((x) => x.id !== last);
    l = first ? LOOKS[0] : pool[Math.floor(Math.random() * pool.length)];
  }
  return { id: l.id, anim: l.make(sheet, dark, doc) };
}
