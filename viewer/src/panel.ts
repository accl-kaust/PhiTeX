// The preview panel: a floating window in Overleaf's own visual language
// (its CSS custom properties inherit into the shadow root, so it follows
// Overleaf's theme; fallbacks are its default values), beside Overleaf's
// PDF, never replacing it. Experimental, and says so.
//
// Quality of life: drag by the header, resize from the corner, collapse to
// a pill (Alt+Shift+P toggles), position/size/zoom/format remembered; a
// build that ships nothing keeps the last good page, dimmed, with why;
// diagnostics jump to their line.

import type { Draws, PageImage, Status } from "./session.ts";
import type { PackageState } from "./packages.ts";
import { ENGINES, errorNeeds, type Engine } from "./engines.ts";
import { Viewer } from "./viewer.ts";
import { VIEWER_CSS } from "./css.ts";
import { REPORT_TO } from "./report.ts";

/** How long a build may ship no page before the last good one is dimmed (typing through `{`). */
const STALE_GRACE_MS = 1000;

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
  speed?: { ms: number; at: number };
  sheet: boolean;
  /** Diagnostics: how many, and the worst severity (or ""). */
  diagCount: number;
  diagWorst: string;
  /** The zoom as a percentage (fit: what fitting the width comes to), as Overleaf's viewer shows it. */
  percent: number;
}

export interface PanelEvents {
  /** The page in view changed (the one an edit paints first). */
  onPage(p: number): void;
  /** Page `k` is wanted: in view, not drawn at its current hash. */
  onNeed?(k: number): void;
  onPdf(): void;
  onDebug(on: boolean): void;
  onMain(m: string): void;
  onReload(): void;
  /** Clean recompile: the project read again, typeset from the start. */
  onClean(): void;
  onFormat(f: PageFormat): void;
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
  /** A link to a web address on a page (absent: opened in a new tab). */
  onLink?(uri: string): void;
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
  license: "LICENSE.txt",
  realPdf: "Overleaf's PDF is the real one.",
  stopped: "Open ⓘ diagnostics for the details. Overleaf's PDF is one click away (PDF).",
  notReady: "Use Overleaf's PDF for it",
  openFailed: "Overleaf's PDF is one click away. Reload the tab to try again.",
  reading: "Your project's files stay in this browser: read from Overleaf, typeset here.",
  keptIn: "this browser",
};

/** What a host may leave out: a control whose event it doesn't handle is hidden. */
export interface PanelOptions {
  words?: Partial<PanelWords>;
  /** The PDF.js page format offered (default: yes). */
  pdfjs?: boolean;
  /** One of the host's own files by its path (the license): its address (default: the path as it is). */
  fileUrl?(path: string): string;
}

/** Where the panel keeps its preferences (chrome.storage.local in the extension). */
export interface Prefs {
  load(): Promise<Partial<PanelPrefs>>;
  save(p: PanelPrefs): void;
}

/** How pages are drawn: vector (SVG from the draw lists) or pdfjs (our PDF, by pdf.js, with its text layer). */
export type PageFormat = "vector" | "pdfjs";
/** A stored format, an older version's "png" read as vector. */
export const pageFormat = (f: unknown): PageFormat => (f === "pdfjs" ? "pdfjs" : "vector");

export interface PanelPrefs {
  x: number | null;
  y: number | null;
  w: number;
  h: number;
  zoom: string;
  format: PageFormat;
  collapsed: boolean;
  details: boolean;
}

const DEFAULTS: PanelPrefs = { x: null, y: null, w: 440, h: 620, zoom: "fit", format: "vector", collapsed: false, details: false };

/** PhiTeX's fonts (the PDF base 14) as the browser's. */
function cssFont(name: string, px: number): string {
  const bold = /Bold/.test(name),
    italic = /Italic|Oblique/.test(name);
  const family = /^Times/.test(name)
    ? '"Times New Roman", Times, "Liberation Serif", "Nimbus Roman", "Noto Serif", serif'
    : /^Helvetica/.test(name)
      ? 'Helvetica, Arial, "Liberation Sans", "Nimbus Sans", "Noto Sans", sans-serif'
      : /^Courier/.test(name)
        ? '"Courier New", Courier, "Liberation Mono", "DM Mono", monospace'
        : '"Noto Serif", serif';
  return `${italic ? "italic " : ""}${bold ? "bold " : ""}${px}px ${family}`;
}

/**
 * pdf.js's approximateFraction (pdfjs-dist 5.1.91, web/pdf_viewer.mjs): x as
 * a fraction a/b with b ≤ 8. pdf.js sizes a page box with
 * `round(down, scale × width, b px)`, b from the device pixel ratio, so its
 * canvas lands on whole device pixels; the same here makes our page the
 * size of Overleaf's, to the pixel.
 */
export function approximateFraction(x: number): [number, number] {
  if (Math.floor(x) === x) return [x, 1];
  const xinv = 1 / x;
  const limit = 8;
  if (xinv > limit) return [1, limit];
  if (Math.floor(xinv) === xinv) return [1, xinv];
  const x_ = x > 1 ? xinv : x;
  let a = 0, b = 1, c = 1, d = 1;
  for (;;) {
    const p = a + c, q = b + d;
    if (q > limit) break;
    if (x_ <= p / q) {
      c = p;
      d = q;
    } else {
      a = p;
      b = q;
    }
  }
  if (x_ - a / b < c / d - x_) return x_ === x ? [a, b] : [b, a];
  return x_ === x ? [c, d] : [d, c];
}

/** A page's CSS size at `cssWidth`, rounded down as pdf.js does at this device pixel ratio. */
export function pageBox(d: { w: number; h: number }, cssWidth: number, dpr = globalThis.devicePixelRatio || 1): [number, number] {
  const step = approximateFraction(dpr)[1];
  const w = cssWidth, h = (d.h * cssWidth) / d.w;
  return [w - (w % step), h - (h % step)];
}

const esc = (t: string) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/**
 * A draw list as SVG: a <text> per line (words on one baseline in one
 * font), a <tspan> per word at PhiTeX's x, each word but a line's last
 * followed by a space, so the text selects and copies as text (and Ctrl+F
 * finds it). Crisp at any zoom; positions are PhiTeX's, glyphs the browser's.
 */
export function svg(d: Draws, cssWidth: number): string {
  const lines = new Map<string, [number, string, number | undefined][]>();
  const order: string[] = [];
  for (const [x, y, size, f, text, width] of d.t) {
    const k = `${y}|${size}|${f}`;
    if (!lines.has(k)) {
      lines.set(k, []);
      order.push(k);
    }
    lines.get(k)!.push([x, text, width]);
  }
  let body = "";
  for (const k of order) {
    const [y, size, f] = k.split("|");
    const words = lines.get(k)!.sort((a, b) => a[0] - b[0]);
    // (each word fitted to the width PhiTeX laid it out with: the browser's glyphs, TeX's layout)
    const spans = words
      .map(([x, t, width], i) => {
        const fit = width && [...t].length > 1 ? ` textLength="${width}" lengthAdjust="spacingAndGlyphs"` : "";
        const space = i < words.length - 1 ? `<tspan> </tspan>` : "";
        return `<tspan x="${x}"${fit}>${esc(t)}</tspan>${space}`;
      })
      .join("");
    body += `<text y="${y}" style="font:${esc(cssFont(d.f[Number(f)], Number(size)))}">${spans}</text>\n`;
  }
  for (const [x, y, w, h] of d.r) body += `<rect x="${x}" y="${y}" width="${Math.max(w, 0.4)}" height="${Math.max(h, 0.4)}"/>`;
  const [w, h] = pageBox(d, cssWidth);
  return `<svg class="page" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${d.w} ${d.h}" width="${w}" height="${h}" preserveAspectRatio="none" xml:space="preserve"><rect class="paper" width="${d.w}" height="${d.h}"/>${body}</svg>`;
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
};
const icon = (name: string, size = 18) =>
  `<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`;

const CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
.win {
  --dark: var(--bg-dark-primary, #1b222c); --dark2: var(--bg-dark-secondary, #2f3a4c); --dark3: var(--bg-dark-tertiary, #495365);
  --light: var(--bg-light-primary, #fff); --light2: var(--bg-light-secondary, #f4f5f6); --light3: var(--bg-light-tertiary, #e7e9ee);
  --accent: var(--bg-accent-01, #098842); --accent2: var(--bg-accent-02, #1e6b41);
  --fg: var(--content-primary, #1b222c); --fg2: var(--content-secondary, #495365); --fg-dark: var(--content-primary-dark, #f4f5f6);
  --fg2-dark: var(--content-placeholder-dark, #8d96a5);
  --danger: var(--content-danger, #b83a33); --warn: var(--content-warning, #8f5514); --info: var(--content-info, #366cbf);
  --ok-dark: var(--content-positive-dark, #53b57f); --warn-dark: var(--content-warning-dark, #de8014); --danger-dark: var(--content-danger-dark, #e36d66);
  --divider: var(--border-divider, #e7e9ee); --r: var(--border-radius-base, 4px); --r2: var(--border-radius-medium, 8px);
  position: fixed; z-index: 2147483000; display: flex; flex-direction: column;
  min-width: 300px; min-height: 180px; max-width: calc(100vw - 16px); max-height: calc(100vh - 16px);
  resize: both; overflow: hidden;
  background: var(--light); color: var(--fg); border: 1px solid var(--dark3); border-radius: var(--r2);
  box-shadow: 0 8px 28px rgba(27,34,44,.35);
  font: 13px/1.4 "Noto Sans", system-ui, sans-serif;
}
.icon { display: block; flex: none; }
header { display: flex; align-items: center; gap: 2px; height: 40px; padding: 0 6px 0 10px; background: var(--dark); color: var(--fg-dark);
  cursor: grab; user-select: none; flex: none; }
header:active { cursor: grabbing; }
.title { font-weight: 600; font-size: 14px; margin: 0 6px 0 4px; white-space: nowrap; }
.badge { font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 9999px; background: var(--dark2); color: var(--fg2-dark); white-space: nowrap; }
.chip { display: inline-flex; align-items: center; gap: 5px; font-size: 12px; padding: 2px 8px; border-radius: 9999px; background: var(--dark2);
  white-space: nowrap; cursor: pointer; margin-right: 4px; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg2-dark); }
.chip.ok .dot { background: var(--ok-dark); } .chip.warning .dot { background: var(--warn-dark); } .chip.error .dot { background: var(--danger-dark); }
.chip.busy .dot { animation: pulse 1s infinite; } @keyframes pulse { 50% { opacity: .3; } }
.grow { flex: 1; }
.group { display: inline-flex; align-items: center; }
button.ib { all: unset; display: inline-grid; place-items: center; width: 28px; height: 28px; border-radius: var(--r); color: inherit; cursor: pointer; }
header button.ib:hover { background: var(--dark2); } .bar button.ib:hover { background: var(--light3); }
button.ib:focus-visible { outline: 2px solid var(--info); }
button.ib[aria-pressed=true] { background: var(--dark3); }
button.ib:disabled { opacity: .35; cursor: default; }
.pno { font-size: 12px; min-width: 44px; text-align: center; font-variant-numeric: tabular-nums; }
.bar { display: flex; align-items: center; gap: 8px; padding: 6px 10px; background: var(--light2); border-bottom: 1px solid var(--divider);
  color: var(--fg2); flex: none; flex-wrap: wrap; }
.bar label { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; }
select { font: inherit; font-size: 12px; color: var(--fg); background: var(--light); border: 1px solid var(--border-primary, #677283);
  border-radius: var(--r); padding: 2px 4px; max-width: 140px; }
button.btn { font: inherit; font-size: 12px; font-weight: 600; color: #fff; background: var(--accent); border: 0; border-radius: 9999px;
  padding: 4px 12px 4px 8px; cursor: pointer; display: inline-flex; align-items: center; gap: 4px; }
button.btn:hover { background: var(--accent2); }

.sum { display: flex; align-items: center; gap: 8px; height: 28px; padding: 0 10px; font-size: 12px; color: var(--fg2); flex: none;
  border-bottom: 1px solid var(--divider); cursor: pointer; white-space: nowrap; overflow: hidden; user-select: none; }
.sum:hover { background: var(--light2); }
.sum .n { display: inline-flex; align-items: center; gap: 2px; font-weight: 600; }
.sum .n.error { color: var(--danger); } .sum .n.warning { color: var(--warn); } .sum .n.info { color: var(--info); }
.sum .first { overflow: hidden; text-overflow: ellipsis; color: var(--fg); }
.sum .caret { margin-left: auto; transition: transform .15s; }
.win.diags-open .sum .caret { transform: rotate(90deg); }
.body { position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column; }
.diags { position: absolute; z-index: 2; left: 0; right: 0; top: 0; max-height: 50%; overflow: auto; background: var(--light);
  border-bottom: 1px solid var(--divider); box-shadow: 0 6px 16px rgba(27,34,44,.18); display: none; }
.win.diags-open .diags { display: block; }
.diag { display: flex; gap: 6px; align-items: flex-start; padding: 5px 10px; font-size: 12px; border-top: 1px solid var(--divider); }
.diag:first-child { border-top: 0; }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--light2); }
.diag .icon { margin-top: 1px; }
.diag .detail { margin: 4px 0 0; padding: 4px 6px; font: 11px/1.35 "DM Mono", monospace; white-space: pre-wrap; overflow-wrap: anywhere; background: var(--divider); border-radius: 4px; max-height: 12em; overflow: auto; }
.diag.error .icon { color: var(--danger); } .diag.warning .icon { color: var(--warn); } .diag.info .icon { color: var(--info); }
.diag .where { color: var(--fg2); white-space: nowrap; margin-left: auto; padding-left: 8px; font-family: "DM Mono", monospace; font-size: 11px; }
.stage { position: relative; flex: 1; overflow: auto; background: var(--light3); padding: 12px; text-align: center; min-height: 0; }
.stage svg.page { display: inline-block; vertical-align: top; user-select: text; cursor: text; }
.stage svg.page .paper { fill: #fff; }
.stage svg.page text { fill: #000; white-space: pre; }
.stage svg.page ::selection { fill: #fff; background: var(--info); }
.stage svg.page, .stage img { background: #fff; box-shadow: 0 1px 3px rgba(27,34,44,.25); border-radius: 2px; }
.stage img { max-width: 100%; }
/* PDF.js pages: the canvas, and pdf.js's text layer over it (its own rules, the ones that place and hide the text) */
.pdfjs-page { position: relative; width: 100%; height: 100%; background: #fff; }
.pdfjs-page canvas { display: block; }
.textLayer { position: absolute; inset: 0 auto auto 0; overflow: clip; line-height: 1; transform-origin: 0 0; z-index: 0;
  --min-font-size: 1; --text-scale-factor: calc(var(--total-scale-factor) * var(--min-font-size)); --min-font-size-inv: calc(1 / var(--min-font-size)); }
.textLayer :is(span, br) { color: transparent; position: absolute; white-space: pre; cursor: text; transform-origin: 0% 0%; }
.textLayer > :not(.markedContent), .textLayer .markedContent span:not(.markedContent) { z-index: 1; --font-height: 0;
  font-size: calc(var(--text-scale-factor) * var(--font-height)); --scale-x: 1; --rotate: 0deg;
  transform: rotate(var(--rotate)) scaleX(var(--scale-x)) scale(var(--min-font-size-inv)); }
.textLayer .markedContent { display: contents; }
.textLayer ::selection { background: rgb(0 100 255 / 25%); }
.stage.stale svg.page, .stage.stale img { opacity: .45; filter: grayscale(1); }
.banner { position: sticky; top: -12px; z-index: 1; display: none; margin: -12px -12px 10px; padding: 6px 10px; text-align: left; font-size: 12px;
  background: var(--bg-warning-03, #fcf1e3); color: var(--warn); border-bottom: 1px solid var(--divider); }
.stage.stale .banner { display: block; }
.approx { position: sticky; top: -12px; z-index: 1; display: none; margin: -12px -12px 10px; padding: 6px 10px; text-align: left; font-size: 12px; background: #fff4d6; color: #5c4400; border-bottom: 1px solid #e8c766; }
.win.docked .approx { margin: 0; top: 0; }
.stage.fit { overflow-x: hidden; }
.empty { color: var(--fg2); padding: 40px 12px; font-size: 13px; }
footer { display: flex; align-items: center; gap: 8px; height: 26px; padding: 0 10px; background: var(--light2); border-top: 1px solid var(--divider);
  color: var(--fg2); font-size: 11px; font-variant-numeric: tabular-nums; flex: none; white-space: nowrap; overflow: hidden; }
footer .lat { cursor: pointer; overflow: hidden; text-overflow: ellipsis; }
footer .msg { overflow: hidden; text-overflow: ellipsis; }
footer .msg.err { color: var(--danger); }
.details { display: none; flex-basis: 100%; padding: 6px 0 0; font-size: 11px; color: var(--fg2); border-top: 1px solid var(--divider);
  white-space: pre-wrap; text-align: left; font-family: "DM Mono", monospace; }
.win.details-open .details { display: block; }
/* docked: the preview fills the host's pane, in place of its PDF viewer */
.win.docked { position: relative; left: auto !important; top: auto !important; width: 100% !important; height: 100% !important;
  max-width: none; max-height: none; min-width: 0; min-height: 0; resize: none; border: 0; border-radius: 0; box-shadow: none; }
.win.docked header { height: 34px; background: var(--light2); color: var(--fg); border-bottom: 1px solid var(--divider); cursor: default; }
.win.docked header button.ib:hover { background: var(--light3); } .win.docked header button.ib[aria-pressed=true] { background: var(--light3); }
.win.docked .chip { background: var(--light3); }
.win.docked .badge { background: var(--light3); color: var(--fg2); }
.win.docked .hide-docked { display: none; }
.win:not(.docked) .show-docked { display: none; }
.win.docked { background: var(--pdf-bg, var(--pane, #2f3a4c)); border: 0; }
/* Overleaf's "dark mode PDF preview" (.pdf-dark-mode on the pane): its own filter, no shadow */
.win.docked.pdf-dark .stage svg.page, .win.docked.pdf-dark .stage img { filter: invert(95%) hue-rotate(180deg) brightness(90%) contrast(90%); box-shadow: none; }
.win.docked.light .sum { background: var(--light2); color: var(--fg2); border-bottom-color: var(--divider); }
.win.docked.light .sum:hover { background: var(--light3); } .win.docked.light .sum .first { color: var(--fg); }
.win.docked.light .sum .n.error { color: var(--danger); } .win.docked.light .sum .n.warning { color: var(--warn); } .win.docked.light .sum .n.info { color: var(--info); }
.win.docked.light .empty { color: var(--fg2); }
.win.docked > header, .win.docked > footer { display: none; }
.win.docked > .bar { display: none; }
.win.docked.sheet-open > .bar { display: flex; position: absolute; z-index: 4; top: 6px; right: 8px; width: min(340px, calc(100% - 16px));
  flex-direction: column; align-items: stretch; gap: 8px; padding: 12px; border-radius: var(--r2); border: 1px solid var(--divider);
  background: var(--light); box-shadow: 0 8px 24px rgba(27,34,44,.3); animation: sheet .14s ease-out; }
.win.docked.sheet-open > .bar label { justify-content: space-between; } .win.docked.sheet-open > .bar .grow { display: none; }
.win.docked.sheet-open .details { display: block; }
@keyframes sheet { from { opacity: 0; transform: translateY(-4px); } }
.win.docked > .sum { display: none; }
.win.docked .diags { max-height: 60%; }
.pkgs { display: none; position: absolute; z-index: 4; left: 50%; transform: translateX(-50%); top: 10px; max-width: calc(100% - 24px); padding: 8px 12px; font-size: 13px; border-radius: 8px;
  background: var(--light); color: var(--fg); border: 1px solid var(--divider); border-left: 3px solid var(--accent); box-shadow: 0 4px 14px rgba(0,0,0,.28); }
.pkgs.on { display: flex; gap: 8px; align-items: center; }
.pkgs > span:not(.mini) { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pkgs .mini.busy i { width: 35% !important; animation: phx-busy 1.1s ease-in-out infinite; }
.pkgs .icon { color: var(--info); }
/* the loading card: packages downloading, then the first build */
.load { max-width: 360px; margin: 0 auto; text-align: left; color: var(--fg2-dark); }
.win.docked.light .load, .win:not(.docked) .load { color: var(--fg2); }
.load h3 { margin: 0 0 10px; font: 600 14px "Noto Sans", system-ui, sans-serif; color: inherit; }
.load .bar { display: block; height: 6px; padding: 0; border: 0; border-radius: 9999px; background: rgb(127 127 127 / 25%); overflow: hidden; }
.load .bar i { display: block; height: 100%; background: var(--accent); border-radius: inherit; transition: width .25s ease; }
.load .bar.busy i { width: 35% !important; animation: phx-busy 1.1s ease-in-out infinite; }
@keyframes phx-busy { from { transform: translateX(-100%); } to { transform: translateX(290%); } }
.load .count { margin: 8px 0 10px; font-size: 12px; font-variant-numeric: tabular-nums; }
.load .names { display: flex; flex-wrap: wrap; gap: 4px; min-height: 22px; }
.load .names span { font: 11px "DM Mono", monospace; padding: 1px 6px; border-radius: 4px; background: rgb(127 127 127 / 18%); }
.load .names span.ing { animation: phx-pulse 1s ease-in-out infinite alternate; }
.load .names span.ok { color: var(--accent); }
@keyframes phx-pulse { from { opacity: .45; } to { opacity: 1; } }
.load .note { margin-top: 12px; font-size: 11px; opacity: .8; }
.load .steps { display: flex; gap: 6px; margin: 0 0 12px; font-size: 11px; }
.load .steps span { display: flex; align-items: center; gap: 4px; opacity: .5; }
.load .steps span::before { content: ""; width: 8px; height: 8px; border-radius: 50%; border: 1.5px solid currentColor; box-sizing: border-box; }
.load .steps span.now { opacity: 1; font-weight: 600; }
.load .steps span.now::before { background: var(--accent); border-color: var(--accent); animation: phx-pulse .8s ease-in-out infinite alternate; }
.load .steps span.ok { opacity: .8; }
.load .steps span.ok::before { background: currentColor; }
.load .steps i { flex: 1; align-self: center; height: 1px; background: currentColor; opacity: .25; font-style: normal; }
.load .bar:not(.busy) i { background-image: linear-gradient(110deg, transparent 30%, rgb(255 255 255 / 35%) 50%, transparent 70%); background-size: 200% 100%; animation: phx-shine 1.4s linear infinite; }
@keyframes phx-shine { from { background-position: 150% 0; } to { background-position: -50% 0; } }
.load .alive { margin-top: 8px; font-size: 12px; display: flex; gap: 6px; align-items: center; }
.load .alive::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
@media (prefers-reduced-motion: reduce) { .load .bar i, .load .steps span.now::before, .load .alive::before { animation: none !important; } }
.pkgs .mini { flex: none; width: 80px; height: 4px; border-radius: 9999px; background: rgb(127 127 127 / 25%); overflow: hidden; }
.pkgs .mini i { display: block; height: 100%; background: var(--accent); }
.speedchip { display: none; position: absolute; right: 16px; bottom: 14px; z-index: 3; pointer-events: none; white-space: nowrap;
  height: 24px; padding: 0 10px; border-radius: 9999px; align-items: center; gap: 4px; font-size: 12px; font-weight: 600;
  font-variant-numeric: tabular-nums; color: #fff; background: var(--accent); box-shadow: 0 4px 12px rgba(0,0,0,.3); opacity: 0; }
.win.docked .speedchip { display: inline-flex; }
.win.docked.nospeed .speedchip { display: none; }
/* who made this, always said (it looks native on purpose; it must never pass for Overleaf's) */
.byline { display: none; position: absolute; left: 14px; bottom: 14px; z-index: 3; all: unset; }
.win.docked .byline { display: inline-block; position: absolute; left: 14px; bottom: 14px; z-index: 3; height: 22px; padding: 0 9px;
  max-width: calc(100% - 160px); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; box-sizing: border-box; /* clear of the ⚡ chip, now always on */
  border-radius: 9999px; font: 600 11px/22px "Noto Sans", system-ui, sans-serif; color: var(--fg2-dark); background: rgb(27 34 44 / 55%);
  backdrop-filter: blur(4px); cursor: pointer; }
.win.docked .byline:hover { color: var(--fg-dark); background: rgb(27 34 44 / 80%); }
.win.docked.light .byline { color: var(--fg2); background: rgb(255 255 255 / 75%); }
.about { display: none; position: absolute; left: 14px; bottom: 44px; z-index: 4; width: min(320px, calc(100% - 28px)); padding: 12px;
  border-radius: var(--r2); background: var(--dark); color: var(--fg-dark); font-size: 12px; line-height: 1.5; box-shadow: 0 8px 24px rgba(0,0,0,.35); }
.about.open { display: block; }
.about .about-foot { margin-top: 8px; color: var(--fg2-dark); }
.about code { font-size: 11px; }
.about a { color: var(--ok-dark); }
.speedchip.slow { background: var(--bg-warning-01, #8f5514); }
/* the chip: always there (the setting on), its number updated in place, a brief flash on each repaint */
.speedchip { opacity: 1; transition: background-color .4s; }
.speedchip.flash { animation: chipflash .5s ease-out; }
@keyframes chipflash { 0% { filter: brightness(1.45); } 100% { filter: none; } }
@media (prefers-reduced-motion: reduce) { .speedchip.flash { animation: none; } }
/* startup: the first paint is in, the SSA program (instant typing) still coming */
.warmup { display: none; position: absolute; left: 12px; right: 12px; margin: 0 auto; width: fit-content; top: 10px; z-index: 3; pointer-events: none;
  min-height: 24px; box-sizing: border-box; padding: 4px 12px; border-radius: 12px; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; line-height: 16px; text-align: center;
  font-variant-numeric: tabular-nums; color: #fff; background: rgb(27 34 44 / 82%); box-shadow: 0 4px 12px rgba(0,0,0,.3); backdrop-filter: blur(4px); }
.warmup.on { display: inline-flex; }
/* a diff shown: what against, the changes, and its controls (a pill over the page, above the warmup) */
.diffbar { display: none; position: absolute; left: 12px; right: 12px; margin: 0 auto; width: fit-content; max-width: calc(100% - 24px); top: 10px; z-index: 4;
  box-sizing: border-box; padding: 3px 4px 3px 12px; border-radius: 14px; align-items: center; gap: 4px; font-size: 12px; font-weight: 600; line-height: 16px;
  font-variant-numeric: tabular-nums; color: #fff; background: rgb(27 34 44 / 88%); box-shadow: 0 4px 12px rgba(0,0,0,.3); backdrop-filter: blur(4px); }
.diffbar.on { display: inline-flex; }
.diffbar.on ~ .warmup, .diffbar.on ~ .pkgs { top: 46px; }
.diffbar .what { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.diffbar .what small { font-weight: 400; opacity: .75; }
.diffbar .n { white-space: nowrap; padding: 0 4px; }
.diffbar .busy::before { content: ""; display: inline-block; width: 7px; height: 7px; margin-right: 6px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
.diffbar button { all: unset; cursor: pointer; padding: 2px 7px; border-radius: 10px; color: #fff; white-space: nowrap; }
.diffbar button:hover:not(:disabled) { background: rgb(255 255 255 / 16%); }
.diffbar button:disabled { opacity: .4; cursor: default; }
.diffbar button.cur { background: rgb(255 255 255 / 22%); }
.diffset { display: none; position: absolute; top: 44px; left: 50%; transform: translateX(-50%); z-index: 5; width: 300px; max-width: calc(100% - 24px);
  box-sizing: border-box; padding: 12px; border-radius: 12px; font-size: 12px; color: #fff; background: rgb(27 34 44 / 96%); box-shadow: 0 8px 24px rgba(0,0,0,.4); }
.diffset.on { display: block; }
.diffset h4 { margin: 0 0 6px; font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .04em; color: rgb(255 255 255 / 60%); }
.diffset h4:not(:first-child) { margin-top: 12px; }
.diffset .opts { display: flex; flex-wrap: wrap; gap: 4px; }
.diffset button { all: unset; cursor: pointer; padding: 3px 9px; border-radius: 10px; background: rgb(255 255 255 / 10%); color: #fff; }
.diffset button:hover { background: rgb(255 255 255 / 18%); }
.diffset button[aria-pressed=true] { background: var(--accent); }
.diffset .pair { display: flex; align-items: center; gap: 6px; padding: 3px 6px 3px 3px; }
.diffset .pair i { display: inline-block; width: 12px; height: 12px; border-radius: 50%; box-shadow: inset 0 0 0 1px rgb(255 255 255 / 30%); }
.diffset .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 6px; }
.diffset input[type=color] { width: 30px; height: 22px; padding: 0; border: 0; background: none; cursor: pointer; }
.diffset .sample { margin-top: 10px; padding: 6px 8px; border-radius: 6px; background: #fff; color: #1b222c; font: 13px/1.4 "Noto Serif", serif; }
.diffset .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 12px; }
.diffset .foot small { opacity: .6; }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px 2px; background: rgb(255 255 255 / 25%); }
.pkgs.on ~ .warmup { top: 56px; } /* below the strip, both said */
.warmup .dot { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
.warmup .slow { color: #ffd27a; font-weight: 500; }
.warmup.ready { background: var(--accent); }
.warmup.ready .dot { display: none; }
@media (prefers-reduced-motion: reduce) { .warmup .dot { animation: none; } }
.win.docked .sum { background: var(--dark2); color: var(--fg2-dark); border-bottom: 1px solid var(--dark); height: 26px; }
.win.docked .sum:hover { background: var(--dark3); }
.win.docked .sum .first { color: var(--fg-dark); }
.win.docked .sum .n.error { color: var(--danger-dark); } .win.docked .sum .n.warning { color: var(--warn-dark); } .win.docked .sum .n.info { color: #97b6e5; }
/* as Overleaf's pdf.js viewer: the scrollbar always there, pages 12 px apart and centered, and the
   viewer min-height 100% with the first page's margin collapsing through it (so it scrolls by 12 px, as native) */
.win.docked .viewer { min-height: 100%; }
.win.docked .stage { background: transparent; padding: 0; overflow-y: scroll; overflow-x: auto; }
.stage { overscroll-behavior: contain; will-change: scroll-position; }
/* (the slots' own rules, the raster's among them: the renderer's VIEWER_CSS, before these) */
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgba(27,34,44,.25); }
.load .btns { display: flex; gap: 8px; justify-content: center; margin-top: 10px; }
.load .btns .btn { cursor: pointer; padding: 4px 12px; border-radius: 9999px; border: 1px solid currentColor; background: none; color: inherit; font: inherit; font-weight: 600; }
.report { display: none; position: absolute; inset: 12px; z-index: 6; padding: 12px; flex-direction: column; gap: 6px; background: var(--bg, #fff); color: var(--fg, #1b222c); border-radius: 8px; box-shadow: 0 4px 24px rgba(0,0,0,.35); font-size: 12px; }
.report.on { display: flex; }
.report p { margin: 0; opacity: .8; }
.report textarea { flex: 1; min-height: 160px; font: 11px/1.35 ui-monospace, monospace; white-space: pre; resize: none; }
.report .btns { display: flex; gap: 8px; justify-content: flex-end; }
.report .btn, .diags .reportbtn { cursor: pointer; padding: 3px 12px; border-radius: 9999px; border: 1px solid currentColor; background: none; color: inherit; font: inherit; font-weight: 600; }
.diags .reportbtn { margin: 6px 8px; }
.slot .mark { position: absolute; background: rgba(255, 221, 0, .55); mix-blend-mode: multiply; border-radius: 3px; pointer-events: none;
  box-shadow: 0 0 0 1.5px rgba(232, 172, 0, .9), 0 0 10px 1px rgba(255, 200, 0, .55);
  transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; animation: phitex-mark-in .18s ease-out; }
@keyframes phitex-mark-in { from { opacity: 0; transform: scale(1.15); } }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; animation: none; }
.slot svg.page, .slot img { display: block; width: 100%; height: 100%; margin: 0; box-shadow: none !important; border-radius: 0 !important; }
.win.docked .slot { box-shadow: rgba(35,40,47,.05) 0 5px 5px, rgba(35,40,47,.03) 0 3px 14px, rgba(35,40,47,.08) 0 8px 10px; }
.win.docked.pdf-dark .slot { filter: invert(95%) hue-rotate(180deg) brightness(90%) contrast(90%); box-shadow: none; }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.win.docked .stage svg.page, .win.docked .stage img { border-radius: 0;
  box-shadow: rgba(35,40,47,.05) 0 5px 5px, rgba(35,40,47,.03) 0 3px 14px, rgba(35,40,47,.08) 0 8px 10px; }
.win.docked .banner { margin: 0; top: 0; }
.win.docked .empty { color: var(--fg2-dark); }
.win.collapsed { resize: none; min-height: 0; min-width: 0; border-radius: 9999px; }
.win.collapsed > :not(header) { display: none; }
.win.collapsed header { border-radius: 9999px; padding-right: 4px; }
.win.collapsed .hide-collapsed { display: none; }
`;

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
  private speed?: { ms: number; at: number };
  private debugOn = false;
  private ev!: PanelEvents;
  /** A page that came while text in the shown one was selected: shown once the selection ends. */
  private viewer!: Viewer;

  private $ = <T extends HTMLElement = HTMLElement>(sel: string) => this.root.querySelector(sel) as T;

  private words: PanelWords;

  constructor(ev: PanelEvents, store?: Prefs, opts: PanelOptions = {}) {
    this.store = store;
    this.ev = ev;
    const w = (this.words = { ...OVERLEAF_WORDS, ...opts.words });
    const host = document.createElement("phitex-preview");
    this.root = host.attachShadow({ mode: "open" });
    this.root.innerHTML = (`<style>${VIEWER_CSS}${CSS}</style>
<div class="win" role="dialog" aria-label="PhiTeX preview">
  <header>
    <span class="icon" aria-hidden="true">preview</span>
    <span class="title">PhiTeX</span>
    <span class="badge hide-collapsed" title="${w.badgeTitle}">${w.badge}</span>
    <span class="grow"></span>
    <span class="chip" id="chip" title="Status"><span class="dot"></span><span id="chiptext">starting…</span></span>
    <span class="group hide-collapsed">
      <button class="ib" id="prev" title="Previous page" aria-label="Previous page"><span class="icon">chevron_left</span></button>
      <span class="pno" id="pno">–</span>
      <button class="ib" id="next" title="Next page" aria-label="Next page"><span class="icon">chevron_right</span></button>
      <button class="ib" id="dbg" title="Debug: check against a fresh build every 5 s" aria-pressed="false"><span class="icon">bug_report</span></button>
    </span>
    <button class="ib hide-docked" id="min" title="Collapse (Alt+Shift+P)" aria-label="Collapse"><span class="icon">close_fullscreen</span></button>
  </header>
  <div class="bar">
    <label>Main <select id="main" title="The file PhiTeX typesets"></select></label>
    <label>Zoom <select id="zoom"><option value="fit">Fit</option><option value="0.75">75%</option><option value="1">100%</option><option value="1.5">150%</option><option value="2">200%</option></select></label>
    <label title="Vector: drawn as SVG, updated in place as you type. PDF.js: our PDF drawn by pdf.js, as Overleaf's viewer draws a PDF.">View <select id="fmt"><option value="vector">Vector</option><option value="pdfjs">PDF.js</option></select></label>
    <span class="grow"></span>
    <label class="show-docked" title="Check against a fresh build every 5 s"><input type="checkbox" id="dbg2"> Debug check</label>
    <button class="ib show-docked" id="tour" title="Take the tour" aria-label="Take the tour" style="width:auto;padding:0 6px;font-size:12px;color:var(--info)">Take the tour</button>
    <button class="ib show-docked" id="news" title="What's new" aria-label="What's new" style="width:auto;padding:0 6px;font-size:12px;color:var(--info)">What's new</button>
    <button class="ib" id="reload" title="Fetch the project's files again" aria-label="Reload files"><span class="icon">sync</span></button>
    <button class="btn" id="pdf" title="PhiTeX's PDF, made locally"><span class="icon">download</span>PDF</button>
    <div class="details" id="details"></div>
  </div>
  <div class="sum" id="sum" role="button" aria-expanded="false" tabindex="0" title="Diagnostics (click to list)"></div>
  <div class="body">
    <div class="diags" id="diags" role="list" aria-label="Diagnostics"></div>
    <div class="report" id="report" role="dialog" aria-label="Debug report">
      <b>Debug report</b>
      <p>Anonymized: no document text, nothing you typed, no file names (file1.tex, …), no project id. Read it before you send it.</p>
      <textarea id="reporttext" readonly spellcheck="false"></textarea>
      <div class="btns"><button class="btn" id="reportcopy">Copy</button><button class="btn" id="reportmail">Email</button><button class="btn" id="reportclose">Close</button></div>
    </div>
    <div class="speedchip" id="speedchip" aria-live="off">⚡ – ms</div>
    <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
    <div class="diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="pkgs" id="pkgs" role="status" aria-live="polite"></div>
    <div class="warmup" id="warmup" role="status" aria-live="polite"></div>
    <button class="byline" id="byline" title="About this preview">${w.byline}</button>
    <div class="about" id="about" role="dialog" aria-label="About the PhiTeX preview">
      ${w.about}
    </div>
    <div class="stage" id="stage"><div class="approx" id="approx"></div><div class="banner" id="banner"></div><div class="empty" id="empty"><div class="load"><div class="steps"><span class="now">Project read</span><i></i><span>Packages</span><i></i><span>Typesetting</span></div><h3>Reading the project…</h3><div class="bar busy"><i></i></div></div></div><div class="viewer" id="viewer"></div></div>
  </div>
  <footer><span class="lat" id="lat" title="Click for details">–</span><span class="grow"></span><span class="msg" id="msg">all local</span></footer>
</div>`).replace(/<span class="icon"(?: aria-hidden="true")?>(\w+)<\/span>/g, (_, n) => icon(n, n === "download" ? 16 : 18));
    this.host = host;
    // (hidden until the host docks it, or decides it floats: no flash of a window on load)
    host.style.display = "none";
    (document.body ?? document.documentElement).append(host);
    this.win = this.$(".win");
    this.$("#prev").onclick = () => this.prev();
    this.$("#next").onclick = () => this.next();
    this.$("#pdf").onclick = () => ev.onPdf();
    this.$("#reload").onclick = () => ev.onReload();
    const dbg = this.$("#dbg");
    dbg.onclick = () => this.toggleDebug();
    this.$("#dbg2").onchange = () => this.toggleDebug();
    const license = this.$("#license") as HTMLAnchorElement | null;
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    // (what the host doesn't handle, not offered)
    if (!ev.onTour) this.$("#tour").style.display = "none";
    if (!ev.onNews) this.$("#news").style.display = "none";
    if (opts.pdfjs === false) this.$("#fmt").closest("label")!.style.display = "none";
    this.$("#byline").onclick = (e) => {
      e.stopPropagation();
      this.$("#about").classList.toggle("open");
    };
    this.$("#stage").addEventListener("pointerdown", () => this.$("#about").classList.remove("open"));
    this.$("#news").onclick = () => {
      this.sheet(false);
      ev.onNews?.();
    };
    this.$("#tour").onclick = () => {
      this.sheet(false);
      ev.onTour?.();
    };
    this.$<HTMLSelectElement>("#main").onchange = (e) => ev.onMain((e.target as HTMLSelectElement).value);
    this.$<HTMLSelectElement>("#zoom").onchange = (e) => this.zoomTo((e.target as HTMLSelectElement).value);
    this.$<HTMLSelectElement>("#fmt").onchange = (e) => {
      this.prefs.format = (e.target as HTMLSelectElement).value as PanelPrefs["format"];
      this.save();
      this.viewer.invalidate();
      ev.onFormat(this.prefs.format);
    };
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#chip").onclick = () => this.prefs.collapsed && this.collapse(false);
    this.$("#lat").onclick = () => {
      this.prefs.details = !this.prefs.details;
      this.win.classList.toggle("details-open", this.prefs.details);
      this.save();
    };
    this.$("#diags").onclick = (e) => {
      const d = (e.target as HTMLElement).closest<HTMLElement>(".diag[data-line]");
      if (d) ev.onGoto(d.dataset.file!, Number(d.dataset.line));
    };
    const sum = this.$("#sum");
    sum.onclick = () => this.diagsOpen(!this.win.classList.contains("diags-open"));
    sum.onkeydown = (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), sum.click());
    this.$("#stage").addEventListener("pointerdown", () => {
      this.diagsOpen(false);
      this.sheet(false);
    });
    this.root.addEventListener("keydown", (e) => (e as KeyboardEvent).key === "Escape" && this.diagsOpen(false));
    window.addEventListener("keydown", (e) => {
      if (e.altKey && e.shiftKey && e.code === "KeyP") {
        e.preventDefault();
        if (!ev.onShortcut?.()) this.collapse(!this.prefs.collapsed);
      }
    });
    this.drag();
    this.viewer = new Viewer(this.$("#viewer"), this.$("#stage"), {
      need: (k) => ev.onNeed?.(k),
      inView: (k) => {
        this.at = k;
        this.nav();
        ev.onPage(k);
      },
      svg: (img, w) => ("draws" in img ? svg(img.draws, w) : null),
      scale: () => this.scale(),
      box: (d, w) => pageBox(d, w),
      dbl: (k, x, y) => ev.onSyncSource?.(k, x, y),
      selected: (sel) => ev.onSelectPage?.(sel),
      link: ev.onLink ? (u) => ev.onLink!(u) : undefined,
    });
    // (keep it on screen when the window shrinks)
    window.addEventListener("resize", () => this.place());
    new ResizeObserver(() => {
      if (this.docked) return this.prefs.zoom === "fit" ? this.redraw() : undefined;
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
    store?.load().then((p) => {
      this.prefs = { ...DEFAULTS, ...p };
      // (an older version's "png", gone: vector)
      this.prefs.format = pageFormat(this.prefs.format);
      // (a size saved while hidden, by an older version)
      if (this.prefs.w < 300 || this.prefs.h < 180) Object.assign(this.prefs, { w: DEFAULTS.w, h: DEFAULTS.h });
      this.place();
      if (this.prefs.format !== "vector") ev.onFormat(this.prefs.format);
    });
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
      this.win.classList.toggle("light", document.body.dataset.theme === "light");
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
    this.$<HTMLSelectElement>("#zoom").value = p.zoom;
    this.$<HTMLSelectElement>("#fmt").value = p.format;
    this.win.classList.toggle("details-open", p.details);
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

  /** What the strip over the page says: packages on their way, or a build that is taking a while. */
  private pkgNow?: { names: string[]; done: number; total: number; pct: number; source: string };
  private buildSince = 0;
  private actTick: ReturnType<typeof setInterval> | undefined;

  private prepSince = 0;
  /** The core readying its next rebuild: past 400 ms the strip says so, not "Typesetting". */
  preparing(on: boolean): void {
    this.prepSince = on ? this.prepSince || performance.now() : 0;
    this.activity();
  }

  /** A build started (`on`) or ended: past 400 ms the strip says so, with its time. */
  /** A streamed open's pages so far (the strip: which page it is on). */
  private streamed: number | undefined;
  streaming(s: { pages: number; phase: string } | null): void {
    this.streamed = s ? s.pages : undefined;
    this.activity();
  }

  busy(on: boolean): void {
    this.$("#chip").classList.toggle("busy", on);
    this.buildSince = on ? this.buildSince || performance.now() : 0;
    this.activity();
  }

  /** The strip over the page: never silent while the writer waits. */
  private activity(): void {
    const el = this.$("#pkgs");
    const p = this.pkgNow;
    const prep = this.prepSince && performance.now() - this.prepSince > 400;
    const long = (this.buildSince && performance.now() - this.buildSince > 400) || prep;
    // (over a page on screen; before any page, the loading card says it all)
    const on = (!!this.last || this.viewer.pages > 0) && (!!p || !!long);
    el.classList.toggle("on", on);
    if (on || this.buildSince || this.prepSince) this.actTick ??= setInterval(() => this.activity(), 250);
    else (clearInterval(this.actTick), (this.actTick = undefined));
    if (!on) return;
    if (p) {
      const names = p.names.slice(0, 3).join(", ") + (p.names.length > 3 ? ` and ${p.names.length - 3} more` : "");
      el.innerHTML = `${icon("download", 16)}<span></span><span class="mini"><i style="width:${p.pct}%"></i></span>`;
      el.querySelector("span")!.textContent = `Fetching ${names} from ${p.source} (${p.done} of ${p.total})…`;
    } else if (prep) {
      // (a build waiting behind it is part of the same wait)
      const s = (performance.now() - this.prepSince) / 1000;
      el.innerHTML = `${icon("sync", 16)}<span></span><span class="mini busy"><i></i></span>`;
      el.querySelector("span")!.textContent = `Getting ready for instant edits… ${s < 10 ? s.toFixed(1) : Math.round(s)} s (pages still scroll)`;
    } else {
      const s = (performance.now() - this.buildSince) / 1000;
      el.innerHTML = `${icon("sync", 16)}<span></span><span class="mini busy"><i></i></span>`;
      const page = this.streamed ? ` page ${this.streamed + 1}` : "";
      el.querySelector("span")!.textContent = `Typesetting…${page} ${s < 10 ? s.toFixed(1) : Math.round(s)} s`;
    }
  }

  /** Packages still on their way (the loading card's), so a stop reads as one only once they are in. */
  private pkgBusy = false;

  /**
   * Before any page: a build that stopped at an error says so, in place of
   * "Typesetting…" (a stopped job's PDF is unfinished: no page to draw).
   */
  private stopped(): boolean {
    if (this.engineNow && !this.engineNow.ready) return this.engineCard(this.engineNow.engine, null);
    if (this.last || this.pkgBusy) return false;
    const e = this.lastStatus?.diagnostics?.find((d) => d.severity === "error");
    if (!e) return false;
    // (fontspec, unicode-math, polyglossia: a XeLaTeX/LuaLaTeX project, which pdfTeX can't run)
    const needed = errorNeeds(e.message);
    if (needed) return this.engineCard(needed, e.message);
    const empty = this.$("#empty");
    empty.innerHTML = `<div class="load"><h3></h3><div class="count"></div><div class="note"></div></div>`;
    empty.querySelector("h3")!.textContent = "The build stopped before a page could be shown";
    empty.querySelector(".count")!.textContent = e.message + (e.file && e.line ? ` (${e.file}:${e.line})` : "");
    empty.querySelector(".note")!.textContent = this.words.stopped;
    this.reportLink(empty.querySelector(".load")!);
    empty.style.display = "";
    return true;
  }

  /** "Copy debug info" under a card, if reports are wired. */
  private reportLink(card: Element): void {
    if (!this.ev.onReport) return;
    const b = document.createElement("button");
    b.className = "reportbtn";
    b.textContent = "Copy debug info…";
    b.style.cssText = "margin-top:10px;cursor:pointer;padding:3px 12px;border-radius:9999px;border:1px solid currentColor;background:none;color:inherit;font:inherit;font-weight:600";
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
    a.style.display = this.approx ? "block" : "none";
    a.textContent = `Approximate: needs ${ENGINES[e.engine].label}, shown with pdfLaTeX (fonts substituted, breaks may differ). ${this.words.realPdf}`;
    if (!e.ready) this.stopped();
    else if (this.$("#empty").querySelector(".engine")) this.$("#empty").style.display = "none";
  }

  /**
   * The project needs `want` (its preamble, or `error`, a build that
   * stopped, says so): which engine to run it with, as buttons.
   */
  private engineCard(want: Engine, error: string | null): boolean {
    const empty = this.$("#empty");
    const ready = ENGINES[want].ready;
    empty.innerHTML = `<div class="load engine"><h3></h3><div class="count"></div><div class="note"></div><div class="btns"></div></div>`;
    empty.querySelector("h3")!.textContent = error
      ? `This project needs ${want === "lualatex" ? "LuaLaTeX" : "XeLaTeX or LuaLaTeX"}`
      : `This project runs with ${ENGINES[want].label}`;
    empty.querySelector(".count")!.textContent = error ?? "Its preamble loads a package that pdfLaTeX can't run (fontspec, unicode-math, polyglossia, …), or the engine was chosen for it.";
    empty.querySelector(".note")!.textContent = ready
      ? `Run it with ${ENGINES[want].label}?`
      : `⚡ Instant doesn't run ${ENGINES[want].label} yet: only pdfLaTeX. ${this.words.notReady}, or try pdfLaTeX if the project can do without those packages.`;
    const btns = empty.querySelector(".btns")!;
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
    empty.style.display = "";
    return true;
  }

  status(s: Status): void {
    this.lastStatus = s;
    queueMicrotask(() => this.stopped());
    const diags = s.diagnostics ?? [];
    const worst = diags[0]?.severity;
    const chip = this.$("#chip");
    const partial = !!s.pending || s.pages === 0;
    chip.className = `chip ${worst === "error" ? "error" : partial || worst === "warning" ? "warning" : "ok"}`;
    this.$("#chiptext").textContent = `${s.pages} page${s.pages === 1 ? "" : "s"}${s.pending ? " · partial" : ""}`;
    chip.title =
      (s.pending ? `${s.pending} parts not read (out of fuel or unsupported). ` : "") +
      (diags.length ? `${diags.length} diagnostic${diags.length === 1 ? "" : "s"}` : "No problems found") +
      (s.file ? ` · editing ${s.file}` : "");
    const list = this.$("#diags");
    list.innerHTML = "";
    for (const d of diags.slice(0, 50)) {
      const row = document.createElement("div");
      row.className = `diag ${d.severity}`;
      row.setAttribute("role", "listitem");
      if (d.file && d.line) {
        row.dataset.file = d.file;
        row.dataset.line = String(d.line);
        row.title = `Go to ${d.file}:${d.line}`;
      }
      row.innerHTML = `${icon(d.severity, 16)}<span class="text"></span><span class="where"></span>`;
      row.querySelector(".text")!.textContent = d.message;
      if (d.detail) {
        const pre = document.createElement("pre");
        pre.className = "detail";
        pre.textContent = d.detail;
        row.querySelector(".text")!.append(pre);
      }
      row.querySelector(".where")!.textContent = d.file ? `${d.file}${d.line ? `:${d.line}` : ""}` : "";
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
    // (one line, whatever the count: the counts by severity, and the worst)
    const sum = this.$("#sum");
    sum.style.display = diags.length ? "" : "none";
    if (!diags.length) this.diagsOpen(false);
    const count = (sev: string) => diags.filter((d) => d.severity === sev).length;
    sum.innerHTML = (["error", "warning", "info"] as const)
      .filter((k) => count(k))
      .map((k) => `<span class="n ${k}">${icon(k, 14)}${count(k)}</span>`)
      .join("") + `<span class="first"></span>${icon("chevron_right", 16).replace('class="icon"', 'class="icon caret"')}`;
    if (diags[0]) sum.querySelector(".first")!.textContent = diags[0].message;
    this.stale(s);
    queueMicrotask(() => this.emit());
  }

  /**
   * Packages downloading: said plainly, so a slow first build reads as the
   * download's (TeX Live's files, not the project's, not Overleaf's).
   */
  /** The loading card's clock: when loading began, the last file that arrived, its ticker. */
  private loadT0 = 0;
  private loadLast = 0;
  private loadDone = -1;
  private loadTick: ReturnType<typeof setInterval> | undefined;
  private loadPct = 0;

  /** The card's live line: time so far and the last arrival, so a long fetch reads as working, not stuck. */
  private alive(): void {
    const el = this.$("#empty").querySelector<HTMLElement>(".alive");
    if (!el || this.last) {
      clearInterval(this.loadTick);
      this.loadTick = undefined;
      return;
    }
    const now = performance.now(), s = Math.round((now - this.loadT0) / 1000), quiet = (now - this.loadLast) / 1000;
    const fetching = el.dataset.phase === "fetch";
    if (el.dataset.phase === "read") {
      el.textContent = `Reading · ${s} s`;
      return;
    }
    el.textContent = fetching
      ? quiet < 4
        ? `Still fetching · ${s} s · files arriving`
        : `Still fetching · ${s} s · a large package (TikZ, fonts) is on its way`
      : `Typesetting · ${s} s`;
    if (s >= 20 && fetching) el.textContent += " · only the first time: next visits reuse them";
  }

  /** The reading step's last state, and when its card last changed (redrawn at most every 50 ms). */
  private readNow: { h: string; detail?: string; done?: number; total?: number; pct?: number } | undefined;
  private readDrawn = 0;
  private readTimer: ReturnType<typeof setTimeout> | undefined;

  /**
   * Reading the project, before the packages: what is happening (the
   * download from Overleaf, each file unpacked, each figure handed to the
   * engine), the file in hand and how far, so the first seconds read as work.
   */
  reading(r: { h: string; detail?: string; done?: number; total?: number; pct?: number }): void {
    if (this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readNow = r;
    this.loadT0 ||= performance.now();
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
    const empty = this.$("#empty");
    let card = empty.querySelector<HTMLElement>(".load.reading");
    if (!card) {
      empty.innerHTML =
        `<div class="load reading"><div class="steps"><span class="now">Project</span><i></i><span>Packages</span><i></i><span>Typesetting</span></div>` +
        `<h3></h3><div class="bar busy"><i></i></div><div class="alive" data-phase="read"></div><div class="count"></div><div class="names"></div>` +
        `<div class="note">${this.words.reading}</div></div>`;
      card = empty.querySelector<HTMLElement>(".load.reading")!;
    }
    card.querySelector("h3")!.textContent = r.h;
    const pct = r.pct ?? (r.total ? Math.round((100 * (r.done ?? 0)) / r.total) : undefined);
    const bar = card.querySelector(".bar")!;
    bar.classList.toggle("busy", pct === undefined);
    bar.querySelector("i")!.style.width = pct === undefined ? "" : `${pct}%`;
    card.querySelector(".count")!.textContent = r.total ? `${r.done ?? 0} of ${r.total}` : "";
    const names = card.querySelector(".names")!;
    names.innerHTML = "";
    if (r.detail) {
      const c = document.createElement("span");
      c.className = "ing";
      c.textContent = r.detail;
      names.append(c);
    }
    this.alive();
    this.loadTick ??= setInterval(() => this.alive(), 1000);
  }

  packages(p: PackageState): void {
    const done = p.done?.length ?? 0;
    const total = done + p.loading.length;
    const now = performance.now();
    if (!this.loadT0) this.loadT0 = now;
    if (done !== this.loadDone) {
      this.loadDone = done;
      this.loadLast = now;
    }
    // (never backwards: a build asking for more grows the total)
    const pct = (this.loadPct = Math.max(this.loadPct, total ? Math.round((100 * done) / total) : 100));
    // (before the first page: a loading card in the stage)
    if (!this.last) {
      const empty = this.$("#empty");
      if (!p.loading.length && !p.building) {
        // (done: the build's own state says the rest; a card left at "163 of 164" would not.
        // Files not found are optional ones LaTeX only checks for (amsart.cfg, …): the
        // diagnostics list them, and an error says when one was needed)
        this.pkgBusy = false;
        // (the same card as while fetching, at its last step: one screen until the page)
        if (!this.stopped()) {
          empty.innerHTML =
            `<div class="load"><div class="steps"><span class="ok">Project read</span><i></i><span class="ok">Packages</span><i></i><span class="now">Typesetting</span></div>` +
            `<h3>Typesetting…</h3><div class="bar busy"><i></i></div><div class="alive" data-phase="tex"></div></div>`;
          this.loadT0 ||= performance.now();
          this.alive();
          this.loadTick ??= setInterval(() => this.alive(), 1000);
        }
        return;
      }
      const recent = [...(p.done ?? []).slice(-10).map((n) => [n, "ok"]), ...p.loading.slice(0, 8).map((n) => [n, "ing"])];
      const fetch = p.loading.length > 0;
      this.pkgBusy = true;
      empty.innerHTML =
        `<div class="load"><div class="steps"><span class="ok">Project read</span><i></i><span class="${fetch ? "now" : "ok"}">Packages</span><i></i><span class="${fetch ? "" : "now"}">Typesetting</span></div>` +
        `<h3></h3><div class="bar${fetch ? "" : " busy"}"><i style="width:${pct}%"></i></div>` +
        `<div class="alive" data-phase="${fetch ? "fetch" : "tex"}"></div>` +
        `<div class="count"></div><div class="names"></div><div class="note"></div></div>`;
      this.alive();
      this.loadTick ??= setInterval(() => this.alive(), 1000);
      empty.querySelector("h3")!.textContent = p.loading.length ? "Fetching LaTeX packages…" : "Typesetting…";
      empty.querySelector(".count")!.textContent = p.loading.length
        ? `${done} of ${total} files${p.unavailable.length ? ` · ${p.unavailable.length} not found` : ""}`
        : `with ${done} package file${done === 1 ? "" : "s"}; a build may ask for more`;
      const names = empty.querySelector(".names")!;
      for (const [n, k] of recent) {
        const c = document.createElement("span");
        c.className = k;
        c.textContent = n;
        names.append(c);
      }
      empty.querySelector(".note")!.textContent =
        `From ${p.source} (fetched once, then kept in ${this.words.keptIn}). Only package names leave it, never your project's files.`;
      return;
    }
    this.pkgNow = p.loading.length ? { names: p.loading, done, total, pct, source: p.source } : undefined;
    this.activity();
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

  /** Follow what the panel shows (a host toolbar's controls). */
  onState(cb: (s: ViewState) => void): void {
    this.listeners.push(cb);
    cb(this.state());
  }

  private state(): ViewState {
    const chip = this.$("#chip");
    return {
      page: this.at,
      pages: this.lastStatus?.pages ?? 0,
      zoom: this.prefs.zoom,
      debug: this.debugOn,
      level: ["error", "warning", "ok"].find((c) => chip.classList.contains(c)) ?? "ok",
      chip: this.$("#chiptext").textContent ?? "",
      chipTitle: chip.title,
      speed: this.speed,
      sheet: this.win.classList.contains("sheet-open"),
      // (the badge counts what needs a look: info is listed, not counted)
      diagCount: this.lastStatus?.diagnostics?.filter((d) => d.severity !== "info").length ?? 0,
      diagWorst: this.lastStatus?.diagnostics?.[0]?.severity ?? "",
      percent: Math.round(this.scale() * 100),
    };
  }

  private emit(): void {
    const s = this.state();
    for (const f of this.listeners) f(s);
  }

  /**
   * The zoom, 1 being 100% (a PDF point at 96/72 CSS pixels). Fit is
   * pdf.js's "page-width", as Overleaf's viewer: (clientWidth − 40) / the
   * page's width, 40 being its SCROLLBAR_PADDING.
   */
  private scale(): number {
    if (this.prefs.zoom !== "fit") return Number(this.prefs.zoom);
    const d = this.last && "draws" in this.last ? this.last.draws : null;
    const pageW = (d?.w ?? 612) * (96 / 72);
    return Math.max(this.$("#stage").clientWidth - (this.docked ? 40 : 24), 200) / pageW;
  }

  goPage(k: number): void {
    this.viewer.goTo(k);
  }

  /** One zoom step in or out, from where it is (as Overleaf's − +). */
  zoomStep(dir: 1 | -1): void {
    const steps = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
    const now = this.scale();
    const next = dir > 0 ? steps.find((z) => z > now + 0.01) : [...steps].reverse().find((z) => z < now - 0.01);
    this.zoomTo(String(next ?? now));
  }

  /** The diagnostics drawer. */
  diagnostics(on = !this.win.classList.contains("diags-open")): void {
    this.diagsOpen(on);
    this.emit();
  }

  prev(): void {
    this.viewer.goTo(this.at - 1);
  }
  next(): void {
    this.viewer.goTo(this.at + 1);
  }
  clean(): void {
    this.ev.onClean();
  }

  pdf(): void {
    this.ev.onPdf();
  }

  zoomTo(z: string): void {
    this.prefs.zoom = z;
    this.$<HTMLSelectElement>("#zoom").value = z;
    this.save();
    this.redraw();
    this.emit();
  }

  /** The page format, set from outside (the popup). */
  formatTo(f: PanelPrefs["format"]): void {
    if (this.prefs.format === f) return;
    this.prefs.format = f;
    this.$<HTMLSelectElement>("#fmt").value = f;
    this.viewer.invalidate();
    this.ev.onFormat(f);
  }

  toggleDebug(): void {
    this.debugOn = !this.debugOn;
    this.$("#dbg").setAttribute("aria-pressed", String(this.debugOn));
    this.$<HTMLInputElement>("#dbg2").checked = this.debugOn;
    this.ev.onDebug(this.debugOn);
    this.msg(this.debugOn ? "debug check on" : "debug check off");
    this.emit();
  }

  /** The settings sheet (docked: main file, view, reload, the latency details). */
  sheet(on = !this.win.classList.contains("sheet-open")): void {
    this.win.classList.toggle("sheet-open", on && !!this.docked);
    this.emit();
  }

  /** The ⚡ chip shown or not (the popup's setting). */
  speedChip(on: boolean): void {
    this.win.classList.toggle("nospeed", !on);
  }

  /** A repaint reached the screen, `ms` after its keystroke: the ⚡ chip says how fast. */
  painted(ms: number): void {
    this.speed = { ms, at: performance.now() };
    const chip = this.$("#speedchip");
    chip.textContent = `⚡ ${ms < 10 ? ms.toFixed(1) : Math.round(ms)} ms`;
    chip.classList.toggle("slow", ms > 100);
    chip.classList.remove("flash");
    void chip.offsetWidth;
    chip.classList.add("flash");
    this.emit();
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

  private warmTimer?: ReturnType<typeof setInterval>;
  /** Startup: the first paint is in, instant typing still being prepared (a pill over the page, the time going); then "ready", briefly. */
  warmup(w: { state: "preparing" | "ready"; since?: number; slow?: boolean }): void {
    const el = this.$("#warmup");
    clearInterval(this.warmTimer);
    if (w.state === "ready") {
      el.className = "warmup on ready";
      el.innerHTML = `<span class="dot"></span>⚡ Instant typing ready`;
      this.warmTimer = setTimeout(() => (el.className = "warmup"), 2500) as unknown as ReturnType<typeof setInterval>;
      return;
    }
    const since = w.since ?? Date.now();
    const tick = () => {
      const s = Math.round((Date.now() - since) / 1000);
      el.innerHTML = `<span class="dot"></span><span>⚡ Preview ready · preparing instant typing… ${s} s${w.slow ? ` <span class="slow">· edits slower until ready</span>` : ""}</span>`;
    };
    el.className = "warmup on";
    tick();
    this.warmTimer = setInterval(tick, 1000);
  }

  /** The diagnostics list: a drawer over the page, so it never takes the page's room. */
  private diagsOpen(on: boolean): void {
    this.win.classList.toggle("diags-open", on);
    this.$("#sum").setAttribute("aria-expanded", String(on));
  }

  latency(summary: string, details?: string): void {
    this.$("#lat").textContent = summary;
    if (details) this.$("#details").textContent = details;
  }

  msg(t: string, err = false): void {
    const m = this.$("#msg");
    m.textContent = t;
    m.className = `msg${err ? " err" : ""}`;
    m.title = t;
    m.onclick = null;
    m.style.cursor = "";
  }

  error(t: string): void {
    this.msg(t, true);
    // (before any page: the loading card says it stopped, and why, not "Reading" forever)
    if (!this.last && !this.pkgBusy) {
      clearInterval(this.loadTick);
      this.loadTick = undefined;
      const empty = this.$("#empty");
      empty.innerHTML = `<div class="load"><h3>Couldn't open the project</h3><div class="count"></div><div class="note"></div></div>`;
      empty.querySelector(".note")!.textContent = this.words.openFailed;
      empty.querySelector(".count")!.textContent = t;
      if (this.ev.onReport) {
        const b = document.createElement("button");
        b.className = "reportbtn";
        b.textContent = "Report a problem…";
        b.onclick = () => void this.report();
        empty.querySelector(".load")!.append(b);
      }
    }
    // (an error line opens the debug report, to send)
    if (this.ev.onReport) {
      const m = this.$("#msg");
      m.title = `${t} — click for a debug report to send`;
      m.style.cursor = "pointer";
      m.onclick = () => void this.report();
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

  private redraw(): void {
    this.viewer?.redraw();
  }

  /** ⌃ ⌄ and the page number, for the page in view. */
  private nav(): void {
    const n = this.viewer.pages;
    const k = this.at;
    this.$("#pno").textContent = n ? `${k + 1} / ${n}` : "– / 0";
    this.$<HTMLButtonElement>("#prev").disabled = k <= 0;
    this.$<HTMLButtonElement>("#next").disabled = k >= n - 1;
    queueMicrotask(() => this.emit());
  }

  /** The pages there are, by hash (the viewer draws the changed ones it shows). */
  layout(hashes: string[]): void {
    this.$("#empty").style.display = "none";
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes);
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
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

  /** The debug report, shown whole before it is copied or emailed. */
  async report(): Promise<void> {
    const text = (await this.ev.onReport?.()) ?? "";
    const box = this.$("#report");
    this.$<HTMLTextAreaElement>("#reporttext").value = text;
    box.classList.add("on");
    const copy = () => navigator.clipboard.writeText(text).catch(() => this.$<HTMLTextAreaElement>("#reporttext").select());
    this.$("#reportcopy").onclick = () => void copy().then(() => (this.$("#reportcopy").textContent = "Copied ✓"));
    const mail = this.$("#reportmail");
    mail.style.display = REPORT_TO ? "" : "none";
    // (a mailto body is short in most mail apps: the report goes by the clipboard, pasted)
    mail.onclick = () =>
      void copy().then(() => {
        const body = "Please paste the debug report here (it is on your clipboard), and say what you were doing:\n\n";
        window.open(`mailto:${REPORT_TO}?subject=${encodeURIComponent("PhiTeX Instant debug report")}&body=${encodeURIComponent(body)}`);
      });
    this.$("#reportclose").onclick = () => box.classList.remove("on");
  }

  /** Highlight `boxes` on page `k` (the source the editor is at). */
  mark(k: number, boxes: [number, number, number, number][], scroll = true): void {
    this.viewer.mark(k, boxes, scroll);
  }

  page(img: PageImage | null, k: number, n: number, hash?: string | null): void {
    const empty = !img || ("draws" in img && !img.draws.t.length && !img.draws.r.length);
    if (empty) {
      // (nothing new to show: the pages shown stay, dimmed; see stale())
      if (!this.last) {
        this.$("#empty").textContent = n ? "This page is empty." : "No page shipped yet.";
        this.$("#empty").style.display = "";
      }
      if (this.lastStatus) this.stale({ ...this.lastStatus, pages: n });
      return;
    }
    this.last = img;
    this.$("#empty").style.display = "none";
    this.$("#stage").classList.remove("stale");
    this.$("#stage").classList.toggle("fit", this.prefs.zoom === "fit");
    // (a view with no layout yet: this page's slots, up to it)
    if (this.viewer.pages <= k) this.viewer.layout(Array.from({ length: Math.max(n, k + 1) }, (_, i) => (i === k && hash) || `?${i}`));
    this.viewer.set(k, img, hash ?? null);
    this.nav();
  }
}
