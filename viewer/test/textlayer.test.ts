// A page's text layer (strips.ts's textLayer) is made empty and filled when
// first needed (text.ts's deferText, ensureText): the view at rest, or a
// reader of its lines (a selection, a link's text, the reading aid). Filled
// as each page arrived, its SVG text built and laid out in the frame the
// page came into view was a scroll's and a zoom's long frames.
//
//   scripts/viewer-test.sh   (node --test, a DOM from linkedom)

import { test } from "node:test";
import assert from "node:assert/strict";

const linkedom = await import(new URL("../../target/viewer-test/node_modules/linkedom/esm/index.js", import.meta.url).href);
const { document, window } = linkedom.parseHTML("<!doctype html><html><body></body></html>");
Object.assign(globalThis, { document, window });

const { textLayer } = await import("../src/strips.ts");
const { ensureText, holdText, textPending, LAYER_LINES } = await import("../src/text.ts");
const { waspOn } = await import("../src/wasp.ts");

const lines = [
  { x: 72, y: 100, w: 200, size: 10, text: "First line" },
  { x: 72, y: 112, w: 180, size: 10, text: "Second line" },
];

test("a text layer is empty, nothing to lay out, until its lines are needed", () => {
  const page = document.createElement("div");
  const svg = textLayer(lines, 612, 792);
  page.append(svg);
  assert.equal(svg.children.length, 0);
  assert.equal(textPending(page), true);
  ensureText(page);
  assert.equal(textPending(page), false);
  assert.deepEqual([...svg.children].map((t: Element) => t.textContent), ["First line", "Second line"]);
  assert.equal(LAYER_LINES.get(svg.children[0])?.y, 100);
  // (filled once: again, nothing more)
  ensureText(page);
  ensureText(svg);
  assert.equal(svg.children.length, 2);
});

test("the layer itself can be filled, and an empty page or none is no error", () => {
  const svg = textLayer(lines, 612, 792);
  ensureText(svg);
  assert.equal(svg.children.length, 2);
  ensureText(null);
  ensureText(document.createElement("div"));
});

test("the reading aid fills the layer it reads", () => {
  const page = document.createElement("div");
  page.append(textLayer(lines, 612, 792));
  waspOn(page);
  assert.equal(textPending(page), false);
  assert.ok(page.querySelector(".phx-wasp"), "its bands drawn from the lines");
});

test("a zoom's end holds the layers shown out of the layout; shown again when read", () => {
  const page = document.createElement("div");
  const svg = textLayer(lines, 612, 792);
  page.append(svg);
  ensureText(page);
  holdText(page);
  assert.equal(svg.hasAttribute("data-hold"), true);
  assert.equal(textPending(page), true);
  ensureText(page);
  assert.equal(svg.hasAttribute("data-hold"), false);
  assert.equal(svg.children.length, 2, "filled once, not again");
  // (one never filled is not held: it waits to be filled)
  const other = textLayer(lines, 612, 792);
  page.append(other);
  holdText(page);
  assert.equal(other.hasAttribute("data-hold"), false);
});
