// The watch's viewer in a real browser against a real `phitex watch`: what a
// person clicks and sees (lib.mjs). Each test reproduces a bug a user
// reported, then checks it stays fixed.
//
//   PHITEX_BIN=target/fastdev/phitex node --test --test-concurrency=1 viewer/browser/watch.test.mjs
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Browser, Watch, project, sleep, git } from "./lib.mjs";

// (the PDFs dated alike: an edited one compared with a full build)
process.env.SOURCE_DATE_EPOCH = "0";
process.env.FORCE_SOURCE_DATE = "1";
const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
let browser;
before(async () => (browser = await Browser.start()));
after(async () => (Watch.killAll(), await browser?.stop()));

const doc = (body, pre = "") => `\\documentclass{article}\n${pre}\\begin{document}\n${body}\n\\end{document}\n`;

/** A watch on `files` and a tab on its viewer, its first page in. */
async function open(files, { args = [], pages = true, at = home, env = {} } = {}) {
  const dir = project(files);
  const w = new Watch(dir, { home: at, args, env });
  const t = await browser.tab(await w.url());
  if (pages) await t.until(`+$("#pgn")?.textContent > 0`, "the first page", 240000);
  return { dir, w, t };
}
const builds = (w) => (w.out.match(/^(ok|failed|✓|✗):?\s+\d+/gm) ?? []).length;
const waitBuild = async (w, n, ms = 60000) => {
  const end = Date.now() + ms;
  while (builds(w) < n) {
    if (Date.now() > end) throw new Error(`no build ${n}: ${w.out}`);
    await sleep(50);
  }
  await sleep(300);
};

test("Clean rebuild builds again from nothing; Rebuild reads the files again (item 4)", async () => {
  const { w, t } = await open({ "main.tex": doc("Hello.") });
  await waitBuild(w, 1);
  await t.click("#go");
  await waitBuild(w, 2);
  assert.match(w.out, /2\s+asked in the viewer/);
  await t.click("#b-rec");
  await t.click("#r-clean");
  await waitBuild(w, 3);
  assert.match(w.out, /3\s+asked: a cold build/);
  await w.stop();
});

test("a build with errors says its time; the badge counts the last build's errors only (items 9, 27)", async () => {
  const good = doc("Hello."), bad = doc("Hello \\nosuch.");
  const { w, t } = await open({ "main.tex": good });
  await waitBuild(w, 1);
  w.write("main.tex", bad);
  await waitBuild(w, 2);
  await t.until(`/Built with 1 error · \\d/.test($("#st").innerText.replace(/\\n/g, " "))`, "the time beside the errors");
  assert.equal(await t.q(`$("#phx-bar .new").textContent`), "1 error");
  w.write("main.tex", good);
  await waitBuild(w, 3);
  await t.until(`$("#phx-bar .new").hidden`, "the badge gone once the error is fixed");
  await w.stop();
});

