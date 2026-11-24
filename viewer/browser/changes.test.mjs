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

test("Changes, Diff: the tip once; a hover does nothing; a click opens it, starting nothing; a press outside closes it, a drag from inside does not; the picker's rules; the look at the top and in Settings; closed, the diff stays on the pages till Stop comparing; Esc and ✕", async () => {
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
  // (the look at the top of the list, open: no fold anywhere)
  assert.ok(await t.q(`(() => { const l = $("#chg-look"), b = $(".chg-pop .body"); return !!l && l.offsetHeight > 0 && b.firstElementChild === l && !$(".chg-pop .lookbox, .chg-pop .lookhead, .chg-pop details"); })()`), "Diff's look at the top, open, no fold");
  // (the pointer over the pages: still open; a drag begun inside it and let go on the pages: still open)
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 60, y: 400 });
  await sleep(300);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "the pointer gone: still open");
  const ins = await t.point(".chg-pop .ttl h2");
  await t.drag(ins.x, ins.y, 60, 400);
  await sleep(300);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "a drag from inside let go on the pages: still open");
  // (a press on the pages: closed; on the bar beside it: closed too)
  await t.mouse(60, 400);
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-pressed") === "false"`, "a press on the pages closes it", 5000);
  await click("#b-changes");
  await t.until(`!$(".chg-pop").hidden`, "open again", 5000);
  await t.mouse(...(await t.q(`(() => { const r = $("#pgn").getBoundingClientRect(); return [r.left + 2, r.top + r.height / 2]; })()`)));
  await t.until(`$(".chg-pop").hidden`, "a press on the bar closes it", 5000);
  await click("#b-changes");
  await t.until(`!$(".chg-pop").hidden`, "open again", 5000);
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
  assert.ok(await t.q(`$("#chg-n").textContent === $(".chg-pop .foot .count").textContent.split(" ")[0] && /The number: \\d+ changes? in the diff shown/.test($("#b-changes").title)`), "Diff's badge: the diff's changes, as the title says");
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
  // (the look at the top of the list: ⚙ goes to it, marked a moment; the colour-blind-safe preset, the marker; kept)
  await t.q(`(() => { $(".chg-pop .body").scrollTop = 1e6; return 1; })()`);
  await click("#chg-gear");
  await t.until(`$(".chg-pop .body").scrollTop === 0 && $("#chg-look").classList.contains("mark")`, "⚙: the look at the top, marked", 5000);
  assert.equal(await t.q(`${view} === "diff" && $$(".chg-pop .citem").length > 0`), true, "the look above the list, the list still there");
  await click(`[data-look="colors"][data-v="safe"]`);
  await click(`[data-look="ins"][data-v="mark"]`);
  assert.equal(await t.q(`$('#chg-look [data-look="ins"][data-v="mark"]').getAttribute("aria-pressed")`), "true");
  assert.equal(await t.q(`getComputedStyle($(".chg-pop")).getPropertyValue("--chg-add").trim()`), "#0072B2");
  await shot("look-diff-light");
  await dark(t, true);
  await shot("look-diff-dark");
  await dark(t, false);
  // (the same settings in the panel's Settings, Changes; Esc closes Settings first, Changes stays)
  await t.q(`(() => { $("#x-settings").click(); return 1; })()`);
  await t.until(`!$("#settings").hidden && !!$('#settings [data-section="Changes"]')`, "Settings", 5000);
  await t.click(`#settings [data-section="Changes"]`);
  await t.until(`/Diff: colours/.test($("#settings").innerText) && /Blame: detail/.test($("#settings").innerText) && /Blame: style/.test($("#settings").innerText)`, "Settings, Changes: Diff's and Blame's look, Blame's style", 5000);
  await shot("settings-changes-light");
  await dark(t, true);
  await shot("settings-changes-dark");
  await dark(t, false);
  await t.key("Escape", "Escape", 27);
  assert.deepEqual(await t.q(`[$("#settings").hidden, $(".chg-pop").hidden]`), [true, false], "Esc closes Settings first");
  // (All settings… from the look: Settings at Changes; a press in it leaves the popover open under it)
  await click("#chg-all");
  await t.until(`!$("#settings").hidden && /Diff: colours/.test($("#settings").innerText)`, "All settings…: Settings, Changes", 5000);
  await t.click(`#settings [data-id="chg-reset"] button`);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "a press in Settings leaves Changes open");
  await t.key("Escape", "Escape", 27);
  // (Esc closes it, the diff left on the pages and the button pressed; opened again, the list at once, nothing built again)
  const ops = await t.q(`globalThis.__ops.length`);
  await t.key("Escape", "Escape", 27);
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-expanded") === "false"`, "Esc closes it", 5000);
  assert.deepEqual(await t.q(`[$("#b-changes").getAttribute("aria-pressed"), $("#b-changes").classList.contains("chg-comparing"), !!$("#chg-n").textContent, /^Comparing /.test($("#b-changes").title)]`), ["true", true, true, true], "closed with a diff shown: the button pressed, its count, its title says so");
  assert.ok(!(await t.q(`globalThis.__ops.slice(${ops}).includes("diff_show")`)), "closed: the diff's pages stay (no diff_show current)");
  await shot("closed-comparing-light");
  await click("#b-changes");
  assert.ok(await t.q(`$$(".chg-pop .citem").length > 0`), "opened again: the list at once");
  assert.ok(!(await t.q(`globalThis.__ops.slice(${ops}).some((o) => o === "diff_start")`)), "opened again: the same comparison, nothing rebuilt");
  // (closed by a press on the pages: the diff stays there too)
  await t.mouse(60, 400);
  await t.until(`$(".chg-pop").hidden && $("#b-changes").getAttribute("aria-pressed") === "true"`, "a press on the pages: closed, the diff kept", 5000);
  await click("#b-changes");
  // (dark)
  await dark(t, true);
  await shot("diff-dark");
  await click("#chg-from");
  await shot("picker-dark");
  await click("#chg-done");
  await dark(t, false);
  // (a diff shown: Stop comparing, in words, and its downloads beside it, never in the look)
  assert.deepEqual(await t.q(`[$("#chg-stop")?.textContent, $$(".chg-pop .cmpacts [data-dl]").map((b) => b.textContent), $$(".chg-pop #chg-look [data-dl]").length, $("#chg-stop").offsetWidth > 0]`), ["✕ Stop comparing", ["PDF", "diff.tex"], 0, true], "Stop comparing and the downloads under From → To");
  await shot("stop-light");
  await dark(t, true);
  await shot("stop-dark");
  await dark(t, false);
  await click("#chg-stop");
  await t.until(`$(".chg-pop .body .empty b")?.textContent === "Pick a version to compare with" && $(".chg-pop .foot .count").textContent === "No diff yet" && !$("#chg-stop") && !$$("#smarks .change").length && !$("#chg-n").textContent`, "stopped: the pages as they are, no marks, no count, Pick a version", 10000);
  assert.ok(!(await t.q(`$("#b-changes").classList.contains("chg-comparing")`)), "stopped: the button no longer says comparing");
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

