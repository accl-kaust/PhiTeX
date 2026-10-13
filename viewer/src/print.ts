// The viewer's look, the same in every host (phitex watch, Overleaf,
// VS Code): a proof sheet's, not an app's. Paper and ink, one red for the
// proofreader's marks (errors, changes, the cursor), hairline rules, square
// corners, crop marks at the pages' corners, numbers in tabular figures.
// The rules are in viewer/STYLE.md; this sheet comes last, over the
// panel's older one, and only these tokens give colours.

/** The tokens: light, then dark (the system's, or `.dark` on the window). */
export const TOKENS = `
  --paper: #f1f1ef; --paper-2: #e6e6e3; --sheet: #ffffff;
  --ink: #141414; --ink-2: #4a4a48; --ink-3: #8a8a86;
  --rule: #c9c9c5; --rule-2: #dededa;
  --mark: #d71920; --caution: #8a5a00;
  --sans: "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  --serif: "Latin Modern Roman", "CMU Serif", "Computer Modern", Georgia, serif;
`;
export const TOKENS_DARK = `
  --paper: #121212; --paper-2: #1c1c1c; --sheet: #ffffff;
  --ink: #e8e8e4; --ink-2: #a9a9a4; --ink-3: #6e6e6a;
  --rule: #3a3a38; --rule-2: #2a2a28;
  --mark: #ff4a3d; --caution: #d9a441;
`;

/** Crop marks around a page: eight hairlines, 10 px long, 4 px off its corners. */
const CROP = `
.slot::after { content: ""; position: absolute; inset: -14px; pointer-events: none;
  --c: var(--ink-3, #8a8a86);
  background:
    linear-gradient(var(--c), var(--c)) left 14px top 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) left 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px top 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) right 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) left 14px bottom 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) left 0 bottom 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px bottom 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) right 0 bottom 14px / 10px 1px;
  background-repeat: no-repeat; }
/* a page an edit changed: a change bar in the left margin, fading */
.slot::before { content: ""; position: absolute; left: -9px; top: 0; bottom: 0; width: 2px; background: var(--mark, #d71920);
  opacity: 0; pointer-events: none; }
.slot.changed::before { animation: phx-revised 1.6s linear; }
@keyframes phx-revised { 0%, 40% { opacity: 1; } 100% { opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .slot.changed::before { animation: none; } }
`;

/** The renderer's part (the pages), for any host: crop marks and change bars. */
export const PAGE_CSS = CROP;