test("LaTeX's errors have their line and a mark on the page; a missing package says which (items 16, 17)", async () => {
  {
    const { w, t } = await open({ "main.tex": doc("Hi.\n\\begin{itemize}\n\\end{itemize}\nMore text after it.") });
    await t.until(`$$(".slot .culprit").length > 0`, "the underline");
    const d = await t.q(`$("#diags").innerText`);
    assert.match(d, /Something's wrong--perhaps a missing \\item/);
    assert.match(d, /main\.tex:5/);
    await w.stop();
  }
  {
    const { w, t } = await open({ "main.tex": doc("Hi.", "\\usepackage{nosuchpackagehere}\n") }, { pages: false });
    await t.until(`/nosuchpackagehere\\.sty' not found/.test($("#diags").innerText)`, "the missing file named");
    await w.stop();
  }
});

test("a .bib edited while watching is read again (item 17)", async () => {
  const main = doc("Hi \\cite{x}.\n\n\\bibliographystyle{plain}\n\\bibliography{refs}");
  const { w, t } = await open({ "main.tex": main, "refs.bib": "" });
  await t.until(`/didn't find a database entry/.test($("#diags").innerText)`, "the warning");
  w.write("refs.bib", "@book{x,title={T},author={A B},year=2000,publisher={P}}\n");
  await t.until(`!/didn't find a database entry/.test($("#diags").innerText)`, "the entry found");
  await w.stop();
});

test("Next change goes to the page of that change; the style popover floats over the pages (items 24, 22)", async () => {
  const para = (i, w) => `Paragraph ${i} ${w} ` + "lorem ipsum dolor sit amet ".repeat(30) + "\n\n";
  const body = (w) => Array.from({ length: 30 }, (_, i) => para(i, i % 10 === 0 ? w : "same")).join("");
  const dir = project({ "main.tex": doc(body("alpha")) });
  git(dir, "init", "-q");
  git(dir, "add", ".");
  git(dir, "commit", "-qm", "one");
  const w = new Watch(dir, { home });
  w.write("main.tex", doc(body("beta")));
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent > 3`, "the pages");
  await t.click("#b-compare");
  await t.until(`$$("#phx-pick .row").length > 0`, "the commits");
  await t.click("#phx-pick .row[data-i='0']");
  await t.until(`/3 changes/.test($("#diffbar").innerText)`, "the diff", 120000);
  const pages = [];
  for (let i = 0; i < 3; i++) {
    await t.click("#diffbar button[data-a='next']");
    await sleep(1200);
    pages.push(Number(await t.q(`$("#pgin").value`)));
  }
  assert.ok(pages[0] < pages[1] && pages[1] < pages[2], `each change on its own page, further on: ${pages}`);
  await t.click("#diffbar button[data-a='settings']");
  const box = await t.q(`(() => { const b = $("#diffset").getBoundingClientRect(), s = $("#stage").getBoundingClientRect(); return { pos: getComputedStyle($("#diffset")).position, w: b.width, stage: s.width }; })()`);
  assert.equal(box.pos, "absolute", "a popover, not a column beside the pages");
  await t.click("#diffset button[data-bars]");
  await t.until(`$("#diffset button[data-bars]").getAttribute("aria-checked") === "true"`, "the change bars on");
  await w.stop();
});

test("a double-click finds its source after comment lines were added above it (item 18)", async () => {
  const text = (pre) => doc(`${pre}Zebra words here.\n\nSecond paragraph Quokka.`);
  const { w, t } = await open({ "main.tex": text("") });
  const dbl = async () => {
    await t.q(`(() => { const i = $("#findq"); $("#find").hidden = false; i.value = "Quokka"; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
    const at = await t.until(`(() => { const m = $(".mark.fnd"); if (!m) return null; const r = m.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`, "the word found");
    const from = w.out.length;
    await t.mouse(at.x, at.y, 1);
    await t.mouse(at.x, at.y, 2);
    const end = Date.now() + 10000;
    while (!/source: main\.tex:\d+/.test(w.out.slice(from)) && Date.now() < end) await sleep(50);
    return /source: (main\.tex:\d+:\d+)/.exec(w.out.slice(from))?.[1];
  };
  const before = await dbl();
  w.write("main.tex", text("%\n%\n%\n%\n%\n"));
  await waitBuild(w, 2);
  await sleep(1500);
  const after = await dbl();
  const [, l0, c0] = before.split(":"), [, l1, c1] = after.split(":");
  assert.equal(+l1, +l0 + 5, `${before} -> ${after}`);
  assert.equal(c1, c0, `${before} -> ${after}`);
  await w.stop();
});

test("Esc clears the highlights on the pages (item 21)", async () => {
  const { w, t } = await open({ "main.tex": doc("Find Quokka here.") });
  await t.q(`(() => { const i = $("#findq"); $("#find").hidden = false; i.value = "Quokka"; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  await t.until(`$$(".mark").length > 0`, "a highlight");
  await t.click("#stage");
  await t.key("Escape", "Escape", 27);
  await t.until(`$$(".mark").length === 0`, "the highlight gone");
  await w.stop();
});

test("the watch writes nothing into the folder unless asked (item 26)", async () => {
  const { dir, w } = await open({ "main.tex": doc("Hello.") });
  await waitBuild(w, 1);
  w.write("main.tex", doc("Hello again."));
  await waitBuild(w, 2);
  await w.stop();
  assert.deepEqual(readdirSync(dir).sort(), ["main.tex"]);
  const { dir: d2, w: w2 } = await open({ "main.tex": doc("Hello.") }, { args: ["--write"] });
  await waitBuild(w2, 1);
  await w2.stop();
  assert.ok(existsSync(join(d2, "main.pdf")) && existsSync(join(d2, "main.synctex.gz")), readdirSync(d2).join(" "));
});

test("the same document is served at the same address; its tab joins again by itself (item 5)", async () => {
  const dir = project({ "main.tex": doc("Hello.") });
  const w = new Watch(dir, { home });
  const url = await w.url();
  const t = await browser.tab(url);
  await t.until(`+$("#pgn")?.textContent > 0`, "the first page");
  await w.stop();
  await t.until(`$("#phx-bar").innerText.includes("not connected")`, "the tab sees the watch go");
  const w2 = new Watch(dir, { home });
  assert.equal(await w2.url(), url);
  await t.until(`!$("#phx-bar").innerText.includes("not connected")`, "the tab back on its own", 15000);
  await w2.stop();
});

test("Skip on the tips holds on another port and another document (item 6)", async () => {
  // (a user who has seen no tip yet: every tab above saw the first one)
  const fresh = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
  const { w, t } = await open({ "main.tex": doc("One.") }, { at: fresh });
  await t.until(`!$("#tip").hidden`, "the tips");
  await t.click("#tip button[data-t='skip']");
  await t.until(`$("#tip").hidden`, "the tips gone");
  await sleep(500);
  await w.stop();
  assert.match(readFileSync(join(fresh, ".config", "phitex", "phitex.toml"), "utf8"), /tips-seen = \[/);
  // (another folder: another document, another port, another origin for the browser's storage)
  const { w: w2, t: t2 } = await open({ "main.tex": doc("Two, elsewhere.") }, { at: fresh });
  await sleep(2500);
  assert.equal(await t2.q(`$("#tip").hidden`), true, "no tips again");
  await w2.stop();
});

/** `phitex build` of a copy of `dir`'s sources (its outputs left out): the cold PDF an edited one must equal. */
async function coldPdf(dir, main = "main.tex") {
  const { cpSync } = await import("node:fs");
  const { spawnSync } = await import("node:child_process");
  const copy = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-cold-"));
  cpSync(dir, copy, { recursive: true, filter: (p) => !/\.(pdf|log|aux|synctex\.gz)$/.test(p) || /fig/.test(p) });
  const r = spawnSync(process.env.PHITEX_BIN ?? "phitex", ["build", "--no-synctex", main], { cwd: copy, env: { ...process.env, HOME: home } });
  if (r.status !== 0) throw new Error(`cold build: ${r.stderr}`);
  return readFileSync(join(copy, main.replace(/\.tex$/, ".pdf")));
}

test("a figure or a file of any extension replaced on disk rebuilds, as a full build makes it (images, unlisted extensions)", async () => {
  const { spawnSync } = await import("node:child_process");
  const { writeFileSync, copyFileSync } = await import("node:fs");
  const magick = (args, out) => spawnSync("magick", [...args, out]);
  const tex = doc("Figures.\n\n\\includegraphics{figpng}\n\\includegraphics{figjpg}\n\\includegraphics[width=3cm]{figpdf}\n\n\\input{notes.md}", "\\usepackage{graphicx}\n");
  const dir = project({ "main.tex": tex, "notes.md": "Notes one.\n" });
  // (the PDF figure made elsewhere, copied in: its own build's files stay out of the project)
  const side = project({ "inc.tex": doc("Inner text A.") });
  const figure = (text) => {
    writeFileSync(join(side, "inc.tex"), doc(text));
    spawnSync(process.env.PHITEX_BIN ?? "phitex", ["build", "--no-synctex", "inc.tex"], { cwd: side, env: { ...process.env, HOME: home } });
    copyFileSync(join(side, "inc.pdf"), join(dir, "figpdf.pdf"));
  };
  magick(["-size", "40x30", "xc:red"], join(dir, "figpng.png"));
  magick(["-size", "40x30", "xc:blue"], join(dir, "figjpg.jpg"));
  // (a PDF figure with fonts in it: a page PhiTeX built)
  figure("Inner text A.");
  const w = new Watch(dir, { home, args: ["--write", "--exact"] });
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0`, "the first page");
  await waitBuild(w, 1);
  const edits = [
    // (the same size, another picture: the page's boxes are the same, its picture not)
    ["figpng.png", () => magick(["-size", "40x30", "xc:green"], join(dir, "figpng.png"))],
    ["figjpg.jpg", () => magick(["-size", "40x30", "xc:yellow"], join(dir, "figjpg.jpg"))],
    ["figpdf.pdf", () => figure("Inner text B.")],
    ["notes.md", () => writeFileSync(join(dir, "notes.md"), "Notes two, edited.\n")],
  ];
  let n = 1;
  for (const [name, edit] of edits) {
    const before = readFileSync(join(dir, "main.pdf"));
    edit();
    n++;
    await waitBuild(w, n);
    await sleep(800);
    const now = readFileSync(join(dir, "main.pdf"));
    assert.notDeepEqual(now, before, `${name}: the PDF changed`);
    assert.match(w.out, new RegExp(`\\b${n}\\s+${name.replace(".", "\\.")}\\s+pages? \\d`), `${name}: the build says why, and that a page changed`);
    await t.until(`$("#phx-bar").innerText.includes("build ${n} ")`, `the viewer told of build ${n}`);
    assert.ok(now.equals(await coldPdf(dir)), `${name}: the edited PDF is a full build's`);
  }
  await w.stop();
});

/** The problems' popover open (an error may have opened it already). */
async function problems(t) {
  await t.until(`$("#b-probs") && !$("#b-probs").hidden`, "the problems' button");
  await sleep(300);
  if (await t.q(`$("#logs").hidden`)) await t.click("#b-probs");
}
/** The rectangle of `word` on the pages, as find marks it (find closed after). */
async function wordAt(t, word) {
  await t.q(`(() => { const i = $("#findq"); $("#find").hidden = false; i.value = ${JSON.stringify(word)}; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  const r = await t.until(`(() => { const m = $(".mark.fnd"); if (!m) return null; const r = m.getBoundingClientRect(); return { l: r.left, r: r.right, t: r.top, b: r.bottom, n: $$(".mark.fnd").length }; })()`, `${word} found`);
  await t.q(`(() => { const i = $("#findq"); i.value = ""; i.dispatchEvent(new Event("input", { bubbles: true })); $("#find").hidden = true; return 1; })()`);
  await t.until(`$$(".mark.fnd").length === 0`, "find's marks gone");
  return r;
}
/** The selected text on the pages, and its blocks (`.mark.seltext`) by line: [top, count, gaps]. */
const selected = (t) =>
  t.q(`(() => { const s = (R.getSelection?.() ?? getSelection()); const bs = $$(".mark.seltext").map((m) => m.getBoundingClientRect()); const lines = new Map(); for (const b of bs) { const k = Math.round(b.top); (lines.get(k) ?? lines.set(k, []).get(k)).push(b); } return { text: s?.toString() ?? "", blocks: bs.length, lines: [...lines.values()].map((l) => l.length) }; })()`);

test("a selection is one block a line; its text has its spaces, no space inside a kerned word, and ligatures as letters (selection)", async () => {
  const words = "Quokka office efficient AVAWATER Toyota waffle fluffy official ".repeat(3);
  const { w, t } = await open({ "main.tex": doc(`Start ${words}\n\n\\parbox{4cm}{Narrow incomprehensibilities characteristically unconstitutional Zebra end.}`) });
  const a = await wordAt(t, "Quokka"), b = await wordAt(t, "Zebra");
  // (across words and lines, into the narrow box with its hyphenated breaks)
  await t.drag(a.l + 1, (a.t + a.b) / 2, b.r - 1, (b.t + b.b) / 2);
  await sleep(300);
  const s = await selected(t);
  assert.ok(s.lines.length >= 3, `several lines: ${JSON.stringify(s.lines)}`);
  assert.ok(s.lines.every((n) => n === 1), `one block a line: ${JSON.stringify(s.lines)}`);
  const flat = s.text.replace(/\s+/g, " ");
  assert.match(flat, /^Quokka office efficient AVAWATER Toyota waffle fluffy official Quokka/, `words and their spaces: ${JSON.stringify(s.text.slice(0, 120))}`);
  assert.doesNotMatch(s.text, /[ﬀﬁﬂﬃﬄ]/, "ligatures given as their letters");
  assert.match(s.text, /Zebra/);
  // (a hyphenated break: the word's halves around a hyphen, nothing lost)
  assert.match(s.text, /-\s*\n?/, `a hyphen at a break: ${JSON.stringify(s.text.slice(-160))}`);
  // (double-click: the word, one block)
  await t.key("Escape", "Escape", 27);
  const o = await wordAt(t, "Toyota");
  await t.mouse((o.l + o.r) / 2, (o.t + o.b) / 2, 1);
  await t.mouse((o.l + o.r) / 2, (o.t + o.b) / 2, 2);
  await sleep(300);
  const d = await selected(t);
  assert.equal(d.text.trim(), "Toyota", "a double-click selects the word");
  assert.equal(d.blocks, 1, "the word one block");
  // (find: one mark a match)
  assert.equal((await wordAt(t, "efficient")).n, 3, "three matches, three marks");
  await w.stop();
});

test("the problems' kinds filter the list, the page's marks and the scrollbar's, kept in phitex.toml (item 1, kinds)", async () => {
  const fresh = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
  const body = "{\\hsize=2cm \\noindent Incomprehensibilities characteristically.\\par}\n\nThen \\nosuchcommandhere here.";
  const { w, t } = await open({ "main.tex": doc(body) }, { at: fresh });
  await problems(t);
  await t.until(`!!$("#diags [data-kind='overfull']") && !!$("#diags [data-kind='error']")`, "the kinds' filters");
  await t.until(`$$(".slot .culprit.rule.overfull").length > 0`, "the overfull box's bar on the page");
  const marks0 = await t.q(`$$("#smarks i").length`);
  assert.ok(marks0 >= 2, `scrollbar marks: ${marks0}`);
  assert.match(await t.q(`$("#diags").innerText`), /Overfull/);
  await t.click("#diags [data-kind='overfull']");
  await t.until(`$("#diags [data-kind='overfull']")?.getAttribute("aria-pressed") === "false"`, "overfull off");
  assert.equal(await t.q(`$$(".slot .culprit.rule").length`), 0, "the bar gone from the page");
  assert.doesNotMatch(await t.q(`[...$$("#diags .phx-card, #diags li")].map((e) => e.innerText).join(" ")`), /Overfull/, "gone from the list");
  assert.ok((await t.q(`$$("#smarks i").length`)) < marks0, "fewer scrollbar marks");
  await sleep(500);
  assert.match(readFileSync(join(fresh, ".config", "phitex", "phitex.toml"), "utf8"), /problems-hidden = \[\s*"overfull"/);
  await w.stop();
  // (another document: the choice held)
  const { w: w2, t: t2 } = await open({ "main.tex": doc(body + "\n\nOther.") }, { at: fresh });
  await problems(t2);
  await t2.until(`$("#diags [data-kind='overfull']")?.getAttribute("aria-pressed") === "false"`, "overfull still off");
  await w2.stop();
});

test("LaTeX's warnings are problems: an undefined \\ref and \\cite are warnings of kind reference, underlined on the page, hidden by their kind's filter (item 16, warnings)", async () => {
  const fresh = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
  // (an error too, so that the list has two kinds and offers the filters)
  const { w, t } = await open({ "main.tex": doc("See \\ref{nosuchlabel} here and \\cite{nosuchkey}.\n\nThen \\nosuchcommandhere here.") }, { at: fresh });
  await problems(t);
  await t.until(`!!$("#diags [data-kind='reference']") && !!$("#diags [data-kind='error']")`, "the reference kind's filter");
  const text = await t.q(`$("#diags").innerText`);
  assert.match(text, /Reference `nosuchlabel' on page 1 undefined/);
  assert.match(text, /Citation `nosuchkey' on page 1 undefined/);
  assert.match(text, /main\.tex:3/, "the warnings' line");
  assert.match(text, /References \d+/, "counted under the reference kind");
  await t.until(`$$(".slot .culprit.warning").length >= 2`, "the warnings' underlines on the page");
  await t.until(`$$(".slot .culprit.error").length >= 1`, "the error's underline");
  await t.click("#diags [data-kind='reference']");
  await t.until(`$("#diags [data-kind='reference']")?.getAttribute("aria-pressed") === "false"`, "reference off");
  await t.until(`$$(".slot .culprit.warning").length === 0`, "the warnings' underlines gone");
  assert.ok((await t.q(`$$(".slot .culprit.error").length`)) >= 1, "the error's underline stays");
  assert.doesNotMatch(await t.q(`[...$$("#diags .phx-card, #diags li")].map((e) => e.innerText).join(" ")`), /undefined on page/, "gone from the list");
  await sleep(500);
  assert.match(readFileSync(join(fresh, ".config", "phitex", "phitex.toml"), "utf8"), /problems-hidden = \[\s*"reference"/);
  // (the label defined: the warnings go, the underlines with them)
  await t.click("#diags [data-kind='reference']");
  w.write("main.tex", doc("See \\ref{lbl} here\\label{lbl} and \\cite{nosuchkey}.\n\nThen \\nosuchcommandhere here."));
  await t.until(`!/Reference \`nosuchlabel'/.test($("#diags").innerText) && /Citation \`nosuchkey'/.test($("#diags").innerText)`, "the reference's warning gone, the citation's kept");
  await w.stop();
});

test("a problem's card shows its place on the pages (item 2); which kinds have an underline (batch 3)", async () => {
  const filler = Array.from({ length: 12 }, (_, i) => `Paragraph ${i} ` + "words fill the page ".repeat(40)).join("\n\n");
  const { w, t } = await open({ "main.tex": doc(`${filler}\n\nLate \\nosuchcommand here.`) });
  await t.until(`+$("#pgn")?.textContent > 1`, "two pages");
  await problems(t);
  await t.until(`!!$("#diags .phx-card, #diags li")`, "the card");
  await t.q(`($("#stage").scrollTop = 0, 1)`);
  await sleep(500);
  await t.click("#diags .phx-card li, #diags li");
  await t.until(`+$("#pgin").value > 1`, "the error's page shown");
  await w.stop();
  // (each kind as a real build reports it: whether its place has an underline)
  const kinds = {
    "missing citation": doc("Cite \\cite{nosuchkey} here."),
    "undefined reference": doc("See \\ref{nosuchlabel} here."),
    "missing file": doc("Include \\input{nosuchfile} here."),
    "font fallback": doc("Shape {\\fontshape{sc}\\bfseries Bold small caps} here."),
    "unclosed brace": doc("Open \\textbf{never closed here.\n\nMore."),
    "undefined command": doc("Bad \\nosuchcommand here."),
  };
  const got = {};
  for (const [kind, text] of Object.entries(kinds)) {
    const { w, t } = await open({ "main.tex": text }, { pages: false });
    await t.until(`$("#diags")?.innerText.trim().length > 0 || +$("#pgn")?.textContent > 0`, `${kind}: built`, 120000);
    // (the underline comes once the page is laid out: waited for, not long)
    const underline = await t.until(`$$(".slot .culprit").length > 0`, `${kind}: an underline`, 8000).then(() => true, () => false);
    got[kind] = { underline, ...(await t.q(`({ pages: +$("#pgn")?.textContent, said: $("#diags").innerText.replace(/\\s+/g, " ").slice(0, 160) })`)) };
    await w.stop();
  }
  console.log("underlines by kind:", JSON.stringify(got, null, 1));
  // (a kind whose build gives no page cannot be underlined: a missing file
  // and an unclosed brace stop the job; their places are in the card)
  for (const kind of ["missing citation", "undefined reference", "font fallback", "undefined command"]) assert.ok(got[kind].underline, `${kind} is underlined`);
  assert.match(got["missing file"].said, /nosuchfile\.tex' not found/);
});

test("the tab's title says the state; Settings writes the user's file; What's new and the times show; Copy for an AI has the source (items 12, 15, 7, 3, 11)", async () => {
  const fresh = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
  const good = doc("Hello."), bad = doc("Hello \\nosuch.");
  const { w, t } = await open({ "main.tex": good }, { at: fresh });
  await waitBuild(w, 1);
  await t.until(`/main\\.tex · Built/.test(document.title)`, "the title: built");
  // (an edit: the times' bars, first build and this edit)
  w.write("main.tex", doc("Hello there."));
  await waitBuild(w, 2);
  await t.until(`$("#st").innerText.startsWith("Built")`, "an edit's time, not called a first build");
  await t.click("#st");
  await t.until(`!!$("#m-time .tbars")`, "the times as bars");
  await t.key("Escape", "Escape", 27);
  // (an error: the title; its own Copy for an AI, its source line and nothing sent)
  await t.js(`(() => { window.__copied = null; Object.defineProperty(navigator, "clipboard", { value: { writeText: async (s) => void (window.__copied = s) }, configurable: true }); return 1; })()`);
  w.write("main.tex", bad);
  await waitBuild(w, 3);
  await t.until(`/1 error/.test(document.title)`, "the title: an error");
  await problems(t);
  await t.click("#diags .error .copyai, #diags .phx-error .copyai");
  const copied = await t.until(`window.__copied`, "copied");
  assert.match(copied, /Undefined control sequence/);
  assert.match(copied, /Hello \\nosuch/, "the source line");
  await t.key("Escape", "Escape", 27);
  w.write("main.tex", good);
  await waitBuild(w, 4);
  // (What's new: this version's entry, from the binary)
  await t.click("#b-more");
  await t.click("#x-tips");
  await t.until(`!$("#logview").hidden && /What's new in/.test($("#logview").innerText) && $("#logtext").textContent.trim().length > 0`, "the news");
  await t.key("Escape", "Escape", 27);
  // (Settings: a switch written to the user's phitex.toml)
  await sleep(300);
  await t.click("#b-more");
  await t.click("#x-settings");
  await t.until(`document.querySelector(".phx-settings input[data-k='watch-write']")`, "the settings");
  await t.click(".phx-settings input[data-k='watch-write']");
  const end = Date.now() + 5000;
  const toml = join(fresh, ".config", "phitex", "phitex.toml");
  while (!(existsSync(toml) && /watch-write = true/.test(readFileSync(toml, "utf8"))) && Date.now() < end) await sleep(50);
  assert.match(readFileSync(toml, "utf8"), /watch-write = true/);
  await w.stop();
  assert.equal(await t.until(`/not connected|Not connected/i.test(document.title) && document.title`, "the title: not connected").then((x) => /not connected/i.test(x)), true);
});

test("a double-click starts only the user's editor; a project's phitex.toml runs nothing (item 20)", async () => {
  const { chmodSync, writeFileSync, mkdirSync } = await import("node:fs");
  const run = async (toml, user) => {
    const fresh = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
    const marks = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-mark-"));
    const script = join(marks, "mark.sh");
    writeFileSync(script, `#!/bin/sh\ntouch "${marks}/ran"\n`);
    chmodSync(script, 0o755);
    const line = `editor = "${script} {file}"\n`;
    if (user) {
      mkdirSync(join(fresh, ".config", "phitex"), { recursive: true });
      writeFileSync(join(fresh, ".config", "phitex", "phitex.toml"), line);
    }
    const { w, t } = await open({ "main.tex": doc("Find Quokka here."), ...(toml ? { "phitex.toml": line } : {}) }, { at: fresh, env: { PHITEX_EDITOR: "", VISUAL: "", EDITOR: "" } });
    const a = await wordAt(t, "Quokka");
    await t.mouse((a.l + a.r) / 2, (a.t + a.b) / 2, 1);
    await t.mouse((a.l + a.r) / 2, (a.t + a.b) / 2, 2);
    await sleep(1500);
    await w.stop();
    return { ran: existsSync(join(marks, "ran")), out: w.out };
  };
  const p = await run(true, false);
  assert.equal(p.ran, false, "a project's editor ran");
  assert.match(p.out, /`editor` in a project's phitex\.toml is ignored/);
  assert.equal((await run(false, true)).ran, true, "the user's editor did not run");
});