test("Changes, Blame: asked by its half; every line coloured (a highlight, by default), each style; its look at the top, ⚙ to it; who wrote this version; a person, a commit or a hover picks out their lines; skip from a commit's row, kept; the merged commit's card; a rebuild blames nothing again", async () => {
  const { w, t, shot, click } = await opened();
  await click("#b-changes");
  await sleep(500);
  assert.deepEqual(await t.q(asked), [], "opened: nothing asked");
  await click(`[data-m="blame"]`);
  const paint = `$$(".slot svg.bl-paint rect")`;
  await t.until(`${paint}.length >= 8 && $$(".chg-pop .prow").length >= 5`, "the lines coloured and the people", 60000);
  assert.ok(await t.q(`${asked}.includes("blame")`), "Blame asked once its half was clicked");
  // (Highlight by default: one blended SVG a page, its rects grouped by commit; no box of its own per line)
  assert.deepEqual(await t.q(`[$$(".slot svg.bl-paint.s-highlight").length > 0, getComputedStyle($(".slot svg.bl-paint")).mixBlendMode, $$(".slot div.bl").length, $('#chg-look [data-look="style"][data-v="highlight"]').getAttribute("aria-pressed")]`), [true, "multiply", 0, "true"], "Highlight on by default, blended behind the words");
  const people = () => t.q(`$$(".chg-pop .prow .nm > span:first-child").map((e) => e.textContent).sort()`);
  assert.deepEqual(await people(), ["Alice", "Bob", "Carol", "Lina", "Mary", "Not committed"]);
  assert.deepEqual(await t.q(`[$("#chg-n").textContent, /The number: 5 people wrote this version/.test($("#b-changes").title)]`), ["5", true], "Blame's badge: the people who wrote this version, as the title says");
  assert.match(await t.q(`$(".chg-pop .sec h3 span").textContent`), /· [\d,]+ words$/, "counted in the sources' words");
  assert.ok(await t.q(`$$('.slot svg.bl-paint rect[fill^="url(#blh-"]').length >= 1`), "the sentence not committed hatched");
  assert.ok(await t.q(`${paint}.some((r) => r.getBoundingClientRect().height > 30)`), "the figure's picture: a highlight of its own (its image's last commit)");
  // (the look at the top, open, no fold: Colour by, Style, Not committed; no pinned footer)
  assert.ok(await t.q(`(() => { const l = $("#chg-look"); return $(".chg-pop .body").firstElementChild === l && ["color", "style", "wip"].every((k) => l.querySelector('[data-look="' + k + '"]')) && !$(".chg-pop .lookbox, .chg-pop .lookhead, .chg-pop .skips, .chg-pop details, .chg-pop > .foot"); })()`), "Blame's look at the top, open; no fold, no footer");
  await shot("blame-author-light");
  await dark(t, true);
  await shot("blame-author-dark");
  await dark(t, false);
  // (⚙ goes to it, marked a moment)
  await t.q(`(() => { $(".chg-pop .body").scrollTop = 1e6; return 1; })()`);
  await click("#chg-gear");
  await t.until(`$(".chg-pop .body").scrollTop === 0 && $("#chg-look").classList.contains("mark")`, "⚙: the look at the top, marked", 5000);
  // (each style: the words' own colour, blended lighten; a bar in the margin, no blend; back to the highlight)
  await click(`#chg-look [data-look="style"][data-v="text"]`);
  await t.until(`$$(".slot svg.bl-paint.s-text rect").length >= 8 && getComputedStyle($(".slot svg.bl-paint")).mixBlendMode === "lighten"`, "Text: the words coloured", 10000);
  await shot("blame-text-light");
  await click(`#chg-look [data-look="style"][data-v="bar"]`);
  await t.until(`!$(".slot svg.bl-paint") && $$(".slot svg.bl-marks .b").length >= 8`, "Bar: a bar a line", 10000);
  // (on the screen, whatever the page's zoom: a page shown small, as in a smaller window, keeps them so)
  assert.ok(await t.q(`$$(".slot svg.bl-marks .b").every((b) => parseFloat(getComputedStyle(b).strokeWidth) > 4 && b.getAttribute("vector-effect") === "non-scaling-stroke")`), "the bars thicker than the old 4 px on the screen");
  await shot("blame-bar-light");
  await click(`#chg-look [data-look="color"][data-v="age"]`);
  await t.until(`!!$("#chg-look .ramp")`, "Age: its ramp", 5000);
  await shot("blame-age-bar-light");
  await click(`#chg-look [data-look="style"][data-v="highlight"]`);
  await shot("blame-age-highlight-light");
  await click(`#chg-look [data-look="color"][data-v="author"]`);
  await t.until(`$$(".slot svg.bl-paint.s-highlight rect").length >= 8`, "back to the highlight", 10000);
  // (not committed hidden, then hatched again)
  await click(`#chg-look [data-look="wip"][data-v="hide"]`);
  await t.until(`!$$('.slot svg.bl-paint rect[fill^="url(#blh-"]').length`, "not committed: not marked", 10000);
  await click(`#chg-look [data-look="wip"][data-v="show"]`);
  await t.until(`$$('.slot svg.bl-paint rect[fill^="url(#blh-"]').length >= 1`, "not committed: hatched again", 10000);
  // (a person clicked: their lines picked out, the rest set back; kept till clicked again or Esc)
  const lit = `($$(".slot svg.bl.hl").length > 0 && $$(".slot svg.bl-paint g.on").length > 0)`;
  await click(`.chg-pop .prow`);
  await t.until(lit, "a person's lines picked out", 5000);
  await sleep(400);
  assert.ok(await t.q(lit), "kept");
  await shot("blame-person-light");
  await t.key("Escape", "Escape", 27);
  await t.until(`!$$(".slot svg.bl.hl").length && !$(".chg-pop").hidden`, "Esc lets the person go, the popover stays", 5000);
  await click(`.chg-pop .crow`);
  await t.until(lit, "a commit's lines picked out", 5000);
  await click(`.chg-pop .crow`);
  await t.until(`!$$(".slot svg.bl.hl").length`, "clicked again: every line again", 5000);
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
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 3, y: 3 });
  await t.q(`(() => { $("#stage").scrollTop = 0; return 1; })()`);
  await sleep(300);
  // (a rebuild, every page drawn again: no blame asked again, the lists' nodes the same ones)
  await t.q(`(() => { $$(".chg-pop .prow, .chg-pop .crow").forEach((e) => (e.__kept = 1)); globalThis.__ops.length = 0; return 1; })()`);
  const before = (await t.q(`document.title`)) ?? "";
  // (Rebuild asked with no press outside the popover: a press on the bar would close it, Blame with it)
  await t.q(`(() => { $("#go").click(); return 1; })()`);
  await t.until(`globalThis.__ops.includes("rebuild")`, "the rebuild asked", 5000);
  await sleep(2500);
  await t.until(`document.title.endsWith("· Built")`, "built again", 120000);
  await sleep(1500);
  assert.ok(!(await t.q(`globalThis.__ops.includes("blame")`)), `no blame again after a rebuild (${before}): ${JSON.stringify(await t.q("globalThis.__ops"))}`);
  assert.ok(await t.q(`$$(".chg-pop .prow, .chg-pop .crow").every((e) => e.__kept === 1)`), "the lists' nodes the same ones: no flash");
  await t.until(`${paint}.length >= 8`, "the pages coloured, mapped again", 10000).catch(async (e) => { throw new Error(`${e.message}: ${await t.q(`JSON.stringify({ slots: $$(".slot").length, bl: $$(".slot .bl").map((x) => x.getAttribute("class") + ":" + x.querySelectorAll("rect").length), canv: $$(".slot canvas").length })`)}`); });
  // (Lina's line hovered: the card after a short dwell, where it was written and merged; her commit's lines picked out
  // (Author: her lines), the rest set back; the pointer gone, all as before)
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
  await t.until(lit, "the hovered line's writer picked out", 5000);
  assert.equal(await t.q(`$(".chg-pop").hidden`), false, "the popover still open beside the card");
  await shot("blame-card-light");
  const short = await t.q(`$(".blcard .meta code").textContent`);
  await click(`.blcard [data-a="cmp"]`);
  await t.until(`${view} === "diff" && ${to} === ${JSON.stringify(short)}`, "What this commit changed: Diff, the commit against its parent", 10000);
  await t.until(counted, "its changes", 120000);
  await click(`[data-m="blame"]`);
  await t.until(`$$(".chg-pop .crw .skipb").length >= 4`, "the commits, each with its Skip", 30000);
  // (Bob's commit skipped from its row: his lines go to who wrote them before (Carol); the row stays, said skipped)
  const bobRow = `$$(".chg-pop .crw").find((r) => /Bob's paragraph/.test(r.textContent))`;
  await t.q(`(() => { ${bobRow}.querySelector(".skipb").click(); return 1; })()`);
  await t.until(`$$(".chg-pop .prow").length > 0 && !$$(".chg-pop .prow .nm > span:first-child").some((e) => e.textContent === "Bob") && ${bobRow}?.classList.contains("skipped") && ${bobRow}.querySelector(".skipb").getAttribute("aria-pressed") === "true"`, "Bob's lines gone to who wrote them before, his row said skipped", 60000);
  await shot("blame-skipped-light");
  await dark(t, true);
  await shot("blame-skipped-dark");
  await dark(t, false);
  // (kept: a new tab, Blame asked, the commit still skipped; taken back from its row there)
  const t2 = await browser.tab(await w.url());
  await t2.until(`document.title.endsWith("· Built")`, "the build", 240000);
  if (await t2.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`)) await t2.click(".tip [data-t=skip]");
  await t2.click("#b-changes");
  await t2.until(`!$(".chg-pop").hidden`, "open", 5000);
  if (await t2.q(`!!$("#chg-blame-go")`)) await t2.click("#chg-blame-go");
  await t2.until(`!!${bobRow}?.classList.contains("skipped")`, "the skip kept", 60000);
  await t2.q(`(() => { ${bobRow}.querySelector(".skipb").click(); return 1; })()`);
  await t2.until(`$$(".chg-pop .prow .nm > span:first-child").some((e) => e.textContent === "Bob") && !${bobRow}?.classList.contains("skipped")`, "taken back: Bob's lines his again", 60000);
  // (changed since the tag, from Settings: the lines older than it set back, uncoloured)
  await t.q(`(() => { $("#x-settings").click(); return 1; })()`);
  await t.until(`!$("#settings").hidden && !!$('#settings [data-section="Changes"]')`, "Settings", 5000);
  await t.click(`#settings [data-section="Changes"]`);
  await t.until(`!!$('#settings [data-id="bl-since"] select')`, "Blame: changed since", 5000);
  await t.q(`(() => { const s = $('#settings [data-id="bl-since"] select'); s.value = [...s.options].find((o) => /submitted/.test(o.textContent)).value; s.dispatchEvent(new Event("change", { bubbles: true })); return 1; })()`);
  await t.key("Escape", "Escape", 27);
  await t.until(`$$(".slot svg.bl-marks rect.v.o").length > 0`, "since the tag: the lines older than it set back", 10000);
  await shot("blame-since-light");
  // (the pages' own colours, from Settings: Sepia (a light sheet: the highlight multiplied), Dark (a dark one: screened, the
  // veil the sheet's colour), Dark PDF; each looked at in the highlight and the words' colour)
  const set = async (id, v) => {
    await t.q(`(() => { if ($("#settings").hidden) $("#x-settings").click(); return 1; })()`);
    await t.until(`!$("#settings").hidden`, "Settings", 5000);
    // (its row in whichever section has it: each section shown in turn till it is there)
    await t.until(`(() => { const b = $('#settings [data-id="${id}"] [data-v="${v}"]'); if (b) return (b.click(), true); const ss = $$("#settings [data-section]"); const i = ss.findIndex((s) => s.getAttribute("aria-selected") === "true" || s.getAttribute("aria-pressed") === "true"); ss[(i + 1) % ss.length]?.click(); return false; })()`, `Settings: ${id} ${v}`, 10000);
    await t.key("Escape", "Escape", 27);
    await t.until(`$("#settings").hidden`, "Settings closed", 5000);
  };
  const blend = `getComputedStyle($(".slot svg.bl-paint")).mixBlendMode`;
  await set("page-colours", "sepia");
  await t.until(`$(".win").classList.contains("paged") && ${blend} === "multiply"`, "Sepia: the highlight multiplied", 5000);
  await shot("blame-sepia-light");
  await click(`#chg-look [data-look="style"][data-v="text"]`);
  await t.until(`${blend} === "lighten"`, "Sepia: the words coloured", 5000);
  await shot("blame-sepia-text-light");
  await set("page-colours", "dark");
  await t.until(`$(".win").classList.contains("paged-dark") && ${blend} === "darken"`, "Dark pages: the words' colour darkened in", 5000);
  await shot("blame-darkpages-text-light");
  await click(`#chg-look [data-look="style"][data-v="highlight"]`);
  await t.until(`${blend} === "screen"`, "Dark pages: the highlight screened", 5000);
  await shot("blame-darkpages-light");
  await set("page-colours", "paper");
  await dark(t, true);
  await set("dark-pdf", "on");
  await t.until(`$(".win").classList.contains("dark-pdf") && ${blend} === "screen"`, "Dark PDF: the highlight screened", 5000);
  await shot("blame-darkpdf-dark");
  await set("dark-pdf", "off");
  await dark(t, false);
  await t.q(`(() => { $("#x-settings").click(); return 1; })()`);
  await t.until(`!$("#settings").hidden && !!$('#settings [data-id="bl-reset"] button')`, "Settings", 5000);
  await t.click(`#settings [data-id="bl-reset"] button`);
  await t.key("Escape", "Escape", 27);
  await click("#chg-x");
  assert.equal(await t.q(`$$(".slot .bl").length`), 0, "closed: nothing over the pages");
  assert.deepEqual(t.errors, []);
  await w.stop();
});

