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
  // (a build makes page 1 again: its new hash's glyphs not here yet, Ann
  // keeps her mark, no blink; they come and she is drawn from them)
  const before = last()[0];
  w.hashes = ["a", "c"];
  w.drawPeers();
  assert.deepEqual(last().map((m) => [m.k, m.label]), [[1, "Ann"], [0, "Bo"]]);
  assert.deepEqual(last()[0], before);
  w.glyphCache.set("1:c", [6, 7, 8, 9, 10].map((i) => ({ ...g(i), x: 80 + 6 * i })));
  w.drawPeers();
  assert.notDeepEqual(last()[0].boxes, before.boxes);
  // (her page here and her word not on it: dropped)
  w.glyphCache.set("1:c", [18, 19].map(g));
  w.drawPeers();
  assert.deepEqual(last().map((m) => m.label), ["Bo"]);
  s.others("main.tex", []);
  assert.deepEqual(last(), []);
});

// A problem's culprit on the pages (the wavy underline): its bytes' glyphs,
// on every page that typesets them.
test("a range's boxes on the pages", async () => {
  let open!: (f: string, t: string) => void;
  const host: EditorHost = { loadProject: async () => ({}), onOpen: (cb) => (open = cb), onChanges: () => undefined };
  const core = { request: async () => ({ ok: false }) };
  const sink = new Proxy({}, { get: () => () => undefined });
  const s = new PreviewSession(host, core as never, sink as never, { schedule: () => undefined });
  open("main.tex", "hello \\oops world\n");
  const w = s as unknown as { opened: boolean; hashes: string[]; glyphCache: Map<string, unknown[]> };
  w.opened = true;
  w.hashes = ["a"];
  const g = (i: number) => ({ x: 72 + 6 * i, y: 100, file: "main.tex", start: i, end: i + 1, synth: false });
  w.glyphCache.set("0:a", [0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 12, 13].map(g));
  const got = await s.boxesOf("main.tex", 6, 11);
  assert.equal(got.length, 1);
  assert.equal(got[0].k, 0);
  assert.ok(got[0].boxes.length > 0);
  assert.deepEqual(await s.boxesOf("other.tex", 0, 3), []);
  // (as TeX names it, "./main.tex"; a culprit not typeset, an undefined
  // command's name at 14..20: the typeset word nearest it, bytes 12..13)
  const near = await s.boxesOf("./main.tex", 14, 20);
  assert.equal(near.length, 1);
  assert.deepEqual(near[0].boxes, (await s.boxesOf("main.tex", 12, 14))[0].boxes);
});

// A core that fetches its own packs (Shelf, through the host): the host's
// pack events are the panel's package state, loading then done with their
// bytes, or failed (it said "0 package files" all through the downloads).
test("the host's pack events are the packages fetched", () => {
  const host: EditorHost = { loadProject: async () => ({}), onOpen: () => undefined, onChanges: () => undefined };
  let emit!: (e: unknown) => void;
  const core = { request: async () => ({ ok: false }), onEvent: (cb: (e: unknown) => void) => (emit = cb) };
  const told: { loading: string[]; done?: string[]; failed?: { name: string }[]; bytes?: number }[] = [];
  const sink = new Proxy({ packages: (p: (typeof told)[number]) => told.push(p) }, { get: (t, p) => (t as Record<string | symbol, unknown>)[p] ?? (() => undefined) });
  new PreviewSession(host, core as never, sink as never, { schedule: () => undefined });
  emit({ event: "pack", id: "latex-1", state: "fetching" });
  emit({ event: "pack", id: "amsmath-1", state: "fetching" });
  assert.deepEqual(told.at(-1)!.loading, ["latex-1", "amsmath-1"]);
  emit({ event: "pack", id: "latex-1", state: "done", bytes: 3000 });
  emit({ event: "pack", id: "amsmath-1", state: "failed" });
  emit({ event: "pack", id: "latex-1", state: "done", bytes: 3000 });
  const last = told.at(-1)!;
  assert.deepEqual(last.loading, []);
  assert.deepEqual(last.done, ["latex-1"]);
  assert.equal(last.bytes, 3000);
  assert.deepEqual(last.failed?.map((f) => f.name), ["amsmath-1"]);
});

// A core that queues an edit (phitex-live's view: the build comes later,
// with `settled`): the keystroke is timed then, with that build's own time
// and parts, never the last build's (a reopen's 7.8 s said as a letter's).
test("a queued keystroke is timed by its own build", () => {
  const host: EditorHost = { loadProject: async () => ({}), onOpen: () => undefined, onChanges: () => undefined };
  const said: string[] = [];
  const sink = new Proxy({ latency: (s: string, d?: string) => said.push(`${s}\n${d}`) }, { get: (t, p) => (t as Record<string | symbol, unknown>)[p] ?? (() => undefined) });
  const s = new PreviewSession(host, { request: async () => ({ ok: false }) } as never, sink as never, { schedule: () => undefined });
  const w = s as unknown as { queued: { t0: number; tSend: number; tBack: number } | null; queuedLatency(e: unknown): void; now(): number };
  // (no keystroke queued: a reopen's settled says nothing)
  w.queuedLatency({ event: "settled", ms: 7847, ran: 0, kept: 0 });
  assert.deepEqual(said, []);
  const t = w.now();
  w.queued = { t0: t - 5, tSend: t - 4, tBack: t - 3 };
  w.queuedLatency({ event: "settled", ms: 2.5, ran: 3, kept: 97, passes: 1 });
  assert.equal(said.length, 1);
  assert.match(said[0], /build 2\.5 ms/);
  assert.match(said[0], /rebuilt +3 of 100 steps, 1 pass/);
  assert.equal(w.queued, null);
});
