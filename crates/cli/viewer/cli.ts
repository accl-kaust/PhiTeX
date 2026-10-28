// The live viewer's page in the CLI (DESIGN 4.8): the one viewer every
// host shows (viewer/src: the panel, panel.ts, on the session, session.ts;
// the Overleaf extension and VS Code show the same), here on `phitex
// watch`, which builds from disk: the session in its remote mode
// (Options.remote) asks the watch for the pages over a WebSocket, as the
// extension's session asks its core (CoreReq ops: pages, status, origins;
// and the watch's own: diagnostics, source), and shows
// what the watch tells (page, progress, diagnostics, settled, superseded,
// sync; reveal, an LSP editor's cursor, marked here). The page's keys, the contents beside the pages and the problems
// are the panel's (PanelOptions.keys, .outline).
//
// The pages are drawn here, with MuPDF (viewer/src/mupdf.ts), from the
// watch's doc.pdf: the `png` and `outline` requests are answered in the
// page, the PDF loaded again after each build.
//
// Bundled by scripts/viewer-bundle.sh into viewer.js.

import { Panel } from "../../../viewer/src/panel.ts";
import { PreviewSession, type CoreEvent, type CoreRes, type CoreTransport, type EditorHost } from "../../../viewer/src/session.ts";
import { MupdfDoc, toCanvas, type Hit } from "../../../viewer/src/mupdf.ts";
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
 * The watch's core with the pages drawn here: `png` and `outline` answered
 * by MuPDF from doc.pdf (loaded again after each `settled`), the rest
 * asked of the watch.
 */
class Pages implements CoreTransport {
  private sock: Socket;
  private mupdf: Promise<any> | null = null;
  private doc: MupdfDoc | null = null;
  /** The builds settled, and the one the open PDF is of. */
  private version = 0;
  private docVersion = -1;
  private loading: Promise<MupdfDoc | null> | null = null;
  /** A change on its way to the screen: ms from its save to `settled`, and when that came. */
  private change: { since: number; at: number } | null = null;

  constructor(sock: Socket) {
    this.sock = sock;
    sock.onEvent((e) => {
      if (e.event !== "settled") return;
      this.version++;
      const since = (e as { since_ms?: number | null }).since_ms;
      this.change = typeof since === "number" ? { since, at: performance.now() } : null;
    });
  }

  private async open(): Promise<MupdfDoc | null> {
    if (this.doc && this.docVersion === this.version) return this.doc;
    if (this.loading) return this.loading;
    const v = this.version;
    this.loading = (async () => {
      // (MuPDF's module, from the watch, its .wasm beside it)
      this.mupdf ??= import(new URL("mupdf/mupdf.js", location.href).href);
      const m = await this.mupdf;
      const r = await fetch(new URL("doc.pdf", location.href), { cache: "no-store" });
      if (!r.ok) return null;
      const pdf = new Uint8Array(await r.arrayBuffer());
      this.doc?.destroy();
      this.doc = new MupdfDoc(m, pdf);
      this.docVersion = v;
      return this.doc;
    })().finally(() => (this.loading = null));
    return this.loading;
  }

  async request(op: object): Promise<CoreRes> {
    const o = op as { op: string; page?: number };
    if (o.op === "png") {
      const d = await this.open().catch(() => null);
      // (at the zoom shown, on this screen's pixels: the viewer asks again after a zoom in)
      const drawn = d?.draw(o.page ?? 0, panel.pixelsPerPoint());
      if (!drawn) return { ok: false, json: { error: "no such page" } };
      // (the first page drawn after a change: on the screen at the next frame;
      // the ⚡ chip and the watch's status line say how long from the save)
      const c = this.change;
      if (c) {
        this.change = null;
        requestAnimationFrame(() => {
          const ms = c.since + performance.now() - c.at;
          panel.painted(ms);
          void this.sock.request({ op: "painted", ms });
        });
      }
      return { ok: true, canvas: toCanvas(drawn), size: [drawn.w, drawn.h], links: drawn.links };
    }
    if (o.op === "outline") {
      const d = await this.open().catch(() => null);
      return { ok: true, json: { items: d?.outline() ?? [] } };
    }
    return this.sock.request(op);
  }