/**
 * A long paper in git (16+ pages): Carol's draft, then Bob's, Lina's and
 * Mary's paragraphs; an \input data table of numbers and an \input TikZ
 * picture, so Blame's words are the reader's (no numbers, no drawing).
 */
function longPaper() {
  const W = "results method latency typeset page edit reader writer measure figure claim history version change output document sentence paragraph".split(" ");
  let seed = 7;
  const word = () => W[(seed = (seed * 1103515245 + 12345) % 2147483648) % W.length];
  const p = (who, i) => `${who} wrote this: ${Array.from({ length: 90 }, word).join(" ")}.\n\n`;
  const paras = (who) => Array.from({ length: 90 }, (_, i) => p(who, i));
  const body = (ps) => ps.map((x, i) => (i % 15 === 0 ? `\\section{Part ${"ABCDEFGH"[i / 15]} of the study}\n${x}` : x)).join("");
  const table = `\\begin{table}[h]\\centering\\begin{tabular}{rrr}\n${Array.from({ length: 30 }, (_, i) => `${i} & ${(i * 0.37).toFixed(2)} & ${i * i}e-3 \\\\`).join("\n")}\n\\end{tabular}\\caption{Measured data}\\end{table}\n`;
  const tikz = "\\begin{figure}[h]\\centering\\begin{tikzpicture}[scale=0.6, every node/.style={draw, fill=blue!10}]\n\\draw[thick, ->] (0,0) -- (4,0) node[right] {Time};\n\\draw (0,0) circle (1) node[above=3pt] {Peak load};\n\\foreach \\x in {1,...,4} \\draw (\\x,0.1) -- (\\x,-0.1);\n\\end{tikzpicture}\\caption{A drawing}\\end{figure}\n";
  const ps = paras("Carol");
  const write = (dir) => writeFileSync(join(dir, "main.tex"), doc(`${body(ps)}\\input{data}\n\\input{drawing}\n`, "\\usepackage{tikz}\n"));
  const dir = project({ "main.tex": "", "data.tex": table, "drawing.tex": tikz });
  write(dir);
  const now = Math.floor(Date.now() / 1000);
  const commit = (who, days, msg) => {
    process.env.GIT_AUTHOR_DATE = process.env.GIT_COMMITTER_DATE = `@${Math.round(now - days * 86400)} +0000`;
    git(dir, "add", "-A");
    git(dir, "-c", `user.name=${who}`, "-c", `user.email=${who.toLowerCase()}@example.com`, "commit", "-qm", msg);
  };
  git(dir, "init", "-q", "-b", "main");
  commit("Carol", 60, "Draft");
  for (const [k, who] of ["Bob", "Lina", "Mary"].entries()) {
    for (let i = k; i < ps.length; i += 4) ps[i] = p(who, i);
    write(dir);
    commit(who, 20 - k * 5, `${who}'s paragraphs`);
  }
  delete process.env.GIT_AUTHOR_DATE;
  delete process.env.GIT_COMMITTER_DATE;
  // (the words a reader reads: the paragraphs' and the section titles', the captions', the drawing's two labels)
  const words = (s) => (s.match(/\p{L}+/gu) ?? []).length;
  const expected = ps.reduce((n, x) => n + words(x), 0) + 6 * words("Part A of the study") + words("Measured data A drawing Time Peak load");
  return { dir, expected };
}

