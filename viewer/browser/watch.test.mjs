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
import { Browser, SHELF, Watch, project, sleep, git } from "./lib.mjs";

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
  // (a new session on the target resolved again, its first build a clean one: numbered from 1)
  const again = w.out.split("a clean rebuild was asked: a new session\n");
  assert.equal(again.length, 2, `the new session said: ${w.out}`);
  assert.match(again[1], /^ok:\s+1\s+clean build/m);
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
  // (the picker marks a range: the commit clicked is From, To the working tree; then Compare)
  await t.click("#phx-pick .row[data-i='0']");
  await t.until(`!$("#phx-pick .ask .yes").disabled`, "the pair marked");
  await t.click("#phx-pick .ask .yes");
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
  const r = spawnSync(process.env.PHITEX_BIN ?? "phitex", ["build", ...SHELF, "--no-synctex", main], { cwd: copy, env: { ...process.env, HOME: home } });
  if (r.status !== 0) throw new Error(`cold build: ${r.stderr}`);
  return readFileSync(join(copy, main.replace(/\.tex$/, ".pdf")));
}

test("a figure or a file of any extension replaced on disk rebuilds, as a full build makes it (images, unlisted extensions)", async () => {
  const { spawnSync } = await import("node:child_process");
  const { writeFileSync, copyFileSync, renameSync } = await import("node:fs");
  const { dirname, basename } = await import("node:path");
  // (ImageMagick 7's magick, else 6's convert: an Ubuntu runner has only convert)
  const im = spawnSync("magick", ["-version"]).status === 0 ? "magick" : "convert";
  // (drawn beside it, then renamed over it: a figure replaced whole, as a
  // plotting tool's save does. Drawn in place, ImageMagick 6 on the CI runner
  // left the watch a file it closed before it was whole, and that build
  // failed reading it (issue #36); this test is about a replaced figure)
  const magick = (args, out) => {
    const tmp = join(dirname(out), `.tmp-${Date.now()}-${basename(out)}`);
    const r = spawnSync(im, [...args, tmp]);
    if (r.status !== 0) throw new Error(`${im} failed: ${r.error ?? r.stderr}`);
    renameSync(tmp, out);
  };
  const tex = doc("Figures.\n\n\\includegraphics{figpng}\n\\includegraphics{figjpg}\n\\includegraphics[width=3cm]{figpdf}\n\n\\input{notes.md}", "\\usepackage{graphicx}\n");
  const dir = project({ "main.tex": tex, "notes.md": "Notes one.\n" });
  // (the PDF figure made elsewhere, copied in: its own build's files stay out of the project)
  const side = project({ "inc.tex": doc("Inner text A.") });
  const figure = (text) => {
    writeFileSync(join(side, "inc.tex"), doc(text));
    spawnSync(process.env.PHITEX_BIN ?? "phitex", ["build", ...SHELF, "--no-synctex", "inc.tex"], { cwd: side, env: { ...process.env, HOME: home } });
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

test("the problems' kinds filter the list, the page's marks and the scrollbar's, kept for the document (item 1, kinds)", async () => {
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
  // (the filters are this document's, kept in the browser: not the user's phitex.toml, 98d9b0888)
  assert.match(await t.q(`JSON.stringify(Object.entries(localStorage).filter(([k]) => k.startsWith("phitex.kinds.hidden.")))`), /\\"overfull\\"/);
  assert.ok(!existsSync(join(fresh, ".config", "phitex", "phitex.toml")) || !/problems-hidden/.test(readFileSync(join(fresh, ".config", "phitex", "phitex.toml"), "utf8")), "not in phitex.toml");
  // (the same document reloaded: the choice held)
  await t.go(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0`, "the pages after a reload");
  await problems(t);
  await t.until(`$("#diags [data-kind='overfull']")?.getAttribute("aria-pressed") === "false"`, "overfull still off");
  await w.stop();
  // (another document: its own filters, every kind shown)
  const { w: w2, t: t2 } = await open({ "main.tex": doc(body + "\n\nOther.") }, { at: fresh });
  await problems(t2);
  await t2.until(`$("#diags [data-kind='overfull']")?.getAttribute("aria-pressed") === "true"`, "overfull shown in another document");
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
  assert.match(await t.q(`JSON.stringify(Object.entries(localStorage).filter(([k]) => k.startsWith("phitex.kinds.hidden.")))`), /\\"reference\\"/, "kept for this document");
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
  // (Settings: the watch's switch, a choice and the panel's own, all written to the user's phitex.toml)
  await sleep(300);
  await t.click("#b-more");
  await t.click("#x-settings");
  await t.until(`!!$("#settings [data-group='Build']")`, "the settings");
  await t.click("#settings [data-id='follow'] [data-v='selection']");
  await t.click("#settings [data-group='Build']");
  await t.click("#settings [data-id='watch-write'] .switch");
  await t.click("#settings [data-group='Appearance']");
  await t.click("#settings [data-id='dark-pdf'] [data-v='on']");
  await t.click("#settings [data-group='Preview']");
  const end = Date.now() + 5000;
  const toml = join(fresh, ".config", "phitex", "phitex.toml");
  const all = (x) => /watch-write = true/.test(x) && /follow = "selection"/.test(x) && /dark-pdf = "on"/.test(x);
  while (!(existsSync(toml) && all(readFileSync(toml, "utf8"))) && Date.now() < end) await sleep(50);
  const written = readFileSync(toml, "utf8");
  assert.match(written, /watch-write = true/);
  assert.match(written, /follow = "selection"/);
  assert.match(written, /dark-pdf = "on"/);
  await t.until(`$(".win").classList.contains("dark-pdf")`, "the dark pages applied");
  // (a project's setting is shown, never set from the page)
  await t.until(`!!$("#settings [data-id='w-engine'] .val") && !$("#settings [data-id='w-engine'] button, #settings [data-id='w-engine'] select")`, "the engine shown, not set");
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

test("a link's window zooms: Ctrl+wheel and a trackpad's pinch at the pointer, two fingers, its magnifiers; pinned, it keeps its place and zoom, dragged to pan (peek zoom)", async () => {
  const fig = "\\begin{figure}[t]\\centering\\fbox{\\parbox{0.8\\linewidth}{\\tiny tiny figure text \\par\\rule{2cm}{1cm}}}\\caption{A figure}\\label{f}\\end{figure}";
  const { w, t } = await open({ "main.tex": doc(`${fig}\n\nSee Figure~\\ref{f}. ${"Words around it. ".repeat(40)}`, "\\usepackage{hyperref}") });
  await waitBuild(w, 1);
  // (the reference's number, hovered: its window)
  const at = await t.until(`(() => { for (const e of $$(".phx-text text")) { const s = e.textContent, i = s.indexOf("See Figure 1"); if (i < 0) continue; const x = e.getExtentOfChar(i + 11), p = new DOMPoint(x.x + x.width / 2, x.y + x.height / 2).matrixTransform(e.getScreenCTM()); return [p.x, p.y]; } return null; })()`, "the reference on the page");
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at[0], y: at[1] });
  await t.until(`$(".peek:not([hidden]) .zl")?.textContent === "100%" && !$(".peek").classList.contains("wait")`, "the link's window, drawn, at 100%");
  const peek = (sel = ".peek:not([hidden])") =>
    t.q(`(() => { const el = $(${JSON.stringify(sel)}), pv = el.querySelector(".pv"), c = pv.querySelector("canvas"), v = pv.getBoundingClientRect(); const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; let ink = 0; for (let i = 0; i < d.length; i += 16) if (d[i] + d[i + 1] + d[i + 2] < 450) ink++; return { z: el.querySelector(".zl").textContent, pv: [v.x, v.y, v.width, v.height], scroll: [pv.scrollLeft, pv.scrollTop], canvas: [parseFloat(c.style.left), parseFloat(c.style.top)], ink, vv: visualViewport.scale, pages: $("#viewer")?.style.cssText ?? "" }; })()`);
  let s = await peek();
  const pages = s.pages;
  assert.ok(s.ink > 20, "the window drawn");
  // (a mouse's notch with Ctrl: the window by a step of 25%, not the browser's page, not the pages)
  const wheel = (dy, x, y) => t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x, y, deltaX: 0, deltaY: dy, modifiers: 2 });
  await wheel(-100, s.pv[0] + s.pv[2] / 2, s.pv[1] + s.pv[3] / 2);
  await sleep(150);
  s = await peek();
  assert.equal(s.z, "125%");
  assert.equal(s.vv, 1, "the browser's page not zoomed");
  assert.equal(s.pages, pages, "the pages' zoom not changed");
  // (pinned: where it was scrolled, drawn (a moved element loses its scroll: it was blank))
  const before = s.scroll;
  await t.click(".peek:not([hidden]) [data-pin]");
  await t.until(`!!$(".peek.pinned")`, "the pinned window");
  await sleep(200);
  s = await peek(".peek.pinned");
  assert.deepEqual(s.scroll, before, "pinned, its scroll kept");
  assert.ok(s.ink > 20, `pinned, drawn (${s.ink})`);
  assert.deepEqual(s.canvas, s.scroll, "its canvas over the part in view");
  // (a trackpad's pinch: small pixel deltas with Ctrl, in proportion)
  for (let i = 0; i < 20; i++) await wheel(-6, s.pv[0] + 40, s.pv[1] + 40);
  await sleep(100);
  s = await peek(".peek.pinned");
  assert.ok(Math.abs(parseInt(s.z) - 125 * Math.exp(1.2)) < 6, `pinched to ${s.z}`);
  // (the magnifiers: a step of 25% in and out; the level between them back to 100%; never below it)
  await t.click(".peek.pinned [data-zoom='0']");
  assert.equal((await peek(".peek.pinned")).z, "100%");
  await t.click(".peek.pinned [data-zoom='1']");
  await t.click(".peek.pinned [data-zoom='1']");
  assert.equal((await peek(".peek.pinned")).z, "156%");
  await t.click(".peek.pinned [data-zoom='-1']");
  assert.equal((await peek(".peek.pinned")).z, "125%");
  for (let i = 0; i < 6; i++) await t.click(".peek.pinned [data-zoom='-1']");
  assert.equal((await peek(".peek.pinned")).z, "100%", "never below 100%");
  // (two fingers spread on a touch screen: their distance's ratio)
  s = await peek(".peek.pinned");
  const [mx, my] = [s.pv[0] + s.pv[2] / 2, s.pv[1] + s.pv[3] / 2];
  const two = (d) => [{ x: mx - d, y: my, id: 1 }, { x: mx + d, y: my, id: 2 }];
  await t.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: two(20) });
  for (let d = 24; d <= 60; d += 4) await t.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: two(d) });
  await t.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await sleep(150);
  s = await peek(".peek.pinned");
  assert.ok(Math.abs(parseInt(s.z) - 300) <= 10, `two fingers 40 to 120 px apart: ${s.z}`);
  assert.equal(s.vv, 1, "the browser's page not zoomed");
  // (zoomed, a drag pans it)
  const s0 = s.scroll;
  await t.drag(mx, my, mx + 60, my + 40);
  await sleep(150);
  s = await peek(".peek.pinned");
  assert.deepEqual([Math.round(s0[0] - s.scroll[0]), Math.round(s0[1] - s.scroll[1])], [60, 40], "dragged 60, 40: panned as much");
  assert.ok(s.ink > 20, "drawn where it panned to");
  assert.deepEqual(t.errors, []);
  await w.stop();
});

/** Text of `n` sentences, its words drawn from a fixed list by a seeded generator (every line its own: no two lines of the page alike). */
function prose(n, seed) {
  const words = "incremental typesetting rebuilds only what an edit touches dependency records which macros registers fonts node read change invalidates exactly readers hyphenation interword glue page builder decisions replayed deterministically output identical conventional sources nevertheless characteristically unpredictable implementations occasionally misrepresent responsibilities measure everything twice before believing anything column baseline justification typographical considerations notwithstanding experimental evaluation demonstrates substantially improved latency across representative workloads".split(" ");
  const rnd = (k) => ((seed = (seed * 1103515245 + 12345) % 2147483648), seed % k);
  return Array.from({ length: n }, (_, i) => {
    const ws = Array.from({ length: 12 + rnd(8) }, () => words[rnd(words.length)]);
    if (i % 3 === 0) ws.splice(3, 0, `\\emph{${words[rnd(words.length)]}}`);
    if (i % 3 === 1) ws.splice(4, 0, `$x_{${i}}^2 + \\alpha$`);
    if (i % 4 === 2) ws.splice(2, 0, `${words[rnd(words.length)]}~\\cite{k}`);
    return ws.join(" ") + ".";
  }).join("\n");
}

test("a note made in one column of a two-column paper is drawn in that column, as selected, its handles at its ends, after a reload too; a handle dragged past its line keeps to the column; pins numbered as the list (notes in two columns)", async () => {
  const { execFileSync } = await import("node:child_process");
  const at = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
  execFileSync(process.env.PHITEX_BIN ?? "phitex", ["config", "experimental-review", "on"], { env: { ...process.env, HOME: at, XDG_CONFIG_HOME: join(at, ".config") } });
  // (a paragraph with a footnote, a float between it and the next, the column
  // full: the selection's bytes hold the footnote's and the float's text, set
  // at the column's foot and atop the other column)
  const body = `${prose(14, 3)}\n\n${prose(9, 5)}\\footnote{Footnote text typeset at the foot of the column.}\n\\begin{figure}[t]\\centering\\fbox{Figure}\\caption{A caption typeset where the float lands.}\\end{figure}\n\n${prose(9, 7)}\n\n${prose(40, 11)}\n\\begin{thebibliography}{1}\\bibitem{k} D. Knuth. 1984.\\end{thebibliography}`;
  const { w, t } = await open({ "main.tex": `\\documentclass[twocolumn]{article}\n\\begin{document}\n${body}\n\\end{document}\n` }, { at });
  await t.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 1700, deviceScaleFactor: 1, mobile: false });
  // (the pages' layout at rest: the first page's box the same for 400 ms. A
  // resize, or the review's side opening, fits the pages to the narrower stage
  // a moment later; a place measured before it is not where the page is
  // after, and a press there falls on the text, not on a handle)
  const settled = async () => {
    let last = null, same = 0;
    for (const end = Date.now() + 20000; same < 2; await sleep(200)) {
      assert.ok(Date.now() < end, "the pages' layout at rest");
      const now = await t.q(`(() => { const b = $(".slot")?.getBoundingClientRect(); return b ? [b.left, b.top, b.width, b.height].map(Math.round).join() : null; })()`);
      same = now && now === last ? same + 1 : 0;
      last = now;
    }
  };
  await t.until(`$$(".slot text").length > 50`, "the first page's text");
  await settled();
  // (places on the first page as fractions of it: the review's side opening
  // narrows the pages; `F` maps a screen box to them)
  const F = `const S = $(".slot").getBoundingClientRect(); const F = (b) => [(b.left - S.left) / S.width, (b.right - S.left) / S.width, (b.top - S.top) / S.height, (b.bottom - S.top) / S.height];`;
  // (a line of its text: `col` "L" or "R", the `li`th from the top, its `ci`th
  // character (from the end if negative): the screen point at its left edge or
  // right, `f` the same as a fraction of the page; `mid` where the right column starts)
  const line = (col, li, ci, right = false) =>
    t.q(`(() => { $(".slot").scrollIntoView({ block: "start" }); ${F} const all = [...$(".slot").querySelectorAll("text")].filter((x) => x.textContent.length > 30).map((x) => [x, x.getBoundingClientRect()]); const mid = Math.min(...all.map(([, b]) => b.left).filter((l) => l > S.left + S.width / 2)); const ls = all.filter(([, b]) => (${JSON.stringify(col)} === "L") === (b.left < mid)).sort((a, b) => a[1].top - b[1].top); const [x] = ls.at(${li}) ?? []; if (!x) return null; const n = x.textContent.length, off = ${ci} < 0 ? n + ${ci} : ${ci}; const r = document.createRange(); r.setStart(x.firstChild, off); r.setEnd(x.firstChild, off + 1); const b = r.getBoundingClientRect(); const px = ${right} ? b.right - 1 : b.left + 1, py = (b.top + b.bottom) / 2; return { x: px, y: py, f: [(px - S.left) / S.width, (py - S.top) / S.height], mid: (mid - S.left) / S.width, hyphen: x.textContent.trimEnd().endsWith("-") }; })()`);
  // (from the paragraph with the footnote to the one after the float, all in the left column)
  const first = await line("L", 18, 4), last = await line("L", 30, -5, true);
  assert.ok(first && last && last.y > first.y, "the lines are there");
  const mid = first.mid;
  await t.drag(first.x, first.y, last.x, last.y);
  await sleep(400);
  const blocks = await t.q(`(() => { ${F} return $$(".mark.seltext").map((m) => F(m.getBoundingClientRect())); })()`);
  assert.ok(blocks.length >= 10 && blocks.every(([, r]) => r < mid), `the selection in the left column (it starts at ${mid}): ${JSON.stringify(blocks)}`);
  await t.until(`$(".rv-pop [data-k='note']")`, "the note's button");
  await t.click(".rv-pop [data-k='note']");
  await t.until(`$(".rv-compose textarea")`, "the note's box");
  await t.q(`(() => { const a = $(".rv-compose textarea"); a.value = "Two columns"; a.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  await t.click(".rv-compose [data-save]");
  const marks = () =>
    t.until(`(() => { ${F} const ms = $$(".slot .rv.n-note:not(.rv-pin):not(.rv-h)").map((m) => F(m.getBoundingClientRect())); const hs = $$(".slot .rv-h").map((h) => { const b = h.getBoundingClientRect(), f = F(b); return { end: h.classList.contains("end"), x: (f[0] + f[1]) / 2, top: f[2], bottom: f[3], sx: b.left + b.width / 2, sy: (b.top + b.bottom) / 2, on: b.top >= 0 && b.bottom <= innerHeight }; }); return ms.length && hs.length === 2 ? { ms, hs } : null; })()`, "the note's marks and its two handles");
  const top = Math.min(...blocks.map((b) => b[2])), bottom = Math.max(...blocks.map((b) => b[3]));
  const rows = (bs) => new Set(bs.map((b) => Math.round((b[2] + b[3]) * 200))).size;
  const near = (h, p) => Math.abs(h.x - p.f[0]) < 0.008 && h.top <= p.f[1] && h.bottom >= p.f[1];
  const check = async (what) => {
    const { ms, hs } = await marks();
    // (every mark in the left column, between the selection's first and last
    // lines: not the footnote's text at the column's foot, nor the float's in
    // the right column, nor a bar across both columns)
    assert.ok(ms.every(([, r, mt, mb]) => r < mid && mt >= top - 0.004 && mb <= bottom + 0.004), `${what}: the marks where the selection was (${top}-${bottom}, left of ${mid}): ${JSON.stringify(ms)}`);
    assert.ok(Math.abs(rows(ms) - rows(blocks)) <= 2, `${what}: a mark a selected line (${rows(ms)} rows, the selection ${rows(blocks)})`);
    const s = hs.find((h) => !h.end), e = hs.find((h) => h.end);
    assert.ok(near(s, first), `${what}: the start handle at the first character: ${JSON.stringify(s)} vs ${JSON.stringify(first.f)}`);
    assert.ok(near(e, last), `${what}: the end handle at the last character: ${JSON.stringify(e)} vs ${JSON.stringify(last.f)}`);
    assert.ok(s.on && e.on, `${what}: both handles on screen`);
    return e;
  };
  await settled();
  await check("saved");
  const notes = JSON.parse(w.read(".phitex/notes.json"));
  const note = (notes.notes ?? notes).find((n) => n.thread?.[0]?.text === "Two columns");
  assert.doesNotMatch(note.anchor.quote, /^\s/, "the anchor starts at the first character selected");
  // (a reload: the note drawn again from its anchor alone, selected by a click on its mark)
  await t.go(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0 && $$(".slot .rv.n-note").length > 0`, "the note's marks after a reload", 120000);
  await sleep(800);
  await line("L", 0, 0);
  const m = await t.q(`(() => { const b = $(".slot .rv.n-note:not(.rv-pin)").getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; })()`);
  await t.mouse(m[0], m[1]);
  // (the note selected opens the review's side: the pages fitted again)
  await settled();
  await check("after a reload");
  // (the end handle dragged two lines down, past its line's end into the gap
  // between the columns: the note ends on that line, in the left column)
  const to = await line("L", 32, -1, true);
  const gap = await t.q(`(() => { const S = $(".slot").getBoundingClientRect(); return S.left + ${mid} * S.width - 3; })()`);
  // (the handle where it is now, after line()'s scroll)
  const e = await t.q(`(() => { const b = $(".slot .rv-h.end").getBoundingClientRect(); return { sx: b.left + b.width / 2, sy: (b.top + b.bottom) / 2 }; })()`);
  await t.drag(e.sx, e.sy, gap, to.y, 16);
  for (const end = Date.now() + 20000; ; await sleep(100)) {
    const n = JSON.parse(w.read(".phitex/notes.json"));
    if ((n.notes ?? n).find((x) => x.id === note.id).anchor.end > note.anchor.end) break;
    assert.ok(Date.now() < end, "the note's end moved");
  }
  await sleep(800);
  const { ms, hs } = await marks();
  const moved = hs.find((h) => h.end);
  // (snapped to its word's end: a word a break hyphenated ends on the next line)
  const below = await line("L", 33, 0);
  const on = to.hyphen ? below : to;
  assert.ok(moved.x < mid && moved.top <= on.f[1] && moved.bottom >= on.f[1], `the end on the line it was let go beside (its word's end), in the left column: ${JSON.stringify(moved)} vs ${JSON.stringify(on.f)} (hyphenated: ${to.hyphen})`);
  assert.ok(ms.every(([, r]) => r < mid), `the marks still in the left column: ${JSON.stringify(ms)}`);
  // (two notes more, written after it: one in the right column, one above it;
  // the pins number them as they come in the pages, as the list does, not
  // by their posts (each had one: every pin said 1))
  for (const [a, b, text] of [[["R", 4, 2], ["R", 6, -3, true], "Right"], [["L", 3, 2], ["L", 5, -3, true], "Above"]]) {
    await settled();
    const p = await line(...a), q = await line(...b);
    await t.drag(p.x, p.y, q.x, q.y);
    await t.until(`$(".rv-pop [data-k='note']")`, "the note's button");
    await t.click(".rv-pop [data-k='note']");
    await t.until(`$(".rv-compose textarea")`, "the note's box");
    await t.q(`(() => { const a = $(".rv-compose textarea"); a.value = ${JSON.stringify(text)}; a.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
    await t.click(".rv-compose [data-save]");
    await t.until(`$$(".slot .rv-pin").length === ${text === "Right" ? 2 : 3}`, `${text}'s pin`);
  }
  await sleep(500);
  const all = JSON.parse(w.read(".phitex/notes.json"));
  const id = (text) => (all.notes ?? all).find((n) => n.thread?.[0]?.text === text).id;
  const pins = await t.q(`Object.fromEntries($$(".slot .rv-pin").map((p) => [p.dataset.id, p.textContent]))`);
  assert.deepEqual([pins[id("Above")], pins[id("Two columns")], pins[id("Right")]], ["1", "2", "3"], `the pins in reading order: ${JSON.stringify(pins)}`);
  const cards = await t.q(`$$(".rside .card.note").map((c) => [c.dataset.id, c.querySelector(".num")?.textContent])`);
  assert.deepEqual(cards, [[id("Above"), "1"], [id("Two columns"), "2"], [id("Right"), "3"]], "the list in the same order, the same numbers");
  assert.deepEqual(t.errors, []);
  await w.stop();
});
