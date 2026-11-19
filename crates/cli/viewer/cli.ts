// The live viewer's page in the CLI (DESIGN 4.8): the one viewer every
// host shows (viewer/src: the panel, panel.ts, on the session, session.ts;
// the Overleaf extension and VS Code show the same), here on `phitex
// watch`, which builds from disk: the session in its remote mode
// (Options.remote) asks the watch for the pages over a WebSocket, as the
// extension's session asks its core (CoreReq ops: pages, status, origins;
// and the watch's own: diagnostics, source), and shows
// what the watch tells (page, progress, diagnostics, settled, superseded,
// sync; follow and select, an LSP editor's cursor and selection in bytes, shown by the session). The page's keys, the contents beside the pages and the problems
// are the panel's (PanelOptions.keys, .outline).
//
// The pages are drawn by the watch as strips (crates/output/render): the
// page keeps each band's bitmap by its key and composes the pages from
// them (`Strips`); the page's text (finding) and the outline are the
// watch's too. No PDF is loaded here.
//
// Bundled by scripts/viewer-bundle.sh into viewer.js.

import { Panel } from "../../../viewer/src/panel.ts";
import { html } from "../../../viewer/src/html.ts";
import { PreviewSession, type CoreEvent, type CoreRes, type CoreTransport, type EditorHost } from "../../../viewer/src/session.ts";
import { composeLine, search, type Hit, type TextLine } from "../../../viewer/src/text.ts";
import { Strips, type Frame, type StripSource } from "../../../viewer/src/strips.ts";
import type { SettingRow, SettingsGroup } from "../../../viewer/src/panel.ts";
import type { Commit } from "../../../viewer/src/commits.ts";
import type { ChangeItem, ChangesDiffLook, ChangesHost, ChangesRunner, Pair } from "../../../viewer/src/changes.ts";
import type { DiffChange } from "../../../viewer/src/diff.ts";
import { diffStyle } from "../../../viewer/src/compare.ts";
import type { ReviewHost, ReviewState } from "../../../viewer/src/review.ts";
import type { BlameData, BlameHost } from "../../../viewer/src/blame.ts";


/** The watch's core, over a WebSocket: requests answered in order, events pushed; joined again when it drops. */
class Socket implements CoreTransport {
  ws?: WebSocket;
  /** The wait before the next try (doubled to 3 s while the watch is away: a watch started again on this document serves this same address, and the tab joins it within that). */
  private wait = 1000;
  private next = 1;
  private waiting = new Map<number, (r: CoreRes) => void>();
  private events: ((e: CoreEvent) => void)[] = [];
  onOpen: () => void = () => {};
  onClose: () => void = () => {};
  /**
   * Called on every open, beside `onOpen` (which later code assigns over:
   * a listener chained into it was dropped by that, so the notes were asked
   * for on no open, and a reloaded tab showed none until the next build).
   */
  private opens: (() => void)[] = [];
  /** `f` on every open from now on, and now if it is open. */
  onOpened(f: () => void): void {
    this.opens.push(f);
    if (this.ws?.readyState === WebSocket.OPEN) f();
  }

  /**
   * Join the watch: first asked over HTTP whether it is there, so a tab
   * left open after its watch ended never opens WebSockets to a dead port
   * (Firefox opens one WebSocket at a time to an address, and delays
   * those after failures: such a tab would hold back every other's).
   */
  async connect(): Promise<void> {
    // (a tab in the background asks too, over HTTP first: the watch then sees it and opens no second tab)
    const there = await this.there();
    if (!there) return this.again();
    // (another phitex's watch at this address, its viewer not this page's: the
    // tab is told to reload, its pages kept as they are, and talks to it no more)
    if (there !== true && ownViewer() && there !== ownViewer()) return this.onStale();
    this.open();
  }

  /** A watch serving another viewer than this page's (`ping`'s `viewer` line): the tab must be reloaded. */
  onStale: () => void = () => {};

  /** Whether the watch answers HTTP here: false, else the viewer it serves (`ping`), true where it says none. */
  async there(): Promise<string | boolean> {
    try {
      const r = await fetch(new URL("ping", location.href), { cache: "no-store" });
      if (!r.ok) return false;
      const m = /^viewer (\S+)$/m.exec(await r.text());
      return m ? m[1] : true;
    } catch {
      return false;
    }
  }

  private again(): void {
    setTimeout(() => void this.connect(), this.wait);
    this.wait = Math.min(this.wait * 2, 3000);
  }

  private open(): void {
    const url = new URL("ws", location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    this.ws = new WebSocket(url);
    this.ws.onopen = () => {
      this.wait = 1000;
      this.onOpen();
      for (const f of this.opens) f();
    };
    this.ws.onmessage = (m) => {
      const r = JSON.parse(m.data as string);
      if (r.event) return this.events.forEach((f) => f(r));
      this.waiting.get(r.id)?.(r);
      this.waiting.delete(r.id);
    };
    this.ws.onclose = () => {
      for (const f of this.waiting.values()) f({ ok: false, error: "closed" } as CoreRes);
      this.waiting.clear();
      this.onClose();
      // (the watch restarted, or the network blinked: try again)
      this.again();
    };
  }

  request(op: object): Promise<CoreRes> {
    const id = this.next++;
    return new Promise((ok) => {
      this.waiting.set(id, ok);
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ ...op, id }));
      else (this.waiting.delete(id), ok({ ok: false, error: "not connected" } as CoreRes));
    });
  }

  onEvent(cb: (e: CoreEvent) => void): void {
    this.events.push(cb);
  }
}

/**
 * The watch's core, its pages drawn as strips (`Strips`): `png` answered
 * from the bands the watch draws, the page's text and links with them;
 * `outline` and every other request asked of the watch.
 */
/** The watch's `settled` event: the build's time and parts (a failed build: since_ms, ms and history only). */
type Settled = {
  since_ms?: number | null; ms?: number; history?: number; cold?: boolean; passes?: number; ran?: number; kept?: number; threads?: number; fetched?: number;
  /** The change's wait before its build, by what the watch did meanwhile (the last build, the viewer's asks, …; "reaching the watch": nothing). */
  waited?: [string, number][];
};

