// The pages, continuous: every page stacked in the scroll view, as Overleaf's
// pdf.js viewer shows them. Only the pages near the view are drawn, and a
// page is drawn again only when its box changed (each page's hash, from the
// core). The page most in view is the one an edit paints first.

import type { PageImage } from "./types.ts";
import { patch, type Draws2, type Link } from "./page2.ts";

export interface ViewerHost {
  /** Draw page `k` (the viewer wants it: in view, and not drawn at its hash). */
  need(k: number): void;
  /** The page most in view changed. */
  inView(k: number): void;
  /** A page as markup: the SVG of its draws at `cssWidth`, or null for a PNG. */
  svg(img: PageImage, cssWidth: number): string | null;
  /** CSS pixels per PDF point at the current zoom (1 is 100%). */
  scale(): number;
  /** Text selected on the pages: its boxes by page, in PDF points from each page's top left. */
  selected?(sel: { k: number; rects: [number, number, number, number][] }[]): void;
  /** A double-click at (x, y) on page `k`, in PDF points from its top left. */
  dbl?(k: number, x: number, y: number): void;
  /** A click on a link to a web address (http, https or mailto): opened by the host (absent: in a new tab). */
  link?(uri: string): void;
  /** A page box's CSS size at `cssWidth` (pdf.js's rounding). */
  box(d: { w: number; h: number }, cssWidth: number): [number, number];
}

interface Slot {
  el: HTMLElement;
  /** The hash the page has now, and the one drawn (null: not drawn). */
  hash: string;
  drawn: string | null;
  img: PageImage | null;
}

/** A US Letter page, in PDF points, until a page says otherwise. */
const LETTER = { w: 612, h: 792 };

export class Viewer {
  private slots: Slot[] = [];
  private size = LETTER;
  private visible = new Set<number>();
  private current = 0;
  private io: IntersectionObserver;
  private asked = new Set<string>();
  private root: HTMLElement;
  private scroller: HTMLElement;
  private host: ViewerHost;

