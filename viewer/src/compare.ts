// Compare with a past version: the editor's project against a version's
// files, diffed by PhiTeX's latexdiff (diff.ts, run by the core's host: its
// `latexdiff` op), the marked-up document typeset by a session of its own on
// a transport of its own, so the editor's stays warm; one panel shows either
// (gated sinks). Each edit the editor makes is diffed again, 400 ms after the
// last, and goes in as an edit of the marked-up document. The diff bar's
// state and keys (DiffControls) drive it. Where versions come from (Overleaf's
// history, history.ts; a git revision) and how a file is saved are the
// host's (CompareHost); the version picker's DOM too (overleaf/src/diffui.ts).

import type { DiffChange, DiffMarkup, DiffSubtype } from "./diff.ts";
import { charOffset } from "./edits.ts";
import { PreviewSession, type CoreTransport, type EditorHost, type Options, type PreviewSink } from "./session.ts";
import type { DiffBar, DiffBarActions, DiffLook } from "./panel.ts";

/** latexdiff's own: UNDERLINE, SAFE, xcolor's blue and red. */
export const LATEXDIFF_LOOK: DiffLook = { markup: "underline", subtype: "safe", add_color: "#0000ff", del_color: "#ff0000" };

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

/** A compare's files: the marked-up document only (diff.ts made it), nothing live. */
export class DiffHost implements EditorHost {
  private readonly tex: Record<string, string>;
  constructor(tex: Record<string, string>) {
    this.tex = tex;
  }
  loadProject() {
    return Promise.resolve(this.tex);
  }
  onOpen() {}
  onChanges() {}
  ready() {}
}

/** What the diff bar drives (DiffControls; overleaf/src/diffui.ts picks the version). */
export interface DiffRunner<V> {
  /** Diff the version against the editor's text: its changes; `say` while it works, `count` as edits change them. */
  start(v: V, say: (busy: string) => void, count: (n: number) => void): Promise<{ changes: number }>;
  /** Show the diff or the current version. */
  show(which: "diff" | "current"): void;
  /** Go to change `k` (1-based). */
  goto(k: number): void;
  download(what: "pdf" | "tex"): void;
  /** The diff's look as the user left it. */
  look(): Promise<DiffLook>;
  /** The diff's look changed: kept, and diffed again with it. */
  restyle(look: DiffLook): void;
  stop(): void;
}

/** What a compare needs of its host. */
export interface CompareHost<V> {
  /** The editor's session (its texts, main file and pages) and its transport (the `latexdiff` op). */
  session(): PreviewSession;
  core: CoreTransport;
  /** Called at each of the editor's edits. */
  onChanges(cb: () => void): void;
  /** Version `v`'s text files, by path (`say`: what it is doing, "Fetching version 42…"). */
  files(v: V, say: (busy: string) => void): Promise<Record<string, string>>;
  /** A transport of its own for the marked-up document's session, connected, the project's binary files given. */
  connect(): Promise<CoreTransport & { close(): void }>;
  /** The marked-up document's sink, painting while `on()` (gate it). */
  sink(on: () => boolean): PreviewSink;
  /** The editor at [from, to) of `file` (UTF-16), the focus kept on the page. */
  goto(file: string, from: number, to: number): void;
  save(bytes: BlobPart, type: string, name: string): void;
  msg(text: string): void;
  /** The marked-up document's session's options (the editor's). */
  options: Partial<Options>;
  /** The diff's look as kept (any part not kept: LATEXDIFF_LOOK's), and keeping it. */
  loadLook(): Promise<Partial<DiffLook> | undefined>;
  saveLook(l: DiffLook): void;
}

type DiffJson = { tex: string; changes: DiffChange[] };

/** A compare: the runner the diff bar drives, and which session the panel shows. */
export class Compare<V> implements DiffRunner<V> {
  /** Which the panel shows: the diff (the marked-up document's pages) or the current version. */
  showing: "diff" | "current" = "current";
  private c: { session: PreviewSession; transport: CoreTransport & { close(): void }; base: Record<string, string>; tex: string; changes: DiffChange[] } | undefined;
  private h: CompareHost<V>;
  private counted: ((n: number) => void) | undefined;
  private looks: DiffLook | undefined;

