// Untrusted text never reaches the DOM as markup: a commit's subject, author
// and refs (a cloned repository's), a problem's message and places (TeX's
// log, a .tex file's), an outline's titles (the PDF's bookmarks) and a diff
// look's colors (a host's store, a server's state) all carry
// `<img src=x onerror=alert(1)>`, and the HTML the viewer writes must hold
// no tag made of it: every `<` that came in is escaped. The viewer page holds
// the watch's token; a tag from a document would run in it.
//
//   scripts/viewer-test.sh   (node --test, no DOM: the components write
//   their HTML into the stand-in elements below, read back as written)

import { test } from "node:test";
import assert from "node:assert/strict";

const EVIL = `<img src=x onerror=alert(1)>`;
/** No tag in `html` came from EVIL (an escaped `&lt;img` is text). */
const clean = (html: string) => assert.ok(!/<img\b/i.test(html), `a tag made of untrusted text: ${html.slice(0, 400)}`);

/** An element as the components use it: its HTML kept as written. */
class Stub {
  innerHTML = "";
  className = "";
  hidden = false;
  dataset: Record<string, string> = {};
  style: Record<string, string> = {};
  children: Stub[] = [];
  setAttribute() {}
  addEventListener() {}
  append(...c: Stub[]) {
    this.children.push(...c);
  }
  querySelectorAll() {
    return [];
  }
  querySelector() {
    return null;
  }
}
(globalThis as any).document = { createElement: () => new Stub() };

test("a commit's subject, author and refs are text", async () => {
  const { Commits } = await import("../src/commits.ts");
  const root = new Stub();
  const g = new Commits(root as unknown as HTMLElement, {
    load: async () => [{ id: "a".repeat(40), short: "aaaaaaa", parents: [], author: EVIL, time: 0, subject: EVIL, refs: [{ name: EVIL, kind: EVIL as never }] }],
    pick() {},
  });
  await g.load();
  clean(root.children[0].innerHTML);
  assert.ok(root.children[0].innerHTML.includes("&lt;img"), "the subject is shown, as text");
});

test("a repository's error is text", async () => {
  const { Commits } = await import("../src/commits.ts");
  const root = new Stub();
  const g = new Commits(root as unknown as HTMLElement, { load: async () => EVIL, pick() {} });
  await g.load();
  clean(root.children[0].innerHTML);
});

test("a problem's message, places, excerpt and context are text", async () => {
  const { problemHtml } = await import("../src/problems.ts");
  const html = problemHtml({
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
  });
  clean(html);
  assert.ok(html.includes("&lt;img"), "the message is shown, as text");
});

test("an outline's titles are text", async () => {
  const { Outline } = await import("../src/outline.ts");
  const root = new Stub();
  const o = new Outline(root as unknown as HTMLElement, { goToPlace() {} });
  o.set([{ t: EVIL, p: EVIL as never, y: null, k: [{ t: EVIL, p: 0, y: 0, k: [] }] }]);
  clean(root.children[0].innerHTML);
});

test("a diff look's colors are only #rrggbb", async () => {
  const { safeColor } = await import("../src/panel.ts");
  assert.equal(safeColor(`"><img src=x onerror=alert(1)>`, "#0000ff"), "#0000ff");
  assert.equal(safeColor("red;background:url(x)", "#ff0000"), "#ff0000");
  assert.equal(safeColor("#0072B2", "#0000ff"), "#0072B2");
});
