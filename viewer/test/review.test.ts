// Notes and changes beside the pages (review.ts): what the side says of
// them, their order, and the changes grouped under the notes they answer.

import { test } from "node:test";
import assert from "node:assert/strict";
import { ago, groupHunks, oneLine, placeOf, type ReviewHunk, type ReviewNote, type ReviewState } from "../src/review.ts";

const note = (id: string, kind: ReviewNote["kind"], text: string, quote = "the words"): ReviewNote => ({
  id,
  kind,
  anchor: { file: "main.tex", start: 0, end: quote.length, quote, before: "", after: "", line: 3 },
  detached: false,
  status: "open",
  created: "2026-11-14T10:00:00Z",
  updated: "2026-11-14T10:00:00Z",
  thread: [{ by: "you", text, at: "2026-11-14T10:00:00Z" }],
});
const hunk = (id: string, file: string, noteId: string | null = null): ReviewHunk => ({
  id,
  file,
  line: 1,
  end_line: 1,
  now: [0, 3],
  parts: [["del", "old"], ["add", "new"]],
  before: "",
  after: "",
  old: "old",
  new: "new",
  whole: null,
  note: noteId,
  comments: [],
});

test("times are said as a person says them", () => {
  const now = Date.parse("2026-11-14T12:00:00Z");
  assert.equal(ago("2026-11-14T11:59:40Z", now), "now");
  assert.equal(ago("2026-11-14T11:55:00Z", now), "5 min ago");
  assert.equal(ago("2026-11-14T09:00:00Z", now), "3 h ago");
  assert.equal(ago("2026-11-12T12:00:00Z", now), "2 d ago");
  assert.equal(ago("not a time", now), "");
});

test("a quote is one line, cut with an ellipsis", () => {
  assert.equal(oneLine("a\n  b\tc"), "a b c");
  assert.equal(oneLine("x".repeat(20), 10), `${"x".repeat(9)}…`);
});

test("a place is file:line, a range of lines with an en dash", () => {
  assert.equal(placeOf("paper.tex", 118), "paper.tex:118");
  assert.equal(placeOf("paper.tex", 118, 121), "paper.tex:118–121");
});

test("changes are grouped under the note they answered, then by file, notes first", () => {
  const s: ReviewState = {
    notes: [note("n1", "comment", "Use a fresher image.")],
    hunks: [hunk("h1", "ch1.tex"), hunk("h2", "main.tex", "n1"), hunk("h3", "ch1.tex")],
    reviews: [],
    git: true,
  };
  const g = groupHunks(s);
  assert.deepEqual(g.map((x) => x.key), ["n:n1", "f:ch1.tex"]);
  assert.equal(g[0].title, "For your comment: “Use a fresher image.”");
  assert.deepEqual(g[1].hunks.map((h) => h.id), ["h1", "h3"]);
});
