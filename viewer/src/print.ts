// The viewer's one stylesheet (viewer/STYLE.md): the panel looks native to
// the host it is in. Every rule reads only the --ph-* tokens; each host maps
// them once, below: Overleaf to its own variables (custom properties inherit
// into the shadow root, so the panel follows Overleaf's theme), VS Code to
// its webview's --vscode-*, and the watch, which has no host, to its own
// palette (the logo's navy and orange), light or dark with the system.
// Every token has a fallback, so a variable a host renames leaves a look,
// not a hole.

/** Overleaf, dark (its default): its PDF toolbar's, its menus', its green. */
const OVERLEAF = `
  --ph-bar: var(--toolbar-alt-bg-color, #2f3a4c); --ph-fg: var(--content-primary-dark, #f4f5f6); --ph-fg2: var(--neutral-40, #afb5c0);
  --ph-border: var(--neutral-70, #495365); --ph-hover: var(--toolbar-btn-hover-bg-color, #495365);
  --ph-stage: var(--pane, var(--pdf-bg, #2f3a4c)); --ph-menu: var(--bg-dark-primary, #1b222c);
  --ph-accent: var(--bg-accent-01, #098842); --ph-accent-hover: var(--bg-accent-02, #1e6b41); --ph-accent-fg: #fff;
  --ph-danger: var(--content-danger-dark, #e36d66); --ph-warning: var(--content-warning-dark, #de8014); --ph-info: var(--content-info-dark, #97b6e5);
  --ph-link: var(--link-ui-dark, #97b6e5); --ph-progress: var(--bg-accent-01, #098842);
  --ph-radius: var(--border-radius-base, 4px); --ph-pill: var(--border-radius-full, 9999px);
  --ph-font: var(--font-sans, "Noto Sans", system-ui, sans-serif); --ph-size: 13px; --ph-bar-h: 32px;
`;
const OVERLEAF_LIGHT = `
  --ph-bar: var(--toolbar-alt-bg-color, #fff); --ph-fg: var(--content-primary, #1b222c); --ph-fg2: var(--content-secondary, #495365);
  --ph-border: var(--border-divider, #e7e9ee); --ph-hover: var(--toolbar-btn-hover-bg-color, #f4f5f6);
  --ph-stage: var(--pane, var(--pdf-bg, #f4f5f6)); --ph-menu: var(--bg-light-primary, #fff);
  --ph-danger: var(--content-danger, #b83a33); --ph-warning: var(--content-warning, #8f5514); --ph-info: var(--content-info, #366cbf);
  --ph-link: var(--link-ui, #366cbf);
`;
/** VS Code: the webview's theme variables (they change with the theme). */
const VSCODE = `
  --ph-bar: var(--vscode-editorGroupHeader-tabsBackground, #181818); --ph-fg: var(--vscode-foreground, #cccccc);
  --ph-fg2: var(--vscode-descriptionForeground, #9d9d9d); --ph-border: var(--vscode-panel-border, #2b2b2b);
  --ph-hover: var(--vscode-toolbar-hoverBackground, rgb(90 93 94 / 31%)); --ph-stage: var(--vscode-editor-background, #1f1f1f);
  --ph-menu: var(--vscode-menu-background, #1f1f1f); --ph-accent: var(--vscode-button-background, #0078d4);
  --ph-accent-hover: var(--vscode-button-hoverBackground, #026ec1); --ph-accent-fg: var(--vscode-button-foreground, #fff);
  --ph-danger: var(--vscode-errorForeground, #f85149); --ph-warning: var(--vscode-editorWarning-foreground, #cca700);
  --ph-info: var(--vscode-editorInfo-foreground, #3794ff); --ph-link: var(--vscode-textLink-foreground, #4daafc);
  --ph-progress: var(--vscode-progressBar-background, #0078d4);
  --ph-radius: 2px; --ph-pill: 2px; --ph-font: var(--vscode-font-family, system-ui, sans-serif); --ph-size: var(--vscode-font-size, 13px); --ph-bar-h: 35px;
`;
/** The watch: its own look, from the logo's colours. */
const WATCH = `
  --ph-bar: #ffffff; --ph-fg: #1b222c; --ph-fg2: #5d6778; --ph-border: #e3e6ec; --ph-hover: #eef1f6; --ph-stage: #eceef2;
  --ph-menu: #ffffff; --ph-accent: #1d3e72; --ph-accent-hover: #16325d; --ph-accent-fg: #fff;
  --ph-danger: #c23a2f; --ph-warning: #a3620a; --ph-info: #1d3e72; --ph-link: #1d3e72; --ph-progress: #f58f2d;
  --ph-radius: 6px; --ph-pill: 9999px; --ph-font: "Noto Sans", system-ui, sans-serif; --ph-size: 13px; --ph-bar-h: 40px;
`;
const WATCH_DARK = `
  --ph-bar: #161b23; --ph-fg: #e7e9ee; --ph-fg2: #98a1b0; --ph-border: #262d38; --ph-hover: #222935; --ph-stage: #0f1319;
  --ph-menu: #1b212b; --ph-accent: #2c5aa0; --ph-accent-hover: #3567b3; --ph-danger: #f0786d; --ph-warning: #e5a54b;
  --ph-info: #8fb3ec; --ph-link: #8fb3ec;
`;

