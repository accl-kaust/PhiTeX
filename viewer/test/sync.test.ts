// The glyph origins as phitex-live answers them (the watch and the wasm
// core alike): base64 columns of little-endian words, decoded by sync.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { boxes, extent, from, glyphAt, glyphNear, glyphs, lineAt, lineBoxes, lines, sourceBetween, wordNear } from "../src/sync.ts";

const b64 = (words: ArrayBuffer) => btoa(String.fromCharCode(...new Uint8Array(words)));
const u32 = (xs: number[]) => b64(new Uint32Array(xs).buffer);
const f32 = (xs: number[]) => b64(new Float32Array(xs).buffer);

test("origins' columns decode to glyphs, a file of none and a synthesized one kept", () => {
  const json = {
    files: ["./hello.tex", "article.cls"],
    n: 3,
    file: u32([0, 0xffffffff, 1]),
    start: u32([83, 0, 5]),
    end: u32([84, 0, 9]),
    // (one bit a glyph: the third synthesized)
    synth: u32([0b100]),
    x: f32([72.5, 80, 90.25]),
    y: f32([100, 100, 112]),
  };
  const gs = glyphs(json, (n) => n.replace(/^\.\//, ""));
  assert.deepEqual(gs, [
    { x: 72.5, y: 100, file: "hello.tex", start: 83, end: 84, synth: false },
    { x: 80, y: 100, file: null, start: 0, end: 0, synth: false },
    { x: 90.25, y: 112, file: "article.cls", start: 5, end: 9, synth: true },
  ]);
  assert.equal(from(gs, "hello.tex", 78, 116).length, 1);
});

test("a page with no glyphs, or an answer with none, gives none", () => {
  assert.deepEqual(glyphs({ files: [], n: 0 }, (n) => n), []);
});

test("the follow's word: the cursor's, else in spaces the one before on its line, else after; never across lines", () => {
  const t = (s: string) => new TextEncoder().encode(s);
  assert.deepEqual(wordNear(t("one two"), 5, 0, 7), [4, 7]);
  // (after a space typed at the end of a line: the word before it)
  assert.deepEqual(wordNear(t("one two \nx"), 8, 0, 8), [4, 7]);
  // (spaces at a line's start: the word after it)
  assert.deepEqual(wordNear(t("a\n  bc"), 3, 2, 6), [4, 6]);
  assert.equal(wordNear(t("a\n   "), 3, 2, 5), null);
});

test("the glyph nearest the cursor on its line: the last at or before it, else the first after", () => {
  const g = (start: number) => ({ file: "a.tex", start, end: start + 1, synth: false, x: start, y: 10 });
  const hits = [g(0), g(1), g(4), g(5)];
  // (a word just typed at 8, its glyphs not built yet: the typeset one before it)
  assert.equal(glyphNear(hits as never, 8)?.start, 5);
  assert.equal(glyphNear(hits as never, 3)?.start, 1);
  assert.equal(glyphNear([g(4)] as never, 1)?.start, 4);
  assert.equal(glyphNear([], 1), null);
});

// Two columns: each line of the left set whole before the right's, the right
// column's lines on the left's baselines; a subscript within a line.
const g = (x: number, y: number, start: number, file: string | null = "a.tex", synth = false) => ({ x, y, file, start, end: start + 1, synth });
function twoColumns() {
  const all = [];
  // (left column: lines at y 100, 112, 124, bytes 0.., x 50..; a subscript on the second)
  let b = 0;
  for (const y of [100, 112, 124]) {
    for (let i = 0; i < 10; i++) all.push(g(50 + 5 * i, y, b++));
    if (y === 112) all.push(g(100, 114, b++), g(104, 112, b++));
  }
  // (a footnote at the column's foot, from bytes far later: a \footnote's)
  for (let i = 0; i < 4; i++) all.push(g(50 + 5 * i, 300, 500 + i));
  // (right column: the same baselines, x 300..; then a float's caption, its bytes inside the left's span)
  for (const y of [100, 112, 124]) for (let i = 0; i < 10; i++) all.push(g(300 + 5 * i, y, b++));
  return all;
}

test("boxes keep two columns apart: a box a printed line, never across the gap, in the order they were set", () => {
  const all = twoColumns();
  const ls = lines(all);
  assert.notEqual(ls.get(all[0]), ls.get(all.find((x) => x.x === 300 && x.y === 100)!), "the columns' lines on one baseline are two lines");
  // (the subscript its own run; its line goes on after it)
  const second = all.filter((x) => x.y === 112 && x.x < 200);
  assert.equal(new Set(second.map((x) => ls.get(x))).size, 1);
  // (the left's last line and the right's first: two boxes, each in its column)
  const hits = [...all.filter((x) => x.y === 124 && x.x < 200), ...all.filter((x) => x.y === 100 && x.x >= 300)];
  const bs = boxes(hits, all);
  assert.equal(bs.length, 2);
  assert.ok(bs[0][0] + bs[0][2] < 200 && bs[1][0] > 290, `in their columns: ${JSON.stringify(bs)}`);
  const lb = lineBoxes(hits, all);
  assert.equal(lb.ids.length, 2);
});

test("a source line's glyphs on two columns: the cursor's printed line, not the other column's on its baseline", () => {
  const all = twoColumns();
  const hits = all.filter((x) => x.y === 100);
  const got = lineAt(hits, 3, all);
  assert.ok(got.length === 10 && got.every((x) => x.x < 200));
});

test("a selection's source is from its first character's glyph to its last's, in the order they were set; a note is drawn so", () => {
  const all = twoColumns();
  // (the text layer's edges: the left's 2nd line's 3rd character, the 3rd line's 5th's right edge)
  const i = glyphAt(all, 60, 112, "left")!, j = glyphAt(all, 75, 124, "right")!;
  assert.deepEqual([all[i].start, all[j].start], [12, 26]);
  // (the right column's glyphs on that baseline are not the left's edge's)
  assert.equal(all[glyphAt(all, 310, 112, "left")!].x, 310);
  const src = sourceBetween(() => all, { k: 0, i }, { k: 0, i: j })!;
  assert.deepEqual([src.file, src.start, src.end], ["a.tex", 12, 27]);
  // (drawn again from the span: those glyphs, not the right column's nor the footnote's)
  const ext = extent([all], "a.tex", src.start, src.end);
  assert.equal(ext.length, 1);
  assert.deepEqual(ext[0].gs.map((x) => x.start), Array.from({ length: 15 }, (_, k) => 12 + k));
  // (a span whose bytes hold a footnote's typeset elsewhere: from its first glyph to its last, the footnote not between them)
  const wide = extent([all], "a.tex", 20, 600);
  assert.ok(wide[0].gs.every((x) => x.start < 500 || x.y === 300));
  // (bytes typeset nowhere: none)
  assert.deepEqual(extent([all], "b.tex", 0, 5), []);
});
