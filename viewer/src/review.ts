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
// - Proposals (§9.6, `propose` mode): the agent's change kept apart from
//   the files, one card each (its title, why, files and hunks): Accept all,
//   Reject all, Request changes, Comment, Show diff (typeset, latexdiff,
//   against the files as they are), the source as words or lines, and each
//   hunk accepted, rejected or commented on (keys a, r, c; n, p to step).
//   In `direct` mode the agent's changes waiting are one such card.

import { html } from "./html.ts";
import type { Viewer } from "./viewer.ts";

type Box = [number, number, number, number];

export interface ReviewPost {
  by: string;
  text: string;
  at: string;
  edited?: { file: string; start: number; end: number; from_line: number; to_line: number }[];
  diff?: { id: string; file: string; line: number; old: string; new: string };
  /** A proposal's revision: what changed since the last. */
  changes?: { id: string; file: string; line: number; old: string; new: string }[];
  /** When its author last edited it. */
  edited_at?: string;
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
  /** A proposal's hunk: decided, open, or conflicting with the user's edits since. */
  state?: "open" | "accepted" | "rejected" | "conflict";
  proposal?: string;
}

export interface ReviewProposal {
  id: string;
  title: string;
  why: string;
  status: "open" | "changes_requested" | "accepted" | "rejected" | "partial" | "done";
  mode: "direct" | "propose";
  created: string;
  updated: string;
  revision: number;
  files: string[];
  thread: ReviewPost[];
  hunks: ReviewHunk[];
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
  /** Reviewing the agent's edits on (Settings, Experimental; off by default): off, no Changes tab, no marks of them. */
  changes?: boolean;
  /** The proposals, newest first (§9.6). */
  proposals?: ReviewProposal[];
  mode?: "direct" | "propose";
}

/** Where the review's state lives (the watch), and what the pages give it. */
export interface ReviewHost {
  /** A `review_*` op: its answer, or why not. */
  request(op: string, params?: object): Promise<{ ok: boolean; json?: any; error?: string }>;
  /** The state, whenever it changes (the `review` event). */
  onState(cb: (s: ReviewState) => void): void;
  /** Where bytes [start, end) of `file` are on the pages: the glyphs that came from them. */
  boxes(file: string, start: number, end: number): Promise<{ k: number; boxes: Box[] }[]>;
  /** The source of text selected on the pages. */
  source(sel: { k: number; rects: Box[] }[]): Promise<{ file: string; start: number; end: number } | null>;
  /** The source place nearest a point of a page. */
  spot(k: number, x: number, y: number): Promise<{ file: string; start: number } | null>;
  /** The editor to `file`'s `line`. */
  goto(file: string, line: number): void;
  /** Compare against the accepted text, typeset (true), or back to the pages (false); `proposal`: that proposal against the files as they are. */
  showChanges?(on: boolean, git: boolean, proposal?: string): void;
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
  /** Marks beside the pages' scrollbar: each note's and pending change's place, its kind's colour. */
  scrollMarks?(m: { k: number; y: number; cls: string; title: string; go: () => void }[]): void;
}

const KIND_WORD: Record<ReviewNote["kind"], string> = { highlight: "Highlight", note: "Note", comment: "Comment" };
const VERDICT_WORD: Record<ReviewRec["verdict"], string> = { approve: "Approved", request_changes: "Changes requested", reject: "Rejected", comment: "Commented" };
const STATUS_WORD: Record<ReviewProposal["status"], string> = { open: "Waiting for you", changes_requested: "Changes requested", accepted: "Accepted", rejected: "Rejected", partial: "Partly accepted", done: "Done" };
const STATE_WORD: Record<NonNullable<ReviewHunk["state"]>, string> = { open: "", accepted: "Accepted", rejected: "Rejected", conflict: "Conflicts with your edit" };

/** The open changes of a proposal (a direct batch's are all waiting). */
export function openHunks(x: ReviewProposal): ReviewHunk[] {
  return x.hunks.filter((h) => (h.state ?? "open") === "open");
}

