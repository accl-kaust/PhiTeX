// The glyph origins as phitex-live answers them (the watch and the wasm
// core alike): base64 columns of little-endian words, decoded by sync.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { from, glyphs } from "../src/sync.ts";

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
