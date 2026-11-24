// Changes' Diff where the core runs in the browser (Overleaf, VS Code's
// builtin engine, the web demo): two versions' files diffed by PhiTeX's
// latexdiff (diff.ts, run by the core's host: its `latexdiff` op), the
// marked-up document typeset by a session of its own, so the editor's stays
// warm, on the transport the host gives (Overleaf's: another document of the
// editor's own core, its format and packs there already); one panel shows
// either (gated sinks). The marked-up document is opened as the editor's
// main file, the same job: a core that keeps the editor's persisted graph
// starts the diff's build from the editor's build, an edit of it, as
// `phitex watch`'s Compare forks the document's session.
// Against the editor's text, each edit is diffed again, 400 ms after the
// last, and goes in as an edit of the marked-up document. Where versions
// come from (Overleaf's history, history.ts; a git revision) and how a file
// is saved are the host's (CompareHost); the popover is changes.ts's.

import { diffPlace, diffSource, type DiffChange, type DiffMarkup, type DiffOut, type DiffSubtype } from "./diff.ts";
import { DIFF_COLORS, type ChangesDiffLook, type ChangesRunner, type DiffMark } from "./changes.ts";
import { byteOffset, charOffset } from "./edits.ts";
import { PreviewSession, type CoreTransport, type EditorHost, type Options, type PreviewSink } from "./session.ts";
import type { Glyph } from "./sync.ts";


/**
 * A sink that paints only while `on()`: two sessions, one panel (the
 * editor's and a compare's), the one not shown keeping its pages warm.
 * `drop`: calls never passed on (a compare's jumps to source: its source
 * is the marked-up document, not a project file).
 */
export function gate(sink: PreviewSink, on: () => boolean, drop: string[] = []): PreviewSink {
  return new Proxy(sink, {
    get(t, k, r) {
      const v = Reflect.get(t, k, r);
      if (typeof v !== "function") return v;
      return (...a: unknown[]) => (on() && !drop.includes(String(k)) ? v.apply(t, a) : undefined);
    },
  });
}

/** A compare's files: the marked-up document as `main` (diff.ts made it), the project's other files as the editor has them, nothing live. */
export class DiffHost implements EditorHost {
  private readonly tex: Record<string, string>;
  private readonly main: string;
  constructor(tex: Record<string, string>, main: string) {
    [this.tex, this.main] = [tex, main];
  }
  loadProject() {
    return Promise.resolve(this.tex);
  }
  mainFile() {
    return this.main;
  }
  onOpen() {}
  onChanges() {}
  ready() {}
}

/** What a compare needs of its host. */
export interface CompareHost<V> {
  /** The editor's session (its texts, main file and pages) and its transport (the `latexdiff` op). */
  session(): PreviewSession;
  core: CoreTransport;
  /** Called at each of the editor's edits. */
  onChanges(cb: () => void): void;
  /** Version `v`'s text files, by path (`say`: what it is doing, "Fetching version 42…", `done` of `total` where it counts). */
  files(v: V, say: (busy: string, done?: number, total?: number) => void): Promise<Record<string, string>>;
  /** A transport for the marked-up document's session (another document of the editor's core, or a core of its own), connected, the project's binary files given. */
  connect(): Promise<CoreTransport & { close(): void }>;
  /** The marked-up document's sink, painting while `on()` (gate it). */
  sink(on: () => boolean): PreviewSink;
  /** The editor at [from, to) of `file` (UTF-16), the focus kept on the page. */
  goto(file: string, from: number, to: number): void;
  save(bytes: BlobPart, type: string, name: string): void;
  msg(text: string, err?: boolean, shown?: boolean): void;
  /** The marked-up document's session's options (the editor's). */
  options: Partial<Options>;
}

/** The look as phitex-diff's request takes it: UNDERLINE, the preset's colours, the viewer's styles (Hide deletions: deleted text not shown). */
export function diffStyle(l: ChangesDiffLook, hide: boolean): { markup: DiffMarkup; subtype: DiffSubtype; add_color: string; del_color: string; ins_style: string; del_style: string; fig_style: string } {
  const [add, del] = DIFF_COLORS[l.colors] ?? DIFF_COLORS.blue;
  // (change bars: latexdiff's classic marks with them, underlined and struck)
  return { markup: l.bars ? "culinechbar" : "underline", subtype: "safe", add_color: add, del_color: del, ins_style: l.ins, del_style: hide ? "hide" : l.del, fig_style: l.fig };
}

