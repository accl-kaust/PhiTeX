// Changes (Diff and Blame, one popover under ± Changes) in the watch's
// viewer, in a real browser against a real `phitex watch` (lib.mjs): what a
// person clicks and sees, on a git history with a merged branch.
// PHITEX_SHOTS=DIR keeps each state's screenshot there, light and dark.
//
//   PHITEX_BIN=target/fastdev/phitex node --test --test-concurrency=1 viewer/browser/changes.test.mjs
import { test, before, after, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Browser, Watch, project, sleep, git } from "./lib.mjs";

const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
// (the tips seen before ± Changes: the first one's, and Compare's and Blame's of the toolbar before it;
// the Changes tip still shows, once)
mkdirSync(join(home, ".config", "phitex"), { recursive: true });
writeFileSync(join(home, ".config", "phitex", "phitex.toml"), 'tips-seen = ["live", "compare", "blame"]\n');
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

/** The watch on the history, its first page built; `shot(name)` (PHITEX_SHOTS) and `click`, the feature tips out of the way first (`tips`: left as they are). */
async function opened({ tips: keep = false } = {}) {
  const w = new Watch(history(), { home });
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent > 0`, "the first page", 240000);
  await t.until(`document.title.endsWith("· Built")`, "the build", 240000);
  const tips = async () => {
    if (!keep && (await t.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`))) await t.click(".tip [data-t=skip]");
  };
  const shot = async (name) => {
    if (!process.env.PHITEX_SHOTS) return;
    await tips();
    await sleep(400);
    await t.shot(join(process.env.PHITEX_SHOTS, `watch-${name}.png`));
  };
  const click = async (sel) => (await tips(), await t.click(sel));
  // (what the page asks the watch from here: each request's op)
  await t.q(`(() => { const send = WebSocket.prototype.send; globalThis.__ops = []; WebSocket.prototype.send = function (d) { try { globalThis.__ops.push(JSON.parse(d).op); } catch {} return send.call(this, d); }; return 1; })()`);
  return { w, t, shot, click };
}
const view = `$(".chg-pop").dataset.view`;
const from = `($("#chg-from code")?.textContent.split(" · ")[0])`;
const to = `($("#chg-to code")?.textContent.split(" · ")[0] ?? null)`;
const counted = `/^\\d+ changes?$/.test($(".chg-pop .foot .count")?.textContent ?? "")`;
/** Changes' and Blame's requests since the page loaded (the history, a diff, its pages, blame). */
const asked = `globalThis.__ops.filter((o) => /^(blame|diff_)/.test(o))`;
const dark = (t, on) => t.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: on ? "dark" : "light" }] });

