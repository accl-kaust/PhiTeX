// A live preview session, independent of the editor and of where the core
// runs: the project's texts, the edits batched once a frame and sent in
// order, page-first painting, the latencies, and the debug check.
//
// An editor is an EditorHost (Overleaf: the page hook + the ZIP; VS Code:
// its workspace API), the core is reached through a CoreTransport (a chrome
// port to the offscreen document's core host; VS Code's, in process:
// corehost.ts), and the preview is drawn by a PreviewSink (the shadow-DOM
// panel; in VS Code the same panel in a webview, told over messages).

import { Batch, byteOffset, charOffset, type Edit } from "./edits.ts";
import type { DiffReq } from "./diff.ts";
import { ENGINES, type Engine } from "./engines.ts";
import { report } from "./report.ts";

/** Whether a source loads minted (as the worker's own test, worker.ts). */
/** A main file's preamble: what its engine is read from (engines.ts `needs`). */
const preambleOf = (t: string | undefined) => {
  const end = t?.indexOf("\\begin{document}") ?? -1;
  return end < 0 ? (t ?? "") : t!.slice(0, end);
};
const usesMinted = (t: string) => /\\(usepackage|RequirePackage)\s*(\[[^\]]*\])?\s*\{[^}]*\bminted\b/.test(t);
import { boxes, from, glyphs, lineAt, nearest, wordBytes, type Glyph } from "./sync.ts";
import { diagnose, type Diagnostic, type TexError } from "./diagnostics.ts";
import { DELIVERED, isPackageFile, referenced, noPackages, type PackageSource, type PackageState } from "./packages.ts";
import { asDiagnostic, type Problem } from "./problems.ts";
import type { Entry } from "./outline.ts";

/** The events Options.remote's host tells (WatchEvent), routed to onWatch. */
const WATCH = new Set(["page", "progress", "diagnostics", "settled", "superseded", "sync"]);

/** An editor. Edits are sequential (each against the text just before it), in UTF-16. */
export interface EditorHost {
  /** Every text file of the project, by path. */
  loadProject(): Promise<Record<string, string>>;
  /** The main file among them, if the host knows it (else guessed). */
  mainFile?(files: Record<string, string>): string | null;
  onOpen(cb: (file: string, text: string) => void): void;
  onChanges(cb: (file: string, edits: Edit[]) => void): void;
  /** Called once the session is ready for onOpen (the host may re-announce the open file). */
  ready?(): void;
}

/** The core's requests (worker.ts's `Req`, less the routing fields). */
export type CoreReq =
  | { op: "open"; main: string; files: Record<string, string>; fuel: number; engine?: Engine; workers?: 1 }
  | { op: "edit"; file: string; start: number; end: number; text: string; page: number; dpi: number }
  | { op: "set_file"; file: string; text: string }
  | { op: "png"; page: number; dpi: number }
  | { op: "pdf" }
  | { op: "status" }
  | { op: "log" }
  | { op: "pages" }
  | { op: "origins"; page: number }
  | { op: "trace"; on: boolean }
  | { op: "auxdump" }
  | { op: "check"; file?: string; expect?: string }
  /** (answered by the offscreen document, shelf.ts: not the core) */
  | { op: "package"; name: string; engine?: string }
  /** (answered by the offscreen document, diff.ts: PhiTeX's latexdiff) */
  | { op: "latexdiff"; req: DiffReq };

// (the page types the renderer shares: PhiTeX's viewer/src, vendored)
import type { Draws, PageImage } from "./types.ts";
export type { Draws, PageImage };

export interface CoreRes {
  ok: boolean;
  json?: any;
  png?: Uint8Array;
  draws?: Draws;
  pdf?: Uint8Array;
  /** (`package`: the file's text, null: none; and where from) */
  text?: string | null;
  from?: string;
  /** (a page drawn from the PDF, in the tab: its canvas and size in PDF points) */
  canvas?: HTMLElement;
  size?: [number, number];
  /** (`package`: a binary file, handed to the core by the offscreen document) */
  delivered?: boolean;
  error?: string;
}

/** A request, for the trace: its op and fields, texts by their length. */
function traceReq(r: CoreReq): Record<string, unknown> {
  const o: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(r)) {
    if (k === "op" || k === "id") continue;
    o[k] = typeof v === "string" && v.length > 40 ? `${v.length} chars` : v && typeof v === "object" ? `${Object.keys(v).length} keys` : v;
  }
  return o;
}
/** A reply, for the trace: what the core said, briefly. */
function traceRes(r: CoreRes): Record<string, unknown> {
  const j = (r.json ?? {}) as Record<string, any>;
  return {
    ok: r.ok,
    ...(r.error ? { error: r.error } : {}),
    ...(typeof j.how === "string" ? { how: j.how.split("\n")[0].slice(0, 140) } : {}),
    ...(j.pages !== undefined ? { pages: Array.isArray(j.pages) ? j.pages.length : j.pages } : {}),
    ...(j.history !== undefined ? { history: j.history } : {}),
    ...(Array.isArray(j.missing) && j.missing.length ? { missing: j.missing.length } : {}),
    ...(j.error ? { texError: typeof j.error === "string" ? j.error : j.error.message } : {}),
    ...(j.build_ms !== undefined ? { build_ms: Math.round(j.build_ms) } : {}),
    ...(r.pdf ? { pdf: r.pdf.length } : {}),
    // (a page from the draw worker: its draw time, and whether it was drawn ahead)
    ...(j.draw_ms !== undefined ? { drawer: true, draw_ms: Math.round(j.draw_ms), kept: j.kept } : {}),
  };
}

/**
 * A streamed open's (the plain build's): each page as it is shipped, how far
 * it is, and its end, with what the open would have answered.
 */
export type StreamEvent =
  | { event: "page"; pages: number; hashes: string[] }
  | { event: "progress"; phase: string; pass?: number; pages: number; ms?: number }
  | { event: "done"; ok: boolean; json?: any; error?: string };

/**
 * `phitex watch`'s events, the host building from disk (Options.remote,
 * crates/partex-cli/src/view.rs): a page shipped while the build runs, how
 * far it is, its problems, the build in (or a pass of it, settling), a
 * newer save superseding it, and a forward search's place.
 */
export type WatchEvent =
  | { event: "page"; k: number; hash: string; pages?: number }
  | { event: "progress"; pass?: number; pages?: number; phase?: string; ms?: number }
  | { event: "diagnostics"; items: Problem[] }
  | { event: "settled"; settling?: boolean; pass?: number }
  | { event: "superseded" }
  | { event: "sync"; file: string; lo: number; hi: number; at: number; page?: number; y?: number };

export type CoreEvent = StreamEvent | WatchEvent | { event: "fetching"; pack: string; name: string; failed?: boolean } | { event: "preparing"; on: boolean } | { event: "settled" } | { event: "switched" } | { event: "release"; release?: { release: string; min_extension?: string; notice?: string | null } };

export interface CoreTransport {
  request(req: CoreReq): Promise<CoreRes>;
  /** Called when the core is lost (the session reopens). */
  onLost?(cb: () => void): void;
  /** What the core says unasked: a Shelf pack it is fetching mid-build. */
  onEvent?(cb: (e: CoreEvent) => void): void;
}

