// Zoom: a page is shown at its own pixels once drawn at the zoom, and drawn
// again after any zoom, in or out, as soon as the core can.
//
// - pageBox rounds a page's box up to whole device pixels as the core
//   rounds its bitmap (crates/output/render: ceil of points × scale): a
//   page drawn at the zoom fills its box to the pixel and is copied, never
//   resampled. Rounded down, with the height made from the rounded width,
//   the box was up to three pixels short, and the page shown stretched.
// - A page drawn at another zoom, finer as well as coarser, is asked for
//   again (a zoom out kept the finer bitmap, shrunk by the browser, its
//   glyphs aliased for good); asked once at a time, the zoom it rests at
//   last (a pinch asked once a frame, and the core drew every one).
// - A page at its own pixels has no transform: Chrome paints a canvas
//   snapped to the device's pixels, and a translate by the fraction its
//   box sat off them was not snapped, every glyph blurred half a pixel.
//
//   scripts/viewer-test.sh   (node --test, a DOM from linkedom)

import { test } from "node:test";
import assert from "node:assert/strict";

const linkedom = await import(new URL("../../target/viewer-test/node_modules/linkedom/esm/index.js", import.meta.url).href);
const { document, window } = linkedom.parseHTML("<!doctype html><html><body></body></html>");
// (what a page's layout would give: every box on screen, 100 by 140)
window.HTMLElement.prototype.getBoundingClientRect = function () {
  return { top: 0, bottom: 140, left: 0, right: 100, width: 100, height: 140, x: 0, y: 0 } as DOMRect;
};
let seen: ((es: { target: Element; isIntersecting: boolean }[]) => void) | null = null;
Object.assign(globalThis, {
  document,
  window,
  requestAnimationFrame: (f: (t: number) => void) => setTimeout(() => f(performance.now()), 0),
  cancelAnimationFrame: (h: ReturnType<typeof setTimeout>) => clearTimeout(h),
  IntersectionObserver: class {
    constructor(cb: typeof seen) {
      seen = cb;
    }
    observe() {}
    unobserve() {}
  },
});
window.devicePixelRatio = 2;

const { pageBox } = await import("../src/panel.ts");
const { Viewer } = await import("../src/viewer.ts");

const A4 = { w: 595.276, h: 841.89 };
const tick = () => new Promise((r) => setTimeout(r, 5));

test("a page's box is the core's bitmap of it, to the device pixel", () => {
  for (const dpr of [1, 1.25, 1.5, 2, 3])
    for (const zoom of [0.5, 0.62, 0.8217, 1, 1.25, 1.6, 2.6, 4.3])
      for (const d of [A4, { w: 612, h: 792 }, { w: 419.528, h: 595.276 }]) {
        const s = (96 / 72) * zoom * dpr;
        const [w, h] = pageBox(d, d.w * (96 / 72) * zoom, dpr);
        // (a product a whole number but for the floats' last bits, Letter at
        // 100% say, may round either way here or in the core: a pixel at the
        // box's edge, the page still at its own pixels, Viewer.snap going by
        // its scale; elsewhere the same pixel)
        const same = (box: number, x: number, what: string) =>
          Math.abs(x - Math.round(x)) < 1e-3 ? assert.ok(Math.abs(Math.round(box * dpr) - x) <= 1, what) : assert.equal(Math.round(box * dpr), Math.ceil(x), what);
        same(w, d.w * s, `width at ${zoom} × ${dpr}`);
        same(h, d.h * s, `height at ${zoom} × ${dpr}`);
      }
});

/** A viewer of `n` pages at `zoom`, its host's asks recorded and settled by the test. */
function setup(n: number) {
  const host = {
    zoom: 1,
    asks: [] as { k: number; done: () => void }[],
    scale: () => host.zoom,
    need(k: number) {
      return new Promise<void>((done) => host.asks.push({ k, done }));
    },
    inView() {},
    box: (d: { w: number; h: number }, w: number) => pageBox(d, w),
  };
  const root = document.createElement("div"), scroller = document.createElement("div");
  scroller.append(root);
  document.body.append(scroller);
  const v = new Viewer(root, scroller, host);
  v.layout(Array.from({ length: n }, (_, k) => `h${k}`));
  seen!([...root.children].map((target: Element) => ({ target, isIntersecting: true })));
  return { v, host, root };
}

/** Page `k` as Strips.page makes it: drawn at the host's zoom now (A4, device ratio 2). */
function drawn(zoom: number) {
  const s = (96 / 72) * zoom * 2;
  const el = document.createElement("div");
  el.className = "phx-page";
  Object.assign(el.dataset, { scale: String(s), pw: String(Math.ceil(A4.w * s)), ph: String(Math.ceil(A4.h * s)) });
  const c = document.createElement("canvas");
  c.width = Math.ceil(A4.w * s);
  c.height = Math.ceil(A4.h * s);
  el.append(c);
  return { canvas: el, w: A4.w, h: A4.h };
}

