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

const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
let browser;
before(async () => (browser = await Browser.start()));
after(async () => (Watch.killAll(), await browser?.stop()));

const doc = (body, pre = "") => `\\documentclass{article}\n${pre}\\begin{document}\n${body}\n\\end{document}\n`;

/** A watch on `files` and a tab on its viewer, its first page in. */
async function open(files, { args = [], pages = true, at = home } = {}) {
  const dir = project(files);
  const w = new Watch(dir, { home: at, args });
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
  await t.click("#diffset button[data-v='changebar']");
  await t.until(`$("#diffset button[data-v='changebar']").getAttribute("aria-pressed") === "true"`, "the style picked");
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
