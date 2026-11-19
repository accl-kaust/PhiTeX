// Changes: Compare and Blame as one, a popover anchored to the toolbar's
// ± Changes button (the panel's, or a host's own: Overleaf's toolbar). A
// click opens it, a second click, ✕ or Esc closes it; nothing else does (a
// pointer passing, a click on the pages beside it, where the work goes on).
// Opening it starts nothing: a diff is typeset once a pair is chosen, Blame
// worked out once its half is clicked; the history is read when asked for.
//
// - Diff: From → To, two identical fields; the body the list of changes,
//   grouped by place, with kind filters; the footer Hide deletions, n of m,
//   ‹ › (and ] [). Clicking From or To opens the history picker
//   (commits.ts) in place of the list: quick choices, then the graph.
// - Blame (blame.ts): no range, the paper as it is now: colour by Author
//   or Age, who wrote this version (share bars: click picks out a person's
//   lines), the commits that still own lines (click picks out a commit's).
// - The look (⚙, or its section under the list): the mode's settings, kept
//   as the viewer's other ones, also in the panel's Settings, Changes.
//
// Reading the history is never a frozen panel: rows shaped like the real
// ones while the first page comes, a progress line ("Reading history… 340
// of ~1,200 edits") with a real bar where the total is known, the list and
// the graph filling in as pages arrive, and a failed read says what failed
// with Try again. Each host gives its history and its diff (ChangesHost);
// the view is the same everywhere.

import { Commits, COMMITS_CSS, initials, mergedVia, type Commit } from "./commits.ts";
import { AGE_RAMPS, AUTHOR_COLORS, BLAME_LOOK, OVERLEAF_COLORS, UNCOMMITTED, agoWords, type Blame, type BlameCommit, type BlameLook } from "./blame.ts";
import type { DiffChange } from "./diff.ts";
import { html } from "./html.ts";

/** Two versions: `from` a commit (a version), `to` another, null the working tree (the editor's text). */
export interface Pair {
  from: string;
  to: string | null;
}

/** How the diff looks: colours, how added and deleted text are marked, a changed figure. */
export interface ChangesDiffLook {
  colors: "blue" | "green" | "safe" | "mono";
  ins: "wavy" | "line" | "mark" | "colour";
  del: "strike" | "mark" | "caret";
  fig: "side" | "over" | "new";
  /** Change bars in the margin (latexdiff's classic marks with them). */
  bars?: boolean;
}

/** The colour presets: added, deleted. */
export const DIFF_COLORS: Record<ChangesDiffLook["colors"], [string, string, string]> = {
  blue: ["#1347ae", "#bb1b1b", "Blue / red"],
  green: ["#1a7f37", "#cf222e", "Green / red"],
  safe: ["#0072B2", "#D55E00", "Blue / orange (colour-blind safe)"],
  mono: ["#111111", "#7a7a7a", "Black / grey (print)"],
};

export const DIFF_LOOK: ChangesDiffLook = { colors: "blue", ins: "wavy", del: "strike", fig: "side", bars: false };

/** What the popover keeps between visits (the viewer's settings). */
/** What ± Changes does, its button's title (the badge's meaning added when it shows a number). */
export const CHANGES_TITLE = "Changes: what changed between two versions, and who wrote what";

export interface ChangesPrefs {
  mode: "diff" | "blame";
  diff: ChangesDiffLook;
  blame: BlameLook;
  hide: boolean;
  kinds: string[];
  /** Folded to its title row and the Diff/Blame switch, the pages free, the marks on. */
  folded?: boolean;
}

/** A change as the list shows it: its kind and place, its text before and after, and who made it where the host can tell. */
export interface ChangeItem {
  /** Its index in the diff's changes (the runner's goto takes it, from 1). */
  k: number;
  kind: "Text" | "Section" | "Citation" | "Table" | "Figure" | "Equation";
  sec: string;
  old: string;
  neu: string;
  file: string;
  /** Where its new text is in its file, and its old text in the old one (UTF-8 bytes). */
  at?: number;
  end?: number;
  oldAt?: number;
  oldEnd?: number;
  who?: { name: string; color?: string; time?: number; short?: string; via?: string | null };
  /** Not committed (typed since the last commit or version). */
  wip?: boolean;
}

export const KINDS: ChangeItem["kind"][] = ["Text", "Section", "Citation", "Table", "Figure", "Equation"];