  /** Where `q` is in the build's PDF (MuPDF's text, every page). */
  async find(q: string): Promise<Hit[]> {
    const d = await this.open().catch(() => null);
    return d?.search(q) ?? [];
  }

  onEvent(cb: (e: CoreEvent) => void): void {
    this.sock.onEvent(cb);
  }
}

const sock = new Socket();
const pages = new Pages(sock);
/** A place in a file (1-based line and column): the watch opens it in the editor (`--editor`). */
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
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
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
      steps: ["Connected", "Format", "Building"],
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
  remote: { source: (file, start, end) => void sock.request({ op: "source", file, start, end }) },
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
  if (!hits.length) return void panel.msg(`change ${k}: not found on the pages`);
  const from = k > lastAt.k ? lastAt.page : 0;
  const at = Math.max(0, hits.findIndex((h) => h.k >= from));
  lastAt = { k, page: hits[at].k };
  panel.reveal(hits.slice(at, at + 1));
};

// (forward search, `phitex watch --lsp`: the editor's cursor as the watch
// placed it, the words around it (`texts`, longest first) found in MuPDF's
// text on its page `page` (from 1) and marked; with no page (a PDF build:
// the engine reads no page text there), the first words found, nearest the
// place last revealed; else the page brought into view)
let revealed = 0;
sock.onEvent(async (e: any) => {
  if (e.event !== "reveal") return;
  const k = typeof e.page === "number" ? e.page - 1 : -1;
  for (const q of (e.texts ?? [e.text]) as string[]) {
    const hits = q ? await pages.find(q) : [];
    const near = (a: Hit, h: Hit) => (Math.abs(h.k - revealed) < Math.abs(a.k - revealed) ? h : a);
    const hit = k >= 0 ? hits.find((h) => h.k === k) : hits.length ? hits.reduce(near) : undefined;
    if (!hit) continue;
    revealed = hit.k;
    return panel.reveal([hit]);
  }
  if (k < 0) return;
  revealed = k;
  panel.reveal([{ k, boxes: [[0, 0, 0, 0]] }]);
});

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
#phx-pick { position: fixed; z-index: 20; top: 36px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; overflow: auto;
  background: #f1f1ef; color: #141414; border: 1px solid #141414; border-radius: 0; box-shadow: none; font: 12px "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif; }
#phx-pick header { display: flex; align-items: baseline; gap: 10px; padding: 8px 12px; border-bottom: 1px solid #c9c9c5; }
#phx-pick header b { font-size: 10px; letter-spacing: .1em; text-transform: uppercase; }
#phx-pick header small { color: #8a8a86; }
#phx-pick header button { margin-left: auto; border: 1px solid #c9c9c5; background: none; border-radius: 0; padding: 0 8px; cursor: pointer; font: 11px ui-monospace, monospace; }
@media (prefers-color-scheme: dark) { #phx-pick { background: #121212; color: #e8e8e4; border-color: #e8e8e4; } #phx-pick header { border-color: #3a3a38; } #phx-pick header button { border-color: #3a3a38; color: #e8e8e4; } }`,
}));
let pick: HTMLElement | undefined;
let graph: Commits | undefined;
const closePick = () => {
  if (pick) pick.hidden = true;
};
panel.headerButton("Compare", "Compare with a past version: the commit graph (click From, shift-click To, double-click a commit against its parent)", () => {
  if (pick && !pick.hidden) return closePick();
  if (!pick) {
    pick = document.createElement("div");
    pick.id = "phx-pick";
    pick.innerHTML = `<header><b>Compare</b><small>click: from · shift-click: to (else the files as they are) · double-click: a commit's own changes</small><button type="button">Esc</button></header>`;
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
});
addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pick && !pick.hidden) (closePick(), e.stopImmediatePropagation());
}, { capture: true });