test("a page is asked for once at a time, and again at the zoom it rests at", async () => {
  const { v, host } = setup(1);
  assert.deepEqual(host.asks.map((a) => a.k), [0]);
  // (zoomed twice while its drawing is on its way: not asked again yet)
  host.zoom = 1.1;
  v.redraw();
  host.zoom = 1.21;
  v.redraw();
  assert.equal(host.asks.length, 1, "asked once while one is on its way");
  // (the drawing comes, at the zoom it was asked at: asked again, once)
  v.set(0, drawn(1), "h0");
  host.asks[0].done();
  await tick();
  assert.deepEqual(host.asks.map((a) => a.k), [0, 0], "asked again at the zoom now");
  v.set(0, drawn(1.21), "h0");
  host.asks[1].done();
  await tick();
  assert.equal(host.asks.length, 2, "drawn at the zoom: not asked again");
});

test("a zoom out draws the page again (a finer bitmap is not kept, shrunk)", async () => {
  const { v, host, root } = setup(1);
  v.set(0, drawn(2), "h0");
  host.zoom = 2;
  host.asks[0].done();
  await tick();
  const n = host.asks.length;
  host.zoom = 1;
  v.redraw();
  assert.equal(host.asks.length, n + 1, "asked for at the smaller zoom");
  // (until it comes, the finer bitmap is stretched to the box)
  await tick();
  const wrap = root.querySelector(".phx-page") as HTMLElement;
  assert.equal(wrap.style.width, "100%");
});

test("a page drawn at the zoom is shown at its own pixels, with no transform", async () => {
  const { v, host, root } = setup(1);
  host.zoom = 1.37;
  v.redraw();
  v.set(0, drawn(1.37), "h0");
  await tick();
  const wrap = root.querySelector(".phx-page") as HTMLElement;
  const s = (96 / 72) * 1.37 * 2;
  assert.equal(wrap.style.width, `${Math.ceil(A4.w * s) / 2}px`);
  assert.equal(wrap.style.height, `${Math.ceil(A4.h * s) / 2}px`);
  assert.ok(!wrap.style.transform, "no transform");
});

test("a page streamed not exact is marked a preview until the build's page replaces it; one not shipped says it comes at the end", () => {
  const { v, root } = setup(3);
  const cls = (k: number) => [...root.children][k].classList;
  v.layout(["a", "", "c"], new Set(["c"]));
  assert.ok(cls(2).contains("preview"));
  assert.ok(!cls(0).contains("preview") && !cls(0).contains("later"));
  assert.ok(cls(1).contains("later") && !cls(1).contains("preview"));
  // (the settled page: another hash, the mark gone)
  v.layout(["a", "b", "c2"], new Set(["c"]));
  assert.ok(!cls(2).contains("preview") && !cls(1).contains("later"));
});

test("a zoom kept is always a zoom: NaN, Infinity, out of range or a stray word is Fit, or clamped", async () => {
  const { zoomOf } = await import("../src/panel.ts");
  // (a NaN kept with the panel's prefs made every page's scale NaN: stretched bitmaps, no zoom, every load)
  for (const bad of ["NaN", NaN, "Infinity", Infinity, -Infinity, "", "  ", "page-width", null, undefined, {}]) assert.equal(zoomOf(bad), "fit", String(bad));
  assert.equal(zoomOf("fit"), "fit");
  assert.equal(zoomOf("page"), "page");
  assert.equal(zoomOf("1.5"), "1.5");
  assert.equal(zoomOf(1.25), "1.25");
  assert.equal(zoomOf("0.01"), "0.25");
  assert.equal(zoomOf("100"), "8");
});

test("a zoom gesture's frames reach its goal, finite at every step, in and out alike", async () => {
  const { zoomToward } = await import("../src/panel.ts");
  for (const goal of [0.5, 1 / 1.1, 1.1, 1.21, 3]) {
    let f = 1;
    for (let n = 0; n < 60 && f !== goal; n++) {
      f = zoomToward(f, goal, 16);
      assert.ok(Number.isFinite(f) && f > 0, `frame ${n} toward ${goal}: ${f}`);
    }
    assert.equal(f, goal, `settled at ${goal}`);
  }
  // (a frame with no time, or a broken one, is never NaN)
  assert.equal(zoomToward(1, 1.1, 0), 1);
  assert.ok(Number.isFinite(zoomToward(1, 1.1, NaN)));
  assert.equal(zoomToward(NaN, 1.1, 16), 1.1);
});
