// The viewer's one stylesheet (viewer/STYLE.md): the panel looks native to
// the host it is in. Every rule reads only the --ph-* tokens; each host maps
// them once, below: Overleaf to its own variables (custom properties inherit
// into the shadow root, so the panel follows Overleaf's theme), VS Code to
// its webview's --vscode-*, and the watch, which has no host, to its own
// palette (the logo's navy and orange), light or dark with the system.
// Every token has a fallback, so a variable a host renames leaves a look,
// not a hole.

/** Overleaf, dark (its default): its PDF toolbar's, its menus'; the accent PhiTeX's blue (the logo's), not Overleaf's green. */
const OVERLEAF = `
  --ph-bar: var(--toolbar-alt-bg-color, #2f3a4c); --ph-fg: var(--content-primary-dark, #f4f5f6); --ph-fg2: var(--neutral-40, #afb5c0);
  --ph-border: var(--neutral-70, #495365); --ph-hover: var(--toolbar-btn-hover-bg-color, #495365);
  --ph-stage: var(--pane, var(--pdf-bg, #2f3a4c)); --ph-menu: var(--bg-dark-primary, #1b222c);
  --ph-accent: #2c5aa0; --ph-accent-hover: #3567b3; --ph-accent-fg: #fff;
  --ph-danger: var(--content-danger-dark, #e36d66); --ph-warning: var(--content-warning-dark, #de8014); --ph-info: var(--content-info-dark, #97b6e5);
  --ph-link: var(--link-ui-dark, #97b6e5); --ph-progress: #f58f2d;
  --ph-radius: var(--border-radius-base, 4px); --ph-pill: var(--border-radius-full, 9999px);
  --ph-font: var(--font-sans, "Noto Sans", system-ui, sans-serif); --ph-size: 13px; --ph-bar-h: 32px;
`;
const OVERLEAF_LIGHT = `
  --ph-bar: var(--toolbar-alt-bg-color, #fff); --ph-fg: var(--content-primary, #1b222c); --ph-fg2: var(--content-secondary, #495365);
  --ph-border: var(--border-divider, #e7e9ee); --ph-hover: var(--toolbar-btn-hover-bg-color, #f4f5f6);
  --ph-stage: var(--pane, var(--pdf-bg, #f4f5f6)); --ph-menu: var(--bg-light-primary, #fff);
  --ph-danger: var(--content-danger, #b83a33); --ph-warning: var(--content-warning, #8f5514); --ph-info: var(--content-info, #366cbf);
  --ph-link: var(--link-ui, #366cbf); --ph-accent: #1d3e72; --ph-accent-hover: #16325d;
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
/* (the carriage runs only while the line shows, and by transform: a hidden one moving by left laid the page out every frame, scrolling included) */
.progress::after { content: ""; position: absolute; top: 0; bottom: 0; left: 0; width: 30%; background: var(--ph-progress); transform: translateX(-100%); }
.progress.on::after { animation: phx-carriage 1.2s ease-in-out infinite; }
@keyframes phx-carriage { from { transform: translateX(-100%); } to { transform: translateX(333.4%); } }
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

/* menus and popovers: plain lists in the host's menu colour; above a
   feature tip (30): a menu is what the user just opened, and the welcome
   tip's card covered the record menu's Clean rebuild */
.menu { position: absolute; z-index: 31; min-width: 230px; max-width: calc(100% - 12px); padding: 4px 0; text-align: left;
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
.timing { width: 340px; padding: 12px 14px; font-variant-numeric: tabular-nums; }
.timing .head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-bottom: 8px; }
.timing .head span { color: var(--ph-fg2); }
/* (Did you know: under the opening's step, quiet) */
.opening .steps .hint { margin: 10px auto 0; color: var(--ph-fg2); font-size: 12px; line-height: 1.4; max-width: 420px; }
/* (the first build beside the edits: bars as long as their times) */
.timing .tbars { padding: 4px 12px 8px; display: grid; gap: 4px; }
.timing .tb { display: grid; grid-template-columns: 88px 1fr auto; align-items: center; gap: 8px; font-variant-numeric: tabular-nums; }
.timing .tb .k { color: var(--ph-fg2); }
.timing .tb .track { height: 8px; border-radius: 4px; background: var(--ph-hover, rgb(0 0 0 / 6%)); overflow: hidden; }
.timing .tb .track i { display: block; height: 100%; border-radius: 4px; background: var(--ph-fg2); }
.timing .tb .track i.edit, .timing .tb .track i.median { background: var(--ph-progress, #f58f2d); }
.timing .tb.x { display: block; color: var(--ph-fg2); font-size: 11px; }
.timing .head b { font-size: 20px; font-weight: 600; }
.timing .row { display: grid; grid-template-columns: 1fr auto; gap: 3px 12px; padding: 7px 0; border-top: 1px solid var(--ph-border); }
.timing .row .k { color: var(--ph-fg2); }
.timing .row .v { font-weight: 600; text-align: right; }
.timing .row .note { grid-column: 1 / -1; font-size: 11px; line-height: 1.35; color: var(--ph-fg2); }
.timing .row .tbar { grid-column: 1 / -1; display: flex; gap: 2px; height: 6px; margin-top: 2px; }
.timing .row .tbar i { min-width: 3px; border-radius: 2px; }
.timing .row .legend { grid-column: 1 / -1; display: flex; flex-wrap: wrap; gap: 2px 10px; font-size: 11px; color: var(--ph-fg2); }
.timing .row .legend span { display: inline-flex; align-items: center; gap: 4px; }
.timing .row .legend i { width: 8px; height: 8px; border-radius: 2px; }
.timing .c0 { background: var(--ph-progress); } .timing .c1 { background: var(--ph-accent); } .timing .c2 { background: var(--ph-fg2); } .timing .c3 { background: var(--ph-border); }
.timing .foot { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.timing .foot.err { color: var(--ph-danger); }
.timing .copydbg { margin: 8px 0 0; font-size: 12px; }

/* problems: Overleaf's log entries, in a popover under the bar */
/* (the problems' filters: a kind a pill, on or off) */
.logs .kinds { display: flex; flex-wrap: wrap; gap: 4px; padding: 6px 12px 8px; border-bottom: 1px solid var(--ph-border); }
.logs .kinds button { all: unset; cursor: pointer; padding: 2px 9px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); font-size: 12px; font-variant-numeric: tabular-nums; }
.logs .kinds button[aria-pressed=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.logs .kinds button:hover { background: var(--ph-hover); }
.logs .kinds button[aria-pressed=true]:hover { background: var(--ph-accent); }
.logs { right: 6px; top: 4px; width: min(460px, calc(100% - 12px)); max-height: calc(100% - 8px); overflow: auto; padding: 0; }
.logs > header { all: unset; display: flex; align-items: center; gap: 8px; padding: 8px 8px 8px 14px; position: sticky; top: 0; z-index: 1;
  background: var(--ph-menu); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
.diag { display: block; padding: 9px 14px 10px; border-left: 3px solid var(--sev, var(--ph-fg2)); border-bottom: 1px solid var(--ph-border); }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--ph-hover); }
.diag.more { display: flex; gap: 8px; }
.diag.more button { font: inherit; color: var(--ph-accent, inherit); background: none; border: 1px solid var(--ph-border); border-radius: 5px; padding: 2px 8px; cursor: pointer; }
.diag.error { --sev: var(--ph-danger); } .diag.warning { --sev: var(--ph-warning); } .diag.info { --sev: var(--ph-info); }
.diag .where { float: right; margin-left: 10px; color: var(--ph-link); font-size: 12px; }
.diag .text { font-weight: 600; overflow-wrap: anywhere; }
.diag .detail, .folded .detail { margin: 6px 0 0; padding: 0; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; color: var(--ph-fg2);
  white-space: pre-wrap; overflow-wrap: anywhere; max-height: 12em; overflow: auto; font-weight: 400; }
.folded { margin-top: 6px; font-size: 12px; font-weight: 400; }
.folded summary { cursor: pointer; color: var(--ph-fg2); }
.diags .phx-inline { margin: 0; padding: 0; list-style: none; }
/* a problem's Copy for an AI assistant: its row's top right corner, clear when the row is pointed at or focused */
.diag, .diags .phx-inline > li { position: relative; padding-right: 40px; }
.copyai { all: unset; position: absolute; top: 6px; right: 8px; display: inline-flex; padding: 4px; border-radius: var(--ph-radius, 4px); color: var(--ph-fg2); opacity: .45; cursor: pointer; }
.diag:hover .copyai, .phx-inline > li:hover .copyai, .copyai:focus-visible { opacity: 1; }
.copyai:hover { background: var(--ph-hover); color: var(--ph-fg); }
.copyai:focus-visible { outline: 2px solid var(--ph-link); }
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
/* (Diff | Current: which pages are shown, the one shown filled; the diff's style on its half) */
.diffbar .seg { display: inline-flex; border: 1px solid var(--ph-accent); border-radius: var(--ph-pill); overflow: hidden; }
.diffbar .seg button { border-radius: 0; padding: 2px 10px; color: var(--ph-accent); font-weight: 600; }
.diffbar .seg button[aria-pressed=true] { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.diffbar .seg button[aria-pressed=true]:hover:not(:disabled) { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.diffbar .seg .chev { font-weight: 400; margin-left: 2px; }
.diffset .row.bars button { padding: 1px 10px; }
.diffset .row.bars button[aria-checked=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.diffset .row.bars button:disabled { opacity: .45; cursor: default; }
.diffbar button.cur { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px; background: var(--ph-border); }
.diffset { display: none; left: 50%; transform: translateX(-50%); top: 4px; width: 320px; max-height: calc(100% - 12px); overflow: auto; padding: 12px; font-size: 12px; }
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
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); container-type: inline-size; }
/* (a page the build has not shipped alone yet: it comes at the build's end, said, not a blank sheet) */
.slot.later::before { content: "Drawn when the build ends"; position: absolute; inset: 0; display: grid; place-items: center; color: #8a8f98; font: 500 clamp(9px, 2.6cqw, 18px)/1.3 var(--ph-font); }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
/* (a page streamed while the build runs and drawn without some of what it draws with: a quiet mark, gone when the build's page replaces it) */
.slot.preview::after { content: "Preview: final colours and shading when the build ends"; position: absolute; left: 50%; bottom: 6px; transform: translateX(-50%); max-width: calc(100% - 12px); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding: 1px 8px; border-radius: 999px; background: rgb(20 24 31 / 55%); color: #fff; font: 500 clamp(8px, 1.7cqw, 12px)/1.5 var(--ph-font); pointer-events: none; }
.win.dark-pdf .slot, .win.docked.pdf-dark .slot { background: ${DARK_PAGE}; }
.win.dark-pdf .slot canvas, .win.docked.pdf-dark .slot canvas { filter: ${DARK_PDF_FILTER}; }
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
/* the reading aid (wasp.ts): bands over the lines, blended: lighten on a white page (black ink takes the colour, the paper stays
   white), darken on Dark PDF's (its light ink takes a light colour, the grey stays); a stop "wi" leaves the ink as it is; the
   colours the panel's variables (Panel.waspApply: the preset or the user's, at the strength chosen), Classic's at full without them */
.phx-wasp { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; mix-blend-mode: lighten; }
.phx-wasp .wi { stop-color: #000; }
.phx-wasp .w0 { stop-color: var(--ph-wasp0, #1747d6); }
.phx-wasp .w1 { stop-color: var(--ph-wasp1, #d0202a); }
.win.dark-pdf .phx-wasp, .win.docked.pdf-dark .phx-wasp { mix-blend-mode: darken; }
.win.dark-pdf .phx-wasp .wi, .win.docked.pdf-dark .phx-wasp .wi { stop-color: #fff; }
.win.dark-pdf .phx-wasp .w0, .win.docked.pdf-dark .phx-wasp .w0 { stop-color: var(--ph-wasp0-dark, #6e98ff); }
.win.dark-pdf .phx-wasp .w1, .win.docked.pdf-dark .phx-wasp .w1 { stop-color: var(--ph-wasp1-dark, #ff7b72); }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
/* (out of the layout while a zoom gesture runs, Panel.zoomLay: its glyphs laid out again at every size, most of a frame's layout) */
.zooming .phx-text, .phx-text[data-hold] { display: none; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
/* (a link under the pointer, viewer.ts: the hand, over the text's cursor) */
.slot.on-link, .slot.on-link .phx-text text { cursor: pointer; }
/* a link hovered: where it goes, in a small window (viewer.ts peekShow) */
.peek { position: absolute; z-index: 4; width: min(440px, calc(100% - 16px)); text-align: left; background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 24px rgb(0 0 0 / 22%); overflow: hidden; cursor: default; font: var(--ph-size)/1.4 var(--ph-font); }
.peek .pv { position: relative; height: 250px; overflow: auto; overscroll-behavior: contain; background: #fff; touch-action: pan-x pan-y; }
.peek .pv canvas { display: block; position: absolute; left: 0; top: 0; }
.peek .pv .pz { pointer-events: none; }
.peek.zoomed .pv { cursor: grab; }
.peek.zoomed .pv.grabbing { cursor: grabbing; }
.peek .pf .zm { display: inline-flex; align-items: center; gap: 1px; margin-left: auto; }
.peek .pf .zm button { all: unset; cursor: pointer; display: inline-grid; place-items: center; min-width: 20px; height: 20px; border-radius: var(--ph-radius); color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.peek .pf .zm button svg { width: 14px; height: 14px; }
.peek .pf .zm .zl { min-width: 38px; font-size: 11.5px; }
.peek .pf .zm button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); color: var(--ph-fg); }
.peek .pf .zm button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.peek.cite .pf .zm, .peek.url .pf .zm, .peek.wait .pf .zm { display: none; }
.peek .pf .zm + .go { margin-left: 4px; }
.peek .pv .at { position: absolute; left: 0; width: 3px; height: 18px; background: var(--ph-accent); border-radius: 0 2px 2px 0; }
.peek.wait .pv canvas, .peek.wait .pv .at { visibility: hidden; }
.peek .pu { display: none; padding: 8px 10px 0; word-break: break-all; color: var(--ph-link, var(--ph-accent)); }
.peek.url .pv { display: none; }
.peek.url .pu { display: block; }
.peek .pf { display: flex; justify-content: space-between; gap: 12px; padding: 5px 10px; font-size: 12px; color: var(--ph-fg2); border-top: 1px solid var(--ph-border); }
.peek.url .pf { border-top: 0; }
.peek .pf .pin { all: unset; cursor: pointer; display: inline-flex; align-items: center; margin-left: 8px; color: var(--ph-fg2); }
.peek .pf .pin svg { width: 14px; height: 14px; }
.peek .pf .pin:hover { color: var(--ph-fg); }
.peek .pf .pin:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 2px; border-radius: 2px; }
.peek .pf { align-items: center; }
.peek .pf .go { all: unset; margin-left: auto; cursor: pointer; padding: 1px 10px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); color: var(--ph-fg); }
.peek .pf .go:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.peek .pf .go:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 2px; }
.peek .pf .go[hidden] { display: none; }
/* a window kept open (viewer.ts peekPin): over the pages, where it was put, a bar to move it by */
.peek.pinned { z-index: 5; box-shadow: 0 10px 32px rgb(0 0 0 / 28%); resize: both; display: flex; flex-direction: column; min-width: 240px; min-height: 120px; max-width: calc(100% - 8px); }
/* (resized: what it shows fills it, scrolling inside) */
.peek.pinned .pv { flex: 1 1 auto; height: auto; min-height: 60px; }
.peek.pinned.cite .pc { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; }
.peek.pinned .pc .cs { flex: 1 1 auto; max-height: none; min-height: 0; }
/* (the corner kept for resizing: the footer's button clear of it) */
.peek.pinned .pf { flex: none; padding-right: 28px; }
.peek.pinned .pf .pin { display: none; }
.peek .ph { display: flex; align-items: center; gap: 8px; padding: 4px 6px 4px 10px; cursor: move; user-select: none; touch-action: none; font-size: 12px; font-weight: 600; color: var(--ph-fg); background: var(--ph-hover, rgb(0 0 0 / 5%)); border-bottom: 1px solid var(--ph-border); }
.peek .ph .t { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.peek .ph .x { all: unset; cursor: pointer; width: 20px; height: 20px; display: grid; place-items: center; border-radius: var(--ph-radius); color: var(--ph-fg2); font-size: 15px; }
.peek .ph .x:hover { background: var(--ph-border); color: var(--ph-fg); }
.peek .pc { display: none; }
.peek.cite .pv { display: none; }
.peek.cite .pc { display: block; }
.peek .pc .mh { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 6px 12px; font-size: 12px; color: var(--ph-fg2); border-bottom: 1px solid var(--ph-border); }
.peek .pc .mh button { all: unset; cursor: pointer; padding: 2px 10px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); color: var(--ph-fg); }
.peek .pc .mh button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.peek .pc .cs { position: relative; max-height: 300px; overflow-y: auto; overscroll-behavior: contain; }
/* an entry, the same one or many: the label, title and key; who, when and where; its links */
.peek .pc .ce { padding: 7px 12px 7px 9px; border-left: 3px solid transparent; }
.peek .pc .ce + .ce { border-top: 1px solid var(--ph-border); }
.peek .pc .ce.here { border-left-color: var(--ph-accent); background: color-mix(in srgb, var(--ph-accent) 7%, transparent); }
.peek .pc .hd { display: flex; align-items: baseline; gap: 6px; min-width: 0; }
.peek .pc .hd .lab { flex: none; font-size: 12px; font-weight: 600; color: var(--ph-accent); }
.peek .pc .hd .ti { flex: 1; min-width: 0; font-size: 13px; font-weight: 600; line-height: 1.3; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.peek .pc .hd code { flex: none; font: 10.5px/1.3 ui-monospace, "DM Mono", monospace; padding: 0 4px; border-radius: 3px; background: var(--ph-hover, rgb(0 0 0 / 6%)); color: var(--ph-fg2); }
.peek .pc .mt { margin-top: 1px; font-size: 12px; line-height: 1.35; color: var(--ph-fg2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.peek .pc .lk { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0 5px; margin-top: 2px; font-size: 11.5px; }
.peek .pc .lk > i { font-style: normal; color: var(--ph-fg2); }
.peek .pc .lk button { all: unset; cursor: pointer; color: var(--ph-link, var(--ph-accent)); }
.peek .pc .lk button:hover { text-decoration: underline; }
.peek .pc .lk button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; border-radius: 2px; }
/* where a jump landed, lit for a moment (viewer.ts flash): only its opacity animates */
.slot .phx-flash { position: absolute; z-index: 2; pointer-events: none; border-radius: 3px; background: color-mix(in srgb, var(--ph-accent) 22%, transparent); box-shadow: 0 0 0 1.5px color-mix(in srgb, var(--ph-accent) 55%, transparent); animation: phx-flash 1.5s ease-out forwards; }
@keyframes phx-flash { 0%, 35% { opacity: 1; } 100% { opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .slot .phx-flash { animation-duration: 2.5s; } }
/* Back, after a link followed: over the pages' bottom left corner */
.pback { position: absolute; z-index: 4; left: 12px; bottom: 12px; display: flex; align-items: stretch; border-radius: var(--ph-pill); background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); box-shadow: 0 3px 12px rgb(0 0 0 / 18%); font: 12.5px/1 var(--ph-font); }
.pback[hidden] { display: none; }
.pback button { all: unset; cursor: pointer; padding: 7px 10px; }
.pback .go { display: flex; gap: 6px; align-items: center; padding-left: 12px; font-weight: 600; }
.pback .go .ar { font-size: 14px; }
.pback .x { color: var(--ph-fg2); border-left: 1px solid var(--ph-border); padding: 7px 9px; }
.pback { overflow: visible; }
.pback .more, .pback .fw { border-left: 1px solid var(--ph-border); color: var(--ph-fg2); font-weight: 600; }
.pback .more[hidden], .pback .fw[hidden] { display: none; }
.pback.noback .go { display: none; }
.pback.noback .fw { border-left: 0; padding-left: 12px; }
.pback .go { border-radius: var(--ph-pill) 0 0 var(--ph-pill); }
.pback .x { border-radius: 0 var(--ph-pill) var(--ph-pill) 0; }
.pback .stops { position: absolute; left: 0; bottom: calc(100% + 6px); width: max-content; min-width: 300px; max-width: min(460px, 80vw); max-height: 260px; overflow-y: auto; padding: 4px 0; background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 20px rgb(0 0 0 / 20%); }
.pback .stops[hidden] { display: none; }
.pback .stops button { display: grid; grid-template-columns: 1fr; gap: 1px; line-height: 1.35; width: 100%; box-sizing: border-box; padding: 6px 12px; text-align: left; font-size: 12.5px; }
.pback .stops button b { font-weight: 600; white-space: nowrap; }
.pback .stops button b small { margin-left: 5px; font-weight: 400; color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.pback .stops button span { color: var(--ph-fg2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.pback button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.pback button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: -2px; }
.win.dark-pdf .peek .pv canvas, .win.docked.pdf-dark .peek .pv canvas { filter: ${DARK_PDF_FILTER}; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: transparent; }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.stage.offline .slot { opacity: .4; pointer-events: none; }
.slot .mark { position: absolute; pointer-events: none; border-radius: 2px; background: rgb(255 214 0 / 45%); mix-blend-mode: multiply;
  box-shadow: 0 0 0 1px rgb(232 172 0 / 80%); transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; }
/* (the text selected on the pages, one block a line: viewer.ts drawSelection) */
.slot .mark.seltext { background: rgb(0 100 255 / 25%); mix-blend-mode: normal; border-radius: 0; }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); box-shadow: none; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); box-shadow: 0 0 0 1px rgb(150 100 0 / 90%); }
.slot .culprit { position: absolute; pointer-events: none; }
/* (the marks along the pages' scrollbar: the problems' places and the diff's changes) */
.smarks { position: absolute; right: 0; top: 0; bottom: 0; width: 8px; z-index: 4; pointer-events: none; }
.smarks[hidden] { display: none; }
.smarks i { position: absolute; right: 1px; width: 6px; height: 3px; margin-top: -1px; border-radius: 1px; pointer-events: auto; cursor: pointer; background: var(--ph-warning, #c77700); transition: transform .1s; }
/* (a target larger than the tick: it is 3 px tall) */
.smarks i::before { content: ""; position: absolute; inset: -4px -3px; }
.smarks i:hover { transform: scale(1.5, 2); }
.smarks i.error { background: var(--ph-danger, #d93025); }
.smarks i.change { background: var(--ph-accent, #1d3e72); }
.smarks i.rv-highlight { background: #c9a800; } .smarks i.rv-note { background: #2f6fd1; } .smarks i.rv-comment { background: #e08a1e; }
.smarks i.rv-change { background: #1a7f37; } .smarks i.rv-dim { opacity: .4; }
.phx-card[data-key] { cursor: pointer; }
/* (an overfull box's bar at its line's end, where pdflatex's draft rule goes, drawn by the viewer only, in the warning's
   amber, as its tooltip and the underfull's outline: draft black read as a broken block, and vanished on a dark page; an underfull's hollow) */
.slot .culprit.rule::after { display: none; }
.slot .culprit.rule.overfull { background: rgb(199 119 0 / 85%); border-radius: 1px; }
.slot .culprit.rule.underfull { box-shadow: inset 0 0 0 1.5px #c77700; }
/* (a culprit hovered: its problem's message, a small tooltip under the line) */
.ctip { position: absolute; z-index: 8; max-width: min(420px, calc(100% - 16px)); display: flex; gap: 6px; align-items: flex-start; padding: 5px 9px;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius);
  box-shadow: 0 3px 12px rgb(0 0 0 / 25%); font-size: 12px; line-height: 1.35; pointer-events: none; overflow-wrap: anywhere; }
.ctip .icon { margin-top: 1px; }
.ctip.error .icon { color: var(--ph-danger); } .ctip.warning .icon { color: var(--ph-warning); }
.ctip .txt { min-width: 0; text-align: left; }
.ctip code, .ctip .ex { font: 11.5px/1.4 ui-monospace, "DejaVu Sans Mono", monospace; }
.ctip .ex { margin: 3px 0 0; white-space: pre; overflow: hidden; text-overflow: ellipsis; opacity: .85; }
.ctip .ex mark { background: none; color: inherit; font-weight: 600; text-decoration: underline wavy var(--ph-danger); text-underline-offset: 3px; }
.ctip.warning .ex mark { text-decoration-color: var(--ph-warning); }
.ctip .at { margin-top: 2px; font-size: 11px; color: var(--ph-fg2); }
.stage.onculprit, .stage.onculprit * { cursor: pointer !important; }
/* (a problem's culprit: a wavy line along its glyphs' foot, as an editor underlines one; no fill, the text stays readable) */
.slot .culprit::after { content: ""; position: absolute; left: 0; right: 0; bottom: -2px; height: 3px; background: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23d93025' stroke-width='1'/%3E%3C/svg%3E") repeat-x left bottom / 6px 3px; }
.slot .culprit.warning::after { background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23c77700' stroke-width='1'/%3E%3C/svg%3E"); }
.slot .peer { position: absolute; pointer-events: none; border-radius: 2px; background: hsl(var(--hue) 70% 50% / 22%); box-shadow: inset 0 -1.5px 0 hsl(var(--hue) 70% 45%); }
/* (the name as large as the line it marks, so it zooms with the page; readable when far out, never a headline when close) */
.slot .peer-tag { width: auto; height: auto; transform: translateY(-100%); padding: 0 .4em; border-radius: .25em .25em .25em 0; background: hsl(var(--hue) 70% 42%);
  box-shadow: none; color: #fff; font: 600 clamp(7px, calc(var(--fs, .012) * 100cqw), 22px)/1.35 var(--ph-font); white-space: nowrap; z-index: 2; }
.slot .peer-tag[data-file] { pointer-events: auto; cursor: pointer; }
.slot .peer-tag[data-file]:hover { background: hsl(var(--hue) 70% 34%); }

/* strips over the pages: plain, in the host's colours */
/* (over the pages' scroll, not in it: in it, a strip went with the pages when they scrolled sideways) */
.stagebox { position: relative; flex: 1; min-height: 0; min-width: 0; display: flex; flex-direction: column; }
.strips { position: absolute; top: 0; left: 0; right: 0; z-index: 2; pointer-events: none; }
.strip { pointer-events: auto; display: none; padding: 6px 12px; text-align: left; font-size: 12px;
  background: var(--ph-menu); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-warning)); }
.stage.stale ~ .strips .banner, .approx.on { display: block; }
.gone { --sev: var(--ph-danger); font-weight: 600; }
.stage.offline ~ .strips .gone { display: flex; align-items: baseline; gap: 8px; }
.gone .x { margin-left: auto; flex: none; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; padding: 0 2px; }
.offpill { display: none; position: absolute; top: 8px; right: 14px; z-index: 3; padding: 1px 8px; border-radius: var(--ph-pill); font-size: 12px;
  background: var(--ph-menu); color: var(--ph-danger); border: 1px solid var(--ph-border); pointer-events: none; }
.win.offline-quiet .offpill { display: block; }

/* before any page: the first page's sheet, the opening playing on it, the step under it */
.opening { position: absolute; z-index: 3; pointer-events: none; left: 0; top: 0; width: 0; height: 0;
  transition: left .3s ease, top .3s ease, width .3s ease, height .3s ease; }
.opening .sheet { position: absolute; inset: 0; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); container-type: inline-size; }
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
@keyframes phx-slide { from { transform: translateX(-100%); } to { transform: translateX(333.4%); } }
.win.dark-pdf .steps, .win.docked.pdf-dark .steps { color: #a3a9b4; }
.win.dark-pdf .steps .what, .win.docked.pdf-dark .steps .what { color: #e4e6ea; }
.win.dark-pdf .steps .track, .win.docked.pdf-dark .steps .track { background: #3d4047; }
.opening.in .steps { opacity: 0; }
/* (an opening that says the steps itself, the animation's own part) */
.opening.owns .steps { display: none; }
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

/* live: a heading typed in the editor, the page's heading following it, letter by letter, and how long that took */
.tip .live .ed { position: absolute; left: 6px; top: 8px; bottom: 8px; width: 50%; padding: 6px 7px; background: var(--ph-menu); border: 1px solid var(--ph-border);
  border-radius: var(--ph-radius); font: 10px/14px ui-monospace, "DejaVu Sans Mono", monospace; white-space: nowrap; overflow: hidden; }
.tip .live .ed .k { color: var(--ph-link); }
.tip .live .ed i { display: block; height: 3px; margin: 7px 0 0; border-radius: 1px; background: var(--ph-border); }
.tip .live .ty { display: inline-block; vertical-align: bottom; overflow: hidden; white-space: nowrap; max-width: 0; animation: phx-type 5s steps(7, end) infinite; }
.tip .live .ed .ty { --to: 7ch; }
.tip .live .car { display: inline-block; width: 1px; height: 11px; vertical-align: -1px; background: var(--ph-fg); animation: phx-caret 1s steps(1) infinite; }
.tip .live .pg { right: 8px; top: 8px; width: 38%; bottom: -8px; }
.tip .live .pg b { display: block; margin: 2px 0 6px; font: 700 10px/1.2 "Noto Serif", Georgia, serif; color: #1b222c; white-space: nowrap; }
.tip .live .pg .ty { --to: 4.4em; animation-delay: .12s; }
.tip .live .ms { position: absolute; right: 10px; bottom: 6px; display: flex; align-items: center; gap: 2px; padding: 1px 6px; border-radius: 9px;
  font: 600 10px/14px var(--ph-font); color: #fff; background: rgb(27 34 44 / 85%); opacity: 0; animation: phx-ms 5s infinite; }
@keyframes phx-type { 0%, 10% { max-width: 0; } 50%, 88% { max-width: var(--to); } 100% { max-width: 0; } }
@keyframes phx-caret { 50% { opacity: 0; } }
@keyframes phx-ms { 0%, 14% { opacity: 0; } 20%, 86% { opacity: 1; } 94%, 100% { opacity: 0; } }

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
/* Settings: the panel's own rows and the host's, one dialog in every host, a tab per group */
.settings { inset: 12px auto auto 50%; translate: -50% 0; width: min(560px, calc(100% - 24px)); max-height: calc(100% - 24px); padding: 0; gap: 0; overflow: hidden; }
.settings > header { padding: 12px 14px 0; }
/* (the tabs wrap onto a second line when they don't fit: scrolled sideways, with no scrollbar, the last ones were out of reach) */
.settings .stabs { display: flex; flex-wrap: wrap; gap: 2px; padding: 6px 10px 0; border-bottom: 1px solid var(--ph-border); }
.settings .stabs button { all: unset; cursor: pointer; white-space: nowrap; padding: 6px 8px 7px; font-size: 13px; color: var(--ph-fg2); border-bottom: 2px solid transparent; margin-bottom: -1px; }
.settings .stabs button:hover { color: var(--ph-fg); }
.settings .stabs button[aria-selected="true"] { color: var(--ph-fg); font-weight: 600; border-bottom-color: var(--ph-accent); }
.settings .stabs button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: -2px; }
.settings .sbody { overflow: auto; padding: 4px 0 8px; }
.settings .srow { display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "lab ctl" "help help" "err err"; align-items: center; column-gap: 16px; padding: 10px 16px; }
.settings .srow + .srow { border-top: 1px solid var(--ph-border); }
.settings .srow .lab { grid-area: lab; font-weight: 600; min-width: 0; }
.settings .srow .ctl { grid-area: ctl; justify-self: end; }
.settings .srow .help { grid-area: help; margin-top: 2px; font-size: 12px; line-height: 1.4; color: var(--ph-fg2); }
.settings .srow .help:empty { display: none; }
.settings .srow.off .lab, .settings .srow.off .ctl { opacity: .55; }
.settings .srow .err { grid-area: err; margin-top: 4px; font-size: 12px; color: var(--ph-danger); }
.settings .srow.picks { grid-template-areas: "lab ctl" "help help" "picks picks" "err err"; }
.settings .tiles { grid-area: picks; display: grid; grid-template-columns: repeat(auto-fill, minmax(92px, 1fr)); gap: 12px 10px; margin-top: 10px; }
.settings .pick { all: unset; cursor: pointer; display: grid; gap: 6px; min-width: 0; }
.settings .pick:disabled { cursor: default; }
.settings .pick .lk { position: relative; display: block; aspect-ratio: 210 / 297; overflow: hidden; border-radius: 2px; background: #fff; box-shadow: 0 0 0 1px var(--ph-border); opacity: .5; transition: opacity .15s, box-shadow .15s; }
.settings .pick .lk.dark { background: ${DARK_PAGE}; }
.settings .pick:hover .lk { opacity: .8; }
.settings .pick[aria-checked="true"] .lk { opacity: 1; box-shadow: 0 0 0 2px var(--ph-accent); }
.settings .pick .lk .pg, .settings .pick .lk .sheet { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.settings .pick .lk .sheet { background: #fff; container-type: inline-size; }
.settings .pick .lk.dark .sheet { background: ${DARK_PAGE}; }
.settings .pick .lk .sheet.in { background: none; }
.settings .pick .lk .phx-open { position: absolute; inset: 0; width: 100%; height: 100%; }
.settings .pick .nm { display: flex; align-items: center; gap: 6px; font-size: 12px; line-height: 1.3; color: var(--ph-fg2); min-width: 0; }
.settings .pick[aria-checked="true"] .nm { color: var(--ph-fg); }
.settings .pick .box { flex: none; width: 12px; height: 12px; border-radius: 3px; box-shadow: inset 0 0 0 1.5px var(--ph-fg2); }
.settings .pick[aria-checked="true"] .box { box-shadow: none; background: var(--ph-accent) url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12'%3E%3Cpath d='M2.5 6.2 5 8.6 9.6 3.6' fill='none' stroke='%23fff' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E") center / 100% no-repeat; }
.settings .pick:focus-visible .lk { outline: 2px solid var(--ph-link); outline-offset: 2px; }
/* Colour lines' tab: the presets as swatches, the strength's slider, Custom's two pickers, the sample */
.settings .srow.swatches { grid-template-areas: "lab lab" "help help" "picks picks" "err err"; }
.settings .swatches .tiles { grid-template-columns: repeat(auto-fill, minmax(76px, 1fr)); gap: 8px; }
.settings .sw { all: unset; cursor: pointer; display: grid; gap: 5px; justify-items: center; font-size: 12px; color: var(--ph-fg2); }
.settings .sw .art { display: grid; gap: 3px; width: 100%; box-sizing: border-box; padding: 7px 8px; border-radius: var(--ph-radius); background: #fff; box-shadow: 0 0 0 1px var(--ph-border); }
.settings .sw .art.dark { background: ${DARK_PAGE}; }
.settings .sw .art i { display: block; height: 4px; border-radius: 2px; }
.settings .sw .art i:last-child { width: 60%; }
.settings .sw[aria-pressed="true"] { color: var(--ph-fg); font-weight: 600; }
.settings .sw[aria-pressed="true"] .art { box-shadow: 0 0 0 2px var(--ph-accent); }
.settings .sw:focus-visible .art { outline: 2px solid var(--ph-link); outline-offset: 2px; }
.settings .sw:disabled { cursor: default; opacity: .55; }
.settings input[type="range"] { width: 180px; accent-color: var(--ph-accent); }
.settings .cols { display: inline-flex; gap: 8px; }
.settings .cols input { width: 34px; height: 24px; padding: 0 2px; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); background: var(--ph-menu); cursor: pointer; }
.settings .srow.custom { grid-template-areas: "lab lab" "help help" "picks picks"; }
.settings .wsample { grid-area: picks; margin-top: 8px; padding: 12px 16px; border-radius: var(--ph-radius); box-shadow: 0 0 0 1px var(--ph-border); background: #fff; color: #000; isolation: isolate; overflow: hidden; text-align: left; font: 13px/1.45 Georgia, "Times New Roman", serif; }
.settings .wsample.dark { background: #282828; color: #e1e1e1; }
.settings .wsample span { position: relative; display: block; width: fit-content; white-space: nowrap; }
.settings .wsample span.h { font-weight: 700; font-size: 15px; margin-bottom: 4px; }
.settings .wsample span i { position: absolute; inset: 0 -2px; mix-blend-mode: lighten; pointer-events: none; }
.settings .wsample.dark span i { mix-blend-mode: darken; }
.settings .srow .val { color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.settings select { font: inherit; font-size: 12px; color: var(--ph-fg); background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 3px 6px; max-width: 220px; }
.settings .btn { padding: 3px 12px; font-size: 12px; white-space: nowrap; max-width: 220px; overflow: hidden; text-overflow: ellipsis; }
.settings .seg { display: inline-flex; gap: 2px; padding: 2px; border-radius: var(--ph-pill); background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.settings .seg button { all: unset; cursor: pointer; padding: 3px 11px; border-radius: var(--ph-pill); font-size: 12px; line-height: 1.4; white-space: nowrap; color: var(--ph-fg); }
.settings .seg button:hover:not([aria-pressed="true"]):not(:disabled) { background: var(--ph-border); }
.settings .seg button[aria-pressed="true"] { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.settings .seg button:disabled { cursor: default; }
.settings .switch { all: unset; cursor: pointer; position: relative; width: 30px; height: 18px; border-radius: 9px; background: var(--ph-border); transition: background .12s; }
.settings .switch i { position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: #fff; box-shadow: 0 1px 2px rgb(0 0 0 / 25%); transition: left .12s; }
.settings .switch[aria-checked="true"] { background: var(--ph-accent); }
.settings .switch[aria-checked="true"] i { left: 14px; }
.settings .switch:disabled { cursor: default; }
.settings .switch:focus-visible, .settings .seg button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }

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
  .tip .live .ty { max-width: none; } .tip .live .ms { opacity: 1; }
  .tip .cmp .del { color: #b91c1c; text-decoration: line-through 2px #b91c1c; } .tip .cmp .add { max-width: 6em; }
  .tip .cmp .tag, .tip .cmp .v1, .tip .tim .mpop { opacity: 1; } .tip .cmp .v0 { opacity: 0; } .tip .tim .mpop i { width: var(--w); }
  .tip .reb .mmenu, .tip .drk .mmenu, .tip .drk .mmenu b { opacity: 1; } .tip .reb .pick, .tip .drk .pick { background: var(--ph-hover); }
  .tip .drk .pg { background: ${DARK_PAGE}; } .tip .drk .pg i { background: #a9aeb6; }
}
`;