/** A hunk as source lines: `-` the old lines, `+` the new, with the line around it. */
export function lineDiff(h: ReviewHunk): { del: string[]; add: string[] } {
  const lines = (t: string) => (t.endsWith("\n") ? t.slice(0, -1) : t).split("\n");
  const pre = h.before.replace(/^…/, "");
  const post = h.after.replace(/…$/, "");
  return { del: h.whole === "added" ? [] : lines(pre + h.old + post), add: h.whole === "deleted" ? [] : lines(pre + h.new + post) };
}

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
.rside .pcard { cursor: default; }
.rside .pcard .ptitle { font-weight: 650; font-size: 14px; margin: 4px 0 2px; overflow-wrap: anywhere; }
.rside .pcard .pwhy { white-space: pre-wrap; overflow-wrap: anywhere; color: var(--ph-fg); margin: 2px 0 4px; max-height: 8.5em; overflow: auto; }
.rside .pcard .pmeta { font-size: 12px; color: var(--ph-fg2); }
.rside .pcard .phunks { margin-top: 8px; }
.rside .pcard .phunks .card { margin: 0 0 6px; padding: 6px 8px; }
.rside .card.phunk:focus-visible, .rside .card.hunk:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.rside .card.phunk.st-accepted, .rside .card.phunk.st-rejected { opacity: .62; }
.rside .chip.st-accepted { color: #1a7f37; } .rside .chip.st-rejected, .rside .chip.st-conflict { color: var(--ph-danger); }
.rside .chip.st-changes_requested { color: var(--ph-warning); } .rside .chip.st-open { color: var(--ph-link); } .rside .chip.st-partial { color: var(--ph-fg2); }
.rside .ldiff { font: 12px/1.45 ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); margin: 6px 0 0; max-height: 260px; overflow: auto; }
.rside .ldiff div { padding: 0 6px; }
.rside .ldiff .d { background: rgb(255 80 70 / 14%); } .rside .ldiff .a { background: rgb(40 170 80 / 16%); }
.rside .ldiff .d::before { content: "−"; color: var(--ph-danger); margin-right: 6px; } .rside .ldiff .a::before { content: "+"; color: #1a7f37; margin-right: 6px; }
.rside .keys { font-size: 11px; color: var(--ph-fg2); margin-top: 6px; }
.rside .keys kbd { font: 11px ui-monospace, monospace; border: 1px solid var(--ph-border); border-radius: 3px; padding: 0 3px; }
.rside .post .on { color: var(--ph-fg2); font-size: 12px; }
.rside .pcompose { margin-top: 8px; }
.slot .rv-h { pointer-events: auto; position: absolute; width: 10px !important; margin-left: -5px; cursor: ew-resize; z-index: 4; background: none; }
.slot .rv-h::before { content: ""; position: absolute; left: 4px; top: 0; bottom: 0; width: 2px; background: var(--ph-accent, #1d3e72); }
.slot .rv-h::after { content: ""; position: absolute; left: 1px; width: 8px; height: 8px; border-radius: 50%; background: var(--ph-accent, #1d3e72); }
.slot .rv-h.start::after { top: -6px; } .slot .rv-h.end::after { bottom: -6px; }
.rv-toast { position: absolute; left: 50%; bottom: 40px; transform: translateX(-50%); z-index: 40; display: flex; gap: 12px; align-items: center; padding: 8px 10px 8px 14px; background: var(--ph-fg, #222); color: var(--ph-bar, #fff); border-radius: calc(var(--ph-radius) + 2px); box-shadow: 0 6px 20px rgb(0 0 0 / 30%); }
.rv-toast .btn { border: 0; background: none; color: inherit; font-weight: 700; cursor: pointer; padding: 2px 6px; text-decoration: underline; }
.rside .editbox { margin-top: 6px; }
.rside .editbox .seg { margin-bottom: 6px; }
.rside .pact { display: flex; gap: 10px; margin: 0 0 2px; }
.rside .lnk { border: 0; background: none; padding: 0; color: var(--ph-link); cursor: pointer; font-size: 12px; }
.rside .lnk:hover { text-decoration: underline; }
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
  /** The proposals shown as source lines (else words). */
  private lines = new Set<string>();
  /** A comment being written on a proposal (on one of its hunks; asking for changes). */
  private pcompose: { id: string; hunk?: string; request: boolean } | null = null;
  /** The proposal shown typeset (Show diff), "" the direct batch. */
  private showingP: string | null = null;
  /** The note being edited (`id`), or one of its replies (`id#i`). */
  private editing: string | null = null;
  private toastEl: HTMLElement | null = null;
  /** The change to focus once drawn (the next after one decided: the state comes after the op). */
  private wantFocus: string | null = null;
  /** The marks drawn last: the selected note's handles from them. */
  private lastOut: { k: number; boxes: Box[]; id: string }[] = [];

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
    this.body.addEventListener("input", (e) => e.target instanceof HTMLTextAreaElement && this.keepDraft(e.target));
    host.onState((s) => this.take(s));
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
      if (r.ok && r.json) this.take(r.json as ReviewState);
    });
  }

  /**
   * Whether the review is on: the user's Experimental switch
   * (`experimental-review`, the state's `changes`). Off, nothing of it
   * shows: no note kinds offered at a selection, no marks on the pages or
   * the scrollbar, no side, and the panel's button and menu item hidden
   * (`onEnabled`). Until the watch says, off.
   */
  get enabled(): boolean {
    return this.s.changes === true;
  }

  private enabledCb?: (on: boolean) => void;

  /** `cb(on)` now and whenever the review is switched on or off. */
  onEnabled(cb: (on: boolean) => void): void {
    this.enabledCb = cb;
    cb(this.enabled);
  }

  /** A state from the watch: the side, the marks, and the switch as it says. */
  private take(s: ReviewState): void {
    const was = this.enabled;
    this.s = s;
    if (!this.enabled) {
      this.closePop();
      if (this.isOpen) this.open(false);
    }
    if (was !== this.enabled) this.enabledCb?.(this.enabled);
    void this.marks();
    this.render();
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
    this.onCount?.(open, this.pendingCount());
  }

  /** Changes waiting for the user: the agent's hunks and the proposals' open ones. */
  private pendingCount(): number {
    const props = (this.s.proposals ?? []).filter((x) => x.mode === "propose").reduce((n, x) => n + openHunks(x).length, 0);
    return this.s.hunks.length + props;
  }

  get isOpen(): boolean {
    return this.p.win.classList.contains("review-on");
  }

  /** The side shown (or hidden), on `tab` if given. */
  open(on = !this.isOpen, tab?: "notes" | "changes"): void {
    if (on && !this.enabled) return;
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
    const nc = this.pendingCount();
    this.side.querySelector("#rv-nc")!.textContent = nc ? String(nc) : "";
    for (const t of this.side.querySelectorAll<HTMLElement>("[data-tab]")) t.setAttribute("aria-selected", String(t.dataset.tab === this.tab));
    if (!this.isOpen) return;
    // (the focus and the reply typed kept across a redraw)
    const focused = this.p.root.activeElement as HTMLElement | null;
    const focusKey = focused && this.body.contains(focused) ? (focused.dataset.focus ?? null) : null;
    const top = this.body.scrollTop;
    // (reviewing the agent's edits off: the notes alone)
    const changesOn = this.s.changes !== false;
    this.side.querySelector<HTMLElement>('[data-tab="changes"]')!.hidden = !changesOn;
    if (!changesOn && this.tab === "changes") this.tab = "notes";
    this.body.replaceChildren(this.tab === "notes" ? this.notesView() : this.changesView());
    // (what was typed in a reply or a comment, put back)
    for (const ta of this.body.querySelectorAll<HTMLTextAreaElement>("textarea[data-reply], textarea[data-comment], textarea[data-pc]"))
      ta.value = this.drafts.get(this.draftKey(ta)) ?? "";
    for (const ta of this.body.querySelectorAll<HTMLTextAreaElement>("textarea[data-edit]")) {
      const n = this.s.notes.find((x) => x.id === ta.dataset.edit);
      const key = `e:${ta.dataset.edit}#${ta.dataset.i}`;
      ta.value = this.drafts.get(key) ?? n?.thread[Number(ta.dataset.i)]?.text ?? "";
    }
    this.body.scrollTop = top;
    const want = this.wantFocus ? this.body.querySelector<HTMLElement>(`[data-ph="${CSS.escape(this.wantFocus)}"], .card.hunk[data-id="${CSS.escape(this.wantFocus)}"]`) : null;
    if (want) {
      want.focus();
      this.wantFocus = null;
    } else if (focusKey) {
      const f = this.body.querySelector<HTMLElement>(`[data-focus="${CSS.escape(focusKey)}"]`);
      if (f) {
        f.focus();
        if (f instanceof HTMLTextAreaElement) f.selectionStart = f.selectionEnd = f.value.length;
      }
    }
  }

  private draftKey(ta: HTMLTextAreaElement): string {
    if (ta.dataset.edit !== undefined) return `e:${ta.dataset.edit}#${ta.dataset.i}`;
    return ta.dataset.reply ?? (ta.dataset.pc !== undefined ? `p:${ta.dataset.pc}` : `h:${ta.dataset.comment}`);
  }

  /** What is typed in a box, kept for the next drawing (as it is typed: a box sent is forgotten with its draft). */
  private keepDraft(ta: HTMLTextAreaElement): void {
    const key = this.draftKey(ta);
    if (ta.value) this.drafts.set(key, ta.value);
    else this.drafts.delete(key);
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

  /** The box editing note `n`'s words (post `i`; its kind with the first). */
  private editBox(n: ReviewNote, i: number): DocumentFragment {
    const kinds = (["highlight", "note", "comment"] as const).map((k) => html`<button type="button" data-kind="${k}" data-id="${n.id}" aria-pressed="${n.kind === k}">${KIND_WORD[k]}</button>`);
    return html`<div class="editbox">${i === 0 && !n.hunk ? html`<span class="seg" role="group" aria-label="Kind">${kinds}</span>` : ""}<textarea data-edit="${n.id}" data-i="${String(i)}" data-focus="ed:${n.id}#${String(i)}" rows="3" aria-label="Edit the words" placeholder="${i === 0 && n.kind === "highlight" ? "Words for this highlight (none: a plain highlight)" : "Your words"}"></textarea><div class="act"><button type="button" class="btn primary" data-act="save-edit" data-id="${n.id}" data-i="${String(i)}">Save</button><button type="button" class="btn" data-act="cancel-edit" data-id="${n.id}">Cancel</button><span class="hint">Ctrl+Enter saves · Esc cancels</span></div></div>`;
  }

  private noteCard(n: ReviewNote, inHunk = false): DocumentFragment {
    const on = this.sel === n.id;
    const a = n.anchor;
    const quote = a.quote
      ? html`<div class="quote ${n.kind}">${oneLine(a.quote, on ? 2000 : 240)}</div>`
      : html`<div class="quote spot ${n.kind}">A spot on page ${String((a.page ?? 0) + 1)}</div>`;
    const editingNote = on && this.editing === n.id;
    const thread = n.thread.map((p, i) => (on && this.editing === `${n.id}#${i}` ? this.editBox(n, i) : html`${editingNote && i === 0 ? "" : this.post(p)}${on && !inHunk && i > 0 && p.by === "you" ? html`<div class="pact"><button type="button" class="lnk" data-act="edit-post" data-id="${n.id}" data-i="${String(i)}">Edit</button><button type="button" class="lnk" data-act="del-post" data-id="${n.id}" data-i="${String(i)}">Delete</button></div>` : ""}`));
    if (editingNote) thread.unshift(this.editBox(n, 0));
    const lastBy = n.thread.at(-1)?.by;
    // (a comment asks the agent; a note and a highlight are kept, asking nothing)
    const waiting = n.status === "open" && n.kind === "comment" && lastBy !== "agent" && !n.draft && !n.verdict;
    const reply = on && !n.draft && !this.editing?.startsWith(n.id)
      ? html`<textarea data-focus="reply:${n.id}" data-reply="${n.id}" placeholder="${n.kind === "highlight" ? "Add a note…" : "Reply…"}" aria-label="Reply to this note" rows="2"></textarea><div class="act"><button type="button" class="btn primary" data-act="reply" data-id="${n.id}" data-focus="send:${n.id}">Reply</button>${n.status === "open" ? html`<button type="button" class="btn" data-act="resolve" data-id="${n.id}" data-focus="res:${n.id}">${icon("check", 14)} Resolve</button>` : html`<button type="button" class="btn" data-act="reopen" data-id="${n.id}" data-focus="reo:${n.id}">${icon("undo", 14)} Reopen</button>`}<span class="grow"></span>${inHunk ? "" : html`<button type="button" class="btn" data-act="edit" data-id="${n.id}" data-focus="edit:${n.id}" title="Edit its words or kind (Enter)">${icon("note", 14)} Edit</button>`}<button type="button" class="btn" data-act="delete" data-id="${n.id}" data-focus="del:${n.id}" title="Delete (Delete key)">${icon("delete", 14)} Delete</button></div><div class="hint">Ctrl+Enter replies · Enter on the card edits · Delete deletes · Esc closes</div>`
      : "";
    const answered = (n.hunks ?? []).length;
    return html`<div class="card note${on ? " on" : ""}${n.status === "resolved" ? " resolved" : ""}" data-id="${n.id}" data-focus="card:${n.id}" tabindex="0" role="button" aria-expanded="${on}" aria-label="${`${KIND_WORD[n.kind]} at ${a.file} line ${a.line}${n.status === "resolved" ? ", resolved" : ""}`}"><div class="top"><span class="chip ${n.kind}">${KIND_WORD[n.kind]}</span>${n.draft ? html`<span class="chip draft">Pending</span>` : ""}${n.detached ? html`<span class="chip det" title="Its text was edited away: shown where it was">Detached</span>` : ""}${n.status === "resolved" ? html`<span class="chip done">Resolved</span>` : ""}${inHunk ? "" : html`<button type="button" class="place" data-goto="${a.file}" data-line="${String(a.line)}" title="Open in the editor">${placeOf(a.file, a.line)}</button>`}<span class="grow"></span><span title="${n.updated}">${ago(n.updated)}</span></div>${inHunk ? "" : quote}${thread}${!on && waiting ? html`<div class="more">Waiting for your agent</div>` : ""}${!on && answered ? html`<div class="more">${String(answered)} change${answered === 1 ? "" : "s"} answer this note: see Changes</div>` : ""}${reply}</div>`;
  }

  private post(p: ReviewPost): DocumentFragment {
    if (p.changes?.length)
      return html`<div class="post update"><span class="by agent">Agent</span>revised it<span class="when">${ago(p.at)}</span>${p.text ? html`<div class="txt">${p.text}</div>` : ""}${p.changes.slice(0, 8).map((c) => html`<div class="diff"><span class="ctx">${c.file}:${String(c.line)} </span>${c.old ? html`<del>${oneLine(c.old, 160)}</del>` : ""}${c.new ? html`<ins>${oneLine(c.new, 160)}</ins>` : ""}</div>`)}${p.changes.length > 8 ? html`<div class="more">and ${String(p.changes.length - 8)} more</div>` : ""}</div>`;
    if (p.diff && p.by !== "agent")
      return html`<div class="post"><span class="by">You</span><span class="txt">${p.text}</span><span class="when" title="${p.at}">${ago(p.at)}</span><div class="on">on ${placeOf(p.diff.file, p.diff.line)}: ${p.diff.old ? html`<del>${oneLine(p.diff.old, 60)}</del>` : ""}${p.diff.new ? html`<ins>${oneLine(p.diff.new, 60)}</ins>` : ""}</div></div>`;
    if (p.diff)
      return html`<div class="post update"><span class="by agent">Agent</span>changed it<span class="when">${ago(p.at)}</span><div class="diff">${p.diff.old ? html`<del>${oneLine(p.diff.old, 160)}</del>` : ""}${p.diff.new ? html`<ins>${oneLine(p.diff.new, 160)}</ins>` : ""}</div></div>`;
    const edited = (p.edited ?? []).map((e) => html`<button type="button" class="place" data-goto="${e.file}" data-line="${String(e.from_line)}" data-from="${String(e.start)}" data-to="${String(e.end)}">${placeOf(e.file, e.from_line, e.to_line)}</button> `);
    return html`<div class="post"><span class="by ${p.by === "agent" ? "agent" : ""}">${p.by === "agent" ? "Agent" : "You"}</span><span class="txt">${p.text}</span><span class="when" title="${p.edited_at ? `Edited ${p.edited_at}` : p.at}">${ago(p.at)}${p.edited_at ? " · edited" : ""}</span>${edited.length ? html`<span class="edited">Edited ${edited}</span>` : ""}</div>`;
  }

  private changesView(): DocumentFragment {
    const mode = this.s.mode ?? "direct";
    const props = this.s.proposals ?? [];
    const drafts = this.s.notes.filter((n) => n.draft);
    const bar = drafts.length
      ? html`<div class="rbar"><span><b>${String(drafts.length)}</b> comment${drafts.length === 1 ? "" : "s"} in your review</span><span class="grow"></span><button type="button" class="btn primary" data-act="finish">Finish review</button></div>`
      : "";
    const last = this.s.reviews.at(-1);
    const rec = last ? html`<div class="reviewrec">Last review: ${VERDICT_WORD[last.verdict]}${last.summary ? `: ${oneLine(last.summary, 80)}` : ""} · ${ago(last.at)}</div>` : "";
    const segb = (m: "direct" | "propose", label: string, title: string) => html`<button type="button" data-mode="${m}" aria-pressed="${mode === m}" title="${title}">${label}</button>`;
    const modes = html`<div class="rtools"><span class="hint">Your agent</span><span class="seg" role="group" aria-label="How your agent's changes come">${segb("direct", "Edits", "It edits the files; its changes show on the pages, waiting for you")}${segb("propose", "Proposes", "Its changes are kept apart until you accept them; the pages show them only with Show diff")}</span></div>`;
    // (the direct batch: the agent's changes waiting, in either mode)
    const batch = this.s.hunks.length ? this.batchCard(props.find((x) => x.mode === "direct" && x.status !== "done")) : "";
    const live = props.filter((x) => x.mode === "propose" && (x.status === "open" || x.status === "changes_requested"));
    const decided = props.filter((x) => x.mode === "propose" && !live.includes(x));
    const empty = !this.s.hunks.length && !live.length
      ? mode === "propose"
        ? html`<div class="empty"><b>No proposals waiting.</b><br>Your agent writes its change in the workspace (<code>.phitex/work/</code>) and sends it with <code>phitex session propose -m "why"</code>. Your files and pages stay as they are until you accept it here.<br><br>Accept all, or change by change; <b>Show diff</b> typesets it against your text.</div>`
        : html`<div class="empty"><b>No changes waiting.</b><br>Edits your agent makes to the files show here, word by word, to accept or reject. ${this.s.git ? "Accepting one stages it in git; rejecting one puts the staged text back." : "Accepted text is kept in .phitex/baseline."}<br><br>Edits you type in an editor attached to this watch are yours and are accepted as you type.</div>`
      : "";
    return html`${bar}${rec}${modes}${empty}${live.map((x) => this.propCard(x))}${batch}${decided.length ? html`<h3>Decided</h3>${decided.slice(0, 6).map((x) => this.propCard(x))}` : ""}${this.earlier(!this.s.hunks.length)}`;
  }

  /** The keys of a card's changes, said under its buttons. */
  private keysHint(): DocumentFragment {
    return html`<div class="keys"><kbd>n</kbd> <kbd>p</kbd> next, previous change · <kbd>a</kbd> accept · <kbd>r</kbd> reject · <kbd>c</kbd> comment</div>`;
  }

  /** A card's buttons and its compose box (`id`: the proposal, "" the direct batch not named). */
  private cardTools(id: string, open: number, showing: boolean, propose: boolean): DocumentFragment {
    const lines = this.lines.has(id);
    return html`<div class="act"><button type="button" class="btn accept" data-pact="accept-all" data-p="${id}" disabled="${!open}" title="Accept every change waiting${propose ? ": applied to your files" : ""}">${icon("check", 14)} Accept all</button><button type="button" class="btn reject" data-pact="reject-all" data-p="${id}" disabled="${!open}" title="${propose ? "Reject every change waiting: your files stay as they are" : "Reject every change: the files get the accepted text back"}">Reject all</button><button type="button" class="btn" data-pact="request" data-p="${id}" title="Ask your agent to change it (it is told at once)">Request changes</button><button type="button" class="btn" data-pact="comment" data-p="${id}">Comment</button></div><div class="act"><label class="hint" title="Typeset it against your text (latexdiff), on the pages"><input type="checkbox" data-pact="show" data-p="${id}" checked="${showing}"> Show diff</label><span class="grow"></span><span class="seg" role="group" aria-label="The source as"><button type="button" data-pact="words" data-p="${id}" aria-pressed="${!lines}">Words</button><button type="button" data-pact="lines" data-p="${id}" aria-pressed="${lines}">Lines</button></span></div>${this.composeBox(id)}`;
  }

  private composeBox(id: string): DocumentFragment | "" {
    const c = this.pcompose;
    if (!c || c.id !== id) return "";
    const h = c.hunk ? this.findHunk(c.hunk) : undefined;
    return html`<div class="pcompose">${h ? html`<div class="hint">On ${placeOf(h.file, h.line)}</div>` : ""}<textarea data-pc="${id}" data-focus="pc:${id}" rows="2" placeholder="${c.request ? "What should your agent change?" : "Your comment for the agent…"}" aria-label="${c.request ? "Changes to request" : "Comment"}"></textarea><div class="act"><button type="button" class="btn primary" data-pact="send" data-p="${id}">${c.request ? "Request changes" : "Comment"}</button><button type="button" class="btn" data-pact="cancel" data-p="${id}">Cancel</button><span class="hint">Ctrl+Enter sends · Esc cancels</span></div></div>`;
  }

  private findHunk(id: string): ReviewHunk | undefined {
    return this.s.hunks.find((h) => h.id === id) ?? (this.s.proposals ?? []).flatMap((x) => x.hunks).find((h) => h.id === id);
  }

  /** The direct batch: the agent's changes waiting, under the title and why it gave them. */
  private batchCard(x: ReviewProposal | undefined): DocumentFragment {
    const id = x?.id ?? "";
    const hs = this.s.hunks;
    const files = [...new Set(hs.map((h) => h.file))];
    const groups = groupHunks(this.s);
    const flash = (this.s.mode ?? "direct") === "propose" ? html`<div class="hint">Your agent edited these files directly, so they show on the pages: <code>phitex session propose</code> makes them a proposal and puts the files back.</div>` : "";
    return html`<div class="card pcard batch" data-p="${id}"><div class="top"><span class="chip st-${x?.status ?? "open"}">${STATUS_WORD[x?.status ?? "open"]}</span><span class="grow"></span><span>${x ? ago(x.updated) : ""}</span></div><div class="ptitle">${x?.title || "The agent's changes"}</div>${x?.why && x.why !== x.title ? html`<div class="pwhy">${x.why}</div>` : ""}<div class="pmeta">${files.join(", ")} · ${String(hs.length)} change${hs.length === 1 ? "" : "s"}${this.s.git ? " · accepting stages them in git" : ""}</div>${flash}${this.cardTools(id, hs.length, this.showing && this.showingP === "", false)}${(x?.thread ?? []).map((p) => this.post(p))}<div class="phunks">${groups.map((g) => html`${groups.length > 1 || g.note ? html`<h3>${g.title}${g.note ? html` <button type="button" class="place" data-note="${g.note.id}">${g.note.id}</button>` : ""}</h3>` : ""}${g.hunks.map((h) => this.hunkCard(h, this.lines.has(id)))}`)}</div>${this.keysHint()}</div>`;
  }

  /** A proposal: its title, why, files, hunks and thread, and what the user can do with it. */
  private propCard(x: ReviewProposal): DocumentFragment {
    const open = openHunks(x).length;
    const count = (st: string) => x.hunks.filter((h) => h.state === st).length;
    const said = [count("accepted") && `${count("accepted")} accepted`, count("rejected") && `${count("rejected")} rejected`, count("conflict") && `${count("conflict")} conflicting`].filter(Boolean).join(", ");
    const live = x.status === "open" || x.status === "changes_requested";
    return html`<div class="card pcard" data-p="${x.id}"><div class="top"><span class="chip st-${x.status}">${STATUS_WORD[x.status]}</span><span>${x.id}${x.revision > 1 ? ` · revision ${x.revision}` : ""}</span><span class="grow"></span><span title="${x.updated}">${ago(x.updated)}</span></div><div class="ptitle">${x.title}</div>${x.why && x.why !== x.title ? html`<div class="pwhy">${x.why}</div>` : ""}<div class="pmeta">${x.files.join(", ")} · ${String(x.hunks.length)} change${x.hunks.length === 1 ? "" : "s"}${said ? ` (${said})` : ""}</div>${live ? this.cardTools(x.id, open, this.showing && this.showingP === x.id, true) : ""}${x.thread.map((p) => this.post(p))}${live ? html`<div class="phunks">${x.hunks.map((h) => this.propHunk(x, h))}</div>${this.keysHint()}` : ""}</div>`;
  }

  /** The source of a change: its words (deleted and added in line), or its lines. */
  private source(h: ReviewHunk, lines: boolean): DocumentFragment {
    if (lines) {
      const d = lineDiff(h);
      return html`<div class="ldiff">${d.del.map((l) => html`<div class="d">${l}</div>`)}${d.add.map((l) => html`<div class="a">${l}</div>`)}</div>`;
    }
    if (h.whole) return html`<div class="diff">${h.whole === "added" ? html`<ins>${oneLine(h.new, 600)}</ins>` : html`<del>${oneLine(h.old, 600)}</del>`}</div>`;
    return html`<div class="diff"><span class="ctx">${h.before}</span>${h.parts.map(([k, t]) => (k === "del" ? html`<del>${t}</del>` : k === "add" ? html`<ins>${t}</ins>` : html`<span>${t}</span>`))}<span class="ctx">${h.after}</span></div>`;
  }

  private propHunk(x: ReviewProposal, h: ReviewHunk): DocumentFragment {
    const st = h.state ?? "open";
    const what = h.whole === "added" ? " (new file)" : h.whole === "deleted" ? " (file deleted)" : "";
    const open = st === "open";
    return html`<div class="card hunk phunk st-${st}" data-ph="${h.id}" data-p="${x.id}" data-focus="ph:${h.id}" tabindex="0" aria-label="${`Proposed change at ${h.file} line ${h.line}${what}${st !== "open" ? `, ${STATE_WORD[st]}` : ""}`}"><div class="top"><button type="button" class="place" data-goto="${h.file}" data-line="${String(h.line)}" title="Open in the editor"><span class="file">${placeOf(h.file, h.line, h.end_line)}</span></button>${what}${st !== "open" ? html`<span class="chip st-${st}">${STATE_WORD[st]}</span>` : ""}<span class="grow"></span><button type="button" class="btn accept" data-pact="accept" data-p="${x.id}" data-h="${h.id}" disabled="${!open}" aria-label="Accept this change">${icon("check", 14)} Accept</button><button type="button" class="btn reject" data-pact="reject" data-p="${x.id}" data-h="${h.id}" disabled="${!open}" aria-label="Reject this change">Reject</button><button type="button" class="ib" data-pact="comment" data-p="${x.id}" data-h="${h.id}" title="Comment on this change" aria-label="Comment on this change">${icon("comment", 16)}</button></div>${this.source(h, this.lines.has(x.id))}</div>`;
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

  private hunkCard(h: ReviewHunk, lines = false): DocumentFragment {
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
    const diff = lines ? this.source(h, true) : h.whole ? html`<div class="diff">${h.whole === "added" ? html`<ins data-at="${String(h.now[0])}">${oneLine(h.new, 600)}</ins>` : html`<del>${oneLine(h.old, 600)}</del>`}</div>` : html`<div class="diff" data-hunk="${h.id}"><span class="ctx">${h.before}</span>${parts}<span class="ctx">${h.after}</span></div>`;
    const box = on
      ? html`<textarea data-focus="hc:${h.id}" data-comment="${h.id}" placeholder="Comment on this change (select words in it to comment on them)…" aria-label="Comment on this change" rows="2"></textarea><div class="act"><button type="button" class="btn primary" data-act="comment" data-id="${h.id}">Comment</button><button type="button" class="btn" data-act="draft" data-id="${h.id}" title="Gather it with others and submit them as one review">Add to review</button></div>`
      : "";
    return html`<div class="card hunk${on ? " on" : ""}" data-id="${h.id}" data-focus="card:${h.id}" tabindex="0" role="button" aria-expanded="${on}" aria-label="${`Change at ${h.file} line ${h.line}${what}`}"><div class="top"><button type="button" class="place" data-goto="${h.file}" data-line="${String(h.line)}" title="Open in the editor"><span class="file">${placeOf(h.file, h.line, h.end_line)}</span></button>${what}<span class="grow"></span><button type="button" class="btn accept" data-act="accept" data-id="${h.id}" title="Accept: ${this.s.git ? "staged in git" : "kept as the accepted text"}" aria-label="Accept this change">${icon("check", 14)} Accept</button><button type="button" class="btn reject" data-act="reject" data-id="${h.id}" title="Reject: the file gets the accepted text back" aria-label="Reject this change">Reject</button></div>${diff}${comments.length ? html`<div class="comments">${comments.map((c) => this.noteCard(c, true))}</div>` : ""}${box}</div>`;
  }

  // ---- what the user does in the side ----

  private keyed(e: KeyboardEvent): void {
    const t = e.target as HTMLElement;
    if (t instanceof HTMLTextAreaElement && t.dataset.edit !== undefined) {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) (e.preventDefault(), void this.act("save-edit", t.dataset.edit, t.dataset.i));
      else if (e.key === "Escape") (e.stopPropagation(), void this.act("cancel-edit", t.dataset.edit));
      return;
    }
    // (a note's card: Enter opens it, again edits; Delete deletes; Esc closes)
    if (t.classList.contains("note") && t.classList.contains("card") && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const id = t.dataset.id!;
      if (e.key === "Enter" && this.sel === id) return void (e.preventDefault(), this.act("edit", id));
      if (e.key === "Delete" || e.key === "Backspace") return void (e.preventDefault(), e.stopPropagation(), this.act("delete", id));
      if (e.key === "Escape" && this.sel === id) return void (e.stopPropagation(), this.select(null), this.body.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(id)}"]`)?.focus());
    }
    if (t instanceof HTMLTextAreaElement && t.dataset.pc !== undefined) {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) (e.preventDefault(), void this.pact("send", t.dataset.pc));
      else if (e.key === "Escape") (e.stopPropagation(), void this.pact("cancel", t.dataset.pc));
      return;
    }
    // (a card's changes: n, p to step; a, r, c on the one focused)
    const hc = t.closest<HTMLElement>(".card.hunk");
    const card = t.closest<HTMLElement>(".pcard");
    if (card && !(t instanceof HTMLTextAreaElement) && !e.ctrlKey && !e.altKey && !e.metaKey && "nprac".includes(e.key) && e.key.length === 1) {
      const list = [...card.querySelectorAll<HTMLElement>(".phunks .card.hunk")];
      const i = hc ? list.indexOf(hc) : -1;
      const pid = card.dataset.p ?? "";
      const hid = hc?.dataset.ph ?? hc?.dataset.id;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "n" || e.key === "p") return void list[Math.max(0, Math.min(list.length - 1, i + (e.key === "n" ? 1 : i < 0 ? 1 : -1)))]?.focus();
      if (!hid) return void list[0]?.focus();
      if (e.key === "c") return void this.pact("comment", pid, hid);
      return void this.pact(e.key === "a" ? "accept" : "reject", pid, hid);
    }
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
    const kd = t.closest<HTMLElement>("[data-kind]");
    if (kd) return void (e.stopPropagation(), this.req("review_edit", { id: kd.dataset.id, kind: kd.dataset.kind }));
    const md = t.closest<HTMLElement>("[data-mode]");
    if (md) return void this.setMode(md.dataset.mode as "direct" | "propose");
    const pa = t.closest<HTMLElement>("[data-pact]");
    if (pa) {
      e.stopPropagation();
      if (pa.dataset.pact === "show") return void this.pshow(pa.dataset.p ?? "", (pa as HTMLInputElement).checked);
      return void this.pact(pa.dataset.pact!, pa.dataset.p ?? "", pa.dataset.h);
    }
    const a = t.closest<HTMLElement>("[data-act]");
    if (a) {
      e.stopPropagation();
      if (a.dataset.act === "show") return this.show((a as HTMLInputElement).checked);
      return void this.act(a.dataset.act!, a.dataset.id, a.dataset.i);
    }
    if (t.closest("textarea, .diff, .ldiff, .pcard:not(.batch) .phunk, .pcard > *:not(.phunks)")) return;
    const card = t.closest<HTMLElement>(".card");
    if (card && this.sel !== card.dataset.id) this.select(card.dataset.id!, true);
  }

  /** The review mode: this watch's (and said). */
  async setMode(m: "direct" | "propose"): Promise<void> {
    if ((this.s.mode ?? "direct") === m) return;
    if (await this.req("review_mode", { mode: m })) this.p.msg(m === "propose" ? "Your agent's changes now come as proposals: your files stay as they are until you accept one" : "Your agent edits the files; its changes wait for you here");
  }

  /** Show diff of proposal `id` ("" the direct batch): typeset, or back to the pages. */
  private pshow(id: string, on: boolean): void {
    this.showingP = on ? id : null;
    this.showing = on;
    if (id && id !== this.directId()) this.host.showChanges?.(on, this.s.git, id);
    else this.host.showChanges?.(on, this.s.git);
    this.render();
    void this.marks();
  }

  private directId(): string {
    return (this.s.proposals ?? []).find((x) => x.mode === "direct" && x.status !== "done")?.id ?? "";
  }

  /** What the user does with proposal `id` ("" the direct batch not named yet), or its hunk `h`. */
  async pact(act: string, id: string, h?: string): Promise<void> {
    const direct = !id || id === this.directId();
    const x = (this.s.proposals ?? []).find((p) => p.id === id);
    switch (act) {
      case "words":
      case "lines":
        if (act === "lines") this.lines.add(id);
        else this.lines.delete(id);
        return this.render();
      case "comment":
      case "request":
        this.pcompose = { id, hunk: h, request: act === "request" };
        this.render();
        this.body.querySelector<HTMLTextAreaElement>(`textarea[data-pc="${CSS.escape(id)}"]`)?.focus();
        return;
      case "cancel":
        this.pcompose = null;
        this.drafts.delete(`p:${id}`);
        return this.render();
      case "send": {
        const c = this.pcompose;
        const ta = this.body.querySelector<HTMLTextAreaElement>(`textarea[data-pc="${CSS.escape(id)}"]`);
        const text = ta?.value.trim() ?? "";
        if (!text && !c?.request) return void ta?.focus();
        const r = await this.req("review_proposal_comment", { id, text, ...(c?.hunk ? { hunk: c.hunk } : {}), ...(c?.request ? { verdict: "request_changes" } : {}) });
        if (!r) return;
        this.drafts.delete(`p:${id}`);
        this.pcompose = null;
        this.p.msg(c?.request ? "Changes requested: your agent is told" : "Comment sent to your agent");
        return this.render();
      }
      case "accept-all":
      case "reject-all": {
        const n = direct ? this.s.hunks.length : x ? openHunks(x).length : 0;
        const yes = act === "accept-all";
        if (!yes && !confirm(`Reject all ${n} change${n === 1 ? "" : "s"}?${direct ? " The files get the accepted text back." : " Your files stay as they are."}`)) return;
        if (direct && !id) return this.verdict(yes ? "accept" : "reject", "all");
        const r = await this.req(`review_proposal_${yes ? "accept" : "reject"}`, { id, hunks: "all" });
        if (r) this.p.msg(`${yes ? "Accepted" : "Rejected"} ${String(r.done ?? n)} change${Number(r.done ?? n) === 1 ? "" : "s"}${yes && !direct ? ": applied to your files" : ""}`);
        return this.afterDecision(id);
      }
      case "accept":
      case "reject": {
        if (!h) return;
        // (the change after it focused next: going down the list)
        const card = this.body.querySelector<HTMLElement>(`.pcard[data-p="${CSS.escape(id)}"]`);
        const list = card ? [...card.querySelectorAll<HTMLElement>(".phunks .card.hunk")].map((c) => c.dataset.ph ?? c.dataset.id!) : [];
        const next = list[list.indexOf(h) + 1] ?? list[list.indexOf(h) - 1];
        this.wantFocus = next ?? null;
        if (direct && !id) await this.verdict(act, [h]);
        else {
          const r = await this.req(`review_proposal_${act}`, { id, hunks: [h] });
          if (!r) return;
          this.p.msg(act === "accept" ? (direct ? "Accepted" : "Accepted: applied to your file") : "Rejected");
        }
        await this.afterDecision(id);
        // (drawn already: focused now; else when the state comes)
        const el = next ? this.body.querySelector<HTMLElement>(`[data-ph="${CSS.escape(next)}"], .card.hunk[data-id="${CSS.escape(next)}"]`) : null;
        if (el && el.isConnected) el.focus();
        return;
      }
    }
  }

  /** A short message with Undo, gone after a few seconds: a deletion's way back. */
  private toast(text: string, undo: { id?: string; gone: unknown }): void {
    this.toastEl?.remove();
    const el = html`<div class="rv-toast" role="status"><span>${text}</span><button type="button" class="btn">Undo</button></div>`.firstElementChild as HTMLElement;
    this.p.win.append(el);
    this.toastEl = el;
    const done = setTimeout(() => el.remove(), 6000);
    el.querySelector("button")!.onclick = async () => {
      clearTimeout(done);
      el.remove();
      if (await this.req("review_restore", undo)) this.p.msg("Restored");
    };
  }

  /** After a decision: the typeset diff made again where shown. */
  private async afterDecision(id: string): Promise<void> {
    if (this.showing && this.showingP === id) this.host.showChanges?.(true, this.s.git, id && id !== this.directId() ? id : undefined);
  }

  private show(on: boolean): void {
    this.showing = on;
    this.host.showChanges?.(on, this.s.git);
    void this.marks();
  }

  /** The op `act` on note or hunk `id` (`i`: a post of its thread). */
  private async act(act: string, id?: string, i?: string): Promise<void> {
    const r = (op: string, params?: object) => this.req(op, params);
    switch (act) {
      case "edit":
      case "edit-post": {
        this.editing = act === "edit" ? id! : `${id}#${i}`;
        if (this.sel !== id) this.sel = id!;
        this.render();
        const ta = this.body.querySelector<HTMLTextAreaElement>(`textarea[data-edit="${CSS.escape(id!)}"]`);
        if (ta) (ta.focus(), (ta.selectionStart = ta.selectionEnd = ta.value.length));
        return;
      }
      case "cancel-edit":
        for (const k of [...this.drafts.keys()]) if (k.startsWith(`e:${id}#`)) this.drafts.delete(k);
        this.editing = null;
        this.render();
        this.body.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(id!)}"]`)?.focus();
        return;
      case "save-edit": {
        const ta = this.body.querySelector<HTMLTextAreaElement>(`textarea[data-edit="${CSS.escape(id!)}"]`);
        const text = ta?.value.trim() ?? "";
        if (!(await r("review_edit", { id, text, post: Number(i ?? 0) }))) return;
        for (const k of [...this.drafts.keys()]) if (k.startsWith(`e:${id}#`)) this.drafts.delete(k);
        this.editing = null;
        this.render();
        this.body.querySelector<HTMLElement>(`.card[data-id="${CSS.escape(id!)}"]`)?.focus();
        return;
      }
      case "del-post": {
        const g = await r("review_delete", { id, post: Number(i) });
        if (g) this.toast("Reply deleted", { id, gone: g.gone });
        return;
      }
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
      case "delete": {
        const n = this.s.notes.find((x) => x.id === id);
        // (the thread goes with it: asked once where the agent took part)
        if (n && (n.thread.some((p) => p.by === "agent") || (n.hunks ?? []).length) && !confirm("Delete this note with its thread? Your agent's replies go too.")) return;
        const g = await r("review_delete", { id });
        if (!g) return;
        this.select(null);
        this.toast(`${n ? KIND_WORD[n.kind] : "Note"} deleted`, { gone: g.gone });
        return;
      }
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
    // (a note's or proposal's id as `target`: `id` is the request's own, the reply's name)
    const { id, ...rest } = (params ?? {}) as { id?: unknown };
    const r = await this.host.request(op, id === undefined ? rest : { ...rest, target: id });
    if (!r.ok) {
      this.p.msg(r.error ?? "The watch did not answer", true);
      return null;
    }
    return r.json ?? {};
  }

  /** Note or hunk `id` selected (its card open, its marks outlined); `scroll`: the page to it; `focus`: its card focused. */
  select(id: string | null, scroll = false, focus = false): void {
    if (this.editing && this.editing.split("#")[0] !== id) this.editing = null;
    this.sel = id;
    const note = id ? this.s.notes.find((n) => n.id === id) : undefined;
    if (note && !note.hunk) {
      this.tab = "notes";
      if (this.filter !== "all" && note.status !== this.filter) this.filter = "all";
    } else if (id && (this.s.hunks.some((h) => h.id === id) || note?.hunk)) this.tab = "changes";
    if (id && !this.isOpen) this.open(true);
    this.render();
    this.outline();
    this.handles(this.lastOut);
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
    const at = await this.host.boxes(file, a, Math.max(b, a + 1));
    if (at.length) this.p.viewer.scrollToPlace(at[0].k, at[0].boxes[0][1]);
    else this.p.msg("Not on the pages (none of its text is typeset)");
  }

  /** Bytes [a, b) of `file` shown on the pages a moment (an edit a thread recorded). */
  private async reveal(file: string, a: number, b: number): Promise<void> {
    const at = await this.host.boxes(file, a, Math.max(b, a + 1));
    if (at.length) this.p.viewer.mark(at[0].k, at[0].boxes);
  }

  /** Each note's and pending change's place beside the scrollbar: its kind's colour, resolved ones fainter; hovered its first line, clicked opened. */
  private scrollbar(where: Map<string, [number, number]>): void {
    if (!this.p.scrollMarks) return;
    const out: { k: number; y: number; cls: string; title: string; go: () => void }[] = [];
    for (const [id, [k, y]] of where) {
      const n = this.s.notes.find((x) => x.id === id);
      const h = n ? undefined : this.s.hunks.find((x) => x.id === id);
      if (!n && !h) continue;
      const title = n ? `${KIND_WORD[n.kind]}: ${oneLine(n.thread.find((p) => p.text)?.text || n.anchor.quote, 80)}` : `Change at ${placeOf(h!.file, h!.line)}: ${oneLine(h!.new || h!.old, 70)}`;
      const cls = n ? `rv-${n.kind}${n.status === "resolved" ? " rv-dim" : ""}` : "rv-change";
      out.push({ k, y, cls, title, go: () => this.select(id, true, true) });
    }
    this.p.scrollMarks(out);
  }

  /** The selected note's ends on the pages: handles that move them (a drag, then its source place). */
  private handles(out: { k: number; boxes: Box[]; id: string }[]): void {
    for (const h of this.p.stage.querySelectorAll(".rv-h")) h.remove();
    const n = this.sel ? this.s.notes.find((x) => x.id === this.sel) : undefined;
    if (!n || !n.anchor.quote || n.hunk) return;
    const mine = out.filter((m) => m.id === n.id && m.boxes.length);
    if (!mine.length) return;
    const size = this.p.viewer.pageSize();
    const first = mine[0], last = mine.at(-1)!;
    const put = (k: number, b: Box, end: boolean) => {
      const slot = this.p.stage.querySelector<HTMLElement>(`.slot[data-k="${k}"]`);
      if (!slot) return;
      const h = document.createElement("div");
      h.className = `rv rv-h ${end ? "end" : "start"}`;
      h.title = end ? "Drag to move the note's end" : "Drag to move the note's start";
      Object.assign(h.style, { left: `${((end ? b[0] + b[2] : b[0]) / size.w) * 100}%`, top: `${(b[1] / size.h) * 100}%`, height: `${(b[3] / size.h) * 100}%` });
      h.addEventListener("pointerdown", (e) => this.drag(e, h, n, end));
      slot.append(h);
    };
    put(first.k, first.boxes[0], false);
    put(last.k, last.boxes.at(-1)!, true);
  }

  private drag(e: PointerEvent, h: HTMLElement, n: ReviewNote, end: boolean): void {
    e.preventDefault();
    e.stopPropagation();
    h.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const slot = h.parentElement!.getBoundingClientRect();
      h.style.left = `${((m.clientX - slot.left) / slot.width) * 100}%`;
      h.style.top = `${((m.clientY - slot.top) / slot.height) * 100 - 1}%`;
    };
    const up = async (u: PointerEvent) => {
      h.removeEventListener("pointermove", move);
      h.removeEventListener("pointerup", up);
      // (the page under the pointer, and the source place nearest it there)
      const slot = (this.p.root.elementsFromPoint?.(u.clientX, u.clientY) ?? []).map((x) => (x as HTMLElement).closest?.(".slot")).find(Boolean) as HTMLElement | undefined;
      if (!slot) return void this.marks();
      const r = slot.getBoundingClientRect();
      const size = this.p.viewer.pageSize();
      const at = await this.host.spot(Number(slot.dataset.k), ((u.clientX - r.left) / r.width) * size.w, ((u.clientY - r.top) / r.height) * size.h);
      if (!at || at.file !== n.anchor.file) return void (this.p.msg("Not in the note's file there", true), this.marks());
      // (to the word's end or start there: the host snaps it)
      if (await this.req("review_anchor", end ? { id: n.id, end: Math.max(at.start + 1, n.anchor.start + 1), snap: true } : { id: n.id, start: Math.min(at.start, n.anchor.end - 1), snap: true })) this.select(n.id);
    };
    h.addEventListener("pointermove", move);
    h.addEventListener("pointerup", up);
  }

  // ---- the marks on the pages ----

  private async marks(): Promise<void> {
    const turn = ++this.drawn;
    if (!this.enabled) return void (this.p.scrollMarks?.([]), this.p.viewer.reviewMarks([]));
    // (the typeset comparison shown: its own marks, latexdiff's; the pages' origins are the diff's)
    if (this.showing) return void (this.p.scrollMarks?.([]), this.p.viewer.reviewMarks([]));
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
      const at = await this.host.boxes(n.anchor.file, n.anchor.start, Math.max(n.anchor.end, n.anchor.start + 1));
      if (turn !== this.drawn) return;
      at.forEach((p, j) => out.push({ ...p, cls: cls(n), id: n.id, ...(j === 0 && pin !== "" ? { pin, title } : {}) }));
    }
    for (const h of this.s.hunks) {
      if (h.whole === "deleted") continue;
      const empty = h.now[1] <= h.now[0];
      const at = await this.host.boxes(h.file, h.now[0], empty ? h.now[0] + 1 : h.now[1]);
      if (turn !== this.drawn) return;
      at.forEach((p) => out.push({ k: p.k, boxes: empty ? p.boxes.slice(0, 1).map(([x, y, , bh]) => [x, y, 2, bh] as Box) : p.boxes, cls: empty ? "h-del" : "h-add", id: h.id }));
    }
    void i;
    this.p.viewer.reviewMarks(out);
    this.lastOut = out;
    this.handles(out);
    this.outline();
    // (the list in the pages' order, where it changed)
    const where = new Map<string, [number, number]>();
    for (const m of out) if (!where.has(m.id)) where.set(m.id, [m.k, m.boxes[0]?.[1] ?? 0]);
    const moved = [...where].some(([id, w]) => this.where.get(id)?.join() !== w.join());
    this.where = where;
    this.scrollbar(where);
    if (moved && this.isOpen && this.tab === "notes") this.render();
  }

  /** A click on the pages: a pin or a mark selects its note or hunk; Alt+click notes a spot. */
  private async pageClick(e: MouseEvent): Promise<void> {
    if (!this.enabled) return;
    const t = e.target as HTMLElement;
    // (a handle's drag ends in a click: not a click on the page)
    if (t.closest(".rv-h")) return;
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
    const hit = [...slot.querySelectorAll<HTMLElement>(".rv:not(.rv-pin):not(.rv-h)")].reverse().find((m) => {
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
    if (!this.enabled) return this.closePopOnly();
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
      const r = await this.req("review_note", { kind, anchor });
      if (r) (this.clearSelection(), this.p.msg("Highlighted"));
      return;
    }
    this.openCompose(end.left, end.bottom + 6, { kind, anchor }, seen);
  }

  /** The box to write a note in, at (x, y). */
  private openCompose(x: number, y: number, note: { kind: ReviewNote["kind"]; anchor: object }, quote: string): void {
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