/** A trace's frames and main-thread work while the pointer moved: what the coordinator's DevTools would show. */
function framesOf(ev) {
  const moves = ev.filter((e) => e.name === "EventDispatch" && /^(pointermove|mousemove)$/.test(e.args?.data?.type ?? ""));
  if (!moves.length) return { error: "no pointer events in the trace" };
  const { pid, tid } = moves[0];
  const t0 = Math.min(...moves.map((e) => e.ts)), t1 = Math.max(...moves.map((e) => e.ts));
  const inWin = (e) => e.ts >= t0 && e.ts <= t1;
  const draws = ev.filter((e) => e.pid === pid && e.name === "DrawFrame" && inWin(e)).map((e) => e.ts).sort((a, b) => a - b);
  const begins = ev.filter((e) => e.pid === pid && e.name === "BeginFrame" && inWin(e)).length;
  const dropped = ev.filter((e) => e.pid === pid && e.name === "DroppedFrame" && inWin(e)).length;
  const gaps = draws.slice(1).map((t, i) => (t - draws[i]) / 1000).sort((a, b) => a - b);
  const q = (a, f) => (a.length ? +a[Math.min(a.length - 1, Math.floor(a.length * f))].toFixed(1) : 0);
  // (the main thread's tasks, each with what it spent on style, layout, paint, script)
  const main = ev.filter((e) => e.pid === pid && e.tid === tid && e.ph === "X" && inWin(e));
  const tasks = main.filter((e) => e.name === "RunTask" || e.name === "ThreadControllerImpl::RunTask");
  const kinds = { style: ["UpdateLayoutTree", "RecalculateStyles"], layout: ["Layout"], paint: ["Paint", "PrePaint", "PaintImage", "Layerize", "UpdateLayer"], script: ["FunctionCall", "EventDispatch", "TimerFire", "FireAnimationFrame", "v8.callFunction"], hittest: ["HitTest"] };
  const split = (task) => {
    const out = {};
    for (const [k, names] of Object.entries(kinds)) out[k] = +(main.filter((e) => names.includes(e.name) && e.ts >= task.ts && e.ts + e.dur <= task.ts + task.dur).reduce((n, e) => n + e.dur, 0) / 1000).toFixed(1);
    return out;
  };
  const durs = tasks.map((e) => e.dur / 1000).sort((a, b) => a - b);
  const long = tasks.filter((e) => e.dur > 16700).map((e) => ({ ms: +(e.dur / 1000).toFixed(1), ...split(e) }));
  const raster = ev.filter((e) => e.pid === pid && e.name === "RasterTask" && inWin(e)).map((e) => e.dur / 1000).sort((a, b) => a - b);
  // (what is ours, by the main thread's own CPU time (tdur), not the wall clock a loaded runner stretches: each
  // move's dispatch (its handlers, and any style or layout they force), the style, layout and paint per move, and
  // the layerizes: their count against the frames begun, their time per move)
  const cpu = (e) => (e.tdur ?? e.dur) / 1000;
  const sum = (names) => main.filter((e) => names.includes(e.name)).reduce((n, e) => n + cpu(e), 0);
  const moveCpu = moves.map(cpu).sort((a, b) => a - b);
  const layerize = main.filter((e) => e.name === "Layerize").map(cpu).sort((a, b) => a - b);
  const per = (names) => +(sum(names) / moves.length).toFixed(3);
  const perMove = { script: { p95: q(moveCpu, 0.95), max: q(moveCpu, 1) }, style: per(kinds.style), layout: per(kinds.layout), paint: per(kinds.paint.filter((n) => n !== "Layerize")), layerize: { n: layerize.length, frames: begins, p50: q(layerize, 0.5), perMove: per(["Layerize"]), max: q(layerize, 1) } };
  return { perMove, seconds: +((t1 - t0) / 1e6).toFixed(2), moves: moves.length, vsyncs: begins, drawn: draws.length, dropped, frameGapMs: { p50: q(gaps, 0.5), p95: q(gaps, 0.95), max: q(gaps, 1) }, mainTaskMs: { p50: q(durs, 0.5), p95: q(durs, 0.95), max: q(durs, 1) }, over16ms: long, rasterMs: { p95: q(raster, 0.95), max: q(raster, 1) } };
}