  constructor(h: CompareHost<V>) {
    this.h = h;
    let again: ReturnType<typeof setTimeout> | undefined;
    h.onChanges(() => {
      if (!this.c) return;
      clearTimeout(again);
      again = setTimeout(() => void this.rediff(), 400);
    });
  }

  /** Diffed again (an edit, a new look): the marked-up document's session given the new text. */
  private async rediff(): Promise<void> {
    const c = this.c;
    const d = c && (await this.diffOf(c.base).catch(() => null));
    if (!d || this.c !== c) return;
    c.tex = d.tex;
    c.changes = d.changes;
    c.session.sync("diff.tex", d.tex);
    this.counted?.(d.changes.length);
  }

  async look(): Promise<DiffLook> {
    this.looks ??= { ...LATEXDIFF_LOOK, ...(await this.h.loadLook().catch(() => undefined)) };
    return this.looks;
  }

  restyle(l: DiffLook): void {
    this.looks = l;
    this.h.saveLook(l);
    void this.rediff();
  }

  /** The session the panel shows: the compare's while its diff is in view. */
  shown(): PreviewSession {
    return (this.showing === "diff" && this.c?.session) || this.h.session();
  }

  /** Whether the editor's session paints (its sink's gate). */
  editorShown = () => this.showing !== "diff";

  private async diffOf(old: Record<string, string>): Promise<DiffJson> {
    const s = this.h.session();
    const l = await this.look();
    const style = { markup: l.markup as DiffMarkup, subtype: l.subtype as DiffSubtype, add_color: l.add_color, del_color: l.del_color };
    const r = await this.h.core.request({ op: "latexdiff", req: { old, new: s.texts(), main: s.main ?? "main.tex", ...style } });
    if (!r.ok) throw new Error(r.error ?? "latexdiff failed");
    if ("error" in r.json) throw new Error(r.json.error);
    return r.json as DiffJson;
  }

  private base(): string {
    return (this.h.session().main ?? "main.tex").replace(/\.tex$/, "").replace(/.*\//, "");
  }

  async start(v: V, say: (busy: string) => void, count: (n: number) => void): Promise<{ changes: number }> {
    this.counted = count;
    const old = await this.h.files(v, say);
    say("Comparing…");
    const d = await this.diffOf(old);
    this.stop();
    const t = await this.h.connect();
    const s = new PreviewSession(new DiffHost({ "diff.tex": d.tex }), t, this.h.sink(() => this.showing === "diff"), this.h.options);
    this.c = { session: s, transport: t, base: old, tex: d.tex, changes: d.changes };
    this.showing = "diff";
    say("Typesetting the diff…");
    await s.start();
    return { changes: d.changes.length };
  }

  show(w: "diff" | "current"): void {
    this.showing = w;
    void this.shown().resync();
  }

  goto(k: number): void {
    const c = this.c?.changes[k - 1];
    if (!c || !this.c) return;
    // (the editor at the change, in its file; the page at it, in the diff or the current version)
    const session = this.h.session();
    const t = session.text(c.new.file);
    if (t !== undefined) {
      const [from, to] = [charOffset(t, c.new.start), charOffset(t, c.new.end)];
      this.h.goto(c.new.file, from, to);
      if (this.showing === "current") void session.toPage(c.new.file, from);
    }
    if (this.showing === "diff") void this.c.session.toPage("diff.tex", charOffset(this.c.tex, c.out[0]));
  }

  async download(w: "pdf" | "tex"): Promise<void> {
    if (!this.c) return;
    if (w === "tex") return this.h.save(this.c.tex, "application/x-tex", `${this.base()}-diff.tex`);
    const pdf = await this.c.session.pdf();
    if (pdf?.length) this.h.save(pdf as BlobPart, "application/pdf", `${this.base()}-diff.pdf`);
    else this.h.msg("No diff PDF yet: the marked-up document's build stopped (see ⓘ diagnostics).");
  }

  stop(): void {
    this.c?.transport.close();
    this.c = undefined;
    if (this.showing === "diff") {
      this.showing = "current";
      void this.h.session().resync();
    }
  }
}

/** What of the panel the diff bar uses (Panel). */
export interface DiffPanel {
  diffBar(d: DiffBar | null, on?: DiffBarActions): void;
  diffSettings(look: DiffLook | null, change?: (l: DiffLook) => void, defaults?: DiffLook): void;
}

/**
 * The diff bar (Panel.diffBar) while comparing: against which version, the
 * changes and the one at, and its controls and keys: d (diff / current), n
 * and Shift+N (next / previous change), Escape (the look's popover, else
 * stop). Its ⚙: the look (Panel.diffSettings), diffed again on each pick.
 */
export class DiffControls<V> {
  private state: DiffBar | null = null;
  private panel: DiffPanel;
  private run: DiffRunner<V>;
  private actions: DiffBarActions;
  private lookOpen = false;

