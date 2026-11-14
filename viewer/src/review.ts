// Reviewing with an agent (docs/spec-editor-protocol.md §9): the notes the
// user leaves on the pages and the agent's edits under review, beside the
// pages. The host keeps them (the watch: `.phitex/notes.json`, the
// baseline) and tells their state whole after each change (`review`
// event); this draws it and asks the host for what the user does
// (`review_*` ops). PhiTeX never calls an AI: the agent is the user's own,
// in a terminal, reading the notes with `phitex session notes`.
//
// - Text selected on a page offers Highlight, Note and Comment (a request
//   to the agent); Alt+click notes a spot (a figure). Each note is drawn
//   over its glyphs (its source range through the pages' origins, as a
//   problem's underline) with a pin at the page's edge.
// - The Notes tab: the notes, Open / Resolved / All, their threads, reply,
//   resolve, Clear resolved; a note whose text is gone is shown detached.
// - The Changes tab: the agent's edits as hunks (grouped by the note they
//   answered), Accept and Reject each or all, comments on a hunk (at once,
//   or gathered as a review submitted with a verdict), and Show changes:
//   Compare against the accepted text, typeset.

import { html } from "./html.ts";
import type { Viewer } from "./viewer.ts";

type Box = [number, number, number, number];

export interface ReviewPost {
  by: string;
  text: string;
  at: string;
  edited?: { file: string; start: number; end: number; from_line: number; to_line: number }[];
  diff?: { id: string; file: string; line: number; old: string; new: string };
}

export interface ReviewNote {
  id: string;
  kind: "highlight" | "note" | "comment";
  anchor: { file: string; start: number; end: number; quote: string; before: string; after: string; line: number; page?: number; point?: [number, number] };
  detached: boolean;
  status: "open" | "resolved";
  created: string;
  updated: string;
  thread: ReviewPost[];
  hunk?: { id: string; file: string; line: number; old: string; new: string };
  draft?: boolean;
  review?: string;
  /** The hunks of the agent's edits this note's thread recorded. */
  hunks?: string[];
  /** A comment on a change accepted or rejected since. */
  verdict?: { verdict: "accepted" | "rejected"; at: string };
}

export interface ReviewHunk {
  id: string;
  file: string;
  line: number;
  end_line: number;
  now: [number, number];
  parts: ["same" | "del" | "add", string][];
  before: string;
  after: string;
  old: string;
  new: string;
  whole: "added" | "deleted" | null;
  note: string | null;
  comments: string[];
}

export interface ReviewRec {
  id: string;
  verdict: "approve" | "request_changes" | "reject" | "comment";
  summary: string;
  at: string;
  notes: string[];
  hunks: string[];
}

export interface ReviewVerdict {
  hunk: { id: string; file: string; line: number; old: string; new: string };
  verdict: "accepted" | "rejected";
  at: string;
  review?: string;
}

export interface ReviewState {
  notes: ReviewNote[];
  hunks: ReviewHunk[];
  reviews: ReviewRec[];
  /** The verdicts, newest first. */
  verdicts?: ReviewVerdict[];
  git: boolean;
}

/** Where the review's state lives (the watch), and what the pages give it. */
export interface ReviewHost {
  /** A `review_*` op: its answer, or why not. */
  request(op: string, params?: object): Promise<{ ok: boolean; json?: any; error?: string }>;
  /** The state, whenever it changes (the `review` event). */
  onState(cb: (s: ReviewState) => void): void;
  /** Where bytes [start, end) of `file` are on the pages; `quote`, its text, looked for on the pages where no glyph names them. */
  boxes(file: string, start: number, end: number, quote?: string): Promise<{ k: number; boxes: Box[] }[]>;
  /** The source of text selected on the pages. */
  source(sel: { k: number; rects: Box[] }[]): Promise<{ file: string; start: number; end: number } | null>;
  /** The source place nearest a point of a page. */
  spot(k: number, x: number, y: number): Promise<{ file: string; start: number } | null>;
  /** The editor to `file`'s `line`. */
  goto(file: string, line: number): void;
  /** Compare against the accepted text, typeset (true), or back to the pages (false). */
  showChanges?(on: boolean, git: boolean): void;
}

/** The panel's part the review needs. */
export interface ReviewPanel {
  root: ShadowRoot;
  win: HTMLElement;
  stage: HTMLElement;
  viewer: Viewer;
  /** A short message in the corner. */
  msg(t: string, err?: boolean): void;
  /** The side pane shown or hidden (the panel lays the pages out again). */
  laidOut(): void;
}

const KIND_WORD: Record<ReviewNote["kind"], string> = { highlight: "Highlight", note: "Note", comment: "Comment" };
const VERDICT_WORD: Record<ReviewRec["verdict"], string> = { approve: "Approved", request_changes: "Changes requested", reject: "Rejected", comment: "Commented" };

/** Material icons (Apache-2.0) the review draws, inline. */
const PATHS: Record<string, string> = {
  rate_review: "M20 2H4c-1.1 0-1.99.9-1.99 2L2 22l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 14v-2.47l6.88-6.88c.2-.2.51-.2.71 0l1.77 1.77c.2.2.2.51 0 .71L8.47 14H6zm12 0h-7.5l2-2H18v2z",
  close: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  check: "M9 16.17 4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z",
  undo: "M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8z",
  delete: "M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z",
  highlight: "M6 14 3 17v3h3l3-3v-1l7.5-7.5-3-3L6 16zm13.71-8.29-2.42-2.42a1 1 0 0 0-1.41 0L14 5.17l3 3 1.71-1.71a1 1 0 0 0 0-1.46zM4 22h16v-1H4z",
  note: "M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04a.996.996 0 0 0 0-1.41l-2.34-2.34a.996.996 0 0 0-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z",
  comment: "M21.99 4c0-1.1-.89-2-1.99-2H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h14l4 4-.01-18zM18 14H6v-2h12v2zm0-3H6V9h12v2zm0-3H6V6h12v2z",
  link: "M3.9 12c0-1.71 1.39-3.1 3.1-3.1h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-1.9H7c-1.71 0-3.1-1.39-3.1-3.1zM8 13h8v-2H8v2zm9-6h-4v1.9h4c1.71 0 3.1 1.39 3.1 3.1s-1.39 3.1-3.1 3.1h-4V17h4c2.76 0 5-2.24 5-5s-2.24-5-5-5z",
};
const icon = (name: string, size = 16) =>
  html`<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name] ?? ""}"></path></svg>`;

/** "now", "5 min ago", "3 h ago", "2 d ago". */
export function ago(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, (now - t) / 1000);
  if (s < 45) return "now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** A source quote on one line, its runs of white space one space, cut to `max` characters. */
