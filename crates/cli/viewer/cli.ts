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
import { PreviewSession, type CoreEvent, type CoreRes, type CoreTransport, type EditorHost } from "../../../viewer/src/session.ts";
import { composeLine, search, type Hit, type TextLine } from "../../../viewer/src/text.ts";
import { Strips, type Frame, type StripSource } from "../../../viewer/src/strips.ts";
import { DiffControls, LATEXDIFF_LOOK, type DiffRunner } from "../../../viewer/src/compare.ts";
import type { DiffLook } from "../../../viewer/src/panel.ts";
import { COMMITS_CSS, Commits, type Commit } from "../../../viewer/src/commits.ts";


/** The watch's core, over a WebSocket: requests answered in order, events pushed; joined again when it drops. */
class Socket implements CoreTransport {
  ws?: WebSocket;
  /** The wait before the next try (doubled to 30 s while the watch is away). */
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
    if (document.hidden) return void addEventListener("visibilitychange", () => void this.connect(), { once: true });
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
    this.wait = Math.min(this.wait * 2, 30000);
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
class Pages implements CoreTransport {
  private sock: Socket;
  /** A change on its way to the screen: ms from its save to `settled`, and when that came. */
  private change: { since: number; at: number } | null = null;
  /** Every page's text, for finding (asked again after each build). */
  private texts: Promise<TextLine[][]> | null = null;

  constructor(sock: Socket) {
    this.sock = sock;
    sock.onEvent((e) => {
      if (e.event !== "settled") return;
      this.texts = null;
      const since = (e as { since_ms?: number | null }).since_ms;
      this.change = typeof since === "number" ? { since, at: performance.now() } : null;
    });
  }

  async request(op: object): Promise<CoreRes> {
    const o = op as { op: string; page?: number };
    if (o.op === "png") {
      const p = await strips.page(o.page ?? 0, panel.pixelsPerPoint()).catch(() => null);
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
    this.sock.onEvent(cb);
  }
}

/**
 * The pages drawn by the watch as strips (crates/output/render,
 * docs/spec-display-list.md): a page is its bands, each a bitmap the
 * watch drew once and this page keeps by its key; a build's page draws
 * only the bands whose keys are new, and the rest are the bitmaps kept,
 * at their places.
 */
/** The watch's strips over HTTP (`frame/K/S`, `strips/KEYS`). */
const stripSource: StripSource = {
  async frame(k, scale) {
    const r = await fetch(new URL(`frame/${k}/${Math.round(scale * 1000)}`, location.href), { cache: "no-store" });
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
    onNeed: (k) => void session.fetch(k),
    // (the build's PDF, as the watch wrote it)
    onPdf: () => {
      const a = document.createElement("a");
      a.href = new URL("doc.pdf", location.href).href;
      a.download = "";
      a.click();
    },
    onMain: () => {},
    onReload: () => void session.resync(),
    onClean: () => void session.resync(),
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
      about: `<b>phitex watch</b>: the document built from the files on disk, again at each save, its pages here as
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
let lastAt = { k: -1, page: 0 };
const gotoChange = async (k: number) => {
  const c = (diffState.changes ?? [])[k - 1] as { new_text?: string; old_text?: string; page?: number } | undefined;
  if (!c) return;
  const q = words(c.new_text || "") || words(c.old_text || "");
  if (!q) return;
  const hits = await pages.find(q);
  if (!hits.length) return void panel.msg(`Change ${k} isn't on the pages`, false, true);
  const from = k > lastAt.k ? lastAt.page : 0;
  const at = Math.max(0, hits.findIndex((h) => h.k >= from));
  lastAt = { k, page: hits[at].k };
  panel.reveal(hits.slice(at, at + 1));
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
  stop: () => void ask({ op: "diff_stop" }),
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
#phx-pick header button { grid-row: 1; grid-column: 2; border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; }
#phx-pick header button:hover { background: var(--hover); }`,
}));
let pick: HTMLElement | undefined;
let graph: Commits | undefined;
const closePick = () => {
  if (pick) pick.hidden = true;
};
/** Compare (the panel's menu, its icon): the commit graph, or why there is none (not a git repository). */
function openPick(): void {
  if (pick && !pick.hidden) return closePick();
  if (!pick) {
    pick = document.createElement("div");
    pick.id = "phx-pick";
    pick.innerHTML = `<header><b>Compare with an earlier version</b><small>Click: from · Shift+click: to (else the files as they are) · Double-click: a commit's own changes</small><button type="button">Close</button></header>`;
    pick.querySelector("button")!.onclick = closePick;
    document.body.append(pick);
    graph = new Commits(pick, {
      load: async (skip, limit) => {
        const r = await sock.request({ op: "diff_log", skip, limit });
        return r.ok ? ((r.json?.commits ?? []) as Commit[]) : (r.error ?? "no git repository here");
      },
      pick: (from, to) => {
        closePick();
        const short = (id: string) => id.slice(0, 7);
        void controls.start({ from, to }, short(from), to ? `to ${short(to)}` : "working tree");
      },
    });
    void graph.load();
  }
  pick.hidden = false;
}
addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pick && !pick.hidden) (closePick(), e.stopImmediatePropagation());
}, { capture: true });

let started = false;
sock.onOpen = () => {
  panel.msg("connected to phitex watch");
  panel.connection(true);
  if (started) return void session.resync();
  started = true;
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
  panel.connection(false, "phitex watch stopped or the connection was lost; reconnecting");
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

const showBar = () => {
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
  if (live && con.hidden) {
    if (line.className === "error") unseen.errors++;
    else if (line.className === "warning") unseen.warnings++;
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
const opened = sock.onOpen;
sock.onOpen = () => (opened(), void backlog());
showBar();