const dur = (ms: number) => (ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)} s`);

/** A sum's parts as the popover's bars read them ("= waiting 5.2 + build 0.6"): names of letters and spaces, then ms. */
const sum = (parts: [string, number][]) => parts.filter(([, ms]) => ms >= 0.05).map(([k, ms]) => `${k.replace(/[^a-z ]/gi, "")} ${ms.toFixed(1)}`).join(" + ");

/**
 * The timing popover's line and parts for a build (panel.latency: one part a
 * line, its name and value two spaces apart): from the save to the page, its
 * parts adding up to it (the wait before the build, by what the watch did
 * meanwhile; the build; the files and the viewer told; the drawing, `drawn`,
 * once the page is on the screen).
 */
function timing(s: Settled, drawn?: number): [string, string] {
  const ms = s.ms ?? 0;
  const waited = s.waited ?? [];
  const wait = waited.reduce((a, [, x]) => a + x, 0);
  const since = typeof s.since_ms === "number" ? s.since_ms : null;
  // (after the build: its files written, the viewer and the editor told)
  const after = since === null ? 0 : Math.max(0, since - wait - ms);
  const total = since === null ? null : since + (drawn ?? 0);
  const parts: [string, string | null][] = [
    [
      drawn === undefined ? "save to built" : "keystroke→page",
      total === null ? null : `${total.toFixed(1)} ms = ${sum([["queued", wait], ["build", ms], ["files and viewer", after], ...(drawn === undefined ? [] : [["paint", drawn] as [string, number]])])}`,
    ],
    ["before the build", wait >= 1 ? `${wait.toFixed(1)} ms = ${sum(waited)}` : null],
    [s.cold ? "first build" : "build", `${dur(ms)}${s.cold ? " (the format, the packages, the whole document)" : ""}`],
    ["LaTeX runs", s.passes === undefined ? null : String(s.passes)],
    ["steps", s.ran === undefined ? null : `${s.ran} run, ${s.kept ?? 0} kept from the last build`],
    ["packages fetched", s.fetched ? String(s.fetched) : null],
    ["threads", s.threads === undefined ? null : String(s.threads)],
  ];
  return [
    s.cold ? `First build in ${dur(ms)}` : `Built in ${dur(ms)}`,
    parts
      .filter((p): p is [string, string] => p[1] !== null)
      .map(([k, v]) => `${k.padEnd(17)}  ${v}`)
      .join("\n"),
  ];
}

/** The watch's `stats` event, after each build's `settled` (a failed build sends none): the CLI's `s` stats. */
type Stats = {
  build?: number; why?: string; version?: string; engine?: string; files?: "texlive" | "shelf"; files_release?: string | null;
  wall_ms?: number; phases_ms?: Record<string, number>; nodes?: { ran?: number; kept?: number; live?: number }; runs?: number; rounds?: number;
  pages?: number; pages_changed?: number[]; threads?: number; cores_busy?: number | null; busy_ms?: number; engine_ms?: number; wasted_ms?: number;
  resident_kb?: number; values_bytes?: number; hints?: Record<string, number>; ok?: boolean;
};

/** The last build's stats, for the timing popover's "Copy debug info". */
let lastStats: Stats | undefined;

/**
 * A build's stats in: kept for Copy debug info, and its parts for the timing
 * popover's "Where the last build's time went" (typesetting, the PDF, the
 * files: one after another, the build's time; the viewer's pages drawn
 * alongside them, said apart).
 */
function statsIn(s: Stats): void {
  lastStats = s;
  const ph = s.phases_ms ?? {};
  const names: [string, string][] = [["typesetting", "Typesetting"], ["output", "Writing the PDF"], ["writing files", "Writing files"]];
  const known = new Set([...names.map(([k]) => k), "viewer"]);
  const parts: [string, number][] = [...names.filter(([k]) => typeof ph[k] === "number").map(([k, n]): [string, number] => [n, ph[k]]), ...Object.entries(ph).filter(([k]) => !known.has(k))];
  panel.buildParts(parts, typeof ph.viewer === "number" ? [["pages for the viewer", ph.viewer]] : []);
}

/** `stats` as lines of plain text, the CLI's `s` stats' words. */
function statsLines(s: Stats): string[] {
  const n = s.nodes ?? {};
  const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);
  // (the pages changed: how many, and which when few)
  const pc = s.pages_changed;
  const changed = pc === undefined ? "?" : pc.length && pc.length <= 8 ? `${pc.length} (${pc.join(", ")})` : String(pc.length);
  return [
    `PhiTeX ${s.version ?? "?"} · ${s.engine ?? "?"} · files from ${s.files === "shelf" ? `Shelf ${s.files_release ?? "?"}` : s.files === "texlive" ? "TeX Live" : "?"}`,
    `build ${s.build ?? "?"} (${s.why ?? "?"}) ${s.ok === false ? "failed" : "ok"} in ${dur(s.wall_ms ?? 0)}: ${Object.entries(s.phases_ms ?? {}).map(([k, ms]) => `${k} ${dur(ms)}`).join(" · ")}`,
    `nodes ${n.ran ?? "?"} ran, ${n.kept ?? "?"} kept of ${n.live ?? "?"} · ${s.runs ?? "?"} runs, ${s.rounds ?? "?"} rounds · pages ${changed} of ${s.pages ?? "?"} changed`,
    `${s.cores_busy == null ? `threads ${s.threads ?? "?"}` : `parallel ${s.cores_busy.toFixed(1)} of ${s.threads ?? "?"} threads busy`} · busy ${dur(s.busy_ms ?? 0)}, engine ${dur(s.engine_ms ?? 0)}, wasted ${dur(s.wasted_ms ?? 0)}`,
    `memory ${s.resident_kb === undefined ? "?" : size(s.resident_kb * 1024)} resident, values ${s.values_bytes === undefined ? "?" : size(s.values_bytes)}`,
    `hints ${Object.entries(s.hints ?? {}).map(([k, v]) => `${k} ${v}`).join(", ") || "—"}`,
  ];
}

class Pages implements CoreTransport {
  private sock: Socket;
  /** A change on its way to the screen: ms from its save to `settled`, and when that came. */
  private change: { since: number; at: number } | null = null;
  /** The last build's `settled` event, for its parts once its page is drawn. */
  private settled: Settled | null = null;
  /** Every page's text, for finding (asked again after each build). */
  private texts: Promise<TextLine[][]> | null = null;

  constructor(sock: Socket) {
    this.sock = sock;
    sock.onEvent((e) => {
      if ((e.event as string) === "stats") return void statsIn(e as unknown as Stats);
      // (a diff's state in: its pages may be new, the `settled` after it; a
      // change gone to at once, as it comes, finds its words in them, not in
      // the text asked while it was typeset)
      if ((e.event as string) === "diff") return void (this.texts = null);
      if (e.event !== "settled") return;
      this.texts = null;
      const s = e as Settled;
      // (a cold build has no save before it: its first page is timed from the build's start, "First build · 14.9 s")
      const since = typeof s.since_ms === "number" ? s.since_ms : s.cold && typeof s.ms === "number" ? s.ms : null;
      this.change = since === null ? null : { since, at: performance.now() };
      this.settled = s;
      if (typeof s.ms === "number") panel.latency(...timing(s));
      // (the bars' times: the session's, from the same event)
    });
  }

  async request(op: object): Promise<CoreRes> {
    const o = op as { op: string; page?: number; scale?: number; view?: [number, number, number, number] };
    if (o.op === "png") {
      // (a link's window zoomed in: its own scale and part of the page, not the pages' zoom, and no repaint to time)
      const zoom = typeof o.scale === "number" && o.scale > 0;
      const p = await strips.page(o.page ?? 0, zoom ? o.scale! : panel.pixelsPerPoint(), zoom ? (o.view ?? null) : panel.pageView(o.page ?? 0)).catch(() => null);
      if (!p) return { ok: false, json: { error: "no such page" } };
      if (!zoom) this.painted();
      return { ok: true, canvas: p.el, size: [p.w, p.h], links: p.links, ...(p.hash !== undefined && p.hash !== null ? { json: { hash: p.hash } } : {}) };
    }
    return this.sock.request(op);
  }

  /**
   * The first page drawn after a change: on the screen at the next frame;
   * the ⚡ chip and the watch's status line say how long from the save.
   */
  private painted(): void {
    const c = this.change;
    if (!c) return;
    this.change = null;
    requestAnimationFrame(() => {
      const ms = c.since + performance.now() - c.at;
      // (the parts again, the drawing among them: they add up to the head's time)
      const s = this.settled;
      if (s && typeof s.ms === "number") panel.latency(...timing(s, ms - c.since));
      panel.painted(ms);
      void this.sock.request({ op: "painted", ms });
    });
  }

  /** Where `q` is in the build's pages (their text as the watch reads it). */
  async find(q: string): Promise<Hit[]> {
    this.texts ??= this.sock.request({ op: "text" }).then((r) =>
      r.ok ? ((r.json?.pages ?? []) as TextLine[][]).map((lines) => lines.map(composeLine)) : [],
    );
    return search(await this.texts, q);
  }

  onEvent(cb: (e: CoreEvent) => void): void {
    // (the editor's place, as the user's `follow` says: the cursor's
    // as it moves, its selection, or neither: a double-click's alone)
    this.sock.onEvent((e) => {
      if (e.event === "follow" && follow !== "cursor") return;
      if (e.event === "select" && follow === "dblclick") return;
      cb(e);
    });
  }
}

/**
 * The pages drawn by the watch as strips (crates/output/render,
 * docs/spec-display-list.md): a page is its bands, each a bitmap the
 * watch drew once and this page keeps by its key; a build's page draws
 * only the bands whose keys are new, and the rest are the bitmaps kept,
 * at their places.
 */
/** The watch's strips over HTTP (`frame/K/S`, `frame/K/S/L,T,R,B` for a part of the page, `strips/KEYS`). */
const stripSource: StripSource = {
  async frame(k, scale, win) {
    const part = win ? `/${win.map((x) => x.toFixed(5)).join(",")}` : "";
    // (the scale as asked, to a ten-millionth: rounded to a thousandth, a
    // page's bitmap came up to a pixel and more off its box, and was shown
    // stretched by that, every glyph resampled)
    const r = await fetch(new URL(`frame/${k}/${(scale * 1000).toFixed(4)}${part}`, location.href), { cache: "no-store" });
    return r.ok ? ((await r.json()) as Frame) : null;
  },
  async strips(keys) {
    const r = await fetch(new URL(`strips/${keys.join(",")}`, location.href), { cache: "no-store" });
    return r.ok ? r.arrayBuffer() : null;
  },
};

/** The last frame's numbers, in the corner above the status bar: only with `#debug` in the address. */
let chip: HTMLElement | undefined;
if (location.hash === "#debug") {
  chip = document.createElement("div");
  chip.style.cssText =
    "position:fixed;right:8px;bottom:32px;z-index:16;font:11px/1.3 ui-monospace,monospace;" +
    "background:rgba(20,24,32,.82);color:#e8eef8;padding:4px 7px;border-radius:5px;pointer-events:none";
  chip.textContent = "strips";
  document.body.append(chip);
}

