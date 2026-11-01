// The page's text as the strips give it: TeX's accents made one with
// their letters, and finding across the pages' lines.

import { test } from "node:test";
import assert from "node:assert/strict";
import { composeLine, search, type TextLine } from "../src/text.ts";

const line = (text: string, y: number, x0 = 72): TextLine => {
  const xs = Array.from(text, (_, i) => x0 + 5 * i);
  return { x: x0, y, w: 5 * text.length, size: 10, text, xs, rs: xs.map((x) => x + 5) };
};

test("an accent set before its letter is one character with it", () => {
  // (TeX's \\"O: the dieresis, its middle over the O)
  const l: TextLine = { x: 72, y: 100, w: 10, size: 10, text: "¨O", xs: [72, 72], rs: [77, 78] };
  assert.equal(composeLine(l).text, "Ö");
});

test("a word is found on its page, and across a line break", () => {
  const pages = [[line("nothing here", 100)], [line("the strips are", 100), line("drawn once", 112)]];
  const hits = search(pages, "strips");
  assert.equal(hits.length, 1);
  assert.equal(hits[0].k, 1);
  const across = search(pages, "are drawn");
  assert.equal(across.length, 1);
  assert.equal(across[0].boxes.length, 2, "boxed on each line");
});
