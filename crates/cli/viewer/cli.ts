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
import { DiffControls, LATEXDIFF_LOOK, type DiffRunner } from "../../../viewer/src/compare.ts";
import type { DiffLook, SettingRow, SettingsGroup } from "../../../viewer/src/panel.ts";
import { COMMITS_CSS, Commits, type Commit } from "../../../viewer/src/commits.ts";


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
   * Join the watch: first asked over HTTP whether it is there, so a tab
   * left open after its watch ended never opens WebSockets to a dead port
   * (Firefox opens one WebSocket at a time to an address, and delays
   * those after failures: such a tab would hold back every other's).
   */
  async connect(): Promise<void> {
    // (a tab in the background asks too, over HTTP first: the watch then sees it and opens no second tab)
    if (!(await this.there())) return this.again();
    this.open();
  }

  /** Whether the watch answers HTTP here. */
  async there(): Promise<boolean> {
    try {
      return (await fetch(new URL("ping", location.href), { cache: "no-store" })).ok;
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
    this.ws.onopen = () => ((this.wait = 1000), this.onOpen());
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
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ id, ...op }));
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
type Settled = { since_ms?: number | null; ms?: number; history?: number; cold?: boolean; passes?: number; ran?: number; kept?: number; threads?: number; fetched?: number };

const dur = (ms: number) => (ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)} s`);

/** The timing popover's line and parts for a build (panel.latency: one part a line, its name and value two spaces apart). */
function timing(s: Settled): [string, string] {
  const ms = s.ms ?? 0;
  const parts: [string, string | null][] = [
    [s.cold ? "first build" : "build", `${dur(ms)}${s.cold ? " (the format, the packages, the whole document)" : ""}`],
    ["save to build", typeof s.since_ms === "number" ? dur(s.since_ms) : null],
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
  /** Every page's text, for finding (asked again after each build). */
  private texts: Promise<TextLine[][]> | null = null;

  constructor(sock: Socket) {
    this.sock = sock;
    sock.onEvent((e) => {
      if ((e.event as string) === "stats") return void statsIn(e as unknown as Stats);
      if (e.event !== "settled") return;
      this.texts = null;
      const s = e as Settled;
      // (a cold build has no save before it: its first page is timed from the build's start, "First build · 14.9 s")
      const since = typeof s.since_ms === "number" ? s.since_ms : s.cold && typeof s.ms === "number" ? s.ms : null;
      this.change = since === null ? null : { since, at: performance.now() };
      if (typeof s.ms === "number") panel.latency(...timing(s));
      // (the bars' times: the session's, from the same event)
    });
  }

  async request(op: object): Promise<CoreRes> {
    const o = op as { op: string; page?: number };
    if (o.op === "png") {
      const p = await strips.page(o.page ?? 0, panel.pixelsPerPoint(), panel.pageView(o.page ?? 0)).catch(() => null);
      if (!p) return { ok: false, json: { error: "no such page" } };
      this.painted();
      return { ok: true, canvas: p.el, size: [p.w, p.h], links: p.links };
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
    onCompare: () => openPick(),
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
    onProblemsHidden: (kinds) => void sock.request({ op: "prefs", set: { "problems-hidden": kinds } }),
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

// ---- The diff against a past version (the watch's diff ops, view.rs) ----

/** Two versions: `from` a revision, `to` another (null: the files as they are). */
type Pair = { from: string; to: string | null };
/** The watch's diff state (the `diff` op's answer, the `diff` event's). */
type DiffState = { shown?: boolean; base?: string; to?: string; label?: string; changes?: unknown[]; look?: Partial<DiffLook> };

let diffState: DiffState = {};
let diffWait: ((d: DiffState) => void) | undefined;
let counted: ((n: number) => void) | undefined;
sock.onEvent((e: any) => {
  if (e.event !== "diff") return;
  diffState = e.diff ?? {};
  // (the changes along the scrollbar while the diff is shown: where the watch placed them)
  const placed = ((diffState.changes ?? []) as { page?: number; y?: number; new_text?: string; old_text?: string }[])
    .map((c, i) => (typeof c.page === "number" ? { k: c.page, y: c.y ?? 0, title: `Change ${i + 1}: ${words(c.new_text || "") || words(c.old_text || "")}` } : null))
    .filter((c): c is { k: number; y: number; title: string } => !!c);
  panel.diffMarks(diffState.shown === false ? [] : placed);
  const n = diffState.changes?.length;
  if (n === undefined) return;
  if (diffWait) (diffWait(diffState), (diffWait = undefined));
  else counted?.(n);
});
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
  const c = (diffState.changes ?? [])[k - 1] as { new_text?: string; old_text?: string; page?: number; y?: number } | undefined;
  if (!c) return;
  const q = words(c.new_text || "") || words(c.old_text || "");
  const hits = q ? await pages.find(q) : [];
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

const runner: DiffRunner<Pair> = {
  async start(v, say, count) {
    counted = count;
    say("Diffing…");
    const got = new Promise<DiffState>((ok) => (diffWait = ok));
    await ask({ op: "diff_start", rev: v.from, ...(v.to ? { to: v.to } : {}) });
    say("Typesetting the diff…");
    const d = await got;
    return { changes: d.changes?.length ?? 0 };
  },
  show: (which) => void ask({ op: "diff_show", which }),
  // (change k found in the diff's pages by its words, MuPDF's text: no
  // SyncTeX here; the match after the last one gone to, else the first)
  goto: (k) => void gotoChange(k),
  download: (what) => save(`diff.${what}`),
  look: async () => ({ ...LATEXDIFF_LOOK, ...diffState.look }),
  restyle: (l) => void ask({ op: "diff_restyle", markup: l.markup, subtype: l.subtype, add_color: l.add_color, del_color: l.del_color }),
  stop: () => (panel.diffMarks([]), void ask({ op: "diff_stop" })),
};
const controls = new DiffControls(panel, runner);
addEventListener("keydown", (e) => controls.key(e));

// (the versions to compare: a commit graph in a popover under the header's Compare)
document.head.append(Object.assign(document.createElement("style"), {
  textContent: COMMITS_CSS + `