export interface Status {
  pages: number;
  /** What the source and the build tell, worst first (diagnostics.ts). */
  diagnostics?: Diagnostic[];
  /** Values pending (unsupported, or out of fuel); undefined: not known yet. */
  pending?: number;
  main: string | null;
  file: string | null;
}

export interface PreviewSink {
  /** Page `k` of `n` drawn (`hash`: its box's, if known). */
  page(img: PageImage | null, k: number, n: number, hash?: string | null): void;
  /** The pages there are, by their boxes' hashes (a continuous view draws the changed ones it shows). */
  layout?(hashes: string[]): void;
  status(s: Status): void;
  /** A one-line summary, and (optionally) the details behind it. */
  latency(summary: string, details?: string): void;
  /** Packages being downloaded (not the project's files: say so), and those not found. */
  packages?(p: PackageState): void;
  /** A build is running (true) or none is (false): the view says so if it takes a while. */
  busy?(on: boolean): void;
  /** A build is in flight. */
  busy?(on: boolean): void;
  /** The core readies its next rebuild (pages still draw; an edit waits for it). */
  preparing?(on: boolean): void;
  /**
   * Between the first paint (a plain build) and the SSA program: "preparing"
   * since `since` (Date.now(): the detached tab's clock too), `slow` once an edit was typed in it
   * (a plain rebuild); then "ready", once.
   */
  warmup?(w: { state: "preparing" | "ready"; since?: number; slow?: boolean }): void;
  /** The document's outline (its PDF's bookmarks), after each build: Options.remote's, the `outline` op's. */
  outline?(items: Entry[]): void;
  /** A streamed open: the pages shipped so far (null: it ended). */
  streaming?(s: { pages: number; phase: string } | null): void;
  /** A repaint is on screen, `ms` after the keystroke that made it. */
  painted?(ms: number): void;
  /** Put the editor at `file`'s [from, to) (UTF-16 offsets), opening it if need be. */
  goto?(file: string, from: number, to: number, focus?: boolean): void;
  /** The editor's selection, highlighted on its pages until it changes ([] clears). */
  marks?(marks: { k: number; boxes: [number, number, number, number][] }[]): void;
  /** Highlight boxes on page `k` ([x, y top, w, h], PDF points), scrolled into view. */
  mark?(k: number, boxes: [number, number, number, number][], scroll?: boolean): void;
  /**
   * The engine the project runs with: `ready` false, ⚡ Instant can't run it
   * (yet); `needed` the engine a build error asked for, if any.
   */
  engine?(e: { engine: Engine; ready: boolean; needed?: Engine | null; approx?: boolean }): void;
  error(e: string): void;
  check?(r: { ok: boolean; ms: number; mismatch?: string }): void;
  mains?(names: string[], main: string | null): void;
}

export interface Options {
  fuel: number;
  /** 1: the one-worker start (the offscreen document's two-worker start otherwise, where the machine allows it). */
  workers?: 1;
  /** Pages as draw lists (vector: real text, a few KB) or PNGs (PhiTeX's: a grey box a glyph). */
  format: "vector" | "pdfjs";
  dpi: number;
  checkEveryMs: number;
  /** Where files the project doesn't have come from (packages.ts; default: nowhere). */
  packages?: PackageSource;
  /** The engine to run the project with, given its main file's text (default: pdfLaTeX). */
  engine?: (main: string | undefined) => Engine;
  /**
   * A XeLaTeX project pdfLaTeX can approximate (engines.ts `approximable`),
   * run with these stand-ins (extension/shims/: fonts ignored) instead of
   * not at all; absent, such a project isn't built.
   */
  shims?: (main: string | undefined) => Promise<Record<string, string> | null>;
  /**
   * The host builds from disk (`phitex watch`): no project sent, no edits;
   * the pages, problems and outline as its events tell; a double-click's
   * source opened by it (`source`, bytes of the file as it is on disk).
   */
  remote?: { source(file: string, start: number, end: number): void };
  /** Schedules a flush (default: the next task). */
  schedule?: (f: () => void) => void;
  now?: () => number;
}

// (fuel is per chunk: 10^5 steps is far more than plain documents need, and
// a runaway recursion costs ~70 ms a keystroke, not 700)
const defaults: Options = { fuel: 100_000, format: "vector", dpi: 96, checkEveryMs: 5000 };

const image = (r: CoreRes): PageImage | null =>
  r.draws
    ? { draws: r.draws }
    : r.canvas && r.size
      ? { canvas: r.canvas, w: r.size[0], h: r.size[1] }
      : r.png?.length
        ? { png: r.png, w: r.size?.[0], h: r.size?.[1] }
        : null;

export function guessMain(files: Record<string, string>): string | null {
  const names = Object.keys(files).filter((n) => /\.tex$/i.test(n));
  const score = (n: string) => {
    const t = files[n];
    let s = 0;
    if (/(^|\n)[^%\n]*\\(bye|end\{document\})/.test(t)) s += 4;
    if (/(^|\n)\s*\\documentclass/.test(t)) s += 2;
    if (/^main\.tex$/i.test(n)) s += 1;
    if (!n.includes("/")) s += 1;
    return s;
  };
  return names.sort((a, b) => score(b) - score(a))[0] ?? null;
}

/** `text` replacing `old`, as the one edit of their differing middle (null: same). */
export function diff(old: string, text: string): Edit | null {
  if (old === text) return null;
  let a = 0;
  while (a < old.length && a < text.length && old[a] === text[a]) a++;
  let z = 0;
  while (z < old.length - a && z < text.length - a && old[old.length - 1 - z] === text[text.length - 1 - z]) z++;
  // (never split a surrogate pair)
  if (a > 0 && /[\udc00-\udfff]/.test(old[a] ?? "")) a--;
  if (z > 0 && /[\udc00-\udfff]/.test(old[old.length - z] ?? "")) z--;
  return { from: a, to: old.length - z, text: text.slice(a, text.length - z) };
}

export class PreviewSession {
  private o: Options;
  /** Each file's text as sent to the core (queued edits: in `batches`). */
  private files: Record<string, string> = {};
  private batches = new Map<string, Batch>();
  main: string | null = null;
  open: string | null = null;
  page = 0;
  pages = 0;
  private opened = false;
  debug = false;
  /** When each file's queued edits began (the oldest keystroke not sent). */
  private since = new Map<string, number>();
  private scheduled = false;
  /** Sends are chained: the core gets the edits in the order they were made. */
  private chain: Promise<void> = Promise.resolve();
  private host: EditorHost;
  private core: CoreTransport;
  private sink: PreviewSink;