const strips = new Strips(stripSource, chip ?? null);
const sock = new Socket();
sock.onStale = staleBar;
const pages = new Pages(sock);
/** A place in a file (1-based line and column): the watch opens it in the editor (`--editor`). */
/** The watch's cache (TeX Live's packs, the formats): its size asked first, emptied on a yes, as `phitex clean --all`. */
async function clearCache(): Promise<void> {
  const mb = (b: number) => `${(b / 1e6).toFixed(b < 1e7 ? 1 : 0)} MB`;
  const now = await sock.request({ op: "cache" });
  if (!now.ok) return void panel.msg(`Couldn't clear the cache: ${now.error ?? "not available"}`, true, true);
  const bytes = Number(now.json?.bytes ?? 0);
  if (!confirm(`Clear PhiTeX's cache (${mb(bytes)} in ${now.json?.dir ?? "its cache folder"})? Packages and formats are fetched again when a build needs them.`)) return;
  const r = await sock.request({ op: "cache", clear: true });
  panel.msg(r.ok ? `Cache cleared: ${mb(Number(r.json?.freed ?? 0))} freed` : `Couldn't clear the cache: ${r.error ?? "not cleared"}`, !r.ok, true);
}

const source = (file: string, line: number, col = 1) => void sock.request({ op: "source", file, line, col });

/**
 * Notes and changes (review.ts, docs/spec-editor-protocol.md §9): kept by
 * the watch (`.phitex/notes.json`, the baseline), its `review_*` ops and
 * its `review` event; the pages' places from the session's glyphs.
 */
const review: ReviewHost = {
  request: (op, params) => sock.request({ op, ...params }),
  onState(cb) {
    sock.onEvent((e: any) => e.event === "review" && cb(e as ReviewState));
    // (asked at every open: a tab opened or reloaded, or joined again after
    // the watch restarted)
    sock.onOpened(() => void sock.request({ op: "review_state" }).then((r) => r.ok && r.json && cb(r.json as ReviewState)));
  },
  boxes: (file, start, end) => session.boxesOf(file, start, end),
  extent: (file, start, end, page) => session.extentOf(file, start, end, page),
  source: (sel) => session.sourceOf(sel),
  spot: (k, x, y) => session.spotOf(k, x, y),
  goto: (file, line) => source(file, line),
  showChanges(on, git, proposal) {
    if (!on) return panel.changes?.off();
    if (proposal) return panel.changes?.show({ from: "now", to: `proposal:${proposal}` }, { from: "Your files as they are", to: `Proposal ${proposal}` });
    panel.changes?.show({ from: git ? "staged" : "baseline", to: null }, { from: git ? "What git has staged" : "The accepted text" });
  },
};

/**
 * Blame (blame.ts): the watch reads the project's git history (its `blame`
 * op, crates/cli/src/blame.rs), each file as the build reads it; the pages'
 * places from the session's glyphs; "Compare with before this commit" is
 * the watch's Compare against the commit's parent.
 */
const blame: BlameHost = {
  async blame(files, o) {
    const r = await sock.request({ op: "blame", files, ...(o ? { detail: o.detail, ws: o.ws, moved: o.moved, skip: o.skip } : {}) });
    return r.ok ? (r.json as unknown as BlameData) : (r.error ?? "no history here");
  },
  glyphs: (k) => session.pageGlyphs(k),
  // (a build done: a file blamed again only once its text on disk changed, every file once HEAD moved)
  text: async (file) => {
    const r = await sock.request({ op: "info", what: "source", file });
    return r.ok && r.json ? String(r.json.text ?? "") : undefined;
  },
  head: async () => {
    const r = await sock.request({ op: "info", what: "head" });
    return r.ok && typeof r.json?.head === "string" ? r.json.head : null;
  },
  goto: (file, line) => source(file, line),
};