#phx-pick, #phx-bar, #phx-con { --bg: #ffffff; --fg: #1b222c; --fg2: #5d6778; --line: #e3e6ec; --hover: #eef1f6; --danger: #c23a2f; --warning: #a3620a; --accent: #1d3e72;
  font: 13px/1.4 "Noto Sans", system-ui, sans-serif; }
@media (prefers-color-scheme: dark) {
  #phx-pick, #phx-bar, #phx-con { --bg: #1b212b; --fg: #e7e9ee; --fg2: #98a1b0; --line: #262d38; --hover: #222935; --danger: #f0786d; --warning: #e5a54b; --accent: #8fb3ec; color-scheme: dark; }
}
#phx-pick { position: fixed; z-index: 20; top: 44px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; display: flex; flex-direction: column; overflow: hidden;
  background: var(--bg); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 6px 20px rgb(0 0 0 / 25%); }
#phx-pick[hidden] { display: none; }
/* (the title and Close on one line, the hint under them; the commits scroll in their own box below, never under the header) */
#phx-pick header { flex: none; display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 2px 10px; padding: 9px 12px; border-bottom: 1px solid var(--line); background: var(--bg); }
#phx-pick header b { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
#phx-pick header small { grid-column: 1 / -1; color: var(--fg2); font-size: 12px; }
#phx-pick .phx-commits { flex: 1; min-height: 0; }
#phx-pick header .close { grid-row: 1; grid-column: 2; }
#phx-pick .ends { grid-column: 1 / -1; display: flex; align-items: center; gap: 8px; margin: 6px 0 2px; min-width: 0; }
#phx-pick .ends .end { flex: 1 1 0; min-width: 0; display: flex; align-items: center; gap: 6px; padding: 4px 8px; border: 1px solid var(--line); border-radius: 8px; }
#phx-pick .ends .end.set, #phx-pick .ends .end.t { border-color: color-mix(in srgb, var(--accent) 55%, var(--line)); }
#phx-pick .ends i { font-style: normal; font-size: 10px; font-weight: 700; letter-spacing: .04em; text-transform: uppercase; color: var(--accent); }
#phx-pick .ends .v { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
#phx-pick .ends .f:not(.set) .v { color: var(--fg2); }
#phx-pick .ends .x { all: unset; cursor: pointer; width: 18px; height: 18px; display: grid; place-items: center; border-radius: 50%; color: var(--fg2); font-size: 14px; }
#phx-pick .ends .x[hidden] { display: none; }
#phx-pick .ends .x:hover { background: var(--hover); color: var(--fg); }
#phx-pick .ends .arrow { color: var(--fg2); }
#phx-pick .ask { order: 2; flex: none; display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-top: 1px solid var(--line); background: var(--bg); transition: background .2s; }
#phx-pick .ask.asking { background: color-mix(in srgb, var(--accent) 14%, var(--bg)); }
#phx-pick .ask .q { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg2); }
#phx-pick .ask.asking .q { color: var(--fg); font-weight: 600; }
#phx-pick .ask .no { display: none; }
#phx-pick .ask.asking .no { display: inline-block; }
#phx-pick .ask .yes:disabled { opacity: .45; cursor: default; }
#phx-pick .ask button { border: 1px solid var(--line); background: var(--bg); color: var(--fg); border-radius: 9999px; padding: 2px 12px; cursor: pointer; font: inherit; }
#phx-pick .ask .yes { background: var(--accent); border-color: var(--accent); color: var(--bg); font-weight: 600; }
#phx-pick .ask.nudge { animation: phx-nudge .35s; }
@keyframes phx-nudge { 0%, 100% { transform: none; } 25% { transform: translateX(-4px); } 75% { transform: translateX(4px); } }
#phx-pick header button { border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; white-space: nowrap; }
#phx-pick header button:hover { background: var(--hover); }
`,
}));
let pick: HTMLElement | undefined;
let graph: Commits | undefined;
/** The pair marked ("from..to", to "" the working tree), "" none; and the pair last compared. A pair
 * marked and not compared is asked about whenever the picker goes, reopened or not. */
let marked = "", compared = "";
const closePick = (ask = true) => {
  if (!pick || pick.hidden) return;
  // (a pair marked, not compared: Compare it? the foot's bar lit, the picker kept; asked
  // already, a second Esc or click outside is "Don't compare", the pair kept for next time)
  const bar = pick.querySelector<HTMLElement>(".ask")!;
  if (ask && marked && marked !== compared && !bar.classList.contains("asking")) {
    bar.classList.add("asking");
    bar.classList.remove("nudge");
    void bar.offsetWidth;
    bar.classList.add("nudge");
    bar.querySelector<HTMLButtonElement>(".yes")!.focus();
    return;
  }
  bar.classList.remove("asking", "nudge");
  pick.hidden = true;
};
/** Compare (the panel's menu, its icon): the commit graph, or why there is none (not a git repository). */
function openPick(): void {
  if (pick && !pick.hidden) return closePick();
  if (!pick) {
    pick = document.createElement("div");
    pick.id = "phx-pick";
    pick.innerHTML = `<header><b>Compare with an earlier version</b><button type="button" class="close">Close</button><div class="ends"><span class="end f"><i>From</i><span class="v">click a commit</span><button type="button" class="x" data-end="from" aria-label="Clear From" hidden>×</button></span><span class="arrow">→</span><span class="end t"><i>To</i><span class="v">the working tree</span><button type="button" class="x" data-end="to" aria-label="To the working tree" hidden>×</button></span></div><small>Click one end, then the other (or drag between them) · Double-click: a commit's own changes</small></header><div class="ask"><span class="q">Choose From and To</span><button type="button" class="no">Don't compare</button><button type="button" class="yes" disabled>Compare</button></div>`;
    pick.querySelector<HTMLButtonElement>(".close")!.onclick = () => closePick();
    for (const x of pick.querySelectorAll<HTMLButtonElement>(".ends .x")) x.onclick = () => graph?.clear(x.dataset.end as "from" | "to");
    const ask = pick.querySelector<HTMLElement>(".ask")!;
    ask.querySelector<HTMLButtonElement>(".yes")!.onclick = () => graph?.run();
    ask.querySelector<HTMLButtonElement>(".no")!.onclick = () => closePick(false);
    document.body.append(pick);
    graph = new Commits(pick, {
      load: async (skip, limit) => {
        const r = await sock.request({ op: "diff_log", skip, limit });
        return r.ok ? ((r.json?.commits ?? []) as Commit[]) : (r.error ?? "no git repository here");
      },
      marked: (from, to, w) => {
        marked = from ? `${from}..${to ?? ""}` : "";
        const f = pick!.querySelector<HTMLElement>(".ends .f")!, t = pick!.querySelector<HTMLElement>(".ends .t")!;
        f.querySelector(".v")!.textContent = from ? w.from : "click a commit";
        f.classList.toggle("set", !!from);
        f.querySelector<HTMLElement>(".x")!.hidden = !from;
        t.querySelector(".v")!.textContent = w.to;
        t.querySelector<HTMLElement>(".x")!.hidden = !to;
        ask.querySelector(".q")!.textContent = from ? `Compare ${w.from.split(" ")[0]} → ${to ? w.to.split(" ")[0] : "the working tree"}${marked === compared ? " (shown now)" : ""}` : "Choose From and To";
        ask.querySelector<HTMLButtonElement>(".yes")!.disabled = !from;
        if (!from) ask.classList.remove("asking");
      },
      pick: (from, to) => {
        compared = `${from}..${to ?? ""}`;
        const q = pick!.querySelector(".ask .q")!;
        if (!q.textContent!.endsWith(" (shown now)")) q.textContent += " (shown now)";
        closePick(false);
        const short = (id: string) => id.slice(0, 7);
        void controls.start({ from, to }, short(from), to ? `to ${short(to)}` : "working tree");
      },
    });
    void graph.load();
  }
  pick.hidden = false;
  pick.querySelector<HTMLElement>(".ask")!.classList.remove("asking", "nudge");
}
// (a click outside the picker: it goes, or asks first if a pair is marked; the Compare button's own click toggles it)
addEventListener("pointerdown", (e) => {
  if (!pick || pick.hidden || e.composedPath().some((n) => n === pick || (n as Element).id === "b-compare")) return;
  closePick();
}, { capture: true });
addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pick && !pick.hidden) (closePick(), e.stopImmediatePropagation());
  else if (e.key === "Enter" && pick && !pick.hidden && !(e.target as Element)?.closest?.("button") && graph?.run()) (e.preventDefault(), e.stopImmediatePropagation());
}, { capture: true });

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
  const [r, info, cache] = await Promise.all([sock.request({ op: "prefs" }), sock.request({ op: "info", what: "settings" }), sock.request({ op: "cache" })]);
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
    { title: "Updates and tips", rows: [toggle("update-check", "Say when a newer PhiTeX is out", "Asked once a day, in the terminal.")] },
    { title: "Where", rows: [{ id: "w-file", label: "Your settings", help: "Every document and port: `phitex config` lists them all", kind: "info", value: file }] },
  ];
}

/** The user's viewer preferences, as the watch keeps them (`prefs`). */
const prefs = async () => {
  const r = await sock.request({ op: "prefs" });
  if (!r.ok) return;
  panel.tipsSeen(((r.json?.["tips-seen"] ?? []) as string[]).filter((x) => typeof x === "string"));
  panel.problemsHidden(((r.json?.["problems-hidden"] ?? []) as string[]).filter((x) => typeof x === "string"));
  // (the panel's own choices, as the user's file has them)
  panel.viewPrefs({ "dark-pdf": r.json?.["dark-pdf"], opening: r.json?.opening, underline: r.json?.underline !== false });
  follow = String(r.json?.follow ?? "cursor");
};
const opened = sock.onOpen;
sock.onOpen = () => (opened(), void backlog(), void prefs());
showBar();