/** A page in dark mode (the user's choice, Off by default: it inverts figures too): a grey page, soft ink. */
const DARK_PAGE = "#2c2e33";
export const DARK_PDF_FILTER = "invert(1) hue-rotate(180deg) contrast(.7) brightness(1.04)";

/** The pages' part, for any host that draws pages (css.ts): none beyond the slots' own. */
export const PAGE_CSS = "";

export const PRINT_CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
.win { ${OVERLEAF} }
.win.light { ${OVERLEAF_LIGHT} }
.win.vscode { ${VSCODE} }
.win.watch { ${WATCH} }
@media (prefers-color-scheme: dark) { .win.watch:not(.light) { ${WATCH_DARK} color-scheme: dark; } }
.win.watch.dark { ${WATCH_DARK} color-scheme: dark; }

.win { position: fixed; z-index: 2147483000; display: flex; flex-direction: column; container-type: inline-size;
  min-width: 300px; min-height: 180px; max-width: calc(100vw - 16px); max-height: calc(100vh - 16px); resize: both; overflow: hidden;
  background: var(--ph-stage); color: var(--ph-fg); font: var(--ph-size)/1.4 var(--ph-font);
  border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) * 2); box-shadow: 0 8px 28px rgb(0 0 0 / 30%); }
.win button { font: inherit; color: inherit; }
.win :focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.icon { display: block; flex: none; }
.grow { flex: 1; min-width: 4px; }
[hidden] { display: none !important; }

/* the floating window's handle (docked: none) */
header { display: flex; align-items: center; gap: 6px; height: 30px; padding: 0 4px 0 12px; flex: none; cursor: grab; user-select: none;
  background: var(--ph-bar); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
header:active { cursor: grabbing; }
.win.docked > header { display: none; }

/* the bar: Rebuild and the status on the left; find, compare, download, the page, zoom, problems, the menu on the right */
.bar { position: relative; flex: none; height: var(--ph-bar-h); display: flex; align-items: center; gap: 4px; padding: 0 6px;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); color: var(--ph-fg); }
.win.docked:not(.own-header) > .bar { display: none; }
.ib { all: unset; box-sizing: border-box; height: 26px; min-width: 26px; padding: 0 5px; display: inline-flex; align-items: center; justify-content: center;
  gap: 4px; border-radius: var(--ph-radius); cursor: pointer; color: var(--ph-fg2); white-space: nowrap; }
.ib:hover, .ib[aria-expanded="true"], .ib[aria-pressed="true"] { background: var(--ph-hover); color: var(--ph-fg); }
.ib:disabled { opacity: .4; cursor: default; }
.ib:focus-visible { outline: 2px solid var(--ph-link); }
.split { display: inline-flex; height: 24px; flex: none; border-radius: var(--ph-pill); overflow: hidden; background: var(--ph-accent); color: var(--ph-accent-fg); }
.split button { all: unset; display: inline-flex; align-items: center; gap: 5px; cursor: pointer; font-weight: 600; }
.split button:hover, .split button[aria-expanded="true"] { background: var(--ph-accent-hover); }
.split button:focus-visible { outline: 2px solid var(--ph-fg); outline-offset: -2px; }
.split .go { padding: 0 10px 0 8px; }
.split .more { padding: 0 4px; border-left: 1px solid rgb(255 255 255 / 25%); }
.split.busy .go .icon { animation: phx-spin 1s linear infinite; }
@keyframes phx-spin { to { transform: rotate(360deg); } }
.status { all: unset; box-sizing: border-box; display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 6px; min-width: 0;
  border-radius: var(--ph-radius); color: var(--ph-fg2); cursor: pointer; white-space: nowrap; overflow: hidden; font-variant-numeric: tabular-nums; }
