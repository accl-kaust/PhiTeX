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

import type { CiteAnswer, PageImage, Status } from "./session.ts";
import { sentence } from "./words.ts";
import { KINDS, kindOf, type Diagnostic, type Kind } from "./diagnostics.ts";
import type { PackageState } from "./packages.ts";
import { ENGINES, errorNeeds, type Engine } from "./engines.ts";
import { Viewer, type Cited } from "./viewer.ts";
import { VIEWER_CSS } from "./css.ts";
import { PRINT_CSS } from "./print.ts";
import { PROBLEMS_CSS, problemNode, type Problem } from "./problems.ts";
import { charOffset } from "./edits.ts";
import { html, rich } from "./html.ts";
import { OUTLINE_CSS, Outline, type Entry } from "./outline.ts";
import { KEYS, bindKeys, typing } from "./keys.ts";
import { REPORT_TO } from "./report.ts";
import type { Opening } from "./opening.ts";
import { LOOKS, lookFrom, lookOf, looksIn, play, preview as lookPreview, type OpeningLook } from "./openings/index.ts";
import { hintsFor, keyed } from "./hints.ts";

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
  settings: boolean;
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

/** A mark along the pages' scrollbar: a problem's place (`p`, its first box `box`) or a diff's change. */
type ScrollMark = { k: number; y: number; cls: string; title: string; p?: { file: string; message: string; line: number | null; excerpt: Problem["excerpt"]; severity: "error" | "warning" }; box?: [number, number, number, number] };

/** What each kind of problem is, under its switch in Settings. */
const KIND_HELP: Record<Kind, string> = {
  error: "LaTeX errors and files not found",
  overfull: "Lines that stick out into the margin",
  underfull: "Lines spread too far apart",
  reference: "Undefined references and citations",
  font: "Missing characters and substituted fonts",
  other: "Every other warning",
};

/** A row of the Settings dialog: the panel's own, or a host's (PanelEvents.settings). */
export interface SettingRow {
  id: string;
  label: string;
  /** A line under the label: what it does, where it is kept. */
  help?: string;
  /** toggle: on or off; choice: a few buttons; select: a list; picks: any of `options`, as tiles (`value` those picked, comma separated); action: a button (`button` its label); info: `value` shown. */
  kind: "toggle" | "choice" | "select" | "picks" | "action" | "info";
  /** A choice's or a list's values and labels. */
  options?: [value: string, label: string][];
  value?: boolean | string;
  button?: string;
  /** Why it can't be changed here: shown in its place, the row dimmed. */
  disabled?: string;
  /** A picks tile's picture, played in `box` while the tile is on screen: stopped by what it returns. */
  tile?(v: string, box: HTMLElement): () => void;
  /** It set (a toggle's on, a choice's value; an action: nothing): the host's error text if it could not. */
  set?(v?: boolean | string): Promise<string | void> | string | void;
}

/** A group of the Settings dialog: its title and its rows. */
export interface SettingsGroup {
  title: string;
  rows: SettingRow[];
}

/** The panel's own choices, kept by the host (PanelEvents.onPref, Panel.viewPrefs). */
export interface ViewPrefs {
  "dark-pdf": DarkPdf;
  opening: OpeningLook;
  underline: boolean;
}

export interface PanelEvents {
  /** Ctrl+click or a middle click on a link: the same viewer opened in a new tab at page `k`, `top` points down it (absent: such a click follows the link here). */
  onOpenAt?(k: number, top: number | null): void;
  /** What the citation under the pointer cites (Viewer's `cited`: its \cite's entries, from the source and the .bib). */
  onCited?(from: { k: number; x: number; y: number }, to: number, top: number | null): Cited[] | null;
  /** The page in view changed (the one an edit paints first). */
  onPage(p: number): void;
  /**
   * Page `k` is wanted: in view, not drawn at its current hash or zoom; a
   * promise settled once it is drawn (or not), so a page is asked for
   * once at a time (no promise: asked again whenever wanted).
   */
  onNeed?(k: number): void | Promise<void>;
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
  /** The host's lines for the timing popover's "Copy debug info" (PhiTeX's version, the engine, where the files come from), first. */
  onDebugInfo?(): Promise<string[]>;
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
  /** The host's rows of the Settings dialog, by group: after the panel's own in a group of the same title. */
  settings?(): SettingsGroup[] | Promise<SettingsGroup[]>;
  /**
   * One of the panel's own choices (ViewPrefs) changed: the host keeps it
   * where its other settings are (the watch: the user's phitex.toml), and
   * says its error if it could not. The panel stores none; the host gives
   * them back with viewPrefs().
   */
  onPref?<K extends keyof ViewPrefs>(key: K, value: ViewPrefs[K]): Promise<string | void> | string | void;
  /** The kinds of problem hidden (the problems' filters), every one: kept by the host (the watch: the user's phitex.toml). */
  onProblemsHidden?(kinds: string[]): void;
  /** A project file's text as built (for Copy for an AI assistant: its lines around a problem, the main file's preamble), or null. */
  onSource(file: string): Promise<string | null>;
  /** The feature tips seen, every one so far: kept by the host where every port and browser reads them (the watch: the user's phitex.toml); absent, this browser's storage alone. */
  onTipsSeen?(ids: string[]): void;
  /** The contents beside the pages opened: the outline wanted now (the host's session's `outline()`). */
  onContents?(): void;
  /** A link to a web address on a page (absent: opened in a new tab). */
  onLink?(uri: string): void;
  /** Where `q` is on the pages (the host's MuPDF searches them all): absent, no find field. */
  onFind?(q: string): Promise<{ k: number; boxes: [number, number, number, number][] }[]>;
}

/**
 * What the panel says that depends on where it runs (the defaults:
 * Overleaf's; VS Code's webview gives its own). Text, but `about`: text with
 * <b> and <code> in it (html.ts's rich), or the host's own nodes (html``),
 * a link to the licence (#license) among them.
 */
export interface PanelWords {
  /** The header's badge, and its tooltip. */
  badge: string;
  badgeTitle: string;
  /** The line under the page, and what it opens. */
  byline: string;
  about: string | Node;
  /** The strip while the first build runs past 400 ms (`{s}`: the seconds so far); empty: no strip for builds at all (the host shows them). */
  preparing: string;
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
  // (made when a panel reads it, not when this module loads: nothing of the DOM before a page)
  get about() {
    return html`<b>⚡ Instant is not part of Overleaf.</b> It is added by the <b>unofficial PhiTeX</b> browser extension, not made,
      endorsed or supported by Overleaf: an experimental incremental TeX engine that runs entirely in your browser. Nothing is
      sent anywhere; Overleaf's own PDF is on the <b>PDF</b> tab.
      <div class="about-foot">LaTeX, with TeX Live's packages fetched as needed. Free software (AGPL-3.0-only,
      <a id="license" target="_blank" rel="noopener">full license</a>), provided as is, without any warranty.
      To turn it off: <code>chrome://extensions</code>.</div>`;
  },
  preparing: "Getting ready for instant edits… {s} s (pages still scroll)",
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
  /** Links: previewed on hover; Back and Forward after a jump; where a jump lands lit; Ctrl+click to a new tab. */
  peeks: boolean;
  backs: boolean;
  flashes: boolean;
  newTabs: boolean;
  /** The documents built in this panel before (a hash of the page's address and the main file's name), newest first: opened again, no opening plays. */
  built: string[];
}

const DEFAULTS: PanelPrefs = { x: null, y: null, w: 440, h: 620, zoom: "fit", collapsed: false, details: false, peeks: true, backs: true, flashes: true, newTabs: true, built: [] };

/**
 * A page's CSS size at `cssWidth`: each side rounded up to whole device
 * pixels, as the core rounds a page's bitmap (crates/output/render:
 * `ceil` of its points times the scale), so a page drawn at the zoom
 * fills its box to the pixel. Rounded down, and the height made from the
 * rounded width, the box was up to three pixels short of the bitmap.
 */
export function pageBox(d: { w: number; h: number }, cssWidth: number, dpr = globalThis.devicePixelRatio || 1): [number, number] {
  const snap = (x: number) => Math.ceil(x * dpr) / dpr;
  return [snap(cssWidth), snap((d.h * cssWidth) / d.w)];
}


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
  copy: "M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z",
  close: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  log: "M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
  toc: "M3 9h14V7H3v2zm0 4h14v-2H3v2zm0 4h14v-2H3v2zm16 0h2v-2h-2v2zm0-10v2h2V7h-2zm0 6h2v-2h-2v2z",
  dark: "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z",
  keyboard: "M20 5H4c-1.1 0-1.99.9-1.99 2L2 17c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm-9 3h2v2h-2V8zm0 3h2v2h-2v-2zM8 8h2v2H8V8zm0 3h2v2H8v-2zm-1 2H5v-2h2v2zm0-3H5V8h2v2zm9 7H8v-2h8v2zm0-4h-2v-2h2v2zm0-3h-2V8h2v2zm3 3h-2v-2h2v2zm0-3h-2V8h2v2z",
  bolt: "M11 21h-1l1-7H7.5c-.58 0-.57-.32-.38-.66.19-.34.05-.08.07-.12C8.48 10.94 10.42 7.54 13 3h1l-1 7h3.5c.49 0 .56.33.47.51l-.07.15C12.96 17.55 11 21 11 21z",
  tips: "M9 21c0 .55.45 1 1 1h4c.55 0 1-.45 1-1v-1H9v1zm3-19C8.14 2 5 5.14 5 9c0 2.38 1.19 4.47 3 5.74V17c0 .55.45 1 1 1h6c.55 0 1-.45 1-1v-2.26c1.81-1.27 3-3.36 3-5.74 0-3.86-3.14-7-7-7z",
};
/** A problem's Copy for an AI assistant button (`i`: its row's place in the list). */
const copyBtn = (i: number) =>
  html`<button type="button" class="copyai" data-i="${i}" title="Copy for an AI assistant: this problem with its source around it, the preamble, the other problems and the log, to paste into the assistant you use (nothing is sent anywhere)" aria-label="Copy for an AI assistant">${icon("copy", 16)}</button>`;

/** An icon by name (PATHS), as an <svg>. */
const icon = (name: string, size = 18) =>
  html`<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`.firstElementChild!;

/** The zooms the menu offers: fit the width, the whole page, then percentages. */
const ZOOMS: [string, string][] = [["fit", "Page width"], ["page", "Whole page"], ["0.5", "50%"], ["0.75", "75%"], ["1", "100%"], ["1.25", "125%"], ["1.5", "150%"], ["2", "200%"], ["3", "300%"]];

/** A page's lines in a tip's demo, as wide as given (%). */
/** Bytes fetched so far, as the sheet says them (" · 3.8 MB"; nothing before any). */
const mb = (b?: number) => (b ? ` · ${b < 1048576 ? `${Math.max(1, Math.round(b / 1024))} KB` : `${(b / 1048576).toFixed(1)} MB`}` : "");

