// Blame on the page in the watch's viewer, in a real browser against a real
// `phitex watch` (lib.mjs): what a person clicks and sees. PHITEX_SHOTS=DIR
// keeps each look's screenshot there (Age, Author, Since; light and dark).
//
//   PHITEX_BIN=target/fastdev/phitex node --test --test-concurrency=1 viewer/browser/blame.test.mjs
import { test, before, after, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Browser, Watch, project, sleep, git } from "./lib.mjs";

const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
let browser;
before(async () => (browser = await Browser.start()));
after(async () => (Watch.killAll(), await browser?.stop()));
afterEach(async (t) => browser?.report(t));

const doc = (body, pre = "") => `\\documentclass{article}\n${pre}\\begin{document}\n${body}\n\\end{document}\n`;
const para = (who, n) => `${who} wrote paragraph ${n}, which says what a paragraph says at some length, so that it runs over a few printed lines of the page and has a margin beside it.\n\n`;
/** A PNG, `n` pixels square, all of colour `rgb`. */
function png(n, [r, g, b]) {
  const chunk = (type, data) => {
    const len = Buffer.alloc(4), crc = Buffer.alloc(4), td = Buffer.concat([Buffer.from(type), data]);
    len.writeUInt32BE(data.length);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const head = Buffer.alloc(13);
  head.writeUInt32BE(n, 0);
  head.writeUInt32BE(n, 4);
  head[8] = 8;
  head[9] = 2;
  const rows = Buffer.concat(Array.from({ length: n }, () => Buffer.concat([Buffer.from([0]), Buffer.from(Array.from({ length: n }, () => [r, g, b]).flat())])));
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", head), chunk("IDAT", deflateSync(rows)), chunk("IEND", Buffer.alloc(0))]);
}

/**
 * A project in git: Carol's first draft four months ago (tagged
 * `submitted`), Bob's paragraph and a new figure nine days ago, Alice's
 * paragraph a couple of hours ago, and a sentence not committed.
 */
function history() {
  const figure = "\\begin{figure}[h]\n\\centering\n\\includegraphics[width=0.3\\linewidth]{fig.png}\n\\caption{A figure, its picture blamed as its image.}\n\\end{figure}\n";
  const dir = project({ "main.tex": doc(`\\section{Blame}\n${para("Carol", 1)}${para("Carol", 2)}${figure}${para("Carol", 3)}`, "\\usepackage{graphicx}\n"), "fig.png": png(16, [40, 90, 200]) });
  const now = Math.floor(Date.now() / 1000);
  const commit = (who, days, msg) => {
    process.env.GIT_AUTHOR_DATE = process.env.GIT_COMMITTER_DATE = `@${Math.round(now - days * 86400)} +0000`;
    git(dir, "add", "-A");
    git(dir, "-c", `user.name=${who}`, "-c", `user.email=${who.toLowerCase()}@example.com`, "commit", "-qm", msg);
  };
  git(dir, "init", "-q");
  commit("Carol", 120, "First draft");
  git(dir, "-c", "tag.gpgSign=false", "tag", "submitted");
  let main = readFileSync(join(dir, "main.tex"), "utf8").replace(para("Carol", 2), para("Bob", 2));
  writeFileSync(join(dir, "main.tex"), main);
  writeFileSync(join(dir, "fig.png"), png(16, [200, 60, 40]));
  commit("Bob", 9, "Bob's paragraph and a new figure");
  main = main.replace(para("Carol", 3), para("Alice", 3));
  writeFileSync(join(dir, "main.tex"), main);
  commit("Alice", 0.1, "Alice's paragraph");
  delete process.env.GIT_AUTHOR_DATE;
  delete process.env.GIT_COMMITTER_DATE;
  writeFileSync(join(dir, "main.tex"), main.replace("\\section{Blame}\n", "\\section{Blame}\nA sentence not committed yet.\n\n"));
  return dir;
}

test("Blame: bars by age, author and since; the figure as its image; the card; Compare with before this commit", async () => {
  const w = new Watch(history(), { home });
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0`, "the first page", 240000);
  await t.until(`document.title.endsWith("· Built")`, "the build", 240000);
  const shots = process.env.PHITEX_SHOTS;
  const shot = async (name) => {
    if (!shots) return;
    if (await t.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`)) await t.click(".tip [data-t=skip]");
    await sleep(400);
    await t.shot(join(shots, `watch-${name}.png`));
  };
  // (the first visit's feature tips, which come one by one, out of the way of each click)
  const click = async (sel) => {
    if (await t.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`)) await t.click(".tip [data-t=skip]");
    await t.click(sel);
  };
  const legend = () => t.q(`$$(".blamebar .lg").map((l) => l.children[1].textContent + " " + l.children[2].textContent)`);
  await click("#b-blame");
  await t.until(`$$(".slot .bl-bar").length >= 8`, "the bars, a printed line each", 30000);
  assert.equal(await t.q(`$("#b-blame").getAttribute("aria-pressed")`), "true");
  const age = (await legend()).join("|");
  assert.match(age, /^Not committed [1-9]\d*\|Under a day [1-9]\d*\|Under a week 0\|Under a month [1-9]\d*\|Under 6 months [1-9]\d*\|Older 0$/, `the age legend: ${age}`);
  assert.ok(await t.q(`$$(".slot .bl-bar.bl-hatch").length >= 1`), "the sentence not committed hatched");
  // (the figure's picture: a bar beside the space over its caption, Bob's (the image's last commit))
  assert.ok(await t.q(`$$(".slot .bl-bar").some((b) => b.getBoundingClientRect().height > 30)`), "the picture's bar");
  await shot("age-light");
  // (hovered, the line's card: Alice's commit, its hash, its source; Compare, Show, Copy)
  const at = await t.q(`(() => { const b = $$(".slot .bl-bar").filter((b) => !b.classList.contains("bl-hatch")).at(-2).getBoundingClientRect(); return [b.left + 120, b.top + b.height / 2]; })()`);
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at[0], y: at[1] });
  await t.until(`$(".blcard") && !$(".blcard").hidden`, "the card", 10000);
  const card = await t.q(`$(".blcard").innerText`);
  assert.match(card, /Alice's paragraph/, card);
  assert.match(card, /[0-9a-f]{7,} · Alice · /, card);
  assert.match(card, /main\.tex:\d+/, card);
  assert.match(card, /Compare with before this commit\s*Show in editor\s*Copy hash/, card);
  assert.ok(await t.q(`$$(".slot .bl-tint").length >= 1`), "the paragraph under the pointer tinted");
  await shot("card-light");
  await click(`#bl-mode button[data-v="author"]`);
  await click(`#bl-tint button[data-v="all"]`);
  const authors = (await legend()).map((l) => l.replace(/ \d+$/, "")).sort();
  assert.deepEqual(authors, ["Alice", "Bob", "Carol", "Not committed"]);
  await shot("author-light");
  await click(`#bl-mode button[data-v="since"]`);
  await click(`#bl-tint button[data-v="hover"]`);
  assert.match(await t.q(`$("#bl-since option").textContent`), /^submitted · 4 months ago$/);
  const since = await legend();
  assert.match(since[0], /^Changed since then [1-9]/, `since: ${since}`);
  await shot("since-light");
  // (the dark PDF: the dark ramp)
  await t.q(`(() => { $(".win").classList.add("dark-pdf"); return 1; })()`);
  await t.until(`$$(".slot .bl-bar:not(.bl-hatch)")[0]?.style.background.includes("199, 166, 240")`, "since's dark violet", 5000);
  await shot("since-dark");
  await click(`#bl-mode button[data-v="author"]`);
  await shot("author-dark");
  await click(`#bl-mode button[data-v="age"]`);
  await shot("age-dark");
  await t.q(`(() => { $(".win").classList.remove("dark-pdf"); return 1; })()`);
  // (Compare with before this commit: the watch's Compare against the commit's parent)
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: 5 });
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: at[0], y: at[1] });
  await t.until(`$(".blcard") && !$(".blcard").hidden`, "the card again", 10000);
  await click(`.blcard [data-a="cmp"]`);
  await t.until(`$("#diffbar")?.innerText.includes("Before ")`, "the diff bar against the commit's parent", 120000);
  // (off: nothing over the pages)
  await click(`.blamebar .x`);
  assert.equal(await t.q(`$$(".slot .bl").length`), 0);
  assert.deepEqual(t.errors, []);
  await w.stop();
});
