// The highlight's scroll margin (Viewer.margin, Settings' "Keep a margin"):
// the highlight that follows the editor stays in the band `margin` of the
// view's height away from its top and bottom edges; the page scrolls just
// enough to bring it back to the band's edge, never re-centred. 0 keeps the
// behaviour before: no scroll until the highlight leaves the view, then the
// middle.

import { test } from "node:test";
import assert from "node:assert/strict";
import { marginScroll } from "../src/viewer.ts";

// a view 1000 px high at 5000 px down the pages; 20% margin: the band 5200..5800
const H = 1000, AT = 5000;

test("inside the band the page does not move", () => {
  assert.equal(marginScroll(5200, 5220, AT, H, 0.2), null);
  assert.equal(marginScroll(5500, 5520, AT, H, 0.2), null);
  assert.equal(marginScroll(5780, 5800, AT, H, 0.2), null);
});

test("above the band: scrolled up just enough to put its top at the band's top edge", () => {
  assert.equal(marginScroll(5150, 5170, AT, H, 0.2), 4950);
  // (off the view altogether, the same rule: at the band's edge, not the middle)
  assert.equal(marginScroll(3000, 3020, AT, H, 0.2), 2800);
});

test("below the band: scrolled down just enough to put its bottom at the band's bottom edge", () => {
  assert.equal(marginScroll(5790, 5810, AT, H, 0.2), 5010);
  assert.equal(marginScroll(9000, 9020, AT, H, 0.2), 8220);
});

test("a highlight taller than the band keeps its top in the band", () => {
  // 900 px tall against a 600 px band: its top at the band's top edge
  assert.equal(marginScroll(5900, 6800, AT, H, 0.2), 5700);
  // its top in the band already, its bottom below: no scroll that would push its top out
  assert.equal(marginScroll(5300, 6200, AT, H, 0.2), 5100);
});

test("0 is the behaviour before: nothing until it leaves the view, then the middle", () => {
  assert.equal(marginScroll(5100, 5120, AT, H, 0), null);
  assert.equal(marginScroll(5900, 5920, AT, H, 0), null);
  assert.equal(marginScroll(6500, 6520, AT, H, 0), 6000);
  assert.equal(marginScroll(4000, 4020, AT, H, 0), 3500);
});

test("the margin is held to 45% of the view, so the band is never empty", () => {
  assert.equal(marginScroll(5500, 5520, AT, H, 0.9), null);
  assert.equal(marginScroll(5400, 5420, AT, H, 0.9), 4950);
});