/** A 3 s pointer sweep over the pages from their top (a move every 16 ms, the wheel now and then), traced: its frames (framesOf). */
async function sweep(t, name, wheel = true) {
  await t.q(`(() => { $("#stage").scrollTop = 0; return 1; })()`);
  await sleep(1500);
  const st = await t.q(`(() => { const r = $("#stage").getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; })()`);
  const ev = await t.trace(async () => {
    for (let i = 0, t0 = Date.now(); Date.now() - t0 < 3000; i++) {
      const fx = (i % 60) / 60, r = Math.floor(i / 60);
      await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: st[0] + st[2] * (0.1 + 0.5 * (r % 2 ? 1 - fx : fx)), y: st[1] + st[3] * (0.2 + 0.7 * ((i % 37) / 37)) });
      if (wheel && i % 20 === 19) await t.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: st[0] + st[2] * 0.3, y: st[1] + st[3] * 0.5, deltaX: 0, deltaY: 300 });
      await sleep(16);
    }
  });
  if (process.env.PHITEX_SHOTS) writeFileSync(join(process.env.PHITEX_SHOTS, `watch-sweep-${name}.json`), JSON.stringify({ traceEvents: ev }));
  return framesOf(ev);
}

test("Blame on a 16-page paper: words as a reader counts them, said as partial till every file is in; a 3 s traced pointer sweep, Blame's own work per move small; the card a dwell after the hover", async () => {
  const { dir, expected } = longPaper();
  const w = new Watch(dir, { home });
  const t = await browser.tab(await w.url());
  await t.until(`+$("#pgn")?.textContent >= 16`, "16 pages", 240000);
  await t.until(`document.title.endsWith("· Built")`, "the build", 240000);
  if (await t.q(`!!$(".tip [data-t=skip]") && !$(".tip").hidden`)) await t.click(".tip [data-t=skip]");
  // (the pages' own frames under the sweep, Blame off: what Blame's are measured against)
  const base = await sweep(t, "blame-off");
  const still = (await sweep(t, "still-off", false)).perMove;
  await t.click("#b-changes");
  // (the summary's head as it changes: partial, said so, till every file is in)
  await t.q(`(() => { const seen = (globalThis.__heads = []); new MutationObserver(() => { const h = $(".chg-pop .sec h3 span")?.textContent; if (h && seen.at(-1) !== h) seen.push(h); }).observe($(".chg-pop"), { subtree: true, childList: true, characterData: true }); return 1; })()`);
  await t.click(`[data-m="blame"]`);
  await t.until(`/· [\\d,]+ words$/.test($(".chg-pop .sec h3 span")?.textContent ?? "")`, "the words counted", 120000);
  const heads = await t.q("globalThis.__heads");
  assert.ok(heads.every((h) => /· so far · \d of 3 files$/.test(h) || h === heads.at(-1)), `partial heads said so: ${JSON.stringify(heads)}`);
  const n = +(await t.q(`$(".chg-pop .sec h3 span").textContent.match(/· ([\\d,]+) words$/)[1].replace(/,/g, "")`));
  assert.ok(n >= expected && n <= expected * 1.02 + 10, `words: ${n}, the reader's ${expected} (no numbers, no drawing)`);
  assert.deepEqual(await t.q(`$$(".chg-pop .prow .nm > span:first-child").map((e) => e.textContent).sort()`), ["Bob", "Carol", "Lina", "Mary"]);
  await t.until(`$$(".slot svg.bl-paint rect").length > 40`, "the lines coloured", 30000);
  // (the title row: the title centred over the popover, ⚙ and ✕ at the right, no fold)
  assert.ok(await t.q(`(() => { const p = $(".chg-pop").getBoundingClientRect(), h = $(".chg-pop .ttl h2").getBoundingClientRect(); return !$("#chg-fold") && Math.abs((h.left + h.right) / 2 - (p.left + p.right) / 2) < 2 && $("#chg-gear").getBoundingClientRect().left > h.right; })()`), "the title centred, ⚙ and ✕ at the right, no fold");
  if (process.env.PHITEX_SHOTS) await t.shot(join(process.env.PHITEX_SHOTS, "watch-title-light.png"));
  await dark(t, true);
  if (process.env.PHITEX_SHOTS) (await sleep(300), await t.shot(join(process.env.PHITEX_SHOTS, "watch-title-dark.png")));
  await dark(t, false);
  // (the same 3 s sweep, Blame on: its frames against the pages' own)
  const f = await sweep(t, "blame-on");
  console.log(`# traced sweep, Blame off: ${JSON.stringify(base)}`);
  console.log(`# traced sweep, Blame on:  ${JSON.stringify(f)}`);
  assert.ok(!f.error && !base.error, f.error ?? base.error);
  // (bounds a frame's budget at most, with room for a runner's CPU at half this box's speed: under 24 busy loops
  // here the move's task stayed under 2 ms, style, layout and paint under 0.5 ms a move, the card's task 6.5 ms)
  // (the frames above are the runner's as much as ours: logged, not asserted. What is Blame's own: the pointer
  // alone over still pages, the wheel left out, so every bit of style, layout, paint and layerize is the hover's;
  // by the main thread's CPU time, not the wall clock or the frames a loaded runner stretches and drops)
  const { perMove: on } = await sweep(t, "still-on", false);
  console.log(`# per move, the pointer alone, Blame off: ${JSON.stringify(still)}`);
  console.log(`# per move, the pointer alone, Blame on:  ${JSON.stringify(on)}`);
  assert.ok(on.script.max <= 8, `each move's own task, its handlers and what they force: ${JSON.stringify(on.script)} ms`);
  assert.ok(on.style + on.layout + on.paint <= 2, `style, layout and paint per move: ${JSON.stringify(on)} ms`);
  assert.ok(on.layerize.n <= on.layerize.frames + 2 && on.layerize.p50 <= still.layerize.p50 + 1, `no layerize of its own per move, none of every layer: ${JSON.stringify(on.layerize)} against ${JSON.stringify(still.layerize)}`);
  // (the hover to the card: its dwell, then drawn)
  const line = await t.q(`(() => { const b = $$(".slot svg.bl-paint rect").find((b) => { const r = b.getBoundingClientRect(); return r.top > 150 && r.bottom < innerHeight - 200 && r.height > 4; }); const r = b.getBoundingClientRect(); return [r.left + 20, r.top + r.height / 2]; })()`);
  await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: line[1] });
  await t.until(`$(".blcard").hidden`, "no card", 5000);
  await t.q(`(() => { const card = $(".blcard"); globalThis.__hc = null; let t0 = null; addEventListener("pointermove", (e) => (t0 ??= e.timeStamp), { capture: true }); new MutationObserver(() => { if (!card.hidden && globalThis.__hc === null && t0 !== null) globalThis.__hc = performance.now() - t0; }).observe(card, { attributes: true, childList: true }); return 1; })()`);
  // (traced: the dwell is ours, the wall clock after it the runner's; what the card costs is its main thread's CPU
  // time from the hover to the card shown, every task in between)
  let hc = 0;
  const ev = await t.trace(async () => {
    await t.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: line[0], y: line[1] });
    hc = await t.until(`globalThis.__hc`, "the card", 5000);
  });
  const mv = ev.find((e) => e.name === "EventDispatch" && /^(pointermove|mousemove)$/.test(e.args?.data?.type ?? ""));
  // (the task that showed it: the dwell's timer, the last to fire before the card's observer saw it, hc after the move)
  const at = (mv?.ts ?? 0) + hc * 1000, main = ev.filter((e) => mv && e.pid === mv.pid && e.tid === mv.tid && e.ph === "X");
  const fired = main.filter((e) => e.name === "TimerFire" && e.ts >= mv.ts + 150000 && e.ts <= at + 3000).sort((x, y) => y.ts - x.ts)[0];
  const shown = fired && main.find((e) => (e.name === "RunTask" || e.name === "ThreadControllerImpl::RunTask") && e.ts <= fired.ts && e.ts + e.dur >= fired.ts + fired.dur);
  const work = shown ? (shown.tdur ?? shown.dur) / 1000 : Infinity;
  console.log(`# hover to card: ${hc.toFixed(0)} ms (a 180 ms dwell, then drawn), the task that drew it ${work.toFixed(1)} ms of CPU`);
  assert.ok(hc >= 170, `the card not before its dwell: ${hc} ms`);
  assert.ok(work <= 16, `the card drawn in one task within a frame's budget after its dwell: ${work.toFixed(1)} ms`);
  assert.deepEqual(t.errors, []);
  await w.stop();
});
