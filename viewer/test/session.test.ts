// The session's copy of a file against the editor's: a change carrying the
// editor's length after it, a copy that drifted (edits applied twice, as an
// Overleaf history restore's were) asks the host for the whole text again,
// once until it comes, and the text that comes is diffed in.
import { test } from "node:test";
import assert from "node:assert/strict";
import { PreviewSession, type EditorHost } from "../src/session.ts";
import type { Edit } from "../src/edits.ts";

test("a change whose length is not the session's asks the editor for its text again", () => {
  let open!: (f: string, t: string) => void;
  let change!: (f: string, es: Edit[], len?: number) => void;
  const asked: string[] = [];
  const host: EditorHost = {
    loadProject: async () => ({}),
    onOpen: (cb) => (open = cb),
    onChanges: (cb) => (change = cb),
    resend: (f) => asked.push(f),
  };
  const core = { request: async () => ({ ok: true, json: {} }) };
  const sink = new Proxy({}, { get: () => () => undefined });
  const s = new PreviewSession(host, core as never, sink as never, { schedule: () => undefined });
  open("main.tex", "hello world");
  // (agreeing: nothing asked)
  change("main.tex", [{ from: 5, to: 5, text: "," }], 12);
  assert.equal(s.text("main.tex"), "hello, world");
  assert.deepEqual(asked, []);
  // (the editor says 18 after " there": the session has 18 too; then a
  // change the session got twice: its copy 4 longer than the editor's)
  change("main.tex", [{ from: 12, to: 12, text: " there" }], 18);
  change("main.tex", [{ from: 0, to: 0, text: "Oh, " }], 22);
  change("main.tex", [{ from: 0, to: 0, text: "Oh, " }], 22);
  assert.deepEqual(asked, ["main.tex"]);
  // (once until the text comes)
  change("main.tex", [{ from: 0, to: 0, text: "!" }], 23);
  assert.deepEqual(asked, ["main.tex"]);
  // (the whole text again: diffed in, the copy the editor's)
  open("main.tex", "!Oh, hello, world there");
  assert.equal(s.text("main.tex"), "!Oh, hello, world there");
  change("main.tex", [{ from: 23, to: 23, text: "." }], 24);
  assert.deepEqual(asked, ["main.tex"]);
});

// Collaborators' cursors (Overleaf's): each the word at its place marked on
// the page whose glyphs are here, in its hue with its label, never through
// the user's own (scrolling) mark; a page whose glyphs come later shows it
// then; [] clears.
test("collaborators' cursors mark their words on the pages, apart from the user's", () => {
  let open!: (f: string, t: string) => void;
  const host: EditorHost = { loadProject: async () => ({}), onOpen: (cb) => (open = cb), onChanges: () => undefined };
  const core = { request: async () => ({ ok: false }) };
  const shown: unknown[][] = [];
  const marked: number[] = [];
  const sink = new Proxy(
    { others: (m: unknown[]) => shown.push(m), mark: (k: number) => marked.push(k) },
    { get: (t, p) => (t as Record<string | symbol, unknown>)[p] ?? (() => undefined) },
  );
  const s = new PreviewSession(host, core as never, sink as never, { schedule: () => undefined });
  open("main.tex", "hello world\nsecond line\n");
  const w = s as unknown as { opened: boolean; hashes: string[]; glyphCache: Map<string, unknown[]>; drawPeers(): void };
  w.opened = true;
  w.hashes = ["a", "b"];
  // (page 1 has "world" at bytes 6..11, a glyph a byte, 6 pt apart)
  const g = (i: number) => ({ x: 72 + 6 * i, y: 100, file: "main.tex", start: i, end: i + 1, synth: false });
  w.glyphCache.set("1:b", [6, 7, 8, 9, 10].map(g));
  s.others("main.tex", [
    { label: "Ann", hue: 200, pos: 8 },
    { label: "Bo", hue: 20, pos: 15 },
  ]);
  const last = () => shown[shown.length - 1] as { k: number; boxes: number[][]; label: string; hue: number }[];
  // (Ann in "world" on page 1; Bo's line not on a page here: not shown)
  assert.deepEqual(last().map((m) => [m.k, m.label, m.hue]), [[1, "Ann", 200]]);
  assert.ok(last()[0].boxes.length > 0);
  // (Bo's page's glyphs come: Bo shown too)
  w.glyphCache.set("0:a", [12, 13, 14, 15, 16, 17].map(g));
  w.drawPeers();
  assert.deepEqual(last().map((m) => [m.k, m.label]), [[1, "Ann"], [0, "Bo"]]);
  // (never the user's own mark, which scrolls)
  assert.deepEqual(marked, []);
  s.others("main.tex", []);
  assert.deepEqual(last(), []);
});