.status:hover, .status[aria-expanded="true"] { background: var(--ph-hover); color: var(--ph-fg); }
/* (squeezed: its detail goes first, then its words end in an ellipsis; the leading word kept) */
.status b { flex: 0 1 auto; min-width: 4.5ch; font-weight: 400; color: var(--ph-fg); overflow: hidden; text-overflow: ellipsis; }
#stmore { flex: 0 1000 auto; min-width: 0; overflow: hidden; }
.bar > .status { min-width: calc(4.5ch + 12px); }
.status.err b { color: var(--ph-danger); }
.pageno { display: inline-flex; align-items: center; gap: 4px; color: var(--ph-fg2); font-variant-numeric: tabular-nums; white-space: nowrap; }
.pageno input { width: 3ch; height: 22px; box-sizing: content-box; padding: 0 2px; padding: 0; text-align: center; font: inherit; color: var(--ph-fg); background: transparent;
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); }
.zoomb { padding: 0 2px 0 8px; }
#zs { display: none; }
.probs { font-weight: 600; font-variant-numeric: tabular-nums; }
.probs.error { color: var(--ph-danger); } .probs.warning { color: var(--ph-warning); }
.progress { position: absolute; left: 0; right: 0; bottom: -1px; height: 2px; overflow: hidden; opacity: 0; transition: opacity .2s; pointer-events: none; }
.progress.on { opacity: 1; }
.progress::after { content: ""; position: absolute; top: 0; bottom: 0; width: 30%; background: var(--ph-progress); animation: phx-carriage 1.2s ease-in-out infinite; }
@keyframes phx-carriage { from { left: -30%; } to { left: 100%; } }
.find { display: inline-flex; align-items: center; gap: 6px; }
.find input { width: 170px; height: 24px; padding: 0 8px; font: inherit; color: var(--ph-fg); background: var(--ph-menu);
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); outline: none; }
.find input:focus { border-color: var(--ph-link); }
#findn { color: var(--ph-fg2); min-width: 48px; font-variant-numeric: tabular-nums; }
#findn.none { color: var(--ph-danger); }
/* narrower: what gives way, in order: the zoom's long label, find's box, then (560) the buttons the ⋯ menu also has, then the page box beside an open find */
.bar > :not(.status):not(.grow) { flex: none; }
.bar > .grow { flex: 1 1 0; min-width: 0; }
@container (max-width: 820px) { #zl { display: none; } #zs { display: inline; } }
@container (max-width: 760px) {
  /* (find open: Compare and Download step aside, to the ⋯ menu, which has them) */
  .bar:has(.find:not([hidden])) :is(#b-compare, #pdf) { display: none; }
  .bar:has(.find:not([hidden])) #x-compare.narrow:not([hidden]) { display: flex !important; }
}
@container (max-width: 700px) { .find input { width: 110px; } #findn { min-width: 0; } }
@container (max-width: 560px) {
  .wide { display: none !important; }
}
@container (max-width: 460px) {
  .bar:has(.find:not([hidden])) .pageno { display: none; }
  .find input { width: 90px; }
  .bar { gap: 2px; }
}
@container (max-width: 360px) { .find input { width: 72px; } .bar { padding: 0 4px; } }
@container (min-width: 561px) { .narrow { display: none !important; } }

/* menus and popovers: plain lists in the host's menu colour */
.menu { position: absolute; z-index: 20; min-width: 230px; max-width: calc(100% - 12px); padding: 4px 0; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 4px 16px rgb(0 0 0 / 22%); }
.menu > button, .menu .mi { all: unset; box-sizing: border-box; display: flex; align-items: center; gap: 10px; width: 100%; padding: 6px 14px; cursor: pointer; }
.menu > button:hover, .menu > button:focus-visible { background: var(--ph-hover); }
.menu .mi { cursor: default; }
.menu .sub { margin-left: auto; padding-left: 12px; color: var(--ph-fg2); font-size: 12px; font-variant-numeric: tabular-nums; }
.menu .desc { display: block; color: var(--ph-fg2); font-size: 12px; }
.menu hr { border: 0; border-top: 1px solid var(--ph-border); margin: 4px 0; }
.menu .check { width: 18px; flex: none; display: inline-flex; justify-content: center; color: var(--ph-link); }
.radios { display: flex; gap: 4px; padding: 2px 14px 6px 42px; }
.radios button { all: unset; padding: 2px 10px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); font-size: 12px; cursor: pointer; }
.radios button[aria-pressed="true"] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.timing { width: 380px; padding: 10px 14px 12px; font-variant-numeric: tabular-nums; }
.timing h3 { margin: 0 0 8px; font-size: 13px; font-weight: 600; }
.timing table { width: 100%; border-collapse: collapse; }
.timing td { padding: 3px 0; vertical-align: top; }
.timing td:first-child { color: var(--ph-fg2); padding-right: 12px; }
.timing td:last-child { text-align: right; }
.timing .foot { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.timing .foot.err { color: var(--ph-danger); }

/* problems: Overleaf's log entries, in a popover under the bar */
.logs { right: 6px; top: 4px; width: min(460px, calc(100% - 12px)); max-height: calc(100% - 8px); overflow: auto; padding: 0; }
.logs > header { all: unset; display: flex; align-items: center; gap: 8px; padding: 8px 8px 8px 14px; position: sticky; top: 0; z-index: 1;
  background: var(--ph-menu); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
.diag { display: block; padding: 9px 14px 10px; border-left: 3px solid var(--sev, var(--ph-fg2)); border-bottom: 1px solid var(--ph-border); }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--ph-hover); }
.diag.error { --sev: var(--ph-danger); } .diag.warning { --sev: var(--ph-warning); } .diag.info { --sev: var(--ph-info); }
.diag .where { float: right; margin-left: 10px; color: var(--ph-link); font-size: 12px; }
.diag .text { font-weight: 600; overflow-wrap: anywhere; }
.diag .detail, .folded .detail { margin: 6px 0 0; padding: 0; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; color: var(--ph-fg2);
  white-space: pre-wrap; overflow-wrap: anywhere; max-height: 12em; overflow: auto; font-weight: 400; }
.folded { margin-top: 6px; font-size: 12px; font-weight: 400; }
.folded summary { cursor: pointer; color: var(--ph-fg2); }
.diags .phx-inline { margin: 0; padding: 0; list-style: none; }
.reportbtn { all: unset; margin: 8px 14px; color: var(--ph-link); cursor: pointer; }
.reportbtn:hover { text-decoration: underline; }

/* the diff against a version: a strip under the bar */
.diffbar { display: none; flex: none; align-items: center; gap: 4px; padding: 3px 6px 3px 12px; flex-wrap: wrap;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); font-variant-numeric: tabular-nums; }
.diffbar.on { display: flex; }
.diffbar .what { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.diffbar .what small { color: var(--ph-fg2); font-size: inherit; }
.diffbar .n { white-space: nowrap; padding: 0 4px; color: var(--ph-fg2); }
.diffbar .busy::before { content: ""; display: inline-block; width: 7px; height: 7px; margin-right: 6px; border-radius: 50%; background: var(--ph-progress);
  animation: phx-pulse .6s ease-in-out infinite alternate; }
@keyframes phx-pulse { from { opacity: .4; } }
.diffbar button { all: unset; cursor: pointer; padding: 2px 8px; border-radius: var(--ph-radius); color: var(--ph-fg2); white-space: nowrap; }
.diffbar button:hover:not(:disabled) { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar button:disabled { opacity: .4; cursor: default; }
.diffbar button.cur { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px; background: var(--ph-border); }
.diffset { display: none; left: 50%; transform: translateX(-50%); top: 4px; width: 320px; padding: 12px; font-size: 12px; }
.diffset.on { display: block; }
.diffset h4 { margin: 0 0 6px; font-size: 12px; font-weight: 600; color: var(--ph-fg2); }
.diffset h4:not(:first-child) { margin-top: 12px; }
.diffset .opts { display: flex; flex-wrap: wrap; gap: 4px; }
.diffset button { all: unset; cursor: pointer; padding: 3px 9px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); }
.diffset button:hover { background: var(--ph-hover); }
.diffset button[aria-pressed=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.diffset .pair { display: inline-flex; align-items: center; gap: 6px; }
.diffset .pair i { display: inline-block; width: 12px; height: 12px; border-radius: 50%; }
.diffset .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 6px; }
.diffset input[type=color] { width: 30px; height: 22px; padding: 0; border: 0; background: none; cursor: pointer; }
.diffset .sample { margin-top: 10px; padding: 6px 8px; border-radius: var(--ph-radius); background: #fff; color: #1b222c; font: 13px/1.4 Georgia, serif; }
.diffset .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 12px; color: var(--ph-fg2); }

/* the pages */
.body { position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column; }
.win.has-side .body { flex-direction: row; }
.side { display: none; flex: none; width: 240px; overflow: auto; background: var(--ph-bar); border-right: 1px solid var(--ph-border); color: var(--ph-fg); }
.win.side-on .side { display: block; }
.win.side-on.side-empty .side { display: none; }
@container (max-width: 560px) { .win.side-on .side { position: absolute; inset: 0 auto 0 0; z-index: 5; box-shadow: 4px 0 16px rgb(0 0 0 / 20%); } }
.stage { position: relative; flex: 1; min-height: 0; overflow: auto; background: var(--ph-stage); text-align: center; overscroll-behavior: contain; will-change: scroll-position; }
.win.docked .stage { overflow-y: scroll; }
.win.docked .viewer { min-height: 100%; }
.stage.fit { overflow-x: hidden; }
/* (a rebuild's opening up: the last build's pages hidden under it until the dive) */
.win.reopening .viewer { visibility: hidden; }
/* (the first page's sheet up: the stage holds still under it, its width kept, so nothing scrolls behind the sheet) */
.win.sheet-up .stage, .win.docked.sheet-up .stage { overflow: hidden; scrollbar-gutter: stable; }
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
.win.dark-pdf .slot, .win.docked.pdf-dark .slot { background: ${DARK_PAGE}; }
.win.dark-pdf .slot canvas, .win.docked.pdf-dark .slot canvas { filter: ${DARK_PDF_FILTER}; }
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: rgb(0 100 255 / 25%); }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.stage.offline .slot { opacity: .4; pointer-events: none; }
.slot .mark { position: absolute; pointer-events: none; border-radius: 2px; background: rgb(255 214 0 / 45%); mix-blend-mode: multiply;
  box-shadow: 0 0 0 1px rgb(232 172 0 / 80%); transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); box-shadow: none; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); box-shadow: 0 0 0 1px rgb(150 100 0 / 90%); }
