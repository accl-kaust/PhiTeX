// The live viewer's page in the CLI (DESIGN 4.8): the Overleaf extension's
// Viewer (viewer.ts, page2.ts: pages stacked, each drawn from its draw list
// and drawn again only when its hash changes) behind a ViewerHost that asks
// `partex watch` for the pages over a WebSocket, as the extension's session
// asks its core (session.ts's CoreReq ops: pages, png, status).
//
// Bundled with the extension's sources at a pinned tag by
// scripts/viewer-bundle.sh into viewer.js.

// (./ext/ is the extension's extension/src/ at the pinned tag, put there by
// the bundle script: not in this repository)
// @ts-ignore
import { Viewer } from "./ext/viewer.ts";
// @ts-ignore
import { boxes, from, glyphs, lineAt, nearest } from "./ext/sync.ts";

/** sync.ts's Glyph: where a glyph is on its page, and its source. */
interface Glyph {
  x: number;
  y: number;
  file: string | null;
  start: number;
  end: number;
  synth: boolean;
}

/** viewer.ts's ViewerHost, the part this host gives. */
interface ViewerHost {
  need(k: number): void;
  inView(k: number): void;
  svg(img: unknown, cssWidth: number): string | null;
  scale(): number;
  box(d: { w: number; h: number }, cssWidth: number): [number, number];
  dbl?(k: number, x: number, y: number): void;
}

// (page2.ts loads its text fonts from the extension's own files: here, the
// server's)
(globalThis as unknown as { chrome: unknown }).chrome = {
  runtime: { getURL: (p: string) => new URL(p, location.href).href },
};

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

/** A link: [x0, y0, x1, y1, uri] or [x0, y0, x1, y1, page, top | null], in points from the page's top left. */
type Link = [number, number, number, number, string | number, (number | null)?];
/** Each page's size and links, from its draw list (`"L"`). */
const pageLinks = new Map<number, { w: number; h: number; links: Link[] }>();

/** The link under a pointer event, and its page. */
function linkAt(e: MouseEvent): { k: number; h: number; l: Link } | null {
  const el = (e.target as Element).closest<HTMLElement>(".slot");
  if (!el) return null;
  const k = Number(el.dataset.k);
  const p = pageLinks.get(k);
  if (!p?.links.length) return null;
  const r = el.getBoundingClientRect();
  const x = ((e.clientX - r.left) / r.width) * p.w;
  const y = ((e.clientY - r.top) / r.height) * p.h;
  const l = p.links.find((l) => x >= l[0] && x <= l[2] && y >= l[1] && y <= l[3]);
  return l ? { k, h: p.h, l } : null;
}

// (a click on a link: a web address opens in a new tab (only http, https
// and mailto: a PDF's javascript: is not followed); a place in the
// document scrolls there)
pagesEl.addEventListener("click", (e) => {
  const hit = linkAt(e);
  if (!hit) return;
  e.preventDefault();
  const to = hit.l[4];
  if (typeof to === "string") {
    if (/^(https?:|mailto:)/i.test(to)) window.open(to, "_blank", "noopener");
    return;
  }
  const slot = pagesEl.querySelector<HTMLElement>(`.slot[data-k="${to}"]`);
  if (!slot) return;
  const top = hit.l[5];
  const h = pageLinks.get(to)?.h ?? hit.h;
  const at = top == null ? 0 : (top / h) * slot.offsetHeight;
  scroller.scrollTo({ top: slot.offsetTop + at - 12, behavior: "auto" });
});
pagesEl.addEventListener("mousemove", (e) => {
  const el = (e.target as Element).closest<HTMLElement>(".slot");
  if (el) el.style.cursor = linkAt(e) ? "pointer" : "";
});

async function draw(k: number): Promise<void> {
  const r = await sock.request({ op: "png", page: k, dpi: 0 });
  if (!r.ok || !r.draws) return;
  pageLinks.set(k, { w: r.draws.w, h: r.draws.h, links: r.draws.L ?? [] });
  if (k === 0 && r.draws.w && r.draws.w !== pageW) {
    pageW = r.draws.w;
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
  statusEl.textContent = !connected ? "not connected to partex watch (retrying)" : building ? "building…" : n ? `page ${current + 1} of ${n}` : "no pages yet";
  document.body.classList.toggle("stale", building || !connected);
}

sock.onEvent = (e: { event: string; on?: boolean; file?: string; lo?: number; hi?: number; at?: number }) => {
  if (e.event === "settled") {
    building = false;
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
addEventListener("keydown", (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.key === "+" || e.key === "=") zoom = Math.min(5, (zoom || fitScale()) * 1.1);
  else if (e.key === "-") zoom = Math.max(0.25, (zoom || fitScale()) / 1.1);
  else if (e.key === "0") zoom = 0;
  else return;
  viewer.redraw();
});