export function oneLine(s: string, max = 160): string {
  const t = s.replace(/\s+/g, " ").trim();
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

/** "main.tex:12" or "main.tex:12–14". */
export function placeOf(file: string, a: number, b = a): string {
  return a === b ? `${file}:${a}` : `${file}:${a}–${b}`;
}

/** The UTF-8 length of `s` (anchors are bytes of the file, as the glyph origins are). */
const utf8 = (s: string) => new TextEncoder().encode(s).length;

/** What the hunks are under: the note they answered, else their file. */
export function groupHunks(s: ReviewState): { key: string; title: string; note?: ReviewNote; hunks: ReviewHunk[] }[] {
  const groups = new Map<string, { key: string; title: string; note?: ReviewNote; hunks: ReviewHunk[] }>();
  for (const h of s.hunks) {
    const note = h.note ? s.notes.find((n) => n.id === h.note) : undefined;
    const key = note ? `n:${note.id}` : `f:${h.file}`;
    let g = groups.get(key);
    if (!g) groups.set(key, (g = { key, title: note ? `For your ${KIND_WORD[note.kind].toLowerCase()}: “${oneLine(note.thread.find((p) => p.by !== "agent" && p.text)?.text || note.anchor.quote, 60)}”` : h.file, note, hunks: [] }));
    g.hunks.push(h);
  }
  // (the notes' groups first, then the files', each as they come in the files)
  return [...groups.values()].sort((a, b) => Number(!a.note) - Number(!b.note));
}

export const REVIEW_CSS = `
.win.review-on .body { flex-direction: row; }
.rside { display: none; flex: none; width: var(--rv-w, 340px); min-width: 260px; max-width: 70%; flex-direction: column; background: var(--ph-bar); border-left: 1px solid var(--ph-border); color: var(--ph-fg); min-height: 0; position: relative; }
.win.review-on .rside { display: flex; }
@container (max-width: 640px) { .win.review-on .rside { position: absolute; inset: 0 0 0 auto; width: min(100%, 360px); z-index: 6; box-shadow: -4px 0 16px rgb(0 0 0 / 22%); } }
.rside .rgrip { position: absolute; left: -3px; top: 0; bottom: 0; width: 6px; cursor: col-resize; z-index: 2; }
.rside header { display: flex; align-items: center; gap: 2px; padding: 4px 6px 0; border-bottom: 1px solid var(--ph-border); }
.rside .rtab { border: 0; background: none; padding: 7px 10px; border-bottom: 2px solid transparent; color: var(--ph-fg2); cursor: pointer; font-weight: 600; }
.rside .rtab[aria-selected="true"] { color: var(--ph-fg); border-bottom-color: var(--ph-accent); }
.rside .rtab .n { margin-left: 5px; font-weight: 500; font-variant-numeric: tabular-nums; color: var(--ph-fg2); }
.rside .ib { border: 0; background: none; padding: 4px; border-radius: var(--ph-radius); cursor: pointer; color: var(--ph-fg2); display: inline-flex; align-items: center; }
.rside .ib:hover { background: var(--ph-hover); color: var(--ph-fg); }
.rside .rbody { flex: 1; min-height: 0; overflow: auto; padding: 8px 8px 24px; }
.rside .rtools { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; padding: 2px 2px 8px; }
.rside .seg { display: inline-flex; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); overflow: hidden; }
.rside .seg button { border: 0; background: none; padding: 3px 9px; cursor: pointer; color: var(--ph-fg2); }
.rside .seg button[aria-pressed="true"] { background: var(--ph-hover); color: var(--ph-fg); font-weight: 600; }
.rside .btn, .rv-pop .btn, .rv-compose .btn, .rv-finish .btn { display: inline-flex; align-items: center; gap: 4px; line-height: 20px; border: 1px solid var(--ph-border); background: var(--ph-bar); color: var(--ph-fg); padding: 3px 10px; border-radius: var(--ph-radius); cursor: pointer; white-space: nowrap; }
.rside .btn:hover, .rv-pop .btn:hover, .rv-compose .btn:hover, .rv-finish .btn:hover { background: var(--ph-hover); }
.rside .btn.primary, .rv-compose .btn.primary, .rv-finish .btn.primary { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.rside .btn.primary:hover, .rv-compose .btn.primary:hover, .rv-finish .btn.primary:hover { background: var(--ph-accent-hover, var(--ph-accent)); }
.rside .btn.accept { color: #1a7f37; } .rside .btn.reject { color: var(--ph-danger); }
.rside .btn:disabled { opacity: .5; cursor: default; }
.rside .empty { color: var(--ph-fg2); padding: 18px 8px; line-height: 1.5; }
.rside .empty b { color: var(--ph-fg); }
.rside h3 { font-size: 11px; text-transform: none; letter-spacing: 0; margin: 12px 2px 6px; color: var(--ph-fg2); font-weight: 600; }
.rside .card { border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) + 2px); background: var(--ph-menu); margin: 0 0 8px; padding: 8px 10px; cursor: pointer; position: relative; }
.rside .card:hover { border-color: color-mix(in srgb, var(--ph-accent) 40%, var(--ph-border)); }
.rside .card.on { border-color: var(--ph-accent); box-shadow: 0 0 0 1px var(--ph-accent); cursor: default; }
.rside .card.resolved { opacity: .72; }
.rside .card .top { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ph-fg2); }
.rside .chip { font-size: 11px; font-weight: 600; padding: 0 6px; border-radius: var(--ph-pill); border: 1px solid currentColor; line-height: 16px; }
.rside .chip.comment { color: #b25e00; } .rside .chip.note { color: #2f6fd1; } .rside .chip.highlight { color: #8a7300; }
.rside .chip.draft { color: var(--ph-fg2); } .rside .chip.det { color: var(--ph-warning); } .rside .chip.done { color: #1a7f37; }
.rside .place { color: var(--ph-link); text-decoration: none; cursor: pointer; font-variant-numeric: tabular-nums; border: 0; background: none; padding: 0; font: inherit; }
.rside .place:hover { text-decoration: underline; }
.rside .quote { margin: 6px 0 2px; padding-left: 8px; border-left: 3px solid var(--q, #d0b400); color: var(--ph-fg); display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; overflow-wrap: anywhere; }
.rside .card.on .quote { -webkit-line-clamp: unset; }
.rside .quote.spot { color: var(--ph-fg2); font-style: italic; }
.rside .quote.comment { --q: #e08a1e; } .rside .quote.note { --q: #4b8cf0; }
.rside .post { margin-top: 6px; overflow-wrap: anywhere; white-space: pre-wrap; }
.rside .card:not(.on) .post:not(.update) { display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden; }
.rside .post .by { font-weight: 600; margin-right: 4px; }
.rside .post .by.agent { color: #7a4fc9; }
.rside .post .when { color: var(--ph-fg2); font-size: 11px; margin-left: 4px; }
.rside .post .edited { display: block; margin-top: 2px; font-size: 12px; color: var(--ph-fg2); }
.rside .post.update { font-size: 12px; color: var(--ph-fg2); }
.rside .more { color: var(--ph-fg2); font-size: 12px; margin-top: 4px; }
.rside .act { display: flex; gap: 6px; margin-top: 8px; flex-wrap: wrap; align-items: center; }
.rside textarea, .rv-compose textarea, .rv-finish textarea { width: 100%; min-height: 54px; resize: vertical; font: inherit; color: var(--ph-fg); background: var(--ph-bar); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 6px 8px; }
.rside textarea:focus, .rv-compose textarea:focus, .rv-finish textarea:focus { outline: 2px solid var(--ph-link); outline-offset: 0; }
.rside .hint { font-size: 11px; color: var(--ph-fg2); }
.rside .diff { font: 12px/1.45 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; background: var(--ph-bar); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 6px 8px; margin: 6px 0 0; max-height: 260px; overflow: auto; cursor: text; }
.rside .diff .ctx { color: var(--ph-fg2); }
.rside .diff del { background: rgb(255 80 70 / 18%); color: var(--ph-danger); text-decoration: line-through; text-decoration-thickness: 1px; }
.rside .diff ins { background: rgb(40 170 80 / 20%); color: #1a7f37; text-decoration: none; }
.win.watch.dark .rside .diff ins, .win.vscode .rside .diff ins { color: #57c97a; }
@media (prefers-color-scheme: dark) { .win.watch:not(.light) .rside .diff ins { color: #57c97a; } .win.watch:not(.light) .rside .btn.accept { color: #57c97a; } }
.rside .hunk .file { font-weight: 600; color: var(--ph-fg); }
.rside .comments { border-top: 1px dashed var(--ph-border); margin-top: 8px; padding-top: 4px; }
.rside .rbar { position: sticky; top: -8px; z-index: 1; margin: -8px -8px 8px; padding: 8px 10px; background: color-mix(in srgb, var(--ph-accent) 12%, var(--ph-bar)); border-bottom: 1px solid var(--ph-border); display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.rside .reviewrec { font-size: 12px; color: var(--ph-fg2); margin: 2px 2px 8px; }
.rv-pop { position: absolute; z-index: 30; display: flex; gap: 4px; padding: 4px; background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) + 2px); box-shadow: 0 6px 20px rgb(0 0 0 / 22%); }
.rv-pop .btn { display: inline-flex; align-items: center; gap: 4px; border: 0; background: none; }
.rv-pop .btn:hover, .rv-pop .btn:focus-visible { background: var(--ph-hover); }
.rv-compose, .rv-finish { position: absolute; z-index: 31; width: min(340px, calc(100% - 16px)); padding: 10px; background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) + 2px); box-shadow: 0 8px 28px rgb(0 0 0 / 28%); }
.rv-compose .q, .rv-finish .q { color: var(--ph-fg2); font-size: 12px; margin-bottom: 6px; overflow-wrap: anywhere; }
.rv-compose .act, .rv-finish .act { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; align-items: center; }
.rv-compose .act .hint, .rv-finish .act .hint { margin-right: auto; font-size: 11px; color: var(--ph-fg2); }
.rv-finish label { display: flex; gap: 8px; align-items: flex-start; padding: 4px 2px; cursor: pointer; }
.rv-finish label small { display: block; color: var(--ph-fg2); }
.slot .rv { position: absolute; pointer-events: none; border-radius: 2px; }
.slot .rv.n-highlight { background: rgb(255 214 0 / 40%); mix-blend-mode: multiply; }
.slot .rv.n-note { background: rgb(75 140 240 / 22%); mix-blend-mode: multiply; box-shadow: inset 0 -2px 0 rgb(75 140 240 / 70%); }
.slot .rv.n-comment { background: rgb(240 150 40 / 26%); mix-blend-mode: multiply; box-shadow: inset 0 -2px 0 rgb(224 138 30 / 85%); }
.slot .rv.n-resolved { background: none; box-shadow: inset 0 -1px 0 rgb(120 120 120 / 45%); mix-blend-mode: normal; }
.slot .rv.h-add { background: rgb(40 170 80 / 18%); box-shadow: inset 0 -2px 0 rgb(30 150 70 / 75%); mix-blend-mode: multiply; }
.slot .rv.h-del { background: rgb(220 50 40 / 85%); width: 2px !important; margin-left: -1px; border-radius: 1px; }
.slot .rv.on { outline: 2px solid var(--ph-accent, #1d3e72); outline-offset: 1px; }
.slot .rv-pin { pointer-events: auto; position: absolute; right: -2px; transform: translate(100%, -10%); min-width: 18px; height: 18px; padding: 0 4px; border-radius: 9px 9px 9px 2px; border: 0; font: 600 10px/18px var(--ph-font, system-ui); color: #fff; cursor: pointer; z-index: 3; box-shadow: 0 1px 3px rgb(0 0 0 / 30%); }
.slot .rv-pin.n-highlight { background: #b39500; } .slot .rv-pin.n-note { background: #2f6fd1; } .slot .rv-pin.n-comment { background: #d27510; }
.slot .rv-pin.n-resolved { background: #8a919c; } .slot .rv-pin.h-add, .slot .rv-pin.h-del { background: #1a7f37; }
.slot .rv-pin.on { outline: 2px solid var(--ph-accent, #1d3e72); outline-offset: 1px; }
.slot .rv-pin:focus-visible { outline: 2px solid var(--ph-link, #1d3e72); outline-offset: 2px; }
.stage.dark-pdf .slot .rv { mix-blend-mode: normal; }
@media (prefers-reduced-motion: no-preference) { .rside .card.flash { animation: rvflash 1.2s ease-out; } @keyframes rvflash { from { box-shadow: 0 0 0 3px var(--ph-progress); } } }
`;

/** The review beside the pages. */
export class Review {
  private p: ReviewPanel;
  private host: ReviewHost;
  private s: ReviewState = { notes: [], hunks: [], reviews: [], git: false };
  private tab: "notes" | "changes" = "notes";
  private filter: "open" | "resolved" | "all" = "open";
  /** The note or hunk selected (its card open, its marks outlined). */
  private sel: string | null = null;
  private side: HTMLElement;
  private body: HTMLElement;
  /** The last text selected on the pages, its source looked up when asked. */
  private picked: { k: number; rects: Box[] }[] | null = null;
  private pointerDown = false;
  private pop: HTMLElement | null = null;
  private compose: HTMLElement | null = null;
  /** Drafts typed in a card's reply box, kept across redraws. */
  private drafts = new Map<string, string>();
  private showing = false;
  private onCount: ((open: number, pending: number) => void) | undefined;
  /** The marks drawn last (a newer drawing stops an older one). */
  private drawn = 0;
  /** Where each note is on the pages, as last drawn: its page and height (the list's order). */
  private where = new Map<string, [number, number]>();

  constructor(p: ReviewPanel, host: ReviewHost) {
    this.p = p;
    this.host = host;
    this.side = document.createElement("aside");
    this.side.className = "rside";
    this.side.setAttribute("aria-label", "Review");
    this.side.append(html`<div class="rgrip" title="Drag to resize"></div><header role="tablist"><button type="button" class="rtab" role="tab" data-tab="notes">Notes<span class="n" id="rv-nn"></span></button><button type="button" class="rtab" role="tab" data-tab="changes">Changes<span class="n" id="rv-nc"></span></button><span class="grow"></span><button type="button" class="ib" id="rv-x" title="Close the review (Esc)" aria-label="Close the review">${icon("close")}</button></header><div class="rbody" id="rv-body" role="tabpanel"></div>`);
    this.body = this.side.querySelector<HTMLElement>("#rv-body")!;
    p.win.querySelector(".body")!.append(this.side);
    this.side.querySelector("header")!.addEventListener("click", (e) => {
      const t = (e.target as HTMLElement).closest<HTMLElement>("[data-tab]");
      if (t) (this.tab = t.dataset.tab as "notes" | "changes"), this.render();
    });
    this.side.querySelector<HTMLElement>("#rv-x")!.onclick = () => this.open(false);
    this.grip();
    this.body.addEventListener("click", (e) => void this.clicked(e));
    this.body.addEventListener("keydown", (e) => this.keyed(e));
    host.onState((s) => {
      this.s = s;
      void this.marks();
      this.render();
    });
    // (the pages: a selection's end offers the note kinds; a click on a mark selects it; Alt+click a spot)
    p.stage.addEventListener("pointerdown", (e) => {
      this.pointerDown = true;
      if (!(e.target as HTMLElement).closest(".rv-pop, .rv-compose, .rv-finish")) this.closePop();
    });
    addEventListener("pointerup", () => {
      if (!this.pointerDown) return;
      this.pointerDown = false;
      setTimeout(() => this.offer(), 0);
    });
    p.stage.addEventListener("keyup", (e) => e.shiftKey && setTimeout(() => this.offer(), 0));
    p.stage.addEventListener("click", (e) => void this.pageClick(e));
    p.root.addEventListener("keydown", (e) => {
      const k = e as KeyboardEvent;
      if (k.key !== "Escape") return;
      if (this.compose || this.pop) (this.closePop(), e.stopPropagation());
    }, { capture: true });
    void host.request("review_state").then((r) => {
      if (r.ok && r.json) (this.s = r.json as ReviewState), this.render(), void this.marks();
    });
  }

  /** The pages' origins changed (a build): the marks found again. */
  refresh(): void {
    void this.marks();
  }

  /** The counts for a button outside (the bar's): open notes, pending changes. */
  counts(cb: (open: number, pending: number) => void): void {
    this.onCount = cb;
    this.count();
  }

  private count(): void {
    const open = this.s.notes.filter((n) => n.status === "open" && !n.draft && !n.hunk).length;
    this.onCount?.(open, this.s.hunks.length);
  }

  get isOpen(): boolean {
    return this.p.win.classList.contains("review-on");
  }

  /** The side shown (or hidden), on `tab` if given. */
  open(on = !this.isOpen, tab?: "notes" | "changes"): void {
    if (tab) this.tab = tab;
    this.p.win.classList.toggle("review-on", on);
    if (on) this.render();
    this.p.laidOut();
    if (on) this.side.querySelector<HTMLElement>(`[data-tab="${this.tab}"]`)?.focus();
  }

  private grip(): void {
    const g = this.side.querySelector<HTMLElement>(".rgrip")!;
    g.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      const x0 = e.clientX, w0 = this.side.getBoundingClientRect().width;
      g.setPointerCapture(e.pointerId);
      const move = (m: PointerEvent) => this.side.style.setProperty("--rv-w", `${Math.round(Math.max(260, w0 + x0 - m.clientX))}px`);
      const up = () => (g.removeEventListener("pointermove", move), g.removeEventListener("pointerup", up), this.p.laidOut());
      g.addEventListener("pointermove", move);
      g.addEventListener("pointerup", up);
    });
  }

  // ---- the side ----

  private render(): void {
    this.count();
    const nn = this.s.notes.filter((n) => n.status === "open" && !n.hunk && !n.draft).length;
    this.side.querySelector("#rv-nn")!.textContent = nn ? String(nn) : "";
    this.side.querySelector("#rv-nc")!.textContent = this.s.hunks.length ? String(this.s.hunks.length) : "";
    for (const t of this.side.querySelectorAll<HTMLElement>("[data-tab]")) t.setAttribute("aria-selected", String(t.dataset.tab === this.tab));
    if (!this.isOpen) return;
    // (the focus and the reply typed kept across a redraw)
    const focused = this.p.root.activeElement as HTMLElement | null;
    const focusKey = focused && this.body.contains(focused) ? (focused.dataset.focus ?? null) : null;
    const top = this.body.scrollTop;
    this.body.replaceChildren(this.tab === "notes" ? this.notesView() : this.changesView());
    // (what was typed in a reply or a comment, put back)
    for (const ta of this.body.querySelectorAll<HTMLTextAreaElement>("textarea[data-reply], textarea[data-comment]"))
      ta.value = this.drafts.get(ta.dataset.reply ?? `h:${ta.dataset.comment}`) ?? "";
    this.body.scrollTop = top;
    if (focusKey) {
      const f = this.body.querySelector<HTMLElement>(`[data-focus="${CSS.escape(focusKey)}"]`);
      if (f) {
        f.focus();
        if (f instanceof HTMLTextAreaElement) f.selectionStart = f.selectionEnd = f.value.length;
      }
    }
  }

  private notesView(): DocumentFragment {
    const all = this.s.notes.filter((n) => !n.hunk);
    const resolved = all.filter((n) => n.status === "resolved").length;
    const shown = all.filter((n) => this.filter === "all" || n.status === this.filter);
    const seg = (v: typeof this.filter, label: string, n: number) => html`<button type="button" data-filter="${v}" aria-pressed="${this.filter === v}">${label} ${n ? String(n) : ""}</button>`;
    const tools = html`<div class="rtools"><span class="seg" role="group" aria-label="Show">${seg("open", "Open", all.length - resolved)}${seg("resolved", "Resolved", resolved)}${seg("all", "All", 0)}</span><span class="grow"></span>${resolved ? html`<button type="button" class="btn" data-act="clear" title="Delete every resolved note">Clear resolved</button>` : ""}</div>`;
    if (!all.length)
      return html`${tools}<div class="empty"><b>No notes yet.</b><br>Select text on a page to highlight it, note it, or comment for your agent. <b>Alt+click</b> notes a spot with no text, such as a figure.<br><br>Your agent reads them with <code>phitex session notes</code>.</div>`;
    if (!shown.length)
      return html`${tools}<div class="empty">${this.filter === "open" ? "Every note is resolved." : "No resolved notes."}</div>`;
    const placed = shown.filter((n) => !n.detached);
    const detached = shown.filter((n) => n.detached);
    // (in the order they are in the files)
    // (as they come in the pages: by page where known, then file and place)
    const at = (n: ReviewNote) => this.where.get(n.id) ?? [n.anchor.page ?? 1e9, 0];
    placed.sort((a, b) => at(a)[0] - at(b)[0] || at(a)[1] - at(b)[1] || a.anchor.file.localeCompare(b.anchor.file) || a.anchor.start - b.anchor.start);
    return html`${tools}${placed.map((n) => this.noteCard(n))}${detached.length ? html`<h3>Detached: their text is gone from the source</h3>${detached.map((n) => this.noteCard(n))}` : ""}`;
  }

  private noteCard(n: ReviewNote, inHunk = false): DocumentFragment {
    const on = this.sel === n.id;
    const a = n.anchor;
    const quote = a.quote
      ? html`<div class="quote ${n.kind}">${oneLine(a.quote, on ? 2000 : 240)}</div>`
      : html`<div class="quote spot ${n.kind}">A spot on page ${String((a.page ?? 0) + 1)}</div>`;
    const thread = n.thread.map((p) => this.post(p));
    const lastBy = n.thread.at(-1)?.by;
    // (a comment asks the agent; a note and a highlight are kept, asking nothing)
    const waiting = n.status === "open" && n.kind === "comment" && lastBy !== "agent" && !n.draft && !n.verdict;
    const reply = on && !n.draft
      ? html`<textarea data-focus="reply:${n.id}" data-reply="${n.id}" placeholder="${n.kind === "highlight" ? "Add a note…" : "Reply…"}" aria-label="Reply to this note" rows="2"></textarea><div class="act"><button type="button" class="btn primary" data-act="reply" data-id="${n.id}" data-focus="send:${n.id}">Reply</button>${n.status === "open" ? html`<button type="button" class="btn" data-act="resolve" data-id="${n.id}" data-focus="res:${n.id}">${icon("check", 14)} Resolve</button>` : html`<button type="button" class="btn" data-act="reopen" data-id="${n.id}" data-focus="reo:${n.id}">${icon("undo", 14)} Reopen</button>`}<span class="grow"></span><button type="button" class="ib" data-act="delete" data-id="${n.id}" title="Delete this note" aria-label="Delete this note">${icon("delete", 16)}</button></div><div class="hint">Ctrl+Enter replies · Esc closes</div>`
      : "";
    const answered = (n.hunks ?? []).length;
    return html`<div class="card note${on ? " on" : ""}${n.status === "resolved" ? " resolved" : ""}" data-id="${n.id}" data-focus="card:${n.id}" tabindex="0" role="button" aria-expanded="${on}" aria-label="${`${KIND_WORD[n.kind]} at ${a.file} line ${a.line}${n.status === "resolved" ? ", resolved" : ""}`}"><div class="top"><span class="chip ${n.kind}">${KIND_WORD[n.kind]}</span>${n.draft ? html`<span class="chip draft">Pending</span>` : ""}${n.detached ? html`<span class="chip det" title="Its text was edited away: shown where it was">Detached</span>` : ""}${n.status === "resolved" ? html`<span class="chip done">Resolved</span>` : ""}${inHunk ? "" : html`<button type="button" class="place" data-goto="${a.file}" data-line="${String(a.line)}" title="Open in the editor">${placeOf(a.file, a.line)}</button>`}<span class="grow"></span><span title="${n.updated}">${ago(n.updated)}</span></div>${inHunk ? "" : quote}${thread}${!on && waiting ? html`<div class="more">Waiting for your agent</div>` : ""}${!on && answered ? html`<div class="more">${String(answered)} change${answered === 1 ? "" : "s"} answer this note: see Changes</div>` : ""}${reply}</div>`;
  }

  private post(p: ReviewPost): DocumentFragment {
    if (p.diff)
      return html`<div class="post update"><span class="by agent">Agent</span>changed it<span class="when">${ago(p.at)}</span><div class="diff">${p.diff.old ? html`<del>${oneLine(p.diff.old, 160)}</del>` : ""}${p.diff.new ? html`<ins>${oneLine(p.diff.new, 160)}</ins>` : ""}</div></div>`;
    const edited = (p.edited ?? []).map((e) => html`<button type="button" class="place" data-goto="${e.file}" data-line="${String(e.from_line)}" data-from="${String(e.start)}" data-to="${String(e.end)}">${placeOf(e.file, e.from_line, e.to_line)}</button> `);
    return html`<div class="post"><span class="by ${p.by === "agent" ? "agent" : ""}">${p.by === "agent" ? "Agent" : "You"}</span><span class="txt">${p.text}</span><span class="when" title="${p.at}">${ago(p.at)}</span>${edited.length ? html`<span class="edited">Edited ${edited}</span>` : ""}</div>`;
  }

  private changesView(): DocumentFragment {
    const hs = this.s.hunks;
    const drafts = this.s.notes.filter((n) => n.draft);
    const bar = drafts.length
      ? html`<div class="rbar"><span><b>${String(drafts.length)}</b> comment${drafts.length === 1 ? "" : "s"} in your review</span><span class="grow"></span><button type="button" class="btn primary" data-act="finish">Finish review</button></div>`
      : "";
    const last = this.s.reviews.at(-1);
    const rec = last ? html`<div class="reviewrec">Last review: ${VERDICT_WORD[last.verdict]}${last.summary ? `: ${oneLine(last.summary, 80)}` : ""} · ${ago(last.at)}</div>` : "";
    if (!hs.length)
      return html`${bar}${rec}${this.earlier(true)}<div class="empty"><b>No changes waiting.</b><br>Edits your agent makes to the files show here, word by word, to accept or reject. ${this.s.git ? "Accepting one stages it in git; rejecting one puts the staged text back." : "Accepted text is kept in .phitex/baseline."}<br><br>Edits you type in an editor attached to this watch are yours and are accepted as you type.</div>`;
    const tools = html`<div class="rtools"><button type="button" class="btn accept" data-act="accept-all" title="Accept every change">${icon("check", 14)} Accept all</button><button type="button" class="btn reject" data-act="reject-all" title="Reject every change: the files get the accepted text back">Reject all</button><span class="grow"></span><label class="hint" title="Compare against the accepted text, typeset (latexdiff)"><input type="checkbox" data-act="show" checked="${this.showing}"> Show typeset</label></div>`;
    const groups = groupHunks(this.s);
    return html`${bar}${rec}${tools}${groups.map((g) => html`<h3>${g.title}${g.note ? html` <button type="button" class="place" data-note="${g.note.id}">${g.note.id}</button>` : ""}</h3>${g.hunks.map((h) => this.hunkCard(h))}`)}${this.earlier()}`;
  }

  /** The comments on changes accepted or rejected since, with their verdicts: a review's history. */
  private earlier(top = false): DocumentFragment | "" {
    const pending = new Set(this.s.hunks.map((h) => h.id));
    const done = this.s.notes.filter((n) => n.hunk && !n.draft && !pending.has(n.hunk.id));
    if (!done.length) return "";
    // (by name, else by what it changed where: alike changes renamed as others were decided)
    const verdict = (n: ReviewNote) => n.verdict ?? (this.s.verdicts ?? []).find((v) => v.hunk.id === n.hunk!.id);
    return html`<h3>${top ? "Reviewed" : "Reviewed earlier"}</h3>${done.map((n) => {
      const v = verdict(n);
      return html`<div class="card hunk past${this.sel === n.id ? " on" : ""}" data-id="${n.id}" tabindex="0" role="button"><div class="top"><span class="file">${placeOf(n.hunk!.file, n.hunk!.line)}</span>${v ? html`<span class="chip ${v.verdict === "accepted" ? "done" : "det"}">${v.verdict === "accepted" ? "Accepted" : "Rejected"}</span>` : ""}<span class="grow"></span><span>${v ? ago(v.at) : ""}</span></div><div class="diff">${n.hunk!.old ? html`<del>${oneLine(n.hunk!.old, 200)}</del>` : ""}${n.hunk!.new ? html`<ins>${oneLine(n.hunk!.new, 200)}</ins>` : ""}</div><div class="comments">${this.noteCard(n, true)}</div></div>`;
    })}`;
  }

  private hunkCard(h: ReviewHunk): DocumentFragment {
    const on = this.sel === h.id;
    const comments = this.s.notes.filter((n) => h.comments.includes(n.id));
    // (each part's place in the current text, bytes: a selection in the diff is a range to comment on)
    let at = h.now[0];
    const parts = h.parts.map(([k, t]) => {
      if (k === "del") return html`<del data-at="${String(at)}">${t}</del>`;
      const el = k === "add" ? html`<ins data-at="${String(at)}">${t}</ins>` : html`<span data-at="${String(at)}">${t}</span>`;
      at += utf8(t);
      return el;
    });
    const what = h.whole === "added" ? " (new file)" : h.whole === "deleted" ? " (file deleted)" : "";
    const diff = h.whole ? html`<div class="diff">${h.whole === "added" ? html`<ins data-at="${String(h.now[0])}">${oneLine(h.new, 600)}</ins>` : html`<del>${oneLine(h.old, 600)}</del>`}</div>` : html`<div class="diff" data-hunk="${h.id}"><span class="ctx">${h.before}</span>${parts}<span class="ctx">${h.after}</span></div>`;
    const box = on
      ? html`<textarea data-focus="hc:${h.id}" data-comment="${h.id}" placeholder="Comment on this change (select words in it to comment on them)…" aria-label="Comment on this change" rows="2"></textarea><div class="act"><button type="button" class="btn primary" data-act="comment" data-id="${h.id}">Comment</button><button type="button" class="btn" data-act="draft" data-id="${h.id}" title="Gather it with others and submit them as one review">Add to review</button></div>`
      : "";
    return html`<div class="card hunk${on ? " on" : ""}" data-id="${h.id}" data-focus="card:${h.id}" tabindex="0" role="button" aria-expanded="${on}" aria-label="${`Change at ${h.file} line ${h.line}${what}`}"><div class="top"><button type="button" class="place" data-goto="${h.file}" data-line="${String(h.line)}" title="Open in the editor"><span class="file">${placeOf(h.file, h.line, h.end_line)}</span></button>${what}<span class="grow"></span><button type="button" class="btn accept" data-act="accept" data-id="${h.id}" title="Accept: ${this.s.git ? "staged in git" : "kept as the accepted text"}" aria-label="Accept this change">${icon("check", 14)} Accept</button><button type="button" class="btn reject" data-act="reject" data-id="${h.id}" title="Reject: the file gets the accepted text back" aria-label="Reject this change">Reject</button></div>${diff}${comments.length ? html`<div class="comments">${comments.map((c) => this.noteCard(c, true))}</div>` : ""}${box}</div>`;
  }

  // ---- what the user does in the side ----

  private keyed(e: KeyboardEvent): void {
    const t = e.target as HTMLElement;
    if (t instanceof HTMLTextAreaElement) {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (t.dataset.reply) void this.act("reply", t.dataset.reply);
        else if (t.dataset.comment) void this.act("comment", t.dataset.comment);
      } else if (e.key === "Escape") {
        e.stopPropagation();
        this.select(null);
      }
      return;
    }
    if ((e.key === "Enter" || e.key === " ") && t.classList.contains("card")) {
      e.preventDefault();
      this.select(t.dataset.id!, true);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const cards = [...this.body.querySelectorAll<HTMLElement>(".rbody > .card, .rbody > div > .card, .card")].filter((c) => !c.closest(".comments"));
      const i = cards.indexOf(t.closest(".card")!);
      const next = cards[i + (e.key === "ArrowDown" ? 1 : -1)];
      if (next) (e.preventDefault(), next.focus());
    }
  }

  private async clicked(e: MouseEvent): Promise<void> {
    const t = e.target as HTMLElement;
    const f = t.closest<HTMLElement>("[data-filter]");
    if (f) return void ((this.filter = f.dataset.filter as typeof this.filter), this.render());
    const go = t.closest<HTMLElement>("[data-goto]");
    if (go) {
      e.stopPropagation();
      if (go.dataset.from) void this.reveal(go.dataset.goto!, Number(go.dataset.from), Number(go.dataset.to));
      return this.host.goto(go.dataset.goto!, Number(go.dataset.line) || 1);
    }
    const nt = t.closest<HTMLElement>("[data-note]");
    if (nt) return void ((this.tab = "notes"), (this.filter = "all"), this.select(nt.dataset.note!, true));
    const a = t.closest<HTMLElement>("[data-act]");
    if (a) {
      e.stopPropagation();
      if (a.dataset.act === "show") return this.show((a as HTMLInputElement).checked);
      return void this.act(a.dataset.act!, a.dataset.id);
    }
    if (t.closest("textarea, .diff")) return;
    const card = t.closest<HTMLElement>(".card");
    if (card && this.sel !== card.dataset.id) this.select(card.dataset.id!, true);
  }

  private show(on: boolean): void {
    this.showing = on;
    this.host.showChanges?.(on, this.s.git);
    void this.marks();
  }

  /** The op `act` on note or hunk `id`. */
  private async act(act: string, id?: string): Promise<void> {
    const r = (op: string, params?: object) => this.req(op, params);
    switch (act) {
      case "clear": {
        const n = this.s.notes.filter((x) => x.status === "resolved" && !x.hunk).length;
        if (!confirm(`Delete ${n} resolved note${n === 1 ? "" : "s"}?`)) return;
        await r("review_clear");
        return;
      }
      case "reply": {
        const ta = this.body.querySelector<HTMLTextAreaElement>(`[data-reply="${id}"]`);
        const text = ta?.value.trim() ?? "";
        if (!text) return void ta?.focus();
        if (await r("review_reply", { id, text })) this.drafts.delete(id!);
        return;
      }
      case "resolve":
      case "reopen": {
        const ta = this.body.querySelector<HTMLTextAreaElement>(`[data-reply="${id}"]`);
        const text = ta?.value.trim() ?? "";
        if (await r(`review_${act}`, { id, text })) this.drafts.delete(id!);
        if (act === "resolve" && this.filter === "open") this.select(null);
        return;
      }
      case "delete":
        if (await r("review_delete", { id })) this.select(null);
        return;
      case "accept":
      case "reject":
        await this.verdict(act, [id!]);
        return;
      case "accept-all":
      case "reject-all": {
        const n = this.s.hunks.length;
        if (act === "reject-all" && !confirm(`Reject all ${n} change${n === 1 ? "" : "s"}? The files get the accepted text back.`)) return;
        await this.verdict(act === "accept-all" ? "accept" : "reject", "all");
        return;
      }
      case "comment":
      case "draft": {
        const ta = this.body.querySelector<HTMLTextAreaElement>(`[data-comment="${id}"]`);
        const text = ta?.value.trim() ?? "";
        if (!text) return void ta?.focus();
        const range = this.diffRange(id!);
        if (await r("review_comment", { hunk: id, text, draft: act === "draft", ...(range ? { range } : {}) })) this.drafts.delete(`h:${id}`);
        return;
      }
      case "finish":
        return this.finish();
    }
  }

  private async verdict(how: "accept" | "reject", ids: string[] | "all"): Promise<void> {
    // (the next change selected, where one was: accepting goes down the list)
    const list = this.s.hunks.map((h) => h.id);
    const at = ids === "all" ? -1 : list.indexOf(ids[0]);
    const r = await this.req(`review_${how}`, { ids });
    if (!r) return;
    const n = Number(r.done ?? 0);
    this.p.msg(`${how === "accept" ? "Accepted" : "Rejected"} ${n} change${n === 1 ? "" : "s"}${how === "accept" && this.s.git ? " (staged)" : ""}`);
    if (at >= 0) {
      // (the state comes as the op's event: after it, the one now there)
      const next = this.s.hunks.find((h) => !list.slice(0, at + 1).includes(h.id)) ?? this.s.hunks.at(-1);
      this.select(next?.id ?? null, false, true);
    }
    if (this.showing) this.host.showChanges?.(true, this.s.git);
  }

  /** A range selected inside hunk `id`'s diff: bytes of the current text. */
  private diffRange(id: string): [number, number] | null {
    const rn = this.p.root as ShadowRoot & { getSelection?: () => Selection | null };
    const sel = rn.getSelection?.() ?? document.getSelection();
    const d = this.body.querySelector<HTMLElement>(`.diff[data-hunk="${id}"]`);
    if (!sel || sel.isCollapsed || !d || !d.contains(sel.anchorNode)) return null;
    const pos = (node: Node | null, off: number): number | null => {
      const el = (node instanceof HTMLElement ? node : node?.parentElement)?.closest<HTMLElement>("[data-at]");
      if (!el || !d.contains(el)) return null;
      const base = Number(el.dataset.at);
      return el.localName === "del" ? base : base + utf8((el.textContent ?? "").slice(0, off));
    };
    const r = sel.getRangeAt(0);
    const a = pos(r.startContainer, r.startOffset), b = pos(r.endContainer, r.endOffset);
    if (a === null || b === null) return null;
    return [Math.min(a, b), Math.max(a, b)];
  }

  private async req(op: string, params?: object): Promise<any | null> {
    const r = await this.host.request(op, params);
    if (!r.ok) {
      this.p.msg(r.error ?? "The watch did not answer", true);
      return null;
    }
    return r.json ?? {};
  }

  /** Note or hunk `id` selected (its card open, its marks outlined); `scroll`: the page to it; `focus`: its card focused. */
  select(id: string | null, scroll = false, focus = false): void {
    // (the reply typed so far kept)
    for (const ta of this.body.querySelectorAll<HTMLTextAreaElement>("textarea[data-reply], textarea[data-comment]")) {
      const key = ta.dataset.reply ?? `h:${ta.dataset.comment}`;
      if (ta.value) this.drafts.set(key, ta.value);
      else this.drafts.delete(key);
    }
    this.sel = id;
    const note = id ? this.s.notes.find((n) => n.id === id) : undefined;
    if (note && !note.hunk) {
      this.tab = "notes";
      if (this.filter !== "all" && note.status !== this.filter) this.filter = "all";
    } else if (id && (this.s.hunks.some((h) => h.id === id) || note?.hunk)) this.tab = "changes";
    if (id && !this.isOpen) this.open(true);
    this.render();
    this.outline();
    const card = id ? this.body.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(id)}"]`) : null;
    if (card) {
      card.scrollIntoView({ block: "nearest" });
      if (focus) {
        const box = card.querySelector<HTMLTextAreaElement>("textarea");
        (box && !focus ? box : card).focus();
      }
    }
    if (scroll && id) void this.toPage(id);
  }

  /** The selected one's marks outlined on the pages. */
  private outline(): void {
    for (const m of this.p.stage.querySelectorAll<HTMLElement>(".rv")) m.classList.toggle("on", !!this.sel && m.dataset.id === this.sel);
  }

  /** The pages scrolled to note or hunk `id`'s place. */
  private async toPage(id: string): Promise<void> {
    const n = this.s.notes.find((x) => x.id === id);
    const h = this.s.hunks.find((x) => x.id === id);
    const [file, a, b] = n ? [n.anchor.file, n.anchor.start, n.anchor.end] : h ? [h.file, h.now[0], h.now[1]] : ["", 0, 0];
    if (!file) return;
    if (n && !n.anchor.quote && n.anchor.point && n.anchor.page !== undefined) return void this.p.viewer.scrollToPlace(n.anchor.page, n.anchor.point[1]);
    const at = await this.host.boxes(file, a, Math.max(b, a + 1), n?.anchor.quote ?? h?.new);
    if (at.length) this.p.viewer.scrollToPlace(at[0].k, at[0].boxes[0][1]);
    else this.p.msg("Not on the pages (none of its text is typeset)");
  }

  /** Bytes [a, b) of `file` shown on the pages a moment (an edit a thread recorded). */
  private async reveal(file: string, a: number, b: number): Promise<void> {
    const at = await this.host.boxes(file, a, Math.max(b, a + 1));
    if (at.length) this.p.viewer.mark(at[0].k, at[0].boxes);
  }

  // ---- the marks on the pages ----

  private async marks(): Promise<void> {
    const turn = ++this.drawn;
    // (the typeset comparison shown: its own marks, latexdiff's; the pages' origins are the diff's)
    if (this.showing) return this.p.viewer.reviewMarks([]);
    const out: Parameters<Viewer["reviewMarks"]>[0] = [];
    const cls = (n: ReviewNote) => (n.status === "resolved" ? "n-resolved" : `n-${n.kind}`);
    let i = 0;
    for (const n of this.s.notes) {
      if (n.hunk || n.draft) continue;
      i++;
      if (n.detached) continue;
      const said = n.thread.filter((p) => p.text).length;
      const pin = n.status === "resolved" ? "✓" : n.kind === "highlight" && !said ? "" : String(said || "•");
      const title = `${KIND_WORD[n.kind]}: ${oneLine(n.thread[0]?.text || n.anchor.quote, 80)}`;
      if (!n.anchor.quote && n.anchor.point && n.anchor.page !== undefined) {
        const [x, y] = n.anchor.point;
        out.push({ k: n.anchor.page, boxes: [[x - 4, y - 4, 8, 8]], cls: cls(n), id: n.id, pin, title });
        continue;
      }
      const at = await this.host.boxes(n.anchor.file, n.anchor.start, Math.max(n.anchor.end, n.anchor.start + 1), n.anchor.quote);
      if (turn !== this.drawn) return;
      at.forEach((p, j) => out.push({ ...p, cls: cls(n), id: n.id, ...(j === 0 && pin !== "" ? { pin, title } : {}) }));
    }
    for (const h of this.s.hunks) {
      if (h.whole === "deleted") continue;
      const empty = h.now[1] <= h.now[0];
      const at = await this.host.boxes(h.file, h.now[0], empty ? h.now[0] + 1 : h.now[1], empty ? undefined : h.new);
      if (turn !== this.drawn) return;
      at.forEach((p) => out.push({ k: p.k, boxes: empty ? p.boxes.slice(0, 1).map(([x, y, , bh]) => [x, y, 2, bh] as Box) : p.boxes, cls: empty ? "h-del" : "h-add", id: h.id }));
    }
    void i;
    this.p.viewer.reviewMarks(out);
    this.outline();
    // (the list in the pages' order, where it changed)
    const where = new Map<string, [number, number]>();
    for (const m of out) if (!where.has(m.id)) where.set(m.id, [m.k, m.boxes[0]?.[1] ?? 0]);
    const moved = [...where].some(([id, w]) => this.where.get(id)?.join() !== w.join());
    this.where = where;
    if (moved && this.isOpen && this.tab === "notes") this.render();
  }

  /** A click on the pages: a pin or a mark selects its note or hunk; Alt+click notes a spot. */
  private async pageClick(e: MouseEvent): Promise<void> {
    const t = e.target as HTMLElement;
    const pin = t.closest<HTMLElement>(".rv-pin");
    if (pin) {
      e.stopPropagation();
      return this.select(pin.dataset.id!, false, true);
    }
    const slot = t.closest<HTMLElement>(".slot");
    if (!slot) return;
    const rn = this.p.root as ShadowRoot & { getSelection?: () => Selection | null };
    const sel = rn.getSelection?.() ?? document.getSelection();
    if (sel && !sel.isCollapsed) return;
    const r = slot.getBoundingClientRect();
    const k = Number(slot.dataset.k);
    const size = this.p.viewer.pageSize();
    const x = ((e.clientX - r.left) / r.width) * size.w, y = ((e.clientY - r.top) / r.height) * size.h;
    if (e.altKey) {
      e.preventDefault();
      const at = await this.host.spot(k, x, y);
      if (!at) return this.p.msg("No source near that spot (nothing typeset from a file there)", true);
      return this.openCompose(e.clientX, e.clientY, { kind: "note", anchor: { file: at.file, start: at.start, end: at.start, page: k, point: [x, y] } }, "A spot on this page");
    }
    // (a mark under the click: the last drawn on top)
    const hit = [...slot.querySelectorAll<HTMLElement>(".rv:not(.rv-pin)")].reverse().find((m) => {
      const b = m.getBoundingClientRect();
      return e.clientX >= b.left - 1 && e.clientX <= b.right + 1 && e.clientY >= b.top - 1 && e.clientY <= b.bottom + 1;
    });
    if (hit) this.select(hit.dataset.id!, false, false);
  }

  // ---- making a note ----

  /** Text selected on the pages, as the panel tells it (each drag's frame). */
  selected(sel: { k: number; rects: Box[] }[]): void {
    this.picked = sel;
    if (!this.pointerDown) setTimeout(() => this.offer(), 0);
  }

  /** The note kinds offered at the selection's end. */
  private offer(): void {
    const rn = this.p.root as ShadowRoot & { getSelection?: () => Selection | null };
    const sel = rn.getSelection?.() ?? document.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount || !this.picked) return this.closePopOnly();
    const range = sel.getRangeAt(0);
    if (!this.p.stage.contains(range.commonAncestorContainer)) return;
    const rects = [...range.getClientRects()].filter((r) => r.width > 0);
    const end = rects.at(-1) ?? range.getBoundingClientRect();
    const sig = JSON.stringify(this.picked);
    if (this.pop?.dataset.sig === sig) return;
    this.closePop();
    const picked = this.picked;
    // (a selection inside one of the agent's changes: comment on it there)
    const pop = html`<div class="rv-pop" role="toolbar" aria-label="Note the selection"><button type="button" class="btn" data-k="highlight" title="Highlight (h)">${icon("highlight", 15)} Highlight</button><button type="button" class="btn" data-k="note" title="A note for you and your agent (n)">${icon("note", 15)} Note</button><button type="button" class="btn" data-k="comment" title="A request for your agent (c)">${icon("comment", 15)} Comment</button></div>`.firstElementChild as HTMLElement;
    pop.dataset.sig = sig;
    // (above the selection's first line, as a reading app's: the line under it stays readable)
    const first = rects[0] ?? end;
    this.place(pop, first.left + first.width / 2, first.top - 6, true, true);
    pop.addEventListener("pointerdown", (e) => e.preventDefault());
    pop.addEventListener("click", (e) => {
      const k = (e.target as HTMLElement).closest<HTMLElement>("[data-k]")?.dataset.k as ReviewNote["kind"] | undefined;
      if (k) void this.make(k, picked, end);
    });
    pop.addEventListener("keydown", (e) => {
      const k = ({ h: "highlight", n: "note", c: "comment" } as Record<string, ReviewNote["kind"]>)[e.key];
      if (k) (e.preventDefault(), void this.make(k, picked, end));
      if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        const bs = [...pop.querySelectorAll<HTMLElement>("button")];
        const i = bs.indexOf(e.target as HTMLElement);
        bs[(i + (e.key === "ArrowRight" ? 1 : bs.length - 1)) % bs.length]?.focus();
      }
    });
    this.pop = pop;
  }

  /** A popover put at (x, y) in the window's coordinates, kept inside the panel; `center`: x is its middle. */
  private place(el: HTMLElement, x: number, y: number, center = false, above = false): void {
    const w = this.p.win;
    w.append(el);
    const wr = w.getBoundingClientRect(), er = el.getBoundingClientRect();
    let left = x - wr.left - (center ? er.width / 2 : 0);
    let top = y - wr.top - (above ? er.height : 0);
    // (no room above: under it)
    if (above && top < 44) top = y - wr.top + 30;
    left = Math.max(8, Math.min(left, wr.width - er.width - 8));
    if (top + er.height > wr.height - 8) top = Math.max(8, y - wr.top - er.height - 30);
    Object.assign(el.style, { left: `${left}px`, top: `${top}px` });
  }

  private closePopOnly(): void {
    this.pop?.remove();
    this.pop = null;
  }

  private closePop(): void {
    this.closePopOnly();
    this.compose?.remove();
    this.compose = null;
    this.p.win.querySelector(".rv-finish")?.remove();
  }

  /** A note of `kind` on the selection `picked` (its source looked up now). */
  private async make(kind: ReviewNote["kind"], picked: { k: number; rects: Box[] }[], end: DOMRect): Promise<void> {
    const src = await this.host.source(picked);
    if (!src) return void (this.closePop(), this.p.msg("That text came from no file of the project (a header, a number the class made): Alt+click notes a spot instead", true));
    const anchor = { file: src.file, start: src.start, end: src.end, page: picked[0].k };
    const sel = (this.p.root as ShadowRoot & { getSelection?: () => Selection | null }).getSelection?.() ?? document.getSelection();
    const seen = sel?.toString() ?? "";
    if (kind === "highlight") {
      this.closePop();
      const r = await this.req("review_note", { kind, anchor, seen });
      if (r) (this.clearSelection(), this.p.msg("Highlighted"));
      return;
    }
    this.openCompose(end.left, end.bottom + 6, { kind, anchor, seen }, seen);
  }

  /** The box to write a note in, at (x, y). */
  private openCompose(x: number, y: number, note: { kind: ReviewNote["kind"]; anchor: object; seen?: string }, quote: string): void {
    this.closePop();
    const c = html`<div class="rv-compose" role="dialog" aria-label="${note.kind === "comment" ? "Comment for your agent" : "Note"}"><div class="q">${note.kind === "comment" ? "Comment for your agent on" : "Note on"} “${oneLine(quote, 120)}”</div><textarea placeholder="${note.kind === "comment" ? "What should your agent do here?" : "Your note…"}" aria-label="Its text" rows="3"></textarea><div class="act"><span class="hint">Ctrl+Enter saves</span><button type="button" class="btn" data-x>Cancel</button><button type="button" class="btn primary" data-save>${note.kind === "comment" ? "Comment" : "Save note"}</button></div></div>`.firstElementChild as HTMLElement;
    const ta = c.querySelector("textarea")!;
    const save = async () => {
      const text = ta.value.trim();
      if (!text) return ta.focus();
      const b = c.querySelector<HTMLButtonElement>("[data-save]")!;
      b.disabled = true;
      const r = await this.req("review_note", { ...note, text });
      b.disabled = false;
      if (!r) return;
      this.closePop();
      this.clearSelection();
      this.select(r.id, false, false);
      this.p.msg(note.kind === "comment" ? "Comment left for your agent" : "Note saved");
    };
    c.querySelector<HTMLElement>("[data-save]")!.onclick = () => void save();
    c.querySelector<HTMLElement>("[data-x]")!.onclick = () => this.closePop();
    ta.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) (e.preventDefault(), void save());
      if (e.key === "Escape") (e.stopPropagation(), this.closePop());
    });
    c.addEventListener("pointerdown", (e) => e.stopPropagation());
    this.place(c, x, y);
    this.compose = c;
    ta.focus();
  }

  private clearSelection(): void {
    const rn = this.p.root as ShadowRoot & { getSelection?: () => Selection | null };
    (rn.getSelection?.() ?? document.getSelection())?.removeAllRanges();
    this.picked = null;
  }

  /** The review's last step: its summary and verdict. */
  private finish(): void {
    this.closePop();
    const drafts = this.s.notes.filter((n) => n.draft);
    const on = new Set(drafts.map((n) => n.hunk?.id).filter(Boolean));
    const n = this.s.hunks.filter((h) => on.has(h.id)).length;
    const opt = (v: string, label: string, help: string, checked = false) => html`<label><input type="radio" name="rv-verdict" value="${v}" checked="${checked}"><span>${label}<small>${help}</small></span></label>`;
    const c = html`<div class="rv-finish" role="dialog" aria-label="Finish your review"><b>Finish your review</b><div class="q">${String(drafts.length)} comment${drafts.length === 1 ? "" : "s"} on ${String(n)} change${n === 1 ? "" : "s"}</div><textarea placeholder="Summary for your agent (optional)" aria-label="Summary" rows="3"></textarea>${opt("comment", "Comment", "Send the comments; decide later", true)}${opt("request_changes", "Request changes", "Your agent revises them; nothing is applied")}${opt("approve", "Approve", "Accept the changes commented on")}${opt("reject", "Reject", "Reject the changes commented on")}<div class="act"><button type="button" class="btn" data-x>Cancel</button><button type="button" class="btn primary" data-go>Submit review</button></div></div>`.firstElementChild as HTMLElement;
    c.querySelector<HTMLElement>("[data-x]")!.onclick = () => this.closePop();
    c.querySelector<HTMLElement>("[data-go]")!.onclick = async () => {
      const verdict = c.querySelector<HTMLInputElement>("input[name=rv-verdict]:checked")?.value ?? "comment";
      const summary = c.querySelector("textarea")!.value;
      const r = await this.req("review_submit", { verdict, summary });
      if (!r) return;
      this.closePop();
      this.p.msg(`Review sent: ${VERDICT_WORD[verdict as ReviewRec["verdict"]].toLowerCase()}`);
    };
    c.addEventListener("keydown", (e) => e.key === "Escape" && (e.stopPropagation(), this.closePop()));
    const r = this.side.getBoundingClientRect();
    this.place(c, r.left + 8, r.top + 48);
    this.compose = c;
    c.querySelector("textarea")!.focus();
  }
}
