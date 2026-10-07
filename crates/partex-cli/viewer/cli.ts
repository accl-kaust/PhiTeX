// The live viewer's page in the CLI (DESIGN 4.8): the renderer (viewer/src:
// viewer.ts, page2.ts: pages stacked, each drawn from its draw list and
// drawn again only when its hash changes; the Overleaf extension draws its
// pages with the same code) behind a ViewerHost that asks `phitex watch`
// for the pages over a WebSocket, as the extension's session asks its core
// (CoreReq ops: pages, png, status).
//
// Bundled by scripts/viewer-bundle.sh into viewer.js.

import { Viewer, type ViewerHost } from "../../../viewer/src/viewer.ts";
import { boxes, from, glyphs, lineAt, nearest } from "../../../viewer/src/sync.ts";
import { setFontBase } from "../../../viewer/src/page2.ts";
import { VIEWER_CSS } from "../../../viewer/src/css.ts";
import { KEYS, bindKeys } from "../../../viewer/src/keys.ts";

/** sync.ts's Glyph: where a glyph is on its page, and its source. */
interface Glyph {
  x: number;
  y: number;
  file: string | null;
  start: number;
  end: number;
  synth: boolean;
}

// (the text fonts, from the server; the renderer's CSS)
setFontBase(new URL("fonts/", location.href).href);
document.head.append(Object.assign(document.createElement("style"), { textContent: VIEWER_CSS }));

type Reply = { id: number; ok: boolean; json?: any; draws?: any; error?: string };

/** The watch's core, over a WebSocket: requests answered in order, events pushed. */
class Socket {
  private ws!: WebSocket;
  private next = 1;
  private waiting = new Map<number, (r: Reply) => void>();
  onEvent: (e: any) => void = () => {};
  onOpen: () => void = () => {};
  onClose: () => void = () => {};

  connect(): void {
    const url = new URL("ws", location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    this.ws = new WebSocket(url);
    this.ws.onopen = () => this.onOpen();
    this.ws.onmessage = (m) => {
      const r = JSON.parse(m.data as string);
      if (r.event) return this.onEvent(r);
      this.waiting.get(r.id)?.(r);
      this.waiting.delete(r.id);
    };
    this.ws.onclose = () => {
      for (const f of this.waiting.values()) f({ id: 0, ok: false, error: "closed" });
      this.waiting.clear();
      this.onClose();
      // (the watch restarted, or the network blinked: try again)
      setTimeout(() => this.connect(), 1000);
    };
  }

  request(op: Record<string, unknown>): Promise<Reply> {
    const id = this.next++;
    return new Promise((ok) => {
      this.waiting.set(id, ok);
      if (this.ws.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ id, ...op }));
      else (this.waiting.delete(id), ok({ id, ok: false, error: "not connected" }));
    });
  }
}

const sock = new Socket();
const pagesEl = document.getElementById("pages")!;
const scroller = document.getElementById("scroller")!;
const statusEl = document.getElementById("status")!;

/** Zoom: 0 fits the page's width to the window; else CSS px per point × 96/72. */
let zoom = 0;
let pageW = 612;
let pageH = 792;
const fitScale = () => Math.max(0.2, (scroller.clientWidth - 32) / ((pageW * 96) / 72));

const host: ViewerHost = {
  need(k: number) {
    void draw(k);
  },
  inView(k: number) {
    current = k;
    status();
  },
  svg: () => null,
  scale: () => zoom || fitScale(),
  box: (d: { w: number; h: number }, w: number) => [Math.floor(w), Math.floor((w * d.h) / d.w)],
  // (a double-click: the source of the glyph nearest it, opened by the watch)
  dbl(k: number, x: number, y: number) {
    void (async () => {
      const g: Glyph | null = nearest(await glyphsOf(k), x, y);
      if (g?.file) await sock.request({ op: "source", file: g.file, start: g.start, end: g.end });
    })();
  },
};

/** Each page's glyphs with their sources, by page and hash (asked again after a build changed the page). */
let glyphCache = new Map<string, Glyph[]>();
async function glyphsOf(k: number): Promise<Glyph[]> {
  const key = `${k}:${hashes[k] ?? ""}`;
  const have = glyphCache.get(key);
  if (have) return have;
  const r = await sock.request({ op: "origins", page: k });
  const gs: Glyph[] = r.ok && r.json?.g ? glyphs(r.json, (n: string) => n) : [];
  if (gs.length) glyphCache.set(key, gs);
  return gs;
}

