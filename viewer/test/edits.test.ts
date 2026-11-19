// The editor's edits as the session sends them to the core: in UTF-16 code
// units, the unit the core's `edit` splices in (phitex-live's watch.rs
// `splice_utf16`, as LSP's didChange), whatever characters come before.

import { test } from "node:test";
import assert from "node:assert/strict";
import { Batch, type CoreEdit } from "../src/edits.ts";

/** The core's splice (`splice_utf16`): a JS string's offsets are UTF-16 code units. */
const coreSplice = (s: string, e: CoreEdit) => s.slice(0, e.start) + e.text + s.slice(e.end);

const doc = "\\textlang{arabic}{مرحبا بالعالم}\n\n\\textlang{hindi}{नमस्ते दुनिया} 😀 x\n";

// The frontends' xe-multi scenario: " سلام" typed after the Arabic words,
// a keystroke a flush. Sent in UTF-8 bytes (12 more than the units before
// it), the space went after the next paragraph's \textlang and the "س"
// became its language: "The language س is not loaded", every later build
// 0 pages.
test("each keystroke after Arabic text is sent at its UTF-16 place", () => {
  const at = doc.indexOf("}\n\n");
  const b = new Batch(doc);
  let core = doc;
  let editor = doc;
  for (const [k, ch] of [..." سلام"].entries()) {
    b.push({ from: at + k, to: at + k, text: ch });
    editor = editor.slice(0, at + k) + ch + editor.slice(at + k);
    const sent = b.take();
    assert.deepEqual(sent, { start: at + k, end: at + k, text: ch });
    core = coreSplice(core, sent!);
    assert.equal(core, editor, `keystroke ${k}`);
  }
  assert.ok(core.includes("{مرحبا بالعالم سلام}\n\n\\textlang{hindi}{"), core);
});

// Rapid keystrokes merged, an edit after an emoji (a surrogate pair), a
// replacement and a deletion past non-ASCII text, in one flush: the core's
// text is the editor's.
test("a flush of merged and separate edits past wide characters makes the editor's text", () => {
  const b = new Batch(doc);
  let editor = doc;
  const push = (from: number, to: number, text: string) => {
    b.push({ from, to, text });
    editor = editor.slice(0, from) + text + editor.slice(to);
  };
  const h = doc.indexOf("दुनिया");
  push(h, h, "भा");
  push(h + 2, h + 2, "रत ");
  const x = editor.indexOf(" x");
  push(x + 1, x + 2, "yz");
  const a = editor.indexOf("بالعالم");
  push(a, a + "بالعالم".length, "");
  const core = coreSplice(doc, b.take()!);
  assert.equal(core, editor);
  assert.equal(b.size, 0);
  assert.equal(b.take(), null);
});

// One transaction of many ranges (a paste over every line, a replace-all):
// one change of the text before the first to the text after the last, so
// the core builds it once (issue #57: 96 ranges, 96 builds). Wide
// characters at its ends are kept whole; edits that undo each other send
// nothing.
test("a batch of many ranges is one change, its ends never inside a surrogate pair", () => {
  const lines = Array.from({ length: 96 }, (_, i) => `row ${i} 😀 ..........`);
  const before = lines.join("\n");
  const b = new Batch(before);
  let editor = before;
  let at = 0;
  for (let i = 0; i < lines.length; i++) {
    const dots = editor.indexOf("..........", at);
    const e = { from: dots, to: dots + 10, text: i % 2 ? "##########" : "#.#.#.#.#." };
    b.push(e);
    editor = editor.slice(0, e.from) + e.text + editor.slice(e.to);
    at = dots + 10;
  }
  assert.equal(b.size, 96);
  const sent = b.take()!;
  assert.equal(coreSplice(before, sent), editor);
  assert.equal(sent.start, before.indexOf(".........."));
  assert.equal(sent.end, before.length);
  // (😀 → 😃: they share their high surrogate; the change is the pair)
  const w = new Batch("a 😀 b 😀 c");
  w.push({ from: 2, to: 4, text: "😃" });
  w.push({ from: 7, to: 9, text: "😃" });
  const ws = w.take()!;
  assert.deepEqual(ws, { start: 2, end: 9, text: "😃 b 😃" });
  // (undone: nothing to send)
  const u = new Batch("abc");
  u.push({ from: 0, to: 1, text: "x" });
  u.push({ from: 2, to: 3, text: "y" });
  u.push({ from: 0, to: 1, text: "a" });
  u.push({ from: 2, to: 3, text: "c" });
  assert.equal(u.take(), null);
});