// ---- Changes: the history (`diff_log`), the diff (the watch's diff ops, view.rs), the last visit ----

/** The watch's diff state (the `diff` op's answer, the `diff` event's). */
type DiffState = { shown?: boolean; base?: string; to?: string; label?: string; changes?: (DiffChange & { page?: number; y?: number })[]; built?: boolean; error?: string };

let diffState: DiffState = {};
let diffWait: ((d: DiffState) => void) | undefined;
let counted: ((c: DiffChange[]) => void) | undefined;
const ask = async (op: object) => {
  const r = await sock.request(op);
  if (!r.ok) throw new Error(r.error ?? "the watch refused");
};
/** A file the watch serves (the diff's PDF or .tex), saved by the browser. */
const save = (name: string) => {
  const a = document.createElement("a");
  a.href = new URL(name, location.href).href;
  a.download = name;
  a.click();
};
/** The look as the watch's diff ops take it. */
const lookFields = (l: ChangesDiffLook, hide: boolean) => diffStyle(l, hide);
/** The changes gone to (the last one's answer alone is shown), and the one to go to once the diff is built. */
let goes = 0;
let wanted: number | undefined;
/** A change's first words as the page shows them: its source without commands, braces and math marks. */
const words = (src: string) =>
  src.replace(/\\[a-zA-Z@]+\*?/g, " ").replace(/[{}$\\%&~^_#\[\]]/g, " ").split(/\s+/).filter(Boolean).slice(0, 5).join(" ");
/**
 * Change k shown: on the page the watch placed it (its first glyph's page
 * and height on the diff's pages), its words marked there, the match
 * nearest that height; a change with no place (none of its glyphs shown:
 * a deletion hidden) found by its words alone, the first match.
 */
const gotoChange = async (k: number) => {
  const go = ++goes;
  wanted = undefined;
  const c = (diffState.changes ?? [])[k - 1];
  if (!c) return;
  // (the diff still being typeset: its changes known, their places not yet; gone to when they are)
  if (typeof c.page !== "number" && diffState.built === false) {
    wanted = k;
    return void panel.msg(`Change ${k}: the diff is being typeset, its page shown once it is`, false, true);
  }
  const q = words(c.new_text || "") || words(c.old_text || "");
  const hits = q ? await pages.find(q) : [];
  if (go !== goes) return;
  if (typeof c.page === "number") {
    const y = c.y ?? 0;
    const near = hits
      .filter((h) => h.k === c.page)
      .sort((a, b) => Math.abs(a.boxes[0][1] + a.boxes[0][3] - y) - Math.abs(b.boxes[0][1] + b.boxes[0][3] - y))[0];
    return panel.reveal([near ?? { k: c.page, boxes: [[18, Math.max(0, y - 10), 14, 12]] }]);
  }
  if (!hits.length) return void panel.msg(`Change ${k} isn't on the pages`, false, true);
  panel.reveal(hits.slice(0, 1));
};

const runner: ChangesRunner = {
  async start(p, look, hide, say, count) {
    counted = count;
    say("Diffing…");
    const got = new Promise<DiffState>((ok) => (diffWait = ok));
    await ask({ op: "diff_start", rev: p.from, ...(p.to ? { to: p.to } : {}), ...lookFields(look, hide) });
    say("Typesetting the diff…");
    const d = await got;
    if (d.error) throw new Error(d.error);
    return d.changes ?? [];
  },
  show: (on) => void ask({ op: "diff_show", which: on ? "diff" : "current" }).catch(() => undefined),
  // (change k found in the diff's pages by its words, MuPDF's text: no SyncTeX here)
  goto: (k) => void gotoChange(k),
  download: (what) => save(`diff.${what}`),
  restyle: (l, hide) => void ask({ op: "diff_restyle", ...lookFields(l, hide) }).catch(() => undefined),
  stop: () => ((wanted = undefined), panel.diffMarks([]), void ask({ op: "diff_stop" }).catch(() => undefined)),
};

/**
 * Your last visit: the commit HEAD was at when this document was last
 * opened here (more than half an hour ago), kept in this browser by the
 * page's address.
 */
const VISIT = `phitex.visit.${location.pathname}`;
let visit: string | null = null;
const noteVisit = (head: string) => {
  try {
    const was = JSON.parse(localStorage.getItem(VISIT) ?? "null") as { cur: string; prev: string | null; at: number } | null;
    const now = Date.now();
    const next = !was ? { cur: head, prev: null, at: now } : now - was.at > 30 * 60e3 && was.cur !== head ? { cur: head, prev: was.cur, at: now } : { ...was, at: was.cur === head ? was.at : now, cur: head };
    visit = next.prev;
    localStorage.setItem(VISIT, JSON.stringify(next));
  } catch {
    /* (no storage: no last visit) */
  }
};

const changes: ChangesHost = {
  async log(skip, limit) {
    const r = await sock.request({ op: "diff_log", skip, limit });
    if (!r.ok) return r.error ?? "No git repository here";
    const cs = (r.json?.commits ?? []) as Commit[];
    if (!skip && cs[0]) noteVisit(cs[0].id);
    for (const c of cs) if (visit && c.id === visit) c.mark = "your last visit";
    return cs;
  },
  lastVisit: () => visit,
  diff: runner,
  /**
   * Who made each change, where git can tell: against the working tree,
   * the commit that last changed its line (the watch's blame); a line not
   * committed is the user's. Between two commits, none (each change's
   * commit is not worked out).
   */
  async attribute(items: ChangeItem[], p: Pair, update, signal) {
    if (p.to !== null) return;
    const files = [...new Set(items.filter((i) => i.neu).map((i) => i.file))];
    for (const f of files) {
      if (signal.aborted) return;
      const r = await sock.request({ op: "blame", files: [f] });
      if (!r.ok || signal.aborted) continue;
      const d = r.json as unknown as BlameData;
      const b = d.files[f];
      if (!b?.text || !b.lines) continue;
      const starts = [0];
      const enc = new TextEncoder().encode(b.text);
      enc.forEach((c, i) => c === 10 && starts.push(i + 1));
      for (const it of items) {
        if (it.file !== f || !it.neu || it.at === undefined) continue;
        let lo = 0;
        while (lo + 1 < starts.length && starts[lo + 1] <= it.at) lo++;
        const k = b.lines[lo];
        if (k === undefined) continue;
        if (k < 0) it.wip = true;
        else {
          const c = d.commits[k];
          it.who = { name: c.author || c.email, time: c.time, short: c.short, via: panel.changes?.via(c.id) ?? null };
        }
      }
      update();
    }
  },
};

const panel = new Panel(
  {
    onPage: () => {},
    // (settled once the page is drawn: the viewer asks for a page once at a time)
    onNeed: (k): Promise<void> => session.fetch(k),
    // (the build's PDF, as the watch wrote it)
    onPdf: () => {
      const a = document.createElement("a");
      a.href = new URL("doc.pdf", location.href).href;
      a.download = "";
      a.click();
    },
    onMain: () => {},
    onReload: () => void session.resync(),
    // (Clean rebuild: the watch drops its build and makes it again from nothing, its format kept)
    onClean: () => void sock.request({ op: "cold" }),
    // (the watch builds again from the files on disk, as `r` in its terminal)
    onRebuild: () => void sock.request({ op: "rebuild" }),
    onLog: (): Promise<string | null> => session.log(),
    onBoxes: (file, start, end): Promise<{ k: number; boxes: [number, number, number, number][] }[]> => session.boxesOf(file, start, end),
    onClearCache: () => void clearCache(),
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
    onContents: () => void session.outline(),
    onFind: (q) => pages.find(q),
    // (Settings: the watch's rows, from and into the user's phitex.toml)
    settings: () => watchSettings(),
    // (the panel's own choices: the user's phitex.toml too, never this browser's)
    // (Ctrl+click on a link: this viewer in a new tab, its address saying where: #at=PAGE,POINTS)
    onOpenAt: (k, top) => void window.open(`${location.pathname}${location.search}#at=${k + 1},${Math.round(top ?? 0)}`, "_blank"),
    onPref: async (key, value) => {
      const r = await sock.request({ op: "prefs", set: { [key]: value } });
      return r.ok ? undefined : `Not saved: ${r.error ?? "the watch did not answer"}`;
    },
    // (a project file's text, for Copy for an AI assistant)
    onSource: async (file) => {
      const r = await sock.request({ op: "info", what: "source", file });
      return r.ok ? String(r.json?.text ?? "") : null;
    },
    // (What's new: this version's CHANGELOG entry, from the binary)
    onNews: () =>
      void sock.request({ op: "info", what: "news" }).then((r) => {
        if (r.ok) panel.news(String(r.json?.version ?? ""), String(r.json?.text ?? ""));
        else panel.msg("No news for this version", false, true);
      }),
    // (the timing popover's "Copy debug info": the last build's stats, the CLI's `s` stats; before any, the version, the news op's)
    onDebugInfo: async () => {
      if (lastStats) return ["phitex watch", ...statsLines(lastStats)];
      const r = await sock.request({ op: "info", what: "news" });
      return [`PhiTeX ${r.ok ? String(r.json?.version ?? "?") : "?"} · phitex watch`];
    },
    // (the tips seen: the user's phitex.toml through the watch, the same on every port and browser)
    onTipsSeen: (ids) => void sock.request({ op: "prefs", set: { "tips-seen": ids } }),
    // (the problems' filters: kept the same way)
  },
  // (the panel's settings: this browser's)
  {
    load: async () => {
      try {
        return JSON.parse(localStorage.getItem("phitex.panel") ?? "{}");
      } catch {
        return {};
      }
    },
    save: (p) => {
      try {
        localStorage.setItem("phitex.panel", JSON.stringify(p));
      } catch {
        /* (no storage: not kept) */
      }
    },
  },
  {
    host: "watch",
    outline: true,
    review,
    blame,
    changes,
    keys: true,
    header: true,
    footer: false,
    words: {
      badge: "watch",
      badgeTitle: "phitex watch: built from the files on disk, again at each save",
      byline: "phitex watch",
      about: html`<b>phitex watch</b>: the document built from the files on disk, again at each save, its pages here as
        they are shipped. Nothing leaves this machine.
        <div class="about-foot">Free software (AGPL-3.0-only), provided as is, without any warranty.</div>`,
      openFailed: "Restart phitex watch, then reload this page.",
      reading: "Reading the document from phitex watch.",
      keptIn: "this machine",
      preparing: "",
      stopped: "The build stopped: its problems are listed above (e).",
      notReady: "This engine does not run here yet.",
      steps: ["Connecting to phitex watch", "Preparing the format", "Typesetting"],
      realPdf: "",
    },
  },
);
// (a tab opened at a place, #at=PAGE,POINTS: the pages start there)
{
  const at = /^#at=(\d+),(\d+)$/.exec(location.hash);
  if (at) panel.startAt(Number(at[1]) - 1, Number(at[2]));
}
/**
 * The user's preferences as the watch put them in the page (`#phitex-prefs`,
 * crates/cli/src/view.rs): known before the first build's opening plays,
 * which played the default's while the socket connected. The `prefs` op
 * answers them again once it is open.
 */
/** The viewer this page was served for (`<meta name="phitex-viewer">`, a hash of viewer.js, crates/cli/src/view.rs). */
function ownViewer(): string {
  return document.querySelector<HTMLMetaElement>('meta[name="phitex-viewer"]')?.content ?? "";
}

/**
 * The tab told that phitex was updated (`Socket.onStale`): a bar across the top it can't
 * close, with Reload; the pages shown stay, and nothing more is asked of the watch.
 */
function staleBar(): void {
  if (document.getElementById("phitex-stale")) return;
  const bar = document.createElement("div");
  bar.id = "phitex-stale";
  bar.setAttribute("role", "alert");
  Object.assign(bar.style, {
    position: "fixed", top: "0", left: "0", right: "0", zIndex: "2147483647",
    display: "flex", gap: "12px", alignItems: "center", justifyContent: "center",
    padding: "10px 16px", font: "14px system-ui, sans-serif",
    background: "#1d3e72", color: "#ffffff", boxShadow: "0 2px 8px rgba(0,0,0,.25)",
  });
  const text = document.createElement("span");
  text.textContent = "phitex was updated: reload this tab to use the new viewer.";
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = "Reload";
  Object.assign(button.style, { font: "inherit", padding: "4px 14px", borderRadius: "6px", border: "0", background: "#ffffff", color: "#1d3e72", cursor: "pointer" });
  button.onclick = () => location.reload();
  bar.append(text, button);
  document.body.append(bar);
  button.focus();
}

const PAGE_PREFS = ((): any => {
  try {
    return JSON.parse(document.getElementById("phitex-prefs")?.textContent ?? "null");
  } catch {
    return null;
  }
})();
/** The panel's part of the user's preferences (`p`: the `prefs` op's answer) applied. */
function panelPrefs(p: any): void {
  if (!p) return;
  panel.tipsSeen(((p["tips-seen"] ?? []) as string[]).filter((x) => typeof x === "string"));
  // (the panel's own choices, as the user's file has them)
  panel.viewPrefs({ "dark-pdf": p["dark-pdf"], opening: p.opening, underline: p.underline !== false });
}
panelPrefs(PAGE_PREFS);
// (the whole window: docked in a pane that fills it)
const pane = document.createElement("div");
pane.style.cssText = "position:fixed;inset:0";
document.body.append(pane);
panel.dock(pane);
panel.shown(true);

const host: EditorHost = { loadProject: async () => ({}), onOpen() {}, onChanges() {} };
// (byte offsets of the file on disk: the watch reads it to find the line)
const session = new PreviewSession(host, pages, panel, {
  checkEveryMs: 0,
  remote: {
    source: (file, start, end) => void sock.request({ op: "source", file, start, end }),
    select: (file, start, end) => void sock.request({ op: "select", file, start, end }),
  },
});

// ---- The diff's events: its changes, their places, its build ----

sock.onEvent((e: any) => {
  if (e.event !== "diff") return;
  diffState = e.diff ?? (e.error ? { error: String(e.error) } : {});
  // (the changes along the scrollbar while the diff is shown: where the watch placed them)
  const placed = (diffState.changes ?? [])
    .map((c, i) => (typeof c.page === "number" ? { k: c.page, y: c.y ?? 0, title: `Change ${i + 1}: ${words(c.new_text || "") || words(c.old_text || "")}` } : null))
    .filter((c): c is { k: number; y: number; title: string } => !!c);
  panel.diffMarks(diffState.shown === false ? [] : placed);
  if (diffState.error && diffWait) return void (diffWait(diffState), (diffWait = undefined));
  if (diffState.changes === undefined) return;
  if (diffWait) (diffWait(diffState), (diffWait = undefined));
  else counted?.(diffState.changes);
  // (a change gone to while the diff was typeset: there now, placed)
  if (diffState.built && wanted !== undefined) void gotoChange(wanted);
});

// ---- Printing: the PDF, not this page ----
// (Ctrl+P here printed the page itself: the toolbar and the pages'
// pictures. The build's PDF, doc.pdf as the watch made it, is printed
// instead, from a hidden frame; a browser that won't print a PDF in a
// frame gets it in a tab, to print from there)
let printFrame: HTMLIFrameElement | undefined;
const printPdf = async () => {
  const r = await fetch(new URL("doc.pdf", location.href), { cache: "no-store" }).catch(() => undefined);
  if (!r?.ok) return void panel.msg("There's no PDF to print yet", true, true);
  const url = URL.createObjectURL(new Blob([await r.arrayBuffer()], { type: "application/pdf" }));
  if (printFrame) (URL.revokeObjectURL(printFrame.src), printFrame.remove());
  const f = (printFrame = document.createElement("iframe"));
  f.setAttribute("aria-hidden", "true");
  f.tabIndex = -1;
  f.style.cssText = "position: fixed; right: 0; bottom: 0; width: 0; height: 0; border: 0; visibility: hidden";
  f.onload = () => {
    try {
      f.contentWindow!.focus();
      f.contentWindow!.print();
    } catch {
      window.open(url, "_blank");
      panel.msg("The PDF opened in a new tab: print it from there", false, true);
    }
  };
  f.src = url;
  document.body.append(f);
};
addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || e.key.toLowerCase() !== "p") return;
  e.preventDefault();
  e.stopImmediatePropagation();
  void printPdf();
}, { capture: true });

