// The preview panel: the pages, and around them controls that look native
// to the host (print.ts, viewer/STYLE.md). In Overleaf it fills the PDF pane
// under Overleaf's own toolbar (the extension puts its controls there); in
// VS Code and the watch it has its own bar: Rebuild and its menu, the
// build's status (its timing a click away), find, compare, download, the
// page, zoom, the problems, and a menu for the rest. Before the first page,
// the first page's sheet plays the opening (opening.ts) with the step under
// it; a package fetched later, a build running on, say so in the corner.
//
// Quality of life: floating (Overleaf's layout not found), drag by the
// header, resize from the corner, collapse (Alt+Shift+P); position, size
// and zoom remembered; a build that ships nothing keeps the last good page,
// dimmed, with why; problems jump to their line.

import type { PageImage, Status } from "./session.ts";
import type { Diagnostic } from "./diagnostics.ts";
import type { PackageState } from "./packages.ts";
import { ENGINES, errorNeeds, type Engine } from "./engines.ts";
import { Viewer } from "./viewer.ts";
import { VIEWER_CSS } from "./css.ts";
import { PRINT_CSS } from "./print.ts";
import { PROBLEMS_CSS, problemHtml } from "./problems.ts";
import { OUTLINE_CSS, Outline, type Entry } from "./outline.ts";
import { KEYS, bindKeys } from "./keys.ts";
import { REPORT_TO } from "./report.ts";
import { opening, type Opening } from "./opening.ts";

/** How long a build may ship no page before the last good one is dimmed (typing through `{`). */
const STALE_GRACE_MS = 1000;
/** What the status says while the engine makes the format (not cached: once per PhiTeX update). */
const MAKING = "Making the format, once per PhiTeX update";

/** The diff bar's state (Panel.diffBar). */
export interface DiffBar {
  /** The version compared against: "Submitted v1", or an update's files. */
  title: string;
  /** "3 h ago · Ammar". */
  when: string;
  changes: number;
  /** The change shown (1-based; 0: none yet). */
  at: number;
  showing: "diff" | "current";
  /** What it is doing, until it is done ("Fetching version 42…"). */
  busy?: string;
}
export type DiffBarActions = Partial<Record<"prev" | "next" | "toggle" | "pdf" | "tex" | "settings" | "close", () => void>>;

/** The diff's look, as diff.ts's DiffStyle (latexdiff's type and subtype, the colors). */
export interface DiffLook {
  markup: string;
  subtype: string;
  add_color: string;
  del_color: string;
}

/** What the panel shows, for controls that live outside it (a host's own toolbar). */
export interface ViewState {
  page: number;
  pages: number;
  zoom: string;
  debug: boolean;
  /** "ok" | "warning" | "error". */
  level: string;
  chip: string;
  chipTitle: string;
  /** The last repaint's keystroke→page time, and when it was (for a flourish). */
  speed?: { ms: number; at: number; first?: boolean };
  sheet: boolean;
  /** Diagnostics: how many, and the worst severity (or ""). */
  diagCount: number;
  diagWorst: string;
  /** The zoom as a percentage (fit: what fitting the width comes to), as Overleaf's viewer shows it. */
  percent: number;
  /** The build's status as the bar says it: "Built", "· 42 ms", an error, a build running. */
  status: { text: string; more: string; err: boolean; busy: boolean };
  /** The timing popover's content: the last build's summary, its parts [what, how long], the last message. */
  timing: { summary: string; parts: [string, string][]; message: string };
}

export interface PanelEvents {
  /** The page in view changed (the one an edit paints first). */
  onPage(p: number): void;
  /** Page `k` is wanted: in view, not drawn at its current hash. */
  onNeed?(k: number): void;
  onPdf(): void;
  /** The debug check (a fresh build compared every 5 s): absent, no debug button. */
  onDebug?(on: boolean): void;
  onMain(m: string): void;
  onReload(): void;
  /** Clean rebuild: the project read again, typeset from the start. */
  onClean(): void;
  /** Rebuild with the files as they are (absent: onReload). Builds run by themselves on each edit; this is for when one is wanted anyway. */
  onRebuild?(): void;
  /** Compare with an earlier version (the host's picker, then the diff bar): absent, not offered. */
  onCompare?(): void;
  /** Clear the packages and formats kept (the host's cache): absent, not offered. */
  onClearCache?(): void;
  /** The boxes of [start, end) (bytes) of `file` on the pages (the session's boxesOf): a problem's culprit, underlined. */
  onBoxes?(file: string, start: number, end: number): Promise<{ k: number; boxes: [number, number, number, number][] }[]>;
  /** The last build's log (the job's .log), or null: absent, no "View log". */
  onLog?(): Promise<string | null>;
  onGoto(file: string, line: number): void;
  /** A double-click on page `k` at (x, y), PDF points from its top left: to the source. */
  onSyncSource?(k: number, x: number, y: number): void;
  /** The anonymized debug report (report.ts), for "Report a problem". */
  onReport?(): Promise<string>;
  /** Run this project with engine `e` from now on (the card's buttons). */
  onEngine?(e: Engine): void;
  /** The editor to `file`'s [from, to) (UTF-16 offsets); `focus` false keeps the focus on the page. */
  onGotoRange?(file: string, from: number, to: number, focus?: boolean): void;
  /** Text selected on the pages (boxes by page, PDF points): to its source. */
  onSelectPage?(sel: { k: number; rects: [number, number, number, number][] }[]): void;
  /** Alt+Shift+P: return true if the host handled it (docked: PDF ⇄ PhiTeX); else the window collapses. */
  onShortcut?(): boolean;
  /** "Take the tour" (settings). */
  onTour?(): void;
  /** "What's new" (settings). */
  onNews?(): void;
  /** The contents beside the pages opened: the outline wanted now (the host's session's `outline()`). */
  onContents?(): void;
  /** A link to a web address on a page (absent: opened in a new tab). */
  onLink?(uri: string): void;
  /** Where `q` is on the pages (the host's MuPDF searches them all): absent, no find field. */
  onFind?(q: string): Promise<{ k: number; boxes: [number, number, number, number][] }[]>;
}

/**
 * What the panel says that depends on where it runs (the defaults:
 * Overleaf's; VS Code's webview gives its own). `about` and `reading` are
 * markup, the rest text.
 */
export interface PanelWords {
  /** The header's badge, and its tooltip. */
  badge: string;
  badgeTitle: string;
  /** The line under the page, and what it opens. */
  byline: string;
  about: string;
  /** The strip while the first build runs past 400 ms (`{s}`: the seconds so far); empty: no strip for builds at all (the host shows them). */
  preparing: string;
  /** The strip, quieter, while a later build runs on past 400 ms (an edit's page already shown: the rest of the document settling). */
  updating?: string;
  /** Where the full license is. */
  license: string;
  /** Where the real PDF is, after a stop, an approximation or an engine not yet run. */
  realPdf: string;
  stopped: string;
  notReady: string;
  openFailed: string;
  /** The reading card's note, and where packages are kept. */
  reading: string;
  keptIn: string;
  /** The first page's three steps, as the sheet names them (reading, getting what it needs, building). */
  steps: [string, string, string];
}

const OVERLEAF_WORDS: PanelWords = {
  badge: "Unofficial · experimental",
  badgeTitle: "An unofficial extension, not part of Overleaf. PhiTeX runs LaTeX (pdfTeX) in your browser; Overleaf's PDF is the real one.",
  byline: "Unofficial PhiTeX extension · experimental",
  about: `<b>⚡ Instant is not part of Overleaf.</b> It is added by the <b>unofficial PhiTeX</b> browser extension, not made,
      endorsed or supported by Overleaf: an experimental incremental TeX engine that runs entirely in your browser. Nothing is
      sent anywhere; Overleaf's own PDF is on the <b>PDF</b> tab.
      <div class="about-foot">LaTeX, with TeX Live's packages fetched as needed. Free software (AGPL-3.0-only,
      <a id="license" target="_blank" rel="noopener">full license</a>), provided as is, without any warranty.
      To turn it off: <code>chrome://extensions</code>.</div>`,
  preparing: "Getting ready for instant edits… {s} s (pages still scroll)",
  updating: "Updating the other pages…",
  license: "LICENSE.txt",
  realPdf: "Overleaf's PDF is the real one.",
  stopped: "Open ⓘ diagnostics for the details. Overleaf's PDF is one click away (PDF).",
  notReady: "Use Overleaf's PDF for it",
  openFailed: "Overleaf's PDF is one click away. Reload the tab to try again.",
  reading: "Your project's files stay in this browser: read from Overleaf, typeset here.",
  keptIn: "this browser",
  steps: ["Reading the project", "Fetching packages", "Typesetting"],
};

/** `c` if it is a #rrggbb color (what goes into a style or value attribute), else `or`. */
export function safeColor(c: unknown, or: string): string {
  return typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c) ? c : or;
}

/** What a host may leave out: a control whose event it doesn't handle is hidden. */
export interface PanelOptions {
  words?: Partial<PanelWords>;
  /** Whose look (print.ts): Overleaf's (default), VS Code's (also found from its webview's body), or the watch's own. */
  host?: "overleaf" | "vscode" | "watch";
  /** One of the host's own files by its path (the license): its address (default: the path as it is). */
  fileUrl?(path: string): string;
  /** The document's contents beside the pages (outline.ts: the host gives them, `outline`), `t` or ☰ to show or hide. */
  outline?: boolean;
  /** The pages' keys (keys.ts: PDF viewers' and vim's, `?` lists them), and `e` for the problems; where the page owns the keyboard (the CLI's), not under an editor. */
  keys?: boolean;
  /** The feature tips shown by themselves on a first visit (default true; false: only What's new shows them, a demo's first visitors seeing no "New"). */
  tips?: boolean;
  /** Docked where the host has no toolbar of its own (the CLI's page, VS Code's webview): the panel's header and footer shown. */
  header?: boolean;
  /** The panel's footer (false: the host has its own status bar, the CLI's). */
  footer?: boolean;
}

/** Where the panel keeps its preferences (chrome.storage.local in the extension). */
export interface Prefs {
  load(): Promise<Partial<PanelPrefs>>;
  save(p: PanelPrefs): void;
}

export interface PanelPrefs {
  x: number | null;
  y: number | null;
  w: number;
  h: number;
  zoom: string;
  collapsed: boolean;
  details: boolean;
}

const DEFAULTS: PanelPrefs = { x: null, y: null, w: 440, h: 620, zoom: "fit", collapsed: false, details: false };

/** A page's CSS size at `cssWidth`, each side rounded down to whole device pixels (its canvas then on the pixel grid). */
export function pageBox(d: { w: number; h: number }, cssWidth: number, dpr = globalThis.devicePixelRatio || 1): [number, number] {
  const snap = (x: number) => Math.floor(x * dpr) / dpr;
  return [snap(cssWidth), snap((d.h * cssWidth) / d.w)];
}

const esc = (t: string) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/** Material icons (Apache-2.0), inline: no dependency on the page's icon font. */
const PATHS: Record<string, string> = {
  preview: "M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z",
  chevron_left: "M15.41 7.41 14 6l-6 6 6 6 1.41-1.41L10.83 12z",
  chevron_right: "M10 6 8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z",
  bug_report:
    "M20 8h-2.81c-.45-.78-1.07-1.45-1.82-1.96L17 4.41 15.59 3l-2.17 2.17C12.96 5.06 12.49 5 12 5s-.96.06-1.41.17L8.41 3 7 4.41l1.62 1.63C7.88 6.55 7.26 7.22 6.81 8H4v2h2.09c-.05.33-.09.66-.09 1v1H4v2h2v1c0 .34.04.67.09 1H4v2h2.81c1.04 1.79 2.97 3 5.19 3s4.15-1.21 5.19-3H20v-2h-2.09c.05-.33.09-.66.09-1v-1h2v-2h-2v-1c0-.34-.04-.67-.09-1H20V8zm-6 8h-4v-2h4v2zm0-4h-4v-2h4v2z",
  close_fullscreen: "M19 13H5v-2h14v2z",
  open_in_full: "M21 11V3h-8l3.29 3.29-10 10L3 13v8h8l-3.29-3.29 10-10z",
  sync: "M12 4V1L8 5l4 4V6c3.31 0 6 2.69 6 6 0 1.01-.25 1.97-.7 2.8l1.46 1.46C19.54 15.03 20 13.57 20 12c0-4.42-3.58-8-8-8zm0 14c-3.31 0-6-2.69-6-6 0-1.01.25-1.97.7-2.8L5.24 7.74C4.46 8.97 4 10.43 4 12c0 4.42 3.58 8 8 8v3l4-4-4-4v3z",
  download: "M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z",
  error: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z",
  warning: "M1 21h22L12 2 1 21zm12-3h-2v-2h2v2zm0-4h-2v-4h2v4z",
  info: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z",
  refresh: "M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z",
  expand_more: "M16.59 8.59 12 13.17 7.41 8.59 6 10l6 6 6-6z",
  more_horiz: "M6 10c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm12 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm-6 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z",
  search: "M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z",
  compare: "M9.01 14H2v2h7.01v3L13 15l-3.99-4v3zm5.98-1v-3H22V8h-7.01V5L11 9l3.99 4z",
  close: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  log: "M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
  toc: "M3 9h14V7H3v2zm0 4h14v-2H3v2zm0 4h14v-2H3v2zm16 0h2v-2h-2v2zm0-10v2h2V7h-2zm0 6h2v-2h-2v2z",
  dark: "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z",
  keyboard: "M20 5H4c-1.1 0-1.99.9-1.99 2L2 17c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm-9 3h2v2h-2V8zm0 3h2v2h-2v-2zM8 8h2v2H8V8zm0 3h2v2H8v-2zm-1 2H5v-2h2v2zm0-3H5V8h2v2zm9 7H8v-2h8v2zm0-4h-2v-2h2v2zm0-3h-2V8h2v2zm3 3h-2v-2h2v2zm0-3h-2V8h2v2z",
  tips: "M9 21c0 .55.45 1 1 1h4c.55 0 1-.45 1-1v-1H9v1zm3-19C8.14 2 5 5.14 5 9c0 2.38 1.19 4.47 3 5.74V17c0 .55.45 1 1 1h6c.55 0 1-.45 1-1v-2.26c1.81-1.27 3-3.36 3-5.74 0-3.86-3.14-7-7-7z",
};
const icon = (name: string, size = 18) =>
  `<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`;