/** Forward search: the glyphs from bytes [lo, hi) of `file` (a line), the one nearest `at`'s, highlighted (the page in view first). */
async function show(file: string, lo: number, hi: number, at: number): Promise<void> {
  const n = hashes.length;
  const order = [current, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== current)];
  for (const k of order) {
    const all = await glyphsOf(k);
    const hit: Glyph[] = lineAt(from(all, file, lo, hi), at);
    if (hit.length) return viewer.mark(k, boxes(hit, all));
  }
}

const viewer = new Viewer(pagesEl, scroller, host);
let current = 0;
let hashes: string[] = [];
let building = false;
// (what the build running is doing beyond typesetting: a newer save
// superseded it, or its first pass is shown while the job settles)
let doing = "";
let connected = false;

async function draw(k: number): Promise<void> {
  const r = await sock.request({ op: "png", page: k, dpi: 0 });
  if (!r.ok || !r.draws) return;
  if (k === 0 && r.draws.w && r.draws.w !== pageW) {
    pageW = r.draws.w;
    pageH = r.draws.h || pageH;
    if (!zoom) viewer.redraw();
  }
  viewer.set(k, { draws: r.draws }, r.json?.hash ?? null);
}

/** A build is in: the page in view first if it changed, then the others the view shows. */
async function layout(): Promise<void> {
  const r = await sock.request({ op: "pages" });
  if (!r.ok) return;
  const next: string[] = r.json.pages;
  const old = hashes;
  hashes = next;
  if (next.length && current < next.length && old[current] !== next[current] && old.length) await draw(current);
  if (next.length) viewer.layout(next);
  status();
}

function status(): void {
  const n = hashes.length;
  statusEl.textContent = !connected
    ? "not connected to partex watch (retrying)"
    : building
      ? n
        ? `${doing || "building"}… (page ${current + 1} of ${n} so far)`
        : `${doing || "building"}…`
      : n
        ? `page ${current + 1} of ${n}`
        : "no pages yet";
  document.body.classList.toggle("stale", building || !connected);
}

sock.onEvent = (e: { event: string; on?: boolean; settling?: boolean; pass?: number; file?: string; lo?: number; hi?: number; at?: number; k?: number; hash?: string; pages?: number }) => {
  if (e.event === "page") {
    // (a page shipped while the build runs: shown before the build is in,
    // the PDF's page replacing it when it settles)
    building = true;
    const k = e.k!;
    while (hashes.length < Math.max(k + 1, e.pages ?? 0)) hashes.push("");
    hashes[k] = e.hash!;
    viewer.shipped(k, e.hash!);
    status();
  } else if (e.event === "progress" || e.event === "diagnostics") {
    // (the build's status and its errors: the status pill and the error
    // overlay are to come; logged for now)
    console.log("partex", e);
  } else if (e.event === "settled") {
    // (a pass shown while the job's own files settle: still building)
    building = !!e.settling;
    doing = e.settling ? `settling (pass ${(e.pass ?? 1) + 1})` : "";
    void layout();
  } else if (e.event === "superseded") {
    building = true;
    doing = "superseded by a newer save: rebuilding";
    status();
  } else if (e.event === "switched") {
    // (the build's glyph origins changed: asked again)
    glyphCache = new Map();
  } else if (e.event === "sync") {
    void show(e.file!, e.lo!, e.hi!, e.at!);
  } else if (e.event === "preparing") {
    building = !!e.on;
    doing = "";
    status();
  }
};
sock.onOpen = () => {
  connected = true;
  void layout();
};
sock.onClose = () => {
  connected = false;
  status();
};
sock.connect();

addEventListener("resize", () => {
  if (!zoom) viewer.redraw();
});
addEventListener(
  "wheel",
  (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    zoom = Math.min(5, Math.max(0.25, (zoom || fitScale()) * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
    viewer.redraw();
  },
  { passive: false },
);
// (the keys: as PDF viewers have them, and vim's; `?` lists them)
const helpEl = document.createElement("div");
helpEl.id = "keys";
helpEl.hidden = true;
helpEl.innerHTML = `<b>Keys</b><table>${KEYS.map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
document.body.append(helpEl);
const setZoom = (z: number) => {
  zoom = z;
  viewer.redraw();
};
bindKeys(window, {
  get pages() {
    return viewer.pages;
  },
  get page() {
    return viewer.page;
  },
  goTo: (k) => viewer.goTo(k),
  turn: (n) => viewer.turn(n),
  scroller,
  zoom: (f) => setZoom(Math.min(5, Math.max(0.25, (zoom || fitScale()) * f))),
  fitWidth: () => setZoom(0),
  fitPage: () => setZoom(Math.max(0.2, Math.min((scroller.clientHeight - 24) / ((pageH * 96) / 72), fitScale()))),
  help: (show) => {
    helpEl.hidden = !show;
  },
});