let started = false;
sock.onOpen = () => {
  panel.msg("connected to phitex watch");
  panel.connection(true);
  if (started) return void session.resync();
  started = true;
  // (a tab that joins after the first build: its time for the bars, and its first page not called one;
  // the last build's settled and stats as the live ones would have said them: the timing popover's
  // build and Copy debug info's stats, without waiting for the next build. Not its keystroke timing:
  // no change of this tab's is on its way)
  void sock.request({ op: "info", what: "built" }).then((r) => {
    if (!r.ok || !(Number(r.json?.builds) > 0)) return;
    const st = r.json?.stats, se = r.json?.settled as Settled | null | undefined;
    const last = se && typeof se.ms === "number" ? { ms: se.ms, cold: !!se.cold } : undefined;
    panel.joinedLate(typeof r.json?.first_ms === "number" ? r.json.first_ms : undefined, Number(r.json.builds), last);
    if (st && typeof st === "object" && !lastStats) statsIn(st as Stats);
    if (se && typeof se.ms === "number") panel.latency(...timing(se));
  });
  void session.start();
};
// (never connected: the card says so, not "Reading" forever; a browser add-on
// blocking WebSockets to this machine is the usual cause)
// (never connected: the card says why, not "Reading" forever)
const unreached = async () => {
  if (started) return;
  if (!(await sock.there())) return panel.error("phitex watch isn't answering here: is it still running? (trying again)");
  if (sock.ws?.readyState === WebSocket.CONNECTING)
    return panel.error(
      "phitex watch answers, but the browser hasn't opened the WebSocket to it yet. Firefox opens one at a time to this machine and waits longer after failures: another tab connecting to 127.0.0.1 (an old phitex viewer?) holds it back. Closing such tabs lets it through.",
    );
  panel.error("phitex watch answers, but the browser refused the WebSocket to it (trying again): a browser add-on blocking connections to 127.0.0.1? Try a private window, or allow 127.0.0.1 in the add-on.");
};
sock.onClose = () => {
  if (!started) return void unreached();
  panel.msg("not connected to phitex watch (retrying)", true);
  panel.connection(false, "phitex watch stopped, or the connection dropped. Reconnecting…");
  // (the watch's last status line, its viewer count with it, no longer holds)
  status = { kind: "status", text: "not connected to phitex watch" };
  busy = "";
  showBar();
};
setTimeout(() => void unreached(), 3000);
void sock.connect();