test("Changes, Diff: the tip once; a hover does nothing; a click opens it, starting nothing; the pointer and clicks beside it leave it open; the picker's rules; the look inline and in Settings; Esc and ✕", async () => {
  const { w, t, shot, click } = await opened({ tips: true });
  // (the tip, once: the old Compare's and Blame's seen, the Changes one still shown)
  await t.until(`!$(".tip").hidden && /Changes/.test($(".tip h4").textContent)`, "the Changes tip", 10000);
  await shot("tip-light");
  // (hovered: nothing, no popover, nothing asked of the watch)
  const b = await t.point("#b-changes");
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: b.x, y: b.y });
  await sleep(1200);
  assert.equal(await t.q(`$(".chg-pop").hidden`), true, "a hover opens nothing");
  assert.deepEqual(await t.q(asked), [], "a hover asks the watch nothing");
  // (clicked: open, the tip gone; nothing worked out till a version is picked)
  await t.mouse(b.x, b.y);
  await t.until(`!$(".chg-pop").hidden`, "open", 5000);
  assert.equal(await t.q(`$("#b-changes").getAttribute("aria-pressed")`), "true");
  assert.equal(await t.q(`$(".tip").hidden`), true, "the tip gone once its button is used");
  assert.equal(await t.q(`$(".chg-pop .body .empty b").textContent`), "Pick a version to compare with");
  assert.equal(await t.q(`$(".chg-pop .foot .count").textContent`), "No diff yet");
  await sleep(800);
  assert.deepEqual(await t.q(asked), [], "opened: nothing asked of the watch");
  assert.ok(await t.q(`[...$(".chg-pop").querySelectorAll("button")].filter((x) => x.offsetParent).every((x) => x.title || x.getAttribute("aria-label"))`), "every control says what it does");
  await shot("empty-light");
  // (the pointer over the pages, a click on them: still open)
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 60, y: 400 });
  await t.mouse(60, 400);
  await sleep(500);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "the pointer gone, a click on the pages: still open");
  // (the picker: the merge's two lanes, main's chain one colour, the branch another)
  await click("#chg-start");
  await t.until(`$$(".phx-commits .row[data-i]").length >= 6`, "the commits");
  const rows = await t.q(`$$(".phx-commits .row[data-i]").map((r) => ({ s: r.querySelector(".s").textContent, fill: r.querySelector("svg circle:last-of-type").getAttribute("fill"), stroke: r.querySelector("svg circle:last-of-type").getAttribute("stroke"), cx: r.querySelector("svg circle:last-of-type").getAttribute("cx") }))`);
  const row = (re) => rows.find((r) => re.test(r.s));
  const [alice, merge, mary, lina, bob] = [/Alice/, /merge:/, /A line on main/, /Related work/, /Bob/].map(row);
  assert.equal(merge.stroke, alice.fill, "the merge on main's colour");
  assert.equal(mary.fill, alice.fill, "main's first-parent chain one colour");
  assert.notEqual(lina.fill, alice.fill, "the merged branch its own colour");
  assert.notEqual(lina.cx, alice.cx, "the branch in a lane of its own");
  assert.equal(bob.cx, alice.cx, "main's chain stays in its lane");
  await t.until(`$$(".chg-pop .qb").length > 0`, "the quick choices", 10000);
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
  await t.until(counted, "the range's changes", 120000);
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
  assert.ok(await t.q(`$$(".chg-pop .citem").some((c) => /Lina/.test(c.textContent))`), "what the merge brought: Lina's paragraph");
  // (against the working tree: the sentence not committed, yours; ] [ and N step, D the pages)
  await click("#chg-to");
  await click(".phx-commits .row:not([data-i]):not(.skel)");
  await t.until(`$$(".chg-pop .citem small").some((s) => /You · not committed/.test(s.textContent))`, "the sentence not committed: yours", 120000);
  await shot("diff-light");
  await t.key("]", "BracketRight", 221);
  await t.until(`/^1 of \\d+$/.test($(".chg-pop .foot .count").textContent)`, "] steps to the first change", 5000);
  await t.key("n", "KeyN", 78);
  await t.until(`/^2 of \\d+$/.test($(".chg-pop .foot .count").textContent)`, "N steps on", 5000);
  await t.key("[", "BracketLeft", 219);
  await t.key("d", "KeyD", 68);
  await t.until(`$('.chg-pop [data-pg="current"]').getAttribute("aria-pressed") === "true"`, "D: the current version's pages", 5000);
  await t.key("d", "KeyD", 68);
  await t.until(`$('.chg-pop [data-pg="diff"]').getAttribute("aria-pressed") === "true"`, "D again: the diff's", 5000);
  // (the look, inline under the list: the colour-blind-safe preset, the marker, change bars; kept)
  await click("#chg-lookhead");
  assert.equal(await t.q(`!$("#chg-look").hidden && ${view} === "diff" && $$(".chg-pop .citem").length > 0`), true, "the look under the list, the list still there");
  await click(`[data-look="colors"][data-v="safe"]`);
  await click(`[data-look="ins"][data-v="mark"]`);
  assert.equal(await t.q(`$(".chg-pop .preview").className`), "preview i-mark d-strike");
  assert.equal(await t.q(`getComputedStyle($(".chg-pop")).getPropertyValue("--chg-add").trim()`), "#0072B2");
  await shot("look-diff-light");
  await dark(t, true);
  await shot("look-diff-dark");
  await dark(t, false);
  // (the same settings in the panel's Settings, Changes; Esc closes Settings first, Changes stays)
  await t.q(`(() => { $("#x-settings").click(); return 1; })()`);
  await t.until(`!$("#settings").hidden && !!$('[data-group="Changes"]')`, "Settings", 5000);
  await t.click(`[data-group="Changes"]`);
  await t.until(`/Diff: colours/.test($("#settings").innerText) && /Blame: detail/.test($("#settings").innerText)`, "Settings, Changes: Diff's and Blame's look", 5000);
  await shot("settings-changes-light");
  await dark(t, true);
  await shot("settings-changes-dark");
  await dark(t, false);
  await t.key("Escape", "Escape", 27);
  assert.deepEqual(await t.q(`[$("#settings").hidden, $(".chg-pop").hidden]`), [true, false], "Esc closes Settings first");
  await click(`[data-reset="diff"]`);
  await click("#chg-lookhead");
  // (Esc closes it; opened again, the list at once)
  await t.key("Escape", "Escape", 27);
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-pressed") === "false"`, "Esc closes it", 5000);
  await click("#b-changes");
  assert.ok(await t.q(`$$(".chg-pop .citem").length > 0`), "opened again: the list at once");
  // (dark)
  await dark(t, true);
  await shot("diff-dark");
  await click("#chg-from");
  await shot("picker-dark");
  await click("#chg-done");
  await dark(t, false);
  // (✕)
  await click("#chg-x");
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-pressed") === "false"`, "✕ closes it", 5000);
  // (a page opened again: the Changes tip never again)
  const t2 = await browser.tab(await w.url());
  await t2.until(`document.title.endsWith("· Built")`, "the build", 240000);
  await sleep(1500);
  const cards = [];
  for (let i = 0; i < 12 && (await t2.q(`!$(".tip").hidden`)); i++) {
    cards.push(await t2.q(`$(".tip h4").textContent`));
    await t2.click(".tip [data-t=next]");
    await sleep(150);
  }
  assert.ok(!cards.some((c) => /Changes/.test(c)), `the Changes tip not again: ${cards.join(", ")}`);
  assert.deepEqual(t.errors, []);
  await w.stop();
});

