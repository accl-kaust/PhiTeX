// A folder watch's file manager without the DOM (viewer/src/folders.ts):
// the breadcrumb never above the root, folders first and names as people
// sort them, the filter, the states and times as the list says them, and
// quick open's ranking.
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { untilWords, counts, crumbs, filterEntries, fuzzy, rank, runs, sizeOf, sortEntries, stateOf, up, when, type FileEntry } from "../src/folders.ts";

const e = (name: string, kind: FileEntry["kind"], mtime = 0): FileEntry => ({ name, path: name, kind, mtime, size: 0 });

test("the breadcrumb and the way up stop at the root", () => {
  assert.deepEqual(crumbs("thesis", ""), [{ name: "thesis", path: "" }]);
  assert.deepEqual(crumbs("thesis", "a/b"), [
    { name: "thesis", path: "" },
    { name: "a", path: "a" },
    { name: "b", path: "a/b" },
  ]);
  assert.equal(up(""), null);
  assert.equal(up("a"), "");
  assert.equal(up("a/b"), "a");
});

test("folders first, names in people's order, by time newest first when asked", () => {
  const all = [e("ch10.tex", "tex", 3), e("figs", "dir", 1), e("ch2.tex", "tex", 5), e("Appendix", "dir", 9), e("a.png", "image", 4)];
  assert.deepEqual(sortEntries(all, "name").map((x) => x.name), ["Appendix", "figs", "a.png", "ch2.tex", "ch10.tex"]);
  assert.deepEqual(sortEntries(all, "mtime", true).map((x) => x.name), ["Appendix", "figs", "ch2.tex", "a.png", "ch10.tex"]);
  assert.deepEqual(filterEntries(all, " CH ").map((x) => x.name), ["ch10.tex", "ch2.tex"]);
  assert.equal(filterEntries(all, "").length, 5);
});

test("a document's state in words", () => {
  const d = { build: "ok" as const, errors: 0, pages: 3, tabs: 1, live: true, message: null };
  assert.equal(stateOf(undefined), null);
  assert.equal(stateOf(d)!.text, "Built · 3 pages");
  assert.equal(stateOf({ ...d, pages: 1 })!.text, "Built · 1 page");
  assert.equal(stateOf({ ...d, build: "errors", errors: 2 })!.tone, "bad");
  assert.equal(stateOf({ ...d, build: "errors", errors: 2 })!.text, "2 errors");
  assert.equal(stateOf({ ...d, build: "waiting" })!.text, "Waiting…");
  assert.equal(stateOf({ ...d, build: "none" })!.text, "Not built");
  assert.match(stateOf({ ...d, tabs: 0 })!.title, /kept ready/);
  assert.equal(stateOf({ ...d, tabs: 0, live: false })!.text, "Stopped");
  const now = 1_000_000;
  const idle = stateOf({ ...d, tabs: 0, stops_at: now + 250_000 }, now)!;
  assert.deepEqual([idle.text, idle.tone], ["No viewer · stops in 5 min", "idle"]);
  assert.equal(stateOf({ ...d, tabs: 0, stops_at: now + 20_000 }, now)!.text, "No viewer · stops in 20 s");
  // (a tab back: its build's state again)
  assert.equal(stateOf({ ...d, tabs: 1, stops_at: now + 20_000 }, now)!.text, "Built · 3 pages");
  assert.equal(untilWords(now - 5, now), "0 s");
});

test("times, sizes and counts, short and fixed", () => {
  const now = new Date(2026, 10, 24, 15, 0).getTime();
  assert.equal(when(new Date(2026, 10, 24, 9, 5).getTime(), now), "09:05");
  assert.equal(when(new Date(2026, 10, 23, 22, 0).getTime(), now), "Yesterday");
  assert.equal(when(new Date(2026, 2, 3).getTime(), now), "Mar 3");
  assert.equal(when(new Date(2024, 0, 9).getTime(), now), "2024-01-09");
  assert.equal(when(null, now), "");
  assert.equal(sizeOf(812), "812 B");
  assert.equal(sizeOf(4200), "4.2 KB");
  assert.equal(sizeOf(13_400_000), "13 MB");
  assert.equal(counts([e("a", "dir"), e("b.tex", "tex"), e("c.tex", "tex")]), "1 folder · 2 files");
  assert.equal(counts([]), "Empty");
});

test("quick open: letters in order, the file's name over its folders'", () => {
  assert.equal(fuzzy("xyz", "paper/main.tex"), null);
  const m = fuzzy("main", "paper/main.tex")!;
  assert.deepEqual(m.hits, [6, 7, 8, 9]);
  const paths = ["main/notes.tex", "thesis/chapters/methods.tex", "paper/main.tex", "talk/slides.tex"];
  assert.equal(rank("main", paths)[0].path, "paper/main.tex");
  assert.equal(rank("meth", paths)[0].path, "thesis/chapters/methods.tex");
  assert.equal(rank("th ch", paths)[0].path, "thesis/chapters/methods.tex");
  assert.deepEqual(rank("", paths).map((x) => x.path), ["main/notes.tex", "paper/main.tex", "talk/slides.tex", "thesis/chapters/methods.tex"]);
  assert.deepEqual(runs("main.tex", [0, 1]), [["ma", true], ["in.tex", false]]);
  assert.deepEqual(runs("main.tex", [6, 7], 6), [["ma", true], ["in.tex", false]]);
});