  /**
   * What happened, in order (dev: scripts/mock-run.mjs reads it): each core
   * request and its reply, package fetches, layouts. A ring of the last 3000,
   * times in ms since the session began; never document text, only sizes.
   */
  readonly trace: { t: number; k: string; d?: unknown }[] = [];
  /** The core fetches what it lacks from Shelf itself (its open reply says so). */
  private coreFetches = false;
  /** Shelf packs the core fetched in the build under way (shown as loading until it ends). */
  private inBuild: string[] = [];
  private tellPackages(): void {
    this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...(this.pkg.done ?? [])], failed: [...(this.pkg.failed ?? [])] });
  }
  /** The core fetches a pack mid-build (the job goes on with it): the view says which. */
  private onFetching(e: Extract<CoreEvent, { event: "fetching" }>): void {
    // (a pack the worker could not get: Shelf unreachable, or not there)
    if (e.failed) {
      this.tr("package: core fetch failed", e);
      this.pkg.loading = this.pkg.loading.filter((n) => n !== e.pack);
      this.pkg.failed = [...(this.pkg.failed ?? []), { name: e.pack, error: `Shelf: ${e.name} not fetched` }];
      this.tellPackages();
      return;
    }
    this.tr("package: core fetches", e);
    this.pkg.source = this.source.label;
    // (one at a time, in order: the one before it has arrived)
    const prev = this.inBuild.at(-1);
    if (prev) this.pkg.loading = this.pkg.loading.filter((n) => n !== prev);
    if (prev) this.pkg.done = [...(this.pkg.done ?? []), prev];
    this.inBuild.push(e.pack);
    this.pkg.loading = [...this.pkg.loading, e.pack];
    this.tellPackages();
  }

  /** Core requests sent and not answered yet; of them, ones that may build. */
  inflight = 0;
  private building = 0;
  private t0 = 0;
  tr(k: string, d?: unknown): void {
    this.t0 ||= this.now();
    this.trace.push({ t: Math.round(this.now() - this.t0), k, d });
    if (this.trace.length > 3000) this.trace.shift();
  }

  constructor(host: EditorHost, core: CoreTransport, sink: PreviewSink, opts: Partial<Options> = {}) {
    this.host = host;
    // (a trap in the core, a panic, restarts its instance: every session it
    // held is gone, "no such handle". Reopened here, a few times at most,
    // since a panic a build always hits would loop)
    core.onEvent?.((e) => {
      if (this.o.remote && "event" in e && WATCH.has(e.event)) return this.onWatch(e as WatchEvent);
      if (e.event === "preparing") return this.sink.preparing?.(e.on);
      // (references settled after one-trip keystrokes: the pages that changed drawn again)
      // (the two-worker start: the SSA worker took over; its glyphs' sources are its own)
      if (e.event === "switched") {
        this.tr("core: the SSA worker took over", {});
        this.glyphCache.clear();
        if (this.opened) this.chain = this.chain.then(() => this.layout()).then(() => this.statusSoon());
        return;
      }
      if (e.event === "settled") {
        if (this.opened && !this.busy) this.chain = this.chain.then(() => this.layout()).then(() => this.statusSoon());
        return;
      }
      if (e.event === "fetching") this.onFetching(e);
      if (e.event === "page" || e.event === "progress" || e.event === "done") this.onStream(e as StreamEvent);
    });
    this.core = {
      request: async (r) => {
        const t0 = this.now();
        this.tr(`→ ${r.op}`, traceReq(r));
        this.inflight++;
        // (the ops that may build: the view shows a build that takes a while)
        const builds = r.op === "edit" || r.op === "status" || r.op === "open";
        if (builds && this.building++ === 0) this.sink.busy?.(true);
        // (the glyphs' sources: an edit maps them, as an editor maps positions;
        // a file set whole or a new session asks them again)
        if (r.op === "open" || r.op === "set_file") this.glyphCache.clear();
        if (r.op === "edit") this.mapGlyphs(r.file, r.start, r.end, new TextEncoder().encode(r.text).length);
        const res = await core.request(r).finally(() => {
          this.inflight--;
          if (builds && --this.building === 0) this.sink.busy?.(false);
          // (the build that fetched them is done: the packs are in)
          if (builds && this.inBuild.length) {
            this.pkg.done = [...(this.pkg.done ?? []), ...this.inBuild.filter((n) => this.pkg.loading.includes(n))];
            this.pkg.loading = this.pkg.loading.filter((n) => !this.inBuild.includes(n));
            this.inBuild = [];
            this.tellPackages();
          }
        });
        this.tr(`← ${r.op}`, { ms: Math.round(this.now() - t0), ...traceRes(res) });
        if (res.error) this.errorsSeen = [...this.errorsSeen.slice(-19), `${r.op}: ${res.error}`];
        const lost = res.json?.error === "no such handle" || /^(core trapped|PDF driver failed)/.test(res.error ?? "");
        // (a build that came back whole: a trap after it is a new one)
        if (res.ok && (r.op === "edit" || r.op === "status")) this.traps = 0;
        if (lost && this.opened && r.op !== "open") {
          this.opened = false;
          // (a trap again with no good build between is the same trap, the
          // same input to the same engine: reopened once, then stopped, however
          // long each takes; counted by time, a slow one would loop forever)
          this.traps++;
          const why = res.error ?? "the core restarted";
          if (this.traps <= 1) {
            this.sink.error(`${why}; reopening`);
            this.chain = this.chain.then(() => this.reopen());
          } else this.sink.error(`${why}, again: the preview stops here (reload the page to retry)`);
        }
        return res;
      },
      onLost: core.onLost?.bind(core),
    };
    this.sink = sink;
    this.o = { ...defaults, ...opts };
    host.onOpen((f0, t) => {
      const f = this.known(f0);
      this.open = f;
      this.sync(f, t);
      this.status();
    });
    host.onChanges((f, es) => this.queue(this.known(f), es));
    core.onLost?.(() => {
      this.opened = false;
      this.sink.error("core lost; reopening");
      this.reopen();
    });
  }

  /**
   * The project's file the editor's name is: itself, else the one file
   * whose path ends with it (a name read from Overleaf's page without its
   * folders: edits to a new, empty file the build never reads, the page
   * never changing while the chip said they were built).
   */
  private known(f: string): string {
    if (f in this.files) return f;
    const like = Object.keys(this.files).filter((p) => p.endsWith("/" + f));
    if (like.length === 1) {
      this.tr("file: named by its path", { from: f, to: like[0] });
      return like[0];
    }
    return f;
  }

  /** The pages' request: 0 their draw lists (vector), -1 the PDF, drawn in the tab by pdf.js. */
  private dpi(): number {
    return this.o.format === "pdfjs" ? -1 : 0;
  }

  setFormat(f: Options["format"]): Promise<void> {
    this.o.format = f;
    // (every page again, in the new format)
    if (this.sink.layout) {
      this.hashes = [];
      return this.layout();
    }
    return this.showPage();
  }

  private now(): number {
    return (this.o.now ?? (() => performance.now()))();
  }

  async start(): Promise<void> {
    if (this.o.remote) {
      this.opened = true;
      return this.watchJoined();
    }
    const files = await this.host.loadProject();
    // (a file the editor already announced keeps the editor's text)
    for (const [f, t] of Object.entries(files)) if (!(f in this.files)) this.files[f] = t;
    this.main = this.host.mainFile?.(this.files) ?? guessMain(this.files);
    this.sink.mains?.(Object.keys(this.files).filter((f) => f.endsWith(".tex")), this.main);
    await this.reopen();
    this.host.ready?.();
    if (this.o.checkEveryMs > 0) this.checkTimer = setInterval(() => this.debug && this.check(), this.o.checkEveryMs);
  }

  private checkTimer: ReturnType<typeof setInterval> | undefined;

  /** The session ends (its view closed): its timers stop and it sends nothing more; the core's side is its transport's to close. */
  stop(): void {
    clearInterval(this.checkTimer);
    if (this.statusTimer) clearTimeout(this.statusTimer);
    this.opened = false;
    this.paused = true;
  }

  /** Files changed outside the editor (e.g. a project reloaded): each diffed in. */
  refresh(files: Record<string, string>): void {
    for (const [f, t] of Object.entries(files)) if (f !== this.open) this.sync(f, t);
  }

  setMain(m: string): Promise<void> {
    this.main = m;
    return this.reopen();
  }

  /**
   * Clean recompile: the project's files as Overleaf has them now, packages
   * that failed or were not found asked for again, and a fresh core session
   * (discovery, first paint, then the incremental program), nothing kept from
   * the last job but the packages already fetched.
   */
  async clean(files: Record<string, string>): Promise<void> {
    for (const [f, t] of Object.entries(files)) if (f !== this.open) this.files[f] = t;
    for (const n of [...this.pkg.unavailable, ...(this.pkg.failed ?? []).map((f) => f.name)]) this.asked.delete(n);
    this.pkg.unavailable = [];
    this.pkg.failed = [];
    this.texError = undefined;
    this.hashes = [];
    return this.reopen();
  }

  /** Whether the open's sources used minted (the worker then has its runner). */
  private minted = false;

  /** A fresh core session from the texts with every queued edit in. */
  async reopen(): Promise<void> {
    if (!this.main) return this.sink.error("no main .tex file found");
    for (const [f, b] of this.batches) this.files[f] = b.text;
    this.batches.clear();
    const t = this.now();
    // (what the project's files load, by a scan, fetched before the first
    // build: a cold build costs the format's load, ~0.7 s in wasm, and stops
    // at the first file it lacks)
    // (pdftex.map: every PDF-mode build reads it first)
    const want = this.o.engine?.(this.files[this.main]) ?? "pdflatex";
    // (XeLaTeX approximated: pdfLaTeX with stand-ins for fontspec & co.)
    const shims = !ENGINES[want].ready && want === "xelatex" ? await this.o.shims?.(this.files[this.main]) : null;
    const engine: Engine = shims ? "pdflatex" : want;
    this.engine = engine;
    this.wanted = want;
    this.preamble = preambleOf(this.files[this.main]);
    this.sink.engine?.({ engine: want, ready: ENGINES[engine].ready, approx: !!shims });
    // (an engine ⚡ Instant doesn't run yet: nothing is built; the view says so and offers another)
    if (!ENGINES[engine].ready) {
      this.opened = false;
      this.tr("engine", { engine, ready: false });
      return;
    }
    await this.prefetch(["pdftex.map", ...Object.values(this.files).flatMap(referenced)]);
    // (an open with minted in the sources loads the worker's \write18 runner, Pyodide)
    this.minted = Object.values(this.files).some(usesMinted);
    const r = await this.core.request({ op: "open", main: this.main, files: { ...this.pkgFiles, ...shims, ...this.files }, fuel: this.o.fuel, engine, workers: this.o.workers });
    if (!r.ok) return this.sink.error((typeof r.json?.error === "string" ? r.json.error : undefined) ?? r.error ?? "open failed");
    // (streamed: the pages come as they are shipped, onStream; then its end,
    // with what a blocking open would have answered)
    if (r.json.building) {
      this.tr("open: streaming", {});
      this.opened = true;
      this.sink.busy?.(true);
      this.sink.streaming?.({ pages: 0, phase: r.json.phase ?? "typesetting" });
      const d = await new Promise<Extract<StreamEvent, { event: "done" }>>((res) => (this.streamEnd = res));
      this.sink.busy?.(false);
      this.sink.streaming?.(null);
      if (!d.ok || !d.json) return this.sink.error(d.error ?? "the build stopped");
      r.json = d.json;
    }
    this.opened = true;
    const latex = r.json.engine === "partex";
    // (diagnosed before this was known: say it again with it)
    if (latex !== this.latex) this.lastWarned = 0;
    this.latex = latex;
    this.coreFetches = !!r.json.fetches;
    // (a plain first paint: the SSA program is still to come, said until it is)
    this.warming = typeof r.json.how === "string" && r.json.how.startsWith("plain") ? this.now() : 0;
    this.warmSlow = false;
    if (this.warming) this.sink.warmup?.({ state: "preparing", since: Date.now() });
    this.setPages(r.json.pages);
    this.noteError(r.json);
    this.status(r.json.pending, r.json.undefined_names);
    this.fetchPackages(r.json.missing);
    this.sink.latency(`opened in ${r.json.build_ms.toFixed(1)} ms (round trip ${(this.now() - t).toFixed(1)} ms)`);
    await (this.sink.layout ? this.layout(Array.isArray(r.json.hashes) ? r.json.hashes : undefined) : this.showPage());
    // (the first build is in: nothing is building for the packages now)
    this.pkg.building = false;
    this.tellPackages();
    this.statusSoon();
  }

  /** A streamed open's end, awaited by `open`. */
  private streamEnd: ((d: Extract<StreamEvent, { event: "done" }>) => void) | undefined;
  /** The streamed open's first page was traced. */
  private streamFirst = false;

  private onStream(e: StreamEvent): void {
    if (e.event === "done") {
      this.tr("open: streamed", { ok: e.ok });
      this.streamFirst = false;
      this.streamEnd?.(e);
      this.streamEnd = undefined;
      return;
    }
    if (!this.streamEnd) return;
    if (e.event === "progress") return this.sink.streaming?.({ pages: e.pages, phase: e.phase });
    // (a page shipped: laid out and drawn, as after an edit)
    if (!this.streamFirst) {
      this.streamFirst = true;
      this.tr("open: first page", { pages: e.pages });
    }
    this.setPages(e.pages);
    this.sink.streaming?.({ pages: e.pages, phase: "typesetting" });
    if (this.sink.layout) this.chain = this.chain.then(() => this.layout(e.hashes));
  }

  /** When the plain first paint came, while the SSA program is still to come (0: not warming). */
  private warming = 0;
  private warmSlow = false;
  /** A status said how the last build was: past a plain one, the SSA program is there. */
  private warmCheck(how: unknown): void {
    if (!this.warming || typeof how !== "string" || how.startsWith("plain")) return;
    this.tr("warmup: ready", { ms: Math.round(this.now() - this.warming) });
    this.warming = 0;
    this.sink.warmup?.({ state: "ready" });
  }

  private lastWarned = 0;
  /** When the core trapped, in the last minute. */
  /** Traps since the last build that came back whole. */
  private traps = 0;
  /** The engine the project runs with (its packages are resolved for it: which tree's file). */
  private engine: Engine = "pdflatex";
  /** The engine the main file asked for at the last open (before the shims' choice), and its preamble then. */
  private wanted?: Engine;
  private preamble?: string;
  /** The core runs LaTeX (the open reply's `engine`; until it says, as partex's, the default build). */
  private latex = true;
  private diags: Diagnostic[] = [];
  /** The last build's first TeX error (the core's status), if it had one. */
  private texError?: TexError;
  private noteError(j: { history?: number; error?: unknown }): void {
    if (j.history === undefined) return;
    this.texError = j.error && typeof j.error === "object" ? (j.error as TexError) : undefined;
  }
  private undefinedNames: string[] = [];

  /** The last status scan's pending count (edits keep it until the next scan). */
  private pending?: number;

  private status(pending?: number, undefinedNames?: string[]): void {
    if (this.o.remote) return this.sink.status({ pages: this.pages, main: this.main, file: null, diagnostics: this.watchDiags });
    this.pending = pending;
    // (the source scan is O(project): at most once a second)
    if (undefinedNames) this.undefinedNames = undefinedNames;
    if (this.now() - this.lastWarned > 250 || undefinedNames) {
      const texts: Record<string, string> = {};
      for (const f of Object.keys(this.files)) texts[f] = this.text(f)!;
      this.diags = diagnose(texts, this.main, {
        pages: this.pages,
        pending,
        undefinedNames: this.undefinedNames,
        unavailable: this.pkg.unavailable,
        packageSource: this.source === noPackages ? undefined : this.source.label,
        latex: this.latex,
        texError: this.texError,
        packageErrors: this.pkg.failed,
      });
      this.lastWarned = this.now();
    }
    this.sink.status({ pages: this.pages, pending, main: this.main, file: this.open, diagnostics: this.diags });
  }

  /** Packages the core was given (kept apart from the project's files: never diagnosed, never a main). */
  private pkgFiles: Record<string, string> = {};
  /** Every package name asked for once: not asked again. */
  private asked = new Set<string>();
  private pkg: PackageState = { loading: [], unavailable: [], source: "none", done: [], building: false };
  private get source(): PackageSource {
    return this.o.packages ?? noPackages;
  }

  /**
   * The core read `missing` and found nothing: each package among them
   * fetched (once), then set as a file (a rebuild each: the core never
   * looks for a missing file again). What they read in turn shows up in
   * the next status, and is fetched then.
   */
  private fetchPackages(missing?: string[], guessed: string[] = []): void {
    // (a core that fetches from Shelf itself, mid-build, looked everywhere:
    // what it still lacks is nowhere; said, not fetched again)
    if (this.coreFetches) {
      const none = (missing ?? []).filter((n) => isPackageFile(n) && !this.pkg.unavailable.includes(n));
      if (none.length) (this.pkg.unavailable.push(...none), this.tellPackages());
      return;
    }
    const wanted = new Set(missing ?? []);
    missing = [...wanted, ...guessed];
    const want = [...new Set(missing ?? [])].filter((n) => isPackageFile(n) && !(n in this.files) && !this.asked.has(n));
    if (!want.length) return;
    for (const n of want) this.asked.add(n);
    const src = this.source;
    this.pkg.source = src.label;
    const tell = () =>
      this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...(this.pkg.done ?? [])], failed: [...(this.pkg.failed ?? [])] });
    if (src === noPackages) {
      this.pkg.unavailable.push(...want);
      tell();
      return this.status(this.pending, this.undefinedNames);
    }
    this.pkg.loading.push(...want);
    tell();
    // (each package fetched is scanned for the ones it loads, and those are
    // fetched too, before the next build: the build stops at the first file
    // it lacks, so learning them one build at a time costs a build each)
    const got: (readonly [string, string | null])[] = [];
    this.tr("packages: want", want);
    const fetchOne = async (n: string): Promise<void> => {
      const t = await src.resolve(n, this.engine).catch((e) => this.failed(n, e));
      this.tr(t === null ? "package: none" : "package: got", n);
      got.push([n, t]);
      this.pkg.loading = this.pkg.loading.filter((m) => m !== n);
      if (t !== null) {
        this.pkg.done!.push(n);
        this.pkg.failed = this.pkg.failed?.filter((f) => f.name !== n);
      }
      const more = t === null || t === DELIVERED ? [] : referenced(t).filter((m) => isPackageFile(m) && !(m in this.files) && !this.asked.has(m));
      for (const m of more) this.asked.add(m);
      this.pkg.loading.push(...more);
      tell();
      await Promise.all(more.map(fetchOne));
    };
    void Promise.all(want.map(fetchOne)).then(() => {
      for (const [n, t] of got) {
        if (t === null) {
          // (a name the scan guessed and the build never asked for is not "unavailable")
          if (wanted.has(n) && !this.pkg.failed?.some((f) => f.name === n)) this.pkg.unavailable.push(n);
          continue;
        }
        // (a binary file: the source gave it to the core, and gives it again on a reopen)
        if (t === DELIVERED) continue;
        this.pkgFiles[n] = t;
        if (this.opened) this.chain = this.chain.then(() => this.core.request({ op: "set_file", file: n, text: t }).then(() => undefined));
      }
      const any = got.some(([, t]) => t !== null);
      this.pkg.building = any;
      tell();
      this.status(this.pending, this.undefinedNames);
      if (any)
        this.chain = this.chain
          .then(() => this.layout())
          .then(() => {
            this.pkg.building = false;
            tell();
            this.statusSoon();
          });
    });
  }

  /** After the core's discovery pass: what it asked for is here (or nowhere), so the first paint may build. Once per open. */
  /** A fetch that failed: kept with why (shown), and askable again. */
  private failed(n: string, e: unknown): null {
    this.asked.delete(n);
    const error = e instanceof Error ? e.message : String(e);
    this.pkg.failed = [...(this.pkg.failed ?? []).filter((f) => f.name !== n), { name: n, error }];
    return null;
  }

  /** Fetch `names` (and what they load, by the scan) into pkgFiles, before a session opens. */
  private async prefetch(names: string[]): Promise<void> {
    const src = this.source;
    const want = [...new Set(names)].filter((n) => isPackageFile(n) && !(n in this.files) && !this.asked.has(n));
    if (src === noPackages || !want.length) return;
    this.pkg.source = src.label;
    const tell = () =>
      this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...(this.pkg.done ?? [])], failed: [...(this.pkg.failed ?? [])] });
    for (const n of want) this.asked.add(n);
    this.pkg.loading.push(...want);
    tell();
    const one = async (n: string): Promise<void> => {
      const t = await src.resolve(n, this.engine).catch((e) => this.failed(n, e));
      this.pkg.loading = this.pkg.loading.filter((m) => m !== n);
      if (t !== null) this.pkg.done!.push(n);
      if (t !== null && t !== DELIVERED) this.pkgFiles[n] = t;
      const more = t === null || t === DELIVERED ? [] : referenced(t).filter((m) => isPackageFile(m) && !(m in this.files) && !this.asked.has(m));
      for (const m of more) this.asked.add(m);
      this.pkg.loading.push(...more);
      tell();
      await Promise.all(more.map(one));
    };
    const t0 = this.now();
    this.tr("prefetch: start", want.length);
    await Promise.all(want.map(one));
    this.tr("prefetch: done", { files: this.pkg.done?.length ?? 0, ms: Math.round(this.now() - t0) });
    this.pkg.building = true;
    tell();
  }

  /** The page count of the last build; the page in view kept inside it
   * (a build with no page, an error, keeps the page: it comes back). */
  private setPages(n: number): void {
    this.pages = n;
    if (n > 0 && this.page >= n) this.page = n - 1;
  }

  /** The page in view (the one an edit paints first). */
  setPage(p: number): Promise<void> {
    this.page = Math.max(0, Math.min(p, Math.max(this.pages - 1, 0)));
    return this.sink.layout ? Promise.resolve() : this.showPage();
  }

  /** The pages' hashes, as the core has them now. */
  private hashes: string[] = [];

  /** Tell the view which pages there are (by hash). */
  /** `given`: the hashes a reply carried (the open's), else asked for. */
  private async layout(given?: string[]): Promise<void> {
    if (!this.sink.layout) return;
    if (given) this.hashes = given;
    else {
      const r = await this.core.request({ op: "pages" });
      if (!r.ok || !r.json?.pages) return;
      this.hashes = r.json.pages;
    }
    this.tr("layout", { pages: this.hashes.length });
    this.setPages(this.hashes.length);
    // (no page shipped: the view keeps what it shows, dimmed; see the sink)
    if (this.hashes.length) this.sink.layout(this.hashes);
    // (the glyphs' sources of the pages around the one read, asked now, after
    // the build, so a double-click or a selection finds them ready)
    if (this.sink.goto && !this.noOriginsPrefetch) this.glyphsAll();
  }

  /** Everything again, for a view that just joined (a detached PDF tab). */
  async resync(): Promise<void> {
    if (!this.opened) return;
    if (this.o.remote) return this.watchJoined();
    this.sink.mains?.(Object.keys(this.files).filter((f) => f.endsWith(".tex")), this.main);
    this.status(this.pending);
    await (this.sink.layout ? this.layout() : this.showPage());
  }

  /** Page `k`, for a view that wants it. */
  async fetch(k: number): Promise<void> {
    if (!this.opened) return;
    const r = await this.core.request({ op: "png", page: k, dpi: this.dpi() });
    const img = image(r);
    // (the hash of the page drawn: the draw worker's PDF may be a build behind or ahead)
    if (img) this.sink.page(img, k, this.pages, r.json?.hash ?? this.hashes[k] ?? null);
  }

  async showPage(): Promise<void> {
    if (!this.opened) return;
    if (this.sink.layout) return this.layout();
    const r = await this.core.request({ op: "png", page: this.page, dpi: this.dpi() });
    this.sink.page(image(r), this.page, this.pages);
  }

  /** (debugging: no glyph-origin requests after layouts) */
  noOriginsPrefetch = false;
  /** The errors replies carried (traps, panics), the last 20. */
  private errorsSeen: string[] = [];

  /** An anonymized debug report (report.ts) for the user to send. */
  report(env: { version: string; engine: string; userAgent: string }): string {
    const st = this.trace.filter((e) => e.k === "← status").at(-1)?.d as Record<string, unknown> | undefined;
    return report({
      ...env,
      paths: Object.keys(this.files),
      trace: this.trace,
      diagnostics: this.diags ?? [],
      status: st,
      packages: this.pkg,
      // (TeX's error by its message only: its context is the document's lines)
      errors: [...this.errorsSeen, ...(this.texError ? [`TeX: ${this.texError.message}`] : [])],
    });
  }

  /** Page `k`'s glyphs with their sources, as the core has them now (kept by the page's hash). */
  private glyphCache = new Map<string, Glyph[]>();
  /**
   * An edit replaced bytes [start, end) of `file` with `len` bytes: every
   * cached glyph's source range moved with it. A page the edit changed has a
   * new hash, so its glyphs are asked again (the cache is by page hash).
   */
  private mapGlyphs(file: string, start: number, end: number, len: number): void {
    const map = (p: number) => (p < start ? p : p >= end ? p + len - (end - start) : start + len);
    for (const [key, gs] of this.glyphCache) {
      // (a page with a glyph whose source the edit replaced: its glyphs
      // asked again, not mapped: all of them would land at the edit's end;
      // a whole document replaced did that to every page)
      if (end > start && gs.some((g) => g.file === file && g.start < end && g.end > start)) {
        this.glyphCache.delete(key);
        continue;
      }
      for (const g of gs) {
        if (g.file !== file) continue;
        const [a, b] = [map(g.start), map(g.end)];
        g.start = a;
        g.end = Math.max(a, b);
      }
    }
  }

  /** Page `k`'s glyphs if they are here, at once (no request, no waiting on a build). */
  private cached(k: number): Glyph[] | undefined {
    return this.glyphCache.get(`${k}:${this.hashes[k] ?? ""}`);
  }

  /** Every page's glyphs asked for, the one read first, one at a time: after each build, so lookups are local. */
  private glyphsAll(): void {
    const gen = ++this.glyphGen;
    const n = this.hashes.length;
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    void (async () => {
      for (const k of order) {
        if (gen !== this.glyphGen) return;
        if (k >= 0 && k < n && !this.cached(k)) await this.glyphsOf(k);
      }
    })();
  }
  private glyphGen = 0;

  private async glyphsOf(k: number): Promise<Glyph[]> {
    // (by the page's hash: an unchanged page keeps its glyphs, mapped through edits)
    const key = `${k}:${this.hashes[k] ?? ""}`;
    const have = this.glyphCache.get(key);
    if (have) return have;
    const r = await this.core.request({ op: "origins", page: k });
    if (!r.ok || !r.json?.g) return [];
    // (the job's names: "main" for main.tex, "ch/intro.tex", "article.cls")
    const path = (n: string) => {
      n = n.replace(/^\.\//, "");
      // (the host's names, as they are on its disk)
      if (this.o.remote) return n;
      return n in this.files ? n : n + ".tex" in this.files ? n + ".tex" : null;
    };
    const gs = glyphs(r.json, path);
    // (old hashes' entries: dropped past a few hundred pages' worth)
    if (this.glyphCache.size > 4 * Math.max(this.hashes.length, 100)) this.glyphCache.clear();
    this.glyphCache.set(key, gs);
    return gs;
  }

  /** A double-click at (x, y) on page `k` (PDF points from its top left): the editor to its source. */
  async toSource(k: number, x: number, y: number): Promise<void> {
    if (!this.opened) return;
    const g = nearest(this.cached(k) ?? (await this.glyphsOf(k)), x, y);
    this.tr("sync→source", { k, x, y, g });
    if (!g?.file) return;
    if (this.o.remote) return this.o.remote.source(g.file, g.start, g.end);
    const t = this.files[g.file];
    this.sink.goto?.(g.file, charOffset(t, g.start), charOffset(t, g.end));
  }

  /**
   * The editor selected [from, to) of `file` (UTF-16): the glyphs that came
   * from it, on every page, highlighted (an empty selection clears them).
   */
  async selectSource(file: string, from: number, to: number): Promise<void> {
    if (!this.opened || !(file in this.files)) return;
    if (from === to) return this.sink.marks?.([]);
    const t = this.files[file];
    const [a, b] = [byteOffset(t, from), byteOffset(t, to)];
    const out: { k: number; boxes: [number, number, number, number][] }[] = [];
    for (let k = 0; k < Math.max(this.hashes.length, this.pages); k++) {
      const all = this.cached(k) ?? (await this.glyphsOf(k));
      const hit = all.filter((g) => g.file === file && !g.synth && g.start < b && g.end > a);
      if (hit.length) out.push({ k, boxes: boxes(hit, all) });
    }
    this.tr("select→page", { file, from, to, pages: out.map((m) => m.k) });
    this.sink.marks?.(out);
  }

  /**
   * Text selected on the pages ([x, y top, w, h] boxes in PDF points, by
   * page): the editor selects the source it came from (the file most of it
   * came from, from its first glyph's start to its last's end).
   */
  async selectPage(sel: { k: number; rects: [number, number, number, number][] }[]): Promise<void> {
    if (!this.opened || !sel.length) return;
    const by = new Map<string, { lo: number; hi: number; n: number }>();
    for (const { k, rects } of sel) {
      for (const g of this.cached(k) ?? (await this.glyphsOf(k))) {
        if (!g.file || g.synth) continue;
        // (a glyph's body: just right of and above its origin)
        const [gx, gy] = [g.x + 1, g.y - 2];
        if (!rects.some(([x, y, w, h]) => gx >= x && gx <= x + w && gy >= y && gy <= y + h)) continue;
        const r = by.get(g.file) ?? { lo: Infinity, hi: -Infinity, n: 0 };
        by.set(g.file, { lo: Math.min(r.lo, g.start), hi: Math.max(r.hi, g.end), n: r.n + 1 });
      }
    }
    const best = [...by].sort((x, y) => y[1].n - x[1].n)[0];
    this.tr("select→source", { pages: sel.map((s) => s.k), file: best?.[0], glyphs: best?.[1].n ?? 0 });
    if (!best) return;
    const [file, { lo, hi }] = best;
    const t = this.files[file];
    if (t === undefined) return;
    this.sink.goto?.(file, charOffset(t, lo), charOffset(t, hi), false);
  }

  /**
   * The editor's cursor at `pos` (UTF-16) in `file`, as it moves: the word it
   * is in highlighted on the page, at once, from the glyphs already here
   * (none asked for: a page not yet here is skipped); scrolled to only when
   * the cursor changed line.
   */
  follow(file: string, pos: number): void {
    if (!this.opened || !(file in this.files) || !this.sink.mark) return;
    const t = this.files[file];
    const bytes = new TextEncoder().encode(t);
    const at = byteOffset(t, pos);
    const lo = t.lastIndexOf("\n", pos - 1) + 1, hiC = t.indexOf("\n", pos);
    const [a, b] = [byteOffset(t, lo), byteOffset(t, hiC < 0 ? t.length : hiC)];
    const line = `${file}:${lo}`;
    const scroll = line !== this.followLine;
    this.followLine = line;
    const w = wordBytes(bytes, at);
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    for (const k of order) {
      const all = this.cached(k);
      if (!all) continue;
      const onLine = from(all, file, a, b);
      if (!onLine.length) continue;
      const word = w ? from(onLine, file, w[0], w[1]) : [];
      const hit = word.length ? word : lineAt(onLine, at);
      this.followWant = "";
      this.sink.mark(k, boxes(hit, all), scroll);
      return;
    }
    // (its page not here yet, far down a long document: asked for now, the
    // pages not here, then shown if the cursor is still there)
    const want = `${file}:${pos}`;
    this.followWant = want;
    void (async () => {
      for (const k of order) {
        if (this.followWant !== want) return;
        if (k >= this.hashes.length || this.cached(k)) continue;
        const all = await this.glyphsOf(k);
        if (from(all, file, a, b).length) {
          if (this.followWant === want) {
            this.followLine = "";
            this.follow(file, pos);
          }
          return;
        }
      }
    })();
  }
  private followLine = "";
  private followWant = "";

  /** The place `pos` (UTF-16) in `file`: its line's glyphs highlighted on the page (the page in view first). */
  async toPage(file: string, pos: number): Promise<void> {
    if (!this.opened || !(file in this.files)) return;
    const t = this.files[file];
    const lo = t.lastIndexOf("\n", pos - 1) + 1, hiC = t.indexOf("\n", pos);
    const [a, b] = [byteOffset(t, lo), byteOffset(t, hiC < 0 ? t.length : hiC)];
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    for (const k of order) {
      const all = this.cached(k) ?? (await this.glyphsOf(k));
      const hit = lineAt(from(all, file, a, b), byteOffset(t, pos));
      if (hit.length) {
        this.tr("sync→page", { file, pos, k, glyphs: hit.length });
        this.sink.mark?.(k, boxes(hit, all));
        return;
      }
    }
    this.tr("sync→page", { file, pos, k: -1 });
  }

  async pdf(): Promise<Uint8Array | undefined> {
    this.flush();
    await this.chain;
    const t = this.now();
    const r = await this.core.request({ op: "pdf" });
    this.sink.latency(`PDF made in ${(this.now() - t).toFixed(1)} ms`);
    return r.pdf;
  }

  /** `file` is now `text`: only the differing middle is sent. */
  sync(file: string, text: string): void {
    const old = this.batches.get(file)?.text ?? this.files[file];
    if (old === undefined) {
      this.files[file] = text;
      if (this.opened) this.chain = this.chain.then(() => this.core.request({ op: "set_file", file, text }).then(() => undefined));
      return;
    }
    const e = diff(old, text);
    if (e) this.queue(file, [e]);
  }

  queue(file: string, edits: Edit[]): void {
    if (!edits.length) return;
    let b = this.batches.get(file);
    if (!b) this.batches.set(file, (b = new Batch(this.files[file] ?? "")));
    if (!(file in this.files)) this.files[file] = "";
    if (!b.size) this.since.set(file, this.now());
    for (const e of edits) b.push(e);
    this.keystrokes += edits.length;
    if (!this.scheduled) {
      this.scheduled = true;
      const f = () => this.flush();
      if (this.o.schedule) this.o.schedule(f);
      // (the next task, not the next frame: a window without frames, an
      // unfocused or covered one, waited for a 100 ms fallback; edits that
      // come while a build runs are merged by the send loop anyway)
      else setTimeout(f, 0);
    }
  }

  /** A send loop is running (it takes what is queued when each build ends). */
  private busy = false;
  /** Edits the host gave, and builds they took (the batching's effect). */
  keystrokes = 0;
  builds = 0;

  /**
   * Send the queued edits, in order; the last of a round painted first
   * (edit_view). Backpressure: at most one round in flight. What is typed
   * meanwhile merges into the batches, and goes as one edit when the core
   * is free: a fast core builds every keystroke, a slow one the last N
   * together, never a state already stale.
   */
  /** Paused (the extension turned off): edits keep merging in their batches, sent on resume. */
  private paused = false;

  pause(on: boolean): void {
    this.paused = on;
    if (!on) void this.flush();
  }

  flush(): Promise<void> {
    this.scheduled = false;
    if (!this.opened || this.busy || this.paused) return this.chain;
    this.busy = true;
    this.sink.busy?.(true);
    this.chain = this.chain.then(async () => {
      try {
        for (;;) {
          const round = [...this.batches].filter(([, b]) => b.size);
          if (!round.length) break;
          for (const [file, b] of round) if (!(await this.send(file, b))) return;
        }
        // (then the other pages: the view draws those it shows that changed)
        await this.layout();
        this.statusSoon();
      } finally {
        this.busy = false;
        this.sink.busy?.(false);
        // (an edit queued after the loop's last look, while the pages were
        // laid out: its flush found the loop busy; sent now, not at the
        // next keystroke)
        if ([...this.batches.values()].some((b) => b.size)) setTimeout(() => void this.flush(), 0);
      }
    });
    return this.chain;
  }

  private statusTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * The pending/undefined scan flattens the whole program (3 ms at 180 KB,
   * ten times an edit): once the edits have settled, at most every 300 ms.
   */
  /** `phitex watch`'s last build's problems (Options.remote), as diagnostics. */
  private watchDiags: Diagnostic[] = [];
  /** A build of the watch's under way (its progress told, not yet settled). */
  private watchBuilding = false;

  private watchBusy(on: boolean): void {
    if (on === this.watchBuilding) return;
    this.watchBuilding = on;
    this.sink.busy?.(on);
    if (!on) this.sink.streaming?.(null);
  }

  /** The watch joined (or joined again): its pages, outline and last problems. */
  private async watchJoined(): Promise<void> {
    await this.layout();
    this.status();
    void this.watchOutline();
    const r = await this.core.request({ op: "diagnostics" } as never);
    if (r.ok) this.onWatch({ event: "diagnostics", items: (r.json?.items ?? []) as Problem[] });
  }

  private async watchOutline(): Promise<void> {
    if (!this.sink.outline) return;
    const r = await this.core.request({ op: "outline" } as never);
    if (r.ok) this.sink.outline((r.json?.items ?? []) as Entry[]);
  }

  private onWatch(e: WatchEvent): void {
    switch (e.event) {
      case "page": {
        // (a page shipped while the build runs: shown before the build is in,
        // the PDF's page replacing it when it settles)
        this.watchBusy(true);
        const hs = [...this.hashes];
        while (hs.length < Math.max(e.k + 1, e.pages ?? 0)) hs.push("");
        hs[e.k] = e.hash;
        this.hashes = hs;
        this.setPages(hs.length);
        this.sink.layout?.(hs);
        return;
      }
      case "progress":
        this.watchBusy(true);
        this.sink.streaming?.({ pages: e.pages ?? 0, phase: e.phase ?? "typesetting" });
        return;
      case "superseded":
        this.watchBusy(true);
        return;
      case "diagnostics":
        this.watchDiags = e.items.map(asDiagnostic);
        this.status();
        return;
      case "settled":
        // (a pass shown while the job's own files settle: still building;
        // the watch's `preparing` ends here, it sends no `off`)
        if (!e.settling) (this.watchBusy(false), this.sink.preparing?.(false));
        this.chain = this.chain.then(() => this.layout()).then(() => (this.status(), this.watchOutline()));
        return;
      case "sync":
        void this.syncTo(e.file, e.lo, e.hi, e.at, e.page);
        return;
    }
  }

  /** Forward search: bytes [lo, hi) of `file` (a line), the glyphs from it nearest `at`'s highlighted, the page in view tried first. */
  private async syncTo(file: string, lo: number, hi: number, at: number, hint?: number): Promise<void> {
    const n = this.hashes.length;
    const first = hint ?? this.page;
    for (const k of [first, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== first)]) {
      const all = this.cached(k) ?? (await this.glyphsOf(k));
      const hit = lineAt(from(all, file, lo, hi), at);
      if (hit.length) return this.sink.mark?.(k, boxes(hit, all), true);
    }
  }

  private statusSoon(): void {
    if (this.statusTimer) return;
    this.statusTimer = setTimeout(async () => {
      this.statusTimer = null;
      if (this.busy || !this.opened) return this.statusSoon();
      const r = await this.core.request({ op: "status" });
      if (r.ok && r.json) {
        this.warmCheck(r.json.how);
        this.setPages(r.json.pages);
        this.noteError(r.json);
        this.status(r.json.pending, r.json.undefined_names);
        this.fetchPackages(r.json.missing);
      }
    }, 300);
  }

  private async send(file: string, b: Batch): Promise<boolean> {
    // (typed before the SSA program: a plain rebuild, slower; said so)
    if (this.warming && !this.warmSlow) {
      this.warmSlow = true;
      this.sink.warmup?.({ state: "preparing", since: Date.now() - (this.now() - this.warming), slow: true });
    }
    const t0 = this.since.get(file) ?? this.now();
    const tSend = this.now();
    const edits = b.take();
    this.files[file] = b.text;
    // (minted appearing in a project opened without it, by a paste or a
    // whole replace: opened again, so the worker loads its runner first;
    // an edit would have run minted's command with none, and kept that)
    if (!this.minted && usesMinted(b.text)) {
      this.tr("minted: open again, with its runner", {});
      await this.reopen();
      return false;
    }
    // (the main file's preamble now asks for another engine, \usepackage{fontspec}
    // added or a whole XeLaTeX paper pasted in, or no longer does: opened
    // again with it; the engine is looked at only when the preamble changed)
    if (file === this.main && this.o.engine) {
      const pre = preambleOf(b.text);
      if (pre !== this.preamble) {
        this.preamble = pre;
        const want = this.o.engine(b.text);
        if (want !== this.wanted) {
          this.tr("engine: open again", { from: this.wanted, to: want });
          await this.reopen();
          return false;
        }
      }
    }
    for (const [i, e] of edits.entries()) {
      const last = i === edits.length - 1;
      this.builds++;
      const r = await this.core.request({ op: "edit", file, ...e, page: last ? this.page : -1, dpi: this.dpi() });
      if (!r.ok) {
        this.sink.error((typeof r.json?.error === "string" ? r.json.error : undefined) ?? r.error ?? "edit failed");
        if (r.error?.includes("trapped")) await this.reopen();
        return false;
      }
      if (!last) continue;
      const tBack = this.now();
      const j = r.json;
      this.setPages(j.pages);
      this.noteError(j);
      this.status(this.pending);
      const img = image(r);
      if (img) this.sink.page(img, this.page, this.pages, j.painted_hash ?? null);
      else if (!this.sink.layout) await this.showPage();
      const e2e = this.now() - t0;
      this.sink.painted?.(e2e);
      this.sink.latency(
        `${e2e.toFixed(0)} ms keystroke→page · core ${j.paint_ms.toFixed(1)} ms · ${this.keystrokes} edits → ${this.builds} builds`,
        [
          `first paint      ${j.paint_ms.toFixed(2)} ms (in the core: edit_view, the page painted)`,
          `fixed point      ${j.total_ms.toFixed(2)} ms (every page, the .aux/.bbl loops)${j.wrong ? " · first paint was superseded" : ""}`,
          `keystroke→page   ${e2e.toFixed(1)} ms = queued ${(tSend - t0).toFixed(1)} + round trip ${(tBack - tSend).toFixed(1)} + paint ${(this.now() - tBack).toFixed(1)}`,
          `rebuilt          ${j.stats.rebuilt} of ${j.stats.rebuilt + j.stats.reused} chunks, ${j.stats.passes} pass${j.stats.passes === 1 ? "" : "es"}`,
          r.draws ? `page             vector, ${r.draws.t.length} words` : j.png_bytes ? `page             PNG ${(j.png_bytes / 1024).toFixed(0)} KB (${j.png_compress_ms.toFixed(1)} ms to compress)` : "",
          `batching         ${this.keystrokes} edits → ${this.builds} builds (one in flight; the rest merged)`,
        ]
          .filter(Boolean)
          .join("\n"),
      );
    }
    return true;
  }

  /** PhiTeX's invariant: the incremental state = a fresh build of the same text. */
  async check(): Promise<CoreRes["json"]> {
    if (!this.opened) return;
    await this.flush();
    const file = this.open ?? this.main!;
    const r = await this.core.request({ op: "check", file, expect: this.files[file] });
    this.sink.check?.(r.json);
    return r.json;
  }

  /** The texts the session has sent (tests). */
  text(file: string): string | undefined {
    return this.batches.get(file)?.text ?? this.files[file];
  }

  /** Every project file's text as it is now (queued edits in): a compare's new side. */
  texts(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const f of Object.keys(this.files)) out[f] = this.text(f)!;
    return out;
  }
}