let started = false;
sock.onOpen = () => {
  panel.msg("connected to phitex watch");
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
sock.onClose = () => (started ? panel.msg("not connected to phitex watch (retrying)", true) : void unreached());
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
#phx-bar, #phx-con { --paper: #f1f1ef; --paper-2: #e6e6e3; --ink: #141414; --ink-2: #4a4a48; --ink-3: #8a8a86; --rule: #c9c9c5; --mark: #d71920; --caution: #8a5a00;
  --mono: "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace; --sans: "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif; }
#phx-bar { position: fixed; left: 0; right: 0; bottom: 0; height: 24px; z-index: 15; display: flex; align-items: center; gap: 10px;
  padding: 0 10px; font: 11px var(--mono); font-variant-numeric: tabular-nums; background: var(--paper); color: var(--ink-2); border-top: 1px solid var(--rule);
  cursor: pointer; user-select: none; white-space: nowrap; overflow: hidden; }
#phx-bar .dot { width: 6px; height: 6px; background: var(--ink-3); flex: none; }
#phx-bar .dot.ok { background: var(--ink); } #phx-bar .dot.bad { background: var(--mark); }
#phx-bar .dot.busy { background: var(--mark); animation: phx-blink .8s steps(2) infinite; }
@keyframes phx-blink { 50% { opacity: 0; } }
#phx-bar .text { overflow: hidden; text-overflow: ellipsis; flex: 1; }
#phx-bar .new { flex: none; padding: 0; color: var(--mark); }
#phx-bar .new.warn { color: var(--caution); }
#phx-bar .hint { flex: none; color: var(--ink-3); font-family: var(--sans); font-size: 10px; letter-spacing: .1em; text-transform: uppercase; }
#phx-bar:hover .hint { color: var(--ink); }
#phx-con { position: fixed; left: 0; right: 0; bottom: 24px; height: 38vh; z-index: 15; display: flex; flex-direction: column;
  background: var(--paper); color: var(--ink); border-top: 1px solid var(--ink); }
#phx-con[hidden] { display: none; }
#phx-con header { display: flex; align-items: baseline; gap: 10px; padding: 5px 10px; font: 11px var(--sans); border-bottom: 1px solid var(--rule); }
#phx-con header b { font-weight: 600; letter-spacing: .1em; text-transform: uppercase; font-size: 10px; }
#phx-con header small { color: var(--ink-3); }
#phx-con header button { border: 1px solid var(--rule); background: none; color: var(--ink-2); border-radius: 0; padding: 0 8px; cursor: pointer;
  font: 10px var(--sans); letter-spacing: .08em; text-transform: uppercase; }
#phx-con header button:hover { border-color: var(--ink); color: var(--ink); }
#phx-con header button:first-of-type { margin-left: auto; }
#phx-con .log { flex: 1; overflow: auto; margin: 0; padding: 8px 10px; font: 12px/1.5 var(--mono); white-space: pre-wrap; word-break: break-word; }
#phx-con .error { color: var(--mark); } #phx-con .warning { color: var(--caution); } #phx-con .note, #phx-con .dim { color: var(--ink-3); }
#phx-con .ok { color: var(--ink); } #phx-con .busy { color: var(--ink-2); }
@media (prefers-color-scheme: dark) {
  #phx-bar, #phx-con { --paper: #121212; --paper-2: #1c1c1c; --ink: #e8e8e4; --ink-2: #a9a9a4; --ink-3: #6e6e6a; --rule: #3a3a38; --mark: #ff4a3d; --caution: #d9a441; }
}`,
}));
// (the pages above the bar)
pane.style.bottom = "24px";
const bar = document.createElement("div");
bar.id = "phx-bar";
bar.title = "phitex watch's status: click (or `) for everything it printed";
bar.innerHTML = `<span class="dot"></span><span class="text">phitex watch</span><span class="new" hidden></span><span class="hint">\` Terminal</span>`;
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
    // `pdftex d.tex: build 1 (cold) · engine · 4.7 s · …`)
    const parts = busy.split(" · ");
    if (parts.length >= 3) {
      const phase = parts[1];
      panel.loadStep(phase === "format" ? 1 : 2, `${phase} · ${parts.slice(2).join(" · ")}`);
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
