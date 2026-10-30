// The diff's map (phitex-diff's DiffOut.map): a place on the diff's pages
// back to the user's files, and the editor's cursor onto the diff.
import { test } from "node:test";
import assert from "node:assert/strict";
import { diffPlace, diffSource, type DiffOut } from "../src/diff.ts";

// (new main.tex "Hello brave world.\n" and ch.tex "Chapter text.\n"; the
// diff's text "<head>Hello \DIFadd{brave }world.\n" then ch.tex's text)
const head = "\\begin{document}\n";
const tex = head + "Hello \\DIFadd{brave }world.\n" + "Chapter text.\n";
const at = (s: string) => tex.indexOf(s);
const d: DiffOut = {
  tex,
  files: ["main.tex", "ch.tex"],
  map: [
    [at("Hello"), 6, 0, 0],
    [at("world"), 7, 0, 12],
    [at("Chapter"), 14, 1, 0],
  ],
  changes: [
    {
      kind: "add",
      old: { file: "main.tex", start: 6, end: 6 },
      new: { file: "main.tex", start: 6, end: 11 },
      old_text: "",
      new_text: "brave",
      section: null,
      out: [at("\\DIFadd"), at("world")],
    },
  ],
};

test("a place on the diff's pages is its place in the user's files", () => {
  assert.deepEqual(diffSource(d, at("world") + 2), { file: "main.tex", at: 14 });
  assert.deepEqual(diffSource(d, at("text")), { file: "ch.tex", at: 8 });
  // (added text: the change's place)
  assert.deepEqual(diffSource(d, at("brave")), { file: "main.tex", at: 6 });
  // (before any run: the head, latexdiff's)
  assert.equal(diffSource(d, 0), null);
});

test("the editor's cursor is placed on the diff", () => {
  assert.equal(diffPlace(d, "main.tex", 14), at("world") + 2);
  assert.equal(diffPlace(d, "ch.tex", 8), at("text"));
  // (in the added word: its change's markup)
  assert.equal(diffPlace(d, "main.tex", 8), at("\\DIFadd"));
  assert.equal(diffPlace(d, "other.tex", 0), null);
});