// ---- The terminal's messages (the watch's console, view.rs) ----
// Every line phitex watch prints (notes, errors, warnings, each build's
// result, the browsers that come and go), in a drawer under the pages, and
// its status line in a bar along the bottom: click it, or press `, to open
// the drawer.

/** A message as the watch tells it (ui.rs `Ui::tap`). */
type Said = { kind: string; text?: string; ok?: boolean | null; event?: unknown };

document.head.append(Object.assign(document.createElement("style"), {
  textContent: `
#phx-bar { position: fixed; left: 0; right: 0; bottom: 0; height: 24px; z-index: 15; display: flex; align-items: center; gap: 10px;
  padding: 0 10px; font-size: 12px; font-variant-numeric: tabular-nums; background: var(--bg); color: var(--fg2); border-top: 1px solid var(--line);
  cursor: pointer; user-select: none; white-space: nowrap; overflow: hidden; }
#phx-bar:hover { color: var(--fg); }
#phx-bar .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg2); flex: none; }
#phx-bar .dot.ok { background: #2e9d63; } #phx-bar .dot.bad { background: var(--danger); }
#phx-bar .dot.busy { background: #f58f2d; animation: phx-pulse .8s ease-in-out infinite alternate; }
@keyframes phx-pulse { from { opacity: .35; } }
@media (prefers-reduced-motion: reduce) { #phx-bar .dot.busy { animation: none; } }
#phx-bar .text { overflow: hidden; text-overflow: ellipsis; flex: 1; }
#phx-bar .new { flex: none; color: var(--danger); font-weight: 600; }
#phx-bar .new.warn { color: var(--warning); }
#phx-bar .hint { flex: none; color: var(--fg2); }
#phx-con { position: fixed; left: 0; right: 0; bottom: 24px; height: 38vh; z-index: 15; display: flex; flex-direction: column;
  background: var(--bg); color: var(--fg); border-top: 1px solid var(--line); box-shadow: 0 -4px 16px rgb(0 0 0 / 12%); }
#phx-con[hidden] { display: none; }
#phx-con header { display: flex; align-items: baseline; gap: 10px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
#phx-con header b { font-weight: 600; }
#phx-con header small { color: var(--fg2); }
#phx-con header button { border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; }
#phx-con header button:hover { background: var(--hover); }
#phx-con header button:first-of-type { margin-left: auto; }
#phx-con .log { flex: 1; overflow: auto; margin: 0; padding: 8px 10px; font: 12px/1.5 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre-wrap; word-break: break-word; }
#phx-con .error { color: var(--danger); } #phx-con .warning { color: var(--warning); } #phx-con .note, #phx-con .dim { color: var(--fg2); }
#phx-con .ok { color: var(--fg); } #phx-con .busy { color: var(--fg2); }
}`,
}));
// (the pages above the bar)
pane.style.bottom = "24px";
const bar = document.createElement("div");
bar.id = "phx-bar";
bar.title = "phitex watch's status: click (or `) for everything it printed";
bar.innerHTML = `<span class="dot"></span><span class="text">phitex watch</span><span class="new" hidden></span><span class="hint">Terminal (\`)</span>`;
const con = document.createElement("div");
con.id = "phx-con";
con.hidden = true;
con.innerHTML = `<header><b>Terminal</b><small>what phitex watch printed</small><button type="button" data-a="rebuild" title="Build again (r in the terminal)">Rebuild</button><button type="button" data-a="clear">Clear</button><button type="button" data-a="close">Close</button></header><pre class="log"></pre>`;
document.body.append(con, bar);
const log = con.querySelector<HTMLElement>(".log")!;
const dot = bar.querySelector<HTMLElement>(".dot")!;
const barText = bar.querySelector<HTMLElement>(".text")!;
const badge = bar.querySelector<HTMLElement>(".new")!;
let status: Said = { kind: "status", text: "" };
let busy = "";
let unseen = { errors: 0, warnings: 0 };

