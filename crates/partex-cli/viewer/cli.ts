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

/** viewer.ts's ViewerHost, the part this host gives. */
interface ViewerHost {
  need(k: number): void;
  inView(k: number): void;
  svg(img: unknown, cssWidth: number): string | null;
  scale(): number;
  box(d: { w: number; h: number }, cssWidth: number): [number, number];
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
};

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

sock.onEvent = (e: { event: string; on?: boolean }) => {
  if (e.event === "settled") {
    building = false;
    void layout();
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