export const PRINT_CSS = `
.win { ${TOKENS} }
@media (prefers-color-scheme: dark) { .win:not(.light) { ${TOKENS_DARK} color-scheme: dark; } }
.win.dark { ${TOKENS_DARK} color-scheme: dark; }
.pkgs .icon { color: var(--ink-2); }

.win, .win.docked { background: var(--paper); color: var(--ink); border: 1px solid var(--rule); border-radius: 0; box-shadow: none;
  font: 12px/1.45 var(--sans); font-variant-numeric: tabular-nums; }
.win.docked { border: 0; }
.win *:focus-visible { outline: 1px solid var(--mark); outline-offset: 1px; }

/* header: one thin bar, ink on paper, a hairline under it */
header, .win.docked header { height: 30px; padding: 0 6px 0 10px; gap: 6px; background: var(--paper); color: var(--ink);
  border-bottom: 1px solid var(--rule); }
.title { font: 600 11px var(--sans); letter-spacing: .08em; text-transform: uppercase; margin: 0 8px 0 0; }
.badge, .win.docked .badge { background: none; color: var(--ink-3); border-radius: 0; padding: 0; font: 400 11px var(--sans); font-variant: small-caps; letter-spacing: .04em; }
header > .icon:first-child { display: none; }
.chip, .win.docked .chip { background: none; border-radius: 0; padding: 0 6px; height: 30px; gap: 6px; font: 11px var(--mono); color: var(--ink-2);
  border-left: 1px solid var(--rule-2); border-right: 1px solid var(--rule-2); }
.dot { width: 6px; height: 6px; border-radius: 0; background: var(--ink-3); }
.chip.ok .dot { background: var(--ink); } .chip.warning .dot { background: var(--caution); } .chip.error .dot { background: var(--mark); }
.chip.busy .dot { animation: phx-blink .8s steps(2) infinite; }
@keyframes phx-blink { 50% { opacity: 0; } }
button.ib { border-radius: 0; width: 26px; height: 26px; color: var(--ink-2); }
header button.ib:hover, .bar button.ib:hover, .win.docked header button.ib:hover { background: var(--paper-2); color: var(--ink); }
button.ib[aria-pressed=true], .win.docked header button.ib[aria-pressed=true] { background: none; color: var(--mark); }
button.ib.txt { width: auto; padding: 0 8px; font: 10px var(--sans); letter-spacing: .1em; text-transform: uppercase; }
.win.no-footer > footer, .win.docked.own-header.no-footer > footer { display: none; }
.find { display: inline-flex; align-items: center; gap: 6px; margin-right: 8px; }
.find[hidden] { display: none; }
.find input { width: 180px; height: 20px; padding: 0 6px; font: 11px var(--mono); color: var(--ink); background: var(--sheet); border: 1px solid var(--ink);
  border-radius: 0; outline: none; }
.find input::placeholder { color: var(--ink-3); font-family: var(--sans); letter-spacing: .04em; }
#findn { font: 11px var(--mono); color: var(--ink-2); min-width: 52px; }
#findn.none { color: var(--mark); }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); outline: 0; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); outline: 1px solid #141414; }
.speedchip:empty { display: none !important; }
.pno { font: 11px var(--mono); color: var(--ink-2); min-width: 48px; }
.bar { background: var(--paper); color: var(--ink-2); border-bottom: 1px solid var(--rule); padding: 4px 10px; }
.bar label { font: 11px var(--sans); font-variant: small-caps; letter-spacing: .04em; }
select { font: 11px var(--mono); color: var(--ink); background: var(--paper); border: 1px solid var(--rule); border-radius: 0; padding: 1px 4px; }
button.btn { color: var(--ink); background: none; border: 1px solid var(--ink); border-radius: 0; padding: 2px 10px 2px 6px; font: 600 11px var(--sans);
  letter-spacing: .04em; text-transform: uppercase; }
button.btn:hover { background: var(--ink); color: var(--paper); }

/* the problems' line and list: rules, not cards */
.sum, .win.docked .sum, .win.docked.light .sum { background: var(--paper); color: var(--ink-2); border-bottom: 1px solid var(--rule); height: 24px; font: 11px var(--sans); }
.sum:hover, .win.docked .sum:hover, .win.docked.light .sum:hover { background: var(--paper-2); }
.sum .first, .win.docked .sum .first, .win.docked.light .sum .first { color: var(--ink); }
.sum .n.error, .win.docked .sum .n.error, .win.docked.light .sum .n.error { color: var(--mark); }
.sum .n.warning, .win.docked .sum .n.warning, .win.docked.light .sum .n.warning { color: var(--caution); }
.sum .n.info, .win.docked .sum .n.info, .win.docked.light .sum .n.info { color: var(--ink-2); }
.diags { background: var(--paper); border-bottom: 1px solid var(--ink); box-shadow: none; }
.diag { border-top: 1px solid var(--rule-2); font: 12px var(--sans); }
.diag[data-line]:hover { background: var(--paper-2); }
.diag.error .icon { color: var(--mark); } .diag.warning .icon { color: var(--caution); } .diag.info .icon { color: var(--ink-3); }
.diag .where { font: 11px var(--mono); color: var(--ink-3); }
.diag .detail, .folded .detail { font: 11px/1.4 var(--mono); background: none; border-left: 1px solid var(--rule); border-radius: 0; padding: 2px 8px; }

/* the stage: pages on paper, crop marks for a frame, no shadows */
.stage, .win.docked .stage { background: var(--paper-2); padding: 24px; }
.slot, .stage .slot, .win.docked .slot, .win.docked .stage .slot { margin: 28px auto; background: var(--sheet); box-shadow: none; border-radius: 0; }
.win.docked.pdf-dark .slot { box-shadow: none; }
.stage.stale .slot { opacity: .5; filter: grayscale(1); }
.banner, .approx { background: var(--paper); color: var(--caution); border-bottom: 1px solid var(--rule); font: 11px var(--sans); }
.slot .mark { border-radius: 0; background: rgb(255 214 0 / 45%); box-shadow: none; outline: 1px solid rgb(180 140 0 / 70%); animation: none; }

/* the loading card: a blank proof with crop marks, its steps in small caps, a rule filling */
.empty, .win.docked .empty, .win.docked.light .empty { color: var(--ink-2); padding: 48px 12px; font: 12px var(--sans); }
.load, .win.docked .load, .win.docked.light .load, .win:not(.docked) .load { max-width: 340px; color: var(--ink-2); position: relative; padding: 22px 24px;
  background: var(--sheet); color: #4a4a48; }
.load::after { content: ""; position: absolute; inset: -14px; pointer-events: none; --c: var(--ink-3);
  background:
    linear-gradient(var(--c), var(--c)) left 14px top 0 / 1px 10px, linear-gradient(var(--c), var(--c)) left 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px top 0 / 1px 10px, linear-gradient(var(--c), var(--c)) right 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) left 14px bottom 0 / 1px 10px, linear-gradient(var(--c), var(--c)) left 0 bottom 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px bottom 0 / 1px 10px, linear-gradient(var(--c), var(--c)) right 0 bottom 14px / 10px 1px;
  background-repeat: no-repeat; }
.load h3 { font: 400 17px/1.3 var(--serif); color: #141414; margin: 0 0 14px; }
.load .steps { gap: 0; margin: 0 0 16px; font: 10px var(--sans); letter-spacing: .1em; text-transform: uppercase; color: #8a8a86; }
.load .steps span { opacity: 1; gap: 0; }
.load .steps span::before { display: none; }
.load .steps span.ok { color: #4a4a48; }
.load .steps span.now { color: #d71920; font-weight: 600; }
.load .steps i { flex: 1; min-width: 12px; margin: 0 8px; height: 1px; background: #c9c9c5; opacity: 1; }
.load .bar, .load .bar.busy { height: 1px; border-radius: 0; background: #dededa; }
.load .bar i { background: #141414; border-radius: 0; transition: width .15s linear; }
.load .bar:not(.busy) i { background-image: none; animation: none; }
.load .bar.busy i { width: 20% !important; animation: phx-carriage 1.4s linear infinite; }
@keyframes phx-carriage { from { transform: translateX(-100%); } to { transform: translateX(500%); } }
.load .alive { font: 11px var(--mono); color: #4a4a48; margin-top: 10px; }
.load .alive::before { display: none; }
.load .count { font: 11px var(--mono); }
.load .names span { border-radius: 0; background: none; border: 1px solid #dededa; font: 10px var(--mono); color: #4a4a48; }
.load .names span.ok { color: #141414; }
.load .names span.ing { animation: none; border-color: #d71920; }
.load .note { font: 11px var(--sans); color: #8a8a86; }
.load .btns .btn, .report .btn, .diags .reportbtn { border-radius: 0; border: 1px solid currentColor; font: 600 11px var(--sans); text-transform: uppercase; letter-spacing: .04em; }

/* overlays: flat sheets with a rule, no blur, no shadow */
.pkgs, .warmup, .diffbar, .diffset, .keyhelp, .about, .report, .win.docked.sheet-open > .bar {
  border-radius: 0 !important; box-shadow: none !important; backdrop-filter: none !important;
  background: var(--paper) !important; color: var(--ink) !important; border: 1px solid var(--ink) !important; font-family: var(--sans); }
.pkgs { border-left: 2px solid var(--mark) !important; }
.pkgs .mini, .pkgs .mini i { border-radius: 0; height: 1px; } .pkgs .mini i { background: var(--ink); }
.warmup .dot, .diffbar .busy::before { border-radius: 0; width: 6px; height: 6px; background: var(--mark); animation: phx-blink .8s steps(2) infinite; }
.warmup.ready { background: var(--paper) !important; color: var(--ink) !important; }
.warmup .slow { color: var(--caution); }
.diffbar button, .diffset button { border-radius: 0; color: var(--ink); }
.diffbar button:hover:not(:disabled), .diffset button:hover { background: var(--paper-2); }
.diffbar button.cur, .diffset button[aria-pressed=true] { background: var(--ink); color: var(--paper); }
.diffbar .sep { background: var(--rule); }
.diffset h4 { color: var(--ink-3); }
.diffset .sample { border-radius: 0; font-family: var(--serif); }
.diffset .pair i { border-radius: 0; }
.keyhelp kbd { font: 11px var(--mono); background: none; border: 1px solid var(--rule); border-radius: 0; }
.about a { color: var(--mark); }
.about .about-foot { color: var(--ink-3); }

/* the time to the screen and who made this: set like a folio, not pills */
.speedchip, .win.docked .speedchip { height: auto; padding: 2px 6px; border-radius: 0; background: var(--paper); color: var(--ink);
  border: 1px solid var(--rule); box-shadow: none; font: 11px var(--mono); }
.speedchip.slow { color: var(--caution); background: var(--paper); }
.speedchip.flash { animation: phx-tick .4s linear; }
@keyframes phx-tick { from { border-color: var(--mark); } }
.win.docked .byline, .win.docked.light .byline { border-radius: 0; background: var(--paper); color: var(--ink-3); backdrop-filter: none;
  font: 10px/20px var(--sans); height: 20px; letter-spacing: .1em; text-transform: uppercase; border: 1px solid var(--rule-2); }
.win.docked .byline:hover { color: var(--ink); background: var(--paper); }

footer { background: var(--paper); color: var(--ink-3); border-top: 1px solid var(--rule); font: 11px var(--mono); height: 22px; }
footer .msg.err { color: var(--mark); }
.details { font: 11px var(--mono); color: var(--ink-2); border-top: 1px solid var(--rule); }
.side { background: var(--paper); color: var(--ink); border-right: 1px solid var(--rule); }
.win.collapsed, .win.collapsed header { border-radius: 0; }
`;