.slot .culprit { position: absolute; pointer-events: none; }
/* (a culprit hovered: its problem's message, a small tooltip under the line) */
.ctip { position: absolute; z-index: 8; max-width: min(420px, calc(100% - 16px)); display: flex; gap: 6px; align-items: flex-start; padding: 5px 9px;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius);
  box-shadow: 0 3px 12px rgb(0 0 0 / 25%); font-size: 12px; line-height: 1.35; pointer-events: none; overflow-wrap: anywhere; }
.ctip .icon { margin-top: 1px; }
.ctip.error .icon { color: var(--ph-danger); } .ctip.warning .icon { color: var(--ph-warning); }
/* (a problem's culprit: a wavy line along its glyphs' foot, as an editor underlines one; no fill, the text stays readable) */
.slot .culprit::after { content: ""; position: absolute; left: 0; right: 0; bottom: -2px; height: 3px; background: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23d93025' stroke-width='1'/%3E%3C/svg%3E") repeat-x left bottom / 6px 3px; }
.slot .culprit.warning::after { background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23c77700' stroke-width='1'/%3E%3C/svg%3E"); }
.slot .peer { position: absolute; pointer-events: none; border-radius: 2px; background: hsl(var(--hue) 70% 50% / 22%); box-shadow: inset 0 -1.5px 0 hsl(var(--hue) 70% 45%); }
.slot .peer-tag { width: auto; height: auto; transform: translateY(-100%); padding: 0 5px; border-radius: 3px 3px 3px 0; background: hsl(var(--hue) 70% 42%);
  box-shadow: none; color: #fff; font: 600 11px/16px var(--ph-font); white-space: nowrap; z-index: 2; }

/* strips over the pages: plain, in the host's colours */
.strip { position: sticky; top: 0; z-index: 2; display: none; padding: 6px 12px; text-align: left; font-size: 12px;
  background: var(--ph-menu); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-warning)); }
.stage.stale .banner, .approx.on { display: block; }
.gone { --sev: var(--ph-danger); font-weight: 600; }
.stage.offline .gone { display: flex; align-items: baseline; gap: 8px; }
.gone .x { margin-left: auto; flex: none; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; padding: 0 2px; }
.offpill { display: none; position: absolute; top: 8px; right: 14px; z-index: 3; padding: 1px 8px; border-radius: var(--ph-pill); font-size: 12px;
  background: var(--ph-menu); color: var(--ph-danger); border: 1px solid var(--ph-border); pointer-events: none; }
.win.offline-quiet .offpill { display: block; }

/* before any page: the first page's sheet, the opening playing on it, the step under it */
.opening { position: absolute; z-index: 3; pointer-events: none; left: 0; top: 0; width: 0; height: 0;
  transition: left .3s ease, top .3s ease, width .3s ease, height .3s ease; }
.opening .sheet { position: absolute; inset: 0; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); }
.opening.in .sheet { background: none; box-shadow: none; }
.win.dark-pdf .opening .sheet, .win.docked.pdf-dark .opening .sheet { background: ${DARK_PAGE}; }
.win.dark-pdf .opening.in .sheet, .win.docked.pdf-dark .opening.in .sheet { background: none; }
.opening .phx-open { position: absolute; inset: 0; width: 100%; height: 100%; }
.steps { position: absolute; left: 10%; right: 10%; bottom: 13%; text-align: center; color: #5d6778; font: 400 13px/1.4 var(--ph-font);
  font-variant-numeric: tabular-nums; transition: opacity .3s; }
