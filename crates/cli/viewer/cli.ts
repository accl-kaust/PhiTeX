// The live viewer's page in the CLI (DESIGN 4.8): the one viewer every
// host shows (viewer/src: the panel, panel.ts, on the session, session.ts;
// the Overleaf extension and VS Code show the same), here on `phitex
// watch`, which builds from disk: the session in its remote mode
// (Options.remote) asks the watch for the pages over a WebSocket, as the
// extension's session asks its core (CoreReq ops: pages, status, origins;
// and the watch's own: diagnostics, source), and shows
// what the watch tells (page, progress, diagnostics, settled, superseded,
// sync). The page's keys, the contents beside the pages and the problems
// are the panel's (PanelOptions.keys, .outline).
//
// The pages are drawn here, with MuPDF (viewer/src/mupdf.ts), from the
// watch's doc.pdf: the `png` and `outline` requests are answered in the
// page, the PDF loaded again after each build.
//
// Bundled by scripts/viewer-bundle.sh into viewer.js.

import { Panel } from "../../../viewer/src/panel.ts";
import { PreviewSession, type CoreEvent, type CoreRes, type CoreTransport, type EditorHost } from "../../../viewer/src/session.ts";
import { MupdfDoc, toCanvas } from "../../../viewer/src/mupdf.ts";
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

  constructor(sock: Socket) {
    this.sock = sock;
    sock.onEvent((e) => {
      if (e.event === "settled") this.version++;
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
      return { ok: true, canvas: toCanvas(drawn), size: [drawn.w, drawn.h], links: drawn.links };
    }
    if (o.op === "outline") {
      const d = await this.open().catch(() => null);
      return { ok: true, json: { items: d?.outline() ?? [] } };
    }
    return this.sock.request(op);
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
    onDebug: () => {},
    onMain: () => {},
    onReload: () => void session.resync(),
    onClean: () => void session.resync(),
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
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
    words: {
      badge: "watch",
      badgeTitle: "phitex watch: built from the files on disk, again at each save",
      byline: "⚡ phitex watch",
      about: `<b>⚡ phitex watch</b>: the document built from the files on disk, again at each save, its pages here as
        they are shipped. Nothing leaves this machine.
        <div class="about-foot">Free software (AGPL-3.0-only), provided as is, without any warranty.</div>`,
      openFailed: "Restart phitex watch, then reload this page.",
      reading: "Reading the document from phitex watch.",
      keptIn: "this machine",
      preparing: "Building… {s} s",
      stopped: "The build stopped: its problems are listed above (e).",
      notReady: "This engine does not run here yet.",
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
  goto: (k) => void ask({ op: "diff_goto", k }),
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
#phx-pick { position: fixed; z-index: 20; top: 44px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; overflow: auto;
  background: #fff; color: #1b222c; border-radius: 10px; box-shadow: 0 12px 40px rgba(0,0,0,.35); font: 13px system-ui, sans-serif; }
#phx-pick header { display: flex; align-items: center; gap: 8px; padding: 10px 14px; border-bottom: 1px solid #e7e9ee; }
#phx-pick header small { color: #6b7280; }
#phx-pick header button { margin-left: auto; border: 0; background: #e7e9ee; border-radius: 6px; padding: 2px 10px; cursor: pointer; }
@media (prefers-color-scheme: dark) { #phx-pick { background: #1b222c; color: #f4f5f6; } #phx-pick header { border-color: #2f3a4c; } #phx-pick header button { background: #2f3a4c; color: #f4f5f6; } }`,
}));
let pick: HTMLElement | undefined;
let graph: Commits | undefined;
const closePick = () => {
  if (pick) pick.hidden = true;
};
panel.headerButton("⇄ Compare", "Compare with a past version: the commit graph (click From, shift-click To, double-click a commit against its parent)", () => {
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
#phx-bar { position: fixed; left: 0; right: 0; bottom: 0; height: 24px; z-index: 15; display: flex; align-items: center; gap: 8px;
  padding: 0 10px; font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; background: #f4f5f6; color: #3c4655; border-top: 1px solid #e0e3e8;
  cursor: pointer; user-select: none; white-space: nowrap; overflow: hidden; }
#phx-bar .dot { width: 8px; height: 8px; border-radius: 50%; background: #9aa3b0; flex: none; }
#phx-bar .dot.ok { background: #2e9e4f; } #phx-bar .dot.bad { background: #d23f3f; }
#phx-bar .dot.busy { background: #2f7de1; animation: phx-pulse 1s infinite alternate; }
@keyframes phx-pulse { to { opacity: .3; } }
#phx-bar .text { overflow: hidden; text-overflow: ellipsis; flex: 1; }
#phx-bar .new { flex: none; border-radius: 8px; padding: 0 7px; background: #d23f3f; color: #fff; }
#phx-bar .new.warn { background: #c98a00; }
#phx-bar .hint { flex: none; opacity: .6; }
#phx-con { position: fixed; left: 0; right: 0; bottom: 24px; height: 38vh; z-index: 15; display: flex; flex-direction: column;
  background: #fbfbfc; color: #1b222c; border-top: 1px solid #d6dae1; box-shadow: 0 -6px 20px rgba(0,0,0,.12); }
#phx-con[hidden] { display: none; }
#phx-con header { display: flex; align-items: center; gap: 8px; padding: 4px 10px; font: 12px system-ui, sans-serif; border-bottom: 1px solid #e7e9ee; }
#phx-con header button { border: 0; background: #e7e9ee; color: inherit; border-radius: 6px; padding: 1px 9px; cursor: pointer; font: inherit; }
#phx-con header button:first-of-type { margin-left: auto; }
#phx-con .log { flex: 1; overflow: auto; margin: 0; padding: 6px 10px; font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap; word-break: break-word; }
#phx-con .error { color: #c62828; } #phx-con .warning { color: #9a6700; } #phx-con .note, #phx-con .dim { color: #6b7280; }
#phx-con .ok { color: #1f7a3a; } #phx-con .busy { color: #2f7de1; }
@media (prefers-color-scheme: dark) {
  #phx-bar { background: #161b22; color: #c9d1d9; border-color: #2f3a4c; }
  #phx-con { background: #0f141a; color: #e6edf3; border-color: #2f3a4c; }
  #phx-con header { border-color: #2f3a4c; } #phx-con header button { background: #2f3a4c; }
  #phx-con .error { color: #ff7b72; } #phx-con .warning { color: #e3b341; } #phx-con .ok { color: #56d364; }
}`,
}));
// (the pages above the bar)
pane.style.bottom = "24px";
const bar = document.createElement("div");
bar.id = "phx-bar";
bar.title = "phitex watch's status: click (or `) for everything it printed";
bar.innerHTML = `<span class="dot"></span><span class="text">phitex watch</span><span class="new" hidden></span><span class="hint">\` terminal</span>`;
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
  if (s.kind === "busy") return void ((busy = s.text ?? ""), showBar());
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