/** A duration as it reads best: 0.7 ms, 42 ms, 2.6 s, 14 s. */
const dur = (ms: number) => (ms < 10 ? `${ms.toFixed(1)} ms` : ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)} s`);

/** A time's parts as the popover draws them: a bar of them, and their names and times under it (`cycle`: colours used again past it). */
const bars = (parts: [string, number][], cycle = Infinity) =>
  html`<div class="tbar">${parts.map(([, ms], i) => html`<i class="c${i % cycle}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`)}</div><div class="legend">${parts.map(([n, ms], i) => html`<span><i class="c${i % cycle}"></i>${n} ${dur(ms)}</span>`)}</div>`;

/** The timing popover's row for the last page drawn as strips (Strips' `data-frame`): its time, split, and its strips drawn and reused. */
function framePart(json: string | undefined): DocumentFragment | "" {
  if (!json) return "";
  const f = JSON.parse(json) as { page: number; inView: boolean; drawn: number; kept: number; probe: number; draw: number; watch: number; request: number; decode: number };
  const split: [string, number][] = [["finding what changed", f.probe], ["drawing", f.draw], ["waiting for the engine", Math.max(0, f.request - f.watch)], ["decoding", f.decode]];
  return html`<div class="row"><span class="k">Drawing page ${f.page}${f.inView ? " (in view)" : ""}</span><span class="v">${dur(f.request + f.decode)}</span>${bars(split)}<span class="note">${f.drawn} slice${f.drawn === 1 ? "" : "s"} of the page drawn, ${f.kept} kept from before</span></div>`;
}

/** The last page drawn (Strips' `data-frame`) in words, for Copy debug info: framePart's numbers as a line. */
function frameText(json: string): string {
  const f = JSON.parse(json) as { page: number; inView: boolean; drawn: number; kept: number; probe: number; draw: number; watch: number; request: number; decode: number };
  return (
    `page ${f.page}${f.inView ? " (in view)" : ""} in ${dur(f.request + f.decode)}: finding what changed ${dur(f.probe)}, drawing ${dur(f.draw)}, ` +
    `waiting for the engine ${dur(Math.max(0, f.request - f.watch))}, decoding ${dur(f.decode)}; ${f.drawn} slices drawn, ${f.kept} kept`
  );
}

/** The session's names for a build's parts, as the popover says them. */
const PART_NAMES: Record<string, string> = {
  "first paint": "Typesetting this page",
  "fixed point": "Typesetting every page",
  "keystroke→page": "Keystroke to page",
  rebuilt: "Parts redone",
  batching: "Edits batched",
};

/** What each of those says, for a writer (the session's own parenthesis is for PhiTeX's developers: the debug report has it). */
const PART_NOTES: Record<string, string> = {
  "first paint": "in the engine",
  "fixed point": "the whole document, references and contents included",
  rebuilt: "",
  batching: "",
};

/** The session's names for a sum's parts ("queued 1.4 + round trip 1.0 + paint 0.7"), as the popover says them. */
const SPLIT_NAMES: Record<string, string> = {
  queued: "waiting",
  "round trip": "engine",
  paint: "drawing",
  "build and paint": "building and drawing",
};

/**
 * One of the session's timing lines ("first paint  7847.00 ms (in the core: …)",
 * "keystroke→page  2.7 ms = queued 1.4 + round trip 1.0 + paint 0.7"): its
 * label, its value said evenly, the sum's parts (a bar), the parenthesis a note.
 */
function part(key: string, raw: string): { key: string; label: string; value: string; ms?: number; split?: [string, number][]; note?: string } {
  const label = PART_NAMES[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
  const paren = /\s*\(([^)]*)\)\s*$/.exec(raw);
  let rest = paren ? raw.slice(0, paren.index) : raw;
  const note = paren?.[1];
  const num = /^([\d.]+)\s*(ms|s)\b\s*/.exec(rest);
  if (!num) return { key, label, value: rest.trim(), note };
  const ms = Number(num[1]) * (num[2] === "s" ? 1000 : 1);
  rest = rest.slice(num[0].length).trim();
  let split: [string, number][] | undefined;
  if (rest.startsWith("=")) {
    split = [...rest.slice(1).matchAll(/([a-z][a-z ]*?)\s+([\d.]+)/gi)].map((m): [string, number] => [SPLIT_NAMES[m[1].trim()] ?? m[1].trim(), Number(m[2])]);
    rest = "";
  }
  const extra = rest.replace(/^·\s*/, "");
  return { key, label, value: dur(ms), ms, split, note: [key in PART_NOTES ? PART_NOTES[key] : note, extra].filter(Boolean).join(" · ") || undefined };
}

const lines = (...w: number[]) => w.map((x) => html`<i style="width:${x}%"></i>`);

/**
 * Feature tips, each shown once (own-bar hosts): the control it points at,
 * its words, new or not, and a small looping demo of it (CSS only: .tip .demo
 * in print.ts; still, at its end state, with reduced motion).
 */
const TIPS: { id: string; at: string; title: string; text: string; demo: () => DocumentFragment; fresh?: boolean }[] = [
  {
    id: "live", at: "#st", title: "Typeset as you type",
    text: "No compile button: change the source and only what you touched is typeset again. The page follows in milliseconds.",
    // (a heading typed in the editor, the page's heading following it letter by letter, the time it took)
    demo: () => html`<div class="demo live" aria-hidden="true"><div class="ed"><span class="k">\\section{</span><span class="ty">Results</span><span class="car"></span><span class="k">}</span><i style="width:80%"></i><i style="width:60%"></i></div><div class="pg"><b>1&nbsp;&nbsp;<span class="ty">Results</span></b>${lines(95, 90, 70)}</div><span class="ms">${icon("bolt", 11)}12 ms</span></div>`,
  },
  {
    id: "compare", at: "#b-compare", title: "Compare versions", fresh: true,
    text: "See what changed since an earlier version, marked as latexdiff marks it: additions in blue, deletions struck out.",
    // (a sentence changes, then the change marked, then the navigator; Overleaf's compare tip had it first)
    demo: () => html`<div class="demo cmp" aria-hidden="true">The results are <span class="del">good</span><span class="add">&nbsp;excellent</span>.<span class="ver"><i class="v0">Submitted v1</i><i class="v1">Now vs Submitted v1</i></span><span class="tag">‹ 1 / 1 ›</span></div>`,
  },
  {
    id: "timing", at: "#st", title: "How long it took",
    text: "Click the status for the last build's times and how long a keystroke takes to reach the page.",
    demo: () => html`<div class="demo tim" aria-hidden="true"><div class="mbar"><span class="mst">Built · 42 ms</span></div><div class="mpop"><b>Keystroke to page</b><p><span>edit</span><i style="--w:30%"></i><em>11 ms</em></p><p><span>typeset</span><i style="--w:55%"></i><em>25 ms</em></p><p><span>paint</span><i style="--w:18%"></i><em>6 ms</em></p></div></div>`,
  },
  {
    id: "rebuild", at: "#b-rec", title: "Clean rebuild",
    text: "Builds run by themselves as you type. Start over from a clean slate here, or clear what PhiTeX keeps.",
    demo: () => html`<div class="demo reb" aria-hidden="true"><div class="mbar"><span class="msplit"><span class="go">${icon("refresh", 12)}Rebuild</span><span class="dd">${icon("expand_more", 12)}</span></span></div><div class="mmenu"><span>Rebuild</span><span class="pick">Clean rebuild</span></div><div class="pg">${lines(60, 95, 90, 70, 92, 50)}</div></div>`,
  },
  {
    id: "more", at: "#b-more", title: "Dark PDF and more",
    text: "The pages in dark mode, the log, the contents and the keys are in this menu.",
    demo: () => html`<div class="demo drk" aria-hidden="true"><div class="mbar"><span class="dots">${icon("more_horiz", 14)}</span></div><div class="mmenu"><span class="pick">${icon("dark", 11)}Dark PDF<b>✓</b></span><span>View log</span></div><div class="pg">${lines(55, 95, 88, 92, 60, 85)}</div></div>`,
  },
  {
    id: "contents", at: "#b-side", title: "Contents", fresh: true,
    text: "The document's sections beside the pages: click one to go there. This button (or t) shows or hides them.",
    demo: () => html`<div class="demo toc" aria-hidden="true"><div class="col"><span>1 Introduction</span><span>2 Method</span><span>3 Results</span><i class="at"></i></div><div class="pg"><div class="roll">${lines(45, 95, 90, 70)}<s></s>${lines(40, 92, 85, 60)}<s></s>${lines(42, 90, 94, 75)}</div></div></div>`,
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
  /** A page drawn in this page's life: the engine is up, a rebuild's opening starts past loading it. */
  private drewOnce = false;
  /** The step an opening says first: the one now, else loading the engine (or, once it ran here, the packages). */
  private firstStep(): string {
    return this.stepNow || this.words.steps[this.drewOnce ? 1 : 0];
  }
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
      html`<button type="button" role="menuitem" id="${id}" class="${cls || null}">${name ? html`<span class="icon">${name}</span>` : html`<span class="check"></span>`}<span>${label}</span>${sub ? html`<span class="sub">${sub}</span>` : ""}</button>`;
    const style = document.createElement("style");
    style.textContent = VIEWER_CSS + PROBLEMS_CSS + OUTLINE_CSS + PRINT_CSS;
    const skeleton = html`<div class="win ${this.kind}" role="dialog" aria-label="PhiTeX preview">
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
      <button type="button" role="menuitem" id="r-rebuild"><span class="check"></span><span>Rebuild<span class="desc">Build again with the files as they are</span></span></button>
      <hr><button type="button" role="menuitem" id="r-clean"><span class="check"></span><span>Clean rebuild<span class="desc">Start over: the project read again, built from the start</span></span></button>
      <hr class="if-compare"><button type="button" role="menuitem" id="r-compare" class="if-compare"><span class="check"></span><span>Compare with an earlier version…<span class="desc">latexdiff against a saved version</span></span></button>
      <hr class="if-cache"><button type="button" role="menuitem" id="r-cache" class="if-cache"><span class="check"></span><span>Clear cache…<span class="desc">The packages and formats PhiTeX keeps</span></span></button>
    </div>
    <div class="menu timing" id="m-time" role="dialog" aria-label="Timing" hidden style="left:6px;top:calc(100% + 4px)"></div>
    <div class="menu" id="m-zoom" role="menu" hidden style="right:60px;top:calc(100% + 4px);min-width:170px">
      ${ZOOMS.map(([z, t], i) => [i === 2 ? html`<hr>` : "", html`<button type="button" role="menuitemradio" data-zoom="${z}"><span class="check"></span>${t}</button>`])}
    </div>
    <div class="menu" id="m-more" role="menu" hidden style="right:6px;top:calc(100% + 4px);min-width:250px">
      ${item("x-pdf", "download", "Download PDF")}
      ${item("x-log", "log", "View log")}
      ${item("x-contents", "toc", "Contents", "t")}
      ${item("x-find", "search", "Find", "Ctrl+F", "narrow")}
      ${item("x-compare", "compare", "Compare…", "", "narrow")}
      <hr>
      ${item("x-settings", "tips", "Settings…")}
      ${item("x-tips", "tips", "What's new")}
      ${item("x-keys", "keyboard", "Keyboard shortcuts", "?")}
      ${item("x-about", "info", "About PhiTeX")}
    </div>
    <div class="tip" id="tip" role="dialog" aria-live="polite" hidden></div>
  </div>
  <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
  <div class="body">
    <nav class="side" id="side" aria-label="Contents"></nav>
    <div class="stagebox"><div class="stage" id="stage"><div class="empty" id="empty" hidden></div><div class="viewer" id="viewer"></div></div><div class="strips" id="strips"><div class="strip gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">✕</button></div><div class="strip approx" id="approx"></div><div class="strip banner" id="banner"></div></div></div><div class="smarks" id="smarks" aria-hidden="true"></div><div class="ctip" id="ctip" role="tooltip" hidden></div><div id="speedchip" hidden><b id="speedms"></b><span id="stripsline"></span></div>
    <div class="opening" id="opening" hidden><div class="sheet"></div><div class="steps" aria-live="polite"><div class="what"></div><div class="track unknown"><i></i></div><div class="detail"></div><div class="hint" id="hint" hidden></div></div></div>
    <div class="menu diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="menu logs" id="logs" role="dialog" aria-label="Problems" hidden><header><span id="logshead">Problems</span><span class="grow"></span><button type="button" class="ib" id="logsx" title="Close (Esc)" aria-label="Close"><span class="icon">close</span></button></header><div class="diags" id="diags" role="list"></div></div>
    <div class="dialog settings" id="settings" role="dialog" aria-label="Settings" hidden></div>
    <div class="toast" id="toast" role="status" aria-live="polite" hidden></div>
    <div class="dialog keys" id="keyhelp" role="dialog" aria-label="Keys" hidden></div>
    <div class="dialog small about" id="about" role="dialog" aria-label="About the PhiTeX preview" hidden><header>About PhiTeX<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><div>${typeof w.about === "string" ? rich(w.about) : w.about.cloneNode(true)}</div></div>
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
</div>`;
    // (the icons by name, <span class="icon">refresh</span>: inline SVG, no dependency on the page's icon font)
    for (const ic of skeleton.querySelectorAll<HTMLElement>("span.icon"))
      if (ic.className === "icon" && /^\w+$/.test(ic.textContent ?? "")) ic.replaceWith(icon(ic.textContent!, ic.textContent === "expand_more" || ic.textContent === "refresh" ? 16 : 18));
    this.root.replaceChildren(style, skeleton);
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
    // (What's new: the host's news where it has some (the watch: this version's
    // CHANGELOG entry), the new features' tips under it; else the tips)
    this.$("#x-tips").onclick = () => (this.menu(null), ev.onNews ? ev.onNews() : this.tips("fresh"));
    this.$("#x-settings").onclick = () => (this.menu(null), void this.settings(true));
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
    this.$("#stage").addEventListener("pointermove", (e) => this.culpritAt(e), { passive: true });
    this.$("#stage").addEventListener("pointerleave", () => ((this.$("#ctip").hidden = true), this.$("#stage").classList.remove("onculprit")));
    this.$("#stage").addEventListener("click", (e) => void this.culpritClicked(e));
    const license = this.$("#license") as HTMLAnchorElement | null;
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    this.$("#settings").onclick = (e) => void this.settingClicked(e);
    this.$("#settings").onchange = (e) => void this.settingClicked(e);
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#diags").onclick = (e) => {
      // (a problem's Copy for an AI assistant)
      const copy = (e.target as HTMLElement).closest<HTMLElement>(".copyai");
      if (copy) return void this.copyForAi(this.listed[Number(copy.dataset.i)]);
      // (a problem card's place)
      const at = (e.target as HTMLElement).closest<HTMLElement>(".phx-place[data-file]");
      if (at) {
        e.preventDefault();
        return ev.onGoto(at.dataset.file!, Number(at.dataset.line) || 1);
      }
      const d = (e.target as HTMLElement).closest<HTMLElement>(".diag[data-line]");
      if (d) return ev.onGoto(d.dataset.file!, Number(d.dataset.line));
      const card = (e.target as HTMLElement).closest<HTMLElement>(".phx-card[data-key]");
      const place = card && this.culpritPlaces.get(card.dataset.key!);
      if (place) this.viewer.mark(place.k, [place.box]);
      else if (card) this.msg("Not on the pages (no text of it was typeset)", false, true);
    };
    this.$("#stage").addEventListener("pointerdown", () => {
      this.menu(null);
      this.diagnostics(false);
      this.settings(false);
      this.$("#about").hidden = true;
    });
    this.root.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key !== "Escape") return;
      const open = this.root.querySelector<HTMLElement>(".dialog:not([hidden])");
      if (open) open.hidden = true;
      else if (this.menus.some(([, m]) => !this.$(m).hidden)) this.menu(null);
      else if (!this.escapeProblems()) this.unmark();
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
      peer: ev.onGotoRange ? (file, pos) => ev.onGotoRange!(file, pos, pos, true) : undefined,
      selected: (sel) => ev.onSelectPage?.(sel),
      link: ev.onLink ? (u) => ev.onLink!(u) : undefined,
      // (only where the host has a source lookup: else the viewer reads the citation from the pages)
      cited: ev.onCited ? (from, to, top) => ev.onCited!(from, to, top) : undefined,
      // (the engine's reading of a citation: the session's, given by it (citesBy); none yet, the pages')
      cites: (k, x, y) => this.citesAsk?.(k, x, y) ?? Promise.resolve(null),
      openAt: ev.onOpenAt ? (k, top) => ev.onOpenAt!(k, top) : undefined,
      // (Back by the mouse's back button and Alt+←: the tab's history in a browser tab (the watch, the demo),
      // the button's events in VS Code's webview; Overleaf's are the browser's, they leave the project)
      backWith: this.kind === "watch" ? "history" : this.kind === "vscode" ? "buttons" : undefined,
    });
    this.extras(opts);
    this.darkPdf("off");
    // (keep it on screen when the window shrinks)
    window.addEventListener("resize", () => this.place());
    // (the stage's size as last laid out, for scale(): read when it changes,
    // not on every call, where after a page's writes it laid the view out
    // again; observed before the window's, whose redraw reads it)
    const stage = this.$("#stage");
    new ResizeObserver(() => ((this.stageSize = { w: stage.clientWidth, h: stage.clientHeight }), this.drawScrollMarks())).observe(stage);
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
      this.linksShown();
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
    // (the last opening's sheet still diving into its page: gone now, so this one's sheet plays and that one's end never closes it)
    if (this.ending) this.stopSheet();
    this.reopen = true;
    this.typesetting = false;
    // (the new build's packages counted from none: the sheet says its own, not the last document's)
    this.loadPct = 0;
    this.loadDone = -1;
    // (its first page is a first build's, "First build · 5.4 s", not a keystroke's)
    this.paints = 0;
    this.walk = null;
    this.opened = false;
    this.sheetUp(true);
    // (the last build's pages hidden under it until the dive: the new page shows through the hole)
    this.win.classList.toggle("reopening", !!this.anim);
  }

  /**
   * Another document opens in this panel (a host's project switched): the
   * opening plays until the new first page, as a cold build's does. Nothing
   * of the last document stays: its pages, its status and its timing are
   * not this one's (never "last complete render" of another document).
   */
  opening(): void {
    this.last = null;
    this.lastStatus = null;
    this.speed = undefined;
    this.built = undefined;
    this.at = 0;
    clearTimeout(this.staleTimer);
    this.staleSince = 0;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout([]);
    this.again();
    this.renderStatus();
    this.nav();
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
    // (shown with no page yet: the first step said (the status), the sheet not put up: that
    // waits for a build's own step (the host reading the files, packages fetched, its
    // phases, typesetting). Put up here, it played while the panel connected to a document
    // already built (the watch's last build), the pages a moment away)
    if (on && !this.last) this.step(this.firstStep(), undefined, "", false);
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
    b.replaceChildren(icon(on ? "open_in_full" : "close_fullscreen"));
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
  /** A build of the core's (`on`) or its end: past 400 ms the status says so. */
  preparing(on: boolean): void {
    // (a host that shows its builds itself, the CLI's status bar: not said here)
    if (!this.words.preparing) return;
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
    if (s && this.firstPending()) (this.typesetting = true), this.step(this.words.steps[2], undefined, this.making ? MAKING : `Page ${s.pages + 1}`);
    this.activity();
  }

  busy(on: boolean): void {
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
      // (what the core says it does: the format, the pages it ships, else typesetting; nothing guessed from earlier builds)
      const what = this.making ? `${MAKING}…` : this.streamed ? `Typesetting page ${this.streamed + 1}…` : "Typesetting…";
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
    // (an opening up: its step (loading the compiler, fetching, typesetting), not "Building…")
    else if (this.firstPending() && this.stepNow) t = this.stepNow;
    else if (this.pkgNow) (t = "Fetching packages…"), (m = `${this.pkgNow.done} of ${this.pkgNow.total}`);
    else if (running) {
      const s = (now - (this.buildSince || this.prepSince)) / 1000;
      t = this.making ? `${MAKING}…` : this.streamed ? `Typesetting page ${this.streamed + 1}…` : "Typesetting…";
      m = s >= 1 ? `${s < 10 ? s.toFixed(1) : Math.round(s)} s` : "";
    } else if (this.warm) (t = "Preparing instant typing…"), (m = `${Math.round((Date.now() - this.warm.since) / 1000)} s`);
    else if (!this.last && this.viewer.pages === 0 && !this.$("#empty").hidden) (t = errs ? `Stopped with ${errs} error${errs === 1 ? "" : "s"}` : "No page yet"), (err = !!errs);
    // (an opening up, a rebuild's too: its step, not the last build's status)
    else if (this.reopen || (!this.last && this.viewer.pages === 0)) t = this.stepNow || "Opening…";
    // (the time, errors or not: "Built with 2 errors · 42 ms")
    else if (errs) (t = `Built with ${errs} error${errs === 1 ? "" : "s"}`), (m = this.latest() ? dur(this.latest()!.ms) : ""), (err = true);
    else (t = this.latest()?.label ?? "Built"), (m = this.latest() ? dur(this.latest()!.ms) : "");
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
    this.placeStrips();
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
  private culpritsDrawn: { file: string; range: [number, number]; severity: "error" | "warning"; message: string; line: number | null; excerpt: Problem["excerpt"] }[] = [];
  /** What the underlines drawn (or being looked up) are of: the problems and the pages' hashes. A build's every status repeats them. */
  private culpritsOf = "";
  /** The pages' hashes as last laid out. */
  private pageHashes: string[] = [];
  private async underline(diags: Diagnostic[]): Promise<void> {
    if (!this.ev.onBoxes) return;
    this.culpritDiags = diags;
    // (off: nothing of them on the pages, neither the lines nor their marks beside the scrollbar)
    if (!this.culpritsOn) return void (++this.culpritsSeq, (this.culpritsOf = ""), this.viewer.culprits([]), (this.$("#ctip").hidden = true), (this.problemMarks = []), this.drawScrollMarks());
    const want = diags
      .filter((d): d is Diagnostic & { problem: { severity: string; file: string; range: [number, number] } } => !!d.problem?.file && !!d.problem.range && d.problem.severity !== "note")
      .filter((d) => !this.hiddenKinds.has(kindOf(d)))
      .map((d) => {
        const k = kindOf(d);
        const amount = Number(/\(([\d.]+)pt too (?:wide|high)\)/.exec(d.message)?.[1]) || undefined;
        return { file: d.problem.file, range: d.problem.range, severity: d.problem.severity === "warning" ? ("warning" as const) : ("error" as const), message: d.message, rule: k === "overfull" || k === "underfull" ? k : undefined, amount, line: d.problem.line ?? d.line ?? null, excerpt: d.problem.excerpt ?? null };
      })
      .sort((a, b) => (a.severity === b.severity ? 0 : a.severity === "error" ? -1 : 1))
      .slice(0, 20);
    // (the same problems on the same pages: the same boxes, drawn already)
    const of = JSON.stringify([want, this.pageHashes, [...this.hiddenKinds]]);
    if (of === this.culpritsOf) return;
    this.culpritsOf = of;
    const seq = ++this.culpritsSeq;
    const found = await Promise.all(want.map((w, i) => this.ev.onBoxes!(w.file, w.range[0], w.range[1]).then((hits) => hits.map((h) => ({ ...h, severity: w.severity, i, rule: w.rule, amount: w.amount })), () => [])));
    // (a newer status came while these were looked up: its own call draws them)
    if (seq !== this.culpritsSeq) return;
    this.culpritsDrawn = want;
    this.viewer.culprits(found.flat());
    // (each problem's place on the pages, for its card's link and the scrollbar)
    this.culpritPlaces = new Map(want.map((w, i) => [`${w.file}\0${w.range[0]}`, found[i][0] ? { k: found[i][0].k, box: found[i][0].boxes[0] } : null]));
    this.problemMarks = want.flatMap((w, i) => (found[i][0]?.boxes[0] ? [{ k: found[i][0].k, y: found[i][0].boxes[0][1], cls: w.severity, title: w.message, p: w, box: found[i][0].boxes[0] }] : []));
    this.drawScrollMarks();
  }

  /** Each problem's first place on the pages, by its file and byte (a card's "on the page"). */
  private culpritPlaces = new Map<string, { k: number; box: [number, number, number, number] } | null>();
  /** The scrollbar's marks: the problems' places, and the diff's changes (diffMarks). */
  private problemMarks: ScrollMark[] = [];
  private changeMarks: ScrollMark[] = [];

  /** The diff's changes' places (page from 0, height in PDF points), marked along the scrollbar; none: gone. */
  diffMarks(changes: { k: number; y: number; title: string }[]): void {
    this.changeMarks = changes.map((c) => ({ ...c, cls: "change" }));
    this.drawScrollMarks();
  }

  /**
   * The marks along the pages' scrollbar, each where its place is in the
   * whole scroll: hovered, what it is (a problem: the underline's tip, its
   * line and file:line; a change: its title) beside it; clicked, the pages
   * there, a problem's place flashed.
   */
  private drawScrollMarks(): void {
    const el = this.$("#smarks");
    const marks = [...this.problemMarks, ...this.changeMarks];
    el.replaceChildren();
    el.hidden = !marks.length;
    if (!marks.length) return;
    const fr = this.viewer.fractions(marks);
    marks.forEach((m, i) => {
      if (fr[i] < 0) return;
      const t = document.createElement("i");
      t.className = m.cls;
      t.setAttribute("aria-label", m.title);
      t.style.top = `${(fr[i] * 100).toFixed(3)}%`;
      t.onmouseenter = () => this.markTip(t, m);
      t.onmouseleave = () => (this.$("#ctip").hidden = true);
      t.onclick = () => {
        this.$("#ctip").hidden = true;
        this.viewer.scrollToPlace(m.k, m.y);
        if (m.box) this.viewer.mark(m.k, [m.box], false);
      };
      el.append(t);
    });
  }

  /** A scrollbar mark's tip, left of the mark, at its height. */
  private markTip(t: HTMLElement, m: ScrollMark): void {
    const tip = this.$("#ctip");
    tip.dataset.i = "";
    tip.className = `ctip ${m.p?.severity ?? "change"}`;
    if (m.p) tip.replaceChildren(icon(m.p.severity, 14), culpritTip(m.p, "click to see it on the page"));
    else tip.replaceChildren(html`<div class="txt"><div>${m.title}</div></div>`);
    tip.hidden = false;
    const body = this.$(".body").getBoundingClientRect(), r = t.getBoundingClientRect();
    tip.style.left = `${Math.max(8, r.left - body.left - tip.offsetWidth - 10)}px`;
    tip.style.top = `${Math.max(8, Math.min(body.height - tip.offsetHeight - 8, r.top - body.top - tip.offsetHeight / 2))}px`;
  }

  /** The underlines on or off. */
  private culpritsShown(on: boolean): void {
    this.culpritsOn = on;
    void this.underline(this.culpritDiags);
  }

  /** The pages' origins came after the problems did: their underlines looked up again (the memo held an answer with no pages). */
  origins(): void {
    this.culpritsOf = "";
    void this.underline(this.culpritDiags);
  }

  /** The underline under the pointer (a few pixels round it: the line is thin), and its problem. */
  private culpritHit(e: MouseEvent): { el: HTMLElement; p: Panel["culpritsDrawn"][number] } | null {
    if (!this.culpritsDrawn.length) return null;
    const el = [...this.root.querySelectorAll<HTMLElement>(".slot .culprit")].find((m) => {
      const r = m.getBoundingClientRect();
      return e.clientX >= r.left - 2 && e.clientX <= r.right + 2 && e.clientY >= r.top - 2 && e.clientY <= r.bottom + 5;
    });
    const p = el && this.culpritsDrawn[Number(el.dataset.i)];
    return el && p ? { el, p } : null;
  }

  /**
   * The pointer over an underline: its problem under it, what it is about
   * (an undefined command's name, the message alone does not say it), its
   * line as TeX read it, the culprit marked, and where (file:line); a click
   * goes there.
   */
  private culpritAt(e: PointerEvent): void {
    const tip = this.$("#ctip");
    const hit = e.buttons ? null : this.culpritHit(e);
    this.$("#stage").classList.toggle("onculprit", !!hit);
    if (!hit) return void (tip.hidden = true);
    const { el, p } = hit;
    if (tip.hidden || tip.dataset.i !== el.dataset.i) {
      tip.dataset.i = el.dataset.i;
      tip.className = `ctip ${p.severity}`;
      tip.replaceChildren(icon(p.severity, 14), culpritTip(p));
      tip.hidden = false;
    }
    const body = this.$(".body").getBoundingClientRect(), r = el.getBoundingClientRect();
    const left = Math.max(8, Math.min(body.width - tip.offsetWidth - 8, e.clientX - body.left - 12));
    const below = r.bottom - body.top + 8;
    tip.style.left = `${left}px`;
    tip.style.top = `${below + tip.offsetHeight > body.height - 8 ? r.top - body.top - tip.offsetHeight - 6 : below}px`;
  }

  /** A click on an underline (not a drag's end, nothing selected): the editor to its culprit, selected; a host with no ranges, to its line. */
  private async culpritClicked(e: MouseEvent): Promise<void> {
    if (e.detail !== 1 || e.button !== 0) return;
    const sel = this.root.ownerDocument.getSelection();
    if (sel && !sel.isCollapsed) return;
    const hit = this.culpritHit(e);
    if (!hit) return;
    const { file, range, line } = hit.p;
    const text = this.ev.onGotoRange ? await this.ev.onSource(file).catch(() => null) : null;
    if (text != null) this.ev.onGotoRange!(file, charOffset(text, range[0]), charOffset(text, range[1]), true);
    else this.ev.onGoto(file, line ?? 1);
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
    // (the filters: each kind there is, its count, on or off; what is off is
    // gone from the list, the page's marks and the scrollbar's)
    const byKind = new Map<Kind, number>();
    for (const d of diags) byKind.set(kindOf(d), (byKind.get(kindOf(d)) ?? 0) + 1);
    if (byKind.size > 1 || this.hiddenKinds.size) {
      const f = document.createElement("div");
      f.className = "kinds";
      f.setAttribute("role", "group");
      f.setAttribute("aria-label", "Show");
      for (const [k, name] of KINDS) {
        const n = byKind.get(k);
        if (!n) continue;
        const b = document.createElement("button");
        b.type = "button";
        b.dataset.kind = k;
        b.setAttribute("aria-pressed", String(!this.hiddenKinds.has(k)));
        b.textContent = `${name} ${n}`;
        b.onclick = (e) => (e.stopPropagation(), this.toggleKind(k));
        f.append(b);
      }
      list.append(f);
    }
    // (the first `listedMax`; the rest a click away, 50 at a time or all)
    const shown = diags.filter((d) => !this.hiddenKinds.has(kindOf(d)));
    this.listed = shown.slice(0, this.listedMax);
    for (const [i, d] of this.listed.entries()) {
      // (each error and warning: its own Copy for an AI assistant)
      const copy = d.severity === "error" || d.severity === "warning" ? copyBtn(i) : null;
      // (a problem the build told whole: its card, its places links)
      if (d.problem) {
        const card = document.createElement("ol");
        card.className = "phx-card phx-inline";
        card.setAttribute("role", "listitem");
        card.replaceChildren(problemNode(d.problem));
        if (copy) card.firstElementChild!.prepend(copy);
        // (a click on the card: its place on the pages; its file:line link: the editor)
        if (d.problem.file && d.problem.range) {
          card.dataset.key = `${d.problem.file}\0${d.problem.range[0]}`;
          card.title = "Show it on the page (its file:line opens it in the editor)";
        }
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
      row.replaceChildren(html`${copy}<span class="where"></span><div class="text"></div>`);
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
    if (shown.length > this.listed.length) {
      const left = shown.length - this.listed.length;
      const more = document.createElement("div");
      more.className = "diag info more";
      const next = document.createElement("button");
      next.type = "button";
      next.textContent = left > 50 ? `Show 50 more (${left} left)` : `Show the other ${left}`;
      next.onclick = (e) => (e.stopPropagation(), this.listMore(50));
      more.append(next);
      if (left > 50) {
        const all = document.createElement("button");
        all.type = "button";
        all.textContent = "Show all";
        all.onclick = (e) => (e.stopPropagation(), this.listMore(Infinity));
        more.append(all);
      }
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
    this.$("#psev").replaceChildren(icon(errors ? "error" : "warning", 16));
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

  /** The problems listed (by the rows' data-i). */
  private listed: Diagnostic[] = [];
  /** How many problems the list shows (50, then more as asked: "Show 50 more", "Show all"). */
  private listedMax = 50;

  /** `n` more problems listed (Infinity: all), the list drawn again where it is. */
  private listMore(n: number): void {
    this.listedMax += n;
    if (this.lastStatus) this.status(this.lastStatus);
  }

  /**
   * Copy for an AI assistant: one problem with what an assistant needs to
   * explain it: its place, message, TeX's notes, help and suggestions, the
   * macros it was in, the files that included its file, its source lines
   * around it, the main file's preamble (the packages and definitions), the
   * other problems listed, and the log where it was met (and its end, for
   * an error). On the clipboard, nothing sent.
   */
  private async copyForAi(d: Diagnostic | undefined): Promise<void> {
    if (!d) return;
    const p = d.problem;
    const file = p?.file ?? d.file, line = p?.line ?? d.line;
    const main = this.lastStatus?.main ?? this.mainFiles.main;
    const source = async (f: string) => (await this.ev.onSource(f).catch(() => null))?.split("\n") ?? null;
    const numbered = (lines: string[], from: number, to: number, at?: number) =>
      lines.slice(from - 1, to).map((t, i) => `${String(from + i).padStart(4)}${from + i === at ? ">" : " "} ${t}`);
    const parts: string[] = [
      `I'm writing a LaTeX document (typeset by PhiTeX, a pdfLaTeX/XeLaTeX-compatible engine) and the build reports this ${d.severity}. What is wrong, and how do I fix it in my source?`,
      "",
      `## The ${d.severity}`,
      `${d.message}${file ? ` (${file}${line ? `, line ${line}${p?.col ? `, column ${p.col}` : ""}` : ""})` : ""}${p?.code ? ` [${p.code}]` : ""}`,
    ];
    if (p?.notes.length) parts.push(...p.notes.map((n) => `- ${n}`));
    if (p?.context.length) parts.push(`TeX was inside: ${p.context.map((m) => `\\${m.name.replace(/^\\/, "")} (${m.before}‸${m.after})`).join(" ← ")}`);
    if (p?.included.length) parts.push(`Its file was included from: ${p.included.map((i) => `${i.file}:${i.line}`).join(" ← ")}`);
    if (p?.help.length || p?.suggestions.length) parts.push(`PhiTeX's hint: ${[...(p?.help ?? []), ...(p?.suggestions ?? [])].join(" ")}`);
    // (the source around it: fifteen lines each side, the problem's marked)
    const lines = file ? await source(file) : null;
    if (lines && line) parts.push("", `## ${file}, lines around ${line}`, "```latex", ...numbered(lines, Math.max(1, line - 15), Math.min(lines.length, line + 15), line), "```");
    else if (p?.excerpt) parts.push("", "## The source line", "```latex", p.excerpt.text, "```");
    // (the main file's preamble: the class, the packages, the definitions; at most 120 lines)
    const mainLines = main ? (main === file ? lines : await source(main)) : null;
    if (main && mainLines) {
      const end = mainLines.findIndex((l) => /\\begin\s*\{document\}/.test(l));
      const upto = Math.min(end >= 0 ? end + 1 : 60, 120);
      if (!(main === file && line && line <= upto + 15)) parts.push("", `## The preamble of ${main}`, "```latex", ...numbered(mainLines, 1, upto), "```");
    }
    // (the other problems, one line each: a cause is often one of them)
    const others = this.listed.filter((x) => x !== d && (x.severity === "error" || x.severity === "warning")).slice(0, 10);
    if (others.length) parts.push("", "## Other problems in the same build", ...others.map((x) => `- ${x.severity}: ${x.message}${x.file ? ` (${x.file}${x.line ? `:${x.line}` : ""})` : ""}`));
    // (the log where TeX met it, and for an error the log's end, where TeX stopped)
    const log = (await this.ev.onLog?.().catch(() => null))?.split("\n") ?? null;
    if (log) {
      const key = d.message.replace(/^.*?:\s*/, "").slice(0, 40);
      const at = key ? log.findIndex((l) => l.includes(key)) : -1;
      if (at >= 0) parts.push("", "## The log where it was met", "```", ...log.slice(Math.max(0, at - 10), at + 15), "```");
      if (d.severity === "error" && (at < 0 || at + 15 < log.length - 40)) parts.push("", "## The end of the log", "```", ...log.slice(-40), "```");
    }
    const text = parts.join("\n");
    try {
      await navigator.clipboard.writeText(text);
      this.msg("Copied: paste it into your AI assistant", false, true);
    } catch {
      this.msg("Couldn't copy to the clipboard", true, true);
    }
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
    // (the time any package was on its way, and what came: the timing popover's Downloads)
    const now = performance.now(), d = this.downloads;
    if (p.loading.length && d.since === undefined) d.since = now;
    else if (!p.loading.length && d.since !== undefined) (d.ms += now - d.since), (d.since = undefined);
    (d.n = done), (d.bytes = p.bytes ?? 0);
    const total = done + p.loading.length;
    if (done !== this.loadDone) this.loadDone = done;
    // (never backwards: a build asking for more grows the total)
    const pct = (this.loadPct = Math.max(this.loadPct, total ? Math.round((100 * done) / total) : 100));
    // (before the first page, or a rebuild's or another document's opening: the sheet's step)
    if (this.firstPending()) {
      if (!p.loading.length && !p.building) {
        // (done: the build's own state says the rest. Files not found are optional ones LaTeX
        // only checks for (amsart.cfg, …): the problems list them, and an error says when one was needed)
        this.pkgBusy = false;
        if (this.stopped()) return;
        // (between two downloads, the build not yet begun: still fetching, not typesetting;
        // nothing downloaded at all (all kept on this machine): the step as it is)
        if (this.typesetting) this.step(this.words.steps[2], undefined, downloaded(done, p.bytes));
        else if (done) this.step(this.words.steps[1], pct, `${done} of ${total}${mb(p.bytes)}`);
        return;
      }
      this.pkgBusy = true;
      if (p.loading.length) {
        const name = p.loading[0];
        const what = `${name} · ${done} of ${total}${mb(p.bytes)}${p.unavailable.length ? ` · ${p.unavailable.length} not found` : ""}`;
        // (typesetting under way asks for a package as it reads: said under its step, not a step back to fetching and on again)
        if (this.typesetting) this.step(this.words.steps[2], undefined, `fetching ${what}`);
        else this.step(this.words.steps[1], pct, what);
      } else (this.typesetting = true), this.step(this.words.steps[2], undefined, downloaded(done, p.bytes));
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
  /** This opening's typesetting has begun (the build running, or pages shipped): a later fetch said under it. */
  private typesetting = false;

  /** The "Did you know" under the opening: one hint a few seconds, from 2 s into a first build, gone with the sheet. */
  private hintTimer: ReturnType<typeof setTimeout> | undefined;
  private hintAt = Math.floor(Math.random() * 1000);
  private hints(on: boolean): void {
    const el = this.$("#hint");
    if (!on) return void (clearTimeout(this.hintTimer), (this.hintTimer = undefined), (el.hidden = true));
    if (this.hintTimer) return;
    const list = hintsFor(this.kind);
    const show = () => {
      const text = list[this.hintAt++ % list.length];
      // (an opening that says the steps itself says this too)
      if (this.anim?.hint) this.anim.hint(text);
      else keyed(el, text, "Did you know? "), (el.hidden = false);
      this.hintTimer = setTimeout(show, 7000);
    };
    this.hintTimer = setTimeout(show, 2000);
  }

  /** The sheet's step, said; `raise`: a build's step (the sheet put up if it isn't), else said only (the panel shown, nothing built yet). */
  private step(what: string, pct: number | undefined, detail = "", raise = true): void {
    if (!this.firstPending()) return;
    this.walkTo(what);
    if (raise) this.sheetUp();
    // (the hints with the sheet: none while a first open waits to put it up)
    if (this.anim) this.hints(true);
    const el = this.$("#opening");
    this.stepNow = what;
    el.querySelector(".what")!.textContent = what;
    el.querySelector(".detail")!.textContent = detail;
    const track = el.querySelector<HTMLElement>(".track")!;
    track.classList.toggle("unknown", pct === undefined);
    track.querySelector("i")!.style.width = pct === undefined ? "" : `${pct}%`;
    this.anim?.step?.(what, pct, detail);
    this.renderStatus();
  }

  /**
   * The sheet up, the opening playing on it (once per panel: a later rebuild has pages),
   * at a build's step; `now`: a Rebuild asked for. Put up once the stage is laid out, at the
   * page's own place and size (before, it could come up at 0 x 0 in the corner).
   */
  private sheetUp(now = false): void {
    if (this.opened || this.anim || !this.wasShown) return;
    const st = this.$("#stage");
    if (st.clientWidth < 40 || st.clientHeight < 40) return void requestAnimationFrame(() => this.firstPending() && this.sheetUp(now));
    const el = this.$("#opening");
    el.hidden = false;
    el.classList.remove("in");
    this.win.classList.add("sheet-up");
    // (the opening's zoom: the whole page, unless the user chose one)
    if (this.prefs.zoom === "fit") this.zoomNow = "page";
    // (placed at the page's box at once: the sheet's transition (print.ts .opening) is for
    // its dive into the page and for the stage's resizing; from the hidden sheet's 0 x 0 at
    // the corner it grew the sheet across the stage as the opening began)
    el.style.transition = "none";
    this.fitSheet();
    void el.offsetWidth;
    el.style.transition = "";
    const dark = this.sheetDark();
    const sheet = el.querySelector<HTMLElement>(".sheet")!;
    // (the first opening ever is PhiTeX's: kept with the tips seen, the host's too)
    const first = !this.seenIds.has("opening");
    // (a document built here before, opened again (a reload, the editor reopened): its steps
    // on a still sheet, no opening played: the animation is for a document's first build.
    // A Rebuild asked for plays it)
    const warm = !now && this.prefs.built.includes(this.builtKey());
    const { id, anim } = play(warm ? "off" : this.look, first, this.lastLook, sheet, dark, (this.mainFiles.main ?? this.doc ?? "document.tex").split("/").pop()!);
    this.anim = anim;
    if (!warm) this.lastLook = id;
    if (id !== "off") this.seen(["opening"]);
    // (one that says the steps itself: the sheet's own not shown)
    el.classList.toggle("owns", !!anim.step);
    // (and the step the sheet says now, a rebuild's first: the watch says no other)
    if (anim.step) anim.step(this.firstStep(), undefined, "");
    this.animSince = performance.now();
    this.hints(true);
  }

  /** This document's key among the built ones: the page's address (the project, the watch's document) and the main file, hashed (FNV-1a). */
  private builtKey(): string {
    const loc = this.win.ownerDocument.defaultView?.location;
    const s = `${loc?.host ?? ""}${loc?.pathname ?? ""}|${this.mainFiles.main ?? this.doc ?? ""}`;
    let h = 0x811c9dc5;
    for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193);
    return (h >>> 0).toString(36);
  }

  /** This document's first page shown: built here, its next open plays no opening (the newest 200 kept). */
  private builtHere(): void {
    const k = this.builtKey(), was = this.prefs.built ?? [];
    if (was[0] === k) return;
    this.prefs.built = [k, ...was.filter((x) => x !== k)].slice(0, 200);
    this.save();
  }

  /** The pages are dark (Dark PDF): an opening's sheet is too. */
  private sheetDark(): boolean {
    return this.win.classList.contains("dark-pdf") || (this.win.classList.contains("docked") && this.win.classList.contains("pdf-dark"));
  }

  /** When the opening began: a page there within 400 ms (a project already built) is shown at once, no opening. */
  private animSince = 0;

  /** The sheet as the first page will be: an A4 page, whole in the stage, where the viewer puts page 1. */
  private fitSheet(): void {
    const el = this.$("#opening");
    if (el.hidden || this.ending) return;
    // (the first page laid out already (its size told before its drawing): its own box, the
    // sheet the page it becomes; an A4 sheet over a Letter page overhung it till the dive)
    const slot = this.$("#viewer").querySelector<HTMLElement>(".slot");
    if (slot && slot.offsetWidth > 40) {
      const r = slot.getBoundingClientRect(), b = this.$(".body").getBoundingClientRect();
      return void Object.assign(el.style, { left: `${r.left - b.left}px`, top: `${r.top - b.top}px`, width: `${r.width}px`, height: `${r.height}px` });
    }
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
    // (the dive at once, whatever the opening's act; gone 650 ms on (the longest, a window
    // maximized, 1.3 s), and at most 1.6 s on even where frames stall, a tab in the
    // background: never left over the pages)
    // (each its own sheet's: a later opening's sheet, up meanwhile, is never closed by this one's)
    const a = this.anim;
    const stop = () => this.anim === a && this.stopSheet();
    // (the first page as drawn, for an exit that becomes it: Bad Apple's morph)
    void a.done(this.$("#viewer").querySelector<HTMLCanvasElement>(".slot canvas") ?? undefined).then(stop);
    setTimeout(stop, 1600);
  }

  /** The sheet gone (played out, or a card in its place). */
  private stopSheet(): void {
    this.hints(false);
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
      this.builtHere();
      this.stepNow = "";
      this.renderStatus();
      setTimeout(() => this.tips("unseen"), 600);
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
    el.replaceChildren(html`<div class="t">${t.icon ? icon(t.icon, 16) : ""}<span></span></div><div class="d"></div>${t.pct === null ? "" : html`<div class="track"><i style="width:${t.pct}%"></i></div>`}`);
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
    this.placeStrips();
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
    const text = `${why ? sentence(why) : "Lost the connection to PhiTeX's engine."} The pages shown are from the last build.`;
    this.placeStrips();
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

  /** The strips over the pages (engine lost, stale, approximate) clear of the pages' scrollbar, whatever its width. */
  private placeStrips(): void {
    const v = this.$("#stage");
    this.$("#strips").style.right = `${Math.max(0, v.offsetWidth - v.clientWidth)}px`;
  }

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
      settings: !this.$("#settings").hidden,
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
  /** The part of page `k` on screen (the viewer's `view`): what a zoomed-in page is drawn sharp around. */
  pageView(k: number): [number, number, number, number] | null {
    return this.viewer?.view(k) ?? null;
  }

  pixelsPerPoint(): number {
    return (96 / 72) * this.scale() * (window.devicePixelRatio || 1);
  }

  /** The stage's client size, kept by its ResizeObserver (null until it is first laid out). */
  private stageSize: { w: number; h: number } | null = null;
  private scale(): number {
    const z = this.zoomNow ?? this.prefs.zoom;
    if (z !== "fit" && z !== "page") return Number(z);
    const st = (this.stageSize ??= { w: this.$("#stage").clientWidth, h: this.$("#stage").clientHeight });
    const width = Math.max(st.w - 40, 200) / ((this.last?.w ?? 595) * (96 / 72));
    if (z === "fit") return width;
    return Math.min(width, Math.max(st.h - 24, 100) / ((this.last?.h ?? 842) * (96 / 72)));
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
  /** The session's citation lookup (PreviewSink.citesBy: the engine's `cites` op), the viewer's for its citation cards. */
  private citesAsk?: (k: number, x: number, y: number) => Promise<CiteAnswer[] | null>;
  citesBy(ask: (k: number, x: number, y: number) => Promise<CiteAnswer[] | null>): void {
    this.citesAsk = ask;
  }

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
    help.replaceChildren(html`<header>Keyboard shortcuts<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><table>${[...KEYS, ...extra].map(([k, w]) => html`<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`)}</table>`);
    // (the problems' and the contents' keys first: Esc closes the list before anything else)
    addEventListener(
      "keydown",
      (e) => {
        if (e.ctrlKey || e.altKey || e.metaKey || typing(e)) return;
        if (e.key === "Escape" && (this.escapeProblems() || this.unmark())) {
          /* (the list closed, else the highlights cleared) */
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

  /** The kinds of problem not shown (the filters): this browser's, and the host's (problemsHidden). */
  private hiddenKinds = (() => {
    try {
      return new Set<Kind>(JSON.parse(localStorage.getItem("phitex.kinds.hidden") ?? "[]"));
    } catch {
      return new Set<Kind>();
    }
  })();

  /** The kinds hidden as the host keeps them (the watch: the user's phitex.toml, on every port and document). */
  problemsHidden(kinds: string[]): void {
    this.hiddenKinds = new Set(kinds.filter((k) => KINDS.some(([x]) => x === k)) as Kind[]);
    if (this.lastStatus) this.status(this.lastStatus);
  }

  /** A kind's filter clicked: shown or hidden, everywhere, and kept. */
  private toggleKind(k: Kind): void {
    if (!this.hiddenKinds.delete(k)) this.hiddenKinds.add(k);
    try {
      localStorage.setItem("phitex.kinds.hidden", JSON.stringify([...this.hiddenKinds]));
    } catch {
      /* (the host's copy only) */
    }
    this.ev.onProblemsHidden?.([...this.hiddenKinds]);
    if (this.lastStatus) this.status(this.lastStatus);
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
  /** The pages' highlights cleared (Esc), find's included: whether there were any. */
  private unmark(): boolean {
    const had = this.viewer.unmark();
    if (had) this.hits = [];
    return had;
  }

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
    | { from: number; f: number; x: number; y: number; at: { k: number; fx: number; fy: number } | null; frame: number; end?: ReturnType<typeof setTimeout> }
    | undefined;

  /**
   * Zoom by `f` keeping the point (`px`, `py`, from the stage's top left)
   * where it is, as pdf.js's viewer does: the pages laid out at the zoom
   * once a frame while the gesture goes on, the place under the pointer
   * kept by its page, and drawn at it as soon as the core can (Viewer.ask:
   * a page once at a time, the zoom it rests at last); their old bitmaps
   * stretched until then. Once it rests (250 ms, longer than a wheel's
   * notches are apart) the zoom is saved and told. The pages were scaled
   * by one transform until the gesture rested, and only then drawn: every
   * zoom showed stretched bitmaps for 250 ms and a drawing more.
   */
  private zoomAt(f: number, px: number, py: number): void {
    const r = this.$("#stage").getBoundingClientRect();
    let g = this.gesture;
    if (!g) {
      const x = r.left + px, y = r.top + py;
      g = this.gesture = { from: this.scale(), f: 1, x, y, at: this.viewer.pointAt(x, y), frame: 0 };
    }
    g.f = Math.min(8, Math.max(0.25, g.from * g.f * f)) / g.from;
    const gg = g;
    if (!g.frame) g.frame = requestAnimationFrame(() => ((gg.frame = 0), this.zoomLay(gg)));
    clearTimeout(g.end);
    g.end = setTimeout(() => this.zoomEnd(), 250);
  }

  /** The gesture's zoom now: the pages laid out at it, the place under the pointer where it was, those drawn at another zoom asked for. */
  private zoomLay(g: NonNullable<Panel["gesture"]>): void {
    this.prefs.zoom = String(Number((g.from * g.f).toFixed(4)));
    this.zoomShown();
    this.$("#stage").classList.remove("fit");
    this.viewer.redraw(true);
    if (g.at) this.viewer.keepAt(g.at, g.x, g.y);
    this.viewer.redraw();
  }

  /** The zoom gesture rested: its last frame laid out, the zoom saved and told. */
  private zoomEnd(): void {
    const g = this.gesture;
    if (!g) return;
    this.gesture = undefined;
    if (g.frame) {
      cancelAnimationFrame(g.frame);
      this.zoomLay(g);
    }
    requestAnimationFrame(() => this.drawScrollMarks());
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
    this.ev.onDebug?.(this.debugOn);
    this.msg(this.debugOn ? "Debug check on" : "Debug check off", false, true);
    this.emit();
  }

  /**
   * The timing popover under a host's own control (Overleaf's toolbar
   * status, where the panel's bar is hidden): moved out of the bar into the
   * window, at the control's left, just under it. Again: closed.
   */
  timingAt(anchor: Element): void {
    const el = this.$("#m-time");
    if (!el.hidden) return this.menu(null);
    if (el.parentElement !== this.win) this.win.append(el);
    const a = anchor.getBoundingClientRect(), w = this.win.getBoundingClientRect();
    el.style.left = `${Math.max(6, Math.min(a.left - w.left, w.width - el.offsetWidth - 6, w.width - 346))}px`;
    el.style.top = `${Math.max(4, a.bottom - w.top + 4)}px`;
    this.menu("#m-time");
  }

  // ---- Settings: the panel's own rows and the host's, one dialog in every host ----

  /** The rows drawn (by data-id): what a click or a change sets. */
  private settingRows = new Map<string, SettingRow>();

  /** The Settings dialog shown (drawn anew: the host's values as they are now) or hidden. */
  async settings(on = this.$("#settings").hidden): Promise<void> {
    const el = this.$("#settings");
    if (!on) {
      this.tilesGone();
      if (!el.hidden) (el.hidden = true), this.emit();
      return;
    }
    const groups: SettingsGroup[] = this.ownSettings();
    for (const g of (await this.ev.settings?.()) ?? []) {
      const same = groups.find((x) => x.title === g.title);
      if (same) same.rows.push(...g.rows);
      else groups.push({ title: g.title, rows: [...g.rows] });
    }
    this.settingRows.clear();
    this.tilesGone();
    // (one group at a time, by its tab: the last one shown, else the first)
    const shown = groups.filter((g) => g.rows.length);
    const at = Math.max(0, shown.findIndex((g) => g.title === this.settingsTab));
    el.replaceChildren(html`<header>Settings<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><nav class="stabs" role="tablist">${shown.map((g, i) => html`<button type="button" role="tab" data-tab="${i}" data-group="${g.title}" aria-selected="${i === at}">${g.title}</button>`)}</nav><div class="sbody"></div>`);
    const body = el.querySelector(".sbody")!;
    shown.forEach((g, i) => {
      const sec = document.createElement("section");
      sec.setAttribute("role", "tabpanel");
      sec.hidden = i !== at;
      for (const r of g.rows) sec.append(this.settingRow(r));
      body.append(sec);
    });
    el.querySelector<HTMLElement>(".stabs")!.onclick = (e) => {
      const t = (e.target as HTMLElement).closest<HTMLElement>("[data-tab]");
      if (!t) return;
      const k = Number(t.dataset.tab);
      this.settingsTab = shown[k].title;
      el.querySelectorAll<HTMLElement>("[data-tab]").forEach((b) => b.setAttribute("aria-selected", String(b === t)));
      body.querySelectorAll<HTMLElement>("section").forEach((s, i) => (s.hidden = i !== k));
    };
    el.hidden = false;
    this.emit();
  }

  /** The Settings tab last shown (its group's title). */
  private settingsTab = "";

  /** One row: its label and help, its control, a line for the host's error. */
  private settingRow(r: SettingRow): HTMLElement {
    this.settingRows.set(r.id, r);
    const row = document.createElement("div");
    row.className = `srow ${r.kind}${r.disabled ? " off" : ""}`;
    row.dataset.id = r.id;
    row.innerHTML = `<b class="lab"></b><div class="ctl"></div><small class="help"></small><div class="err" hidden></div>`;
    row.querySelector("b")!.textContent = r.label;
    row.querySelector("small")!.textContent = r.disabled ?? r.help ?? "";
    const ctl = row.querySelector(".ctl")!;
    const off = !!(r.disabled || !r.set);
    if (r.kind === "toggle") ctl.replaceChildren(html`<button type="button" role="switch" class="switch" aria-checked="${r.value === true}" aria-label="${r.label}" disabled="${off}"><i></i></button>`);
    else if (r.kind === "choice")
      ctl.replaceChildren(html`<div class="seg" role="group" aria-label="${r.label}">${(r.options ?? []).map(([v, l]) => html`<button type="button" data-v="${v}" aria-pressed="${r.value === v}" disabled="${off}">${l}</button>`)}</div>`);
    else if (r.kind === "select") {
      const s = document.createElement("select");
      s.setAttribute("aria-label", r.label);
      s.disabled = off;
      for (const [v, l] of r.options ?? []) s.append(new Option(l, v, v === r.value, v === r.value));
      ctl.append(s);
    } else if (r.kind === "picks") {
      const on = new Set(String(r.value ?? "").split(","));
      ctl.replaceChildren(html`<div class="seg" role="group" aria-label="${r.label}"><button type="button" data-all="1" disabled="${off}">All</button><button type="button" data-all="0" disabled="${off}">None</button></div>`);
      const grid = html`<div class="tiles" role="group" aria-label="${r.label}">${(r.options ?? []).map(([v, l]) => html`<button type="button" class="pick" role="checkbox" data-v="${v}" aria-checked="${String(on.has(v))}" disabled="${off}"><span class="lk"></span><span class="nm"><i class="box"></i>${l}</span></button>`)}</div>`;
      row.querySelector(".help")!.after(grid);
      if (r.tile) for (const b of row.querySelectorAll<HTMLElement>(".pick")) this.tileShown(b.querySelector(".lk")!, (box) => r.tile!(b.dataset.v!, box));
    } else if (r.kind === "action") ctl.replaceChildren(html`<button type="button" class="btn" disabled="${off}">${r.button ?? r.label}</button>`);
    else ctl.innerHTML = `<span class="val"></span>`, (ctl.querySelector(".val")!.textContent = String(r.value ?? ""));
    return row;
  }

  /** A row's control used: its new value to the row's set, its error (if any) under it. */
  private async settingClicked(e: Event): Promise<void> {
    const t = e.target as HTMLElement;
    const row = t.closest<HTMLElement>(".srow");
    const r = row && this.settingRows.get(row.dataset.id!);
    if (!row || !r?.set || r.disabled) return;
    let v: boolean | string | undefined;
    if (r.kind === "toggle" && e.type === "click" && t.closest(".switch")) v = r.value !== true;
    else if (r.kind === "choice" && e.type === "click") v = t.closest<HTMLElement>("[data-v]")?.dataset.v;
    else if (r.kind === "select" && e.type === "change") v = (t as HTMLSelectElement).value;
    else if (r.kind === "picks" && e.type === "click") {
      const all = t.closest<HTMLElement>("[data-all]")?.dataset.all, one = t.closest<HTMLElement>(".pick")?.dataset.v;
      const on = new Set(String(r.value ?? "").split(",").filter(Boolean));
      if (all) (r.options ?? []).forEach(([o]) => (all === "1" ? on.add(o) : on.delete(o)));
      else if (one) on.has(one) ? on.delete(one) : on.add(one);
      else return;
      v = (r.options ?? []).filter(([o]) => on.has(o)).map(([o]) => o).join(",");
    } else if (r.kind === "action" && e.type === "click" && t.closest(".btn")) v = undefined;
    else return;
    if (r.kind !== "action" && (v === undefined || v === r.value)) return;
    const err = row.querySelector<HTMLElement>(".err")!;
    const why = await r.set(v);
    err.hidden = !why;
    err.textContent = why || "";
    if (why) return;
    r.value = v;
    if (r.kind === "toggle") row.querySelector(".switch")!.setAttribute("aria-checked", String(v === true));
    if (r.kind === "choice") row.querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.v === v)));
    if (r.kind === "picks") {
      const on = new Set(String(v).split(","));
      row.querySelectorAll<HTMLElement>(".pick").forEach((b) => b.setAttribute("aria-checked", String(on.has(b.dataset.v!))));
    }
  }

  /** The picks tiles' pictures playing (each its stop), and what sees them on screen. */
  private tiles = new Map<Element, { play(): () => void; stop?: () => void }>();
  private tileSeen?: IntersectionObserver;

  /** `box`'s picture played while it is on screen (its tab shown, scrolled to), stopped when not (another tab, the dialog closed). */
  private tileShown(box: HTMLElement, play: (box: HTMLElement) => () => void): void {
    this.tileSeen ??= new IntersectionObserver((seen) => {
      for (const s of seen) {
        const t = this.tiles.get(s.target);
        if (!t) continue;
        if (s.isIntersecting && !t.stop) t.stop = t.play();
        else if (!s.isIntersecting && t.stop) t.stop(), (t.stop = undefined);
      }
    });
    this.tiles.set(box, { play: () => play(box) });
    this.tileSeen.observe(box);
  }

  /** Every tile's picture stopped, none watched (the dialog drawn anew, or closed). */
  private tilesGone(): void {
    this.tileSeen?.disconnect();
    for (const t of this.tiles.values()) t.stop?.();
    this.tiles.clear();
  }

  /** The panel's own rows: the main file, the pages' look, the debug check, the tips. */
  private ownSettings(): SettingsGroup[] {
    const ev = this.ev;
    // (a choice of the panel's: applied here, kept by the host)
    const pref = <K extends keyof ViewPrefs>(k: K, apply: (v: ViewPrefs[K]) => void) => async (v?: boolean | string) => {
      const why = await ev.onPref?.(k, v as ViewPrefs[K]);
      if (!why) apply(v as ViewPrefs[K]);
      return why;
    };
    const keep = ev.onPref ? undefined : "This host doesn't keep it";
    const preview: SettingRow[] = [];
    if (this.mainFiles.names.length)
      preview.push({ id: "main", label: "Main file", help: "The file PhiTeX typesets", kind: "select", options: this.mainFiles.names.map((n) => [n, n]), value: this.mainFiles.main ?? "", set: (v) => void ev.onMain(String(v)) });
    const look: SettingRow[] = [];
    look.push(
      { id: "dark-pdf", label: "Dark PDF", help: "Darker pages; it inverts figures and photos too", kind: "choice", options: [["off", "Off"], ["on", "On"], ["theme", "Match theme"]], value: this.dark, disabled: keep, set: pref("dark-pdf", (d) => this.darkPdf(d)) },
      {
        id: "opening",
        label: "Loading animation",
        help: "Played on the first page while it is typeset: one of those picked, a different one each time. None: no animation",
        kind: "picks",
        options: LOOKS.map((l): [string, string] => [l.id, l.name]),
        value: looksIn(this.look).join(","),
        disabled: keep,
        set: async (v) => {
          const l = lookFrom(String(v).split(","));
          const why = await ev.onPref?.("opening", l);
          if (!why) this.look = l;
          return why;
        },
        tile: (id, box) => {
          box.classList.toggle("dark", this.sheetDark());
          return lookPreview(id, box, this.sheetDark(), (this.mainFiles.main ?? this.doc ?? "document.tex").split("/").pop()!, this.words.steps, hintsFor(this.kind)[0] ?? "");
        },
      },
    );
    if (ev.onBoxes) look.push({ id: "underline", label: "Show them on the PDF", help: "A wavy line under each one, and a mark beside the scrollbar", kind: "toggle", value: this.culpritsOn, disabled: keep, set: pref("underline", (on) => this.culpritsShown(on)) });
    // (the problems' filters, the same as the buttons at the top of the problems list)
    const problems: SettingRow[] = KINDS.map(([k, name]) => ({
      id: `kind-${k}`,
      label: name,
      help: KIND_HELP[k],
      kind: "toggle",
      value: !this.hiddenKinds.has(k),
      set: (on) => void (!!on === this.hiddenKinds.has(k) && this.toggleKind(k)),
    }));
    if (ev.onBoxes) problems.push(look.splice(look.findIndex((r) => r.id === "underline"), 1)[0]);
    const build: SettingRow[] = [];
    if (ev.onDebug) build.push({ id: "debug", label: "Debug check", help: "Compare with a fresh build every 5 s (this session only)", kind: "toggle", value: this.debugOn, set: () => this.toggleDebug() });
    build.push({ id: "reload", label: "Reload the files", help: "Read the project's files again", kind: "action", button: "Reload", set: () => (this.settings(false), void ev.onReload()) });
    const tips: SettingRow[] = [{ id: "tips", label: "Feature tips", help: "Shown again, from the start", kind: "action", button: "Show again", set: () => (this.tipsReset(), this.settings(false)) }];
    if (ev.onTour) tips.push({ id: "tour", label: "The tour", kind: "action", button: "Take the tour", set: () => (this.settings(false), ev.onTour!()) });
    if (ev.onNews) tips.push({ id: "news", label: "What's new", kind: "action", button: "Show", set: () => (this.settings(false), ev.onNews!()) });
    return [
      { title: "Preview", rows: preview },
      { title: "Appearance", rows: look },
      { title: "Links", rows: this.linkRows() },
      { title: "Warnings/Errors", rows: problems },
      { title: "Build", rows: build },
      { title: "Packages", rows: [] },
      { title: "Updates and tips", rows: tips },
    ];
  }

  /** Start the pages at page `k`, `top` points down it (a tab opened at a place: its address said where). */
  startAt(k: number, top: number | null): void {
    this.viewer.startAt(k, top);
  }

  /** The links' switches (PanelPrefs) given to the viewer. */
  private linksShown(): void {
    const v = this.viewer, p = this.prefs;
    [v.peeks, v.backs, v.flashes, v.newTabs] = [p.peeks, p.backs, p.flashes, p.newTabs];
    if (!p.backs) v.backsOff();
  }

  /** Settings' Links: each feature of the links on or off, kept with the panel's own choices (every host's). */
  private linkRows(): SettingRow[] {
    const sw = (k: "peeks" | "backs" | "flashes" | "newTabs") => (on?: boolean | string) => {
      this.prefs[k] = on === true;
      this.linksShown();
      this.save();
    };
    const rows: SettingRow[] = [
      { id: "peeks", label: "Preview links on hover", help: "A window over a link shows where it goes: the page, a citation's works, an address. Its pin keeps it open, moved and resized as you like", kind: "toggle", value: this.prefs.peeks, set: sw("peeks") },
      { id: "backs", label: "Back and Forward after a jump", help: "A Back button in the corner once you follow a link, with the places before it; the mouse's back and forward buttons", kind: "toggle", value: this.prefs.backs, set: sw("backs") },
      { id: "flashes", label: "Light up where a jump lands", help: "What a link pointed at lit for a moment", kind: "toggle", value: this.prefs.flashes, set: sw("flashes") },
    ];
    if (this.ev.onOpenAt) rows.push({ id: "newTabs", label: "Ctrl+click opens a new tab", help: "A link Ctrl+clicked (⌘ on a Mac) or middle-clicked opens this preview in a new tab at its target", kind: "toggle", value: this.prefs.newTabs, set: sw("newTabs") });
    return rows;
  }

  /** The panel's own choices as the host keeps them (ViewPrefs): applied, not told back. */
  viewPrefs(p: Partial<ViewPrefs>): void {
    if (p["dark-pdf"]) this.darkPdf(p["dark-pdf"]);
    if (p.opening) this.look = lookOf(p.opening);
    if (p.underline !== undefined && p.underline !== this.culpritsOn) this.culpritsShown(p.underline);
  }

  /** Repaints so far: the first is the first build's. */
  private paints = 0;

  /** Where the pages' strips say the last frame's numbers (drawn, kept, watch, probe, draw, fetch, compose): under the ⚡ chip's. */
  stripsChip(): HTMLElement {
    return this.$("#stripsline");
  }

  /** A repaint reached the screen, `ms` after its keystroke: the status says how fast. */
  painted(ms: number): void {
    // (the first paint is the first build's: downloads, the format, the whole document; the status says so, not "Built · 14906 ms")
    const first = this.paints++ === 0;
    // (the first: the time the writer waited, opening to page (the walk), not the engine's last part)
    if (first) this.walkEnd();
    if (first && this.walk?.ms !== undefined) ms = this.walk.ms;
    this.speed = { ms, at: performance.now(), first };
    // (each repaint's time is the status's, "Built · 14 ms", and its popover's
    // with the build's parts and the strips' numbers: nothing over the pages, the user's)
    this.$("#speedms").textContent = `⚡ ${this.speed.first ? "first page " : ""}${dur(ms)}`;
    this.renderStatus();
    if (!this.$("#m-time").hidden) this.timing();
    this.emit();
  }

  // ---- Dark PDF: the user's choice, Off by default (a static filter: it inverts figures too) ----

  private dark: DarkPdf = "off";
  private darkQuery = matchMedia("(prefers-color-scheme: dark)");

  /** Dark pages off, on, or as the host's theme is (the host keeps the choice: viewPrefs). */
  darkPdf(d: DarkPdf): void {
    this.dark = d;
    this.darkQuery.onchange = () => this.applyDark();
    this.applyDark();
  }

  /** The loading animation (the host keeps the choice: viewPrefs). */
  private look: OpeningLook = "random";
  /** The last one played (random plays another). */
  private lastLook: string | null = null;

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

  /**
   * The tips now: those not seen yet, where the host keeps them from showing
   * by themselves (PanelOptions.tips false: after its own); `all`, the tour
   * itself, every tip from the first (a host's own "Take the tour").
   */
  tour(all = false): void {
    this.tips(all ? "all" : "unseen", true);
  }

  /**
   * The tips, from the first: those not seen yet (a first visit), the new
   * features' (What's new: `fresh`), or every one (the tour); `asked`: the
   * host's tour().
   */
  /** The tips seen: this browser's, and the host's (tipsSeen). */
  private seenIds = (() => {
    try {
      return new Set<string>(JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]"));
    } catch {
      return new Set<string>();
    }
  })();

  /** The tips seen as the host keeps them (the watch: the user's phitex.toml, the same on every port): not offered by themselves again. */
  tipsSeen(ids: string[]): void {
    for (const id of ids) this.seenIds.add(id);
    // (one offered meanwhile and seen now: gone)
    if (!this.$("#tip").hidden && this.tipList.every((t) => this.seenIds.has(t.id))) this.tip(-1);
  }

  /** Every tip unseen again (Settings' Show again): offered from the start at the next page. */
  tipsReset(): void {
    this.seenIds.clear();
    try {
      localStorage.removeItem("phitex.tips.seen");
    } catch {
      /* (none kept) */
    }
    this.tips("unseen", true);
  }

  private tips(which: "unseen" | "fresh" | "all", asked = false): void {
    if (!this.ownBar || this.prefs.collapsed || (which === "unseen" && !asked && !this.autoTips)) return;
    const seen = [...this.seenIds];
    // (Overleaf's welcome callout says "as you type" already: not twice)
    const offered = TIPS.filter((t) => !(t.id === "compare" && !this.ev.onCompare) && !(t.id === "contents" && !this.contents) && !(t.id === "live" && this.kind === "overleaf"));
    this.tipList = which === "all" ? offered : which === "fresh" ? offered.filter((t) => t.fresh) : offered.filter((t) => !seen.includes(t.id));
    this.tip(0);
  }

  /** Tips seen: not offered by themselves again (What's new and the tour still show them). */
  private seen(ids: string[]): void {
    const before = this.seenIds.size;
    for (const id of ids) this.seenIds.add(id);
    if (this.seenIds.size === before) return;
    try {
      localStorage.setItem("phitex.tips.seen", JSON.stringify([...this.seenIds]));
    } catch {
      /* (the host's copy only) */
    }
    this.ev.onTipsSeen?.([...this.seenIds]);
  }

  private tip(i: number): void {
    const el = this.$("#tip");
    this.root.querySelectorAll(".ringed").forEach((e) => e.classList.remove("ringed"));
    this.tipAt = i;
    const t = this.tipList[i];
    if (!t) return void (el.hidden = true);
    this.seen([t.id]);
    let target = this.$(t.at);
    // (a control hidden at this width: the menu that holds it)
    if (!target || !target.offsetParent) target = this.$("#b-more");
    target.classList.add("ringed");
    el.replaceChildren(html`<h4>${t.fresh ? html`<span class="new">New</span>` : ""}${t.title}</h4>${t.demo()}<p>${t.text}</p><div class="acts"><span class="n">${i + 1} of ${this.tipList.length}</span><button type="button" data-t="skip">Skip</button><button type="button" class="primary" data-t="next">${i + 1 < this.tipList.length ? "Next" : "Got it"}</button></div>`);
    el.hidden = false;
    const bar = this.$(".bar").getBoundingClientRect(), r = target.getBoundingClientRect(), w = el.offsetWidth;
    const cx = r.left + r.width / 2 - bar.left, left = Math.max(8, Math.min(bar.width - w - 8, cx - w / 2));
    Object.assign(el.style, { left: `${left}px`, top: `${r.bottom - bar.top + 9}px` });
    el.style.setProperty("--ax", `${cx - left}px`);
    el.onclick = (e) => {
      e.stopPropagation();
      const a = (e.target as HTMLElement).closest<HTMLElement>("[data-t]")?.dataset.t;
      // (Skip ends the tour: every card of it seen, so none comes back unasked)
      if (a === "skip") this.seen(this.tipList.map((t) => t.id));
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
    const n = d.busy ? html`<span class="n busy">${d.busy}</span>` : html`<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    const stuck = !!d.busy || !d.changes;
    // (which pages are shown, said by a switch of two, the one shown pressed;
    // the diff's style a click away on its own half: D switches)
    el.replaceChildren(html`<span class="what" title="${d.title} against the current version">${d.title} vs current <small>${d.when}</small></span><button data-a="prev" title="Previous change (Shift+N)" disabled="${stuck}">‹</button>${n}<button data-a="next" title="Next change (N)" disabled="${stuck}">›</button><span class="sep"></span><span class="seg" role="group" aria-label="Pages shown (D)"><button data-a="${d.showing === "diff" ? "settings" : "toggle"}" class="on-diff" aria-pressed="${d.showing === "diff"}" title="${d.showing === "diff" ? "The diff is shown: its style (colors, markup, change bars)" : "Show the diff (D)"}">Diff${d.showing === "diff" ? html` <span class="chev" aria-label="Diff style">▾</span>` : ""}</button><button data-a="${d.showing === "current" ? "" : "toggle"}" class="on-current" aria-pressed="${d.showing === "current"}" title="${d.showing === "current" ? "The current version is shown" : "Show the current version (D)"}">Current</button></span><button data-a="pdf" title="Download the diff PDF" disabled="${!!d.busy}">⬇ PDF</button><button data-a="tex" title="Download diff.tex" disabled="${!!d.busy}">⬇ .tex</button><button data-a="settings" title="Diff style: colors, markup, change bars" aria-label="Diff style">⚙</button><button data-a="close" title="Stop comparing" aria-label="Stop comparing">✕</button>`);
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
      el.className = "menu diffset";
      return;
    }
    // (the look comes from a host, a store or a server: its colors go into
    // attributes, so only #rrggbb is taken, its names only as the lists have them)
    look = { ...look, add_color: safeColor(look.add_color, "#0000ff"), del_color: safeColor(look.del_color, "#ff0000") };
    // (the styles, and change bars in the margin as a switch beside them:
    // latexdiff's types with bars are a style's with them, where it has one)
    const MARKUP: [string, string][] = [
      ["underline", "Underline"], ["ctraditional", "Color + font"], ["traditional", "Font"], ["cfont", "Color + size"],
      ["fontstrike", "Strike"], ["bold", "Bold"], ["invisible", "Hide deletions"],
    ];
    const WITH_BARS: Record<string, string> = { underline: "culinechbar", cfont: "cfontchbar", invisible: "changebar", ctraditional: "cchangebar" };
    const BASE: Record<string, string> = Object.fromEntries(Object.entries(WITH_BARS).map(([a, b]) => [b, a]));
    const base = BASE[look.markup] ?? look.markup;
    const bars = look.markup in BASE;
    const SUB: [string, string][] = [["safe", "Plain"], ["color", "Colored"], ["marker", "Margin marks"]];
    const PAIRS: [string, string, string][] = [
      ["#0000ff", "#ff0000", "Blue / red"], ["#15803d", "#b91c1c", "Green / red"], ["#0072b2", "#d55e00", "Colorblind-safe"], ["#7e22ce", "#6b7280", "Purple / gray"],
    ];
    const seg = (key: "markup" | "subtype", opts: [string, string][]) =>
      html`<div class="opts">${opts.map(([v, t]) => html`<button data-k="${key}" data-v="${v}" aria-pressed="${(key === "markup" ? base : look[key]) === v}">${t}</button>`)}</div>`;
    const canBars = base in WITH_BARS;
    const deco = (add: boolean) => {
      const c = add ? look.add_color : look.del_color;
      if (look.markup === "invisible" && !add) return "display:none";
      const strike = !add && /^(underline|fontstrike|culinechbar)$/.test(look.markup);
      const line = add && /^(underline|culinechbar)$/.test(look.markup) ? `text-decoration: underline wavy ${c}; text-underline-offset: 3px;` : "";
      return `color:${c};${strike ? `text-decoration: line-through ${c};` : ""}${line}${look.markup === "bold" && add ? "font-weight:700;" : ""}`;
    };
    el.replaceChildren(html`<h4>Style</h4>${seg("markup", MARKUP)}<div class="row bars"><span>Change bars in the margin</span><button data-bars="1" role="switch" aria-checked="${bars}" aria-pressed="${bars}" disabled="${!canBars}" title="${canBars ? null : "Not with this style"}">${bars ? "On" : "Off"}</button></div><h4>Colors</h4><div class="opts">${PAIRS.map(([a, d, t]) => html`<button class="pair" data-pair="${a},${d}" aria-pressed="${look.add_color === a && look.del_color === d}"><i style="background:${a}"></i><i style="background:${d}"></i>${t}</button>`)}</div><div class="row"><span>Added</span><input type="color" data-c="add_color" value="${look.add_color}"><span>Deleted</span><input type="color" data-c="del_color" value="${look.del_color}"></div><h4>Mode</h4>${seg("subtype", SUB)}<div class="sample" style="${bars ? `box-shadow: inset 3px 0 0 ${look.add_color}; padding-left: 12px` : null}">The results are <span style="${deco(false)}">good</span> <span style="${deco(true)}">excellent</span>.</div><div class="foot"><small>As latexdiff's --type, --subtype</small><button data-reset="1">Reset</button></div>`);
    el.className = "menu diffset on";
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
      // (a style picked keeps the bars where it can have them)
      if (b.dataset.bars) return canBars && set({ markup: bars ? base : WITH_BARS[base] });
      if (b.dataset.k === "markup") return set({ markup: bars && b.dataset.v! in WITH_BARS ? WITH_BARS[b.dataset.v!] : b.dataset.v! });
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
    if (!this.$("#m-time").hidden) this.timing();
  }

  /**
   * This opening's steps as the sheet said them (the project read, the
   * packages, the format, typesetting), each with the time it was up, from
   * the first step to the first page on screen (`ms`, then closed): the
   * first build's time as the writer waited for it, not the engine's part.
   */
  private walk: { since: number; parts: [string, number][]; now?: string; at: number; ms?: number } | null = null;

  /** The opening's step is now `what` (a step of the sheet's). */
  private walkTo(what: string): void {
    const now = performance.now();
    const w = (this.walk ??= { since: now, parts: [], at: now });
    if (w.ms !== undefined) return;
    const step = what === this.words.steps[1] ? what : what === this.words.steps[2] ? (this.making ? "Making the format" : what) : this.words.steps[0];
    if (step === w.now) return;
    this.walkAdd(now);
    (w.now = step), (w.at = now);
  }

  /** The step up until `now` counted (a step seen again adds to its first time). */
  private walkAdd(now: number): void {
    const w = this.walk!;
    if (!w.now) return;
    const p = w.parts.find(([k]) => k === w.now);
    if (p) p[1] += now - w.at;
    else w.parts.push([w.now, now - w.at]);
  }

  /** The first page is on screen: the opening's time closed. */
  private walkEnd(): void {
    const w = this.walk;
    if (!w || w.ms !== undefined) return;
    const now = performance.now();
    this.walkAdd(now);
    w.now = undefined;
    w.ms = now - w.since;
  }

  /** The timing popover's first build: each step's time, as a bar, from opening to the first page. */
  private walkPart(): DocumentFragment | "" {
    const w = this.walk;
    if (!w || w.ms === undefined || !this.speed?.first) return "";
    const parts = w.parts.filter(([, ms]) => ms >= 1);
    // (its total is the head's, "First page · 124 s": said once)
    return html`<div class="row"><span class="k">Where the time went</span><span class="v"></span>${bars(parts, 4)}</div>`;
  }

  /** The last build's parts as its host measured them (the watch's stats), in order, and work done alongside them (not in the build's time): [name, ms]. */
  private phases: { parts: [string, number][]; beside: [string, number][] } | null = null;

  /** The last build's parts (`parts` add up to its time; `beside`: done at the same time, said under the bar). */
  buildParts(parts: [string, number][], beside: [string, number][] = []): void {
    this.phases = { parts, beside };
    if (!this.$("#m-time").hidden) this.timing();
  }

  /** The timing popover's "Where the last build's time went": its parts as a bar, like the opening's. */
  private phasesPart(): DocumentFragment | "" {
    const p = this.phases;
    if (!p) return "";
    const parts = p.parts.filter(([, ms]) => ms >= 1);
    if (!parts.length) return "";
    const beside = p.beside.filter(([, ms]) => ms >= 1).map(([n, ms]) => `${n} ${dur(ms)}`).join(", ");
    return html`<div class="row"><span class="k">Where the last build's time went</span><span class="v"></span>${bars(parts, 4)}${beside ? html`<span class="note">Alongside: ${beside}</span>` : ""}</div>`;
  }

  /** The packages fetched since the project opened: how many, their bytes, and the time any was on its way (`since`: one is now). */
  private downloads: { n: number; bytes: number; ms: number; since?: number } = { n: 0, bytes: 0, ms: 0 };

  /** The timing popover's Downloads row: the packages' time, how many and their size; none fetched, none. */
  private downloadsPart(): DocumentFragment | "" {
    const d = this.downloads;
    if (!d.n && d.since === undefined) return "";
    const ms = d.ms + (d.since === undefined ? 0 : performance.now() - d.since);
    const size = d.bytes ? ` · ${d.bytes < 1e6 ? `${Math.max(1, Math.round(d.bytes / 1e3))} kB` : `${(d.bytes / 1e6).toFixed(1)} MB`}` : "";
    return html`<div class="row"><span class="k">Downloads</span><span class="v">${dur(ms)}</span><span class="note">${d.n} package${d.n === 1 ? "" : "s"}${size}${d.since === undefined ? "" : ", more on the way"}</span></div>`;
  }

  /** The first build's time and the edits' since (the watch's), for the popover's bars. */
  private times: { first?: number; edits: number[] } = { edits: [] };

  /** A build's time: the first (cold) one's, or an edit's: drawn as bars beside each other in the timing popover. */
  buildTime(ms: number, first: boolean): void {
    if (first) this.times = { first: ms, edits: [] };
    else this.times.edits.push(ms);
    this.built = { ms, at: performance.now(), first };
    // (a build that changed no page paints nothing: the status and the popover say it all the same)
    this.renderStatus();
    if (!this.$("#m-time").hidden) this.timing();
    this.emit();
  }

  /** The last build the host said (buildTime), painted or not; `joined`: the one before this tab joined (joinedLate). */
  private built?: { ms: number; at: number; first: boolean; joined?: boolean };

  /**
   * What the status and the timing popover's head say: the newest of the
   * last paint (its time from the keystroke) and the last build (one that
   * changed no page paints nothing, and was not said before: grail's 7 s).
   */
  private latest(): { label: string; ms: number; first: boolean } | undefined {
    const s = this.speed, b = this.built;
    if (b && (!s || b.at > s.at)) return { label: b.first ? "First build" : b.joined ? "Last build" : "Built, no page changed", ms: b.ms, first: b.first };
    if (s) return { label: s.first ? "First page" : "Built", ms: s.ms, first: !!s.first };
    return undefined;
  }

  /**
   * This tab joined `builds` builds in: `first` the first build's time for
   * the bars; the first build's paint was not this tab's, so its next is not
   * called one; `last` the last build's time, for the status and the popover's
   * head ("Last build", or "First build" if it was the only one).
   */
  joinedLate(first: number | undefined, builds: number, last?: { ms: number; cold: boolean }): void {
    if (first !== undefined && this.times.first === undefined) this.times.first = first;
    if (builds >= 1) this.paints = Math.max(this.paints, 1);
    if (last && !this.built) {
      this.built = { ms: last.ms, at: performance.now(), first: last.cold, joined: true };
      this.renderStatus();
      if (!this.$("#m-time").hidden) this.timing();
    }
  }

  /** The bars: the first build, the last edit, the edits' median, each as long as its time (the longest the full width; a short one at least a sliver). */
  private timeBars(): DocumentFragment | "" {
    const t = this.times;
    if (t.first === undefined || !t.edits.length) return "";
    const sorted = [...t.edits].sort((a, b) => a - b);
    const rows: [string, number, string][] = [
      ["First build", t.first, "first"],
      ["This edit", t.edits[t.edits.length - 1], "edit"],
      ...(t.edits.length > 2 ? [["Edits, median", sorted[sorted.length >> 1], "median"] as [string, number, string]] : []),
    ];
    const max = Math.max(...rows.map((r) => r[1]), 1e-3);
    const x = t.first / Math.max(t.edits[t.edits.length - 1], 1e-3);
    return html`<div class="tbars" role="img" aria-label="First build ${dur(t.first)}, this edit ${dur(t.edits[t.edits.length - 1])}">${rows.map(([k, ms, c]) => html`<div class="tb"><span class="k">${k}</span><span class="track"><i class="${c}" style="width:${Math.max(0.6, (100 * ms) / max).toFixed(2)}%"></i></span><span class="v">${dur(ms)}</span></div>`)}${x >= 1.5 ? html`<div class="tb x">${x < 10 ? x.toFixed(1) : Math.round(x)}× faster than the first build</div>` : ""}</div>`;
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
    const parts = this.timingParts().map(([k, v]) => part(k, v));
    // (the headline: a first build's time, else keystroke to page as the session measured it, else the host's paint)
    const l = this.latest();
    const e2e = parts.find((p) => p.key === "keystroke→page")?.ms ?? l?.ms;
    const head = !l ? (e2e !== undefined ? ["Keystroke to page", e2e] : null) : l.first || l.label !== "Built" ? [l.label, l.ms] : ["Keystroke to page", e2e ?? l.ms];
    const rows = parts.map((p) => html`<div class="row"><span class="k">${p.label}</span><span class="v">${p.value}</span>${p.split?.length ? bars(p.split) : ""}${p.note ? html`<span class="note">${p.note}</span>` : ""}</div>`);
    // (the last page drawn: its time as a bar like the build's parts, the watch's probe and draw, the transfer, the decode)
    const strips = framePart(this.$("#stripsline").dataset.frame);
    el.replaceChildren(html`${head ? html`<div class="head"><span>${String(head[0])}</span><b>${dur(head[1] as number)}</b></div>` : html`<div class="head"><span>No build yet</span></div>`}${this.walkPart()}${this.phasesPart()}${this.timeBars()}${this.downloadsPart()}${rows}${strips}<div class="foot"></div><div class="foot msg${this.msgErr ? " err" : ""}"></div><button type="button" class="reportbtn copydbg">Copy debug info</button>`);
    el.querySelector<HTMLButtonElement>(".copydbg")!.onclick = (e) => {
      e.stopPropagation();
      const b = e.currentTarget as HTMLButtonElement;
      void this.debugInfo()
        .then((t) => navigator.clipboard.writeText(t))
        .then(() => (b.textContent = "Copied ✓"), () => (b.textContent = "Couldn't copy"));
    };
    // (the summary line only when there are no parts to say it)
    el.querySelector(".foot")!.textContent = parts.length || this.walk?.ms !== undefined ? "" : (this.lat?.summary ?? "");
    el.querySelector(".msg")!.textContent = this.msgText;
  }

  /**
   * The timing popover's numbers as plain text, for a bug report or a chat:
   * the host's lines (version, engine, files), the last build's head and
   * parts as the session or the watch said them, the first page's steps,
   * the downloads, the last page drawn, and this machine.
   */
  private async debugInfo(): Promise<string> {
    const out: string[] = [...((await this.ev.onDebugInfo?.().catch(() => [])) ?? [])];
    const l = this.latest();
    if (l) out.push(`${l.label}: ${dur(l.ms)}`);
    if (this.lat?.summary) out.push(this.lat.summary);
    for (const line of (this.lat?.details ?? "").split("\n").filter(Boolean)) out.push(`  ${line.trim()}`);
    const w = this.walk;
    if (w?.ms !== undefined) out.push(`opening to first page ${dur(w.ms)}: ${w.parts.map(([n, ms]) => `${n.toLowerCase()} ${dur(ms)}`).join(", ")}`);
    const d = this.downloads;
    if (d.n) out.push(`downloads: ${d.n} packages, ${(d.bytes / 1e6).toFixed(1)} MB, ${dur(d.ms)}`);
    const t = this.times;
    if (t.first !== undefined) out.push(`first build ${dur(t.first)}${t.edits.length ? `; edits ${t.edits.length}, last ${dur(t.edits[t.edits.length - 1])}, median ${dur([...t.edits].sort((a, b) => a - b)[t.edits.length >> 1])}` : ""}`);
    const f = this.$("#stripsline").dataset.frame;
    if (f) out.push(`last page drawn: ${frameText(f)}`);
    const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
    out.push(`machine: ${nav.hardwareConcurrency ?? "?"} cores, ${nav.userAgentData?.platform || nav.platform || "?"}; ${nav.userAgent}`);
    return out.join("\n") + "\n";
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
    this.msg(`${names.length} file${names.length === 1 ? "" : "s"} read ${took} · they stay in this browser`);
  }

  /** The main files to pick from (Settings' Main file), and the one built. */
  mains(names: string[], main: string | null): void {
    this.mainFiles = { names, main };
    if (!this.$("#settings").hidden) void this.settings(true);
  }
  private mainFiles: { names: string[]; main: string | null } = { names: [], main: null };

  /** The document's main file as the host names it, where it has no Main file to pick (the watch): the openings that show it (a prompt's, a title bar's). */
  docName(name: string): void {
    this.doc = name;
  }
  private doc: string | null = null;

  /** What's new (the host's news, `text` as plain lines), in a dialog over the pages, with the new features' tips a click away. */
  news(version: string, text: string): void {
    const box = this.$("#logview"), pre = this.$("#logtext");
    box.querySelector("header")!.firstChild!.textContent = `What's new in ${version}`;
    pre.textContent = text;
    let b = box.querySelector<HTMLButtonElement>("#newstips");
    if (!b) {
      b = document.createElement("button");
      b.id = "newstips";
      b.className = "btn";
      b.textContent = "Show the new features";
      b.onclick = () => ((box.hidden = true), this.tips("fresh"));
      box.append(b);
    }
    b.hidden = false;
    box.hidden = false;
    box.querySelector<HTMLElement>("[data-close]")?.focus();
  }

  /** The last build's log (the host's), in a dialog over the pages. */
  private async showLog(): Promise<void> {
    const box = this.$("#logview"), pre = this.$("#logtext");
    pre.textContent = "Reading the log…";
    box.querySelector("header")!.firstChild!.textContent = "The last build's log";
    const nb = box.querySelector<HTMLElement>("#newstips");
    if (nb) nb.hidden = true;
    box.hidden = false;
    // (the focus in the dialog: Esc reaches the panel's keys and closes it, the menu that opened it being gone)
    box.querySelector<HTMLElement>("[data-close]")?.focus();
    const text = await this.ev.onLog?.().catch((e: unknown) => `Couldn't read the log: ${e instanceof Error ? e.message : String(e)}`);
    pre.textContent = text ?? "No log yet: the first build hasn't finished.";
  }

  private redraw(): void {
    this.viewer?.redraw();
    // (the pages' heights changed with the zoom: the marks along the scroll with them)
    if (this.viewer) requestAnimationFrame(() => this.drawScrollMarks());
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
  layout(hashes: string[], previews?: ReadonlySet<string>): void {
    this.pageHashes = hashes;
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes, previews);
    // (the sheet up: on the first page's box now that it has one)
    this.fitSheet();
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
    if (hashes.length) this.sheetDown();
    this.nav();
    // (a fast build's problems reach the view before its pages do: their
    // underlines, looked up on no page, are looked up again on these; the
    // same pages as before are not)
    if (this.culpritDiags.length) void this.underline(this.culpritDiags);
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
  others(marks: { k: number; boxes: [number, number, number, number][]; label: string; hue: number; file?: string; pos?: number }[]): void {
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
    this.drewOnce = true;
    this.walkEnd();
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.$("#stage").classList.toggle("fit", (this.zoomNow ?? this.prefs.zoom) === "fit");
    // (a view with no layout yet: this page's slots, up to it)
    if (this.viewer.pages <= k) this.viewer.layout(Array.from({ length: Math.max(n, k + 1) }, (_, i) => (i === k && hash) || `?${i}`));
    this.viewer.set(k, img, hash ?? null);
    // (a page drawn: this document is built here, with or without a sheet over it)
    this.builtHere();
    this.sheetDown();
    this.renderStatus();
    this.nav();
  }
}

export type DarkPdf = "off" | "on" | "theme";

/** The packages a first build downloaded, said under its step ("" when all were kept on this machine: none to count). */
const downloaded = (n: number, bytes?: number) => (n ? `${n} package${n === 1 ? "" : "s"} downloaded${mb(bytes)}` : "");

/** The loading animation: PhiTeX's opening, a Windows 98 setup dialog, or none. */
export type { OpeningLook } from "./openings/index.ts";

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

/** How much of a long line the tip shows each side of its culprit (characters). */
const TIP_AROUND = 36;

/**
 * An underline's tip: the message, with the culprit after it when it is a
 * command the message does not name ("Undefined control sequence \pau");
 * then the line as TeX read it, the culprit marked (cut round it when
 * long), and file:line.
 */
function culpritTip(p: { file: string; message: string; line: number | null; excerpt: Problem["excerpt"] }, click = "click to go there"): HTMLElement {
  const ex = p.excerpt;
  const chars = ex ? [...ex.text] : [];
  const culprit = ex ? chars.slice(ex.start, ex.start + ex.len).join("").trim() : "";
  const named = /^\\[A-Za-z@]+\*?$|^\\.$/.test(culprit) && !p.message.includes(culprit);
  let excerpt: DocumentFragment | string = "";
  if (ex && culprit) {
    const from = Math.max(0, ex.start - TIP_AROUND), to = Math.min(chars.length, ex.start + ex.len + TIP_AROUND);
    const a = (from > 0 ? "…" : "") + chars.slice(from, ex.start).join("").trimStart();
    const b = chars.slice(ex.start, ex.start + ex.len).join("");
    const c = chars.slice(ex.start + ex.len, to).join("").trimEnd() + (to < chars.length ? "…" : "");
    excerpt = html`<pre class="ex">${a}<mark>${b}</mark>${c}</pre>`;
  }
  const where = `${p.file.split("/").pop()}${p.line ? `:${p.line}` : ""}`;
  return html`<div class="txt"><div>${p.message}${named ? html` <code>${culprit}</code>` : ""}</div>${excerpt}<div class="at">${where} · ${click}</div></div>`.firstElementChild as HTMLElement;
}