/** The tab's icon: a dot in the status's colour (green built, orange building, red errors, grey not connected). */
const favicon = (() => {
  const link = Object.assign(document.createElement("link"), { rel: "icon" });
  document.head.append(link);
  let was = "";
  return (colour: string) => {
    if (colour === was) return;
    was = colour;
    link.href = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="${colour}"/></svg>`)}`;
  };
})();

/** The tab's title and icon: the document and what the watch is doing ("paper.tex · Building…", "· 2 errors", "· Built", "· Not connected"). */
const showTitle = () => {
  const file = (status.text ?? "").split(" · ")[0] || "phitex watch";
  const connected = !/not connected/.test(status.text ?? "");
  const errs = /(\d+) errors?/.exec(status.text ?? "")?.[1];
  const [what, colour] = !connected
    ? ["Not connected", "#8a93a3"]
    : busy
      ? ["Building…", "#f58f2d"]
      : status.ok === false
        ? [errs ? `${errs} error${errs === "1" ? "" : "s"}` : "Failed", "#d1453b"]
        : status.ok === true
          ? ["Built", "#2e9d63"]
          : ["Loading…", "#8a93a3"];
  document.title = connected && file !== "phitex watch" ? `${file} · ${what}` : `${what} · phitex watch`;
  // (the openings that show it: a prompt's, a title bar's)
  if (/\.(tex|ltx)$/.test(file)) panel.docName(file);
  favicon(colour);
};

const showBar = () => {
  showTitle();
  dot.className = "dot" + (busy ? " busy" : status.ok === true ? " ok" : status.ok === false ? " bad" : "");
  barText.textContent = busy ? `${busy}…` : status.text || "phitex watch";
  const n = unseen.errors + unseen.warnings;
  badge.hidden = n === 0 || !con.hidden;
  badge.className = "new" + (unseen.errors ? "" : " warn");
  badge.textContent = unseen.errors ? `${unseen.errors} error${unseen.errors > 1 ? "s" : ""}` : `${n} warning${n > 1 ? "s" : ""}`;
};
/** The class a line is shown in: its kind, or what it begins with. */
const look = (s: Said): string => {
  const t = s.text ?? "";
  if (s.kind !== "line") return s.kind;
  if (/^(error|✗|failed:)/.test(t)) return "error";
  if (/^warning/.test(t)) return "warning";
  if (/^(✓|ok:|finished:)/.test(t)) return "ok";
  if (/^(phitex:|viewer:|\s)/.test(t)) return "dim";
  return "";
};
const said = (s: Said, live: boolean) => {
  if (s.kind === "status") return void ((status = s), showBar());
  if (s.kind === "busy") {
    busy = s.text ?? "";
    // (before the first page: the card's steps follow the build's phases,
    // `xelatex d.tex: build 1 (cold) · typesetting · 4.7 s · …`, said for
    // the writer: what is made and for how long, not the engine's cores)
    const parts = busy.split(" · ");
    if (parts.length >= 3) {
      const [phase, time] = [parts[1], parts[2]];
      if (phase.startsWith("making")) panel.loadStep(1, `Making the format, once per PhiTeX update · ${time}`);
      else if (phase === "format") panel.loadStep(1, `Loading the format · ${time}`);
      else panel.loadStep(2, `Preparing the live preview · ${time}`);
    }
    return void showBar();
  }
  if (s.kind === "event") return;
  const at = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  const line = document.createElement("div");
  line.className = look(s);
  line.textContent = s.kind === "note" ? `phitex: ${s.text}` : s.kind === "error" || s.kind === "warning" ? `${s.kind}: ${s.text}` : s.text ?? "";
  log.append(line);
  while (log.childElementCount > 2000) log.firstElementChild!.remove();
  if (at) log.scrollTop = log.scrollHeight;
  // (the count is the last build's: its result line starts it again, its
  // `error:` and `warning:` lines after it counted; the result line itself
  // is not one more. Opening the drawer reads them)
  if (live && /^(✓|ok:|✗|failed:)/.test(s.text ?? "")) (unseen = { errors: 0, warnings: 0 }), showBar();
  else if (live && con.hidden && /^(error|warning): /.test(line.textContent ?? "")) {
    if (line.className === "error") unseen.errors++;
    else unseen.warnings++;
    showBar();
  }
};
const toggle = (open = con.hidden) => {
  con.hidden = !open;
  if (open) (unseen = { errors: 0, warnings: 0 }), (log.scrollTop = log.scrollHeight);
  showBar();
};
bar.onclick = () => toggle();
con.querySelector("header")!.onclick = (e) => {
  const a = (e.target as HTMLElement).dataset?.a;
  if (a === "close") toggle(false);
  if (a === "clear") log.replaceChildren();
  if (a === "rebuild") void sock.request({ op: "rebuild" });
};
addEventListener("keydown", (e) => {
  const t = e.target as HTMLElement;
  if (t?.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t?.tagName ?? "")) return;
  if (e.key === "`" && !e.ctrlKey && !e.metaKey && !e.altKey) (toggle(), e.preventDefault());
  else if (e.key === "Escape" && !con.hidden) toggle(false);
});
sock.onEvent((e: any) => {
  if (e.event === "console") said(e.item as Said, true);
});
/** The messages so far: on each (re)connection, the drawer filled again. */
const backlog = async () => {
  const r = await sock.request({ op: "console" });
  if (!r.ok) return;
  log.replaceChildren();
  for (const s of (r.json?.items ?? []) as Said[]) said(s, false);
  if (r.json?.status) said(r.json.status as Said, false);
};
// ---- Settings (the user's phitex.toml, through the watch: on every port and document) ----

