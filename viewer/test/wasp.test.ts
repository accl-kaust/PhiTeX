// The reading aid's bands (wasp.ts): the body's lines, alternating, the
// centred ones and other sizes left alone.

import { test } from "node:test";
import assert from "node:assert/strict";
import { bands } from "../src/wasp.ts";
import type { TextLine } from "../src/text.ts";

const line = (text: string, y: number, x0 = 72, size = 10): TextLine => {
  const xs = Array.from(text, (_, i) => x0 + 5 * i);
  return { x: x0, y, w: 5 * text.length, size, text, xs, rs: xs.map((x) => x + 5) };
};
const para = (y0: number, n: number, x0 = 72, len = 80) => Array.from({ length: n }, (_, i) => line("w".repeat(i === n - 1 ? 20 : len), y0 + 12 * i, x0));

test("the body's lines alternate direction, in pairs of a colour", () => {
  const b = bands(para(100, 5));
  assert.equal(b.length, 5);
  assert.deepEqual(b.map((x) => x.dir), [0, 1, 0, 1, 0]);
  assert.deepEqual(b.map((x) => x.c), [0, 0, 1, 1, 0]);
  // (a short last line's band its own width: its gradient ends where it does)
  assert.ok(b[4].w < b[3].w / 2);
});

test("a heading, a centred equation and an equation with its number are left alone", () => {
  const lines = [
    line("Heading", 80, 72, 14),
    ...para(100, 3),
    line("x=y+z", 150, 250),
    { ...line("a=b      (1)", 175, 230), xs: [230, 235, 240, 245, ...Array(5).fill(250), 455, 460, 465], rs: [235, 240, 245, 250, ...Array(5).fill(455), 460, 465, 470] },
    ...para(190, 2),
  ];
  const b = bands(lines);
  assert.equal(b.length, 5);
  assert.ok(b.every((x) => x.h < 14));
  assert.deepEqual(b.map((x) => x.dir), [0, 1, 0, 1, 0]);
});

test("a subscript's run is part of its line", () => {
  const b = bands([...para(100, 3), line("i", 126, 300, 7), line("i", 126, 310, 7), line("i", 126, 320, 7)]);
  assert.equal(b.length, 3);
});

test("two columns: each line a band of its own column", () => {
  const left = para(100, 4, 72, 45), right = para(100, 4, 320, 45);
  const b = bands([...left, ...right]);
  assert.equal(b.length, 8);
  assert.ok(b.slice(0, 4).every((x) => x.x < 300) && b.slice(4).every((x) => x.x > 300));
});
