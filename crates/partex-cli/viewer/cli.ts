// The live viewer's page in the CLI (DESIGN 4.8): the one viewer every
// host shows (viewer/src: the panel, panel.ts, on the session, session.ts;
// the Overleaf extension and VS Code show the same), here on `phitex
// watch`, which builds from disk: the session in its remote mode
// (Options.remote) asks the watch for the pages over a WebSocket, as the
// extension's session asks its core (CoreReq ops: pages, png, status,
// origins; and the watch's own: outline, diagnostics, source), and shows
// what the watch tells (page, progress, diagnostics, settled, superseded,
// sync). The page's keys, the contents beside the pages and the problems
// are the panel's (PanelOptions.keys, .outline).
//
// Bundled by scripts/viewer-bundle.sh into viewer.js.

import { Panel } from "../../../viewer/src/panel.ts";
import { PreviewSession, type CoreEvent, type CoreRes, type CoreTransport, type EditorHost } from "../../../viewer/src/session.ts";
import { setFontBase } from "../../../viewer/src/page2.ts";
import { DiffControls, LATEXDIFF_LOOK, type DiffRunner } from "../../../viewer/src/compare.ts";
import type { DiffLook } from "../../../viewer/src/panel.ts";
import { COMMITS_CSS, Commits, type Commit } from "../../../viewer/src/commits.ts";

// (the text fonts, from the server)
setFontBase(new URL("fonts/", location.href).href);

/** The watch's core, over a WebSocket: requests answered in order, events pushed; joined again when it drops. */
class Socket implements CoreTransport {
  private ws!: WebSocket;
  private next = 1;
  private waiting = new Map<number, (r: CoreRes) => void>();
  private events: ((e: CoreEvent) => void)[] = [];
  onOpen: () => void = () => {};
  onClose: () => void = () => {};

  connect(): void {
    const url = new URL("ws", location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    this.ws = new WebSocket(url);
    this.ws.onopen = () => this.onOpen();
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
      setTimeout(() => this.connect(), 1000);
    };
  }

  request(op: object): Promise<CoreRes> {
    const id = this.next++;
    return new Promise((ok) => {
      this.waiting.set(id, ok);
      if (this.ws.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ id, ...op }));
      else (this.waiting.delete(id), ok({ ok: false, error: "not connected" } as CoreRes));
    });
  }

  onEvent(cb: (e: CoreEvent) => void): void {
    this.events.push(cb);
  }
}

const sock = new Socket();
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
    onFormat: () => {},
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
    pdfjs: false,
    outline: true,
    keys: true,
    words: {
      badge: "watch",
      badgeTitle: "phitex watch: built from the files on disk, again at each save",
      byline: "⚡ phitex watch",
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
const session = new PreviewSession(host, sock, panel, {
  format: "vector",
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
        void controls.start({ from, to }, to ? `${short(from)} → ${short(to)}` : `since ${short(from)}`, "git");
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
sock.onClose = () => panel.msg("not connected to phitex watch (retrying)", true);
sock.connect();
