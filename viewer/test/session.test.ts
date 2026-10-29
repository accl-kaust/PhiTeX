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