/** When the page follows the editor's place (`follow`): `cursor`, `selection`, `dblclick`. */
let follow = "cursor";

/** A setting of the user's phitex.toml set (`prefs`): the watch's error, if any. */
const setPref = async (k: string, v: unknown): Promise<string | undefined> => {
  const r = await sock.request({ op: "prefs", set: { [k]: v } });
  return r.ok ? undefined : (r.error ?? "not saved");
};

/** The watch's rows of the Settings dialog: the user's file's (set here) and the project's (shown: its phitex.toml's, or the command line's). */
async function watchSettings(): Promise<SettingsGroup[]> {
  const [r, info, cache, rv] = await Promise.all([sock.request({ op: "prefs" }), sock.request({ op: "info", what: "settings" }), sock.request({ op: "cache" }), sock.request({ op: "review_state" })]);
  if (!r.ok) return [{ title: "Preview", rows: [{ id: "prefs", label: "Your settings", kind: "info", value: "", disabled: `Couldn't read them: ${r.error ?? "not available"}` }] }];
  const p = r.json ?? {}, at = info.ok ? (info.json ?? {}) : {};
  const file = String(p.file ?? "phitex.toml");
  const project = "In the project's phitex.toml (or the command line): the page never changes a project's files";
  const mb = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);
  const toggle = (id: string, label: string, help: string): SettingRow => ({ id, label, help, kind: "toggle", value: !!p[id], set: (v) => setPref(id, v) });
  const cores = Number(p.cores ?? 1);
  return [
    {
      title: "Preview",
      rows: [
        { id: "w-main", label: "Main file", help: project, kind: "info", value: String(at.main ?? "") },
        { id: "w-engine", label: "Engine", help: "Set `engine` in the project's phitex.toml (or a `% !TEX program` line)", kind: "info", value: String(at.engine ?? "") },
        {
          id: "follow",
          label: "Highlight follows",
          help: "Where your editor is, on the page: as the cursor moves, what you select, or a double-click alone",
          kind: "choice",
          options: [["cursor", "Cursor"], ["selection", "Selection"], ["dblclick", "Double-click"]],
          value: String(p.follow ?? "cursor"),
          set: async (v) => (await setPref("follow", v)) ?? void (follow = String(v)),
        },
      ],
    },
    {
      title: "Build",
      rows: [
        toggle("watch-write", "Write the PDF into the folder", "After each build, the PDF with its .synctex.gz, as phitex build does. From the next phitex watch on."),
        toggle("watch-ask-write", "Ask at quit whether to write the PDF", "When nothing was written while watching, quitting asks whether to write the PDF here. Off: it writes nothing and asks nothing."),
        toggle("synctex", "SyncTeX file with a written PDF", "JOB.synctex.gz beside the PDF, for a PDF viewer's jumps to the source."),
        {
          id: "threads",
          label: "Threads",
          help: at.jobs ? `This watch runs with -j ${at.jobs}, over this. After a restart of phitex watch.` : "After a restart of phitex watch. The output is the same at any count.",
          kind: "select",
          options: [["0", `One per core (${cores})`], ...Array.from({ length: cores }, (_, i): [string, string] => [String(i + 1), String(i + 1)])],
          value: String(p.threads ?? 0),
          set: (v) => setPref("threads", Number(v)),
        },
        { id: "w-files", label: "TeX's files", help: "From TeX Live or from Shelf: `files` in your phitex.toml or the project's, or --shelf", kind: "info", value: at.files === "shelf" ? "Shelf" : "TeX Live" },
      ],
    },
    {
      title: "Packages",
      rows: [
        { id: "cache", label: "The cache", help: cache.ok ? `${mb(Number(cache.json?.bytes ?? 0))} in ${cache.json?.dir ?? "its folder"}: packages and formats, fetched again when a build needs them` : "Its size: not available", kind: "action", button: "Clear", set: () => void clearCache() },
        {
          id: "cache-limit",
          label: "Keep at most",
          help: "Past it, the least recently used go first",
          kind: "choice",
          options: [["500MB", "500 MB"], ["1GB", "1 GB"], ["4GB", "4 GB"], ["16GB", "16 GB"]],
          value: { 5e8: "500MB", 1e9: "1GB", 4e9: "4GB", 16e9: "16GB" }[Number(p["cache-limit"] ?? 4e9)] ?? "",
          set: (v) => setPref("cache-limit", v),
        },
      ],
    },
    {
      title: "Experimental",
      rows: [
        {
          id: "experimental-review",
          label: "Review your agent's edits",
          help: "Its edits to your files shown here change by change, to accept, reject or comment on (the Changes tab beside the pages). Off: nothing of it is shown or kept. For this watch now, and from the next on.",
          kind: "toggle",
          value: !!p["experimental-review"],
          set: async (v) => {
            const r = await sock.request({ op: "review_changes", on: !!v });
            if (!r.ok) return r.error ?? "not changed";
            return (await setPref("experimental-review", !!v)) ?? undefined;
          },
        },
        {
          id: "review-mode",
          label: "Your agent's changes",
          help: "Edits: it edits the files, its changes show on the pages and wait for you. Proposes: they are kept apart until you accept them (the agent writes in .phitex/work/). For this watch now, and your default from the next.",
          kind: "choice",
          options: [["direct", "Edits"], ["propose", "Proposes"]],
          value: String(rv.json?.mode ?? p["review-mode"] ?? "direct"),
          set: async (v) => {
            const r = await sock.request({ op: "review_mode", mode: v });
            if (!r.ok) return r.error ?? "not changed";
            return (await setPref("review-mode", v)) ?? undefined;
          },
        },
      ],
    },
    { title: "Updates and tips", rows: [toggle("update-check", "Say when a newer PhiTeX is out", "Asked once a day, in the terminal.")] },
    { title: "Where", rows: [{ id: "w-file", label: "Your settings", help: "Every document and port: `phitex config` lists them all", kind: "info", value: file }] },
  ];
}

/** The user's viewer preferences (the `prefs` op's answer, `any` as `request` gives it) applied. */
const applyPrefs = (p: any) => {
  panelPrefs(p);
  follow = String(p?.follow ?? "cursor");
};
if (PAGE_PREFS) follow = String(PAGE_PREFS.follow ?? "cursor");
/** The user's viewer preferences, as the watch keeps them (`prefs`). */
const prefs = async () => {
  const r = await sock.request({ op: "prefs" });
  if (r.ok) applyPrefs(r.json);
};
const opened = sock.onOpen;
sock.onOpen = () => (opened(), void backlog(), void prefs());
showBar();


