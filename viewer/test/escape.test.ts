// Untrusted text never reaches the DOM as markup: a commit's subject, author
// and refs (a cloned repository's), a problem's message and places (TeX's
// log, a .tex file's), an outline's titles (the PDF's bookmarks) and a diff
// look's colors (a host's store, a server's state) all carry
// `<img src=x onerror=alert(1)>`, and no element may be made of it: it goes
// in as text (html.ts puts every value in through the DOM). The viewer page
// holds the watch's token; a tag from a document would run in it.
//
//   scripts/viewer-test.sh   (node --test, a DOM from linkedom, which the
//   script fetches once: the components build their nodes as in a page; no
//   img element may come of the text, which is there as it came)

import { test } from "node:test";
import assert from "node:assert/strict";

// (the page's globals html.ts and the components use, from linkedom)
const linkedom = await import(new URL("../../target/viewer-test/node_modules/linkedom/esm/index.js", import.meta.url).href);
const { document, window } = linkedom.parseHTML("<!doctype html><html><body></body></html>");
Object.assign(globalThis, { document, window, DOMParser: linkedom.DOMParser, Node: window.Node, HTMLTemplateElement: window.HTMLTemplateElement });

const EVIL = `<img src=x onerror=alert(1)>`;
/** No element in `el` came from EVIL, and EVIL is there, as text. */
const clean = (el: Element) => {
  assert.equal(el.querySelector("img"), null, `an element made of untrusted text: ${el.innerHTML.slice(0, 400)}`);
  assert.ok(el.textContent!.includes(EVIL), "the untrusted text is shown, as text");
};

test("a commit's subject, author and refs are text", async () => {
  const { Commits } = await import("../src/commits.ts");
  const root = document.createElement("div");
  const g = new Commits(root as unknown as HTMLElement, {
    load: async () => [{ id: "a".repeat(40), short: "aaaaaaa", parents: [], author: EVIL, time: 0, subject: EVIL, refs: [{ name: EVIL, kind: EVIL as never }] }],
    pick() {},
  });
  await g.load();
  clean(root);
});

test("a repository's error is text", async () => {
  const { Commits } = await import("../src/commits.ts");
  const root = document.createElement("div");
  const g = new Commits(root as unknown as HTMLElement, { load: async () => EVIL, pick() {} });
  await g.load();
  clean(root);
});

test("a problem's message, places, excerpt and context are text", async () => {
  const { problemNode } = await import("../src/problems.ts");
  const root = document.createElement("ol");
  root.append(
    problemNode({
      severity: EVIL as never,
      code: EVIL,
      message: EVIL,
      notes: [EVIL],
      help: [EVIL],
      suggestions: [EVIL],
      file: EVIL,
      line: EVIL as never,
      col: EVIL as never,
      excerpt: { text: EVIL + EVIL, start: 3, len: 5 },
      context: [{ name: EVIL, before: EVIL, after: EVIL }],
      included: [{ file: EVIL, line: 1 }],
      box: { lines: [1, 2], amount: 3, excerpt: EVIL },
    }),
  );
  clean(root);
});

test("an outline's titles are text", async () => {
  const { Outline } = await import("../src/outline.ts");
  const root = document.createElement("div");
  const o = new Outline(root as unknown as HTMLElement, { goToPlace() {} });
  o.set([{ t: EVIL, p: EVIL as never, y: null, k: [{ t: EVIL, p: 0, y: 0, k: [] }] }]);
  clean(root);
});

test("html``: a value is text, in an attribute too; an event handler, or a value bare in a tag, refused", async () => {
  const { html } = await import("../src/html.ts");
  const root = document.createElement("div");
  root.append(html`<p title="${EVIL}">${EVIL}</p>`);
  clean(root);
  assert.equal(root.querySelector("p")!.getAttribute("title"), EVIL);
  assert.throws(() => html`<b onclick="${"x"}">y</b>`, /event handler/);
  assert.throws(() => html`<b ${"x"}>y</b>`, /quoted attribute/);
  assert.throws(() => html`<style>${"x"}</style>`, /no value inside/);
});

test("a diff look's colors are only #rrggbb", async () => {
  const { safeColor } = await import("../src/panel.ts");
  assert.equal(safeColor(`"><img src=x onerror=alert(1)>`, "#0000ff"), "#0000ff");
  assert.equal(safeColor("red;background:url(x)", "#ff0000"), "#ff0000");
  assert.equal(safeColor("#0072B2", "#0000ff"), "#0072B2");
});
