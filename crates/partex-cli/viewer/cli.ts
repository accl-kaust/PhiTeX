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
import { PROBLEMS_CSS, Problems, counts, type Problem } from "../../../viewer/src/problems.ts";

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
document.head.append(Object.assign(document.createElement("style"), { textContent: VIEWER_CSS + PROBLEMS_CSS }));

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
const pillEl = document.getElementById("pill")!;
// (the build's errors over the pages; a place clicked opens in the editor, as a double-click on the page does)
const problems = new Problems(document.body, (file, line, col) => void sock.request({ op: "source", file, line, col }));
pillEl.addEventListener("click", () => (found.errors || found.warnings) && problems.toggle());

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

/** The build running now (its last `progress` event), the last build's time, its problems counted. */
let progress: { pass: number; pages: number; phase: string; ms: number } | null = null;
let lastMs: number | null = null;
let found = { errors: 0, warnings: 0 };

function status(): void {
  const n = hashes.length;
  statusEl.textContent = !connected ? "" : n ? `page ${current + 1} of ${n}` : "no pages yet";
  // (the pill: what the engine does now, else how the last build went)
  let cls: string, text: string;
  if (!connected) [cls, text] = ["off", "not connected to phitex watch (retrying)"];
  else if (building) {
    const p = progress;
    const secs = p ? ` · ${(p.ms / 1000).toFixed(1)} s` : "";
    [cls, text] = ["busy", p ? `pass ${p.pass} · ${p.phase}${p.pages ? ` · ${p.pages} page${p.pages === 1 ? "" : "s"}` : ""}${secs}` : "building…"];
  } else if (found.errors) [cls, text] = ["bad", `${found.errors} error${found.errors === 1 ? "" : "s"}${found.warnings ? ` · ${found.warnings} warning${found.warnings === 1 ? "" : "s"}` : ""}`];
  else if (found.warnings) [cls, text] = ["warn", `built${lastMs != null ? ` in ${(lastMs / 1000).toFixed(1)} s` : ""} · ${found.warnings} warning${found.warnings === 1 ? "" : "s"}`];
  else [cls, text] = ["ok", `built${lastMs != null ? ` in ${(lastMs / 1000).toFixed(1)} s` : ""}`];
  pillEl.className = cls;
  pillEl.textContent = text;
  pillEl.title = found.errors || found.warnings ? "Show the build's problems (e)" : "";
  document.body.classList.toggle("stale", building || !connected);
  document.body.classList.toggle("building", building && connected);
}

/** A build's diagnostics: counted on the pill, shown over the pages if there are errors. */
function diagnosed(items: Problem[]): void {
  found = counts(items);
  problems.set(items);
  status();
}

sock.onEvent = (e: { event: string; on?: boolean; file?: string; lo?: number; hi?: number; at?: number; k?: number; hash?: string; pages?: number; pass?: number; phase?: string; ms?: number; items?: Problem[] }) => {
  if (e.event === "page") {
    // (a page shipped while the build runs: shown before the build is in,
    // the PDF's page replacing it when it settles)
    building = true;
    const k = e.k!;
    while (hashes.length < Math.max(k + 1, e.pages ?? 0)) hashes.push("");
    hashes[k] = e.hash!;
    viewer.shipped(k, e.hash!);
    status();
  } else if (e.event === "progress") {
    building = true;
    progress = { pass: e.pass ?? 1, pages: e.pages ?? 0, phase: e.phase ?? "", ms: e.ms ?? 0 };
    status();
  } else if (e.event === "diagnostics") {
    diagnosed(e.items ?? []);
  } else if (e.event === "settled") {
    building = false;
    if (progress) lastMs = progress.ms;
    progress = null;
    void layout();
  } else if (e.event === "switched") {
    // (the build's glyph origins changed: asked again)
    glyphCache = new Map();
  } else if (e.event === "sync") {
    void show(e.file!, e.lo!, e.hi!, e.at!);
  } else if (e.event === "preparing") {
    building = !!e.on;
    status();
  }
};
sock.onOpen = () => {
  connected = true;
  void layout();
  // (the last build's problems: a page opened after it settled has had no event)
  void sock.request({ op: "diagnostics" }).then((r) => r.ok && diagnosed(r.json?.items ?? []));
};
sock.onClose = () => {
  connected = false;
  status();
};
sock.connect();

// (pages sized again at most once a frame, however many wheel or resize events come in it)
let sizing = 0;
const resize = () => {
  if (!sizing)
    sizing = requestAnimationFrame(() => {
      sizing = 0;
      viewer.redraw();
    });
};
addEventListener("resize", () => {
  if (!zoom) resize();
});
addEventListener(
  "wheel",
  (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    // (by how far the wheel or the pinch went: a touchpad sends many small steps)
    const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    zoom = Math.min(5, Math.max(0.25, (zoom || fitScale()) * Math.exp(-px / 400)));
    resize();
  },
  { passive: false },
);
// (the keys: as PDF viewers have them, and vim's; `?` lists them)
const helpEl = document.createElement("div");
helpEl.id = "keys";
helpEl.hidden = true;
helpEl.innerHTML = `<b>Keys</b><table>${[...KEYS, ["e", "the build's errors and warnings"] as [string, string]].map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
document.body.append(helpEl);
// (the problems' keys, before the pages' own: `e` shows or hides them, Esc closes them)
addEventListener(
  "keydown",
  (e) => {
    if (e.ctrlKey || e.altKey || e.metaKey || (e.target as HTMLElement | null)?.closest?.("input, textarea, select")) return;
    if (e.key === "Escape" && problems.shown) problems.close();
    else if (e.key === "e") problems.toggle();
    else return;
    e.preventDefault();
    e.stopImmediatePropagation();
  },
  { capture: true },
);
const setZoom = (z: number) => {
  zoom = z;
  resize();
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