  constructor(root: HTMLElement, scroller: HTMLElement, host: ViewerHost) {
    this.root = root;
    this.scroller = scroller;
    this.host = host;
    // (a screen above and below: drawn before they scroll in)
    this.io = new IntersectionObserver((es) => this.seen(es), { root: scroller, rootMargin: "100% 0px", threshold: [0, 0.25, 0.5, 0.75, 1] });
    // (the page in view, as the view scrolls: once a frame)
    let frame = 0;
    scroller.addEventListener("scroll", () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          this.measure();
        });
    }, { passive: true });
    // (text selected on the pages: its boxes, for the editor to select the source)
    const pick = () =>
      setTimeout(() => {
        const rn = root.getRootNode() as ShadowRoot & { getSelection?: () => Selection | null };
        const sel = rn.getSelection?.() ?? document.getSelection();
        if (!sel || sel.isCollapsed || !sel.rangeCount) return;
        const out = new Map<number, [number, number, number, number][]>();
        for (let i = 0; i < sel.rangeCount; i++) {
          for (const r of sel.getRangeAt(i).getClientRects()) {
            for (const s of this.slots) {
              const b = s.el.getBoundingClientRect();
              if (r.bottom < b.top || r.top > b.bottom || r.right < b.left || r.left > b.right) continue;
              const k = Number(s.el.dataset.k), sx = this.size.w / b.width, sy = this.size.h / b.height;
              const list = out.get(k) ?? [];
              list.push([(r.left - b.left) * sx, (r.top - b.top) * sy, r.width * sx, r.height * sy]);
              out.set(k, list);
            }
          }
        }
        if (out.size) host.selected?.([...out].map(([k, rects]) => ({ k, rects })));
      }, 0);
    root.addEventListener("pointerup", pick);
    root.addEventListener("keyup", (e) => e.shiftKey && pick());
    // (a double-click: the source of what is under it, as Overleaf's PDF viewer does)
    root.addEventListener("dblclick", (e) => {
      const el = (e.target as Element).closest<HTMLElement>(".slot");
      if (!el) return;
      const r = el.getBoundingClientRect();
      const k = Number(el.dataset.k);
      host.dbl?.(k, ((e.clientX - r.left) / r.width) * this.size.w, ((e.clientY - r.top) / r.height) * this.size.h);
    });
    // (a click on a link: a place in the document scrolled to; a web
    // address given to the host, or opened in a new tab: http, https and
    // mailto only, a PDF's javascript: and the like never followed)
    root.addEventListener("click", (e) => {
      const hit = this.linkAt(e);
      if (!hit) return;
      e.preventDefault();
      const to = hit[4];
      if (typeof to === "string") {
        if (!/^(https?:|mailto:)/i.test(to)) return;
        if (host.link) host.link(to);
        else window.open(to, "_blank", "noopener");
        return;
      }
      this.goToPlace(to, hit[5] ?? null);
    });
    root.addEventListener("mousemove", (e) => {
      const el = (e.target as Element).closest<HTMLElement>(".slot");
      if (el) el.style.cursor = this.linkAt(e) ? "pointer" : "";
    });
  }

  /** The link under a pointer event (its page's draw list's `L`), if any. */
  private linkAt(e: MouseEvent): Link | null {
    const el = (e.target as Element).closest<HTMLElement>(".slot");
    if (!el) return null;
    const img = this.slots[Number(el.dataset.k)]?.img;
    if (!img || !("draws" in img)) return null;
    const d = img.draws as unknown as Draws2;
    if (!d.L?.length) return null;
    const r = el.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * d.w;
    const y = ((e.clientY - r.top) / r.height) * d.h;
    return d.L.find((l) => x >= l[0] && x <= l[2] && y >= l[1] && y <= l[3]) ?? null;
  }

  /** Scroll to page `k`, `top` points from its top (null: its top). */
  goToPlace(k: number, top: number | null): void {
    const s = this.slots[k];
    if (!s) return;
    const at = top == null ? 0 : (top / this.size.h) * s.el.offsetHeight;
    this.scroller.scrollTo({ top: s.el.offsetTop + at - 12, behavior: "auto" });
  }

  /**
   * The editor's selection on the pages: `marks` highlighted until the next
   * call ([] clears), scrolled to only if none of them is on screen.
   */
  marks(marks: { k: number; boxes: [number, number, number, number][] }[]): void {
    for (const m of this.root.querySelectorAll(".sel")) m.remove();
    const { w, h } = this.size;
    let first: HTMLElement | undefined, seen = false;
    const v = this.scroller.getBoundingClientRect();
    for (const { k, boxes } of marks) {
      const s = this.slots[k];
      if (!s) continue;
      if (s.drawn !== s.hash && !this.asked.has(`${k}:${s.hash}`)) {
        this.asked.add(`${k}:${s.hash}`);
        this.host.need(k);
      }
      for (const [x, y, bw, bh] of boxes) {
        const m = document.createElement("div");
        m.className = "mark sel";
        Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%`, width: `${(bw / w) * 100}%`, height: `${(bh / h) * 100}%` });
        s.el.append(m);
        first ??= m;
        const r = m.getBoundingClientRect();
        if (r.bottom > v.top && r.top < v.bottom) seen = true;
      }
    }
    if (first && !seen) this.scroller.scrollTo({ top: first.offsetTop + first.parentElement!.offsetTop - this.scroller.clientHeight / 3, behavior: "auto" });
  }

  /** Highlight `boxes` ([x, y top, w, h], PDF points) on page `k`, scrolled into view, for a moment. */
  mark(k: number, boxes: [number, number, number, number][], scroll = true): void {
    const s = this.slots[k];
    if (!s || !boxes.length) return;
    for (const m of this.root.querySelectorAll(".mark:not(.sel)")) if (m.parentElement !== s.el) m.remove();
    // (the page it jumps to, drawn now: not when the observer next sees it)
    if (s.drawn !== s.hash && !this.asked.has(`${k}:${s.hash}`)) {
      this.asked.add(`${k}:${s.hash}`);
      this.host.need(k);
    }
    // (the boxes already there move to the new place: the highlight glides
    // from word to word, as the cursor goes)
    const { w, h } = this.size;
    const have = [...s.el.querySelectorAll<HTMLElement>(".mark:not(.sel)")];
    boxes.forEach(([x, y, bw, bh], i) => {
      let m = have[i];
      if (!m) {
        m = document.createElement("div");
        m.className = "mark";
        s.el.append(m);
      }
      m.classList.remove("fade");
      Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%`, width: `${(bw / w) * 100}%`, height: `${(bh / h) * 100}%` });
    });
    for (const m of have.slice(boxes.length)) m.remove();
    // (the first box in the middle of the view, unless it is in view already)
    const top = s.el.offsetTop + (boxes[0][1] / h) * s.el.offsetHeight;
    const v = this.scroller;
    if (scroll && (top < v.scrollTop + 20 || top > v.scrollTop + v.clientHeight - 40)) v.scrollTo({ top: top - v.clientHeight / 2, behavior: "smooth" });
    // (it fades once the cursor rests: each new mark restarts the clock)
    clearTimeout(this.fadeTimer);
    this.fadeTimer = setTimeout(() => this.root.querySelectorAll(".mark:not(.sel)").forEach((m) => m.classList.add("fade")), 1800);
  }

  private fadeTimer: ReturnType<typeof setTimeout> | undefined;

  get pages(): number {
    return this.slots.length;
  }

  get page(): number {
    return this.current;
  }

  /** The pages there are now, by hash: slots added or removed, changed ones drawn again when seen. */
  layout(hashes: string[]): void {
    while (this.slots.length > hashes.length) {
      const s = this.slots.pop()!;
      this.io.unobserve(s.el);
      s.el.remove();
    }
    for (let k = 0; k < hashes.length; k++) {
      let s = this.slots[k];
      if (!s) {
        const el = document.createElement("div");
        el.className = "slot";
        el.dataset.k = String(k);
        el.setAttribute("aria-label", `Page ${k + 1}`);
        this.root.append(el);
        s = { el, hash: hashes[k], drawn: null, img: null };
        this.slots.push(s);
        this.sizeSlot(s);
        this.io.observe(el);
      }
      s.hash = hashes[k];
    }
    // (asked once a layout: an ask lost, say before the session was up, goes again)
    this.asked.clear();
    this.wantVisible();
  }

  /** Page `k` drawn: `img` at `hash` (the painted page of an edit, or one the viewer asked for). */
  set(k: number, img: PageImage, hash: string | null): void {
    const s = this.slots[k];
    if (!s) return;
    if (hash) this.asked.delete(`${k}:${hash}`);
    if ("draws" in img) this.size = { w: img.draws.w, h: img.draws.h };
    else if ("w" in img && img.w && img.h) this.size = { w: img.w, h: img.h };
    s.img = img;
    s.drawn = hash ?? s.hash;
    // (painted at the next frame, a few a frame, those on screen first: a
    // burst of pages never stalls a scroll)
    this.toPaint.add(s);
    if (!this.paintFrame) this.paintFrame = requestAnimationFrame(() => this.paintSome());
  }

  private toPaint = new Set<Slot>();
  private paintFrame = 0;
  private paintSome(): void {
    this.paintFrame = 0;
    const t0 = performance.now();
    const order = [...this.toPaint].sort((a, b) => Number(this.visible.has(Number(b.el.dataset.k))) - Number(this.visible.has(Number(a.el.dataset.k))));
    for (const s of order) {
      // (8 ms a frame at most, at least one page)
      if (performance.now() - t0 > 8) break;
      this.toPaint.delete(s);
      this.paint(s);
    }
    if (this.toPaint.size) this.paintFrame = requestAnimationFrame(() => this.paintSome());
  }

  /** The format changed: every page is to be drawn again (as it comes into view). */
  invalidate(): void {
    for (const s of this.slots) s.drawn = null;
    this.asked.clear();
    this.wantVisible();
  }

  /** The zoom changed: every drawn page again, at its new size. */
  redraw(): void {
    for (const s of this.slots) {
      this.sizeSlot(s);
      if (s.img) this.paint(s);
    }
  }

  /** Scroll page `k` into view, as Overleaf's page buttons do. */
  goTo(k: number): void {
    const s = this.slots[Math.max(0, Math.min(k, this.slots.length - 1))];
    if (!s) return;
    const top = s.el.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
    this.scroller.scrollTo({ top: top - 12, behavior: "auto" });
  }

  private sizeSlot(s: Slot): void {
    const [w, h] = this.host.box(this.size, this.size.w * (96 / 72) * this.host.scale());
    s.el.style.width = `${w}px`;
    s.el.style.height = `${h}px`;
  }

  private paint(s: Slot): void {
    if (!s.img) return;
    // (a highlight outlives the page drawn under it: put back after)
    const marks = [...s.el.querySelectorAll(".mark")];
    this.draw(s);
    for (const m of marks) if (!m.isConnected) s.el.append(m);
  }

  private draw(s: Slot): void {
    if (!s.img) return;
    const w = this.size.w * (96 / 72) * this.host.scale();
    this.sizeSlot(s);
    // (v2, from the PDF: updated by parts, not drawn again whole)
    if ("draws" in s.img && (s.img.draws as unknown as Draws2).v === 2) {
      const [bw, bh] = this.host.box(this.size, w);
      patch(s.el, s.img.draws as unknown as Draws2, bw, bh);
      return;
    }
    const markup = this.host.svg(s.img, w);
    if (markup !== null) {
      s.el.innerHTML = markup;
      return;
    }
    if ("canvas" in s.img) {
      // (drawn already: swapped in whole, so the page never blanks)
      s.img.canvas.style.width = "100%";
      s.img.canvas.style.height = "100%";
      if (s.el.firstChild !== s.img.canvas) s.el.replaceChildren(s.img.canvas);
      // (pdf.js's text layer, laid out at a CSS pixel per PDF point: scaled to the page)
      const tl = s.img.canvas.querySelector<HTMLElement>(".textLayer");
      if (tl) tl.style.transform = `scale(${s.el.getBoundingClientRect().width / s.img.w || parseFloat(s.el.style.width) / s.img.w})`;
      return;
    }
    if (!("png" in s.img)) return;
    const url = URL.createObjectURL(new Blob([s.img.png as BlobPart], { type: "image/png" }));
    const im = document.createElement("img");
    im.alt = `page ${Number(s.el.dataset.k) + 1}`;
    im.onload = () => URL.revokeObjectURL(url);
    im.src = url;
    s.el.replaceChildren(im);
  }

  /** Turn `n` pages (back if negative) from the page at the view's top, as a PDF viewer's Page Down / Page Up: back from inside a page goes to its own top first. */
  turn(n: number): void {
    const v = this.scroller.getBoundingClientRect();
    // (where each page's top is, in the scroller's coordinates: measured now)
    const top = (k: number) => this.slots[k].el.getBoundingClientRect().top - v.top + this.scroller.scrollTop;
    const at = this.scroller.scrollTop + 12;
    let lo = 0, hi = this.slots.length - 1;
    if (hi < 0) return;
    // (the last page whose top is at or above the view's top, with goTo's 12 px)
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (top(mid) <= at + 1) lo = mid;
      else hi = mid - 1;
    }
    this.goTo(n < 0 && top(lo) < at - 1 ? lo + n + 1 : lo + n);
  }

  private seen(es: IntersectionObserverEntry[]): void {
    for (const e of es) {
      const k = Number((e.target as HTMLElement).dataset.k);
      if (e.isIntersecting) this.visible.add(k);
      else this.visible.delete(k);
    }
    this.measure();
    this.wantVisible();
  }

  /** The page most on screen, measured now (the observer only reports crossings of its thresholds, in a root grown by its margin). */
  private measure(): void {
    const v = this.scroller.getBoundingClientRect();
    let best = this.current, most = 0;
    for (const k of this.visible) {
      const b = this.slots[k]?.el.getBoundingClientRect();
      if (!b) continue;
      const on = Math.min(b.bottom, v.bottom) - Math.max(b.top, v.top);
      if (on > most) {
        most = on;
        best = k;
      }
    }
    if (best !== this.current) {
      this.current = best;
      this.host.inView(best);
    }
  }

  /** Ask for each visible page not drawn at its hash (once per hash). */
  private wantVisible(): void {
    for (const k of this.visible) {
      const s = this.slots[k];
      if (!s || s.drawn === s.hash) continue;
      const key = `${k}:${s.hash}`;
      if (this.asked.has(key)) continue;
      this.asked.add(key);
      this.host.need(k);
    }
  }
}