/**
 * A compare between two versions (`to` null: the editor's text), the
 * marked-up document typeset by a session of its own; Changes' Diff drives
 * it (ChangesRunner, with the host mapping its versions' ids).
 */
export class Compare<V> {
  /** Which the panel shows: the diff (the marked-up document's pages) or the current version. */
  showing: "diff" | "current" = "current";
  /** Whether the diff's pages are wanted in view (Changes' Diff on): a diff typeset meanwhile shows only if so. */
  private want = false;
  private unopened: (() => void) | null = null;
  /** The compare's session, its transport, the versions, the diff, and the name the marked-up text has there (the editor's main file's). */
  private c: { session: PreviewSession; transport: CoreTransport & { close(): void }; base: Record<string, string>; neu: Record<string, string> | null; d: DiffOut; file: string } | undefined;
  private h: CompareHost<V>;
  private counted: ((c: DiffChange[]) => void) | undefined;
  private look: ChangesDiffLook = { colors: "blue", ins: "wavy", del: "strike", fig: "side" };
  private hide = false;

  constructor(h: CompareHost<V>) {
    this.h = h;
    let again: ReturnType<typeof setTimeout> | undefined;
    h.onChanges(() => {
      // (against the editor's text only: two past versions do not move)
      if (!this.c || this.c.neu) return;
      clearTimeout(again);
      again = setTimeout(() => void this.rediff(), 400);
    });
  }

  /** Diffed again (an edit, a new look): the marked-up document's session given the new text. */
  private async rediff(): Promise<void> {
    const c = this.c;
    const d = c && (await this.diffOf(c.base, c.neu).catch(() => null));
    if (!d || this.c !== c) return;
    c.d = d;
    c.session.sync(c.file, d.tex);
    this.counted?.(d.changes);
  }

  /** The marked-up text last made (dev: a scenario reads it). */
  tex(): string | null {
    return this.c?.d.tex ?? null;
  }

  restyle(l: ChangesDiffLook, hide: boolean): void {
    this.look = l;
    this.hide = hide;
    void this.rediff();
  }

  /** The session the panel shows: the compare's while its diff is in view. */
  shown(): PreviewSession {
    return (this.showing === "diff" && this.c?.session) || this.h.session();
  }

  /** Whether the editor's session paints (its sink's gate). */
  editorShown = () => this.showing !== "diff";

  private async diffOf(old: Record<string, string>, neu: Record<string, string> | null): Promise<DiffOut> {
    const s = this.h.session();
    const r = await this.h.core.request({ op: "latexdiff", req: { old, new: neu ?? s.texts(), main: s.main ?? "main.tex", ...diffStyle(this.look, this.hide) } });
    if (!r.ok) throw new Error(r.error ?? "latexdiff failed");
    if ("error" in r.json) throw new Error(r.json.error);
    return r.json as DiffOut;
  }

