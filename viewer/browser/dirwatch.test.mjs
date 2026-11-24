// `phitex watch DIR` in a real browser (lib.mjs): the folder's file manager,
// a document opened from it in a tab of its own, the viewer's Files column
// switching that tab to another document in place, quick open (Ctrl+P), and
// two documents side by side in two tabs. PHITEX_SHOTS=dir keeps a
// screenshot of each state there (light and dark, wide and narrow).
//
//   PHITEX_BIN=target/fastdev/phitex node --test --test-concurrency=1 viewer/browser/dirwatch.test.mjs
import { test, before, after, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Browser, Tab, Watch, project, sleep } from "./lib.mjs";

process.env.SOURCE_DATE_EPOCH = "0";
process.env.FORCE_SOURCE_DATE = "1";
const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
const shots = process.env.PHITEX_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
let browser;
before(async () => (browser = await Browser.start()));
after(async () => (Watch.killAll(), await browser?.stop()));
afterEach(async (t) => browser?.report(t));

const doc = (title, body) => `\\documentclass{article}\n\\title{${title}}\n\\begin{document}\n${body}\n\\end{document}\n`;

/** A screenshot of `tab` as `name` (PHITEX_SHOTS), in `scheme` at `width`×`height`. */
async function shot(tab, name, { scheme = "light", width = 1200, height = 800 } = {}) {
  if (!shots) return;
  await tab.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
  await tab.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: width < 600 });
  await sleep(250);
  await tab.shot(join(shots, `${name}.png`));
  await tab.send("Emulation.clearDeviceMetricsOverride");
  await tab.send("Emulation.setEmulatedMedia", { features: [] });
  await sleep(100);
}

/** The rows the file manager shows: name, kind, state. */
const rows = (t) => t.q(`$$(".fm-row").map((r) => ({ path: r.dataset.path, kind: r.className.match(/k-(\\w+)/)[1], state: r.querySelector(".fm-badge")?.textContent ?? null, current: r.getAttribute("aria-current") === "page" }))`);
const crumbs = (t) => t.q(`$(".fm-crumbs").innerText.replace(/\\s+/g, " ").trim()`);

/** A key with modifiers (2: Ctrl, 8: Shift, 1: Alt). */
async function press(tab, key, modifiers = 0, code = key, vk = 0) {
  const base = { key, code, windowsVirtualKeyCode: vk, modifiers };
  await tab.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
  await tab.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
}

/** The tab the browser opened last whose address matches `re`, attached. */
async function newTab(re, ms = 20000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const t = (await browser.tabs()).find((t) => re.test(t.url) && !browser.opened.some((o) => o.id === t.id));
    if (t) {
      const tab = await Tab.attach(t, "about:blank");
      browser.opened.push(tab);
      return tab;
    }
    await sleep(100);
  }
  throw new Error(`no new tab at ${re}`);
}