/** The zooms the menu offers: fit the width, the whole page, then percentages. */
const ZOOMS: [string, string][] = [["fit", "Page width"], ["page", "Whole page"], ["0.5", "50%"], ["0.75", "75%"], ["1", "100%"], ["1.25", "125%"], ["1.5", "150%"], ["2", "200%"], ["3", "300%"]];

/** A page's lines in a tip's demo, as wide as given (%). */
const lines = (...w: number[]) => w.map((x) => `<i style="width:${x}%"></i>`).join("");

/**
 * Feature tips, each shown once (own-bar hosts): the control it points at,
 * its words, new or not, and a small looping demo of it (CSS only: .tip .demo
 * in print.ts; still, at its end state, with reduced motion).
 */
const TIPS: { id: string; at: string; title: string; text: string; demo: string; fresh?: boolean }[] = [
  {
    id: "compare", at: "#b-compare", title: "Compare versions", fresh: true,
    text: "See what changed since an earlier version, marked as latexdiff marks it: additions in blue, deletions struck out.",
    // (a sentence changes, then the change marked, then the navigator; Overleaf's compare tip had it first)
    demo: `<div class="demo cmp" aria-hidden="true">The results are <span class="del">good</span><span class="add">&nbsp;excellent</span>.<span class="ver"><i class="v0">Submitted v1</i><i class="v1">Now vs Submitted v1</i></span><span class="tag">‹ 1 / 1 ›</span></div>`,
  },
  {
    id: "timing", at: "#st", title: "How long it took",
    text: "Click the status for the last build's times and how long a keystroke takes to reach the page.",
    demo: `<div class="demo tim" aria-hidden="true"><div class="mbar"><span class="mst">Built · 42 ms</span></div><div class="mpop"><b>Keystroke to page</b><p><span>edit</span><i style="--w:30%"></i><em>11 ms</em></p><p><span>typeset</span><i style="--w:55%"></i><em>25 ms</em></p><p><span>paint</span><i style="--w:18%"></i><em>6 ms</em></p></div></div>`,
  },
  {
    id: "rebuild", at: "#b-rec", title: "Clean rebuild",
    text: "Builds run by themselves as you type. Start over from a clean slate here, or clear what PhiTeX keeps.",
    demo: `<div class="demo reb" aria-hidden="true"><div class="mbar"><span class="msplit"><span class="go">${icon("refresh", 12)}Rebuild</span><span class="dd">${icon("expand_more", 12)}</span></span></div><div class="mmenu"><span>Rebuild</span><span class="pick">Clean rebuild</span></div><div class="pg">${lines(60, 95, 90, 70, 92, 50)}</div></div>`,
  },
  {
    id: "more", at: "#b-more", title: "Dark PDF and more",
    text: "The pages in dark mode, the log, the contents and the keys are in this menu.",
    demo: `<div class="demo drk" aria-hidden="true"><div class="mbar"><span class="dots">${icon("more_horiz", 14)}</span></div><div class="mmenu"><span class="pick">${icon("dark", 11)}Dark PDF<b>✓</b></span><span>View log</span></div><div class="pg">${lines(55, 95, 88, 92, 60, 85)}</div></div>`,
  },
  {
    id: "contents", at: "#b-side", title: "Contents", fresh: true,
    text: "The document's sections beside the pages: click one to go there. This button (or t) shows or hides them.",
    demo: `<div class="demo toc" aria-hidden="true"><div class="col"><span>1 Introduction</span><span>2 Method</span><span>3 Results</span><i class="at"></i></div><div class="pg"><div class="roll">${lines(45, 95, 90, 70)}<s></s>${lines(40, 92, 85, 60)}<s></s>${lines(42, 90, 94, 75)}</div></div></div>`,
  },
];

export class Panel {
  private root: ShadowRoot;
  private host: HTMLElement;
  private docked: HTMLElement | null = null;
  private win: HTMLElement;
  private prefs: PanelPrefs = { ...DEFAULTS };
  private store?: Prefs;
  private at = 0;
  private last: PageImage | null = null;
  private url: string | null = null;
  private lastStatus: Status | null = null;
  private listeners: ((s: ViewState) => void)[] = [];
  private speed?: { ms: number; at: number; first?: boolean };
  private debugOn = false;
  private ev!: PanelEvents;
  /** A page that came while text in the shown one was selected: shown once the selection ends. */
  private viewer!: Viewer;
  /** The host's look, and whether the panel has a bar of its own (VS Code, the watch, floating). */
  private kind: "overleaf" | "vscode" | "watch";
  private ownBar: boolean;

  private $ = <T extends HTMLElement = HTMLElement>(sel: string) => this.root.querySelector(sel) as T;

  private words: PanelWords;

