// Changes (Diff and Blame, one popover under ± Changes) in the watch's
// viewer, in a real browser against a real `phitex watch` (lib.mjs): what a
// person clicks and sees, on a git history with a merged branch.
// PHITEX_SHOTS=DIR keeps each state's screenshot there, light and dark.
//
//   PHITEX_BIN=target/fastdev/phitex node --test --test-concurrency=1 viewer/browser/changes.test.mjs
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
 * `submitted`), Bob's paragraph and a new figure nine days ago, Lina's
 * paragraph on a branch of her own merged into main five days ago, Alice's
 * paragraph a couple of hours ago, and a sentence not committed.
 */
function history() {
  const figure = "\\begin{figure}[h]\n\\centering\n\\includegraphics[width=0.3\\linewidth]{fig.png}\n\\caption{A figure, its picture blamed as its image.}\n\\end{figure}\n";
  const dir = project({ "main.tex": doc(`\\section{Changes}\n${para("Carol", 1)}${para("Carol", 2)}${figure}${para("Carol", 3)}${para("Carol", 4)}`, "\\usepackage{graphicx}\n"), "fig.png": png(16, [40, 90, 200]) });
  const now = Math.floor(Date.now() / 1000);
  const when = (days) => (process.env.GIT_AUTHOR_DATE = process.env.GIT_COMMITTER_DATE = `@${Math.round(now - days * 86400)} +0000`);
  const commit = (who, days, msg) => {
    when(days);
    git(dir, "add", "-A");
    git(dir, "-c", `user.name=${who}`, "-c", `user.email=${who.toLowerCase()}@example.com`, "commit", "-qm", msg);
  };
  const main = () => readFileSync(join(dir, "main.tex"), "utf8");
  git(dir, "init", "-q", "-b", "main");
  commit("Carol", 120, "First draft");
  git(dir, "-c", "tag.gpgSign=false", "tag", "submitted");
  writeFileSync(join(dir, "main.tex"), main().replace(para("Carol", 2), para("Bob", 2)));
  writeFileSync(join(dir, "fig.png"), png(16, [200, 60, 40]));
  commit("Bob", 9, "Bob's paragraph and a new figure");
  git(dir, "checkout", "-q", "-b", "lina/related-work");
  writeFileSync(join(dir, "main.tex"), main().replace(para("Carol", 4), para("Lina", 4)));
  commit("Lina", 6, "Related work");
  git(dir, "checkout", "-q", "main");
  writeFileSync(join(dir, "main.tex"), main().replace("\\section{Changes}\n", "\\section{Changes}\nA line Mary added on main meanwhile.\n\n"));
  commit("Mary", 5.5, "A line on main");
  when(5);
  git(dir, "-c", "user.name=Mary", "-c", "user.email=mary@example.com", "merge", "-q", "--no-ff", "--no-edit", "lina/related-work");
  writeFileSync(join(dir, "main.tex"), main().replace(para("Carol", 3), para("Alice", 3)));
  commit("Alice", 0.1, "Alice's paragraph");
  delete process.env.GIT_AUTHOR_DATE;
  delete process.env.GIT_COMMITTER_DATE;
  writeFileSync(join(dir, "main.tex"), main().replace("\\section{Changes}\n", "\\section{Changes}\nThis sentence is not committed yet.\n\n"));
  return dir;
}

/** The watch on the history, its first page built; `shot(name)` (PHITEX_SHOTS) and `click`, the feature tips out of the way first. */
async function opened() {
  const w = new Watch(history(), { home });
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0`, "the first page", 240000);
  await t.until(`document.title.endsWith("· Built")`, "the build", 240000);
  const tips = async () => {
    if (await t.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`)) await t.click(".tip [data-t=skip]");
  };
  const shot = async (name) => {
    if (!process.env.PHITEX_SHOTS) return;
    await tips();
    await sleep(400);
    await t.shot(join(process.env.PHITEX_SHOTS, `watch-${name}.png`));
  };
  const click = async (sel) => (await tips(), await t.click(sel));
  return { w, t, shot, click };
}
const view = `$(".chg-pop").dataset.view`;
const from = `($("#chg-from code")?.textContent.split(" · ")[0])`;
const to = `($("#chg-to code")?.textContent.split(" · ")[0] ?? null)`;
const counted = `/^\\d+ changes?$/.test($(".chg-pop .foot [aria-live]")?.textContent ?? "")`;
const dark = (t, on) => t.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: on ? "dark" : "light" }] });