  private base(): string {
    return (this.h.session().main ?? "main.tex").replace(/\.tex$/, "").replace(/.*\//, "");
  }

  /** Version `from` diffed against `to` (null: the editor's text), in `look`: its changes; `count` as edits change them. */
  async start(from: V, to: V | null, look: ChangesDiffLook, hide: boolean, say: (busy: string, done?: number, total?: number) => void, count: (c: DiffChange[]) => void): Promise<DiffChange[]> {
    this.counted = count;
    this.look = look;
    this.hide = hide;
    const old = await this.h.files(from, say);
    const neu = to === null ? null : await this.h.files(to, say);
    say("Comparing…");
    const d = await this.diffOf(old, neu);
    // (the session kept from the last compare, its marked-up text synced: no new transport, no new start)
    if (this.c) {
      this.c.base = old;
      this.c.neu = neu;
      this.c.d = d;
      this.c.session.sync(this.c.file, d.tex);
      this.showing = this.want ? "diff" : "current";
      if (this.want) void this.c.session.resync();
      return d.changes;
    }
    const t = await this.h.connect();
    // (a jump to source from the diff's pages, a double-click or a
    // selection, is at the marked-up text: taken to the user's file
    // through the diff's map, before the host's gate drops it)
    const sink = this.h.sink(() => this.showing === "diff");
    const mapped = new Proxy(sink, {
      get: (o, k, r) => (k === "goto" ? (_f: string, from: number, to: number) => this.toSource(from, to) : Reflect.get(o, k, r)),
    });
    // (the marked-up text as the editor's main file, the project's other files beside it: the
    // editor's job, so its persisted graph holds for it; a double-click on its pages, in the
    // session's remote mode (the host's), taken to the user's file through the diff's map)
    const file = this.h.session().main ?? "main.tex";
    const editor = this.h.session().texts();
    const o = this.h.options;
    const remote = o.remote && { source: (_f: string, a: number, b: number) => this.toSourceBytes(a, b) };
    const s = new PreviewSession(new DiffHost({ ...editor, [file]: d.tex }, file), t, mapped, remote ? { ...o, remote } : o);
    this.c = { session: s, transport: t, base: old, neu, d, file };
    // (in remote mode a build's end is told, not answered: its status asked then, as the
    // editor's host does for the editor's, so the last pack fetched is not left loading)
    if (remote) t.onEvent?.((e) => void (e.event === "settled" && this.c?.session === s && (s as unknown as { core: CoreTransport }).core.request({ op: "status" })));
    this.showing = this.want ? "diff" : "current";
    // (the marked-up document is another one: never the current version's pages as its "last complete render")
    // (typeset out of view: the opening when it first comes into view)
    this.unopened = this.want ? null : () => mapped.opening?.();
    if (this.want) mapped.opening?.();
    say("Typesetting the diff…");
    await s.start();
    // (remote mode joins, as the editor's does: its files then given, its main set, which opens it)
    if (remote) {
      s.refresh({ ...editor, [file]: d.tex });
      await s.setMain(file);
    }
    return d.changes;
  }

  /** [from, to) of the marked-up text (UTF-16): the editor at its source in the user's file. */
  private toSource(from: number, to: number): void {
    const d = this.c?.d;
    if (!d) return;
    this.toSourceBytes(byteOffset(d.tex, from), byteOffset(d.tex, Math.max(from, to - 1)) + (to > from ? 1 : 0));
  }

  /** [from, to) of the marked-up text (bytes): the editor at its source in the user's file. */
  private toSourceBytes(from: number, to: number): void {
    const d = this.c?.d;
    if (!d) return;
    const a = diffSource(d, from);
    const b = diffSource(d, Math.max(from, to - 1));
    if (!a) return;
    const t = this.h.session().text(a.file);
    if (t === undefined) return;
    const end = b && b.file === a.file && b.at >= a.at ? b.at + (to > from ? 1 : 0) : a.at;
    this.h.goto(a.file, charOffset(t, a.at), charOffset(t, end));
  }

  /**
   * The editor's cursor at `pos` (UTF-16) in `file`: on the pages shown,
   * the diff's (through its map) or the current version's.
   */
  follow(file: string, pos: number): void {
    const c = this.c, session = this.h.session();
    if (this.showing !== "diff" || !c) return session.follow(file, pos);
    const t = session.text(file);
    const out = t === undefined ? null : diffPlace(c.d, file, byteOffset(t, pos));
    if (out !== null) c.session.follow(c.file, charOffset(c.d.tex, out));
  }

  show(diff: boolean): void {
    this.want = diff;
    const w = diff && this.c ? "diff" : "current";
    if (w === this.showing) return;
    this.showing = w;
    if (w === "diff" && this.unopened) (this.unopened(), (this.unopened = null));
    void this.shown().resync();
  }

  goto(k: number): void {
    const c = this.c?.d.changes[k - 1];
    if (!c || !this.c) return;
    // (the editor at the change, in its file; the page at it, in the diff or the current version)
    const session = this.h.session();
    const t = session.text(c.new.file);
    if (t !== undefined && !this.c.neu) {
      const [from, to] = [charOffset(t, c.new.start), charOffset(t, c.new.end)];
      this.h.goto(c.new.file, from, to);
      if (this.showing === "current") void session.toPage(c.new.file, from);
    }
    if (this.showing === "diff") void this.c.session.toPage(this.c.file, charOffset(this.c.d.tex, c.out[0]));
  }

  /** The editor at change `n` (from 1) in its file, against the editor's text (a past version's is nowhere to go). */
  source(n: number): void {
    const c = this.c?.d.changes[n - 1];
    const t = c && !this.c!.neu ? this.h.session().text(c.new.file) : undefined;
    if (c && t !== undefined) this.h.goto(c.new.file, charOffset(t, c.new.start), charOffset(t, c.new.end));
  }

  /**
   * The changes' places on page `k` of the diff's pages: the glyphs of
   * each change's markup there (by their origins in the marked-up text),
   * and those of its deleted part (`\DIFdelbegin` to `\DIFdelend`: a
   * caret, struck words). Asked once a page, the pointer on it.
   */
  async marks(k: number): Promise<DiffMark[]> {
    const c = this.c;
    if (!c || this.showing !== "diff") return [];
    const d = c.d;
    const gs = await c.session.pageGlyphs(k);
    if (this.c !== c) return [];
    const bytes = new TextEncoder().encode(d.tex);
    const dec = new TextDecoder();
    // (each change's deleted parts, in the marked-up text's bytes)
    const dels = d.changes.map((x) => {
      const [a, b] = x.out, part = dec.decode(bytes.subarray(a, b)), out: [number, number][] = [];
      const at = (i: number) => a + new TextEncoder().encode(part.slice(0, i)).length;
      // (a range that starts inside its deleted part, after its \DIFdelbegin: that part from the range's start)
      const e0 = part.indexOf("\\DIFdelend"), b0 = part.indexOf("\\DIFdelbegin");
      if (e0 >= 0 && (b0 < 0 || e0 < b0)) out.push([a, at(e0)]);
      for (let i = part.indexOf("\\DIFdelbegin"); i >= 0; i = part.indexOf("\\DIFdelbegin", i + 1)) {
        const e = part.indexOf("\\DIFdelend", i);
        out.push([at(i), e < 0 ? b : at(e)]);
      }
      return out;
    });
    // (a deletion's caret is made by the markup's macros, no text of its
    // own: its glyph is placed at the next call, the added part's
    // \DIFadd; a made glyph there is the deleted part's)
    const caret = (g: Glyph) => {
      if (!g.synth) return false;
      const t = dec.decode(bytes.subarray(g.start, g.end));
      return t === "\\DIFadd" || t === "\\DIFaddFL";
    };
    const out = new Map<number, DiffMark>();
    const grow = (b: [number, number, number, number] | undefined, x: number, y: number): [number, number, number, number] => (b ? [Math.min(b[0], x), Math.min(b[1], y - 8), Math.max(b[2], x + 5), Math.max(b[3], y + 3)] : [x, y - 8, x + 5, y + 3]);
    for (const g of gs) {
      if (g.file !== c.file) continue;
      // (the changes are in the text's order: the first whose markup ends past the glyph)
      let lo = 0, hi = d.changes.length;
      while (lo < hi) {
        const m = (lo + hi) >> 1;
        if (d.changes[m].out[1] <= g.start) lo = m + 1;
        else hi = m;
      }
      const x = d.changes[lo];
      if (!x || x.out[0] > g.start) continue;
      const m = out.get(lo) ?? { n: lo + 1, k, box: grow(undefined, g.x, g.y) };
      m.box = grow(m.box, g.x, g.y);
      if (dels[lo].some(([a, b]) => g.start >= a && g.start < b) || (dels[lo].length > 0 && caret(g))) m.del = grow(m.del, g.x, g.y);
      out.set(lo, m);
    }
    return [...out.values()];
  }

  async download(w: "pdf" | "tex"): Promise<void> {
    if (!this.c) return;
    if (w === "tex") return this.h.save(this.c.d.tex, "application/x-tex", `${this.base()}-diff.tex`);
    const pdf = await this.c.session.pdf();
    if (pdf?.length) this.h.save(pdf as BlobPart, "application/pdf", `${this.base()}-diff.pdf`);
    else this.h.msg("No diff PDF yet: the marked-up document's build stopped (see ⓘ diagnostics).", false, true);
  }

  stop(): void {
    this.c?.transport.close();
    this.c = undefined;
    this.unopened = null;
    if (this.showing === "diff") {
      this.showing = "current";
      void this.h.session().resync();
    }
  }

  /** As Changes drives it: versions by the host's ids (`v` of one; null: the editor's text). */
  runner(v: (id: string) => V): ChangesRunner {
    return {
      start: (p, look, hide, say, count) => this.start(v(p.from), p.to === null ? null : v(p.to), look, hide, say, count),
      show: (on) => this.show(on),
      goto: (k) => this.goto(k),
      restyle: (l, hide) => this.restyle(l, hide),
      stop: () => this.stop(),
      download: (w) => void this.download(w),
      marks: (k) => this.marks(k),
      source: (n) => this.source(n),
    };
  }
}