/** A change's words as the page shows them: its source without commands, braces and math marks. */
export function words(src: string, n = 12): string {
  return src.replace(/%.*$/gm, " ").replace(/\\(?:cite[tp]?|ref|label|eqref|includegraphics)\*?(\[[^\]]*\])?\{[^}]*\}/g, (m) => (/\\cite/.test(m) ? `[${m.replace(/.*\{|\}/g, "")}]` : " ")).replace(/\\[a-zA-Z@]+\*?/g, " ").replace(/[{}$\\%&~^_#[\]]/g, " ").split(/\s+/).filter(Boolean).slice(0, n).join(" ");
}

/** A diff's change as the list shows it. */
export function itemOf(c: DiffChange, k: number): ChangeItem {
  const both = `${c.old_text}\n${c.new_text}`;
  const kind: ChangeItem["kind"] = /\\(sub)*section\*?\{|\\chapter\*?\{|\\paragraph\{/.test(both)
    ? "Section"
    : /\\includegraphics|\\begin\{figure|\\begin\{tikzpicture/.test(both)
      ? "Figure"
      : /\\begin\{(tabular|table)|&.*\\\\/.test(both)
        ? "Table"
        : /\\(cite[a-z]*|bibitem)\b/.test(both) && words(c.new_text.replace(/\\cite[a-z]*\{[^}]*\}/g, "")) === words(c.old_text.replace(/\\cite[a-z]*\{[^}]*\}/g, ""))
          ? "Citation"
          : /\\begin\{(equation|align|gather|multline)|\$\$|\\\[/.test(both)
            ? "Equation"
            : "Text";
  const img = (t: string) => /\\includegraphics\*?(?:\[[^\]]*\])?\{([^}]*)\}/.exec(t)?.[1];
  let old = words(c.old_text), neu = words(c.new_text);
  if (kind === "Figure" && (img(c.old_text) || img(c.new_text))) (old = img(c.old_text) ?? old), (neu = img(c.new_text) ? `${img(c.new_text)}${img(c.old_text) ? " replaced" : ""}` : neu);
  if (kind === "Section") (old = old || ""), (neu = neu || "");
  return { k, kind, sec: c.section ?? c.new.file ?? c.old.file, old: c.kind === "add" ? "" : old, neu: c.kind === "del" ? "" : neu, file: c.new.file, at: c.new.start, end: c.new.end, oldAt: c.old.start, oldEnd: c.old.end };
}

/** What Changes needs of its host: the history, the diff, and where the user was last. */
export interface ChangesHost {
  /** History rows, newest first: `skip`.., at most `limit` (git's `diff_log`; Overleaf's edit sessions); or why there are none. */
  log(skip: number, limit: number): Promise<Commit[] | string>;
  /** The host's word for a commit and whether it has hashes (Overleaf: "version", none). */
  words?: { commit?: string; hashes?: boolean };
  /** The working tree's row in the graph; null: none. */
  tree?: { title: string; detail: string } | null;
  /** The version the user had when they last opened the project, if known. */
  lastVisit?(): string | null;
  /** The diff between two versions, typeset (its marks on the pages). Absent: no Diff. */
  diff?: ChangesRunner;
  /** Each change's author and time, where the host can tell, filled in as they are known (`done` once all are). */
  attribute?(items: ChangeItem[], pair: Pair, update: () => void, signal: AbortSignal): Promise<void>;
  /** An author's colour (avatars), where the host gives one. */
  color?(name: string, email: string): string | null;
  /** The history's own progress ("Reading history… 340 of ~1,200 edits"), as the host reads it: subscribed once. */
  onProgress?(f: (text: string, done?: number, total?: number) => void): void;
  /** A version's first parent where the graph has not read it yet. */
  firstParentOf?(id: string): string | null;
  /** The newest tag (Overleaf's label) where the graph may not have read it yet. */
  lastTag?(): Promise<{ id: string; name: string } | null>;
}

/** What the popover drives: a diff typeset on a session of its own, shown or not. */
export interface ChangesRunner {
  /** Diff `p`: its changes (the runner's own count as edits change them: `count`). */
  start(p: Pair, look: ChangesDiffLook, hide: boolean, say: (busy: string, done?: number, total?: number) => void, count: (changes: DiffChange[]) => void): Promise<DiffChange[]>;
  /** The diff's pages shown, or the current version's. */
  show(diff: boolean): void;
  /** Change `k` (from 1) shown. */
  goto(k: number): void;
  /** The look changed: diffed again. */
  restyle(look: ChangesDiffLook, hide: boolean): void;
  stop(): void;
  download?(what: "pdf" | "tex"): void;
}

/** The panel's part Changes lives in. */
export interface ChangesPanel {
  /** The window the popover goes in (its tokens colour it). */
  win: HTMLElement;
  root: ShadowRoot;
  /** The prefs kept, and keeping them. */
  prefs(): Partial<ChangesPrefs> | undefined;
  savePrefs(p: ChangesPrefs): void;
  msg(t: string, err?: boolean): void;
}

export const CHANGES_CSS = `
${COMMITS_CSS}
.chg-pop { --phc-bg: var(--ph-menu, var(--ph-bar)); --phc-fg: var(--ph-fg); --phc-muted: var(--ph-fg2); --phc-line: var(--ph-border);
  --phc-chip: var(--ph-hover); --phc-chip2: color-mix(in srgb, var(--ph-fg) 12%, var(--phc-bg)); --phc-focus: var(--ph-link, var(--ph-accent));
  --phc-sel: color-mix(in srgb, var(--phc-focus) 13%, transparent); --phc-aim: color-mix(in srgb, var(--phc-focus) 60%, transparent);
  --phc-danger: var(--ph-danger, #c23a2f);
  position: fixed; z-index: 24; /* (under the panel's dialogs (25) and menus (31), over its pages) */ width: min(var(--chg-w, 380px), calc(100vw - 16px)); max-height: min(680px, var(--chg-h, calc(100vh - 70px))); display: flex; flex-direction: column;
  background: var(--phc-bg); color: var(--phc-fg); border: 1px solid var(--phc-line); border-radius: 12px; box-shadow: 0 10px 30px rgb(15 25 40 / 22%);
  font: 13px/1.4 var(--ph-font, "Noto Sans", system-ui, sans-serif); transform-origin: top right; transition: opacity .15s, transform .15s; }
.chg-pop[hidden] { display: flex; opacity: 0; transform: scale(.97); pointer-events: none; visibility: hidden; }
.chg-pop::before { content: ""; position: absolute; top: -6px; right: var(--chg-arrow, 40px); width: 10px; height: 10px; background: var(--phc-bg); border-left: 1px solid var(--phc-line); border-top: 1px solid var(--phc-line); transform: rotate(45deg); }
.chg-pop > * { flex: none; }
.chg-pop > .body { flex: 1 1 auto; min-height: 96px; overflow: auto; overscroll-behavior: contain; }
.chg-pop { container: chgpop / inline-size; }
.chg-pop button { font: inherit; color: inherit; }
.chg-pop button:focus-visible, .chg-pop input:focus-visible, .chg-pop [tabindex]:focus-visible { outline: 2px solid var(--phc-focus); outline-offset: 1px; }
.chg-pop .ph { padding: 10px 12px 12px; display: grid; gap: 10px; border-bottom: 1px solid var(--phc-line); }
.chg-pop .ttl { display: grid; grid-template-columns: 28px 1fr 28px 28px; align-items: center; gap: 4px; }
.chg-pop .ttl h2 { margin: 0; font-size: 14px; text-align: center; grid-column: 2; }
.chg-pop .ttl .fold { grid-column: 1; }
/* (folded: its title row and the Diff/Blame switch only, the pages beside it free; the marks stay) */
.chg-pop.folded > :not(.ph):not(.grip) { display: none !important; }
.chg-pop.folded .ph > :not(.ttl):not(.seg) { display: none !important; }
.chg-pop.folded .ph { padding-bottom: 10px; border-bottom: 0; }
.chg-pop .ib2 { border: 0; background: transparent; color: var(--phc-muted); width: 28px; height: 28px; border-radius: 6px; cursor: pointer; font-size: 15px; display: grid; place-items: center; }
.chg-pop .ib2:hover, .chg-pop .ib2[aria-pressed="true"] { background: var(--phc-chip); color: var(--phc-fg); }
.chg-pop .seg { display: grid; grid-auto-flow: column; grid-auto-columns: 1fr; background: var(--phc-chip); border-radius: 8px; padding: 2px; }
.chg-pop .seg button { font-size: 12.5px; font-weight: 600; border: 0; background: transparent; color: var(--phc-muted); padding: 6px 6px; border-radius: 6px; cursor: pointer; transition: background .15s, color .15s; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.chg-pop .seg button[aria-pressed="true"] { background: var(--phc-bg); color: var(--phc-fg); box-shadow: 0 1px 2px rgb(0 0 0 / 18%); }
.chg-pop .seg.small button { font-weight: 500; font-size: 12px; padding: 4px 6px; }
.chg-pop .ends { display: grid; grid-template-columns: minmax(0, 1fr) 18px minmax(0, 1fr); align-items: end; gap: 6px; }
.chg-pop .ends .arrow { text-align: center; color: var(--phc-muted); padding-bottom: 14px; }
.chg-pop .end { display: grid; gap: 3px; min-width: 0; }
.chg-pop .lab { font-weight: 600; font-size: 10px; letter-spacing: .08em; text-transform: uppercase; color: var(--phc-muted); }
.chg-pop .field { display: flex; align-items: center; gap: 6px; border: 1px solid var(--phc-line); background: var(--phc-chip); border-radius: 7px; padding: 5px 6px 5px 8px; min-width: 0; cursor: pointer; height: 46px; transition: border-color .15s, box-shadow .15s; text-align: left; }
.chg-pop .field:hover { border-color: var(--phc-focus); }
.chg-pop .field.picking { border-color: var(--phc-focus); box-shadow: 0 0 0 2px var(--phc-sel); }
.chg-pop .field .tx { display: grid; min-width: 0; flex: 1; }
.chg-pop .field .tx b { font-weight: 500; font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.chg-pop .field .tx code { font: 500 11px ui-monospace, "DM Mono", monospace; color: var(--phc-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.chg-pop .field.flash .tx { animation: chg-flash .25s ease-out; }
@keyframes chg-flash { from { opacity: .2; transform: translateY(3px); } }
.chg-pop .field .clr { border: 0; background: transparent; color: var(--phc-muted); width: 20px; height: 20px; border-radius: 4px; cursor: pointer; flex: none; }
.chg-pop .field .clr:hover { background: var(--phc-chip2); color: var(--phc-fg); }
.chg-pop .field .chev { color: var(--phc-muted); font-size: 11px; flex: none; }
.chg-pop .field .ph2 { color: var(--phc-muted); font-weight: 500; }
.chg-pop .sub { font-size: 12px; color: var(--phc-muted); margin: 0; text-align: center; }
/* (the progress line floats over the bottom of the list: the rows under the pointer never move as it comes and goes) */
.chg-pop .prog { position: absolute; z-index: 3; left: 8px; right: 8px; bottom: 8px; padding: 6px 10px 7px; border: 1px solid var(--phc-line); border-radius: 8px; background: var(--phc-bg); box-shadow: 0 4px 14px rgb(15 25 40 / 16%); font-size: 12px; color: var(--phc-muted); display: grid; gap: 5px; }
.chg-pop:has(> .foot) .prog { bottom: 44px; }
.chg-pop .prog:not(.err) { pointer-events: none; }
.chg-pop:has(> .prog:not([hidden]):not(.err)) > .body { padding-bottom: 48px; }
.chg-pop .prog[hidden] { display: none; }
.chg-pop .track { height: 3px; border-radius: 2px; background: var(--phc-chip2); overflow: hidden; position: relative; }
.chg-pop .track i { position: absolute; left: 0; top: 0; bottom: 0; background: var(--ph-progress, var(--phc-focus)); border-radius: 2px; transition: width .2s; }
.chg-pop .track.ind i { width: 30%; animation: chg-ind 1.2s ease-in-out infinite; }
@keyframes chg-ind { from { left: -30%; } to { left: 100%; } }
/* (an error stays in the flow: it waits for Try again, its words never over the rows) */
.chg-pop .prog.err { color: var(--phc-danger); position: static; order: 0; margin: 8px 8px 0; box-shadow: none; }
.chg-pop .prog .again { justify-self: start; border: 1px solid var(--phc-line); background: var(--phc-bg); border-radius: 999px; padding: 1px 10px; cursor: pointer; font-size: 12px; color: var(--phc-fg); }
.chg-pop .foot { border-top: 1px solid var(--phc-line); padding: 6px 10px; height: 42px; box-sizing: border-box; display: grid; grid-template-columns: 1fr auto 1fr; gap: 8px; align-items: center; font-size: 12px; color: var(--phc-muted); }
.chg-pop .foot .count { min-width: 76px; text-align: center; font-variant-numeric: tabular-nums; white-space: nowrap; }
.chg-pop .seg.tiny { justify-self: start; }
.chg-pop .seg.tiny button { font-size: 11.5px; font-weight: 500; padding: 3px 8px; }
.chg-pop .seg button:disabled { opacity: .45; cursor: default; }
.chg-pop .primary { display: block; margin: 12px auto 0; border: 0; background: var(--phc-focus); color: #fff !important; border-radius: 7px; padding: 6px 14px; font-weight: 600; font-size: 12.5px; cursor: pointer; }
.chg-pop .primary:hover { filter: brightness(1.08); }
.chg-pop .lookbox { border-top: 1px solid var(--phc-line); display: flex; flex-direction: column; min-height: 0; }
/* (open: the look keeps its height, up to 55% of the popover, scrolling inside; the list above gives way, down to its own least) */
.chg-pop .lookbox.open { max-height: calc(min(680px, var(--chg-h, calc(100vh - 70px))) * .55); flex: 0 0 auto; }
.chg-pop .lookhead { display: flex; align-items: center; gap: 6px; width: 100%; border: 0; background: transparent; padding: 7px 12px; font-size: 11px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--phc-muted); cursor: pointer; text-align: left; flex: none; }
.chg-pop .lookhead:hover { color: var(--phc-fg); background: var(--phc-chip); }
.chg-pop .chev2 { display: inline-block; transition: transform .15s; font-size: 10px; }
.chg-pop .lookbox.open .chev2 { transform: rotate(90deg); }
.chg-pop .lookin { overflow: auto; min-height: 0; }
.chg-pop .lookin[hidden] { display: none; }
.chg-pop .dls { display: flex; gap: 10px; }
.chg-pop .lnk { border: 0; background: transparent; color: var(--phc-focus); padding: 0; cursor: pointer; font-size: 12.5px; text-decoration: underline; text-underline-offset: 2px; }
.chg-pop .lnk:disabled { color: var(--phc-muted); text-decoration: none; cursor: default; }
.chg-pop .filters .hd { margin-left: auto; font-size: 12px; color: var(--phc-muted); white-space: nowrap; flex: none; }
.chg-pop .preview.bars { box-shadow: inset 3px 0 0 var(--chg-add); padding-left: 14px; }
.chg-pop .foot .r { justify-self: end; }
.chg-pop .nav { display: inline-flex; gap: 2px; align-items: center; }
.chg-pop .nav button { border: 0; background: transparent; width: 28px; height: 26px; border-radius: 6px; cursor: pointer; font-size: 15px; color: var(--phc-fg); }
.chg-pop .nav button:hover { background: var(--phc-chip); }
.chg-pop .nav button:disabled { opacity: .35; cursor: default; }
.chg-pop .toggle { display: inline-flex; gap: 6px; align-items: center; cursor: pointer; }
.chg-pop .pickhead { display: grid; grid-template-columns: 1fr auto; align-items: center; gap: 8px; padding: 10px 12px 6px; }
.chg-pop .pickhead b { font-size: 13px; }
.chg-pop .done { border: 1px solid var(--phc-line); background: var(--phc-chip); border-radius: 7px; padding: 4px 12px; cursor: pointer; font-size: 12.5px; font-weight: 600; }
.chg-pop .quick { display: flex; gap: 6px; flex-wrap: wrap; padding: 2px 12px 10px; border-bottom: 1px solid var(--phc-line); }
.chg-pop .qb { font-size: 12px; border: 1px solid var(--phc-line); background: var(--phc-bg); border-radius: 999px; padding: 3px 10px; cursor: pointer; }
.chg-pop .qb:hover { border-color: var(--phc-focus); }
.chg-pop .hint { padding: 8px 12px; font-size: 12px; color: var(--phc-muted); border-bottom: 1px solid var(--phc-line); min-height: 34px; }
.chg-pop .hint b { color: var(--phc-fg); font-weight: 600; }
.chg-pop .filters { display: flex; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--phc-line); overflow-x: auto; scrollbar-width: none; align-items: center; }
.chg-pop .fchip { font-size: 12px; border: 1px solid var(--phc-line); background: var(--phc-bg); border-radius: 999px; padding: 2px 9px; cursor: pointer; display: inline-flex; gap: 5px; align-items: center; white-space: nowrap; flex: none; transition: opacity .15s; }
.chg-pop .fchip[aria-pressed="false"] { opacity: .45; }
.chg-pop .fchip small { color: var(--phc-muted); font-size: inherit; }
.chg-pop .grp { padding: 10px 12px 2px; font-size: 11px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--phc-muted); display: flex; justify-content: space-between; gap: 8px; }
.chg-pop .grp span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.chg-pop .citem { display: grid; grid-template-columns: 60px minmax(0, 1fr); gap: 2px 8px; width: 100%; text-align: left; border: 0; border-left: 3px solid transparent; background: transparent; padding: 7px 12px 7px 9px; cursor: pointer; transition: background .15s; }
.chg-pop .citem.new { animation: phc-in .2s ease-out both; }
.chg-pop .citem:hover { background: var(--phc-chip); }
.chg-pop .citem.cur { background: var(--phc-sel); border-left-color: var(--phc-focus); }
.chg-pop .citem .k { font-weight: 600; font-size: 10px; text-transform: uppercase; letter-spacing: .04em; color: var(--phc-muted); padding-top: 3px; }
.chg-pop .citem .w { min-width: 0; overflow-wrap: anywhere; }
.chg-pop .citem .w small { color: var(--phc-muted); display: flex; gap: 5px; align-items: center; flex-wrap: wrap; font-size: 11.5px; }
.chg-pop .citem del { color: var(--chg-del, #bb1b1b); }
.chg-pop .citem ins { color: var(--chg-add, #1347ae); text-decoration: none; }
.chg-pop .hidedel .citem del { display: none; }
.chg-pop .av { width: 16px; height: 16px; border-radius: 50%; display: inline-grid; place-items: center; font: 700 8px system-ui, sans-serif; color: #fff; flex: none; }
.chg-pop .av.sm { width: 13px; height: 13px; font-size: 6px; }
.chg-pop .refp { font: 600 10.5px ui-monospace, monospace; border-radius: 5px; padding: 0 5px; border: 1px solid currentColor; color: var(--phc-g1, #c2410c); }
.chg-pop .empty { padding: 22px 14px; color: var(--phc-muted); text-align: center; }
.chg-pop .empty b { display: block; color: var(--phc-fg); font-weight: 600; margin-bottom: 4px; }
.chg-pop .skrow { display: grid; grid-template-columns: 60px 1fr; gap: 8px; padding: 9px 12px; }
.chg-pop .bone { display: block; height: 9px; border-radius: 4px; background: var(--phc-chip2); margin: 3px 0; animation: phc-shimmer 1.2s ease-in-out infinite; }
.chg-pop .sec { padding: 10px 12px; border-bottom: 1px solid var(--phc-line); display: grid; gap: 8px; }
.chg-pop .sec:last-child { border-bottom: 0; }
.chg-pop .sec h3 { margin: 0; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: var(--phc-muted); display: flex; justify-content: space-between; }
.chg-pop .prow, .chg-pop .crow { display: grid; gap: 8px; align-items: center; border: 0; background: transparent; text-align: left; padding: 5px 4px; border-radius: 6px; cursor: pointer; grid-template-columns: auto minmax(0, 1fr) auto; width: 100%; transition: background .15s; }
.chg-pop .prow:hover, .chg-pop .crow:hover { background: var(--phc-chip); }
.chg-pop .prow.sel, .chg-pop .crow.sel { background: var(--phc-sel); }
.chg-pop .prow .nm { display: grid; gap: 3px; min-width: 0; font-size: 13px; }
.chg-pop .bar6 { height: 6px; border-radius: 3px; background: var(--phc-chip2); overflow: hidden; }
.chg-pop .bar6 i { display: block; height: 100%; transition: width .25s; }
.chg-pop .ct { font: 500 12px ui-monospace, monospace; color: var(--phc-muted); font-variant-numeric: tabular-nums; }
.chg-pop .crow .s { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.chg-pop .crow small { color: var(--phc-muted); font-size: 11.5px; }
.chg-pop .crow code { font: 500 11px ui-monospace, monospace; }
.chg-pop .sw { width: 4px; height: 28px; border-radius: 2px; }
.chg-pop .sw.hatch { background: repeating-linear-gradient(0deg, #99a3ad 0 3px, transparent 3px 6px) !important; }
.chg-pop .ramp { display: flex; gap: 4px; align-items: center; font-size: 11px; color: var(--phc-muted); justify-content: center; }
.chg-pop .ramp i { width: 26px; height: 8px; border-radius: 2px; display: inline-block; }
.chg-pop .set { padding: 2px 12px 12px; display: grid; gap: 10px; }
.chg-pop .set h3 { margin: 10px 0 0; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; color: var(--phc-muted); }
.chg-pop .opt2 { display: grid; grid-template-columns: 108px minmax(0, 1fr); gap: 8px; align-items: center; font-size: 12.5px; }
.chg-pop .note2 { margin: -4px 0 0 116px; font-size: 12px; color: var(--phc-muted); }
/* (the look's choices sized by their words, the spare width shared: "Underline", "Side by side" whole) */
.chg-pop .opt2 .seg { grid-auto-columns: auto; }
@container chgpop (max-width: 330px) { .chg-pop .opt2 { grid-template-columns: 1fr; gap: 4px; } .chg-pop .note2 { margin-left: 0; } }
.chg-pop .swatches { display: flex; gap: 6px; flex-wrap: wrap; }
.chg-pop .swb { border: 1px solid var(--phc-line); background: var(--phc-bg); border-radius: 8px; padding: 4px 8px; cursor: pointer; display: inline-flex; gap: 6px; align-items: center; font-size: 12px; }
.chg-pop .swb[aria-pressed="true"] { border-color: var(--phc-focus); box-shadow: 0 0 0 2px var(--phc-sel); }
.chg-pop .swb i { width: 12px; height: 12px; border-radius: 3px; display: inline-block; }
.chg-pop .skips { display: grid; gap: 4px; font-size: 12px; max-height: 150px; overflow: auto; }
.chg-pop .skips code { font: 500 11px ui-monospace, monospace; color: var(--phc-muted); }
.chg-pop .skips small { color: var(--phc-muted); }
.chg-pop .skips label { display: flex; gap: 6px; align-items: baseline; cursor: pointer; }
/* (the list across the whole width, under its label; a long row wraps, its whole text in its title too) */
.chg-pop .opt2.wide { grid-template-columns: 1fr; align-items: start; gap: 4px; }
.chg-pop .skips label span { overflow-wrap: anywhere; }
.chg-pop .preview { border: 1px solid var(--phc-line); border-radius: 8px; padding: 10px 12px; background: #fff; color: #111; font: 15px/1.5 "STIX Two Text", "Latin Modern Roman", "Times New Roman", serif; }
.chg-pop .preview .ins { color: var(--chg-add); }
.chg-pop .preview .dl { color: var(--chg-del); }
.chg-pop .preview.i-wavy .ins { text-decoration: underline wavy 1px; text-underline-offset: 3px; }
.chg-pop .preview.i-line .ins { text-decoration: underline 1.5px; text-underline-offset: 3px; }
.chg-pop .preview.i-mark .ins { background: color-mix(in srgb, var(--chg-add) 16%, transparent); border-radius: 2px; }
.chg-pop .preview.d-strike .dl { text-decoration: line-through; }
.chg-pop .preview.d-mark .dl { text-decoration: line-through; background: color-mix(in srgb, var(--chg-del) 14%, transparent); border-radius: 2px; }
.chg-pop .preview.d-caret .dl { font-size: 0; }
.chg-pop .preview.d-caret .dl::after { content: "‸"; font-size: 15px; color: var(--chg-del); font-weight: 700; }
.chg-pop .reset { justify-self: center; border: 0; background: transparent; color: var(--phc-focus); cursor: pointer; font-size: 12px; }
.chg-pop .grip { position: absolute; left: 0; top: 0; bottom: 0; width: 6px; cursor: col-resize; z-index: 3; }
@media (prefers-reduced-motion: reduce) { .chg-pop, .chg-pop * { transition: none !important; animation: none !important; } }
`;

/** The ± Changes button's count (in the panel's bar, or a host's toolbar). */
export const CHANGES_BUTTON_CSS = `
.chg-badge { font: 600 11px ui-monospace, monospace; background: var(--ph-link, var(--ph-accent)); color: #fff; border-radius: 999px; padding: 0 6px; line-height: 16px; display: inline-block; min-width: 8px; text-align: center; }
.chg-badge:empty { display: none; }
.chg-badge.tick { animation: chg-tick .2s ease-out; }
@keyframes chg-tick { from { transform: scale(1.35); } }
@media (prefers-reduced-motion: reduce) { .chg-badge.tick { animation: none; } }
`;

/** Okabe–Ito by a name's hash (an avatar's colour where the host gives none). */
function hashColor(key: string): string {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(key.toLowerCase())) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return AUTHOR_COLORS[h % AUTHOR_COLORS.length];
}

const reduced = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The ± Changes popover. */
export class Changes {
  private p: ChangesPanel;
  private host: ChangesHost;
  private blame: Blame | null;
  readonly el: HTMLElement;
  /** The popover open (its button pressed): its mode's marks on the pages while it is. */
  open = false;
  mode: "diff" | "blame";
  private look: ChangesDiffLook;
  hide: boolean;
  private kinds: Set<string>;
  /** The look's section (under the list) open. */
  private lookOpen = false;
  /** Folded to its title row (kept): the pages it marks free to read. */
  private folded = false;
  private foldScroll = 0;
  /** The picker shown, opened from which field. */
  private pick: "from" | "to" | null = null;
  private pair: Pair | null = null;
  /** The pair whose diff is shown (its changes), and the diff's state. */
  private ran: Pair | null = null;
  private items: ChangeItem[] = [];
  private busy = "";
  private prog: [number, number] | undefined;
  private error = "";
  private cur = -1;
  /** Which pages the diff shows: its marked-up document's, or the current version's (D). */
  private pages: "diff" | "current" = "diff";
  /** Blame asked for (its half clicked) in this visit: worked out again when the popover opens. */
  private blameAsked = false;
  private graph: Commits | null = null;
  private anchors: HTMLElement[] = [];
  private anchor: HTMLElement | null = null;
  private badges: HTMLElement[] = [];
  private attrAbort: AbortController | undefined;
  private histProgress: { text: string; done?: number; total?: number } | null = null;
  private seenItems = new Set<string>();
  private lastCount = -1;
  private width: number | null = null;
  private blameSig = "";

  constructor(p: ChangesPanel, host: ChangesHost, blame: Blame | null) {
    this.p = p;
    this.host = host;
    this.blame = blame;
    this.mode = host.diff ? "diff" : "blame";
    this.look = { ...DIFF_LOOK };
    this.hide = false;
    this.kinds = new Set(KINDS);
    // (in a shadow root of its own inside the panel's window: the window's tokens (--ph-*) reach it,
    // its rules (.body, .row, .seg, …) do not)
    const box = document.createElement("div");
    box.id = "chg";
    const shadow = box.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = CHANGES_CSS;
    this.el = document.createElement("div");
    this.el.className = "chg-pop";
    this.el.hidden = true;
    this.el.setAttribute("role", "dialog");
    this.el.setAttribute("aria-label", "Changes");
    shadow.append(style, this.el);
    p.win.append(box);
    // (Blame's state: its progress line at once; the lists drawn again only when what they show changed,
    // in place: the head, the look and the scroll left as they are)
    blame?.onState(() => {
      if (this.mode === "blame") this.badge();
      if (this.mode !== "blame" || !this.open) return;
      this.renderProg();
      const s = blame.summary();
      const sig = JSON.stringify([this.blameAsked, s.lines, s.unit, !!s.busy && !s.lines, !!s.error && !s.lines, s.waiting, s.files, s.people.map((p) => [p.key, p.n, p.color]), s.commits.map((c) => [c.c, c.n, c.color, c.via]), blame.focusP, blame.focusC, blame.look.color, blame.look.ages]);
      if (sig === this.blameSig) return;
      this.blameSig = sig;
      const focus = this.focusKey();
      this.renderBody();
      this.refocus(focus);
    });
    host.onProgress?.((text, done, total) => {
      this.histProgress = text ? { text, done, total } : null;
      if (this.open) this.renderProg();
    });
    // (Esc: a first end, the picker, then closed; ] [ and N, Shift+N step through the changes; D: the diff's pages or the current ones)
    p.root.addEventListener("keydown", (e) => this.key(e as KeyboardEvent));
    document.addEventListener("keydown", (e) => {
      if (e.composedPath().includes(p.win)) return;
      this.key(e);
    });
    // (the pane resized: placed again, within it)
    addEventListener("resize", () => this.open && this.place());
    // (the browser tab shown again: what was "not saved yet" may be saved now)
    document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && this.historyMoved());
    if (typeof ResizeObserver === "function") new ResizeObserver(() => this.open && this.place()).observe(p.win);
    this.applyColors();
    this.kept(p.prefs());
  }

  /** The settings as kept (the panel's prefs, read once loaded). */
  kept(k: Partial<ChangesPrefs> | undefined): void {
    if (!k) return;
    if (k.mode) this.mode = k.mode === "blame" && this.blame ? "blame" : this.host.diff ? "diff" : "blame";
    this.look = { ...DIFF_LOOK, ...k.diff };
    this.hide = !!k.hide;
    this.folded = !!k.folded;
    this.kinds = new Set(k.kinds?.length ? k.kinds : KINDS);
    if (this.blame && k.blame) this.blame.look = { ...BLAME_LOOK, ...k.blame, skip: [...(k.blame.skip ?? [])] };
    this.applyColors();
    if (this.open) this.render();
  }

  /** The look kept. */
  private save(): void {
    this.p.savePrefs({ mode: this.mode, diff: this.look, blame: this.blame?.look ?? BLAME_LOOK, hide: this.hide, kinds: [...this.kinds], folded: this.folded });
  }

  /**
   * A ± Changes button (the panel's, a host's own): a click opens the
   * popover, a second closes it; `badge` the count of changes beside it.
   * Pointing at it does nothing but say what it is.
   */
  attach(btn: HTMLElement, badge?: HTMLElement): void {
    // (a host's toolbar drawn again (Overleaf's, by React): its new button takes the old one's place, as it was)
    const was = this.anchor && !this.anchor.isConnected;
    this.anchors = [...this.anchors.filter((a) => a.isConnected), btn];
    this.badges = [...this.badges.filter((b) => b.isConnected), ...(badge ? [badge] : [])];
    btn.setAttribute("aria-haspopup", "dialog");
    btn.setAttribute("aria-expanded", String(this.open));
    btn.setAttribute("aria-pressed", String(this.open));
    btn.classList.toggle("on", this.open);
    btn.classList.toggle("active", this.open);
    if (was) {
      this.anchor = btn;
      if (this.open) this.place();
    }
    this.badge();
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      this.setOpen(!this.open, btn);
    });
  }

  /**
   * The popover opened or closed: by its button, ✕ or Esc, nothing else (a
   * pointer moving, a click on the pages beside it). Opened, it shows what is
   * known already and starts nothing: a diff runs once a pair is chosen, Blame
   * once its half is clicked. Its marks are on the pages while it is open.
   */
  setOpen(v: boolean, at?: HTMLElement): void {
    if (at) this.anchor = at;
    if (v === this.open) return;
    this.open = v;
    this.el.hidden = !v;
    for (const a of this.anchors) {
      a.setAttribute("aria-pressed", String(v));
      a.setAttribute("aria-expanded", String(v));
      a.classList.toggle("on", v);
      a.classList.toggle("active", v);
    }
    if (v) {
      this.render();
      this.place();
      this.marks();
      this.historyMoved();
      // (the keyboard's place: the popover's first control)
      if (at && document.activeElement === at) this.el.querySelector<HTMLElement>("[data-m][aria-pressed=true], button")?.focus({ preventScroll: true });
    } else {
      this.pick = null;
      this.graph?.cancel();
      this.marks();
      // (the keyboard back on the button that opened it)
      if (this.el.contains(this.el.getRootNode() instanceof ShadowRoot ? (this.el.getRootNode() as ShadowRoot).activeElement : null)) (this.anchor ?? this.anchors[0])?.focus({ preventScroll: true });
    }
  }

  /**
   * Folded to its title row and the Diff/Blame switch (`on`), or unfolded;
   * kept. The marks and Blame's card stay live; Esc and ✕ still close it.
   */
  fold(on: boolean): void {
    if (on === this.folded) return;
    this.folded = on;
    this.save();
    if (!this.open) return;
    // (in place: the lists and the focus left as they are, only hidden; their scroll put back)
    const body = this.el.querySelector<HTMLElement>(".body");
    if (on && body) this.foldScroll = body.scrollTop;
    this.el.classList.toggle("folded", on);
    if (!on && body) body.scrollTop = this.foldScroll;
    const b = this.el.querySelector<HTMLElement>("#chg-fold");
    if (b) {
      b.textContent = on ? "▸" : "▾";
      b.setAttribute("aria-expanded", String(!on));
      b.setAttribute("aria-label", on ? "Unfold Changes" : "Fold Changes");
      b.title = on ? "Unfold: the list and its controls again" : "Fold to this row: the pages beside it free to read, the marks kept";
    }
    this.place();
    if (!on) this.historyMoved();
  }

  /** Blame shown (opened, unfolded, its tab, the browser tab back): the history asked whether it moved since, once (no polling). */
  private historyMoved(): void {
    if (this.open && this.mode === "blame" && this.blameAsked) this.blame?.refreshHistory();
  }

  /** The popover's box while open (Blame's card keeps clear of it), else null. */
  box(): DOMRect | null {
    return this.open ? this.el.getBoundingClientRect() : null;
  }

  /** The diff's pages asked for last (the host's pages start as the current version's). */
  private diffShown = false;

  /** Closed: ✕, Esc, a second click on the button. */
  off(): void {
    this.setOpen(false);
  }

  /** The marks of the mode shown on the pages while open (only what was asked for: a diff run, Blame clicked), else none. */
  private marks(): void {
    const diff = this.open && this.mode === "diff" && !!this.ran;
    const blame = this.open && this.mode === "blame" && this.blameAsked;
    // (the pages back to the editor's before Blame marks them; asked only when that changes: opened with
    // no diff, nothing is asked of the host)
    const pages = diff && this.pages === "diff";
    if (pages !== this.diffShown) (this.diffShown = pages), this.host.diff?.show(pages);
    this.blame?.toggle(blame);
  }

  /** Under its button, within the pane (never over the button): as wide as the pane allows, its height the pane's below the button at most, scrolled inside. */
  private place(): void {
    const a = this.anchor ?? this.anchors.find((x) => x.isConnected) ?? this.anchors[0];
    if (!a) return;
    const r = a.getBoundingClientRect();
    const pane = this.p.win.getBoundingClientRect();
    const lo = Math.max(0, pane.left), hi = Math.min(innerWidth, pane.right > lo ? pane.right : innerWidth);
    const w = Math.max(260, Math.min(this.width ?? 380, hi - lo - 16));
    this.el.style.setProperty("--chg-w", `${w}px`);
    const left = Math.max(lo + 8, Math.min(hi - w - 8, r.right - w + 10));
    const top = r.bottom + 8;
    const bottom = Math.min(innerHeight, pane.bottom > top ? pane.bottom : innerHeight);
    this.el.style.setProperty("--chg-h", `${Math.max(220, bottom - top - 8)}px`);
    this.el.style.left = `${left}px`;
    this.el.style.top = `${top}px`;
    this.el.style.setProperty("--chg-arrow", `${Math.max(14, Math.min(w - 24, left + w - (r.left + r.width / 2) - 5))}px`);
    // (a fixed box is placed in its containing block, which a host's transform makes its pane: moved by where it landed)
    const at = this.el.getBoundingClientRect();
    if (at.width && (Math.abs(at.left - left) > 0.5 || Math.abs(at.top - top) > 0.5)) {
      this.el.style.left = `${2 * left - at.left}px`;
      this.el.style.top = `${2 * top - at.top}px`;
    }
  }

  private key(e: KeyboardEvent): void {
    if (!this.open || e.defaultPrevented) return;
    const t = e.composedPath()[0] as HTMLElement | undefined;
    if (t?.closest?.("input:not([type=checkbox]):not([type=radio]), textarea, select, [contenteditable=true], .cm-editor")) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const done = () => (e.preventDefault(), e.stopPropagation());
    if (e.key === "Escape") {
      // (a dialog of the panel's open over it, Settings or Keys: Esc is that dialog's)
      if (this.p.root.querySelector(".dialog:not([hidden])")) return;
      if (this.pick && this.graph?.aiming) (this.graph.cancel(), this.refreshHint());
      else if (this.pick) (this.pick = null), this.render(), this.el.querySelector<HTMLElement>("#chg-from")?.focus();
      else this.off();
      done();
    } else if (this.mode === "diff" && (e.key === "]" || e.key === "[" || e.key === "n" || e.key === "N")) {
      this.step(e.key === "]" || e.key === "n" ? 1 : -1);
      done();
    } else if (this.mode === "diff" && (e.key === "d" || e.key === "D") && this.ran) {
      this.showPages(this.pages === "diff" ? "current" : "diff");
      done();
    }
  }

  // ---- the diff ----

  /** The changes the filters let through. */
  private shown(): ChangeItem[] {
    return this.items.filter((c) => this.kinds.has(c.kind));
  }

  private farKnown: { id: string; name: string } | null = null;
  private far: Promise<{ id: string; name: string } | null> | null = null;

  /** The host's newest tag, older than the graph has read (a quick choice at once; the picker asked for it). */
  private farTag(): Promise<{ id: string; name: string } | null> {
    this.far ??= (this.host.lastTag?.() ?? Promise.resolve(null))
      .then((t) => {
        if (t) this.labels.set(t.id, t.name);
        this.farKnown = t;
        if (this.open && this.pick) this.render();
        return t;
      })
      .catch(() => null);
    return this.far;
  }

  private ensureGraph(): Commits {
    if (this.graph) return this.graph;
    const holder = document.createElement("div");
    this.graph = new Commits(holder, {
      load: (skip, limit) => this.host.log(skip, limit),
      pick: (from, to, done) => this.choose({ from, to }, done),
      marked: () => this.renderHead(),
      color: (c) => this.host.color?.(c.author, c.email ?? "") ?? hashColor(c.email || c.author),
      tree: this.host.tree,
      words: this.host.words,
    });
    this.graph.onChange(() => {
      if (this.pair) this.graph!.mark(this.pair.from, this.pair.to);
      if (this.open) this.renderHead();
      if (this.open && this.pick) this.refreshQuick();
    });
    return this.graph;
  }

  /** The picker opened from From or To (the history read from here: asked for). */
  private openPicker(which: "from" | "to"): void {
    this.pick = this.pick === which ? null : which;
    const g = this.ensureGraph();
    if (this.pick) g.open(which);
    this.render();
    this.el.querySelector<HTMLElement>(".phx-commits .row[data-i], .qb, #chg-done")?.focus({ preventScroll: true });
  }

  /** A pair chosen: the diff run; `done` closes the picker. */
  private choose(p: Pair, done: boolean): void {
    this.pair = p;
    this.graph?.mark(p.from, p.to);
    if (done) this.pick = null;
    this.flashFields();
    void this.run(p);
    this.render();
  }

  private turn = 0;

  private async run(p: Pair): Promise<void> {
    const r = this.host.diff;
    if (!r) return;
    const turn = ++this.turn;
    this.attrAbort?.abort();
    this.ran = p;
    this.items = [];
    this.cur = -1;
    this.error = "";
    this.busy = "Reading the two versions…";
    this.prog = undefined;
    this.pages = "diff";
    // (its pages in view once typeset, while the popover is open on Diff)
    this.diffShown = this.open && this.mode === "diff";
    r.show(this.diffShown);
    this.render();
    if (p.to !== null && p.from === p.to) {
      this.busy = "";
      return this.render();
    }
    try {
      const got = await r.start(
        p,
        this.look,
        this.hide,
        (busy, done, total) => {
          if (turn !== this.turn) return;
          this.busy = busy;
          this.prog = done !== undefined && total ? [done, total] : undefined;
          this.renderProg();
        },
        (changes) => {
          if (turn !== this.turn) return;
          this.setItems(changes);
        },
      );
      if (turn !== this.turn) return;
      this.busy = "";
      this.setItems(got);
      this.marks();
    } catch (e) {
      if (turn !== this.turn) return;
      this.busy = "";
      this.error = `Couldn't compare: ${(e as Error).message ?? e}`;
      this.render();
    }
  }

  /** The diff's changes as the list's items, their authors filled in as the host knows them. */
  private setItems(changes: DiffChange[]): void {
    const old = new Map(this.items.map((i) => [`${i.file}:${i.old}:${i.neu}`, i]));
    this.items = changes.map((c, i) => {
      const it = itemOf(c, i + 1);
      const was = old.get(`${it.file}:${it.old}:${it.neu}`);
      if (was) (it.who = was.who), (it.wip = was.wip);
      return it;
    });
    this.cur = Math.min(this.cur, this.shown().length - 1);
    this.render();
    const p = this.ran;
    if (p && this.host.attribute && this.items.length) {
      this.attrAbort?.abort();
      const a = (this.attrAbort = new AbortController());
      // (the list's rows filled in as their authors are known: only the list, the picker left as it is)
      void this.host.attribute(this.items, p, () => !a.signal.aborted && this.open && this.mode === "diff" && !this.pick && this.renderBody(), a.signal).catch(() => undefined);
    }
  }

  /** Change `d` steps on (‹ ›, ] [, N Shift+N): the diff's pages shown at it. */
  step(d: number): void {
    const v = this.shown();
    if (!v.length) return;
    this.cur = this.cur < 0 ? (d > 0 ? 0 : v.length - 1) : (this.cur + d + v.length) % v.length;
    this.pick = null;
    if (this.host.diff && this.ran) {
      if (this.pages !== "diff") this.showPages("diff");
      this.host.diff.goto(v[this.cur].k);
    }
    this.render();
    this.el.querySelector(".citem.cur")?.scrollIntoView({ block: "nearest", behavior: reduced() ? "auto" : "smooth" });
  }

  /** The diff's pages or the current version's (the diff kept). */
  private showPages(w: "diff" | "current"): void {
    this.pages = w;
    this.marks();
    this.render();
  }

  /** Ends the history does not list, named by the host ("What git has staged", "Proposal 3"). */
  private labels = new Map<string, string>();

  /** Diff `p` shown, the popover open (the host's own ask: a review's changes), its ends named by `names` where the history does not list them. */
  show(p: Pair, names: { from?: string; to?: string } = {}): void {
    if (names.from) this.labels.set(p.from, names.from);
    if (names.to && p.to) this.labels.set(p.to, names.to);
    this.mode = "diff";
    this.setOpen(true);
    this.choose(p, true);
    this.marks();
  }

  /** What `c` changed: Diff against its first parent (Blame's card). */
  diffCommit(c: BlameCommit | null): void {
    const g = this.ensureGraph();
    this.mode = "diff";
    const id = c ? (c.rev ?? c.id) : null;
    const at = id ? g.find(id) : -1;
    const parent = c && id ? (c.parent ?? (at >= 0 ? g.commits[at].parents[0] : undefined) ?? this.host.firstParentOf?.(id) ?? null) : null;
    this.save();
    this.setOpen(true);
    if (c && id && parent) {
      this.choose({ from: parent, to: id }, true);
      // (an older one than the graph has read: read on to it, the fields then naming both ends)
      if (at < 0)
        void (async () => {
          while (g.find(id) < 0 && g.hasMore && this.pair?.to === id) await g.load();
          this.renderHead();
        })();
    }
    this.marks();
    this.render();
  }

  /** The diff's colours on the list and the preview. */
  private applyColors(): void {
    const [a, d] = DIFF_COLORS[this.look.colors] ?? DIFF_COLORS.blue;
    this.el.style.setProperty("--chg-add", a);
    this.el.style.setProperty("--chg-del", d);
  }

  /** The words of a version: its subject (a tag's name) and its short hash. */
  private name(id: string | null): { b: string; k: string } {
    if (id === null) return { b: this.host.tree?.title ?? "Working tree", k: "as it is now" };
    const g = this.graph;
    const i = g ? g.find(id) : -1;
    const c = i >= 0 ? g!.commits[i] : null;
    if (!c) return { b: this.labels.get(id) ?? id.slice(0, 12), k: this.labels.has(id) ? "" : "" };
    const tag = c.refs.find((r) => r.kind === "tag");
    const hashes = this.host.words?.hashes !== false;
    return { b: tag ? tag.name : c.subject, k: `${c.short} · ${agoWords(c.time, Date.now() / 1000)}${hashes ? "" : ` · ${c.author}`}` };
  }

  private flashFields(): void {
    if (reduced()) return;
    for (const f of this.el.querySelectorAll<HTMLElement>(".field")) {
      f.classList.remove("flash");
      void f.offsetWidth;
      f.classList.add("flash");
    }
  }

  // ---- drawing ----

  /** The count beside ± Changes: the diff's changes, once one is known (never read for it). */
  /**
   * The count beside ± Changes, one meaning per mode, said in the button's
   * title: Diff, the changes in the diff shown; Blame, the people who wrote
   * this version. Nothing to count (no diff run, Blame not worked out): no
   * number.
   */
  private badge(): void {
    let n = 0, says = "";
    if (this.mode === "diff") {
      n = this.ran && !this.busy ? this.shown().length : 0;
      says = `${n} change${n === 1 ? "" : "s"} in the diff shown`;
    } else if (this.blameAsked && this.blame) {
      const s = this.blame.summary();
      n = s.waiting || s.busy ? 0 : s.people.filter((p) => p.key).length;
      says = `${n} ${n === 1 ? "person" : "people"} wrote this version`;
    }
    for (const b of this.badges) {
      b.textContent = n > 0 ? String(n) : "";
      b.title = n > 0 ? says : "";
      if (n !== this.lastCount && n > 0 && !reduced()) {
        b.classList.remove("tick");
        void b.offsetWidth;
        b.classList.add("tick");
      }
    }
    for (const a of this.anchors) a.title = `${CHANGES_TITLE}${n > 0 ? `. The number: ${says}` : ""}`;
    this.lastCount = n;
  }

  render(): void {
    if (!this.open) return this.badge();
    const keep = this.el.querySelector(".body")?.scrollTop ?? 0;
    const focus = this.focusKey();
    const was = this.el.dataset.view;
    const view = `${this.mode}${this.pick ? "-pick" : ""}`;
    this.el.dataset.view = view;
    this.el.classList.toggle("folded", this.folded);
    this.el.replaceChildren(html`<div class="grip" title="Drag to make it wider or narrower"></div>${this.head()}<div class="prog" hidden></div><div class="${`body${this.hide ? " hidedel" : ""}`}"></div>${this.lookBox()}${this.foot()}`);
    this.renderProg();
    this.renderBody();
    const body = this.el.querySelector<HTMLElement>(".body")!;
    if (was === view) body.scrollTop = keep;
    this.wire();
    this.badge();
    this.refocus(focus);
  }

  /** The control the keyboard was on, by what it is (drawn again, the focus kept on its like). */
  private focusKey(): string | null {
    const a = (this.el.getRootNode() as ShadowRoot).activeElement as HTMLElement | null;
    if (!a || !this.el.contains(a)) return null;
    if (a.id) return `#${a.id}`;
    for (const k of ["m", "look", "i", "k", "col", "p", "c", "q", "reset", "skip"]) {
      const v = a.dataset[k];
      if (v !== undefined) return `[data-${k}="${CSS.escape(v)}"]${a.dataset.v !== undefined ? `[data-v="${CSS.escape(a.dataset.v)}"]` : ""}`;
    }
    return null;
  }

  private refocus(k: string | null): void {
    if (k) this.el.querySelector<HTMLElement>(k)?.focus({ preventScroll: true });
  }

  /** Just the head's fields (a first end clicked). */
  private renderHead(): void {
    if (!this.open) return;
    const ph = this.el.querySelector(".ph");
    if (!ph) return this.render();
    const focus = this.focusKey();
    ph.replaceWith(this.head());
    this.wire();
    this.refocus(focus);
  }

  private head(): DocumentFragment {
    const seg = html`<div class="seg" role="group" aria-label="Mode"><button type="button" data-m="diff" aria-pressed="${String(this.mode === "diff")}" disabled="${!this.host.diff}" title="Diff: what changed between two versions, marked on the pages">Diff</button><button type="button" data-m="blame" aria-pressed="${String(this.mode === "blame")}" disabled="${!this.blame}" title="Blame: who wrote each line of the paper as it is now">Blame</button></div>`;
    const ttl = html`<div class="ttl"><button type="button" class="ib2 fold" id="chg-fold" aria-expanded="${String(!this.folded)}" aria-label="${this.folded ? "Unfold Changes" : "Fold Changes"}" title="${this.folded ? "Unfold: the list and its controls again" : "Fold to this row: the pages beside it free to read, the marks kept"}">${this.folded ? "▸" : "▾"}</button><h2>Changes</h2><button type="button" class="ib2" id="chg-gear" aria-expanded="${String(this.lookOpen)}" aria-controls="chg-look" aria-label="${this.mode === "blame" ? "Blame's look" : "Diff's look"}" title="${this.lookOpen ? "Hide the look settings" : this.mode === "blame" ? "Blame's look: detail, colours, bars" : "Diff's look: colours and marks"}">⚙</button><button type="button" class="ib2" id="chg-x" aria-label="Close Changes" title="Close (Esc)">✕</button></div>`;
    let below: DocumentFragment;
    if (this.mode === "blame") below = html`<p class="sub">Who last changed each line of the paper as it is now</p>`;
    else {
      const p = this.pair;
      const f = p ? this.name(p.from) : null, t = this.name(p ? p.to : null);
      const field = (which: "from" | "to", n: { b: string; k: string } | null, clear: boolean) =>
        html`<div class="end"><span class="lab">${which === "from" ? "From" : "To"}</span><div class="${`field${this.pick === which ? " picking" : ""}`}" id="${`chg-${which}`}" tabindex="0" role="button" aria-expanded="${String(this.pick === which)}" aria-label="${`${which === "from" ? "From" : "To"}: ${n ? n.b : "choose a version"}`}" title="${which === "from" ? "The older version: click to choose it from the history" : "The newer version: click to choose it from the history"}"><span class="tx">${n ? html`<b>${n.b}</b><code>${n.k || " "}</code>` : html`<b class="ph2">Choose…</b><code>a version from the history</code>`}</span>${clear ? html`<button type="button" class="clr" data-clr="${which}" aria-label="${which === "from" ? "Clear From" : "To the working tree"}" title="${which === "from" ? "Clear: compare nothing" : `Back to ${this.host.tree?.title ?? "the working tree"}`}">×</button>` : html`<span class="chev" aria-hidden="true">▾</span>`}</div></div>`;
      below = html`<div class="ends">${field("from", f, !!p)}<span class="arrow" aria-hidden="true">→</span>${field("to", t, !!p?.to)}</div>`;
    }
    return html`<div class="ph">${ttl}${seg}${below}</div>`;
  }

  /** The progress line: the history's read, the diff's, the blame's; or an error with Try again. */
  private renderProg(): void {
    const el = this.el.querySelector<HTMLElement>(".prog");
    if (!el) return;
    let text = "", prog: [number, number] | undefined, err = "", again: (() => void) | null = null;
    const b = this.mode === "blame" && this.blameAsked ? this.blame?.summary() : null;
    if (b) {
      if (b.error) (err = b.error), (again = () => this.blame!.retry());
      else if (b.busy) {
        // (the history's own line while it is still being read; read to its end, what Blame does now: "Matching 25 files…")
        const h = this.histProgress, live = !!h && /history/i.test(h.text) && !(h.total && (h.done ?? 0) >= h.total);
        text = live ? h!.text : b.busy;
        prog = live && h!.total ? [h!.done ?? 0, h!.total] : b.progress;
      }
    } else if (this.mode === "diff") {
      if (this.error) (err = this.error), (again = () => this.ran && this.run(this.ran));
      else if (this.busy) (text = this.histProgress && /history/i.test(this.busy) ? this.histProgress.text : this.busy), (prog = this.prog ?? (this.histProgress?.total ? [this.histProgress.done ?? 0, this.histProgress.total] : undefined));
      else if (this.pick && this.histProgress) (text = this.histProgress.text), (prog = this.histProgress.total ? [this.histProgress.done ?? 0, this.histProgress.total] : undefined);
    }
    el.hidden = !text && !err;
    el.className = `prog${err ? " err" : ""}`;
    el.setAttribute("role", err ? "alert" : "status");
    if (err) {
      el.replaceChildren(html`<span>${err}</span><button type="button" class="again" title="Read it again">Try again</button>`);
      el.querySelector<HTMLButtonElement>(".again")!.onclick = () => again?.();
      return;
    }
    if (!text) return el.replaceChildren();
    el.replaceChildren(html`<span>${text}</span><span class="${`track${prog ? "" : " ind"}`}" role="progressbar" aria-valuemin="0" aria-valuemax="${prog ? String(prog[1]) : null}" aria-valuenow="${prog ? String(prog[0]) : null}"><i style="${prog ? `width:${Math.min(100, (100 * prog[0]) / Math.max(1, prog[1]))}%` : null}"></i></span>`);
  }

  private renderBody(): void {
    const body = this.el.querySelector<HTMLElement>(".body");
    if (!body) return;
    // (where it was scrolled to, kept: the rows under the pointer stay)
    const keep = body.scrollTop;
    if (this.mode === "blame") body.replaceChildren(this.blameBody());
    else if (this.pick) {
      body.replaceChildren(this.pickerHead());
      const g = this.ensureGraph();
      body.append(g.element);
      g.render();
      if (!g.commits.length && g.hasMore && !g.busy) void g.load();
    } else body.replaceChildren(this.list());
    body.scrollTop = keep;
    this.wireBody();
    this.badge();
  }

  /** The footer: Diff's, always there (no jump as a diff comes): the pages shown, n of m, ‹ ›. */
  private foot(): DocumentFragment {
    if (this.mode !== "diff") return html``;
    const n = this.shown().length;
    const has = !!this.ran && !this.busy;
    const say = !this.ran ? "No diff yet" : this.busy && !this.items.length ? "…" : this.cur < 0 ? `${n} change${n === 1 ? "" : "s"}` : `${this.cur + 1} of ${n}`;
    const pg = (w: "diff" | "current", l: string, t: string) => html`<button type="button" data-pg="${w}" aria-pressed="${String(this.pages === w)}" disabled="${!has}" title="${t}">${l}</button>`;
    return html`<div class="foot"><div class="seg tiny" role="group" aria-label="Pages shown (D)">${pg("diff", "Diff", "Show the diff's pages, the changes marked (D)")}${pg("current", "Current", "Show the current version's pages, the diff kept (D)")}</div><span class="count" aria-live="polite">${say}</span><span class="r nav"><button type="button" id="chg-prev" aria-label="Previous change" title="Previous change ([ or Shift+N)" disabled="${!n}">‹</button><button type="button" id="chg-next" aria-label="Next change" title="Next change (] or N)" disabled="${!n}">›</button></span></div>`;
  }

  private pickerHead(): DocumentFragment {
    const w = this.pick!;
    const g = this.ensureGraph();
    const cs = g.commits;
    const word = this.host.words?.commit ?? "commit";
    const q: [string | null, string, string][] = [];
    if (w === "from") {
      const visit = this.host.lastVisit?.();
      if (visit && g.find(visit) >= 0) q.push([cs[g.find(visit)].id, "Your last visit", "The version as it was when you last opened Changes here"]);
      const tag = cs.find((c) => c.refs.some((r) => r.kind === "tag"));
      const far = this.farKnown;
      if (!tag && !far) void this.farTag();
      const tagWord = word === "version" ? "label" : "tag";
      if (tag) q.push([tag.id, `The last ${tagWord} · ${tag.refs.find((r) => r.kind === "tag")!.name}`, `From the newest ${tagWord}`]);
      else if (far) q.push([far.id, `The last ${tagWord} · ${far.name}`, `From the newest ${tagWord}`]);
      const merge = cs.find((c) => c.parents.length > 1);
      if (merge) q.push([merge.parents[0], "Before the merge", "From just before the newest merge"]);
      if (cs[0] && !q.length) q.push([cs[0].id, word === "version" ? "The last version" : "Last commit · HEAD", `From the newest ${word}`]);
    } else {
      q.push([null, this.host.tree?.title ?? "Working tree", "Up to the text as it is now"]);
      if (cs[0]) q.push([cs[0].id, word === "version" ? "The last version" : "Last commit · HEAD", `Up to the newest ${word}`]);
    }
    const hint = g.aiming
      ? html`<b>Now click the other end</b> (the ${this.host.tree === null ? "editor's text" : "working tree"} for now). The same ${word} again cancels.`
      : html`Click one end, then the other. Drag for a range. <b>Shift</b>-click: To. Double-click: one ${word}'s changes.`;
    return html`<div class="pickhead"><b>${w === "from" ? "Compare from…" : "…up to"}</b><button type="button" class="done" id="chg-done" title="Back to the list, nothing changed (Esc)">Done</button></div><div class="quick">${q.map(([h, l, t]) => html`<button type="button" class="qb" data-q="${h ?? ""}" title="${t}">${l}</button>`)}</div><div class="hint" id="chg-hint">${hint}</div>`;
  }

  /** A list's skeleton: rows shaped like the changes'. */
  private skeleton(n = 5): DocumentFragment {
    return html`${Array.from({ length: n }, (_, i) => html`<div class="skrow" aria-hidden="true"><span class="bone" style="width:70%"></span><span><span class="bone" style="${`width:${80 - ((i * 13) % 35)}%`}"></span><span class="bone" style="${`width:${40 - ((i * 7) % 15)}%`}"></span></span></div>`)}`;
  }

  private list(): DocumentFragment {
    const all = this.items;
    const n = this.shown().length;
    const word = this.host.words?.commit ?? "commit";
    const chips = KINDS.map((k) => {
      const m = all.filter((c) => c.kind === k).length;
      return m ? html`<button type="button" class="fchip" data-k="${k}" aria-pressed="${String(this.kinds.has(k))}" title="${this.kinds.has(k) ? `Hide the ${k.toLowerCase()} changes from the list` : `Show the ${k.toLowerCase()} changes`}">${k} <small>${m}</small></button>` : "";
    });
    const filters = all.length ? html`<div class="filters" role="group" aria-label="Kinds of change">${chips}<label class="toggle hd" title="Leave deleted text out of the list and the pages"><input type="checkbox" id="chg-hide" checked="${this.hide}"> Hide deletions</label></div>` : html``;
    if (!this.ran) {
      const g = this.graph;
      if (g && !g.commits.length && !g.hasMore) return html`<div class="empty"><b>No history yet</b>${word === "version" ? "The project's versions show here once Overleaf has saved them." : "Commit once, and what changes after shows here."}</div>`;
      return html`<div class="empty"><b>Pick a version to compare with</b>Choose From: a ${word}, a ${word === "version" ? "label" : "tag"} or your last visit. What changed since then is listed here and marked on the pages.<button type="button" class="primary" id="chg-start" title="Open the history to choose From">Choose a version…</button></div>`;
    }
    if (!all.length && this.busy) return this.skeleton();
    if (!all.length && this.error) return html``;
    if (!all.length) return html`<div class="empty"><b>Nothing between these two points</b>The two versions have the same text. Choose another From.</div>`;
    if (!n) return html`${filters}<div class="empty"><b>No changes match these filters</b>Turn a kind back on above.</div>`;
    const groups = new Map<string, [ChangeItem, number][]>();
    this.shown().forEach((c, i) => (groups.get(c.sec) ?? groups.set(c.sec, []).get(c.sec)!).push([c, i]));
    const seen = this.seenItems;
    const fresh = seen.size > 0;
    const rows = [...groups].map(([s, arr]) => html`<div class="grp"><span>${s}</span><span>${arr.length}</span></div>${arr.map(([c, i]) => {
      const key = `${c.file}:${c.old}:${c.neu}`;
      const isNew = fresh && !seen.has(key);
      seen.add(key);
      const who = c.who;
      const col = who?.color ?? (who ? hashColor(who.name) : UNCOMMITTED);
      const mine = word === "version" ? "not saved yet" : "not committed";
      const by = who ? html`<span class="av sm" style="${`background:${col}`}">${initials(who.name)}</span>${who.name}${who.time ? ` · ${agoWords(who.time, Date.now() / 1000)}` : ""}${who.short ? html` · <code>${who.short}</code>` : ""}${who.via ? html` · <span class="refp">via merge</span>` : ""}${c.wip ? ` · and yours, ${mine}` : ""}` : c.wip ? html`<span class="av sm" style="${`background:${UNCOMMITTED}`}">Y</span>You · ${mine}` : html`<span>${c.file}</span>`;
      return html`<button type="button" class="${`citem${i === this.cur ? " cur" : ""}${isNew ? " new" : ""}`}" data-i="${i}" title="Show this change on the pages"><span class="k">${c.kind}</span><span class="w"><div>${c.old ? html`<del>${c.old}</del> ` : ""}${c.neu ? html`<ins>${c.neu}</ins>` : ""}</div><small>${by}</small></span></button>`;
    })}`);
    return html`${filters}${rows}`;
  }

  private blameBody(): DocumentFragment {
    const b = this.blame;
    if (!b) return html`<div class="empty">Blame needs the project's history.</div>`;
    const L = b.look;
    const ramp = AGE_RAMPS[L.ages] ?? AGE_RAMPS.violet;
    const word = b.words.commit;
    if (!this.blameAsked)
      return html`<div class="empty"><b>See who wrote each line</b>Bars in the margin, one per printed line, coloured by ${L.color === "age" ? "age" : "author"}; the people and the ${word}s that wrote this version listed here.<button type="button" class="primary" id="chg-blame-go" title="Read the history and mark who wrote each line">Show who wrote what</button></div>`;
    const s = b.summary();
    const colour = html`<div class="sec"><h3>Colour lines by</h3><div class="seg small" role="group"><button type="button" data-col="author" aria-pressed="${String(L.color === "author")}" title="A colour per person">Author</button><button type="button" data-col="age" aria-pressed="${String(L.color === "age")}" title="Newer lines darker">Age</button></div>${L.color === "age" ? html`<div class="ramp">newer ${ramp.light.map((c) => html`<i style="${`background:${c}`}"></i>`)} older</div>` : ""}</div>`;
    if (!s.lines && (s.busy || s.waiting)) return html`${colour}<div class="sec"><h3>Who wrote this version</h3>${this.skeleton(3)}</div>`;
    if (!s.lines && s.error) return colour;
    if (!s.lines) return html`${colour}<div class="empty"><b>Nothing to blame yet</b>The bars come once the pages are built.</div>`;
    const people = s.people.map((p) => html`<button type="button" class="${`prow${b.focusP === p.key ? " sel" : ""}`}" data-p="${p.key}" aria-pressed="${String(b.focusP === p.key)}" title="${b.focusP === p.key ? "Show everyone's lines again" : `Pick out ${p.name}'s lines on the pages`}"><span class="av" style="${`background:${p.color}`}">${p.key ? initials(p.name) : "Y"}</span><span class="nm"><span>${p.name}</span><span class="bar6"><i style="${`width:${Math.round((100 * p.n) / s.lines)}%;background:${p.color}`}"></i></span></span><span class="ct">${p.n}</span></button>`);
    const commits = s.commits.map((c) => {
      const x = c.commit;
      return html`<button type="button" class="${`crow${b.focusC === c.c ? " sel" : ""}`}" data-c="${String(c.c)}" aria-pressed="${String(b.focusC === c.c)}" title="${b.focusC === c.c ? "Show every line again" : `Pick out this ${word}'s lines on the pages`}"><span class="${`sw${x ? "" : " hatch"}`}" style="${`background:${c.color}`}"></span><span style="min-width:0"><div class="s">${x ? x.subject || "(no subject)" : word === "version" ? "Not saved to the history yet" : "Not committed"}</div><small>${x ? html`${x.author || x.email} · ${agoWords(x.time, Date.now() / 1000)} · <code>${x.short}</code>${c.via ? html` · <span class="refp">${c.via.replace(/^On (\S+),.*/, "$1")}</span>` : ""}` : "You · now"}</small></span><span class="ct">${c.n}</span></button>`;
    });
    // (the count only once every file is in; till then the lists are said to be partial)
    const count = s.waiting ? `so far · ${(s.files - s.waiting).toLocaleString()} of ${s.files.toLocaleString()} files` : `${s.lines.toLocaleString()} ${s.unit}`;
    return html`${colour}<div class="sec"><h3><span title="${s.unit === "words" ? "Words as a reader counts them: runs of letters, no numbers, commands, comments or options" : ""}">Who wrote this version · ${count}</span></h3>${people}</div><div class="sec"><h3><span>${word === "version" ? "Versions" : "Commits"} that still own lines</span></h3>${commits}</div>`;
  }

  private segOf<T extends string | number>(key: string, cur: T, opts: [T, string, string?][]): DocumentFragment {
    return html`<div class="seg small" role="group">${opts.map(([v, l, t]) => html`<button type="button" data-look="${key}" data-v="${String(v)}" aria-pressed="${String(cur === v)}" title="${t ?? null}">${l}</button>`)}</div>`;
  }

  /** The look's section under the list: the mode's settings, folded or open (⚙ too); the same ones as the panel's Settings, Changes. */
  private lookBox(): DocumentFragment {
    if (this.pick) return html``;
    const name = this.mode === "blame" ? "Blame's look" : "Diff's look";
    return html`<div class="${`lookbox${this.lookOpen ? " open" : ""}`}"><button type="button" class="lookhead" id="chg-lookhead" aria-expanded="${String(this.lookOpen)}" aria-controls="chg-look" title="${this.lookOpen ? `Fold ${name}` : `${name}: how the marks look, kept for next time`}"><span class="chev2" aria-hidden="true">▸</span>${name}</button><div class="lookin" id="chg-look" hidden="${!this.lookOpen}">${this.lookOpen ? (this.mode === "blame" ? this.blameSettings() : this.diffSettings()) : ""}</div></div>`;
  }

  private diffSettings(): DocumentFragment {
    const L = this.look;
    const can = !!this.host.diff?.download && !!this.ran && !this.busy;
    return html`<div class="set">
      <div class="opt2"><span>Colours</span><div class="swatches">${(Object.entries(DIFF_COLORS) as [ChangesDiffLook["colors"], [string, string, string]][]).map(([k, v]) => html`<button type="button" class="swb" data-look="colors" data-v="${k}" aria-pressed="${String(L.colors === k)}" title="${v[2]}" aria-label="${v[2]}"><i style="${`background:${v[0]}`}"></i><i style="${`background:${v[1]}`}"></i></button>`)}</div></div>
      <div class="opt2"><span>Added text</span>${this.segOf("ins", L.ins, [["wavy", "Wavy", "A wavy underline"], ["line", "Underline", "A straight underline"], ["mark", "Marker", "A highlight behind it"], ["colour", "Colour", "Its colour only"]])}</div>
      <div class="opt2"><span>Deleted text</span>${this.segOf("del", L.del, [["strike", "Struck", "Struck through"], ["mark", "Marker", "Struck, a highlight behind it"], ["caret", "Caret only", "A caret where it was, the text left out"]])}</div>
      <div class="opt2"><span>Changed figure</span>${this.segOf("fig", L.fig, [["side", "Side by side", "The old one beside the new"], ["over", "Overlay", "The old one under the new"], ["new", "New only", "The new one alone"]])}</div>
      <div class="opt2"><span>Change bars</span>${this.segOf("bars", L.bars ? "on" : "off", [["off", "Off", "No bars in the margin"], ["on", "In the margin", "A bar beside each changed line (latexdiff's classic marks)"]])}</div>
      <div class="${`preview i-${L.ins} d-${L.del}${L.bars ? " bars" : ""}`}" aria-label="Preview">We show it <span class="dl">cuts the wait</span> <span class="ins">keeps every edit under 50 ms</span>.</div>
      ${this.host.diff?.download ? html`<div class="opt2"><span>Download</span><div class="dls"><button type="button" class="lnk" data-dl="pdf" disabled="${!can}" title="${can ? "The diff's PDF" : "Compare two versions first"}">PDF</button><button type="button" class="lnk" data-dl="tex" disabled="${!can}" title="${can ? "The marked-up source, diff.tex" : "Compare two versions first"}">diff.tex</button></div></div>` : ""}
      <button type="button" class="reset" data-reset="diff" title="Diff's look as it came">Reset to defaults</button></div>`;
  }

  /** Blame's commits as seen (a skipped one's name and subject kept). */
  private seenCommits = new Map<string, BlameCommit>();

  private blameSettings(): DocumentFragment {
    const b = this.blame!;
    const L = b.look;
    const word = b.words.commit;
    const hashes = b.words.hashes !== false;
    const detailNote = L.detail === "line" ? "Each line takes the newest edit in it, as git blame does." : L.detail === "word" ? "Each word keeps who typed it: a word someone changed in another's sentence is theirs." : word === "version" ? "Exact to the character, from Overleaf's history." : "Each character as git's history gives it: words at best.";
    const known = b.commitsKnown();
    // (a commit skipped owns no lines any more: named as it was seen before)
    for (const c of known) this.seenCommits.set(c.id, c);
    const skips = [...new Set([...L.skip, ...known.map((c) => c.id)])].slice(0, 40).map((id) => {
      const c = known.find((x) => x.id === id) ?? this.seenCommits.get(id);
      const what = c ? `${c.short} ${c.subject}${hashes ? "" : ` · ${c.author}`}` : id.slice(0, 10);
      return html`<label title="${what}: skipped, its lines go to whoever wrote them before"><input type="checkbox" data-skip="${id}" checked="${L.skip.includes(id)}"><span>${c ? html`<code>${c.short}</code> ${c.subject}${hashes ? "" : ` · ${c.author}`}` : id.slice(0, 10)}</span></label>`;
    });
    const since = b.summary().since;
    const sinceSel = html`<select id="chg-since" aria-label="Changed since" title="Lines older than this are dimmed"><option value="off" selected="${L.since === "off"}">Off</option>${since.map((s) => html`<option value="${s.v}" selected="${L.since === s.v}">${s.label}</option>`)}${L.since !== "off" && !since.some((s) => s.v === L.since) ? html`<option value="${L.since}" selected>${L.since.split(":").slice(1).join(":")}</option>` : ""}</select>`;
    const sw = (key: string, cur: string, opts: [string, string[], string][]) => html`<div class="swatches">${opts.map(([k, cols, name]) => html`<button type="button" class="swb" data-look="${key}" data-v="${k}" aria-pressed="${String(cur === k)}" title="${name}" aria-label="${name}">${cols.slice(0, 4).map((c) => html`<i style="${`background:${c}`}"></i>`)}</button>`)}</div>`;
    return html`<div class="set">
      <div class="opt2"><span>Detail</span>${this.segOf("detail", L.detail, [["line", "Line", "Whole lines"], ["word", "Word", "Word by word"], ["char", "Character", "Character by character"]])}</div>
      <p class="note2">${detailNote}</p>
      <div class="opt2"><span>Spacing-only edits</span>${this.segOf("ws", L.ws, [["ignore", "Ignore", "A change of spacing alone changes no one's text"], ["count", "Count", "Spacing changes count as edits"]])}</div>
      <div class="opt2"><span>Moved text</span>${this.segOf("moved", L.moved, [["keep", "Keep its writer", "Text moved stays its writer's"], ["mover", "Credit the mover", "Text moved is the mover's"]])}</div>
      <div class="opt2"><span>${word === "version" ? "Not saved yet" : "Not committed"}</span>${this.segOf("wip", L.wip, [["show", "Show hatched", "Grey hatched bars"], ["hide", "Hide", "No bars"]])}</div>
      <div class="opt2 wide"><span>Skip ${word}s</span><div class="skips">${skips.length ? skips : html`<small>None blamed yet</small>`}<small>A skipped ${word}'s lines go to whoever wrote them before (a reformat, a spell-check)</small></div></div>
      <div class="opt2"><span>Changed since</span>${sinceSel}</div>
      <div class="opt2"><span>Tint the text</span>${this.segOf("tint", L.tint, [["off", "Off", "No tint"], ["hover", "Under pointer", "The paragraph under the pointer"], ["all", "Everywhere", "Every line tinted"]])}</div>
      <div class="opt2"><span>Bars</span>${this.segOf("bars", L.bars, [["left", "Left margin", "Bars in the left margin"], ["right", "Right margin", "Bars in the right margin"]])}</div>
      <div class="opt2"><span>Bar width</span>${this.segOf("width", L.width, [[2, "Thin"], [4, "Medium"], [7, "Wide"]])}</div>
      <div class="opt2"><span>People</span>${sw("people", L.people, [["okabe", AUTHOR_COLORS, "Okabe–Ito (colour-blind safe)"], ["overleaf", OVERLEAF_COLORS, "Overleaf's collaborator colours"]])}</div>
      <div class="opt2"><span>Age</span>${sw("ages", L.ages, (Object.entries(AGE_RAMPS) as [string, { name: string; light: string[] }][]).map(([k, v]) => [k, v.light, v.name]))}</div>
      <button type="button" class="reset" data-reset="blame" title="Blame's look as it came">Reset to defaults</button></div>`;
  }

  /** The look changed, from the popover or the panel's Settings: kept, applied. */
  private setDiffLook(l: Partial<ChangesDiffLook>): void {
    this.look = { ...this.look, ...l };
    this.applyColors();
    if (this.ran) this.host.diff?.restyle(this.look, this.hide);
    this.save();
    this.render();
  }

  private setHide(v: boolean): void {
    this.hide = v;
    this.save();
    if (this.ran) this.host.diff?.restyle(this.look, this.hide);
    this.render();
  }

  private setBlameLook(l: Partial<BlameLook>): void {
    this.blame?.setLook(l);
    this.save();
    this.render();
  }

  /**
   * The panel's Settings, Changes: the same choices as the popover's look
   * sections (one place or the other, the same prefs).
   */
  settingRows(): SettingRowLike[] {
    const rows: SettingRowLike[] = [];
    const L = this.look;
    if (this.host.diff) {
      rows.push(
        { id: "chg-colors", label: "Diff: colours", help: "Added and deleted text", kind: "choice", options: Object.entries(DIFF_COLORS).map(([k, v]) => [k, v[2].replace(/ \(.*\)/, "")]), value: L.colors, set: (v) => this.setDiffLook({ colors: v as ChangesDiffLook["colors"] }) },
        { id: "chg-ins", label: "Diff: added text", kind: "choice", options: [["wavy", "Wavy"], ["line", "Underline"], ["mark", "Marker"], ["colour", "Colour"]], value: L.ins, set: (v) => this.setDiffLook({ ins: v as ChangesDiffLook["ins"] }) },
        { id: "chg-del", label: "Diff: deleted text", kind: "choice", options: [["strike", "Struck"], ["mark", "Marker"], ["caret", "Caret only"]], value: L.del, set: (v) => this.setDiffLook({ del: v as ChangesDiffLook["del"] }) },
        { id: "chg-fig", label: "Diff: changed figure", kind: "choice", options: [["side", "Side by side"], ["over", "Overlay"], ["new", "New only"]], value: L.fig, set: (v) => this.setDiffLook({ fig: v as ChangesDiffLook["fig"] }) },
        { id: "chg-bars", label: "Diff: change bars", help: "A bar beside each changed line (latexdiff's classic marks)", kind: "toggle", value: !!L.bars, set: (v) => this.setDiffLook({ bars: !!v }) },
        { id: "chg-hide", label: "Diff: hide deletions", help: "Deleted text left out of the list and the pages", kind: "toggle", value: this.hide, set: (v) => this.setHide(!!v) },
      );
    }
    const b = this.blame;
    if (b) {
      const B = b.look;
      rows.push(
        { id: "bl-detail", label: "Blame: detail", kind: "choice", options: [["line", "Line"], ["word", "Word"], ["char", "Character"]], value: B.detail, set: (v) => this.setBlameLook({ detail: v as BlameLook["detail"] }) },
        { id: "bl-color", label: "Blame: colour lines by", kind: "choice", options: [["author", "Author"], ["age", "Age"]], value: B.color, set: (v) => this.setBlameLook({ color: v as BlameLook["color"] }) },
        { id: "bl-ws", label: "Blame: spacing-only edits", kind: "choice", options: [["ignore", "Ignore"], ["count", "Count"]], value: B.ws, set: (v) => this.setBlameLook({ ws: v as BlameLook["ws"] }) },
        { id: "bl-moved", label: "Blame: moved text", kind: "choice", options: [["keep", "Keep its writer"], ["mover", "Credit the mover"]], value: B.moved, set: (v) => this.setBlameLook({ moved: v as BlameLook["moved"] }) },
        { id: "bl-tint", label: "Blame: tint the text", kind: "choice", options: [["off", "Off"], ["hover", "Under pointer"], ["all", "Everywhere"]], value: B.tint, set: (v) => this.setBlameLook({ tint: v as BlameLook["tint"] }) },
        { id: "bl-bars", label: "Blame: bars", kind: "choice", options: [["left", "Left margin"], ["right", "Right margin"]], value: B.bars, set: (v) => this.setBlameLook({ bars: v as BlameLook["bars"] }) },
        { id: "bl-width", label: "Blame: bar width", kind: "choice", options: [["2", "Thin"], ["4", "Medium"], ["7", "Wide"]], value: String(B.width), set: (v) => this.setBlameLook({ width: Number(v) }) },
        { id: "bl-people", label: "Blame: people's colours", kind: "choice", options: [["okabe", "Okabe–Ito"], ["overleaf", "Overleaf's"]], value: B.people, set: (v) => this.setBlameLook({ people: v as BlameLook["people"] }) },
      );
    }
    return rows;
  }

  private wire(): void {
    const $ = <T extends HTMLElement>(s: string) => this.el.querySelector<T>(s);
    $("#chg-x")!.onclick = () => this.off();
    $("#chg-fold")!.onclick = () => this.fold(!this.folded);
    const look = () => {
      this.lookOpen = !this.lookOpen;
      this.render();
      if (this.lookOpen) this.el.querySelector(".lookbox")?.scrollIntoView({ block: "nearest" });
    };
    $("#chg-gear")!.onclick = look;
    const lh = $("#chg-lookhead");
    if (lh) lh.onclick = look;
    for (const b of this.el.querySelectorAll<HTMLButtonElement>(".ph [data-m]"))
      b.onclick = () => {
        const m = b.dataset.m as "diff" | "blame";
        // (Blame's half clicked: Blame asked for, worked out now; shown again: the history asked if it moved)
        if (m === "blame") (this.blameAsked = true), this.blame?.refreshHistory();
        if (this.mode === m && m !== "blame") return;
        this.mode = m;
        this.pick = null;
        this.cur = -1;
        this.save();
        this.marks();
        this.render();
      };
    const field = (which: "from" | "to") => {
      const f = $<HTMLElement>(`#chg-${which}`);
      if (!f) return;
      f.onclick = (e) => !(e.target as HTMLElement).closest(".clr") && this.openPicker(which);
      f.onkeydown = (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), this.openPicker(which));
    };
    field("from");
    field("to");
    for (const b of this.el.querySelectorAll<HTMLButtonElement>("[data-clr]"))
      b.onclick = (e) => {
        e.stopPropagation();
        if (b.dataset.clr === "from") {
          this.pair = null;
          this.ran = null;
          this.items = [];
          this.host.diff?.stop();
          this.graph?.mark(null, null);
          this.marks();
          this.render();
        } else if (this.pair) this.choose({ from: this.pair.from, to: null }, false);
      };
    for (const b of this.el.querySelectorAll<HTMLButtonElement>("[data-pg]")) b.onclick = () => this.showPages(b.dataset.pg as "diff" | "current");
    const prev = $<HTMLButtonElement>("#chg-prev"), next = $<HTMLButtonElement>("#chg-next");
    if (prev) prev.onclick = () => this.step(-1);
    if (next) next.onclick = () => this.step(1);
    this.wireLook();
    // (dragged wider or narrower from its left edge)
    const grip = $<HTMLElement>(".grip");
    if (grip)
      grip.onpointerdown = (e) => {
        grip.setPointerCapture(e.pointerId);
        const right = this.el.getBoundingClientRect().right;
        grip.onpointermove = (m) => {
          this.width = Math.max(300, Math.min(640, right - m.clientX));
          this.place();
        };
        grip.onpointerup = () => (grip.onpointermove = null);
      };
  }

  /** The look's controls (the section under the list). */
  private wireLook(): void {
    const box = this.el.querySelector<HTMLElement>(".lookin");
    if (!box) return;
    const b = this.blame;
    for (const x of box.querySelectorAll<HTMLButtonElement>("[data-look]"))
      x.onclick = () => {
        const k = x.dataset.look!, v = x.dataset.v!;
        if (this.mode === "blame") this.setBlameLook({ [k]: k === "width" ? Number(v) : v } as Partial<BlameLook>);
        else this.setDiffLook(k === "bars" ? { bars: v === "on" } : ({ [k]: v } as Partial<ChangesDiffLook>));
      };
    for (const x of box.querySelectorAll<HTMLInputElement>("[data-skip]"))
      x.onchange = () => {
        if (!b) return;
        const skip = new Set(b.look.skip);
        x.checked ? skip.add(x.dataset.skip!) : skip.delete(x.dataset.skip!);
        this.setBlameLook({ skip: [...skip] });
      };
    const since = box.querySelector<HTMLSelectElement>("#chg-since");
    if (since) since.onchange = () => this.setBlameLook({ since: since.value });
    for (const x of box.querySelectorAll<HTMLButtonElement>("[data-dl]")) x.onclick = () => this.host.diff?.download?.(x.dataset.dl as "pdf" | "tex");
    for (const x of box.querySelectorAll<HTMLButtonElement>("[data-reset]"))
      x.onclick = () => {
        if (x.dataset.reset === "blame") this.setBlameLook({ ...BLAME_LOOK, skip: [] });
        else this.setDiffLook({ ...DIFF_LOOK });
      };
  }

  private wireBody(): void {
    const body = this.el.querySelector<HTMLElement>(".body");
    if (!body) return;
    const $ = <T extends HTMLElement>(s: string) => body.querySelector<T>(s);
    const done = $("#chg-done");
    if (done)
      done.onclick = () => {
        this.pick = null;
        this.graph?.cancel();
        this.render();
        this.el.querySelector<HTMLElement>("#chg-from")?.focus();
      };
    const start = $("#chg-start");
    if (start) start.onclick = () => this.openPicker("from");
    const go = $("#chg-blame-go");
    if (go)
      go.onclick = () => {
        this.blameAsked = true;
        this.marks();
        this.render();
      };
    for (const q of body.querySelectorAll<HTMLButtonElement>(".qb"))
      q.onclick = () => {
        const h = q.dataset.q || null;
        const p = this.pair;
        const g = this.ensureGraph();
        if (this.pick === "from") {
          if (!h) return;
          const keepTo = p?.to && g.find(p.to) >= 0 && g.find(p.to) < g.find(h) ? p.to : null;
          this.choose({ from: h, to: keepTo }, true);
        } else {
          const from = p?.from ?? g.commits[1]?.id ?? g.commits[0]?.id;
          if (from) this.choose({ from: h === from ? (g.commits[g.find(from)]?.parents[0] ?? from) : from, to: h }, true);
        }
      };
    for (const c of body.querySelectorAll<HTMLButtonElement>(".fchip[data-k]"))
      c.onclick = () => {
        const k = c.dataset.k!;
        this.kinds.has(k) ? this.kinds.delete(k) : this.kinds.add(k);
        this.cur = -1;
        this.save();
        this.render();
      };
    const hide = $<HTMLInputElement>("#chg-hide");
    if (hide) hide.onchange = () => this.setHide(hide.checked);
    for (const c of body.querySelectorAll<HTMLButtonElement>(".citem"))
      c.onclick = () => {
        this.cur = Number(c.dataset.i);
        const it = this.shown()[this.cur];
        if (it && this.host.diff && this.ran) {
          if (this.pages !== "diff") this.showPages("diff");
          this.host.diff.goto(it.k);
        }
        this.render();
      };
    const b = this.blame;
    for (const x of body.querySelectorAll<HTMLButtonElement>("[data-col]")) x.onclick = () => this.setBlameLook({ color: x.dataset.col as BlameLook["color"] });
    for (const x of body.querySelectorAll<HTMLButtonElement>(".prow")) x.onclick = () => b?.focus("person", x.dataset.p ?? "");
    for (const x of body.querySelectorAll<HTMLButtonElement>(".crow")) x.onclick = () => b?.focus("commit", Number(x.dataset.c));
    // (the picker's hint follows a first end clicked)
    const g = this.graph;
    if (this.pick && g) g.element.onpointerup = () => setTimeout(() => this.open && this.pick && this.refreshHint(), 0);
  }

  /** The picker's quick choices, the history read further (a tag, a merge found): replaced where they changed, in place. */
  private refreshQuick(): void {
    const q = this.el.querySelector(".quick");
    const nq = this.pickerHead().querySelector(".quick");
    if (!q || !nq || q.textContent === nq.textContent) return;
    q.replaceWith(nq);
    this.wireBody();
  }

  private refreshHint(): void {
    const h = this.el.querySelector("#chg-hint");
    if (!h || !this.pick) return;
    const fresh = this.pickerHead();
    const nh = fresh.querySelector("#chg-hint");
    if (nh) h.replaceWith(nh);
    this.renderHead();
  }

  /** A merged commit's words for Blame's card ("On lina/related-work, merged into main (5d0a7e3)"), from the graph read so far. */
  via(id: string): string | null {
    const g = this.graph;
    if (!g?.commits.length) return null;
    const m = mergedVia(g.commits, id);
    if (!m) return null;
    const main = g.commits[0]?.refs.find((r) => r.kind === "branch")?.name ?? "main";
    return `On ${m.branch}, merged into ${main} (${m.merge.short})`;
  }

  /** The graph read (Blame's merges need it): its first page; Blame asked for, so the history is being read anyway. */
  readHistory(): void {
    if (!this.blameAsked) return;
    const g = this.ensureGraph();
    if (!g.commits.length && g.hasMore && !g.busy) void g.load();
  }

  /** The history changed (a commit, a new version): read again, if it was read. */
  historyChanged(): void {
    if (!this.graph?.commits.length) return;
    void this.graph.reload();
  }
}

/** A row of the panel's Settings dialog (panel.ts SettingRow's shape, without importing the panel). */
export interface SettingRowLike {
  id: string;
  label: string;
  help?: string;
  kind: "toggle" | "choice";
  options?: [string, string][];
  value?: boolean | string;
  set?(v?: boolean | string): void;
}
