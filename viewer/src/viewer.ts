// The pages, continuous: every page stacked in the scroll view, as Overleaf's
// viewer shows them. Only the pages near the view are drawn, and a
// page is drawn again only when its box changed (each page's hash, from the
// core). The page most in view is the one an edit paints first.

import type { Link, PageImage } from "./types.ts";

export interface ViewerHost {
  /** Draw page `k` (the viewer wants it: in view, and not drawn at its hash). */
  need(k: number): void;
  /** The page most in view changed. */
  inView(k: number): void;
  /** CSS pixels per PDF point at the current zoom (1 is 100%). */
  scale(): number;
  /** Text selected on the pages: its boxes by page, in PDF points from each page's top left. */
  selected?(sel: { k: number; rects: [number, number, number, number][] }[]): void;
  /** A double-click at (x, y) on page `k`, in PDF points from its top left. */
  dbl?(k: number, x: number, y: number): void;
  /** A click on a link to a web address (http, https or mailto): opened by the host (absent: in a new tab). */
  link?(uri: string): void;
  /** A page box's CSS size at `cssWidth` (on whole device pixels). */
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
  /** The last single click or selection on the pages (PDF points from its page's top left). */
  private picked: { k: number; x: number; y: number } | null = null;

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
          // (every box read first, then written: a write between reads
          // makes the browser lay the page out again at the next read, once
          // a page a frame)
          const boxes = this.measure();
          // (scrolled by whole CSS pixels, a fractional device ratio moves a
          // page off the device's grid: snapped again, those on screen)
          for (const [k, r] of boxes) if (this.slots[k]) this.snap(this.slots[k], r);
        });
    }, { passive: true });
    // (text selected on the pages: its boxes, for the editor to select the source)
    let told = "";
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
        if (!out.size) return;
        const sel2 = [...out].map(([k, rects]) => ({ k, rects }));
        const [x, y, w, h] = sel2[0].rects[0];
        this.picked = { k: sel2[0].k, x: x + w / 2, y: y + h / 2 };
        // (as the drag goes: told again only when it changed)
        const key = JSON.stringify(sel2);
        if (key === told) return;
        told = key;
        host.selected?.(sel2);
      }, 0);
    root.addEventListener("pointerup", pick);
    // (while a drag selects: told as it goes, at most once a frame, not only when the button is let go)
    let dragFrame = 0;
    root.addEventListener("pointerdown", () => (told = ""));
    root.addEventListener("pointermove", (e) => {
      if (!(e.buttons & 1) || dragFrame) return;
      dragFrame = requestAnimationFrame(() => {
        dragFrame = 0;
        pick();
      });
    });
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
      if (!hit) {
        // (a single click on a page: the place spot() gives; a drag's
        // selection sets it after, from what it selected)
        const el = (e.target as Element).closest<HTMLElement>(".slot");
        if (el && e.detail === 1) {
          const r = el.getBoundingClientRect();
          this.picked = { k: Number(el.dataset.k), x: ((e.clientX - r.left) / r.width) * this.size.w, y: ((e.clientY - r.top) / r.height) * this.size.h };
        }
        return;
      }
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

  /** The link under a pointer event (its page's, from MuPDF), if any. */
  private linkAt(e: MouseEvent): Link | null {
    const el = (e.target as Element).closest<HTMLElement>(".slot");
    if (!el) return null;
    const img = this.slots[Number(el.dataset.k)]?.img;
    if (!img?.links?.length) return null;
    const r = el.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * img.w;
    const y = ((e.clientY - r.top) / r.height) * img.h;
    return img.links.find((l) => x >= l[0] && x <= l[2] && y >= l[1] && y <= l[3]) ?? null;
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

  /**
   * Collaborators' cursors: each its word's `boxes` on page `k` tinted in
   * its `hue`, its `label` above the first box; the whole set, the last
   * one's replaced. Never scrolls, never asks for a page: a page not drawn
   * yet shows them when it is (paint keeps them).
   */
  others(marks: { k: number; boxes: [number, number, number, number][]; label: string; hue: number }[]): void {
    for (const m of this.root.querySelectorAll(".peer")) m.remove();
    const { w, h } = this.size;
    const at = (m: HTMLElement, x: number, y: number) => Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%` });
    for (const { k, boxes, label, hue } of marks) {
      const s = this.slots[k];
      if (!s || !boxes.length) continue;
      for (const [x, y, bw, bh] of boxes) {
        const m = document.createElement("div");
        m.className = "peer";
        m.style.setProperty("--hue", String(hue));
        at(m, x, y);
        Object.assign(m.style, { width: `${(bw / w) * 100}%`, height: `${(bh / h) * 100}%` });
        s.el.append(m);
      }
      const tag = document.createElement("div");
      tag.className = "peer peer-tag";
      tag.style.setProperty("--hue", String(hue));
      tag.textContent = label;
      at(tag, boxes[0][0], boxes[0][1]);
      s.el.append(tag);
    }
  }

  /**
   * Problems' culprits on the pages (the wavy underline): each box a
   * `.culprit` of its severity, in fractions of its page's box; the whole
   * set, the last one's replaced ([] clears). Never scrolls, never asks
   * for a page: a page not drawn yet shows them when it is (paint keeps them).
   */
  culprits(marks: { k: number; boxes: [number, number, number, number][]; severity: "error" | "warning" }[]): void {
    for (const m of this.root.querySelectorAll(".culprit")) m.remove();
    const { w, h } = this.size;
    for (const { k, boxes, severity } of marks) {
      const s = this.slots[k];
      if (!s) continue;
      for (const [x, y, bw, bh] of boxes) {
        const m = document.createElement("div");
        m.className = `culprit ${severity}`;
        Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%`, width: `${(bw / w) * 100}%`, height: `${(bh / h) * 100}%` });
        s.el.append(m);
      }
    }
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
    // (the first box in the middle of the view, unless it is in view already;
    // zoomed wider than the view, the highlight's end brought in from the side
    // it is off, so typing at a line's end keeps that end in view)
    const top = s.el.offsetTop + (boxes[0][1] / h) * s.el.offsetHeight;
    const v = this.scroller;
    if (scroll) {
      const to: ScrollToOptions = { behavior: "smooth" };
      if (top < v.scrollTop + 20 || top > v.scrollTop + v.clientHeight - 40) to.top = top - v.clientHeight / 2;
      if (v.scrollWidth > v.clientWidth) {
        const [lx, , lw] = boxes[boxes.length - 1];
        const end = s.el.offsetLeft + ((lx + lw) / w) * s.el.offsetWidth;
        const margin = 24;
        if (end + margin > v.scrollLeft + v.clientWidth || end - margin < v.scrollLeft) to.left = Math.max(0, end + margin - v.clientWidth);
      }
      if (to.top !== undefined || to.left !== undefined) v.scrollTo(to);
    }
    // (it fades once the cursor rests: each new mark restarts the clock)
    clearTimeout(this.fadeTimer);
    this.fadeTimer = setTimeout(() => this.root.querySelectorAll(".mark:not(.sel)").forEach((m) => m.classList.add("fade")), 1800);
  }

  private fadeTimer: ReturnType<typeof setTimeout> | undefined;

  /**
   * A search's matches marked on their pages until the next call ([]
   * clears), match `cur` outlined and brought to the middle of the view
   * unless it is in view.
   */
  found(hits: { k: number; boxes: [number, number, number, number][] }[], cur: number): void {
    for (const m of this.root.querySelectorAll(".fnd")) m.remove();
    const { w, h } = this.size;
    hits.forEach(({ k, boxes }, i) => {
      const s = this.slots[k];
      if (!s) return;
      for (const [x, y, bw, bh] of boxes) {
        const m = document.createElement("div");
        m.className = i === cur ? "mark fnd cur" : "mark fnd";
        Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%`, width: `${(bw / w) * 100}%`, height: `${(bh / h) * 100}%` });
        s.el.append(m);
      }
    });
    const hit = hits[cur];
    const s = hit && this.slots[hit.k];
    if (!s) return;
    if (s.drawn !== s.hash && !this.asked.has(`${hit.k}:${s.hash}`)) {
      this.asked.add(`${hit.k}:${s.hash}`);
      this.host.need(hit.k);
    }
    const [x, y] = hit.boxes[0];
    const top = s.el.offsetTop + (y / h) * s.el.offsetHeight;
    const left = s.el.offsetLeft + (x / w) * s.el.offsetWidth;
    const v = this.scroller;
    if (top < v.scrollTop + 20 || top > v.scrollTop + v.clientHeight - 40) v.scrollTo({ top: top - v.clientHeight / 2 });
    if (left < v.scrollLeft || left > v.scrollLeft + v.clientWidth - 40) v.scrollTo({ left: left - v.clientWidth / 3 });
  }

  /**
   * The place on the pages to find in the source (Overleaf's ← arrow), in
   * PDF points from its page's top left: the last single click or
   * selection while it is still in view, else the top of the view (the
   * first page in view, the middle of its part in view, just below the
   * view's top edge); null with no page in view.
   */
  spot(): { k: number; x: number; y: number } | null {
    const v = this.scroller.getBoundingClientRect();
    const p = this.picked;
    if (p && this.slots[p.k]) {
      const b = this.slots[p.k].el.getBoundingClientRect();
      const top = b.top + (p.y / this.size.h) * b.height;
      if (top >= v.top && top <= v.bottom) return { ...p };
    }
    for (const s of this.slots) {
      const b = s.el.getBoundingClientRect();
      if (b.bottom <= v.top || b.top >= v.bottom || !b.width || !b.height) continue;
      const mid = (Math.max(b.left, v.left) + Math.min(b.right, v.right)) / 2;
      // (12 pt below the edge: onto a line, not above the first)
      const y = ((Math.max(b.top, v.top) - b.top) / b.height) * this.size.h + 12;
      return { k: Number(s.el.dataset.k), x: ((mid - b.left) / b.width) * this.size.w, y: Math.min(y, this.size.h) };
    }
    return null;
  }

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
    for (let k = 0; k < hashes.length; k++) this.slotAt(k).hash = hashes[k];
    // (asked once a layout: an ask lost, say before the session was up, goes again)
    this.asked.clear();
    this.wantVisible();
  }

  /**
   * Page `k` is now at `hash`, the others as they are: a page shipped while
   * the build runs, before its layout (slots added up to it, none removed),
   * drawn when seen.
   */
  shipped(k: number, hash: string): void {
    this.slotAt(k).hash = hash;
    this.wantVisible();
  }

  /** Page `k`'s slot, made (with those before it) if there is none yet. */
  private slotAt(k: number): Slot {
    while (this.slots.length <= k) {
      const n = this.slots.length;
      const el = document.createElement("div");
      el.className = "slot";
      el.dataset.k = String(n);
      el.setAttribute("aria-label", `Page ${n + 1}`);
      this.root.append(el);
      const s: Slot = { el, hash: "", drawn: null, img: null };
      this.slots.push(s);
      this.sizeSlot(s);
      this.io.observe(el);
    }
    return this.slots[k];
  }

  /** Page `k` drawn: `img` at `hash` (the painted page of an edit, or one the viewer asked for). */
  set(k: number, img: PageImage, hash: string | null): void {
    const s = this.slots[k];
    if (!s) return;
    if (hash) this.asked.delete(`${k}:${hash}`);
    const resized = !!img.w && !!img.h && (img.w !== this.size.w || img.h !== this.size.h);
    if (img.w && img.h) this.size = { w: img.w, h: img.h };
    s.img = img;
    // (the first page's size known: the pages at it, drawn again if coarser)
    if (resized) queueMicrotask(() => this.redraw());
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

  /**
   * The zoom changed: each page at its new size, the view kept on the same
   * place of the page in view (a page's canvas fills its slot: it scales).
   */
  /**
   * The zoom changed: the pages' boxes at it (their bitmaps stretched),
   * then, unless `sizesOnly` (a pinch still going on), those drawn too
   * coarse drawn again.
   */
  redraw(sizesOnly = false): void {
    if (sizesOnly) {
      for (const s of this.slots) this.sizeSlot(s);
      for (const k of this.visible) if (this.slots[k]) this.snap(this.slots[k]);
      return;
    }
    const at = this.slots[this.current];
    const v = this.scroller.getBoundingClientRect();
    const before = at?.el.getBoundingClientRect();
    const frac = before?.height ? (v.top - before.top) / before.height : 0;
    for (const s of this.slots) this.sizeSlot(s);
    if (at) {
      const b = at.el.getBoundingClientRect();
      this.scroller.scrollTop += b.top + frac * b.height - v.top;
    }
    // (a page drawn coarser than its box now is drawn again at the new size;
    // until then its old bitmap scales)
    const want = Math.round(this.size.w * (96 / 72) * this.host.scale() * (window.devicePixelRatio || 1));
    let coarse = false;
    for (const s of this.slots) {
      const c = s.img?.canvas.querySelector("canvas");
      if (c && c.width < want - 1) (s.drawn = null), (coarse = true);
    }
    if (coarse) {
      this.asked.clear();
      this.wantVisible();
    }
  }

  /**
   * The place on the pages under (`x`, `y`) (client pixels): the page there,
   * or the nearest one above or below, and the fraction across it (outside
   * 0..1 in the gap beside it). For [`keepAt`], around a zoom.
   */
  pointAt(x: number, y: number): { k: number; fx: number; fy: number } | null {
    let best: { k: number; fx: number; fy: number } | null = null;
    let far = Infinity;
    for (const s of this.slots) {
      const r = s.el.getBoundingClientRect();
      const d = y < r.top ? r.top - y : y > r.bottom ? y - r.bottom : 0;
      if (d < far && r.width && r.height) {
        far = d;
        best = { k: Number(s.el.dataset.k), fx: (x - r.left) / r.width, fy: (y - r.top) / r.height };
      }
      if (d === 0) break;
    }
    return best;
  }

  /** Scrolled so the place `at` ([`pointAt`]'s) is under (`x`, `y`) again, the pages' sizes changed. */
  keepAt(at: { k: number; fx: number; fy: number }, x: number, y: number): void {
    const s = this.slots[at.k];
    if (!s) return;
    const r = s.el.getBoundingClientRect();
    this.scroller.scrollLeft += r.left + at.fx * r.width - x;
    this.scroller.scrollTop += r.top + at.fy * r.height - y;
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
    const marks = [...s.el.querySelectorAll(".mark, .peer, .culprit")];
    this.draw(s);
    for (const m of marks) if (!m.isConnected) s.el.append(m);
  }

  private draw(s: Slot): void {
    if (!s.img) return;
    this.sizeSlot(s);
    // (drawn already: swapped in whole, so the page never blanks)
    const c = s.img.canvas;
    c.style.width = "100%";
    c.style.height = "100%";
    if (s.el.firstChild !== c) s.el.replaceChildren(c);
    this.snap(s);
  }

  /**
   * A page drawn as a bitmap shown at its own pixels, on the device's grid:
   * its box exactly its pixel count over the device ratio (the slot's box,
   * rounded on its own, can be a pixel off) and moved by the fraction of a
   * device pixel the slot's corner sits at, so the browser copies the
   * pixels and never resamples them (resampled, the page looks blurry).
   */
  private snap(s: Slot, box?: DOMRect): void {
    const wrap = s.img?.canvas;
    const c = wrap?.querySelector("canvas");
    if (!wrap || !c || !c.width || !c.height) return;
    const dpr = window.devicePixelRatio || 1;
    const r = box ?? s.el.getBoundingClientRect();
    // (a bitmap of another size than its page's box, a zoom's until the page
    // is drawn again: stretched to the box, so the page and the marks on it,
    // placed in fractions of the box, stay together; never overflowing it)
    // (each written only when it changes: an unchanged write still costs
    // the next frame its layout)
    const set = (k: "width" | "height" | "transform", v: string) => {
      if (wrap.style[k] !== v) wrap.style[k] = v;
    };
    if (Math.abs(c.width - r.width * dpr) > 1.5 || Math.abs(c.height - r.height * dpr) > 1.5) {
      set("width", "100%");
      set("height", "100%");
      set("transform", "");
      return;
    }
    set("width", `${c.width / dpr}px`);
    set("height", `${c.height / dpr}px`);
    const fx = r.left * dpr - Math.round(r.left * dpr);
    const fy = r.top * dpr - Math.round(r.top * dpr);
    set("transform", fx || fy ? `translate(${-fx / dpr}px, ${-fy / dpr}px)` : "");
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

  /**
   * The page most on screen, measured now (the observer only reports
   * crossings of its thresholds, in a root grown by its margin); the boxes
   * of the pages on screen, read in one go, for the caller to write after.
   */
  private measure(): Map<number, DOMRect> {
    const v = this.scroller.getBoundingClientRect();
    let best = this.current, most = 0;
    const onScreen = new Map<number, DOMRect>();
    for (const k of this.visible) {
      const b = this.slots[k]?.el.getBoundingClientRect();
      if (!b) continue;
      const on = Math.min(b.bottom, v.bottom) - Math.max(b.top, v.top);
      if (on > 0) onScreen.set(k, b);
      if (on > most) {
        most = on;
        best = k;
      }
    }
    if (best !== this.current) {
      this.current = best;
      this.host.inView(best);
    }
    return onScreen;
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
