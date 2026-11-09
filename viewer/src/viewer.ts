// The pages, continuous: every page stacked in the scroll view, as Overleaf's
// viewer shows them. Only the pages near the view are drawn, and a
// page is drawn again only when its box changed (each page's hash, from the
// core). The page most in view is the one an edit paints first.

import type { Link, PageImage } from "./types.ts";
import { LAYER_LINES } from "./text.ts";

export interface ViewerHost {
  /**
   * Draw page `k` (the viewer wants it: in view, and not drawn at its hash
   * or at the zoom); a promise that settles once it is drawn (set) or not,
   * so the viewer asks for a page no more than once at a time.
   */
  need(k: number): void | Promise<void>;
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
          const v = scroller.getBoundingClientRect();
          // (a page zoomed in drawn sharp only around the part that was in
          // view, `data-win`: asked again once the view leaves that part)
          for (const [k, r] of boxes) {
            const s = this.slots[k];
            const win = s?.img?.canvas.dataset.win?.split(",").map(Number);
            const now = win && part(r, v);
            if (!now || (now[0] >= win[0] - 1e-3 && now[1] >= win[1] - 1e-3 && now[2] <= win[2] + 1e-3 && now[3] <= win[3] + 1e-3)) continue;
            if (this.asked.has(`${k}:${s.hash}`)) continue;
            s.drawn = null;
            this.ask(k);
          }
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
    // (the selection drawn as pdf.js draws it: one block a line, from its
    // first character's left edge to its last's right, the line's height,
    // word spaces and kerns included; the browser's own box a glyph made
    // it striped. What is copied is the text layer's, as before)
    let drawing = 0;
    root.ownerDocument.addEventListener("selectionchange", () => {
      if (drawing) return;
      drawing = requestAnimationFrame(() => {
        drawing = 0;
        this.drawSelection();
      });
    });
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
    // (over a link, the hand: a class on its page, which print.ts makes win
    // over the text layer's own text cursor; a link is nearly always text,
    // so a cursor set on the page alone never showed)
    root.addEventListener("mousemove", (e) => {
      const el = (e.target as Element).closest<HTMLElement>(".slot");
      if (el) el.classList.toggle("on-link", !!this.linkAt(e));
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
      if (s.drawn !== s.hash) this.ask(k);
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

  /** The text selected on the pages, as blocks a line (`.mark.seltext`), the old ones gone. */
  private drawSelection(): void {
    for (const m of this.root.querySelectorAll(".seltext")) m.remove();
    const rn = this.root.getRootNode() as ShadowRoot & { getSelection?: () => Selection | null };
    const sel = rn.getSelection?.() ?? this.root.ownerDocument.getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return;
    const { w, h } = this.size;
    for (let i = 0; i < sel.rangeCount; i++) {
      const r = sel.getRangeAt(i);
      for (const s of this.slots) {
        if (!r.intersectsNode(s.el)) continue;
        for (const t of s.el.querySelectorAll("text")) {
          const l = LAYER_LINES.get(t);
          if (!l || !l.text || !r.intersectsNode(t)) continue;
          const node = t.firstChild;
          const from = node && r.startContainer === node ? r.startOffset : 0;
          const to = node && r.endContainer === node ? r.endOffset : l.text.length;
          if (to <= from) continue;
          const x0 = l.xs?.[from] ?? l.x, x1 = l.rs?.[to - 1] ?? l.x + l.w;
          const m = document.createElement("div");
          m.className = "mark seltext";
          const top = l.y - l.size * 0.8, ht = l.size * 1.05;
          Object.assign(m.style, { left: `${(x0 / w) * 100}%`, top: `${(top / h) * 100}%`, width: `${((x1 - x0) / w) * 100}%`, height: `${(ht / h) * 100}%` });
          s.el.append(m);
        }
      }
    }
  }

  /** Esc: the highlights gone (the editor's selection and cursor's word, a double-click's place, find's matches) and the text selected on the pages; whether there were any. */
  unmark(): boolean {
    const ms = this.root.querySelectorAll(".mark");
    const sel = this.root.ownerDocument.getSelection();
    const had = ms.length > 0 || (!!sel && !sel.isCollapsed && this.root.contains(sel.anchorNode));
    for (const m of ms) m.remove();
    if (had) sel?.removeAllRanges();
    return had;
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
      // (its size the line's, as a part of the page's width: it zooms with the page, CSS's cqw on .slot)
      tag.style.setProperty("--fs", String((boxes[0][3] * 0.72) / w));
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
  /**
   * Where places on the pages are along the whole scroll, as fractions
   * (0 the top of the first page, 1 the end of the last): the scrollbar's
   * marks (panel.ts). `y` in PDF points from the page's top.
   */
  fractions(places: { k: number; y: number }[]): number[] {
    const total = this.scroller.scrollHeight || 1;
    return places.map(({ k, y }) => {
      const s = this.slots[k];
      if (!s) return -1;
      return (s.el.offsetTop + (y / this.size.h) * s.el.offsetHeight) / total;
    });
  }

  /** Scroll so page `k`'s place `y` (PDF points from its top) is a third down the view. */
  scrollToPlace(k: number, y: number): void {
    const s = this.slots[k];
    if (!s) return;
    this.scroller.scrollTo({ top: s.el.offsetTop + (y / this.size.h) * s.el.offsetHeight - this.scroller.clientHeight / 3 });
  }

  /** The culprits drawn last (a page painted later draws its bars again). */
  private culpritMarks: Parameters<Viewer["culprits"]>[0] = [];

  culprits(marks: { k: number; boxes: [number, number, number, number][]; severity: "error" | "warning"; i?: number; rule?: "overfull" | "underfull"; amount?: number }[]): void {
    this.culpritMarks = marks;
    for (const m of this.root.querySelectorAll(".culprit")) m.remove();
    const { w, h } = this.size;
    for (const { k, boxes, severity, i, rule, amount } of marks) {
      const s = this.slots[k];
      if (!s) continue;
      // (an overfull or underfull box: a bar at the end of its line, as
      // pdflatex's draft \overfullrule; drawn here only, never in the PDF)
      if (rule && boxes.length) {
        const [x, y, bw, bh] = boxes[0];
        const lines = [...s.el.querySelectorAll("text")]
          .map((t) => LAYER_LINES.get(t))
          .filter((l): l is NonNullable<typeof l> => !!l && !!l.text)
          .sort((a, b) => a.y - b.y);
        const right = (l: (typeof lines)[number]) => l.rs?.[l.rs.length - 1] ?? l.x + l.w;
        // (the problem's place is its paragraph's: its line, and the lines
        // after it as close as a paragraph's; an overfull one is the line
        // that ends `amount` past where the others end)
        let at = lines.findIndex((l) => l.y >= y && l.y - l.size <= y + bh);
        let line = at >= 0 ? lines[at] : undefined;
        if (line && amount) {
          const par = [line];
          for (let j = at + 1; j < lines.length && lines[j].y - lines[j - 1].y <= lines[j].size * 1.6; j++) par.push(lines[j]);
          const ends = par.map(right).sort((a, b) => a - b);
          const just = ends[ends.length >> 1];
          line = par.reduce((best, l) => (Math.abs(right(l) - just - amount) < Math.abs(right(best) - just - amount) ? l : best), line);
        }
        const end = line ? right(line) : x + bw;
        const top = line ? line.y - line.size * 0.75 : y;
        const ht = line ? line.size * 0.95 : bh;
        const m = document.createElement("div");
        m.className = `culprit rule ${rule}`;
        if (i !== undefined) m.dataset.i = String(i);
        Object.assign(m.style, { left: `${((end + 1.5) / w) * 100}%`, top: `${(top / h) * 100}%`, width: `${(5 / w) * 100}%`, height: `${(ht / h) * 100}%` });
        s.el.append(m);
        continue;
      }
      for (const [x, y, bw, bh] of boxes) {
        const m = document.createElement("div");
        m.className = `culprit ${severity}`;
        // (the caller's own index, the problem it is: the panel's hover says its message)
        if (i !== undefined) m.dataset.i = String(i);
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
    if (s.drawn !== s.hash) this.ask(k);
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
    if (s.drawn !== s.hash) this.ask(hit.k);
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
    const painted: Slot[] = [];
    for (const s of order) {
      // (8 ms a frame at most, at least one page)
      if (performance.now() - t0 > 8) break;
      this.toPaint.delete(s);
      this.paint(s);
      painted.push(s);
    }
    // (every page written first, then their boxes read, then snapped: a
    // read after each page's writes laid the view out again for each)
    const boxes = painted.map((s) => s.el.getBoundingClientRect());
    painted.forEach((s, i) => this.snap(s, boxes[i]));
    // (a box's bar placed by its line's text: drawn again once the page's text is there)
    if (this.culpritMarks.some((m) => m.rule && painted.some((s) => Number(s.el.dataset.k) === m.k))) this.culprits(this.culpritMarks);
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
    // (a page drawn at another zoom, coarser or finer, is drawn again at
    // this one: until then its old bitmap is stretched to its box. Finer
    // was kept before, a bitmap shrunk by the browser: its glyphs' edges
    // aliased, after every zoom out, for good)
    let other = false;
    for (const s of this.slots) if (s.img && !this.fits(s)) (s.drawn = null), (other = true);
    // (every drawn page's bitmap at its pixels again in its new box: one
    // stretched while its box was another size, say Letter's before the
    // first page said A4, stayed stretched by a pixel, its glyphs
    // resampled, until it was drawn again; boxes read first, then written)
    const drawn = this.slots.filter((s) => s.img);
    const boxes = drawn.map((s) => s.el.getBoundingClientRect());
    drawn.forEach((s, i) => this.snap(s, boxes[i]));
    if (other) {
      this.asked.clear();
      this.wantVisible();
    }
  }

  /** Device pixels a PDF point at the zoom now: what a page is drawn at. */
  private ppp(): number {
    return (96 / 72) * this.host.scale() * (window.devicePixelRatio || 1);
  }

  /**
   * Whether page `s` was drawn at the zoom now: its element's `data-scale`
   * (Strips.page's pixels a point), else its bitmap's width.
   */
  private fits(s: Slot): boolean {
    const el = s.img?.canvas;
    if (!el) return false;
    const want = this.ppp();
    const at = Number(el.dataset.scale);
    if (at) return Math.abs(at - want) <= want * 1e-4;
    const c = el.querySelector("canvas");
    return !c || Math.abs(c.width - this.size.w * want) <= 1;
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

  /**
   * The part of page `k` on screen: fractions of its width and height
   * (left, top, right, bottom); null if none of it is. What a zoomed-in
   * page is drawn sharp around (`Strips.page`'s `view`).
   */
  view(k: number): [number, number, number, number] | null {
    const s = this.slots[k];
    return s ? part(s.el.getBoundingClientRect(), this.scroller.getBoundingClientRect()) : null;
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

  /** The page's bitmap put in its slot (snapped by paintSome, once every page's writes are done). */
  private draw(s: Slot): void {
    if (!s.img) return;
    this.sizeSlot(s);
    // (drawn already: swapped in whole, so the page never blanks)
    const c = s.img.canvas;
    c.style.width = "100%";
    c.style.height = "100%";
    if (s.el.firstChild !== c) s.el.replaceChildren(c);
  }

  /**
   * A page drawn as a bitmap shown at its own pixels: its box exactly its
   * pixel count over the device ratio (the slot's box, rounded on its
   * own, can be a pixel off), so the browser copies the pixels and never
   * resamples them (resampled, the page looks blurry). Where on the
   * device's grid it lands is the browser's: Chrome and Firefox paint a
   * canvas snapped to whole device pixels. A transform to move it there
   * by the fraction its box's corner sits at was never snapped itself:
   * at a device ratio of 2 every glyph was blurred by half a pixel.
   * A page drawn in part (`data-win`, zoomed in past `Strips`' budget):
   * its box the whole page's pixels (`data-pw`, `data-ph`), so the part,
   * placed in fractions of it, is at its own pixels too.
   */
  private snap(s: Slot, box?: DOMRect): void {
    const wrap = s.img?.canvas;
    const c = wrap?.querySelector("canvas");
    if (!wrap || !c || !c.width || !c.height) return;
    const dpr = window.devicePixelRatio || 1;
    const r = box ?? s.el.getBoundingClientRect();
    const pw = Number(wrap.dataset.pw) || c.width, ph = Number(wrap.dataset.ph) || c.height;
    // (drawn at the zoom, `data-scale`: at its own pixels, its box the
    // slot's to a pixel by pageBox's rounding; drawn at another, or a
    // host's bitmap of no scale more than a rounding off its box: stretched)
    const own = wrap.dataset.scale ? this.fits(s) : Math.abs(pw - r.width * dpr) <= 1.5 && Math.abs(ph - r.height * dpr) <= 1.5;
    // (a bitmap of another size than its page's box, a zoom's until the page
    // is drawn again: stretched to the box, so the page and the marks on it,
    // placed in fractions of the box, stay together; never overflowing it)
    // (each written only when it changes: an unchanged write still costs
    // the next frame its layout)
    const set = (k: "width" | "height", v: string) => {
      if (wrap.style[k] !== v) wrap.style[k] = v;
    };
    if (!own) {
      set("width", "100%");
      set("height", "100%");
      return;
    }
    set("width", `${pw / dpr}px`);
    set("height", `${ph / dpr}px`);
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

  /**
   * Ask for each visible page not drawn at its hash (once per hash): those
   * on screen first, nearest the view's middle first; those just off it
   * (the observer's margin) once none on screen is on its way, so the
   * page in view is never queued behind them.
   */
  private wantVisible(): void {
    const want = [...this.visible].filter((k) => {
      const s = this.slots[k];
      return s && s.drawn !== s.hash && !this.asked.has(`${k}:${s.hash}`);
    });
    if (!want.length) return;
    const v = this.scroller.getBoundingClientRect();
    const mid = (v.top + v.bottom) / 2;
    const at = new Map(want.map((k) => {
      const b = this.slots[k].el.getBoundingClientRect();
      return [k, { on: b.bottom > v.top && b.top < v.bottom, far: Math.abs((b.top + b.bottom) / 2 - mid) }];
    }));
    want.sort((a, b) => at.get(a)!.far - at.get(b)!.far);
    for (const k of want) if (at.get(k)!.on) this.ask(k);
    if ([...this.flying.keys()].some((k) => this.onScreen(k))) return;
    for (const k of want) if (!at.get(k)!.on) this.ask(k);
  }

  /** Whether page `k`'s box is in the view. */
  private onScreen(k: number): boolean {
    const s = this.slots[k];
    if (!s) return false;
    const b = s.el.getBoundingClientRect(), v = this.scroller.getBoundingClientRect();
    return b.bottom > v.top && b.top < v.bottom;
  }

  /**
   * Pages asked for whose drawing has not come yet, each whether the
   * zoom changed meanwhile (it comes drawn at the zoom it was asked at).
   */
  private flying = new Map<number, { again: boolean }>();

  /**
   * Page `k` asked for at its hash, once at a time: asked again while its
   * drawing is on its way (a zoom, frame after frame of a pinch), it is
   * asked again once that drawing comes, at the zoom then. A zoom asked
   * a page as many times as it had frames, the core drew them all in turn,
   * and the drawing at the zoom it rested at came last.
   */
  private ask(k: number): void {
    const s = this.slots[k];
    if (!s) return;
    const key = `${k}:${s.hash}`;
    if (this.asked.has(key)) return;
    this.asked.add(key);
    const f = this.flying.get(k);
    if (f) return void (f.again = true);
    this.flying.set(k, { again: false });
    const done = () => {
      const g = this.flying.get(k);
      this.flying.delete(k);
      const t = this.slots[k];
      if (t && g?.again && (t.drawn !== t.hash || !this.fits(t))) {
        this.asked.delete(`${k}:${t.hash}`);
        t.drawn = null;
      }
      this.wantVisible();
    };
    const p = this.host.need(k);
    if (p) p.then(done, done);
    else queueMicrotask(done);
  }
}

/** The part of box `r` inside `v`: fractions of `r` (left, top, right, bottom); null if none. */
function part(r: DOMRect, v: DOMRect): [number, number, number, number] | null {
  if (!r.width || !r.height || r.right <= v.left || r.left >= v.right || r.bottom <= v.top || r.top >= v.bottom) return null;
  const fx = (x: number) => Math.min(1, Math.max(0, (x - r.left) / r.width));
  const fy = (y: number) => Math.min(1, Math.max(0, (y - r.top) / r.height));
  return [fx(v.left), fy(v.top), fx(v.right), fy(v.bottom)];
}
