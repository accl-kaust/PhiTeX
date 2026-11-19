// A page drawn while a build comes in (issue #56): the drawing is of the
// page the core had when it answered, which may be a page streamed mid-build
// (a pass's page, rows of the build before still on it), and the build's
// own page comes after. The drawing is said to be of the page it was asked
// for, never of the page the session knows when the answer arrives: said to
// be the build's, the view counted the build's page drawn and never asked
// for it, and kept the streamed page (in Overleaf a band of the frame before
// left at the bottom, until the next edit).
//
//   scripts/viewer-test.sh   (node --test, a DOM from linkedom)

import { test } from "node:test";
import assert from "node:assert/strict";

const linkedom = await import(new URL("../../target/viewer-test/node_modules/linkedom/esm/index.js", import.meta.url).href);
const { document, window } = linkedom.parseHTML("<!doctype html><html><body></body></html>");
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
window.devicePixelRatio = 1;

const { pageBox } = await import("../src/panel.ts");
const { Viewer } = await import("../src/viewer.ts");
const { PreviewSession } = await import("../src/session.ts");

const tick = () => new Promise((r) => setTimeout(r, 5));

test("a page drawn as the build came in is drawn again from the build's page", async () => {
  // (the core: its pages' hashes and what a drawing of page 0 shows now; a
  // drawing is made when the core answers, and its answer may be held, as a
  // core mid-build answers between its steps and the reply crosses a port)
  const core = { hashes: ["A"], shows: "A", hold: false };
  const held: (() => void)[] = [];
  const listeners: ((e: never) => void)[] = [];
  const page = (shows: string) => {
    const el = document.createElement("div");
    el.className = "phx-page";
    // (drawn at the view's zoom, as Strips.page draws it: 100 by 140 points, device ratio 1)
    const sc = 96 / 72;
    Object.assign(el.dataset, { shows, scale: String(sc), pw: String(Math.ceil(100 * sc)), ph: String(Math.ceil(140 * sc)) });
    const c = document.createElement("canvas");
    [c.width, c.height] = [Math.ceil(100 * sc), Math.ceil(140 * sc)];
    el.append(c);
    return el;
  };
  const transport = {
    request: async (r: { op: string }) => {
      if (r.op === "pages") return { ok: true, json: { pages: [...core.hashes], inexact: [] } };
      // (the frontends' strips: the page drawn, no identity said with it)
      if (r.op === "png") {
        const shows = core.shows;
        const reply = { ok: true, canvas: page(shows), size: [100, 140] as [number, number] };
        if (!core.hold) return reply;
        return new Promise((ok) => held.push(() => ok(reply)));
      }
      return { ok: true, json: {} };
    },
    onEvent: (cb: (e: never) => void) => void listeners.push(cb),
  };
  const say = (e: object) => listeners.forEach((f) => f(e as never));
  const root = document.createElement("div"), scroller = document.createElement("div");
  scroller.append(root);
  document.body.append(scroller);
  let session!: InstanceType<typeof PreviewSession>;
  const asked: number[] = [];
  const v = new Viewer(root, scroller, {
    scale: () => 1,
    need: (k: number) => (asked.push(k), session.fetch(k)),
    inView() {},
    box: (d: { w: number; h: number }, w: number) => pageBox(d, w),
  });
  const sink = new Proxy({ layout: (hs: string[], p?: ReadonlySet<string>) => v.layout(hs, p), page: (img: never, k: number, _n: number, hash?: string | null) => v.set(k, img, hash ?? null) } as Record<string, unknown>, {
    get: (t, k: string) => t[k] ?? (() => undefined),
  });
  const host = { loadProject: async () => ({}), onOpen() {}, onChanges() {} };
  // (the extension's: the core's events say when the pages are in)
  session = new PreviewSession(host as never, transport as never, sink as never, { remote: { source() {} }, checkEveryMs: 0 });
  await session.start();
  seen!([...root.children].map((target: Element) => ({ target, isIntersecting: true })));
  const shown = () => (root.querySelector(".phx-page") as HTMLElement | null)?.dataset.shows;
  for (let i = 0; i < 10 && shown() !== "A"; i++) await tick();
  assert.equal(shown(), "A", "the first build's page");
  // (an edit's build: page 0 streamed mid-build, a pass's page, and drawn;
  // the core answers that drawing before the build is in, the reply comes after)
  core.hold = true;
  core.shows = "streamed";
  say({ event: "page", k: 0, hash: "S", pages: 1, exact: true });
  await tick();
  assert.equal(held.length, 1, "the streamed page asked for");
  // (the build in: its own page, another than the streamed one)
  core.hashes = ["B"];
  core.shows = "B";
  core.hold = false;
  say({ event: "settled", ms: 5, since_ms: 6 });
  for (let i = 0; i < 5; i++) await tick();
  held.shift()!();
  for (let i = 0; i < 20 && shown() !== "B"; i++) await tick();
  assert.equal(shown(), "B", `the build's page shown, not the streamed one (asked ${asked.length} times)`);
});