test("Changes, Blame: asked by its half; who wrote this version; the merged commit's card; skip and since; the pointer swept with no long task; a rebuild blames nothing again", async () => {
  const { w, t, shot, click } = await opened();
  await click("#b-changes");
  await sleep(500);
  assert.deepEqual(await t.q(asked), [], "opened: nothing asked");
  await click(`[data-m="blame"]`);
  await t.until(`$$(".slot .bl-bar:not(.bl-wait)").length >= 8 && $$(".chg-pop .prow").length >= 5`, "the bars and the people", 60000);
  assert.ok(await t.q(`${asked}.includes("blame")`), "Blame asked once its half was clicked");
  const people = () => t.q(`$$(".chg-pop .prow .nm > span:first-child").map((e) => e.textContent).sort()`);
  assert.deepEqual(await people(), ["Alice", "Bob", "Carol", "Lina", "Mary", "Not committed"]);
  assert.match(await t.q(`$(".chg-pop .sec h3 span").textContent`), /· [\d,]+ words$/, "counted in the sources' words");
  assert.ok(await t.q(`$$(".slot .bl-bar.bl-hatch").length >= 1`), "the sentence not committed hatched");
  assert.ok(await t.q(`$$(".slot .bl-bar").some((b) => b.getBoundingClientRect().height > 30)`), "the figure's picture: a bar of its own (its image's last commit)");
  await shot("blame-author-light");
  // (a person clicked: their lines picked out)
  await click(`.chg-pop .prow`);
  assert.ok(await t.q(`$$(".slot .bl-wash").length > 0`), "the others' lines washed");
  await click(`.chg-pop .prow`);
  assert.equal(await t.q(`$$(".chg-pop .crow small").filter((s) => s.textContent.includes("lina/related-work")).length`), 1, "the merged commit named by its branch");
  // (the pointer swept over the pages for 2 s, Blame on: no long task, frames on time)
  const st = await t.q(`(() => { const r = $("#stage").getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; })()`);
  await t.q(`(() => { const w = (globalThis.__sweep = { long: [], frames: [] }); new PerformanceObserver((l) => l.getEntries().forEach((e) => w.long.push(Math.round(e.duration)))).observe({ type: "longtask" }); let last = performance.now(); const f = (x) => { w.frames.push(x - last); last = x; if (!w.stop) requestAnimationFrame(f); }; requestAnimationFrame(f); return 1; })()`);
  for (let i = 0, t0 = Date.now(); Date.now() - t0 < 2000; i++) {
    const fx = (i % 60) / 60, r = Math.floor(i / 60);
    await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: st[0] + st[2] * (0.1 + 0.5 * (r % 2 ? 1 - fx : fx)), y: st[1] + st[3] * (0.15 + 0.7 * ((i % 37) / 37)) });
    if (i % 20 === 19) await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: st[0] + st[2] * 0.3, y: st[1] + st[3] * 0.5, deltaX: 0, deltaY: 260 });
    await sleep(16);
  }
  const sw = await t.q(`(() => { const w = globalThis.__sweep; w.stop = true; const fr = w.frames.slice(1).sort((a, b) => a - b); return { long: w.long, p95: Math.round(fr[Math.floor(fr.length * 0.95)] ?? 0) }; })()`);
  assert.deepEqual(sw.long, [], `no long task while the pointer swept: ${JSON.stringify(sw)}`);
  assert.ok(sw.p95 <= 34, `frames on time while the pointer swept: ${JSON.stringify(sw)}`);
  await t.q(`(() => { $("#stage").scrollTop = 0; return 1; })()`);
  await sleep(300);
  // (a rebuild, every page drawn again: no blame asked again, the lists' nodes the same ones)
  await t.q(`(() => { $$(".chg-pop .prow, .chg-pop .crow").forEach((e) => (e.__kept = 1)); globalThis.__ops.length = 0; return 1; })()`);
  const before = (await t.q(`document.title`)) ?? "";
  await click("#go");
  await t.until(`globalThis.__ops.includes("rebuild")`, "the rebuild asked", 5000);
  await sleep(2500);
  await t.until(`document.title.endsWith("· Built")`, "built again", 120000);
  await sleep(1500);
  assert.ok(!(await t.q(`globalThis.__ops.includes("blame")`)), `no blame again after a rebuild (${before}): ${JSON.stringify(await t.q("globalThis.__ops"))}`);
  assert.ok(await t.q(`$$(".chg-pop .prow, .chg-pop .crow").every((e) => e.__kept === 1)`), "the lists' nodes the same ones: no flash");
  assert.ok(await t.q(`$$(".slot .bl-bar:not(.bl-wait)").length >= 8`), "the pages' bars, mapped again");
  // (Lina's line: the card, after a short dwell, says where it was written and merged)
  await t.q(`(() => { const i = $("#findq"); $("#find").hidden = false; i.value = "Lina wrote"; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  await t.until(`!!$(".mark.fnd")`, "Lina's paragraph found", 10000);
  await sleep(500);
  const lina = await t.q(`(() => { const b = $(".mark.fnd").getBoundingClientRect(); $("#find").hidden = true; return [b.left + 10, b.top + b.height / 2]; })()`);
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: lina[0], y: lina[1] });
  await t.until(`$(".blcard") && !$(".blcard").hidden`, "the card", 10000);
  const card = await t.q(`$(".blcard").innerText`);
  assert.match(card, /Related work/, card);
  assert.match(card, /On lina\/related-work, merged into main \([0-9a-f]{7}\)/, card);
  assert.match(card, /What this commit changed\s*Show in editor\s*Copy hash/, card);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "the popover still open beside the card");
  await shot("blame-card-light");
  const short = await t.q(`$(".blcard .meta code").textContent`);
  await click(`.blcard [data-a="cmp"]`);
  await t.until(`${view} === "diff" && ${to} === ${JSON.stringify(short)}`, "What this commit changed: Diff, the commit against its parent", 10000);
  await t.until(counted, "its changes", 120000);
  await click(`[data-m="blame"]`);
  // (Blame's look, inline: Bob's commit skipped, his lines go to who wrote them before (Carol); since the tag)
  await click("#chg-lookhead");
  await shot("look-blame-light");
  const bob = await t.q(`$$(".chg-pop [data-skip]").findIndex((x) => /Bob's paragraph/.test(x.closest("label").textContent))`);
  assert.ok(bob >= 0, "Bob's commit offered to skip");
  await t.q(`(() => { const x = $$(".chg-pop [data-skip]")[${bob}]; x.click(); return 1; })()`);
  await t.q(`(() => { const s = $("#chg-since"); s.value = [...s.options].find((o) => /submitted/.test(o.textContent)).value; s.dispatchEvent(new Event("change")); return 1; })()`);
  await click(`[data-look="bars"][data-v="right"]`);
  await t.until(`$$(".chg-pop .prow").length > 0 && !$$(".chg-pop .prow .nm > span:first-child").some((e) => e.textContent === "Bob")`, "Bob's lines gone to who wrote them before", 60000);
  assert.ok(await t.q(`$$(".slot .bl-wash").length > 0`), "since the tag: the lines older than it washed");
  await shot("blame-since-light");
  await dark(t, true);
  await t.q(`(() => { $(".win").classList.add("dark-pdf"); return 1; })()`);
  await shot("look-blame-dark");
  await t.q(`(() => { $(".win").classList.remove("dark-pdf"); return 1; })()`);
  await dark(t, false);
  await click(`[data-reset="blame"]`);
  await click("#chg-lookhead");
  await click("#chg-x");
  assert.equal(await t.q(`$$(".slot .bl").length`), 0, "closed: nothing over the pages");
  assert.deepEqual(t.errors, []);
  await w.stop();
});
