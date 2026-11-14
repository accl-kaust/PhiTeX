// Where a drag over the pages puts its end (text.ts's caretAt): on two
// columns, a point in a short line's white space or in the gap between the
// columns goes to the end of the nearest line of the column the drag began
// in, never to a line of the other column (the browser's own hit test over
// empty SVG gave one, and a drag selected half the page).
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { caretAt } from "../src/text.ts";
import type { TextLine } from "../src/text.ts";

// (a page: a left column at x 50..290, a right one at 310..550, rows 12 pt apart;
// the left column's third line a paragraph's short last line, "detail.")
const line = (x: number, y: number, text: string, w = 240): TextLine => ({ x, y, w, size: 10, text });
const L = [
  line(50, 100, "Sentence one of this paragraph talks about"),
  line(50, 112, "accelerators and graphs in some detail. Sen-"),
  line(50, 124, "detail.", 35),
  line(50, 136, "Another paragraph begins on this line here"),
];
const R = [
  line(310, 100, "The right column's first line of its text"),
  line(310, 112, "and its second line, which goes on and on"),
  line(310, 124, "its third line, beside the short one there"),
  line(310, 136, "and its fourth line, the last one of these"),
];
const lines = [...L, ...R];

test("over a line's text: the character there", () => {
  // (the line stretched to its width: 42 units over 240 pt)
  const c = caretAt(lines, 50 + (240 * 10.2) / 42, 98, L[0]);
  assert.deepEqual(c, { i: 0, off: 10 });
});

test("past a short line's end, in its column's white space: that line's end", () => {
  for (const x of [90, 120, 200, 289])
    assert.deepEqual(caretAt(lines, x, 122, L[0]), { i: 2, off: "detail.".length }, `x ${x}`);
});

test("in the gap between the columns: the end of the line in the drag's column", () => {
  for (const x of [292, 300, 308]) assert.deepEqual(caretAt(lines, x, 110, L[0]), { i: 1, off: L[1].text.length }, `x ${x}`);
});

test("on the other column's text: there (a drag that means to go on)", () => {
  assert.equal(caretAt(lines, 400, 110, L[0])?.i, 5);
});

test("between two rows: the nearest row of the drag's column", () => {
  // (y 128.5: below row 124's bottom (126.5), above row 136's top (128))
  const c = caretAt(lines, 120, 127, L[0])!;
  assert.ok(c.i === 2 || c.i === 3, `line ${c.i}`);
  assert.ok(c.i < 4);
});

test("below every line: the last row, in the drag's column", () => {
  assert.equal(caretAt(lines, 120, 300, L[0])?.i, 3);
  assert.equal(caretAt(lines, 400, 300, R[0])?.i, 7);
});

test("before a line's start: its first character", () => {
  assert.deepEqual(caretAt(lines, 20, 112, L[0]), { i: 1, off: 0 });
});

test("no lines: none", () => {
  assert.equal(caretAt([], 10, 10), null);
});

test("between two rows of the drag's column, inside a row of the next column: the drag's column", () => {
  // (the right column's rows half a row lower: its baselines not the left's)
  const shifted = [...L, ...R.map((l) => ({ ...l, y: l.y + 6 }))];
  const c = caretAt(shifted, 120, 115.2, L[0])!;
  assert.ok(c.i === 1 || c.i === 2, `line ${c.i}`);
});