test("a folder's file manager; documents in tabs of their own, switched in place, found by name", async () => {
  const dir = project({
    "papers/alpha/main.tex": doc("Alpha", "Alpha.\\input{chapter}"),
    "papers/alpha/chapter.tex": "% !TEX root = main.tex\nThe chapter.\n",
    "papers/alpha/figure.png": "not a picture",
    "papers/alpha/notes.txt": "notes",
    "beta/main.tex": doc("Beta", "Beta."),
    "data/results.csv": "1,2\n",
    "empty/.keep": "",
    ".git/HEAD": "ref: refs/heads/main\n",
  });
  const w = new Watch(dir, { file: ".", home, args: ["-j", "2"] });
  const url = await w.url();
  const index = await browser.tab(url);
  await index.until(`$$(".fm-row").length >= 4`, "the folder's rows");
  // (every file and folder but hidden ones, folders first)
  assert.deepEqual((await rows(index)).map((r) => r.path), ["beta", "data", "empty", "papers"]);
  assert.equal(await crumbs(index), (await index.q(`$(".fm-crumb.root").textContent`)));
  assert.equal(await index.q(`$(".fm-up").disabled`), true, "no way above the root");
  await shot(index, "index-root-light");
  await shot(index, "index-root-dark", { scheme: "dark" });
  await shot(index, "index-root-narrow", { width: 390, height: 760 });
  // (a folder entered with a click, the next with the keyboard: ↓ to it, Enter)
  await index.click(`.fm-row[data-path="papers"]`);
  await index.until(`$(".fm-crumbs").innerText.includes("papers")`, "papers/ entered");
  await index.until(`$(".fm-row")?.dataset.path === "papers/alpha"`, "alpha listed");
  assert.match(await index.q(`location.hash`), /^#dir=papers$/, "the folder in the address");
  assert.ok((await index.q(`$(".fm-row[data-path='papers/alpha']").href`)).endsWith("#dir=papers%2Falpha"), "a folder is a link");
  await press(index, "ArrowDown", 0, "ArrowDown", 40);
  await index.until(`document.activeElement?.dataset?.path === "papers/alpha"`, "alpha focused");
  await index.key("Enter", "Enter", 13);
  await index.until(`$$(".fm-row").some((r) => r.dataset.path === "papers/alpha/main.tex")`, "alpha's files");
  assert.match(await crumbs(index), /papers alpha$/);
  const alphaRows = await rows(index);
  assert.deepEqual(alphaRows.map((r) => [r.path.split("/").pop(), r.kind]), [["chapter.tex", "tex"], ["figure.png", "image"], ["main.tex", "tex"], ["notes.txt", "other"]]);
  assert.equal(await index.q(`$(".fm-row[data-path='papers/alpha/notes.txt']").tagName`), "DIV", "another file is no link");
  // (the filter)
  await press(index, "/", 0, "Slash", 191);
  await index.send("Input.insertText", { text: "ma" });
  await index.until(`$$(".fm-row").length === 1`, "the filter");
  await shot(index, "index-filter");
  await index.send("Input.insertText", { text: "zz" });
  await index.until(`!$(".fm-empty").hidden`, "nothing matches");
  assert.match(await index.q(`$(".fm-empty").innerText`), /Nothing here matches “mazz”/);
  await shot(index, "index-filter-none");
  await index.key("Escape", "Escape", 27);
  await index.until(`$$(".fm-row").length === 4 && $(".fm-empty").hidden`, "the filter cleared");
  // (Backspace goes up; the folder left is the one focused; Back in the browser goes into it again)
  await press(index, "ArrowDown", 0, "ArrowDown", 40);
  await index.key("Backspace", "Backspace", 8);
  await index.until(`$(".fm-row[aria-selected=true]")?.dataset.path === "papers/alpha"`, "up, alpha selected");
  await index.q(`history.back()`);
  await index.until(`$$(".fm-row").some((r) => r.dataset.path === "papers/alpha/main.tex")`, "Back: alpha again");
  // (a document: a click opens it in this tab, as a link does; its session starts then)
  await index.click(`.fm-row[data-path="papers/alpha/main.tex"]`);
  const ta = index;
  await ta.until(`/\\/d\\/[0-9a-f]{12}\\/$/.test(location.pathname)`, "alpha's address");
  await ta.until(`+$("#pgn")?.textContent > 0`, "alpha's first page", 240000);
  // (the first visit's tips, skipped: they are seen once, then the pages are the user's)
  await ta.until(`$("#tip") && !$("#tip").hidden`, "the tips");
  await shot(ta, "viewer-first-visit");
  await ta.click("#tip button[data-t='skip']");
  await ta.until(`$("#tip").hidden`, "the tips gone");
  await ta.until(`document.title === "papers/alpha/main.tex · Built"`, "the tab's title, the document's path");
  // (its Files column: the same folder, this document marked, its state)
  await ta.until(`$(".fcol .fm-row[aria-current=page]")?.dataset.path === "papers/alpha/main.tex"`, "the Files column");
  await ta.until(`$(".fcol .fm-row[aria-current=page] .fm-badge.tone-ok")`, "its build in the column");
  await shot(ta, "viewer-files");
  await shot(ta, "viewer-files-dark", { scheme: "dark" });
  await shot(ta, "viewer-files-narrow", { width: 420, height: 800 });
  // (the part names its root: opened from the column, the root is shown, and why)
  await ta.click(`.fcol .fm-row[data-path="papers/alpha/chapter.tex"]`);
  await ta.until(`/chapter\\.tex names it as its root/.test($("#toast")?.textContent ?? "")`, "the root's note");
  await ta.until(`$(".fcol .fm-row[aria-current=page]")?.dataset.path === "papers/alpha/main.tex"`, "the root marked");
  assert.doesNotMatch(await ta.q(`location.search`), /note/, "the note taken out of the address");
  await shot(ta, "viewer-root-note");
  // (up to the root in the column, a click a folder, then beta: this tab switched to it in place)
  await ta.click(`.fcol .fm-crumb.root`);
  await ta.until(`$$(".fcol .fm-row").some((r) => r.dataset.path === "beta")`, "the root in the column");
  await ta.click(`.fcol .fm-row[data-path="beta"]`);
  await ta.until(`$$(".fcol .fm-row").some((r) => r.dataset.path === "beta/main.tex")`, "beta's files");
  const before = (await browser.tabs()).length;
  await ta.click(`.fcol .fm-row[data-path="beta/main.tex"]`);
  await ta.until(`document.title.startsWith("beta/main.tex") && $(".fcol .fm-row[aria-current=page]")?.dataset.path === "beta/main.tex"`, "beta in this tab", 60000);
  await ta.until(`+$("#pgn")?.textContent > 0`, "beta's first page", 240000);
  assert.equal((await browser.tabs()).length, before, "switched in place, no new tab");
  // (Ctrl+P: alpha by name, Enter: back in this tab)
  await press(ta, "p", 2, "KeyP", 80);
  await ta.until(`$(".qo-back") && !$(".qo-back").hidden`, "quick open");
  await ta.until(`$$(".qo-row").length === 3`, "the folder's documents");
  await ta.send("Input.insertText", { text: "alph main" });
  await ta.until(`$(".qo-row[aria-selected=true] .qo-dir")?.textContent === "papers/alpha" && $(".qo-row[aria-selected=true] .qo-name")?.textContent === "main.tex"`, "alpha first");
  await shot(ta, "viewer-quick-open");
  await shot(ta, "viewer-quick-open-dark", { scheme: "dark" });
  await ta.key("Enter", "Enter", 13);
  await ta.until(`$(".fcol .fm-row[aria-current=page]")?.dataset.path === "papers/alpha/main.tex"`, "alpha again in this tab", 60000);
  await ta.until(`+$("#pgn")?.textContent > 0`, "alpha's page again", 240000);
  // (two documents side by side: a second index, beta opened from it with a middle click, a tab of its own)
  const ix = await browser.tab(`${url}#dir=beta`);
  await ix.until(`$$(".fm-row").some((r) => r.dataset.path === "beta/main.tex")`, "beta's files in the index");
  await ix.until(`$(".fm-row[data-path='beta/main.tex'] .fm-badge")`, "beta's state in the index");
  await shot(ix, "index-built");
  await shot(ix, "index-built-dark", { scheme: "dark" });
  const at = await ix.point(`.fm-row[data-path="beta/main.tex"]`);
  for (const type of ["mousePressed", "mouseReleased"]) await ix.send("Input.dispatchMouseEvent", { type, x: at.x, y: at.y, button: "middle", clickCount: 1 });
  const tb = await newTab(/\/d\/[0-9a-f]{12}\/$/);
  await tb.until(`+$("#pgn")?.textContent > 0`, "beta's page in its own tab", 240000);
  assert.notEqual(await ta.q(`location.pathname`), await tb.q(`location.pathname`));
  assert.equal(await ta.q(`$(".fcol .fm-row[aria-current=page]")?.dataset.path`), "papers/alpha/main.tex");
  assert.equal(await tb.q(`$(".fcol .fm-row[aria-current=page]")?.dataset.path`), "beta/main.tex");
  assert.match(await ix.q(`location.hash`), /dir=beta/, "the index stayed where it was");
  // (an edit of alpha: its tab's pages again, beta's untouched)
  const pagesB = await tb.q(`$("#pgn").textContent`);
  w.write("papers/alpha/main.tex", doc("Alpha", "Alpha, edited.\\input{chapter}\\newpage Two."));
  await ta.until(`+$("#pgn")?.textContent === 2`, "alpha's edit in its tab", 120000);
  assert.equal(await tb.q(`$("#pgn").textContent`), pagesB);
  // (Ctrl+P on the index too)
  await press(ix, "p", 2, "KeyP", 80);
  await ix.until(`!$(".qo-back").hidden && $$(".qo-row").length === 3`, "the index's quick open");
  await shot(ix, "index-quick-open");
  await ix.key("Escape", "Escape", 27);
  // (the empty folder and a folder without documents, as the index says them)
  await ix.click(`.fm-crumb.root`);
  await ix.until(`$$(".fm-row").some((r) => r.dataset.path === "empty")`, "root");
  await ix.click(`.fm-row[data-path="empty"]`);
  await ix.until(`!$(".fm-empty").hidden && /This folder is empty/.test($(".fm-empty").innerText)`, "the empty folder");
  await shot(ix, "index-empty");
  await shot(ix, "index-empty-dark", { scheme: "dark" });
  await ix.click(`.fm-up`);
  await ix.until(`$$(".fm-row").some((r) => r.dataset.path === "data")`, "root again");
  await ix.click(`.fm-row[data-path="data"]`);
  await ix.until(`/No \\.tex files here/.test($(".fm-tip")?.innerText ?? "")`, "no documents here");
  await shot(ix, "index-no-tex");
  // (a folder made while shown: listed at once)
  mkdirSync(join(dir, "data", "more"));
  await ix.until(`$$(".fm-row").some((r) => r.dataset.path === "data/more")`, "the new folder", 20000);
  for (const t of [ix, ta, tb]) assert.deepEqual(t.errors, [], `errors in a tab: ${t.errors}`);
  // (the watch gone: the index says so)
  w.key("quit");
  await w.exited;
  await ix.until(`/phitex watch has stopped/.test($(".fm-note")?.textContent ?? "")`, "the index told the watch stopped");
  await shot(ix, "index-stopped");
});