  constructor(ev: PanelEvents, store?: Prefs, opts: PanelOptions = {}) {
    this.store = store;
    this.ev = ev;
    const w = (this.words = { ...OVERLEAF_WORDS, ...opts.words });
    const vscode = /\bvscode-/.test(document.body?.className ?? "");
    this.kind = opts.host ?? (vscode ? "vscode" : "overleaf");
    this.ownBar = !!opts.header;
    const host = document.createElement("phitex-preview");
    this.root = host.attachShadow({ mode: "open" });
    const item = (id: string, name: string, label: string, sub = "", cls = "") =>
      `<button type="button" role="menuitem" id="${id}"${cls ? ` class="${cls}"` : ""}>${name ? `<span class="icon">${name}</span>` : '<span class="check"></span>'}<span>${label}</span>${sub ? `<span class="sub">${sub}</span>` : ""}</button>`;
    this.root.innerHTML = (`<style>${VIEWER_CSS}${PROBLEMS_CSS}${OUTLINE_CSS}${PRINT_CSS}</style>
<div class="win ${this.kind}" role="dialog" aria-label="PhiTeX preview">
  <header><span>PhiTeX</span><span class="grow"></span><button class="ib" id="min" title="Collapse (Alt+Shift+P)" aria-label="Collapse"><span class="icon">close_fullscreen</span></button></header>
  <div class="bar" role="toolbar" aria-label="PhiTeX">
    <button type="button" class="ib" id="b-side" title="Contents (t)" aria-label="Contents" aria-pressed="false"><span class="icon">toc</span></button>
    <span class="split" id="split"><button type="button" class="go" id="go" title="Build again with the files as they are (builds run by themselves as you type)"><span class="icon">refresh</span><span class="wide">Rebuild</span></button><button type="button" class="more" id="b-rec" aria-haspopup="menu" aria-expanded="false" title="More ways to build" aria-label="More ways to build"><span class="icon">expand_more</span></button></span>
    <button type="button" class="status" id="st" aria-haspopup="dialog" aria-expanded="false" title="The last build's times"><b id="sttext">Starting…</b><span id="stmore"></span></button>
    <span class="grow"></span>
    <span class="find" id="find" hidden><input id="findq" type="search" placeholder="Find" spellcheck="false" aria-label="Find in the pages"><span id="findn"></span></span>
    <button type="button" class="ib wide" id="b-find" title="Find in the pages (Ctrl+F)" aria-label="Find"><span class="icon">search</span></button>
    <button type="button" class="ib wide" id="b-compare" title="Compare with an earlier version (latexdiff)" aria-label="Compare"><span class="icon">compare</span></button>
    <button type="button" class="ib wide" id="pdf" title="Download the PDF" aria-label="Download the PDF"><span class="icon">download</span></button>
    <span class="pageno"><input id="pgin" value="–" aria-label="Page" inputmode="numeric" autocomplete="off"><span>/&nbsp;<span id="pgn">0</span></span></span>
    <button type="button" class="ib zoomb wide" id="b-zoom" aria-haspopup="menu" aria-expanded="false" title="Zoom"><span id="zl">Page width</span><span id="zs">Fit</span><span class="icon">expand_more</span></button>
    <button type="button" class="ib probs" id="b-probs" aria-haspopup="dialog" aria-expanded="false" title="Problems" hidden><span id="psev"></span><span id="pcount"></span></button>
    <button type="button" class="ib" id="b-more" aria-haspopup="menu" aria-expanded="false" title="More" aria-label="More"><span class="icon">more_horiz</span></button>
    <span class="progress" id="prog"></span>
    <div class="menu" id="m-rec" role="menu" hidden style="left:6px;top:calc(100% + 4px)">
      ${item("r-rebuild", "", "Rebuild")}
      <button type="button" role="menuitem" id="r-clean"><span class="check"></span><span>Clean rebuild<span class="desc">Start over: the project read again, built from the start</span></span></button>
      <hr class="if-compare"><button type="button" role="menuitem" id="r-compare" class="if-compare"><span class="check"></span><span>Compare with an earlier version…<span class="desc">latexdiff against a saved version</span></span></button>
      <hr class="if-cache"><button type="button" role="menuitem" id="r-cache" class="if-cache"><span class="check"></span><span>Clear cache…<span class="desc">The packages and formats PhiTeX keeps</span></span></button>
    </div>
    <div class="menu timing" id="m-time" role="dialog" aria-label="Timing" hidden style="left:6px;top:calc(100% + 4px)"></div>
    <div class="menu" id="m-zoom" role="menu" hidden style="right:60px;top:calc(100% + 4px);min-width:170px">
      ${ZOOMS.map(([z, t], i) => `${i === 2 ? "<hr>" : ""}<button type="button" role="menuitemradio" data-zoom="${z}"><span class="check"></span>${t}</button>`).join("")}
    </div>
    <div class="menu" id="m-more" role="menu" hidden style="right:6px;top:calc(100% + 4px);min-width:250px">
      ${item("x-pdf", "download", "Download PDF")}
      ${item("x-log", "log", "View log")}
      ${item("x-contents", "toc", "Contents", "t")}
      ${item("x-find", "search", "Find", "Ctrl+F", "narrow")}
      ${item("x-compare", "compare", "Compare…", "", "narrow")}
      <hr>
      <div class="mi"><span class="icon">dark</span>Dark PDF</div>
      <div class="radios" id="darkpdf" role="group" aria-label="Dark PDF"><button type="button" data-dark="off">Off</button><button type="button" data-dark="on">On</button><button type="button" data-dark="theme">Match theme</button></div>
      <button type="button" role="menuitemcheckbox" id="x-culprits" aria-checked="true"><span class="check">✓</span><span>Underline problems<span class="desc">A wavy line under what each error and warning is about</span></span></button>
      <button type="button" role="menuitemcheckbox" id="dbg" aria-checked="false"><span class="check"></span><span>Debug check<span class="desc">Compare with a fresh build every 5 s</span></span></button>
      <hr>
      ${item("x-tips", "tips", "What's new")}
      ${item("x-keys", "keyboard", "Keyboard shortcuts", "?")}
      ${item("x-about", "info", "About PhiTeX")}
    </div>
    <div class="tip" id="tip" role="dialog" aria-live="polite" hidden></div>
  </div>
  <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
  <div class="body">
    <nav class="side" id="side" aria-label="Contents"></nav>
    <div class="stage" id="stage"><div class="strip gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">✕</button></div><div class="strip approx" id="approx"></div><div class="strip banner" id="banner"></div><div class="empty" id="empty" hidden></div><div class="viewer" id="viewer"></div></div><div class="ctip" id="ctip" role="tooltip" hidden></div>
    <div class="opening" id="opening" hidden><div class="sheet"></div><div class="steps" aria-live="polite"><div class="what"></div><div class="track unknown"><i></i></div><div class="detail"></div></div></div>
    <div class="menu diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="menu logs" id="logs" role="dialog" aria-label="Problems" hidden><header><span id="logshead">Problems</span><span class="grow"></span><button type="button" class="ib" id="logsx" title="Close (Esc)" aria-label="Close"><span class="icon">close</span></button></header><div class="diags" id="diags" role="list"></div></div>
    <div class="menu sheet-pop" id="sheet" role="dialog" aria-label="PhiTeX settings" hidden>
      <label>Main file <select id="main" title="The file PhiTeX typesets"></select></label>
      <label id="dbglabel">Debug check <input type="checkbox" id="dbg2"></label>
      <div class="links"><button type="button" id="reload">Reload the files</button><button type="button" id="tour">Take the tour</button><button type="button" id="news">What's new</button></div>
      <pre class="details" id="details"></pre>
    </div>
    <div class="toast" id="toast" role="status" aria-live="polite" hidden></div>
    <div class="dialog keys" id="keyhelp" role="dialog" aria-label="Keys" hidden></div>
    <div class="dialog small about" id="about" role="dialog" aria-label="About the PhiTeX preview" hidden><header>About PhiTeX<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><div>${w.about}</div></div>
    <div class="dialog" id="logview" role="dialog" aria-label="The build's log" hidden><header>The last build's log<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><pre id="logtext"></pre></div>
    <div class="dialog" id="report" role="dialog" aria-label="Debug report" hidden>
      <header>Debug report</header>
      <p>Anonymized: no document text, nothing you typed, no file names (file1.tex, …), no project id. Read it before you send it.</p>
      <textarea id="reporttext" readonly spellcheck="false"></textarea>
      <div class="btns"><button type="button" class="btn" id="reportcopy">Copy</button><button type="button" class="btn" id="reportmail">Email</button><button type="button" class="btn" id="reportclose">Close</button></div>
    </div>
    <div class="offpill" id="offpill">Disconnected</div>
    <button type="button" class="byline" id="byline" title="About this preview">${w.byline}</button>
  </div>
</div>`).replace(/<span class="icon"(?: aria-hidden="true")?>(\w+)<\/span>/g, (_, n) => icon(n, n === "expand_more" || n === "refresh" ? 16 : 18));
    this.host = host;
    // (hidden until the host docks it, or decides it floats: no flash of a window on load)
    host.style.display = "none";
    (document.body ?? document.documentElement).append(host);
    this.win = this.$(".win");
    // (what the host doesn't handle, not offered)
    const offer = (sel: string, on: boolean) => this.root.querySelectorAll<HTMLElement>(sel).forEach((e) => (e.hidden = !on));
    offer(".if-compare, #b-compare, #x-compare", !!ev.onCompare);
    offer(".if-cache", !!ev.onClearCache);
    offer("#x-log", !!ev.onLog);
    offer("#x-contents, #b-side", !!opts.outline);
    offer("#x-keys", !!opts.keys);
    offer("#b-find, #x-find", !!ev.onFind);
    offer("#dbg, #dbglabel", !!ev.onDebug);
    offer("#x-culprits", !!ev.onBoxes);
    offer("#tour", !!ev.onTour);
    offer("#news", !!ev.onNews);
    // (the menus: one open at a time, closed by a click elsewhere or Esc)
    const menus: [string, string][] = [["#b-rec", "#m-rec"], ["#st", "#m-time"], ["#b-zoom", "#m-zoom"], ["#b-more", "#m-more"], ["#b-probs", "#logs"]];
    this.menus = menus;
    for (const [b, m] of menus)
      this.$(b).addEventListener("click", (e) => {
        e.stopPropagation();
        if (m === "#logs") return this.diagnostics();
        this.menu(m);
      });
    this.root.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      if (t.closest("[data-close]")) t.closest<HTMLElement>(".dialog")!.hidden = true;
      if (!t.closest(".menu, .tip")) this.menu(null);
    });
    this.$("#go").onclick = () => this.rebuild();
    this.$("#r-rebuild").onclick = () => (this.menu(null), this.rebuild());
    this.$("#r-clean").onclick = () => (this.menu(null), this.clean());
    this.$("#r-compare").onclick = this.$("#b-compare").onclick = this.$("#x-compare").onclick = () => (this.menu(null), ev.onCompare?.());
    this.$("#r-cache").onclick = () => (this.menu(null), ev.onClearCache?.());
    this.$("#pdf").onclick = this.$("#x-pdf").onclick = () => (this.menu(null), ev.onPdf());
    this.$("#x-log").onclick = () => (this.menu(null), void this.showLog());
    this.$("#x-contents").onclick = this.$("#b-side").onclick = () => (this.menu(null), this.side());
    this.$("#x-find").onclick = this.$("#b-find").onclick = () => (this.menu(null), this.find(true));
    this.$("#x-tips").onclick = () => (this.menu(null), this.tips(true));
    this.$("#x-keys").onclick = () => {
      this.menu(null);
      this.$("#keyhelp").hidden = false;
    };
    this.$("#x-about").onclick = this.$("#byline").onclick = (e) => {
      e.stopPropagation();
      this.menu(null);
      this.$("#about").hidden = !this.$("#about").hidden;
    };
    this.$("#m-zoom").onclick = (e) => {
      const z = (e.target as HTMLElement).closest<HTMLElement>("[data-zoom]")?.dataset.zoom;
      if (z) (this.menu(null), this.zoomTo(z));
    };
    this.$("#darkpdf").onclick = (e) => {
      const d = (e.target as HTMLElement).closest<HTMLElement>("[data-dark]")?.dataset.dark;
      if (d) this.darkPdf(d as DarkPdf);
    };
    this.$("#logsx").onclick = () => this.diagnostics(false);
    const pg = this.$<HTMLInputElement>("#pgin");
    pg.onfocus = () => pg.select();
    pg.onkeydown = (e) => {
      if (e.key !== "Enter") return;
      const k = Number(pg.value) - 1;
      if (Number.isInteger(k) && k >= 0 && k < this.viewer.pages) this.viewer.goTo(k);
      pg.blur();
    };
    pg.onblur = () => this.nav();
    this.$("#dbg").onclick = () => this.toggleDebug();
    this.$("#x-culprits").onclick = () => this.culpritsShown(!this.culpritsOn);
    try {
      this.culpritsOn = localStorage.getItem("phitex.culprits") !== "0";
    } catch {
      /* (shown) */
    }
    this.culpritsShown(this.culpritsOn, false);
    this.$("#stage").addEventListener("pointermove", (e) => this.culpritAt(e), { passive: true });
    this.$("#stage").addEventListener("pointerleave", () => (this.$("#ctip").hidden = true));
    this.$("#dbg2").onchange = () => this.toggleDebug();
    this.$("#reload").onclick = () => (this.sheet(false), ev.onReload());
    const license = this.$("#license") as HTMLAnchorElement | null;
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    this.$("#news").onclick = () => {
      this.sheet(false);
      ev.onNews?.();
    };
    this.$("#tour").onclick = () => {
      this.sheet(false);
      ev.onTour?.();
    };
    this.$<HTMLSelectElement>("#main").onchange = (e) => ev.onMain((e.target as HTMLSelectElement).value);
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#diags").onclick = (e) => {
      // (a problem card's place)
      const at = (e.target as HTMLElement).closest<HTMLElement>(".phx-place[data-file]");
      if (at) {
        e.preventDefault();
        return ev.onGoto(at.dataset.file!, Number(at.dataset.line) || 1);
      }
      const d = (e.target as HTMLElement).closest<HTMLElement>(".diag[data-line]");
      if (d) ev.onGoto(d.dataset.file!, Number(d.dataset.line));
    };
    this.$("#stage").addEventListener("pointerdown", () => {
      this.menu(null);
      this.diagnostics(false);
      this.sheet(false);
      this.$("#about").hidden = true;
    });
    this.root.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key !== "Escape") return;
      const open = this.root.querySelector<HTMLElement>(".dialog:not([hidden])");
      if (open) open.hidden = true;
      else if (this.menus.some(([, m]) => !this.$(m).hidden)) this.menu(null);
      else this.escapeProblems();
    });
    this.$("#gonex").onclick = () => this.dismissOffline();
    addEventListener("keydown", (e) => e.key === "Escape" && this.$("#stage").classList.contains("offline") && this.dismissOffline());
    window.addEventListener("keydown", (e) => {
      if (e.altKey && e.shiftKey && e.code === "KeyP") {
        e.preventDefault();
        if (!ev.onShortcut?.()) this.collapse(!this.prefs.collapsed);
      }
    });
    // (a zoom gesture over the opening's whole page: the zoom it starts from made the user's, the sheet's ease stopped first)
    this.$("#stage").addEventListener("wheel", (e) => e.ctrlKey && this.ownZoom(), { capture: true, passive: true });
    this.drag();
    this.viewer = new Viewer(this.$("#viewer"), this.$("#stage"), {
      need: (k) => ev.onNeed?.(k),
      inView: (k) => {
        this.at = k;
        this.contents?.at(k);
        this.nav();
        ev.onPage(k);
      },
      scale: () => this.scale(),
      box: (d, w) => pageBox(d, w),
      dbl: (k, x, y) => ev.onSyncSource?.(k, x, y),
      selected: (sel) => ev.onSelectPage?.(sel),
      link: ev.onLink ? (u) => ev.onLink!(u) : undefined,
    });
    this.extras(opts);
    this.darkPdf(this.darkPref(), false);
    // (keep it on screen when the window shrinks)
    window.addEventListener("resize", () => this.place());
    new ResizeObserver(() => {
      this.fitSheet();
      if (this.docked) return this.prefs.zoom === "fit" || this.zoomNow ? this.redraw() : undefined;
      if (this.prefs.collapsed) return;
      const r = this.win.getBoundingClientRect();
      // (hidden, not yet placed: nothing to remember)
      if (r.width < 100 || r.height < 100) return;
      if (Math.abs(r.width - this.prefs.w) + Math.abs(r.height - this.prefs.h) > 2) {
        this.prefs.w = Math.round(r.width);
        this.prefs.h = Math.round(r.height);
        this.save();
        if (this.prefs.zoom === "fit") this.redraw();
      }
    }).observe(this.win);
    this.place();
    this.zoomShown();
    this.renderStatus();
    store?.load().then((p) => {
      this.prefs = { ...DEFAULTS, ...p };
      // (a size saved while hidden, by an older version)
      if (this.prefs.w < 300 || this.prefs.h < 180) Object.assign(this.prefs, { w: DEFAULTS.w, h: DEFAULTS.h });
      this.place();
    });
  }

  /** The menus and popovers by their buttons. */
  private menus: [string, string][] = [];

  /** Open the menu `m` (closing the others; open already: closed), or close all (null). */
  private menu(m: string | null): void {
    for (const [b, el] of this.menus) {
      const on = el === m && this.$(el).hidden;
      if (el === "#logs") continue;
      this.$(el).hidden = !on;
      this.$(b).setAttribute("aria-expanded", String(on));
      if (on && el === "#m-time") this.timing();
    }
    if (m !== "#logs" && m !== null) this.diagnostics(false);
  }

  /** Rebuild: the host's (the files as they are), or its reload. */
  private rebuild(): void {
    this.again();
    (this.ev.onRebuild ?? this.ev.onReload).call(this.ev);
  }

  /**
   * A build asked for from the start (Rebuild, Clean rebuild): the opening
   * plays again over the pages, its steps on the sheet, until the new
   * build's first page (no "last complete render" meanwhile).
   */
  private again(): void {
    this.reopen = true;
    this.opened = false;
    this.sheetUp();
    // (the last build's pages hidden under it until the dive: the new page shows through the hole)
    this.win.classList.toggle("reopening", !!this.anim);
  }

  /** A rebuild's opening is up (firstPending, whatever pages are here). */
  private reopen = false;

    /**
   * The place on the pages for the source (Overleaf's ← arrow), as
   * `toSource` takes it: the last single click or selection on the pages
   * while still in view, else the top of the view; null with no page shown.
   */
  spot(): { k: number; x: number; y: number } | null {
    return this.viewer.spot();
  }

  /**
   * Dock into `pane` (after `after`, filling the rest of it), or float
   * (null): the fallback when the host's layout is not found.
   */
  dock(pane: HTMLElement | null, below?: Element | null): void {
    const h = this.host.style;
    if (pane) {
      // (fill the pane under its toolbar, whatever the pane's own layout;
      // called on every content-script tick: only what changed is written,
      // so an idle tab neither lays out nor repaints)
      const top = below ? Math.max(0, below.getBoundingClientRect().bottom - pane.getBoundingClientRect().top) : 0;
      const cs = getComputedStyle(pane);
      if (cs.position === "static") pane.style.position = "relative";
      // (the pane's own background, as its PDF viewer shows it)
      if (this.win.style.getPropertyValue("--pane") !== cs.backgroundColor) this.win.style.setProperty("--pane", cs.backgroundColor);
      this.win.classList.toggle("pdf-dark", pane.classList.contains("pdf-dark-mode"));
      // (Overleaf's theme, from its body; a host that says its own, theme(), keeps it)
      if (this.pageTheme === undefined) this.win.classList.toggle("light", document.body.dataset.theme === "light");
      this.applyDark();
      if (h.top !== `${top}px` || h.position !== "absolute") Object.assign(h, { position: "absolute", left: "0", right: "0", bottom: "0", top: `${top}px`, zIndex: "12", flexDirection: "column" });
    }
    if (pane === this.docked && this.host.isConnected) return;
    this.docked = pane;
    if (pane) {
      pane.append(this.host);
      this.win.classList.add("docked");
      this.win.classList.remove("collapsed");
      this.win.setAttribute("role", "region");
    } else {
      (document.body ?? document.documentElement).append(this.host);
      Object.assign(h, { position: "", left: "", right: "", bottom: "", top: "", zIndex: "" });
      this.win.classList.remove("docked");
      this.win.setAttribute("role", "dialog");
      this.place();
    }
    this.redraw();
  }

  /** Shown or not (docked: the host's own PDF is showing instead). */
  shown(on: boolean): void {
    const display = on ? (this.docked ? "flex" : "") : "none";
    // (called on every content-script tick: a redraw only when it was hidden)
    if (this.host.style.display === display && this.wasShown === on) return;
    const was = this.wasShown;
    this.wasShown = on;
    this.host.style.display = display;
    if (on && !was) this.redraw();
    // (shown with no page yet: the first page's sheet, the opening on it)
    if (on && !this.last) this.step(this.stepNow || this.words.steps[0], undefined, "");
  }
  private wasShown?: boolean;

  private save(): void {
    this.store?.save(this.prefs);
  }

  private place(): void {
    if (this.docked) return;
    const p = this.prefs,
      s = this.win.style;
    // (the default: bottom right, over the corner of Overleaf's PDF pane)
    const x = p.x ?? window.innerWidth - p.w - 16,
      y = p.y ?? window.innerHeight - p.h - 16;
    s.left = `${Math.max(0, Math.min(x, window.innerWidth - 120))}px`;
    s.top = `${Math.max(0, Math.min(y, window.innerHeight - 40))}px`;
    this.zoomShown();
    this.collapse(p.collapsed, false);
  }

  private collapse(on: boolean, save = true): void {
    this.prefs.collapsed = on;
    this.win.classList.toggle("collapsed", on);
    const b = this.$("#min");
    b.title = on ? "Expand (Alt+Shift+P)" : "Collapse (Alt+Shift+P)";
    b.innerHTML = icon(on ? "open_in_full" : "close_fullscreen");
    this.win.style.width = on ? "" : `${this.prefs.w}px`;
    this.win.style.height = on ? "" : `${this.prefs.h}px`;
    if (!on) this.redraw();
    if (save) this.save();
  }

    private drag(): void {
    this.$("header").onpointerdown = (d: PointerEvent) => {
      if ((d.target as HTMLElement).closest("button, .chip")) return;
      const r = this.win.getBoundingClientRect();
      const move = (m: PointerEvent) => {
        this.prefs.x = Math.max(0, Math.min(r.left + m.clientX - d.clientX, window.innerWidth - 120));
        this.prefs.y = Math.max(0, Math.min(r.top + m.clientY - d.clientY, window.innerHeight - 40));
        this.win.style.left = `${this.prefs.x}px`;
        this.win.style.top = `${this.prefs.y}px`;
      };
      window.addEventListener("pointermove", move);
      window.addEventListener(
        "pointerup",
        () => {
          window.removeEventListener("pointermove", move);
          this.save();
        },
        { once: true },
      );
    };
  }

  /** Packages on their way after the first page, and a build running on: said in the bar, or in the corner. */
  private pkgNow?: { names: string[]; done: number; total: number; pct: number; source: string; failed: { name: string; error: string }[] };
  private buildSince = 0;
  private actTick: ReturnType<typeof setInterval> | undefined;

  private prepSince = 0;
  /** A build has ended with pages: a later one is the quiet "updating", never "getting ready". */
  private built = false;
  /** A build of the core's (`on`) or its end: past 400 ms the status says so. */
  preparing(on: boolean): void {
    // (a host that shows its builds itself, the CLI's status bar: not said here)
    if (!this.words.preparing) return;
    if (!on && (this.last || this.viewer.pages > 0)) this.built = true;
    this.prepSince = on ? this.prepSince || performance.now() : 0;
    this.activity();
  }

  /** A streamed open's pages so far (before the first: the sheet's step; after: the status's page). */
  private streamed: number | undefined;
  /** The engine makes the format (not cached: once per PhiTeX update), before the document. */
  private making = false;
  streaming(s: { pages: number; phase: string } | null): void {
    this.streamed = s ? s.pages : undefined;
    this.making = s?.phase === "format";
    if (s && this.firstPending()) this.step(this.words.steps[2], undefined, this.making ? MAKING : `Page ${s.pages + 1}`);
    this.activity();
  }

  busy(on: boolean): void {
    if (!on && (this.last || this.viewer.pages > 0)) this.built = true;
    this.buildSince = on ? this.buildSince || performance.now() : 0;
    this.activity();
  }

  /** The bar's status, the progress line, and in a host without the bar the corner's note: never silent while the writer waits. */
  private activity(): void {
    const p = this.pkgNow;
    const now = performance.now();
    const prep = !!this.prepSince && now - this.prepSince > 400;
    const long = (!!this.buildSince && now - this.buildSince > 400) || prep;
    if (long || this.buildSince || this.prepSince || p) this.actTick ??= setInterval(() => this.activity(), 250);
    else (clearInterval(this.actTick), (this.actTick = undefined));
    this.renderStatus();
    // (before any page, the sheet says it all)
    if (!this.last && this.viewer.pages === 0) return;
    if (p) {
      const names = p.names.slice(0, 3).join(", ") + (p.names.length > 3 ? ` and ${p.names.length - 3} more` : "");
      return this.toast("pkgs", { title: `Fetching ${names}`, detail: `${p.done} of ${p.total} files${known(p.source) ? ` · from ${p.source}` : ""}`, pct: p.pct, icon: "download" });
    }
    if (this.toastKind === "pkgs") this.toast("pkgs", null);
    // (no bar: the rest of a long build in the corner, quietly; with the bar its status says it)
    if (this.ownBar) return;
    if (long && this.words.preparing) {
      const s = ((now - (this.buildSince || this.prepSince)) / 1000) | 0;
      const what = this.making ? `${MAKING}…` : this.built ? (this.words.updating ?? "Updating the other pages…") : this.streamed ? `Typesetting page ${this.streamed + 1}…` : "Typesetting…";
      this.toast("build", { title: what, detail: s >= 2 ? `${s} s` : "", pct: null, icon: "refresh" });
    } else if (this.toastKind === "build") this.toast("build", null);
  }

  /** The bar's status: what is running, or how the last build went and how fast. */
  private renderStatus(): void {
    const st = this.$("#st"), text = this.$("#sttext"), more = this.$("#stmore");
    const now = performance.now();
    const running = (!!this.buildSince && now - this.buildSince > 400) || (!!this.prepSince && now - this.prepSince > 400) || !!this.pkgNow;
    const errs = this.lastStatus?.diagnostics?.filter((d) => d.severity === "error").length ?? 0;
    let t: string, m = "", err = false;
    if (this.offline) (t = "Disconnected"), (err = true);
    else if (this.pkgNow) (t = "Fetching packages…"), (m = `${this.pkgNow.done} of ${this.pkgNow.total}`);
    else if (running) {
      const s = (now - (this.buildSince || this.prepSince)) / 1000;
      t = this.making ? `${MAKING}…` : this.built ? "Building…" : this.streamed ? `Typesetting page ${this.streamed + 1}…` : "Building…";
      m = s >= 1 ? `${s < 10 ? s.toFixed(1) : Math.round(s)} s` : "";
    } else if (this.warm) (t = "Preparing instant typing…"), (m = `${Math.round((Date.now() - this.warm.since) / 1000)} s`);
    else if (!this.last && this.viewer.pages === 0 && !this.$("#empty").hidden) (t = errs ? `Stopped with ${errs} error${errs === 1 ? "" : "s"}` : "No page yet"), (err = !!errs);
    else if (!this.last && this.viewer.pages === 0) t = this.stepNow || "Opening…";
    else if (errs) (t = `Built with ${errs} error${errs === 1 ? "" : "s"}`), (err = true);
    else if (this.speed?.first) (t = "First build"), (m = this.speed.ms < 1000 ? `${Math.round(this.speed.ms)} ms` : `${(this.speed.ms / 1000).toFixed(this.speed.ms < 10000 ? 1 : 0)} s`);
    else (t = "Built"), (m = this.speed ? `${this.speed.ms < 10 ? this.speed.ms.toFixed(1) : Math.round(this.speed.ms)} ms` : "");
    text.textContent = t;
    more.textContent = m ? `· ${m}` : "";
    const was = this.statusNow;
    if (was.text !== t || was.more !== m || was.err !== err || was.busy !== running) {
      this.statusNow = { text: t, more: m, err, busy: running };
      queueMicrotask(() => this.emit());
    }
    st.classList.toggle("err", err);
    this.$("#prog").classList.toggle("on", running || (!this.last && this.viewer.pages === 0 && !this.offline && this.$("#empty").hidden === true));
    this.$("#split").classList.toggle("busy", running);
  }

  /** The status as last said (ViewState.status: a host's toolbar shows it). */
  private statusNow: ViewState["status"] = { text: "", more: "", err: false, busy: false };

  /** Packages still on their way before the first page, so a stop reads as one only once they are in. */
  private pkgBusy = false;

  /**
   * Before any page: a build that stopped at an error says so, in place of
   * the sheet (a stopped job's PDF is unfinished: no page to draw).
   */
  private stopped(): boolean {
    if (this.engineNow && !this.engineNow.ready) return this.engineCard(this.engineNow.engine, null);
    if (this.last || this.pkgBusy) return false;
    const e = this.lastStatus?.diagnostics?.find((d) => d.severity === "error");
    if (!e) return false;
    // (fontspec, unicode-math, polyglossia: a XeLaTeX/LuaLaTeX project, which pdfTeX can't run)
    const needed = errorNeeds(e.message);
    if (needed) return this.engineCard(needed, e.message);
    const card = this.card();
    // (the engine itself gave out: its sentence, its dump folded away)
    if (e.code === "engine") {
      card.querySelector("h3")!.textContent = "PhiTeX's engine stopped";
      card.querySelector(".count")!.textContent = e.message;
      card.querySelector(".note")!.textContent = "Your document is probably fine. Save again to retry.";
      if (e.detail) card.append(foldedDetail(e.detail, "Engine details"));
      this.reportLink(card);
      return true;
    }
    card.querySelector("h3")!.textContent = "The build stopped before a page could be shown";
    card.querySelector(".count")!.textContent = e.message + (e.file && e.line ? ` (${e.file}:${e.line})` : "");
    card.querySelector(".note")!.textContent = this.words.stopped;
    this.reportLink(card);
    return true;
  }

  /** The card in place of the pages (the sheet gone): its heading, its line, its note. */
  private card(): HTMLElement {
    this.stopSheet();
    const empty = this.$("#empty");
    empty.innerHTML = `<div class="card"><h3></h3><div class="count"></div><div class="note"></div></div>`;
    empty.hidden = false;
    queueMicrotask(() => this.renderStatus());
    return empty.querySelector<HTMLElement>(".card")!;
  }

  /** "Copy debug info" under a card, if reports are wired. */
  private reportLink(card: Element): void {
    if (!this.ev.onReport) return;
    const b = document.createElement("button");
    b.className = "reportbtn";
    b.textContent = "Copy debug info…";
    b.onclick = () => void this.report();
    card.append(b);
  }

  /** The engine the session runs, as it last said. */
  private engineNow?: { engine: Engine; ready: boolean };
  /** A XeLaTeX project shown approximated with pdfLaTeX (fonts substituted). */
  approx = false;

  engine(e: { engine: Engine; ready: boolean; approx?: boolean }): void {
    this.engineNow = e;
    this.approx = !!e.approx;
    const a = this.$("#approx");
    a.classList.toggle("on", this.approx);
    a.textContent = `Approximate: needs ${ENGINES[e.engine].label}, shown with pdfLaTeX (fonts substituted, breaks may differ). ${this.words.realPdf}`;
    if (!e.ready) this.stopped();
    else if (this.$("#empty").querySelector(".engine")) this.$("#empty").hidden = true;
  }

  /**
   * The project needs `want` (its preamble, or `error`, a build that
   * stopped, says so): which engine to run it with, as buttons.
   */
  private engineCard(want: Engine, error: string | null): boolean {
    const ready = ENGINES[want].ready;
    const card = this.card();
    card.classList.add("engine");
    card.style.setProperty("--sev", "var(--ph-info)");
    card.insertAdjacentHTML("beforeend", `<div class="btns"></div>`);
    card.querySelector("h3")!.textContent = error
      ? `This project needs ${want === "lualatex" ? "LuaLaTeX" : "XeLaTeX or LuaLaTeX"}`
      : `This project runs with ${ENGINES[want].label}`;
    card.querySelector(".count")!.textContent = error ?? "Its preamble loads a package that pdfLaTeX can't run (fontspec, unicode-math, polyglossia, …), or the engine was chosen for it.";
    card.querySelector(".note")!.textContent = ready
      ? `Run it with ${ENGINES[want].label}?`
      : `⚡ Instant doesn't run ${ENGINES[want].label} yet: only pdfLaTeX. ${this.words.notReady}, or try pdfLaTeX if the project can do without those packages.`;
    const btns = card.querySelector(".btns")!;
    // (a build that stopped: the engines it asks for; running one already: back to pdfLaTeX)
    const offer: Engine[] = this.engineNow?.engine === want ? ["pdflatex"] : want === "xelatex" ? ["xelatex", "lualatex"] : [want];
    for (const e of offer) {
      const b = document.createElement("button");
      b.className = "btn";
      b.textContent = e === "pdflatex" ? "Try pdfLaTeX anyway" : `Use ${ENGINES[e].label}${ENGINES[e].ready ? "" : " (coming)"}`;
      b.title = e === "pdflatex" ? "For this project: pdfLaTeX, whatever its preamble loads" : `For this project from now on (Settings → Engine for every project)`;
      b.onclick = () => this.ev.onEngine?.(e);
      btns.append(b);
    }
    return true;
  }

  /** The level and line a host's toolbar shows (ViewState's chip). */
  private chip = { level: "ok", text: "starting…", title: "Status" };

  /** The problems' culprits underlined on the pages: the latest status's, a later one replacing an earlier still being looked up. */
  private culpritsSeq = 0;
  /** Whether they are (the ⋯ menu's Underline problems, remembered on this machine); the last status's problems, and those drawn (by data-i). */
  private culpritsOn = true;
  private culpritDiags: Diagnostic[] = [];
  private culpritsDrawn: { severity: "error" | "warning"; message: string }[] = [];
  private async underline(diags: Diagnostic[]): Promise<void> {
    if (!this.ev.onBoxes) return;
    this.culpritDiags = diags;
    const seq = ++this.culpritsSeq;
    if (!this.culpritsOn) return void (this.viewer.culprits([]), (this.$("#ctip").hidden = true));
    const want = diags
      .filter((d): d is Diagnostic & { problem: { severity: string; file: string; range: [number, number] } } => !!d.problem?.file && !!d.problem.range && d.problem.severity !== "note")
      .map((d) => ({ file: d.problem.file, range: d.problem.range, severity: d.problem.severity === "warning" ? ("warning" as const) : ("error" as const), message: d.message }))
      .sort((a, b) => (a.severity === b.severity ? 0 : a.severity === "error" ? -1 : 1))
      .slice(0, 20);
    const found = await Promise.all(want.map((w, i) => this.ev.onBoxes!(w.file, w.range[0], w.range[1]).then((hits) => hits.map((h) => ({ ...h, severity: w.severity, i })), () => [])));
    // (a newer status came while these were looked up: its own call draws them)
    if (seq !== this.culpritsSeq) return;
    this.culpritsDrawn = want;
    this.viewer.culprits(found.flat());
  }

  /** The underlines on or off (`keep`: remembered). */
  private culpritsShown(on: boolean, keep = true): void {
    this.culpritsOn = on;
    const b = this.$("#x-culprits");
    b.setAttribute("aria-checked", String(on));
    b.querySelector(".check")!.textContent = on ? "✓" : "";
    if (keep) {
      try {
        localStorage.setItem("phitex.culprits", on ? "1" : "0");
      } catch {
        /* (not remembered) */
      }
      void this.underline(this.culpritDiags);
    }
  }

  /** The pointer over an underline: its problem's message, under it. */
  private culpritAt(e: PointerEvent): void {
    const tip = this.$("#ctip");
    if (!this.culpritsDrawn.length || e.buttons) return void (tip.hidden = true);
    // (a few pixels round the line: it is thin)
    const hit = [...this.root.querySelectorAll<HTMLElement>(".slot .culprit")].find((m) => {
      const r = m.getBoundingClientRect();
      return e.clientX >= r.left - 2 && e.clientX <= r.right + 2 && e.clientY >= r.top - 2 && e.clientY <= r.bottom + 5;
    });
    const p = hit && this.culpritsDrawn[Number(hit.dataset.i)];
    if (!hit || !p) return void (tip.hidden = true);
    if (tip.hidden || tip.dataset.i !== hit.dataset.i) {
      tip.dataset.i = hit.dataset.i;
      tip.className = `ctip ${p.severity}`;
      tip.innerHTML = `${icon(p.severity, 14)}<span></span>`;
      tip.querySelector("span")!.textContent = p.message;
      tip.hidden = false;
    }
    const body = this.$(".body").getBoundingClientRect(), r = hit.getBoundingClientRect();
    const left = Math.max(8, Math.min(body.width - tip.offsetWidth - 8, e.clientX - body.left - 12));
    const below = r.bottom - body.top + 8;
    tip.style.left = `${left}px`;
    tip.style.top = `${below + tip.offsetHeight > body.height - 8 ? r.top - body.top - tip.offsetHeight - 6 : below}px`;
  }

  status(s: Status): void {
    this.lastStatus = s;
    queueMicrotask(() => this.stopped());
    const diags = s.diagnostics ?? [];
    void this.underline(diags);
    const worst = diags[0]?.severity;
    const partial = !!s.pending || s.pages === 0;
    this.chip = {
      level: worst === "error" ? "error" : partial || worst === "warning" ? "warning" : "ok",
      text: `${s.pages} page${s.pages === 1 ? "" : "s"}${s.pending ? " · partial" : ""}`,
      title:
        (s.pending ? `${s.pending} parts not read (out of fuel or unsupported). ` : "") +
        (diags.length ? `${diags.length} diagnostic${diags.length === 1 ? "" : "s"}` : "No problems found") +
        (s.file ? ` · editing ${s.file}` : ""),
    };
    const list = this.$("#diags");
    list.innerHTML = "";
    for (const d of diags.slice(0, 50)) {
      // (a problem the build told whole: its card, its places links)
      if (d.problem) {
        const card = document.createElement("ol");
        card.className = "phx-card phx-inline";
        card.setAttribute("role", "listitem");
        card.innerHTML = problemHtml(d.problem);
        list.append(card);
        continue;
      }
      const row = document.createElement("div");
      row.className = `diag ${d.severity}`;
      row.setAttribute("role", "listitem");
      if (d.file && d.line) {
        row.dataset.file = d.file;
        row.dataset.line = String(d.line);
        row.title = `Go to ${d.file}:${d.line}`;
      }
      row.innerHTML = `<span class="where"></span><div class="text"></div>`;
      row.querySelector(".text")!.textContent = d.message;
      if (d.detail && d.code === "engine") row.append(foldedDetail(d.detail, "Engine details"));
      else if (d.detail) {
        const pre = document.createElement("pre");
        pre.className = "detail";
        pre.textContent = d.detail;
        row.append(pre);
      }
      row.querySelector(".where")!.textContent = d.file ? `${d.file}${d.line ? `, line ${d.line}` : ""}` : "";
      list.append(row);
    }
    if (diags.length > 50) {
      const more = document.createElement("div");
      more.className = "diag info";
      more.textContent = `… and ${diags.length - 50} more`;
      list.append(more);
    }
    if (this.ev.onReport) {
      const b = document.createElement("button");
      b.className = "reportbtn";
      b.textContent = "Report a problem…";
      b.onclick = (e) => (e.stopPropagation(), void this.report());
      list.append(b);
    }
    // (the bar's count: errors, else warnings; info is listed, not counted)
    const count = (sev: string) => diags.filter((d) => d.severity === sev).length;
    const errors = count("error"), warnings = count("warning");
    const pb = this.$("#b-probs");
    pb.hidden = !errors && !warnings;
    pb.className = `ib probs ${errors ? "error" : "warning"}`;
    pb.title = `${errors} error${errors === 1 ? "" : "s"}, ${warnings} warning${warnings === 1 ? "" : "s"}`;
    this.$("#psev").innerHTML = icon(errors ? "error" : "warning", 16);
    this.$("#pcount").textContent = String(errors || warnings);
    this.$("#logshead").textContent = diags.length ? [errors && `${errors} error${errors === 1 ? "" : "s"}`, warnings && `${warnings} warning${warnings === 1 ? "" : "s"}`].filter(Boolean).join(", ") || "Notes" : "No problems";
    // (the list opened by itself only when there are no pages for it to cover; problems that changed close it again)
    const nopages = s.pages === 0 && !this.last;
    const key = this.problemsKey();
    if (key !== this.shownKey) {
      this.shownKey = key;
      this.diagnostics(nopages && errors > 0 && key !== this.dismissed);
    }
    if (!diags.length) this.diagnostics(false);
    this.stale(s);
    this.renderStatus();
    queueMicrotask(() => this.emit());
  }

  /** The reading step's last state, and when the sheet last changed (redrawn at most every 50 ms). */
  private readNow: { h: string; detail?: string; done?: number; total?: number; pct?: number } | undefined;
  private readDrawn = 0;
  private readTimer: ReturnType<typeof setTimeout> | undefined;
  private loadDone = -1;
  private loadPct = 0;

  /**
   * Reading the project, before the packages: what is happening (the
   * download from Overleaf, each file unpacked, each figure handed to the
   * engine), the file in hand and how far, so the first seconds read as work.
   */
  reading(r: { h: string; detail?: string; done?: number; total?: number; pct?: number }): void {
    if (this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readNow = r;
    const wait = 50 - (performance.now() - this.readDrawn);
    if (wait > 0) {
      this.readTimer ??= setTimeout(() => ((this.readTimer = undefined), this.drawReading()), wait);
      return;
    }
    this.drawReading();
  }

  private drawReading(): void {
    const r = this.readNow;
    if (!r || this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readDrawn = performance.now();
    const pct = r.pct ?? (r.total ? Math.round((100 * (r.done ?? 0)) / r.total) : undefined);
    const count = r.total ? `${r.done ?? 0} of ${r.total}` : "";
    this.step(r.h, pct, [r.detail, count].filter(Boolean).join(" · "));
  }

  packages(p: PackageState): void {
    const done = p.done?.length ?? 0;
    const total = done + p.loading.length;
    if (done !== this.loadDone) this.loadDone = done;
    // (never backwards: a build asking for more grows the total)
    const pct = (this.loadPct = Math.max(this.loadPct, total ? Math.round((100 * done) / total) : 100));
    // (before the first page: the sheet's step)
    if (!this.last) {
      if (!p.loading.length && !p.building) {
        // (done: the build's own state says the rest. Files not found are optional ones LaTeX
        // only checks for (amsart.cfg, …): the problems list them, and an error says when one was needed)
        this.pkgBusy = false;
        if (!this.stopped()) this.step(this.words.steps[2], undefined, `with ${done} package file${done === 1 ? "" : "s"}`);
        return;
      }
      this.pkgBusy = true;
      if (p.loading.length) {
        const name = p.loading[0];
        this.step(this.words.steps[1], pct, `${name} · ${done} of ${total}${p.unavailable.length ? ` · ${p.unavailable.length} not found` : ""}`);
      } else this.step(this.words.steps[2], undefined, `with ${done} package file${done === 1 ? "" : "s"}; a build may ask for more`);
      return;
    }
    this.pkgNow = p.loading.length ? { names: p.loading, done, total, pct, source: p.source, failed: p.failed ?? [] } : undefined;
    if (p.failed?.length && !p.loading.length) {
      const f = p.failed[0];
      this.toast("pkgs", { title: `Couldn't fetch ${f.name}`, detail: `${f.error}. Asked again at the next build.`, pct: null, icon: "warning", err: true });
    }
    this.activity();
  }

  /**
   * The first page's step `k` (0, 1, 2 of the words' steps), saying `text`
   * (the host's own phases: the CLI's build); nothing once a page is shown.
   */
  loadStep(k: number, text: string): void {
    if (this.last) return;
    // (pages shipped while it runs: the page reached said first)
    this.step(this.words.steps[k] ?? "", undefined, k === 2 && this.streamed !== undefined && !this.making ? `Page ${this.streamed + 1} · ${text}` : text);
  }

  // ---- The first page's sheet: the opening playing on it, the step under it ----

  /** The opening on the sheet (null: none playing), and whether it has played out once. */
  private anim: Opening | null = null;
  private opened = false;
  private ending = false;
  /** The step the sheet says now (the status says it too). */
  private stepNow = "";
  /** The zoom the opening set (the whole page, so the page that arrives keeps the sheet's height), until the user zooms. */
  private zoomNow: string | null = null;

  /** Before any page and no card: the sheet is (or now goes) up. */
  private firstPending(): boolean {
    return this.reopen || (!this.last && this.viewer.pages === 0 && this.$("#empty").hidden === true);
  }

  /** The sheet's step: what, how far (undefined: not known), and the detail; the sheet put up first if it isn't. */
  private step(what: string, pct: number | undefined, detail = ""): void {
    if (!this.firstPending()) return;
    this.sheetUp();
    const el = this.$("#opening");
    this.stepNow = what;
    el.querySelector(".what")!.textContent = what;
    el.querySelector(".detail")!.textContent = detail;
    const track = el.querySelector<HTMLElement>(".track")!;
    track.classList.toggle("unknown", pct === undefined);
    track.querySelector("i")!.style.width = pct === undefined ? "" : `${pct}%`;
    this.renderStatus();
  }

  /** The sheet up, the opening playing on it (once per panel: a later rebuild has pages). */
  private sheetUp(): void {
    if (this.opened || this.anim || !this.wasShown) return;
    const el = this.$("#opening");
    el.hidden = false;
    el.classList.remove("in");
    this.win.classList.add("sheet-up");
    // (the opening's zoom: the whole page, unless the user chose one)
    if (this.prefs.zoom === "fit") this.zoomNow = "page";
    this.fitSheet();
    const dark = this.win.classList.contains("dark-pdf") || (this.win.classList.contains("docked") && this.win.classList.contains("pdf-dark"));
    this.anim = opening(el.querySelector<HTMLElement>(".sheet")!, dark ? "#e4e6ea" : "#1d3e72", dark ? "#2c2e33" : "#ffffff");
    this.animSince = performance.now();
  }

  /** When the opening began: a page there within 400 ms (a project already built) is shown at once, no opening. */
  private animSince = 0;

  /** The sheet as the first page will be: an A4 page, whole in the stage, where the viewer puts page 1. */
  private fitSheet(): void {
    const el = this.$("#opening");
    if (el.hidden || this.ending) return;
    const st = this.$("#stage");
    const room = { w: st.clientWidth - 24, h: st.clientHeight - 24 };
    let h = room.h, w = (h * 210) / 297;
    if (w > room.w) (w = room.w), (h = (w * 297) / 210);
    Object.assign(el.style, { left: `${st.offsetLeft + (st.clientWidth - w) / 2}px`, top: `${st.offsetTop + 12}px`, width: `${w}px`, height: `${h}px` });
  }

  /**
   * The first page is in: the sheet eases to the page's box (its height
   * kept, the zoom being the whole page: only its width changes), the logo
   * settles and dives into the page beneath, and the sheet is gone.
   */
  private sheetDown(): void {
    if (!this.anim || this.ending) return;
    if (performance.now() - this.animSince < 400) return this.stopSheet();
    this.ending = true;
    this.win.classList.remove("reopening");
    const el = this.$("#opening");
    // (each frame until it is gone: the page moves under it when the contents' column comes in meanwhile, the first outline)
    const follow = () => {
      if (!this.ending) return;
      const slot = this.$("#viewer").querySelector<HTMLElement>(".slot");
      if (slot) {
        const r = slot.getBoundingClientRect(), b = this.$(".body").getBoundingClientRect();
        Object.assign(el.style, { left: `${r.left - b.left}px`, top: `${r.top - b.top}px`, width: `${r.width}px`, height: `${r.height}px` });
      }
      requestAnimationFrame(follow);
    };
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return this.stopSheet();
    requestAnimationFrame(() => {
      follow();
      el.classList.add("in");
    });
    // (the dive at once, whatever the opening's act; gone 650 ms on, and at most 900 ms on
    // even where frames stall, a tab in the background: never left over the pages)
    void this.anim.done().then(() => this.stopSheet());
    setTimeout(() => this.stopSheet(), 900);
  }

  /** The sheet gone (played out, or a card in its place). */
  private stopSheet(): void {
    const was = !!this.anim;
    this.anim?.stop();
    this.anim = null;
    this.ending = false;
    this.reopen = false;
    this.win.classList.remove("reopening");
    this.$("#opening").hidden = true;
    this.win.classList.remove("sheet-up");
    if (was) {
      this.opened = true;
      this.stepNow = "";
      this.renderStatus();
      setTimeout(() => this.tips(false), 600);
    }
  }

  /** The zoom the user has from here: the opening's whole page made theirs (a gesture starts from it), the sheet's ease stopped. */
  private ownZoom(): void {
    if (!this.zoomNow) return;
    this.prefs.zoom = this.zoomNow;
    this.zoomNow = null;
  }

  // ---- The corner's note ----

  private toastKind = "";
  private toastTimer: ReturnType<typeof setTimeout> | undefined;

  /** A note in the corner, of `kind` (a later one replaces it), or that kind's gone (null). */
  private toast(kind: string, t: { title: string; detail?: string; pct: number | null; icon?: string; err?: boolean; ms?: number } | null): void {
    const el = this.$("#toast");
    if (!t) {
      if (this.toastKind !== kind) return;
      this.toastKind = "";
      el.classList.add("out");
      clearTimeout(this.toastTimer);
      this.toastTimer = setTimeout(() => !this.toastKind && (el.hidden = true), 300);
      return;
    }
    clearTimeout(this.toastTimer);
    this.toastKind = kind;
    el.className = `toast ${kind}${t.err ? " err" : ""}`;
    el.hidden = false;
    el.innerHTML = `<div class="t">${t.icon ? icon(t.icon, 16) : ""}<span></span></div><div class="d"></div>` + (t.pct === null ? "" : `<div class="track"><i style="width:${t.pct}%"></i></div>`);
    el.querySelector(".t span")!.textContent = t.title;
    el.querySelector(".d")!.textContent = t.detail ?? "";
    if (t.ms) this.toastTimer = setTimeout(() => this.toast(kind, null), t.ms);
    // (an error stays until clicked away)
    el.title = t.err && !t.ms ? "Click to dismiss" : "";
    el.onclick = t.err && !t.ms ? () => this.toast(kind, null) : null;
  }

    /** Keep the last good page when a build ships nothing (or not this page): dimmed, with why. */
  /** When the page first went missing (a build that shipped none); its dimming waits. */
  private staleSince = 0;
  private staleTimer: ReturnType<typeof setTimeout> | undefined;

  /**
   * Dim the last good page when a build ships none, but only once that has
   * lasted STALE_GRACE_MS: typing `\section{B…` is an error at every
   * keystroke until the `}`, and the page shouldn't flash grey for each.
   */
  private stale(s: Status): void {
    const stage = this.$("#stage");
    // (a rebuild's opening is up: its sheet says what happens, not the last render's banner)
    if (this.reopen) return void (clearTimeout(this.staleTimer), (this.staleSince = 0), stage.classList.remove("stale"));
    const gone = (s.pages === 0 || this.at >= s.pages) && !!this.last;
    clearTimeout(this.staleTimer);
    if (!gone) {
      this.staleSince = 0;
      stage.classList.remove("stale");
      return;
    }
    const now = performance.now();
    this.staleSince ||= now;
    const left = STALE_GRACE_MS - (now - this.staleSince);
    if (left > 0) {
      this.staleTimer = setTimeout(() => this.lastStatus && this.stale(this.lastStatus), left);
      return;
    }
    stage.classList.add("stale");
    const why = s.diagnostics?.find((d) => d.severity !== "info" && d.code !== "no-pages");
    this.$("#banner").textContent = `Showing the last complete render: ${why ? why.message + (why.line ? ` (${why.file}:${why.line})` : "") : "this build shipped no page"}.`;
  }

  /**
   * The connection to the engine lost (`up` false: the watch stopped or
   * exited, the socket closed, the core trapped) or back: while it is lost
   * the last pages stay, dimmed and inert, under a banner saying so and why.
   */
  connection(up: boolean, why?: string): void {
    const was = this.offline;
    this.offline = !up;
    this.renderStatus();
    if (up) {
      this.$("#stage").classList.remove("offline");
      this.win.classList.remove("offline-quiet");
      return;
    }
    const text = `Disconnected from the engine: the pages are the last ones built${why ? ` (${why})` : ""}.`;
    this.$("#gonewhy").textContent = text;
    this.$("#offpill").title = text;
    // (dismissed once: it stays dismissed until the connection comes back)
    if (!was || !this.win.classList.contains("offline-quiet")) this.$("#stage").classList.add("offline");
  }

  /** The disconnection's banner dismissed (✕ or Esc): the pages interactive again, a small pill saying so. */
  private dismissOffline(): void {
    if (!this.offline) return;
    this.$("#stage").classList.remove("offline");
    this.win.classList.add("offline-quiet");
    // (clear of the pages' scrollbar, whatever its width)
    const v = this.$("#viewer");
    this.$("#offpill").style.right = `${Math.max(0, v.offsetWidth - v.clientWidth) + 8}px`;
  }

  private offline = false;

  /** Follow what the panel shows (a host toolbar's controls). */
  onState(cb: (s: ViewState) => void): void {
    this.listeners.push(cb);
    cb(this.state());
  }

  private state(): ViewState {
    return {
      page: this.at,
      pages: this.lastStatus?.pages ?? 0,
      zoom: this.zoomNow ?? this.prefs.zoom,
      debug: this.debugOn,
      level: this.chip.level,
      chip: this.chip.text,
      chipTitle: this.chip.title,
      speed: this.speed,
      sheet: !this.$("#sheet").hidden,
      // (the badge counts what needs a look: info is listed, not counted)
      diagCount: this.lastStatus?.diagnostics?.filter((d) => d.severity !== "info").length ?? 0,
      diagWorst: this.lastStatus?.diagnostics?.[0]?.severity ?? "",
      percent: Math.round(this.scale() * 100),
      status: { ...this.statusNow },
      timing: { summary: this.lat?.summary ?? "", parts: this.timingParts(), message: this.msgText },
    };
  }

    private emit(): void {
    const s = this.state();
    for (const f of this.listeners) f(s);
  }

  /**
   * The zoom, 1 being 100% (a PDF point at 96/72 CSS pixels). Fit is the
   * page's width, as Overleaf's viewer fits it: (clientWidth − 40) / the
   * page's width, 40 being its scrollbar's padding; the whole page, its
   * height in the stage's (12 px above and below) too.
   */
    /** Device pixels per PDF point at the current zoom: the size pages are drawn at. */
  pixelsPerPoint(): number {
    return (96 / 72) * this.scale() * (window.devicePixelRatio || 1);
  }

  private scale(): number {
    const z = this.zoomNow ?? this.prefs.zoom;
    if (z !== "fit" && z !== "page") return Number(z);
    const st = this.$("#stage");
    const width = Math.max(st.clientWidth - 40, 200) / ((this.last?.w ?? 595) * (96 / 72));
    if (z === "fit") return width;
    return Math.min(width, Math.max(st.clientHeight - 24, 100) / ((this.last?.h ?? 842) * (96 / 72)));
  }

    goPage(k: number): void {
    this.viewer.goTo(k);
  }

  /** One zoom step in or out, from where it is (as Overleaf's − +). */
  zoomStep(dir: 1 | -1): void {
    const steps = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];
    const now = this.scale();
    const next = dir > 0 ? steps.find((z) => z > now + 0.01) : [...steps].reverse().find((z) => z < now - 0.01);
    this.zoomTo(String(next ?? now));
  }

  /** The contents beside the pages (PanelOptions.outline). */
  private contents?: Outline;

  /** Whether the contents are shown: the session asks for the outline after a build only then. */
  outlineWanted(): boolean {
    return !!this.contents && this.win.classList.contains("side-on");
  }

  /** The document's outline (its PDF's bookmarks), from the host after each build. */
  outline(items: Entry[]): void {
    this.contents?.set(items);
    this.contents?.at(this.at);
    this.noOutline(!items.length);
  }

  /** No outline (none yet, or a PDF without one): the column hidden, the menu's Contents saying so. */
  private noOutline(none: boolean): void {
    const was = this.win.classList.contains("side-empty");
    this.win.classList.toggle("side-empty", none);
    // (the column came or went: a fitted page takes the width it leaves)
    if (was !== none && this.win.classList.contains("side-on") && this.prefs.zoom === "fit") this.redraw();
    const label = this.$("#x-contents").querySelector("span:not(.sub)");
    if (label) label.textContent = none ? "Contents (none)" : "Contents";
    this.$("#x-contents").title = none ? "This PDF has no outline: hyperref makes one from the sections" : "";
  }

  /** The contents shown or hidden (remembered on this machine). */
  side(on = !this.win.classList.contains("side-on")): void {
    if (!this.contents) return;
    this.sideOn(on);
    // (the outline is asked for only while it is shown: opened, now)
    if (on) this.ev.onContents?.();
    try {
      localStorage.setItem("phitex.side", on ? "1" : "0");
    } catch {
      /* (no storage: not remembered) */
    }
    if (this.prefs.zoom === "fit") this.redraw();
  }

  /** The column's class and the bar's button pressed with it. */
  private sideOn(on: boolean): void {
    this.win.classList.toggle("side-on", on);
    this.$("#b-side").setAttribute("aria-pressed", String(on));
  }

  /** A button of the host's in the bar, before find (the CLI's own). */
  headerButton(label: string, title: string, onclick: (b: HTMLButtonElement) => void): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "ib wide";
    b.title = title;
    b.textContent = label;
    b.onclick = () => onclick(b);
    this.$("#b-find").before(b);
    return b;
  }

  /** The contents and the keys, as the host asked (PanelOptions). */
  /** The tips shown by themselves (PanelOptions.tips). */
  private autoTips = true;

  private extras(opts: PanelOptions): void {
    this.autoTips = opts.tips !== false;
    if (opts.header) this.win.classList.add("own-header");
    this.findAndZoom(!!opts.keys);
    if (opts.outline) {
      this.win.classList.add("has-side");
      this.contents = new Outline(this.$("#side"), { goToPlace: (k, top) => this.viewer.goToPlace(k, top) });
      let on = true;
      try {
        on = localStorage.getItem("phitex.side") !== "0";
      } catch {
        /* (shown) */
      }
      this.sideOn(on);
      // (no outline until one arrives: no empty column while the first build runs)
      this.noOutline(true);
    }
    if (!opts.keys) return;
    const help = this.$("#keyhelp");
    const extra: [string, string][] = [["e", "the build's errors and warnings"], ...(opts.outline ? ([["t", "the contents beside the pages"]] as [string, string][]) : [])];
    help.innerHTML = `<header>Keyboard shortcuts<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><table>${[...KEYS, ...extra].map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
    // (the problems' and the contents' keys first: Esc closes the list before anything else)
    addEventListener(
      "keydown",
      (e) => {
        if (e.ctrlKey || e.altKey || e.metaKey || (e.target as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable=true]")) return;
        if (e.key === "Escape" && this.escapeProblems()) {
          /* (the list closed) */
        } else if (e.key === "e") this.diagnostics();
        else if (e.key === "t" && this.contents) this.side();
        else return;
        e.preventDefault();
        e.stopImmediatePropagation();
      },
      { capture: true },
    );
    const stage = this.$("#stage");
    const pageH = () => {
      return (this.last?.h ?? 792) * (96 / 72);
    };
    const panel = this;
    bindKeys(window, {
      get pages() {
        return panel.viewer.pages;
      },
      get page() {
        return panel.viewer.page;
      },
      goTo: (k) => this.viewer.goTo(k),
      turn: (n) => this.viewer.turn(n),
      scroller: stage,
      zoom: (f) => this.zoomTo(String(Math.min(5, Math.max(0.25, this.scale() * f)))),
      fitWidth: () => this.zoomTo("fit"),
      fitPage: () => this.zoomTo(String(Math.max(0.2, Math.min((stage.clientHeight - 24) / pageH(), this.scale())))),
      help: (show) => {
        help.hidden = !show;
      },
    });
  }

  /** The problems last closed by hand (not opened by themselves again until they change), and those listed. */
  private dismissed = "";
  private shownKey = "";

  private problemsKey(): string {
    return JSON.stringify((this.lastStatus?.diagnostics ?? []).map((d) => [d.severity, d.message, d.file, d.line]));
  }

  /** The problems' list opened or closed. */
  diagnostics(on = this.$("#logs").hidden === true): void {
    const was = !this.$("#logs").hidden;
    if (on && !(this.lastStatus?.diagnostics ?? []).length && !was) on = false;
    if (on) this.menu(null);
    this.$("#logs").hidden = !on;
    this.$("#b-probs").setAttribute("aria-expanded", String(on));
    this.win.classList.toggle("diags-open", on);
    if (was && !on) this.dismissed = this.problemsKey();
    if (was !== on) this.emit();
  }

  /** Esc on the problems: the list closed; false when it wasn't open. */
  private escapeProblems(): boolean {
    if (this.$("#logs").hidden) return false;
    this.diagnostics(false);
    return true;
  }

  prev(): void {
    this.viewer.goTo(this.at - 1);
  }
  next(): void {
    this.viewer.goTo(this.at + 1);
  }
  clean(): void {
    this.again();
    this.ev.onClean();
  }

  pdf(): void {
    this.ev.onPdf();
  }

  zoomTo(z: string): void {
    this.zoomNow = null;
    this.prefs.zoom = z;
    this.zoomShown();
    this.save();
    this.redraw();
    this.emit();
  }

  /** The zoom menu at the zoom now: a zoom not on it (a pinch's) by its percentage. */
  private zoomShown(): void {
    const z = this.zoomNow ?? this.prefs.zoom;
    const named = ZOOMS.find(([v]) => v === z);
    this.$("#zl").textContent = named ? named[1] : `${Math.round(Number(z) * 100)}%`;
    // (the short form a narrow bar shows)
    this.$("#zs").textContent = z === "fit" ? "Fit" : z === "page" ? "Page" : `${Math.round(Number(z) * 100)}%`;
    this.root.querySelectorAll<HTMLElement>("#m-zoom [data-zoom]").forEach((b) => {
      const on = b.dataset.zoom === z;
      b.setAttribute("aria-checked", String(on));
      b.querySelector(".check")!.textContent = on ? "✓" : "";
    });
  }

    /**
   * A zoom gesture under way (Ctrl+wheel, a pinch): the zoom it started
   * at, the factor so far, the place under the pointer when it began
   * (client pixels, and on its page), and when it ends.
   */
  private gesture:
    | { from: number; f: number; x: number; y: number; at: { k: number; fx: number; fy: number } | null; end?: ReturnType<typeof setTimeout> }
    | undefined;

  /**
   * Zoom by `f` keeping the point (`px`, `py`, from the stage's top left)
   * where it is, as pdf.js's and hayro's viewers do: while the gesture goes
   * on, the pages as they are scaled by one transform around the pointer
   * (no layout, no drawing: the pages, their marks and their text scale
   * together); once it rests (250 ms, longer than a wheel's notches are
   * apart), the zoom set once, the place kept under the pointer by its
   * page, and the pages drawn once at it.
   */
  private zoomAt(f: number, px: number, py: number): void {
    const stage = this.$("#stage"), view = this.$("#viewer");
    const r = stage.getBoundingClientRect();
    let g = this.gesture;
    if (!g) {
      const x = r.left + px, y = r.top + py;
      const v = view.getBoundingClientRect();
      view.style.transformOrigin = `${x - v.left}px ${y - v.top}px`;
      g = this.gesture = { from: this.scale(), f: 1, x, y, at: this.viewer.pointAt(x, y) };
    }
    g.f = Math.min(8, Math.max(0.25, g.from * g.f * f)) / g.from;
    view.style.transform = `scale(${g.f})`;
    view.style.willChange = "transform";
    // (the zoom itself set once it rests: a page drawn meanwhile is sized
    // at the zoom the transform scales)
    clearTimeout(g.end);
    g.end = setTimeout(() => this.zoomEnd(), 250);
  }

  /** The zoom gesture rested: the pages at the zoom, the place under the pointer kept, drawn once. */
  private zoomEnd(): void {
    const g = this.gesture;
    if (!g) return;
    this.gesture = undefined;
    const view = this.$("#viewer");
    view.style.transform = "";
    view.style.willChange = "";
    this.prefs.zoom = String(Number((g.from * g.f).toFixed(4)));
    this.zoomShown();
    this.$("#stage").classList.remove("fit");
    this.viewer.redraw(true);
    if (g.at) this.viewer.keepAt(g.at, g.x, g.y);
    this.viewer.redraw();
    this.save();
    this.emit();
  }

  /** The find field's matches, the one shown, and the search asked last. */
  private hits: { k: number; boxes: [number, number, number, number][] }[] = [];
  private hit = 0;
  private findSeq = 0;
  private findTimer: ReturnType<typeof setTimeout> | undefined;
  /** The pointer over the panel (in a host's page, its keys only then). */
  private hover = false;

  /** The find field opened (its text selected) or closed (the marks gone). */
  find(on: boolean): void {
    const box = this.$("#find");
    const q = this.$<HTMLInputElement>("#findq");
    box.hidden = !on;
    if (on) {
      q.focus();
      q.select();
      if (q.value) this.search(q.value);
      return;
    }
    this.findSeq++;
    this.hits = [];
    this.viewer.found([], 0);
    q.blur();
  }

  private search(q: string): void {
    const seq = ++this.findSeq;
    const n = this.$("#findn");
    if (!q.trim()) {
      this.hits = [];
      this.viewer.found([], 0);
      n.textContent = "";
      return;
    }
    void this.ev.onFind?.(q).then((hits) => {
      if (seq !== this.findSeq) return;
      this.hits = hits;
      // (the first match at or after the page in view)
      const at = hits.findIndex((h) => h.k >= this.viewer.page);
      this.hit = at < 0 ? 0 : at;
      this.showHit();
    });
  }

  private showHit(): void {
    const n = this.$("#findn");
    n.textContent = this.hits.length ? `${this.hit + 1} / ${this.hits.length}` : "0 / 0";
    n.classList.toggle("none", !this.hits.length);
    this.viewer.found(this.hits, this.hit);
  }

  /** Matches found by the host (a diff's change): marked, `cur` brought into view; [] clears. */
  reveal(hits: { k: number; boxes: [number, number, number, number][] }[], cur = 0): void {
    this.viewer.found(hits, cur);
  }

  /** The next match (`dir` 1) or the one before (−1), round the ends. */
  findNext(dir: 1 | -1): void {
    if (!this.hits.length) return;
    this.hit = (this.hit + dir + this.hits.length) % this.hits.length;
    this.showHit();
  }

  /** Find, and zoom by the wheel and its keys: in every host (`all`: the page's keys are the panel's). */
  private findAndZoom(all: boolean): void {
    const stage = this.$("#stage");
    this.win.addEventListener("pointerenter", () => (this.hover = true));
    this.win.addEventListener("pointerleave", () => (this.hover = false));
    // (Ctrl+wheel, a trackpad's pinch too: the pages zoom at the pointer, not the browser's page)
    stage.addEventListener(
      "wheel",
      (e) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        const r = stage.getBoundingClientRect();
        // (a mouse wheel's notch, lines or a whole step of pixels: one step
        // of 10%, as pdf.js's; a trackpad's pinch, small pixel deltas: in
        // proportion, as it goes)
        const notch = e.deltaMode !== 0 || Math.abs(e.deltaY) >= 50;
        const f = notch ? (e.deltaY < 0 ? 1.1 : 1 / 1.1) : Math.exp(-e.deltaY / 100);
        this.zoomAt(f, e.clientX - r.left, e.clientY - r.top);
      },
      { passive: false },
    );
    const q = this.$<HTMLInputElement>("#findq");
    q.addEventListener("input", () => {
      clearTimeout(this.findTimer);
      this.findTimer = setTimeout(() => this.search(q.value), 120);
    });
    q.addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.findNext(e.shiftKey ? -1 : 1);
      else if (e.key === "Escape") this.find(false);
      else return;
      e.preventDefault();
      e.stopPropagation();
    });
    addEventListener(
      "keydown",
      (e) => {
        const ours = all || this.hover || this.root.activeElement != null;
        if (!ours || e.altKey || e.metaKey) return;
        const typing = (e.target as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable=true]") && e.composedPath()[0] !== q;
        if (e.ctrlKey) {
          const k = e.key;
          if (k === "f" && this.ev.onFind) this.find(true);
          else if (k === "=" || k === "+") this.zoomStep(1);
          else if (k === "-") this.zoomStep(-1);
          else if (k === "0") this.zoomTo("fit");
          else return;
        } else if (typing || e.composedPath()[0] === q) return;
        else if (e.key === "/" && this.ev.onFind) this.find(true);
        else if ((e.key === "n" || e.key === "N") && !this.$("#find").hidden && this.hits.length) this.findNext(e.key === "n" ? 1 : -1);
        else if (e.key === "Escape" && !this.$("#find").hidden) this.find(false);
        else return;
        e.preventDefault();
        e.stopImmediatePropagation();
      },
      { capture: true },
    );
  }

  toggleDebug(): void {
    this.debugOn = !this.debugOn;
    this.$("#dbg").setAttribute("aria-checked", String(this.debugOn));
    this.$("#dbg").querySelector(".check")!.textContent = this.debugOn ? "✓" : "";
    this.$<HTMLInputElement>("#dbg2").checked = this.debugOn;
    this.ev.onDebug?.(this.debugOn);
    this.msg(this.debugOn ? "Debug check on" : "Debug check off", false, true);
    this.emit();
  }

  /** The settings (docked under a host's toolbar: the main file, the debug check, reload, the latency's details). */
  sheet(on = this.$("#sheet").hidden): void {
    this.$("#sheet").hidden = !(on && !!this.docked);
    this.emit();
  }

  /** Repaints so far: the first is the first build's. */
  private paints = 0;

  /** Kept for hosts that call it: the speed is the bar's status now. */
  speedChip(_on: boolean): void {}

  /** A repaint reached the screen, `ms` after its keystroke: the status says how fast. */
  painted(ms: number): void {
    // (the first paint is the first build's: downloads, the format, the whole document; the status says so, not "Built · 14906 ms")
    this.speed = { ms, at: performance.now(), first: this.paints++ === 0 };
    this.renderStatus();
    this.emit();
  }

  // ---- Dark PDF: the user's choice, Off by default (a static filter: it inverts figures too) ----

  private darkPref(): DarkPdf {
    try {
      const d = localStorage.getItem("phitex.darkpdf");
      return d === "on" || d === "theme" ? d : "off";
    } catch {
      return "off";
    }
  }

  private dark: DarkPdf = "off";
  private darkQuery = matchMedia("(prefers-color-scheme: dark)");

  /** Dark pages off, on, or as the host's theme is; remembered on this machine. */
  darkPdf(d: DarkPdf, save = true): void {
    this.dark = d;
    if (save)
      try {
        localStorage.setItem("phitex.darkpdf", d);
      } catch {
        /* (not kept) */
      }
    this.root.querySelectorAll<HTMLElement>("#darkpdf [data-dark]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.dark === d)));
    this.darkQuery.onchange = () => this.applyDark();
    this.applyDark();
  }

  /** The host page's own theme (theme()): undefined, never said. */
  private pageTheme: "dark" | "light" | null | undefined = undefined;

  /**
   * The host page's theme: "dark" or "light" as its own toggle says, null
   * to follow the system (prefers-color-scheme). For hosts whose page has a
   * toggle of its own (the web frontend); Overleaf's comes from dock(), VS
   * Code's from its body.
   */
  theme(t: "dark" | "light" | null): void {
    this.pageTheme = t;
    this.win.classList.toggle("dark", t === "dark");
    this.win.classList.toggle("light", t === "light");
    this.applyDark();
  }

  /** Whether the host's theme is dark: Overleaf's (dock() says), VS Code's (its body's class), the page's (theme()), the system's. */
  private hostDark(): boolean {
    if (this.kind === "overleaf") return !this.win.classList.contains("light");
    if (this.kind === "vscode") return !/\bvscode-light\b/.test(document.body.className);
    return this.pageTheme ? this.pageTheme === "dark" : this.darkQuery.matches;
  }

  private applyDark(): void {
    this.win.classList.toggle("dark-pdf", this.dark === "on" || (this.dark === "theme" && this.hostDark()));
  }

  // ---- Feature tips, each once (own-bar hosts; Overleaf's toolbar has the extension's) ----

  private tipAt = -1;
  private tipList: typeof TIPS = [];

  /** The tips not seen yet (`all`: every one, What's new), from the first. */
  private tips(all: boolean): void {
    if (!this.ownBar || this.prefs.collapsed || (!all && !this.autoTips)) return;
    let seen: string[] = [];
    try {
      seen = JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]");
    } catch {
      /* (none seen) */
    }
    const offered = TIPS.filter((t) => !(t.id === "compare" && !this.ev.onCompare) && !(t.id === "contents" && !this.contents));
    this.tipList = all ? offered : offered.filter((t) => !seen.includes(t.id));
    this.tip(0);
  }

  private tip(i: number): void {
    const el = this.$("#tip");
    this.root.querySelectorAll(".ringed").forEach((e) => e.classList.remove("ringed"));
    this.tipAt = i;
    const t = this.tipList[i];
    if (!t) return void (el.hidden = true);
    try {
      const seen = new Set<string>(JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]"));
      seen.add(t.id);
      localStorage.setItem("phitex.tips.seen", JSON.stringify([...seen]));
    } catch {
      /* (shown again next time) */
    }
    let target = this.$(t.at);
    // (a control hidden at this width: the menu that holds it)
    if (!target || !target.offsetParent) target = this.$("#b-more");
    target.classList.add("ringed");
    el.innerHTML =
      `<h4>${t.fresh ? '<span class="new">New</span>' : ""}${esc(t.title)}</h4>${t.demo}<p>${esc(t.text)}</p>` +
      `<div class="acts"><span class="n">${i + 1} of ${this.tipList.length}</span><button type="button" data-t="skip">Skip</button><button type="button" class="primary" data-t="next">${i + 1 < this.tipList.length ? "Next" : "Got it"}</button></div>`;
    el.hidden = false;
    const bar = this.$(".bar").getBoundingClientRect(), r = target.getBoundingClientRect(), w = el.offsetWidth;
    const cx = r.left + r.width / 2 - bar.left, left = Math.max(8, Math.min(bar.width - w - 8, cx - w / 2));
    Object.assign(el.style, { left: `${left}px`, top: `${r.bottom - bar.top + 9}px` });
    el.style.setProperty("--ax", `${cx - left}px`);
    el.onclick = (e) => {
      e.stopPropagation();
      const a = (e.target as HTMLElement).closest<HTMLElement>("[data-t]")?.dataset.t;
      if (a) this.tip(a === "next" ? i + 1 : -1);
    };
  }

    /**
   * The diff's bar over the page (null: none): against which version, the
   * changes and the one at, and its controls. `showing` "diff" or "current":
   * which of the two the page is (toggled with "d").
   */
  diffBar(d: DiffBar | null, on?: DiffBarActions): void {
    const el = this.$("#diffbar");
    if (!d) {
      el.className = "diffbar";
      el.innerHTML = "";
      return;
    }
    const esc = (t: string) => t.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
    const n = d.busy ? `<span class="n busy">${esc(d.busy)}</span>` : `<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    el.innerHTML =
      `<span class="what" title="${esc(d.title)}">Diff vs ${esc(d.title)} <small>${esc(d.when)}</small></span>` +
      `<button data-a="prev" title="Previous change (Shift+N)" ${d.busy || !d.changes ? "disabled" : ""}>‹</button>${n}<button data-a="next" title="Next change (N)" ${d.busy || !d.changes ? "disabled" : ""}>›</button>` +
      `<span class="sep"></span><button data-a="toggle" class="${d.showing === "current" ? "cur" : ""}" title="Show the ${d.showing === "diff" ? "current version" : "diff"} (D)">${d.showing === "diff" ? "Current" : "Diff"}</button>` +
      `<button data-a="pdf" title="Download the diff PDF" ${d.busy ? "disabled" : ""}>⬇ PDF</button><button data-a="tex" title="Download diff.tex" ${d.busy ? "disabled" : ""}>⬇ .tex</button>` +
      `<button data-a="settings" title="Diff style: colors, markup" aria-label="Diff style">⚙</button>` +
      `<button data-a="close" title="Stop comparing" aria-label="Stop comparing">✕</button>`;
    el.className = "diffbar on";
    el.onclick = (e) => {
      const a = (e.target as HTMLElement).closest<HTMLElement>("button[data-a]")?.dataset.a as keyof DiffBarActions | undefined;
      if (a) on?.[a]?.();
    };
  }

  /**
   * The diff's style, a popover under the bar (null: shut): latexdiff's
   * markup and subtype, and the two colors, `change` called on each pick.
   */
  diffSettings(look: DiffLook | null, change?: (l: DiffLook) => void, defaults?: DiffLook): void {
    const el = this.$("#diffset");
    if (!look) {
      el.className = "diffset";
      return;
    }
    // (the look comes from a host, a store or a server: its colors go into
    // attributes, so only #rrggbb is taken, its names only as the lists have them)
    look = { ...look, add_color: safeColor(look.add_color, "#0000ff"), del_color: safeColor(look.del_color, "#ff0000") };
    const MARKUP: [string, string][] = [
      ["underline", "Underline"], ["ctraditional", "Color + font"], ["traditional", "Font"], ["cfont", "Color + size"],
      ["fontstrike", "Strike"], ["bold", "Bold"], ["changebar", "Change bars"], ["culinechbar", "Underline + bars"], ["invisible", "Hide deletions"],
    ];
    const SUB: [string, string][] = [["safe", "Plain"], ["color", "Colored"], ["marker", "Margin marks"]];
    const PAIRS: [string, string, string][] = [
      ["#0000ff", "#ff0000", "Blue / red"], ["#15803d", "#b91c1c", "Green / red"], ["#0072b2", "#d55e00", "Colorblind-safe"], ["#7e22ce", "#6b7280", "Purple / gray"],
    ];
    const seg = (key: "markup" | "subtype", opts: [string, string][]) =>
      `<div class="opts">${opts.map(([v, t]) => `<button data-k="${key}" data-v="${v}" aria-pressed="${look[key] === v}">${t}</button>`).join("")}</div>`;
    const deco = (add: boolean) => {
      const c = add ? look.add_color : look.del_color;
      if (look.markup === "invisible" && !add) return "display:none";
      const strike = !add && /^(underline|fontstrike|culinechbar)$/.test(look.markup);
      const line = add && /^(underline|culinechbar)$/.test(look.markup) ? `text-decoration: underline wavy ${c}; text-underline-offset: 3px;` : "";
      return `color:${c};${strike ? `text-decoration: line-through ${c};` : ""}${line}${look.markup === "bold" && add ? "font-weight:700;" : ""}`;
    };
    el.innerHTML =
      `<h4>Style</h4>${seg("markup", MARKUP)}` +
      `<h4>Colors</h4><div class="opts">${PAIRS.map(([a, d, t]) => `<button class="pair" data-pair="${a},${d}" aria-pressed="${look.add_color === a && look.del_color === d}"><i style="background:${a}"></i><i style="background:${d}"></i>${t}</button>`).join("")}</div>` +
      `<div class="row"><span>Added</span><input type="color" data-c="add_color" value="${look.add_color}"><span>Deleted</span><input type="color" data-c="del_color" value="${look.del_color}"></div>` +
      `<h4>Mode</h4>${seg("subtype", SUB)}` +
      `<div class="sample">The results are <span style="${deco(false)}">good</span> <span style="${deco(true)}">excellent</span>.</div>` +
      `<div class="foot"><small>As latexdiff's --type, --subtype</small><button data-reset="1">Reset</button></div>`;
    el.className = "diffset on";
    const set = (l: Partial<DiffLook>) => {
      const next = { ...look, ...l };
      change?.(next);
      this.diffSettings(next, change, defaults);
    };
    el.onclick = (e) => {
      const b = (e.target as HTMLElement).closest<HTMLElement>("button");
      if (!b) return;
      if (b.dataset.reset && defaults) return set(defaults);
      if (b.dataset.pair) {
        const [a, d] = b.dataset.pair.split(",");
        return set({ add_color: a, del_color: d });
      }
      if (b.dataset.k) set({ [b.dataset.k]: b.dataset.v } as Partial<DiffLook>);
    };
    el.onchange = (e) => {
      const i = e.target as HTMLInputElement;
      if (i.dataset.c) set({ [i.dataset.c]: i.value } as Partial<DiffLook>);
    };
  }

  /** Between the first paint and instant typing (since when); the status says so, or the corner where there is no bar. */
  private warm: { since: number } | null = null;
  private warmTimer?: ReturnType<typeof setInterval>;
  /** Startup: the first paint is in, instant typing still being prepared (the time going); then "ready", briefly. */
  warmup(w: { state: "preparing" | "ready"; since?: number; slow?: boolean }): void {
    clearInterval(this.warmTimer);
    if (w.state === "ready") {
      this.warm = null;
      this.renderStatus();
      if (!this.ownBar) this.toast("warm", { title: "Instant typing ready", pct: null, ms: 2500 });
      return;
    }
    this.warm = { since: w.since ?? Date.now() };
    const tick = () => {
      this.renderStatus();
      if (this.ownBar || !this.warm) return;
      const s = Math.round((Date.now() - this.warm.since) / 1000);
      this.toast("warm", { title: "Preparing instant typing…", detail: `${s} s${w.slow ? " · edits are slower until it is ready" : ""}`, pct: null });
    };
    tick();
    this.warmTimer = setInterval(tick, 1000);
  }

  /** The last build's numbers, as the session says them: a line, and the lines behind it. */
  private lat: { summary: string; details?: string } | null = null;

  latency(summary: string, details?: string): void {
    this.lat = { summary, details: details ?? this.lat?.details };
    this.$("#details").textContent = details ?? summary;
    if (!this.$("#m-time").hidden) this.timing();
  }

  /** The last build's parts as the session lists them: [what, how long]. */
  private timingParts(): [string, string][] {
    return (this.lat?.details ?? "")
      .split("\n")
      .filter(Boolean)
      .map((l) => l.trim().split(/\s{2,}/))
      .map(([k, ...v]): [string, string] => [k ?? "", v.join(" ")]);
  }

  /** The timing popover: the last build's line, its parts as the session lists them, the last message. */
  private timing(): void {
    const el = this.$("#m-time");
    const rows = this.timingParts()
      .map(([k, v]) => `<tr><td>${esc(k)}</td><td>${esc(v)}</td></tr>`)
      .join("");
    const ms = this.speed ? `${this.speed.ms < 10 ? this.speed.ms.toFixed(1) : Math.round(this.speed.ms)} ms` : "";
    el.innerHTML =
      `<h3>${this.speed?.first ? `The first build: ${(this.speed.ms / 1000).toFixed(1)} s` : ms ? `Keystroke to page: ${ms}` : "The last build"}</h3>` +
      (rows ? `<table>${rows}</table>` : "") +
      `<div class="foot"></div><div class="foot msg${this.msgErr ? " err" : ""}"></div>`;
    el.querySelector(".foot")!.textContent = this.lat?.summary ?? "No build yet.";
    el.querySelector(".msg")!.textContent = this.msgText;
  }

  /** The last message (connected, files read, a debug check's result), shown in the timing popover. */
  private msgText = "";
  private msgErr = false;

  /**
   * The status's message (in its popover); `shown`: the answer to something
   * the user did (Clear cache, a toggle), so a note in the corner says it
   * too, for a few seconds, or until clicked away if it is an error.
   */
  msg(t: string, err = false, shown = false): void {
    this.msgText = t;
    this.msgErr = err;
    if (!this.$("#m-time").hidden) this.timing();
    if (shown) this.toast("msg", { title: t, pct: null, icon: err ? "warning" : "info", err, ms: err ? undefined : 4000 });
  }

  error(t: string): void {
    this.msg(t, true);
    // (before any page: a card says it stopped, and why, not "Reading" forever)
    if (!this.last && !this.pkgBusy) {
      const card = this.card();
      card.querySelector("h3")!.textContent = "Couldn't open the project";
      card.querySelector(".note")!.textContent = this.words.openFailed;
      card.querySelector(".count")!.textContent = t;
      if (this.ev.onReport) {
        const b = document.createElement("button");
        b.className = "reportbtn";
        b.textContent = "Report a problem…";
        b.onclick = () => void this.report();
        card.append(b);
      }
      return;
    }
    // (with pages: in the corner, a while; a report a click away)
    this.toast("msg", { title: "Something went wrong", detail: t, pct: null, icon: "error", err: true, ms: 8000 });
    if (this.ev.onReport) {
      const el = this.$("#toast");
      el.style.cursor = "pointer";
      el.title = "Click for a debug report to send";
      el.onclick = () => ((el.onclick = null), (el.style.cursor = ""), (el.title = ""), void this.report());
    }
  }

  check(r: { ok: boolean; ms: number; mismatch?: string }): void {
    if (r.ok) this.msg(`✓ incremental = fresh build (${r.ms.toFixed(1)} ms)`);
    else this.error(`✗ mismatch vs fresh build: ${r.mismatch}`);
  }

  files(names: string[], _skipped: string[], took: string): void {
    this.msg(`${names.length} file${names.length === 1 ? "" : "s"} · ${took} · all local`);
  }

  mains(names: string[], main: string | null): void {
    const s = this.$<HTMLSelectElement>("#main");
    s.innerHTML = "";
    for (const n of names) s.append(new Option(n, n, n === main, n === main));
  }

  /** The last build's log (the host's), in a dialog over the pages. */
  private async showLog(): Promise<void> {
    const box = this.$("#logview"), pre = this.$("#logtext");
    pre.textContent = "Reading the log…";
    box.hidden = false;
    // (the focus in the dialog: Esc reaches the panel's keys and closes it, the menu that opened it being gone)
    box.querySelector<HTMLElement>("[data-close]")?.focus();
    const text = await this.ev.onLog?.().catch((e: unknown) => `Couldn't read the log: ${e instanceof Error ? e.message : String(e)}`);
    pre.textContent = text ?? "No log yet: the first build hasn't finished.";
  }

  private redraw(): void {
    this.viewer?.redraw();
  }

  /** The page box, for the page in view. */
  private nav(): void {
    const n = this.viewer.pages;
    const k = this.at;
    const pg = this.$<HTMLInputElement>("#pgin");
    if (this.root.activeElement !== pg) pg.value = n ? String(k + 1) : "–";
    this.$("#pgn").textContent = String(n);
    // (as wide as the count's digits, the most it can say)
    pg.style.width = `${String(n || 1).length + 1.6}ch`;
    queueMicrotask(() => this.emit());
  }

  /** The pages there are, by hash (the viewer draws the changed ones it shows). */
  layout(hashes: string[]): void {
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes);
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
    if (hashes.length) this.sheetDown();
    this.nav();
  }

    /** The editor to a page's source (a double-click on it). */
  goto(file: string, from: number, to: number, focus?: boolean): void {
    this.ev.onGotoRange?.(file, from, to, focus);
  }

  /** The editor's selection on the pages ([] clears). */
  marks(marks: { k: number; boxes: [number, number, number, number][] }[]): void {
    this.viewer.marks(marks);
  }

  /** Collaborators' cursors on the pages ([] clears). */
  others(marks: { k: number; boxes: [number, number, number, number][]; label: string; hue: number }[]): void {
    this.viewer.others(marks);
  }

  /** The debug report, shown whole before it is copied or emailed. */
  async report(): Promise<void> {
    const text = (await this.ev.onReport?.()) ?? "";
    const box = this.$("#report");
    this.$<HTMLTextAreaElement>("#reporttext").value = text;
    box.hidden = false;
    const copy = () => navigator.clipboard.writeText(text).catch(() => this.$<HTMLTextAreaElement>("#reporttext").select());
    this.$("#reportcopy").onclick = () => void copy().then(() => (this.$("#reportcopy").textContent = "Copied ✓"));
    const mail = this.$("#reportmail");
    mail.hidden = !REPORT_TO;
    // (a mailto body is short in most mail apps: the report goes by the clipboard, pasted)
    mail.onclick = () =>
      void copy().then(() => {
        const body = "Please paste the debug report here (it is on your clipboard), and say what you were doing:\n\n";
        window.open(`mailto:${REPORT_TO}?subject=${encodeURIComponent("PhiTeX Instant debug report")}&body=${encodeURIComponent(body)}`);
      });
    this.$("#reportclose").onclick = () => (box.hidden = true);
  }

    /** Highlight `boxes` on page `k` (the source the editor is at). */
  mark(k: number, boxes: [number, number, number, number][], scroll = true): void {
    this.viewer.mark(k, boxes, scroll);
  }

  page(img: PageImage | null, k: number, n: number, hash?: string | null): void {
    if (!img) {
      // (nothing new to show: the pages shown stay, dimmed; see stale())
      if (!this.last && !this.anim) {
        const empty = this.$("#empty");
        empty.textContent = n ? "This page is empty." : "No page shipped yet.";
        empty.hidden = false;
      }
      if (this.lastStatus) this.stale({ ...this.lastStatus, pages: n });
      return;
    }
    this.last = img;
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.$("#stage").classList.toggle("fit", (this.zoomNow ?? this.prefs.zoom) === "fit");
    // (a view with no layout yet: this page's slots, up to it)
    if (this.viewer.pages <= k) this.viewer.layout(Array.from({ length: Math.max(n, k + 1) }, (_, i) => (i === k && hash) || `?${i}`));
    this.viewer.set(k, img, hash ?? null);
    this.sheetDown();
    this.renderStatus();
    this.nav();
  }
}

type DarkPdf = "off" | "on" | "theme";

/** A long detail (an engine's dump) folded under `label`, opened on a click. */
function foldedDetail(text: string, label: string): HTMLElement {
  const d = document.createElement("details");
  d.className = "folded";
  const sum = document.createElement("summary");
  sum.textContent = label;
  const pre = document.createElement("pre");
  pre.className = "detail";
  pre.textContent = text;
  d.append(sum, pre);
  return d;
}

/** A package source worth naming: not empty, nor a placeholder. */
function known(source: string | undefined): boolean {
  return !!source && source !== "none";
}