test("Changes, Diff: hover peeks, a click keeps it on; the list, ] and [; the merge graph by branch; the picker's rules; Diff's settings kept", async () => {
  const { w, t, shot, click } = await opened();
  // (hovered: a peek; the pointer gone: closed)
  const b = await t.point("#b-changes");
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: b.x, y: b.y });
  await t.until(`!$(".chg-pop").hidden`, "the peek", 5000);
  assert.equal(await t.q(`$(".chg-pop .pin").textContent`), "peek");
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: 300 });
  await t.until(`$(".chg-pop").hidden`, "closed once the pointer left", 5000);
  // (clicked: kept on; Diff, the last commit against the working tree)
  await click("#b-changes");
  assert.equal(await t.q(`$("#b-changes").getAttribute("aria-pressed")`), "true");
  await t.until(counted, "the changes counted", 120000);
  assert.equal(await t.q(`$(".chg-pop .pin").textContent`), "on");
  assert.equal(await t.q(`$("#chg-to b").textContent`), "Working tree");
  await t.until(`$$(".chg-pop .citem small").some((s) => /You · not committed/.test(s.textContent))`, "the sentence not committed: yours", 30000);
  await shot("diff-light");
  await t.key("]", "BracketRight", 221);
  await t.until(`/^1 of \\d+$/.test($(".chg-pop .foot [aria-live]").textContent)`, "] steps to the first change", 5000);
  await t.key("[", "BracketLeft", 219);
  // (the picker: the merge's two lanes, main's chain one colour, the branch another; the merge drawn hollow)
  await click("#chg-from");
  await t.until(`$$(".phx-commits .row[data-i]").length >= 6`, "the commits");
  const rows = await t.q(`$$(".phx-commits .row[data-i]").map((r) => ({ s: r.querySelector(".s").textContent, fill: r.querySelector("svg circle:last-of-type").getAttribute("fill"), stroke: r.querySelector("svg circle:last-of-type").getAttribute("stroke"), cx: r.querySelector("svg circle:last-of-type").getAttribute("cx") }))`);
  const row = (re) => rows.find((r) => re.test(r.s));
  const [alice, merge, mary, lina, bob] = [/Alice/, /merge:/, /A line on main/, /Related work/, /Bob/].map(row);
  assert.equal(merge.stroke, alice.fill, "the merge on main's colour");
  assert.equal(mary.fill, alice.fill, "main's first-parent chain one colour");
  assert.notEqual(lina.fill, alice.fill, "the merged branch its own colour");
  assert.notEqual(lina.cx, alice.cx, "the branch in a lane of its own");
  assert.equal(bob.cx, alice.cx, "main's chain stays in its lane");
  assert.deepEqual(await t.q(`$$(".chg-pop .qb").map((q) => q.textContent)`), ["The last tag · submitted", "Before the merge"]);
  await shot("picker-light");
  const ids = await t.q(`$$(".phx-commits .row[data-i]").map((r) => r.dataset.id)`);
  const at = (i) => `.phx-commits .row[data-i="${i}"]`;
  // (one end, the pointer moved: the rows between aimed; the other end: the range, closed)
  await click(at(3));
  await t.until(`/^Now click the other end/.test($("#chg-hint").textContent)`, "one end clicked: the hint asks for the other", 5000);
  const p1 = await t.point(at(1));
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: p1.x, y: p1.y });
  await t.until(`$$(".phx-commits .row.aim").map((r) => r.dataset.i).join() === "1,2,3"`, "the aim from the first end to the pointer", 5000);
  await shot("picker-aim-light");
  await click(at(1));
  assert.equal(await t.q(view), "diff");
  assert.deepEqual([await t.q(from), await t.q(to)], [ids[3].slice(0, 7), ids[1].slice(0, 7)]);
  // (the same again cancels; Esc lets the first end go, the picker staying; a double-click: the commit
  // against its first parent, the picker staying; Done)
  await click("#chg-from");
  await click(at(2));
  await click(at(2));
  await t.until(`/^Click one end/.test($("#chg-hint").textContent)`, "the same again cancels", 5000);
  await click(at(2));
  await t.key("Escape", "Escape", 27);
  assert.equal(await t.q(view), "diff-pick");
  const m = rows.indexOf(merge);
  const mp = await t.point(at(m));
  await t.mouse(mp.x, mp.y, 1);
  await t.mouse(mp.x, mp.y, 2);
  await t.until(`${view} === "diff-pick" && ${to} === ${JSON.stringify(ids[m].slice(0, 7))}`, "the merge against its first parent, the picker open", 5000);
  await click("#chg-done");
  await t.until(counted, "the merge's changes", 120000);
  assert.ok(await t.q(`$$(".chg-pop .citem").some((c) => /Lina wrote paragraph 4/.test(c.textContent) || /Lina/.test(c.textContent))`), "what the merge brought: Lina's paragraph");
  // (Diff's settings: the colour-blind-safe preset, the marker, kept)
  await click("#chg-gear");
  assert.equal(await t.q(view), "set-diff");
  await shot("diff-settings-light");
  await click(`[data-look="colors"][data-v="safe"]`);
  await click(`[data-look="ins"][data-v="mark"]`);
  assert.equal(await t.q(`$(".chg-pop .preview").className`), "preview i-mark d-strike");
  assert.equal(await t.q(`getComputedStyle($(".chg-pop")).getPropertyValue("--chg-add").trim()`), "#0072B2");
  await click(`[data-reset="diff"]`);
  await click("#chg-gear");
  // (dark)
  await dark(t, true);
  await shot("diff-dark");
  await click("#chg-from");
  await shot("picker-dark");
  await click("#chg-done");
  await dark(t, false);
  // (✕: off)
  await click("#chg-x");
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-pressed") === "false"`, "off", 5000);
  assert.deepEqual(t.errors, []);
  await w.stop();
});