.steps .what { color: #1b222c; }
.steps .detail { min-height: 1.4em; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.steps .track { position: relative; height: 3px; width: min(100%, 280px); margin: 8px auto; border-radius: 2px; overflow: hidden; background: #e7e9ee; }
.steps .track i { position: absolute; inset: 0 auto 0 0; border-radius: 2px; background: #f58f2d; transition: width .3s; }
.steps .track.unknown i { width: 30% !important; animation: phx-slide 1.2s ease-in-out infinite; }
@keyframes phx-slide { from { left: -30%; } to { left: 100%; } }
.win.dark-pdf .steps, .win.docked.pdf-dark .steps { color: #a3a9b4; }
.win.dark-pdf .steps .what, .win.docked.pdf-dark .steps .what { color: #e4e6ea; }
.win.dark-pdf .steps .track, .win.docked.pdf-dark .steps .track { background: #3d4047; }
.opening.in .steps { opacity: 0; }
/* a card in place of the pages (a build that stopped, an engine to choose): plain */
.empty { padding: 40px 16px; color: var(--ph-fg2); }
.card { max-width: 420px; margin: 0 auto; padding: 16px 18px; text-align: left; background: var(--ph-menu); color: var(--ph-fg);
  border: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-danger)); border-radius: var(--ph-radius); }
.card h3 { margin: 0 0 6px; font-size: 14px; font-weight: 600; }
.card .count { overflow-wrap: anywhere; }
.card .note { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; }
.card .btns { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 12px; }
.card .btn { all: unset; cursor: pointer; padding: 3px 12px; border-radius: var(--ph-pill); background: var(--ph-accent); color: var(--ph-accent-fg); font-weight: 600; }

/* a small notification in the corner: a package fetched, the rest of a build, a message */
.toast { position: absolute; right: 12px; bottom: 12px; z-index: 7; width: min(320px, calc(100% - 24px)); display: grid; gap: 4px; padding: 10px 12px;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 4px 16px rgb(0 0 0 / 22%);
  font-variant-numeric: tabular-nums; text-align: left; transition: opacity .3s, transform .3s; }
.toast.out { opacity: 0; transform: translateY(6px); }
.toast .t { display: flex; align-items: center; gap: 8px; font-weight: 600; min-width: 0; }
.toast .t span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.toast.msg .t { align-items: flex-start; } .toast.msg .t span { white-space: normal; overflow-wrap: anywhere; }
.toast.msg .t .icon { margin-top: 2px; }
.toast .d { color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.toast.err { border-left: 3px solid var(--ph-danger); }
.toast .track { position: relative; height: 3px; margin-top: 2px; border-radius: 2px; overflow: hidden; background: var(--ph-border); }
.toast .track i { position: absolute; inset: 0 auto 0 0; background: var(--ph-progress); transition: width .25s; }
.toast .track.unknown i { width: 30% !important; animation: phx-slide 1.2s ease-in-out infinite; }

/* feature tips: a popover with an arrow at the control it is about */
.tip { position: absolute; z-index: 30; width: 270px; max-width: calc(100% - 16px); padding: 12px 14px 10px; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) + 2px); box-shadow: 0 6px 20px rgb(0 0 0 / 25%); }
.tip::before { content: ""; position: absolute; top: -6px; left: var(--ax, 50%); width: 10px; height: 10px; margin-left: -5px; transform: rotate(45deg);
  background: var(--ph-menu); border-left: 1px solid var(--ph-border); border-top: 1px solid var(--ph-border); }
.tip .new { display: inline-block; margin-right: 6px; padding: 0 7px; border-radius: var(--ph-pill); font-size: 11px; font-weight: 600; vertical-align: 1px;
  background: var(--ph-accent); color: var(--ph-accent-fg); }
.tip h4 { margin: 0 0 4px; font-size: 13px; font-weight: 600; }
.tip p { margin: 0 0 10px; color: var(--ph-fg2); }
.tip .acts { display: flex; align-items: center; gap: 8px; }
.tip .acts .n { flex: 1; color: var(--ph-fg2); font-size: 12px; font-variant-numeric: tabular-nums; }
.tip .acts button { all: unset; cursor: pointer; padding: 3px 4px; color: var(--ph-link); }
.tip .acts .primary { padding: 3px 12px; border-radius: var(--ph-pill); background: var(--ph-accent); color: var(--ph-accent-fg); font-weight: 600; }
.ringed { box-shadow: 0 0 0 2px var(--ph-accent); }

/* a tip's demo: a little stage looping what the feature does (4 s; the compare's 6 s), in the host's colours; a page is paper */
.tip .demo { position: relative; height: 84px; margin: 8px 0 10px; overflow: hidden; border-radius: var(--ph-radius);
  background: var(--ph-stage); border: 1px solid var(--ph-border); font: 10px/1.3 var(--ph-font); color: var(--ph-fg); }
.tip .demo .mbar { position: absolute; inset: 0 0 auto; height: 20px; display: flex; align-items: center; gap: 4px; padding: 0 6px;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); }
.tip .demo .pg { position: absolute; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 25%); padding: 6px 7px; overflow: hidden; }
.tip .demo .pg i { display: block; height: 3px; margin: 0 0 4px; border-radius: 1px; background: #c3c8d0; }
.tip .demo .mmenu { position: absolute; display: flex; flex-direction: column; padding: 3px; opacity: 0; z-index: 1;
  background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 3px 10px rgb(0 0 0 / 25%); }
.tip .demo .mmenu span { display: flex; align-items: center; gap: 4px; padding: 2px 6px; border-radius: 2px; white-space: nowrap; }
.tip .demo .mmenu b { margin-left: auto; padding-left: 6px; }

/* compare: "good" struck out, "excellent" added, then the navigator */
.tip .demo.cmp { height: auto; padding: 10px 12px 30px; background: #fff; color: #1b222c; font: 13px/1.5 "Noto Serif", Georgia, serif; }
.tip .cmp .del { animation: phx-del 6s ease-in-out infinite; }
.tip .cmp .add { display: inline-block; max-width: 0; overflow: hidden; vertical-align: bottom; white-space: nowrap; color: #1d4ed8;
  text-decoration: underline wavy #1d4ed8; text-underline-offset: 3px; animation: phx-add 6s ease-in-out infinite; }
.tip .cmp .tag { position: absolute; right: 8px; bottom: 7px; padding: 3px 8px; border-radius: 9px; font: 600 11px/1 var(--ph-font);
  color: #fff; background: rgb(27 34 44 / 88%); opacity: 0; animation: phx-tag 6s ease-in-out infinite; }
.tip .cmp .ver { position: absolute; left: 12px; right: 76px; bottom: 9px; height: 12px; font: 11px/12px var(--ph-font); color: #6b7280; }
.tip .cmp .ver i { position: absolute; left: 0; font-style: normal; white-space: nowrap; }
.tip .cmp .v0 { animation: phx-v0 6s steps(1) infinite; }
.tip .cmp .v1 { opacity: 0; animation: phx-v1 6s steps(1) infinite; }
@keyframes phx-del { 0%, 30% { color: #1b222c; text-decoration: none; } 40%, 92% { color: #b91c1c; text-decoration: line-through 2px #b91c1c; } 100% { color: #1b222c; } }
@keyframes phx-add { 0%, 45% { max-width: 0; } 60%, 92% { max-width: 6em; } 100% { max-width: 0; } }
@keyframes phx-tag { 0%, 62% { opacity: 0; transform: translateY(6px); } 70%, 92% { opacity: 1; transform: none; } 100% { opacity: 0; } }
@keyframes phx-v0 { 0% { opacity: 1; } 31% { opacity: 0; } }
@keyframes phx-v1 { 0% { opacity: 0; } 31% { opacity: 1; } }

/* timing: the status clicked, its times drop down, each bar filling */
.tip .tim .mst { padding: 1px 6px; border-radius: var(--ph-radius); font-weight: 600; animation: phx-press 4s infinite; }
.tip .tim .mpop { position: absolute; left: 6px; top: 24px; width: 75%; padding: 3px 7px 4px; line-height: 12px; opacity: 0; animation: phx-drop 4s ease-out infinite;
  background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 3px 10px rgb(0 0 0 / 25%); }
.tip .tim .mpop b { display: block; margin-bottom: 2px; font-weight: 600; }
.tip .tim .mpop p { display: grid; grid-template-columns: 40px 1fr 34px; align-items: center; gap: 5px; margin: 0; color: var(--ph-fg2); }
.tip .tim .mpop i { height: 4px; width: 0; border-radius: 2px; background: var(--ph-progress); animation: phx-fill 4s ease-out infinite; }
.tip .tim .mpop em { text-align: right; font-style: normal; font-variant-numeric: tabular-nums; white-space: nowrap; }
@keyframes phx-press { 0%, 12% { background: none; } 18%, 85% { background: var(--ph-hover); } 100% { background: none; } }
@keyframes phx-drop { 0%, 18% { opacity: 0; transform: translateY(-4px); } 28%, 88% { opacity: 1; transform: none; } 100% { opacity: 0; } }
@keyframes phx-fill { 0%, 28% { width: 0; } 50%, 88% { width: var(--w); } 100% { width: var(--w); } }

/* rebuild: the menu opens, Clean rebuild chosen, the page typeset afresh */
.tip .reb .msplit { display: flex; border-radius: var(--ph-pill); overflow: hidden; color: var(--ph-accent-fg); background: var(--ph-accent); font-weight: 600; }
.tip .reb .msplit > span { display: flex; align-items: center; gap: 2px; padding: 1px 5px; }
.tip .reb .dd { border-left: 1px solid rgb(255 255 255 / 30%); animation: phx-press-on 4s infinite; }
.tip .reb .go .icon { animation: phx-turn 4s linear infinite; }
.tip .reb .mmenu { left: 6px; top: 24px; animation: phx-menu 4s infinite; }
.tip .reb .mmenu .pick { animation: phx-pick 4s infinite; }
.tip .reb .pg { right: 14px; top: 26px; width: 34%; height: 70px; }
.tip .reb .pg i { animation: phx-retype 4s infinite; transform-origin: left; }
.tip .reb .pg i:nth-child(2) { animation-delay: .08s; } .tip .reb .pg i:nth-child(3) { animation-delay: .16s; }
.tip .reb .pg i:nth-child(4) { animation-delay: .24s; } .tip .reb .pg i:nth-child(5) { animation-delay: .32s; } .tip .reb .pg i:nth-child(6) { animation-delay: .4s; }
@keyframes phx-press-on { 0%, 10% { background: none; } 15%, 45% { background: var(--ph-accent-hover); } 50% { background: none; } }
@keyframes phx-menu { 0%, 14% { opacity: 0; transform: translateY(-3px); } 20%, 46% { opacity: 1; transform: none; } 52%, 100% { opacity: 0; } }
@keyframes phx-pick { 0%, 30% { background: none; } 36%, 52% { background: var(--ph-hover); } }
@keyframes phx-turn { 0%, 50% { transform: none; } 75%, 100% { transform: rotate(360deg); } }
@keyframes phx-retype { 0%, 52% { transform: none; } 56% { transform: scaleX(0); } 64%, 100% { transform: none; } }

/* dark PDF: the ⋯ menu, Dark PDF ticked, the page goes soft dark (a grey page, off-white ink; never black) */
.tip .drk .mbar { justify-content: flex-end; }
.tip .drk .dots { display: flex; padding: 1px 3px; border-radius: var(--ph-radius); animation: phx-press-on2 4s infinite; }
.tip .drk .mmenu { right: 6px; top: 24px; animation: phx-menu 4s infinite; }
.tip .drk .mmenu .pick { animation: phx-pick 4s infinite; }
.tip .drk .mmenu b { opacity: 0; animation: phx-tick 4s steps(1) infinite; }
.tip .drk .pg { left: 14px; top: 26px; width: 38%; height: 70px; animation: phx-dark 4s infinite; }
.tip .drk .pg i { animation: phx-ink 4s infinite; }
@keyframes phx-press-on2 { 0%, 10% { background: none; } 15%, 45% { background: var(--ph-hover); } 50% { background: none; } }
@keyframes phx-tick { 0%, 38% { opacity: 0; } 40%, 100% { opacity: 1; } }
@keyframes phx-dark { 0%, 46% { background: #fff; } 56%, 92% { background: ${DARK_PAGE}; } 100% { background: #fff; } }
@keyframes phx-ink { 0%, 46% { background: #c3c8d0; } 56%, 92% { background: #a9aeb6; } 100% { background: #c3c8d0; } }

/* contents: the column beside the page, the section at following the page as it scrolls */
.tip .toc .col { position: absolute; inset: 0 auto 0 0; width: 44%; padding: 6px 0; background: var(--ph-bar); border-right: 1px solid var(--ph-border);
  animation: phx-col 4s ease-out infinite; }
.tip .toc .col span { position: relative; z-index: 1; display: block; height: 18px; padding: 0 8px; line-height: 18px; white-space: nowrap; }
.tip .toc .col .at { position: absolute; left: 4px; right: 4px; top: 6px; height: 18px; border-radius: 2px; background: var(--ph-hover);
  box-shadow: inset 2px 0 var(--ph-accent); animation: phx-at 4s infinite; }
.tip .toc .pg { left: 50%; right: 10%; top: 8px; bottom: -40px; padding-top: 0; }
.tip .toc .roll { padding-top: 6px; animation: phx-roll 4s infinite; }
.tip .toc .roll s { display: block; height: 6px; }
@keyframes phx-col { 0% { transform: translateX(-100%); } 12%, 100% { transform: none; } }
@keyframes phx-at { 0%, 30% { transform: none; } 38%, 62% { transform: translateY(18px); } 70%, 100% { transform: translateY(36px); } }
@keyframes phx-roll { 0%, 30% { transform: none; } 38%, 62% { transform: translateY(-28px); } 70%, 100% { transform: translateY(-56px); } }

/* dialogs over the pages: the keys, the log, the debug report, about */
.dialog { position: absolute; z-index: 25; inset: 12px; display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 24px rgb(0 0 0 / 30%); }
.dialog > header { all: unset; display: flex; align-items: center; gap: 8px; font-weight: 600; }
.dialog p { margin: 0; color: var(--ph-fg2); }
.dialog textarea, .dialog pre { flex: 1; min-height: 120px; margin: 0; overflow: auto; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre;
  color: var(--ph-fg); background: var(--ph-stage); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 8px; resize: none; }
.dialog .btns { display: flex; gap: 8px; justify-content: flex-end; }
.dialog .btn { all: unset; cursor: pointer; padding: 3px 12px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); }
.dialog .btn:hover { background: var(--ph-hover); }
.dialog.small { inset: auto 12px 12px auto; width: min(340px, calc(100% - 24px)); }
.keys { inset: 12px 12px auto auto; width: min(420px, calc(100% - 24px)); max-height: calc(100% - 24px); overflow: auto; }
.keys table { border-collapse: collapse; }
.keys td { padding: 2px 12px 2px 0; vertical-align: top; }
.keys kbd { font: 12px ui-monospace, monospace; padding: 0 5px; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); white-space: nowrap; }
.about a { color: var(--ph-link); }
.about .about-foot { margin-top: 8px; color: var(--ph-fg2); }
/* who made this, in Overleaf's pane (it looks native on purpose; it must never pass for Overleaf's) */
.byline { all: unset; display: none; position: absolute; left: 12px; bottom: 10px; z-index: 3; max-width: calc(100% - 24px); padding: 1px 8px;
  border-radius: var(--ph-pill); font-size: 11px; color: var(--ph-fg2); background: var(--ph-menu); border: 1px solid var(--ph-border);
  cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.byline:hover { color: var(--ph-fg); }
.win.docked:not(.own-header) .byline { display: block; }
/* Overleaf's own settings, docked: the main file, the debug check, the latency's details */
.sheet-pop { right: 8px; top: 6px; width: min(340px, calc(100% - 16px)); padding: 12px; display: grid; gap: 8px; }
.sheet-pop label { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.sheet-pop select { font: inherit; color: var(--ph-fg); background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 2px 4px; max-width: 180px; }
.sheet-pop .links { display: flex; flex-wrap: wrap; gap: 12px; }
.sheet-pop .links button { all: unset; cursor: pointer; color: var(--ph-link); }
.details { margin: 0; font: 11px/1.4 ui-monospace, monospace; color: var(--ph-fg2); white-space: pre-wrap; }

/* docked: the panel fills the host's pane under its toolbar */
.win.docked { position: relative; left: auto !important; top: auto !important; width: 100% !important; height: 100% !important; max-width: none; max-height: none;
  min-width: 0; min-height: 0; resize: none; border: 0; border-radius: 0; box-shadow: none; }
.win.collapsed { resize: none; min-height: 0; min-width: 0; border-radius: var(--ph-pill); }
.win.collapsed > :not(header) { display: none; }
.win.collapsed header { border-bottom: 0; }
@media (prefers-reduced-motion: reduce) {
  .progress::after, .split.busy .go .icon, .steps .track.unknown i, .toast .track.unknown i, .diffbar .busy::before { animation: none; }
  .opening, .toast { transition: none; }
  /* (the demos still, at what they show: the change marked, the times, Dark PDF on, the contents) */
  .tip .demo, .tip .demo * { animation: none !important; }
  .tip .cmp .del { color: #b91c1c; text-decoration: line-through 2px #b91c1c; } .tip .cmp .add { max-width: 6em; }
  .tip .cmp .tag, .tip .cmp .v1, .tip .tim .mpop { opacity: 1; } .tip .cmp .v0 { opacity: 0; } .tip .tim .mpop i { width: var(--w); }
  .tip .reb .mmenu, .tip .drk .mmenu, .tip .drk .mmenu b { opacity: 1; } .tip .reb .pick, .tip .drk .pick { background: var(--ph-hover); }
  .tip .drk .pg { background: ${DARK_PAGE}; } .tip .drk .pg i { background: #a9aeb6; }
}
`;