  constructor(panel: DiffPanel, run: DiffRunner<V>) {
    this.panel = panel;
    this.run = run;
    this.actions = {
      prev: () => this.step(-1),
      next: () => this.step(1),
      toggle: () => {
        const st = this.state;
        if (!st || st.busy) return;
        st.showing = st.showing === "diff" ? "current" : "diff";
        run.show(st.showing);
        this.bar();
      },
      pdf: () => run.download("pdf"),
      tex: () => run.download("tex"),
      settings: async () => {
        if (this.lookOpen) return this.closeLook();
        this.lookOpen = true;
        panel.diffSettings(await run.look(), (l) => run.restyle(l), LATEXDIFF_LOOK);
      },
      close: () => this.stop(),
    };
  }

  private closeLook(): void {
    this.lookOpen = false;
    this.panel.diffSettings(null);
  }

  /** A compare is on (its bar shown). */
  get active(): boolean {
    return !!this.state;
  }

  private bar(): void {
    this.panel.diffBar(this.state, this.actions);
  }

  private step(d: number): void {
    const st = this.state;
    if (!st || st.busy || !st.changes) return;
    st.at = ((Math.max(st.at, d > 0 ? 0 : 1) - 1 + d + st.changes) % st.changes) + 1;
    if (st.showing === "current") this.actions.toggle!();
    this.run.goto(st.at);
    this.bar();
  }

  /** Compare with `v`, named `title`, `when` it was ("3 h ago · v42"). */
  async start(v: V, title: string, when: string): Promise<void> {
    this.state = { title, when, changes: 0, at: 0, showing: "diff", busy: "Fetching that version…" };
    this.bar();
    try {
      const r = await this.run.start(
        v,
        (busy) => {
          if (this.state) (this.state.busy = busy), this.bar();
        },
        (n) => {
          const st = this.state;
          if (!st || st.busy) return;
          st.changes = n;
          st.at = Math.min(st.at, n);
          this.bar();
        },
      );
      if (!this.state) return;
      this.state.busy = undefined;
      this.state.changes = r.changes;
    } catch (err) {
      if (!this.state) return;
      this.state.busy = `Couldn't compare: ${(err as Error).message ?? err}`;
    }
    this.bar();
  }

  stop(): void {
    this.closeLook();
    this.state = null;
    this.run.stop();
    this.bar();
  }

  /** A key while comparing (not in an editor or a field): handled, or false. */
  key(e: KeyboardEvent): boolean {
    if (!this.state || e.ctrlKey || e.metaKey || e.altKey) return false;
    const t = e.target as HTMLElement | null;
    if (t?.closest?.("input, textarea, [contenteditable=true], .cm-editor")) return false;
    if (e.key === "d" || e.key === "D") this.actions.toggle!();
    else if (e.key === "n") this.step(1);
    else if (e.key === "N") this.step(-1);
    else if (e.key === "Escape") this.lookOpen ? this.closeLook() : this.stop();
    else return false;
    e.preventDefault();
    return true;
  }
}