test("Changes, Blame: who wrote this version, by person and by commit; the merged commit's card; skip and since; What this commit changed", async () => {
  const { w, t, shot, click } = await opened();
  await click("#b-changes");
  await click(`[data-m="blame"]`);
  await t.until(`$$(".slot .bl-bar:not(.bl-wait)").length >= 8 && $$(".chg-pop .prow").length >= 5`, "the bars and the people", 60000);
  const people = () => t.q(`$$(".chg-pop .prow .nm > span:first-child").map((e) => e.textContent).sort()`);
  assert.deepEqual(await people(), ["Alice", "Bob", "Carol", "Lina", "Mary", "Not committed"]);
  assert.ok(await t.q(`$$(".slot .bl-bar.bl-hatch").length >= 1`), "the sentence not committed hatched");
  assert.ok(await t.q(`$$(".slot .bl-bar").some((b) => b.getBoundingClientRect().height > 30)`), "the figure's picture: a bar of its own (its image's last commit)");
  await shot("blame-author-light");
  // (a person clicked: their lines picked out; a commit clicked: its lines)
  await click(`.chg-pop .prow`);
  assert.ok(await t.q(`$$(".slot .bl-wash").length > 0`), "the others' lines washed");
  await click(`.chg-pop .prow`);
  assert.equal(await t.q(`$$(".chg-pop .crow small").filter((s) => s.textContent.includes("lina/related-work")).length`), 1, "the merged commit named by its branch");
  // (Lina's line: the card says where it was written and merged)
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: 300 });
  await t.until(`$(".chg-pop").hidden`, "the popover out of the way", 5000);
  // (found by the viewer's Find, which shows its page: the pointer on it)
  await t.q(`(() => { const i = $("#findq"); $("#find").hidden = false; i.value = "Lina wrote"; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  await t.until(`!!$(".mark.fnd")`, "Lina's paragraph found", 10000);
  await sleep(500);
  const lina = await t.q(`(() => { const b = $(".mark.fnd").getBoundingClientRect(); $("#find").hidden = true; return [b.left + b.width / 2, b.top + b.height / 2]; })()`);
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: lina[0], y: lina[1] });
  await t.until(`$(".blcard") && !$(".blcard").hidden`, "the card", 10000);
  const card = await t.q(`$(".blcard").innerText`);
  assert.match(card, /Related work/, card);
  assert.match(card, /On lina\/related-work, merged into main \([0-9a-f]{7}\)/, card);
  assert.match(card, /What this commit changed\s*Show in editor\s*Copy hash/, card);
  await shot("blame-card-light");
  const short = await t.q(`$(".blcard .meta code").textContent`);
  await click(`.blcard [data-a="cmp"]`);
  await t.until(`${view} === "diff" && ${to} === ${JSON.stringify(short)}`, "What this commit changed: Diff, the commit against its parent", 10000);
  await t.until(counted, "its changes", 120000);
  await click(`[data-m="blame"]`);
  // (Blame's settings: Bob's commit skipped, his lines go to who wrote them before (Carol); since the tag)
  await click("#chg-gear");
  assert.equal(await t.q(view), "set-blame");
  await shot("blame-settings-light");
  const bob = await t.q(`$$(".chg-pop [data-skip]").findIndex((x) => /Bob's paragraph/.test(x.closest("label").textContent))`);
  assert.ok(bob >= 0, "Bob's commit offered to skip");
  await t.q(`(() => { const x = $$(".chg-pop [data-skip]")[${bob}]; x.click(); return 1; })()`);
  await t.q(`(() => { const s = $("#chg-since"); s.value = [...s.options].find((o) => /submitted/.test(o.textContent)).value; s.dispatchEvent(new Event("change")); return 1; })()`);
  await click(`[data-look="bars"][data-v="right"]`);
  await click("#chg-gear");
  await t.until(`$$(".chg-pop .prow").length > 0 && !$$(".chg-pop .prow .nm > span:first-child").some((e) => e.textContent === "Bob")`, "Bob's lines gone to who wrote them before", 60000);
  assert.ok(await t.q(`$$(".slot .bl-wash").length > 0`), "since the tag: the lines older than it washed");
  await shot("blame-since-light");
  await dark(t, true);
  await t.q(`(() => { $(".win").classList.add("dark-pdf"); return 1; })()`);
  await shot("blame-since-dark");
  await click(`[data-col="age"]`);
  await shot("blame-age-dark");
  await t.q(`(() => { $(".win").classList.remove("dark-pdf"); return 1; })()`);
  await dark(t, false);
  await click("#chg-gear");
  await click(`[data-reset="blame"]`);
  await click("#chg-x");
  assert.equal(await t.q(`$$(".slot .bl").length`), 0, "off: nothing over the pages");
  assert.deepEqual(t.errors, []);
  await w.stop();
});
