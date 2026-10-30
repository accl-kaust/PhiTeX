// phitex's live viewer: viewer/src and crates/cli/viewer/cli.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)

// viewer/src/engines.ts
var ENGINES = {
  pdflatex: { label: "pdfLaTeX", ready: true },
  xelatex: { label: "XeLaTeX", ready: true },
  lualatex: { label: "LuaLaTeX", ready: false }
};
function errorNeeds(message) {
  if (/LuaTeX is required|requires LuaTeX/i.test(message)) return "lualatex";
  if (/requires either XeTeX or|XeTeX or LuaTeX|(fontspec|unicode-math|polyglossia)\b.*(XeTeX|LuaTeX)/i.test(message)) return "xelatex";
  return null;
}

// viewer/src/viewer.ts
var LETTER = { w: 612, h: 792 };
var Viewer = class {
  slots = [];
  size = LETTER;
  visible = /* @__PURE__ */ new Set();
  current = 0;
  io;
  asked = /* @__PURE__ */ new Set();
  root;
  scroller;
  host;
  /** The last single click or selection on the pages (PDF points from its page's top left). */
  picked = null;
  constructor(root, scroller, host2) {
    this.root = root;
    this.scroller = scroller;
    this.host = host2;
    this.io = new IntersectionObserver((es) => this.seen(es), { root: scroller, rootMargin: "100% 0px", threshold: [0, 0.25, 0.5, 0.75, 1] });
    let frame = 0;
    scroller.addEventListener("scroll", () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          const boxes2 = this.measure();
          for (const [k, r] of boxes2) if (this.slots[k]) this.snap(this.slots[k], r);
        });
    }, { passive: true });
    let told = "";
    const pick2 = () => setTimeout(() => {
      const rn = root.getRootNode();
      const sel = rn.getSelection?.() ?? document.getSelection();
      if (!sel || sel.isCollapsed || !sel.rangeCount) return;
      const out = /* @__PURE__ */ new Map();
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
      const key = JSON.stringify(sel2);
      if (key === told) return;
      told = key;
      host2.selected?.(sel2);
    }, 0);
    root.addEventListener("pointerup", pick2);
    let dragFrame = 0;
    root.addEventListener("pointerdown", () => told = "");
    root.addEventListener("pointermove", (e) => {
      if (!(e.buttons & 1) || dragFrame) return;
      dragFrame = requestAnimationFrame(() => {
        dragFrame = 0;
        pick2();
      });
    });
    root.addEventListener("keyup", (e) => e.shiftKey && pick2());
    root.addEventListener("dblclick", (e) => {
      const el = e.target.closest(".slot");
      if (!el) return;
      const r = el.getBoundingClientRect();
      const k = Number(el.dataset.k);
      host2.dbl?.(k, (e.clientX - r.left) / r.width * this.size.w, (e.clientY - r.top) / r.height * this.size.h);
    });
    root.addEventListener("click", (e) => {
      const hit = this.linkAt(e);
      if (!hit) {
        const el = e.target.closest(".slot");
        if (el && e.detail === 1) {
          const r = el.getBoundingClientRect();
          this.picked = { k: Number(el.dataset.k), x: (e.clientX - r.left) / r.width * this.size.w, y: (e.clientY - r.top) / r.height * this.size.h };
        }
        return;
      }
      e.preventDefault();
      const to = hit[4];
      if (typeof to === "string") {
        if (!/^(https?:|mailto:)/i.test(to)) return;
        if (host2.link) host2.link(to);
        else window.open(to, "_blank", "noopener");
        return;
      }
      this.goToPlace(to, hit[5] ?? null);
    });
    root.addEventListener("mousemove", (e) => {
      const el = e.target.closest(".slot");
      if (el) el.style.cursor = this.linkAt(e) ? "pointer" : "";
    });
  }
  /** The link under a pointer event (its page's, from MuPDF), if any. */
  linkAt(e) {
    const el = e.target.closest(".slot");
    if (!el) return null;
    const img = this.slots[Number(el.dataset.k)]?.img;
    if (!img?.links?.length) return null;
    const r = el.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width * img.w;
    const y = (e.clientY - r.top) / r.height * img.h;
    return img.links.find((l) => x >= l[0] && x <= l[2] && y >= l[1] && y <= l[3]) ?? null;
  }
  /** Scroll to page `k`, `top` points from its top (null: its top). */
  goToPlace(k, top) {
    const s = this.slots[k];
    if (!s) return;
    const at = top == null ? 0 : top / this.size.h * s.el.offsetHeight;
    this.scroller.scrollTo({ top: s.el.offsetTop + at - 12, behavior: "auto" });
  }
  /**
   * The editor's selection on the pages: `marks` highlighted until the next
   * call ([] clears), scrolled to only if none of them is on screen.
   */
  marks(marks) {
    for (const m of this.root.querySelectorAll(".sel")) m.remove();
    const { w, h } = this.size;
    let first, seen = false;
    const v = this.scroller.getBoundingClientRect();
    for (const { k, boxes: boxes2 } of marks) {
      const s = this.slots[k];
      if (!s) continue;
      if (s.drawn !== s.hash && !this.asked.has(`${k}:${s.hash}`)) {
        this.asked.add(`${k}:${s.hash}`);
        this.host.need(k);
      }
      for (const [x, y, bw, bh] of boxes2) {
        const m = document.createElement("div");
        m.className = "mark sel";
        Object.assign(m.style, { left: `${x / w * 100}%`, top: `${y / h * 100}%`, width: `${bw / w * 100}%`, height: `${bh / h * 100}%` });
        s.el.append(m);
        first ??= m;
        const r = m.getBoundingClientRect();
        if (r.bottom > v.top && r.top < v.bottom) seen = true;
      }
    }
    if (first && !seen) this.scroller.scrollTo({ top: first.offsetTop + first.parentElement.offsetTop - this.scroller.clientHeight / 3, behavior: "auto" });
  }
  /**
   * Collaborators' cursors: each its word's `boxes` on page `k` tinted in
   * its `hue`, its `label` above the first box; the whole set, the last
   * one's replaced. Never scrolls, never asks for a page: a page not drawn
   * yet shows them when it is (paint keeps them).
   */
  others(marks) {
    for (const m of this.root.querySelectorAll(".peer")) m.remove();
    const { w, h } = this.size;
    const at = (m, x, y) => Object.assign(m.style, { left: `${x / w * 100}%`, top: `${y / h * 100}%` });
    for (const { k, boxes: boxes2, label, hue } of marks) {
      const s = this.slots[k];
      if (!s || !boxes2.length) continue;
      for (const [x, y, bw, bh] of boxes2) {
        const m = document.createElement("div");
        m.className = "peer";
        m.style.setProperty("--hue", String(hue));
        at(m, x, y);
        Object.assign(m.style, { width: `${bw / w * 100}%`, height: `${bh / h * 100}%` });
        s.el.append(m);
      }
      const tag = document.createElement("div");
      tag.className = "peer peer-tag";
      tag.style.setProperty("--hue", String(hue));
      tag.textContent = label;
      at(tag, boxes2[0][0], boxes2[0][1]);
      s.el.append(tag);
    }
  }
  /** Highlight `boxes` ([x, y top, w, h], PDF points) on page `k`, scrolled into view, for a moment. */
  mark(k, boxes2, scroll = true) {
    const s = this.slots[k];
    if (!s || !boxes2.length) return;
    for (const m of this.root.querySelectorAll(".mark:not(.sel)")) if (m.parentElement !== s.el) m.remove();
    if (s.drawn !== s.hash && !this.asked.has(`${k}:${s.hash}`)) {
      this.asked.add(`${k}:${s.hash}`);
      this.host.need(k);
    }
    const { w, h } = this.size;
    const have = [...s.el.querySelectorAll(".mark:not(.sel)")];
    boxes2.forEach(([x, y, bw, bh], i) => {
      let m = have[i];
      if (!m) {
        m = document.createElement("div");
        m.className = "mark";
        s.el.append(m);
      }
      m.classList.remove("fade");
      Object.assign(m.style, { left: `${x / w * 100}%`, top: `${y / h * 100}%`, width: `${bw / w * 100}%`, height: `${bh / h * 100}%` });
    });
    for (const m of have.slice(boxes2.length)) m.remove();
    const top = s.el.offsetTop + boxes2[0][1] / h * s.el.offsetHeight;
    const v = this.scroller;
    if (scroll) {
      const to = { behavior: "smooth" };
      if (top < v.scrollTop + 20 || top > v.scrollTop + v.clientHeight - 40) to.top = top - v.clientHeight / 2;
      if (v.scrollWidth > v.clientWidth) {
        const [lx, , lw] = boxes2[boxes2.length - 1];
        const end = s.el.offsetLeft + (lx + lw) / w * s.el.offsetWidth;
        const margin = 24;
        if (end + margin > v.scrollLeft + v.clientWidth || end - margin < v.scrollLeft) to.left = Math.max(0, end + margin - v.clientWidth);
      }
      if (to.top !== void 0 || to.left !== void 0) v.scrollTo(to);
    }
    clearTimeout(this.fadeTimer);
    this.fadeTimer = setTimeout(() => this.root.querySelectorAll(".mark:not(.sel)").forEach((m) => m.classList.add("fade")), 1800);
  }
  fadeTimer;
  /**
   * A search's matches marked on their pages until the next call ([]
   * clears), match `cur` outlined and brought to the middle of the view
   * unless it is in view.
   */
  found(hits, cur) {
    for (const m of this.root.querySelectorAll(".fnd")) m.remove();
    const { w, h } = this.size;
    hits.forEach(({ k, boxes: boxes2 }, i) => {
      const s2 = this.slots[k];
      if (!s2) return;
      for (const [x2, y2, bw, bh] of boxes2) {
        const m = document.createElement("div");
        m.className = i === cur ? "mark fnd cur" : "mark fnd";
        Object.assign(m.style, { left: `${x2 / w * 100}%`, top: `${y2 / h * 100}%`, width: `${bw / w * 100}%`, height: `${bh / h * 100}%` });
        s2.el.append(m);
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
    const top = s.el.offsetTop + y / h * s.el.offsetHeight;
    const left = s.el.offsetLeft + x / w * s.el.offsetWidth;
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
  spot() {
    const v = this.scroller.getBoundingClientRect();
    const p = this.picked;
    if (p && this.slots[p.k]) {
      const b = this.slots[p.k].el.getBoundingClientRect();
      const top = b.top + p.y / this.size.h * b.height;
      if (top >= v.top && top <= v.bottom) return { ...p };
    }
    for (const s of this.slots) {
      const b = s.el.getBoundingClientRect();
      if (b.bottom <= v.top || b.top >= v.bottom || !b.width || !b.height) continue;
      const mid = (Math.max(b.left, v.left) + Math.min(b.right, v.right)) / 2;
      const y = (Math.max(b.top, v.top) - b.top) / b.height * this.size.h + 12;
      return { k: Number(s.el.dataset.k), x: (mid - b.left) / b.width * this.size.w, y: Math.min(y, this.size.h) };
    }
    return null;
  }
  get pages() {
    return this.slots.length;
  }
  get page() {
    return this.current;
  }
  /** The pages there are now, by hash: slots added or removed, changed ones drawn again when seen. */
  layout(hashes) {
    while (this.slots.length > hashes.length) {
      const s = this.slots.pop();
      this.io.unobserve(s.el);
      s.el.remove();
    }
    for (let k = 0; k < hashes.length; k++) this.slotAt(k).hash = hashes[k];
    this.asked.clear();
    this.wantVisible();
  }
  /**
   * Page `k` is now at `hash`, the others as they are: a page shipped while
   * the build runs, before its layout (slots added up to it, none removed),
   * drawn when seen.
   */
  shipped(k, hash) {
    this.slotAt(k).hash = hash;
    this.wantVisible();
  }
  /** Page `k`'s slot, made (with those before it) if there is none yet. */
  slotAt(k) {
    while (this.slots.length <= k) {
      const n = this.slots.length;
      const el = document.createElement("div");
      el.className = "slot";
      el.dataset.k = String(n);
      el.setAttribute("aria-label", `Page ${n + 1}`);
      this.root.append(el);
      const s = { el, hash: "", drawn: null, img: null };
      this.slots.push(s);
      this.sizeSlot(s);
      this.io.observe(el);
    }
    return this.slots[k];
  }
  /** Page `k` drawn: `img` at `hash` (the painted page of an edit, or one the viewer asked for). */
  set(k, img, hash) {
    const s = this.slots[k];
    if (!s) return;
    if (hash) this.asked.delete(`${k}:${hash}`);
    const resized = !!img.w && !!img.h && (img.w !== this.size.w || img.h !== this.size.h);
    if (img.w && img.h) this.size = { w: img.w, h: img.h };
    s.img = img;
    if (resized) queueMicrotask(() => this.redraw());
    s.drawn = hash ?? s.hash;
    this.toPaint.add(s);
    if (!this.paintFrame) this.paintFrame = requestAnimationFrame(() => this.paintSome());
  }
  toPaint = /* @__PURE__ */ new Set();
  paintFrame = 0;
  paintSome() {
    this.paintFrame = 0;
    const t0 = performance.now();
    const order = [...this.toPaint].sort((a, b) => Number(this.visible.has(Number(b.el.dataset.k))) - Number(this.visible.has(Number(a.el.dataset.k))));
    for (const s of order) {
      if (performance.now() - t0 > 8) break;
      this.toPaint.delete(s);
      this.paint(s);
    }
    if (this.toPaint.size) this.paintFrame = requestAnimationFrame(() => this.paintSome());
  }
  /** The format changed: every page is to be drawn again (as it comes into view). */
  invalidate() {
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
  redraw(sizesOnly = false) {
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
    const want = Math.round(this.size.w * (96 / 72) * this.host.scale() * (window.devicePixelRatio || 1));
    let coarse = false;
    for (const s of this.slots) {
      const c = s.img?.canvas.querySelector("canvas");
      if (c && c.width < want - 1) s.drawn = null, coarse = true;
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
  pointAt(x, y) {
    let best = null;
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
  keepAt(at, x, y) {
    const s = this.slots[at.k];
    if (!s) return;
    const r = s.el.getBoundingClientRect();
    this.scroller.scrollLeft += r.left + at.fx * r.width - x;
    this.scroller.scrollTop += r.top + at.fy * r.height - y;
  }
  /** Scroll page `k` into view, as Overleaf's page buttons do. */
  goTo(k) {
    const s = this.slots[Math.max(0, Math.min(k, this.slots.length - 1))];
    if (!s) return;
    const top = s.el.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
    this.scroller.scrollTo({ top: top - 12, behavior: "auto" });
  }
  sizeSlot(s) {
    const [w, h] = this.host.box(this.size, this.size.w * (96 / 72) * this.host.scale());
    s.el.style.width = `${w}px`;
    s.el.style.height = `${h}px`;
  }
  paint(s) {
    if (!s.img) return;
    const marks = [...s.el.querySelectorAll(".mark, .peer")];
    this.draw(s);
    for (const m of marks) if (!m.isConnected) s.el.append(m);
  }
  draw(s) {
    if (!s.img) return;
    this.sizeSlot(s);
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
  snap(s, box) {
    const wrap = s.img?.canvas;
    const c = wrap?.querySelector("canvas");
    if (!wrap || !c || !c.width || !c.height) return;
    const dpr = window.devicePixelRatio || 1;
    const r = box ?? s.el.getBoundingClientRect();
    const set = (k, v) => {
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
  turn(n) {
    const v = this.scroller.getBoundingClientRect();
    const top = (k) => this.slots[k].el.getBoundingClientRect().top - v.top + this.scroller.scrollTop;
    const at = this.scroller.scrollTop + 12;
    let lo = 0, hi = this.slots.length - 1;
    if (hi < 0) return;
    while (lo < hi) {
      const mid = lo + hi + 1 >> 1;
      if (top(mid) <= at + 1) lo = mid;
      else hi = mid - 1;
    }
    this.goTo(n < 0 && top(lo) < at - 1 ? lo + n + 1 : lo + n);
  }
  seen(es) {
    for (const e of es) {
      const k = Number(e.target.dataset.k);
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
  measure() {
    const v = this.scroller.getBoundingClientRect();
    let best = this.current, most = 0;
    const onScreen = /* @__PURE__ */ new Map();
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
  wantVisible() {
    for (const k of this.visible) {
      const s = this.slots[k];
      if (!s || s.drawn === s.hash) continue;
      const key = `${k}:${s.hash}`;
      if (this.asked.has(key)) continue;
      this.asked.add(key);
      this.host.need(k);
    }
  }
};

// viewer/src/print.ts
var OVERLEAF = `
  --ph-bar: var(--toolbar-alt-bg-color, #2f3a4c); --ph-fg: var(--content-primary-dark, #f4f5f6); --ph-fg2: var(--neutral-40, #afb5c0);
  --ph-border: var(--neutral-70, #495365); --ph-hover: var(--toolbar-btn-hover-bg-color, #495365);
  --ph-stage: var(--pane, var(--pdf-bg, #2f3a4c)); --ph-menu: var(--bg-dark-primary, #1b222c);
  --ph-accent: var(--bg-accent-01, #098842); --ph-accent-hover: var(--bg-accent-02, #1e6b41); --ph-accent-fg: #fff;
  --ph-danger: var(--content-danger-dark, #e36d66); --ph-warning: var(--content-warning-dark, #de8014); --ph-info: var(--content-info-dark, #97b6e5);
  --ph-link: var(--link-ui-dark, #97b6e5); --ph-progress: var(--bg-accent-01, #098842);
  --ph-radius: var(--border-radius-base, 4px); --ph-pill: var(--border-radius-full, 9999px);
  --ph-font: var(--font-sans, "Noto Sans", system-ui, sans-serif); --ph-size: 13px; --ph-bar-h: 32px;
`;
var OVERLEAF_LIGHT = `
  --ph-bar: var(--toolbar-alt-bg-color, #fff); --ph-fg: var(--content-primary, #1b222c); --ph-fg2: var(--content-secondary, #495365);
  --ph-border: var(--border-divider, #e7e9ee); --ph-hover: var(--toolbar-btn-hover-bg-color, #f4f5f6);
  --ph-stage: var(--pane, var(--pdf-bg, #f4f5f6)); --ph-menu: var(--bg-light-primary, #fff);
  --ph-danger: var(--content-danger, #b83a33); --ph-warning: var(--content-warning, #8f5514); --ph-info: var(--content-info, #366cbf);
  --ph-link: var(--link-ui, #366cbf);
`;
var VSCODE = `
  --ph-bar: var(--vscode-editorGroupHeader-tabsBackground, #181818); --ph-fg: var(--vscode-foreground, #cccccc);
  --ph-fg2: var(--vscode-descriptionForeground, #9d9d9d); --ph-border: var(--vscode-panel-border, #2b2b2b);
  --ph-hover: var(--vscode-toolbar-hoverBackground, rgb(90 93 94 / 31%)); --ph-stage: var(--vscode-editor-background, #1f1f1f);
  --ph-menu: var(--vscode-menu-background, #1f1f1f); --ph-accent: var(--vscode-button-background, #0078d4);
  --ph-accent-hover: var(--vscode-button-hoverBackground, #026ec1); --ph-accent-fg: var(--vscode-button-foreground, #fff);
  --ph-danger: var(--vscode-errorForeground, #f85149); --ph-warning: var(--vscode-editorWarning-foreground, #cca700);
  --ph-info: var(--vscode-editorInfo-foreground, #3794ff); --ph-link: var(--vscode-textLink-foreground, #4daafc);
  --ph-progress: var(--vscode-progressBar-background, #0078d4);
  --ph-radius: 2px; --ph-pill: 2px; --ph-font: var(--vscode-font-family, system-ui, sans-serif); --ph-size: var(--vscode-font-size, 13px); --ph-bar-h: 35px;
`;
var WATCH = `
  --ph-bar: #ffffff; --ph-fg: #1b222c; --ph-fg2: #5d6778; --ph-border: #e3e6ec; --ph-hover: #eef1f6; --ph-stage: #eceef2;
  --ph-menu: #ffffff; --ph-accent: #1d3e72; --ph-accent-hover: #16325d; --ph-accent-fg: #fff;
  --ph-danger: #c23a2f; --ph-warning: #a3620a; --ph-info: #1d3e72; --ph-link: #1d3e72; --ph-progress: #f58f2d;
  --ph-radius: 6px; --ph-pill: 9999px; --ph-font: "Noto Sans", system-ui, sans-serif; --ph-size: 13px; --ph-bar-h: 40px;
`;
var WATCH_DARK = `
  --ph-bar: #161b23; --ph-fg: #e7e9ee; --ph-fg2: #98a1b0; --ph-border: #262d38; --ph-hover: #222935; --ph-stage: #0f1319;
  --ph-menu: #1b212b; --ph-accent: #2c5aa0; --ph-accent-hover: #3567b3; --ph-danger: #f0786d; --ph-warning: #e5a54b;
  --ph-info: #8fb3ec; --ph-link: #8fb3ec;
`;
var DARK_PAGE = "#2c2e33";
var DARK_PDF_FILTER = "invert(1) hue-rotate(180deg) contrast(.7) brightness(1.04)";
var PAGE_CSS = "";
var PRINT_CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
.win { ${OVERLEAF} }
.win.light { ${OVERLEAF_LIGHT} }
.win.vscode { ${VSCODE} }
.win.watch { ${WATCH} }
@media (prefers-color-scheme: dark) { .win.watch:not(.light) { ${WATCH_DARK} color-scheme: dark; } }
.win.watch.dark { ${WATCH_DARK} color-scheme: dark; }

.win { position: fixed; z-index: 2147483000; display: flex; flex-direction: column; container-type: inline-size;
  min-width: 300px; min-height: 180px; max-width: calc(100vw - 16px); max-height: calc(100vh - 16px); resize: both; overflow: hidden;
  background: var(--ph-stage); color: var(--ph-fg); font: var(--ph-size)/1.4 var(--ph-font);
  border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) * 2); box-shadow: 0 8px 28px rgb(0 0 0 / 30%); }
.win button { font: inherit; color: inherit; }
.win :focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.icon { display: block; flex: none; }
.grow { flex: 1; min-width: 4px; }
[hidden] { display: none !important; }

/* the floating window's handle (docked: none) */
header { display: flex; align-items: center; gap: 6px; height: 30px; padding: 0 4px 0 12px; flex: none; cursor: grab; user-select: none;
  background: var(--ph-bar); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
header:active { cursor: grabbing; }
.win.docked > header { display: none; }

/* the bar: Rebuild and the status on the left; find, compare, download, the page, zoom, problems, the menu on the right */
.bar { position: relative; flex: none; height: var(--ph-bar-h); display: flex; align-items: center; gap: 4px; padding: 0 6px;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); color: var(--ph-fg); }
.win.docked:not(.own-header) > .bar { display: none; }
.ib { all: unset; box-sizing: border-box; height: 26px; min-width: 26px; padding: 0 5px; display: inline-flex; align-items: center; justify-content: center;
  gap: 4px; border-radius: var(--ph-radius); cursor: pointer; color: var(--ph-fg2); white-space: nowrap; }
.ib:hover, .ib[aria-expanded="true"] { background: var(--ph-hover); color: var(--ph-fg); }
.ib:disabled { opacity: .4; cursor: default; }
.ib:focus-visible { outline: 2px solid var(--ph-link); }
.split { display: inline-flex; height: 24px; flex: none; border-radius: var(--ph-pill); overflow: hidden; background: var(--ph-accent); color: var(--ph-accent-fg); }
.split button { all: unset; display: inline-flex; align-items: center; gap: 5px; cursor: pointer; font-weight: 600; }
.split button:hover, .split button[aria-expanded="true"] { background: var(--ph-accent-hover); }
.split button:focus-visible { outline: 2px solid var(--ph-fg); outline-offset: -2px; }
.split .go { padding: 0 10px 0 8px; }
.split .more { padding: 0 4px; border-left: 1px solid rgb(255 255 255 / 25%); }
.split.busy .go .icon { animation: phx-spin 1s linear infinite; }
@keyframes phx-spin { to { transform: rotate(360deg); } }
.status { all: unset; box-sizing: border-box; display: inline-flex; align-items: center; gap: 6px; height: 26px; padding: 0 6px; min-width: 0;
  border-radius: var(--ph-radius); color: var(--ph-fg2); cursor: pointer; white-space: nowrap; overflow: hidden; font-variant-numeric: tabular-nums; }
.status:hover, .status[aria-expanded="true"] { background: var(--ph-hover); color: var(--ph-fg); }
.status b { font-weight: 400; color: var(--ph-fg); overflow: hidden; text-overflow: ellipsis; }
.status.err b { color: var(--ph-danger); }
.pageno { display: inline-flex; align-items: center; gap: 4px; color: var(--ph-fg2); font-variant-numeric: tabular-nums; white-space: nowrap; }
.pageno input { width: 2.8em; height: 22px; padding: 0; text-align: center; font: inherit; color: var(--ph-fg); background: transparent;
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); }
.zoomb { padding: 0 2px 0 8px; }
.probs { font-weight: 600; font-variant-numeric: tabular-nums; }
.probs.error { color: var(--ph-danger); } .probs.warning { color: var(--ph-warning); }
.progress { position: absolute; left: 0; right: 0; bottom: -1px; height: 2px; overflow: hidden; opacity: 0; transition: opacity .2s; pointer-events: none; }
.progress.on { opacity: 1; }
.progress::after { content: ""; position: absolute; top: 0; bottom: 0; width: 30%; background: var(--ph-progress); animation: phx-carriage 1.2s ease-in-out infinite; }
@keyframes phx-carriage { from { left: -30%; } to { left: 100%; } }
.find { display: inline-flex; align-items: center; gap: 6px; }
.find input { width: 170px; height: 24px; padding: 0 8px; font: inherit; color: var(--ph-fg); background: var(--ph-menu);
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); outline: none; }
.find input:focus { border-color: var(--ph-link); }
#findn { color: var(--ph-fg2); min-width: 48px; font-variant-numeric: tabular-nums; }
#findn.none { color: var(--ph-danger); }
@container (max-width: 560px) {
  .wide { display: none !important; }
  .find input { width: 110px; }
}
@container (min-width: 561px) { .narrow { display: none !important; } }

/* menus and popovers: plain lists in the host's menu colour */
.menu { position: absolute; z-index: 20; min-width: 230px; max-width: calc(100% - 12px); padding: 4px 0; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 4px 16px rgb(0 0 0 / 22%); }
.menu > button, .menu .mi { all: unset; box-sizing: border-box; display: flex; align-items: center; gap: 10px; width: 100%; padding: 6px 14px; cursor: pointer; }
.menu > button:hover, .menu > button:focus-visible { background: var(--ph-hover); }
.menu .mi { cursor: default; }
.menu .sub { margin-left: auto; padding-left: 12px; color: var(--ph-fg2); font-size: 12px; font-variant-numeric: tabular-nums; }
.menu .desc { display: block; color: var(--ph-fg2); font-size: 12px; }
.menu hr { border: 0; border-top: 1px solid var(--ph-border); margin: 4px 0; }
.menu .check { width: 18px; flex: none; display: inline-flex; justify-content: center; color: var(--ph-link); }
.radios { display: flex; gap: 4px; padding: 2px 14px 6px 42px; }
.radios button { all: unset; padding: 2px 10px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); font-size: 12px; cursor: pointer; }
.radios button[aria-pressed="true"] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.timing { width: 380px; padding: 10px 14px 12px; font-variant-numeric: tabular-nums; }
.timing h3 { margin: 0 0 8px; font-size: 13px; font-weight: 600; }
.timing table { width: 100%; border-collapse: collapse; }
.timing td { padding: 3px 0; vertical-align: top; }
.timing td:first-child { color: var(--ph-fg2); padding-right: 12px; }
.timing td:last-child { text-align: right; }
.timing .foot { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.timing .foot.err { color: var(--ph-danger); }

/* problems: Overleaf's log entries, in a popover under the bar */
.logs { right: 6px; top: 4px; width: min(460px, calc(100% - 12px)); max-height: calc(100% - 8px); overflow: auto; padding: 0; }
.logs > header { all: unset; display: flex; align-items: center; gap: 8px; padding: 8px 8px 8px 14px; position: sticky; top: 0; z-index: 1;
  background: var(--ph-menu); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
.diag { display: block; padding: 9px 14px 10px; border-left: 3px solid var(--sev, var(--ph-fg2)); border-bottom: 1px solid var(--ph-border); }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--ph-hover); }
.diag.error { --sev: var(--ph-danger); } .diag.warning { --sev: var(--ph-warning); } .diag.info { --sev: var(--ph-info); }
.diag .where { float: right; margin-left: 10px; color: var(--ph-link); font-size: 12px; }
.diag .text { font-weight: 600; overflow-wrap: anywhere; }
.diag .detail, .folded .detail { margin: 6px 0 0; padding: 0; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; color: var(--ph-fg2);
  white-space: pre-wrap; overflow-wrap: anywhere; max-height: 12em; overflow: auto; font-weight: 400; }
.folded { margin-top: 6px; font-size: 12px; font-weight: 400; }
.folded summary { cursor: pointer; color: var(--ph-fg2); }
.diags .phx-inline { margin: 0; padding: 0; list-style: none; }
.reportbtn { all: unset; margin: 8px 14px; color: var(--ph-link); cursor: pointer; }
.reportbtn:hover { text-decoration: underline; }

/* the diff against a version: a strip under the bar */
.diffbar { display: none; flex: none; align-items: center; gap: 4px; padding: 3px 6px 3px 12px; flex-wrap: wrap;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); font-variant-numeric: tabular-nums; }
.diffbar.on { display: flex; }
.diffbar .what { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.diffbar .what small { color: var(--ph-fg2); font-size: inherit; }
.diffbar .n { white-space: nowrap; padding: 0 4px; color: var(--ph-fg2); }
.diffbar .busy::before { content: ""; display: inline-block; width: 7px; height: 7px; margin-right: 6px; border-radius: 50%; background: var(--ph-progress);
  animation: phx-pulse .6s ease-in-out infinite alternate; }
@keyframes phx-pulse { from { opacity: .4; } }
.diffbar button { all: unset; cursor: pointer; padding: 2px 8px; border-radius: var(--ph-radius); color: var(--ph-fg2); white-space: nowrap; }
.diffbar button:hover:not(:disabled) { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar button:disabled { opacity: .4; cursor: default; }
.diffbar button.cur { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px; background: var(--ph-border); }
.diffset { display: none; left: 50%; transform: translateX(-50%); top: 4px; width: 320px; padding: 12px; font-size: 12px; }
.diffset.on { display: block; }
.diffset h4 { margin: 0 0 6px; font-size: 12px; font-weight: 600; color: var(--ph-fg2); }
.diffset h4:not(:first-child) { margin-top: 12px; }
.diffset .opts { display: flex; flex-wrap: wrap; gap: 4px; }
.diffset button { all: unset; cursor: pointer; padding: 3px 9px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); }
.diffset button:hover { background: var(--ph-hover); }
.diffset button[aria-pressed=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.diffset .pair { display: inline-flex; align-items: center; gap: 6px; }
.diffset .pair i { display: inline-block; width: 12px; height: 12px; border-radius: 50%; }
.diffset .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 6px; }
.diffset input[type=color] { width: 30px; height: 22px; padding: 0; border: 0; background: none; cursor: pointer; }
.diffset .sample { margin-top: 10px; padding: 6px 8px; border-radius: var(--ph-radius); background: #fff; color: #1b222c; font: 13px/1.4 Georgia, serif; }
.diffset .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 12px; color: var(--ph-fg2); }

/* the pages */
.body { position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column; }
.win.has-side .body { flex-direction: row; }
.side { display: none; flex: none; width: 240px; overflow: auto; background: var(--ph-bar); border-right: 1px solid var(--ph-border); color: var(--ph-fg); }
.win.side-on .side { display: block; }
.win.side-on.side-empty .side { display: none; }
@container (max-width: 560px) { .win.side-on .side { position: absolute; inset: 0 auto 0 0; z-index: 5; box-shadow: 4px 0 16px rgb(0 0 0 / 20%); } }
.stage { position: relative; flex: 1; min-height: 0; overflow: auto; background: var(--ph-stage); text-align: center; overscroll-behavior: contain; will-change: scroll-position; }
.win.docked .stage { overflow-y: scroll; }
.win.docked .viewer { min-height: 100%; }
.stage.fit { overflow-x: hidden; }
/* (the first page's sheet up: the stage holds still under it, its width kept, so nothing scrolls behind the sheet) */
.win.sheet-up .stage, .win.docked.sheet-up .stage { overflow: hidden; scrollbar-gutter: stable; }
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
.win.dark-pdf .slot, .win.docked.pdf-dark .slot { background: ${DARK_PAGE}; }
.win.dark-pdf .slot canvas, .win.docked.pdf-dark .slot canvas { filter: ${DARK_PDF_FILTER}; }
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: rgb(0 100 255 / 25%); }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.stage.offline .slot { opacity: .4; pointer-events: none; }
.slot .mark { position: absolute; pointer-events: none; border-radius: 2px; background: rgb(255 214 0 / 45%); mix-blend-mode: multiply;
  box-shadow: 0 0 0 1px rgb(232 172 0 / 80%); transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); box-shadow: none; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); box-shadow: 0 0 0 1px rgb(150 100 0 / 90%); }
.slot .peer { position: absolute; pointer-events: none; border-radius: 2px; background: hsl(var(--hue) 70% 50% / 22%); box-shadow: inset 0 -1.5px 0 hsl(var(--hue) 70% 45%); }
.slot .peer-tag { width: auto; height: auto; transform: translateY(-100%); padding: 0 5px; border-radius: 3px 3px 3px 0; background: hsl(var(--hue) 70% 42%);
  box-shadow: none; color: #fff; font: 600 11px/16px var(--ph-font); white-space: nowrap; z-index: 2; }

/* strips over the pages: plain, in the host's colours */
.strip { position: sticky; top: 0; z-index: 2; display: none; padding: 6px 12px; text-align: left; font-size: 12px;
  background: var(--ph-menu); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-warning)); }
.stage.stale .banner, .approx.on { display: block; }
.gone { --sev: var(--ph-danger); font-weight: 600; }
.stage.offline .gone { display: flex; align-items: baseline; gap: 8px; }
.gone .x { margin-left: auto; flex: none; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; padding: 0 2px; }
.offpill { display: none; position: absolute; top: 8px; right: 14px; z-index: 3; padding: 1px 8px; border-radius: var(--ph-pill); font-size: 12px;
  background: var(--ph-menu); color: var(--ph-danger); border: 1px solid var(--ph-border); pointer-events: none; }
.win.offline-quiet .offpill { display: block; }

/* before any page: the first page's sheet, the opening playing on it, the step under it */
.opening { position: absolute; z-index: 3; pointer-events: none; left: 0; top: 0; width: 0; height: 0;
  transition: left .3s ease, top .3s ease, width .3s ease, height .3s ease; }
.opening .sheet { position: absolute; inset: 0; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); }
.opening.in .sheet { background: none; box-shadow: none; }
.win.dark-pdf .opening .sheet, .win.docked.pdf-dark .opening .sheet { background: ${DARK_PAGE}; }
.win.dark-pdf .opening.in .sheet, .win.docked.pdf-dark .opening.in .sheet { background: none; }
.opening .phx-open { position: absolute; inset: 0; width: 100%; height: 100%; }
.steps { position: absolute; left: 10%; right: 10%; bottom: 13%; text-align: center; color: #5d6778; font: 400 13px/1.4 var(--ph-font);
  font-variant-numeric: tabular-nums; transition: opacity .3s; }
.steps .what { color: #1b222c; }
.steps .detail { min-height: 1.4em; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.steps .track { position: relative; height: 3px; width: min(100%, 280px); margin: 8px auto; border-radius: 2px; overflow: hidden; background: #e7e9ee; }
.steps .track i { position: absolute; inset: 0 auto 0 0; border-radius: 2px; background: #f58f2d; transition: width .3s; }
.steps .track.unknown i { width: 30% !important; animation: phx-slide 1.2s ease-in-out infinite; }
@keyframes phx-slide { from { left: -30%; } to { left: 100%; } }
.win.dark-pdf .steps, .win.docked.pdf-dark .steps { color: #a3a9b4; }
.win.dark-pdf .steps .what, .win.docked.pdf-dark .steps .what { color: #e4e6ea; }
.win.dark-pdf .steps .track, .win.docked.pdf-dark .steps .track { background: #3d4047; }
.opening.in .steps { opacity: 0; }
/* a card in place of the pages (a build that stopped, an engine to choose): plain */
.empty { padding: 40px 16px; color: var(--ph-fg2); }
.card { max-width: 420px; margin: 0 auto; padding: 16px 18px; text-align: left; background: var(--ph-menu); color: var(--ph-fg);
  border: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-danger)); border-radius: var(--ph-radius); }
.card h3 { margin: 0 0 6px; font-size: 14px; font-weight: 600; }
.card .count { overflow-wrap: anywhere; }
.card .note { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; }
.card .btns { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 12px; }
.card .btn { all: unset; cursor: pointer; padding: 3px 12px; border-radius: var(--ph-pill); background: var(--ph-accent); color: var(--ph-accent-fg); font-weight: 600; }

/* a small notification in the corner: a package fetched, the rest of a build, a message */
.toast { position: absolute; right: 12px; bottom: 12px; z-index: 7; width: min(320px, calc(100% - 24px)); display: grid; gap: 4px; padding: 10px 12px;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 4px 16px rgb(0 0 0 / 22%);
  font-variant-numeric: tabular-nums; text-align: left; transition: opacity .3s, transform .3s; }
.toast.out { opacity: 0; transform: translateY(6px); }
.toast .t { display: flex; align-items: center; gap: 8px; font-weight: 600; min-width: 0; }
.toast .t span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.toast .d { color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.toast.err { border-left: 3px solid var(--ph-danger); }
.toast .track { position: relative; height: 3px; margin-top: 2px; border-radius: 2px; overflow: hidden; background: var(--ph-border); }
.toast .track i { position: absolute; inset: 0 auto 0 0; background: var(--ph-progress); transition: width .25s; }
.toast .track.unknown i { width: 30% !important; animation: phx-slide 1.2s ease-in-out infinite; }

/* feature tips: a popover with an arrow at the control it is about */
.tip { position: absolute; z-index: 30; width: 270px; max-width: calc(100% - 16px); padding: 12px 14px 10px; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: calc(var(--ph-radius) + 2px); box-shadow: 0 6px 20px rgb(0 0 0 / 25%); }
.tip::before { content: ""; position: absolute; top: -6px; left: var(--ax, 50%); width: 10px; height: 10px; margin-left: -5px; transform: rotate(45deg);
  background: var(--ph-menu); border-left: 1px solid var(--ph-border); border-top: 1px solid var(--ph-border); }
.tip .new { display: inline-block; margin-right: 6px; padding: 0 7px; border-radius: var(--ph-pill); font-size: 11px; font-weight: 600; vertical-align: 1px;
  background: var(--ph-accent); color: var(--ph-accent-fg); }
.tip h4 { margin: 0 0 4px; font-size: 13px; font-weight: 600; }
.tip p { margin: 0 0 10px; color: var(--ph-fg2); }
.tip .acts { display: flex; align-items: center; gap: 8px; }
.tip .acts .n { flex: 1; color: var(--ph-fg2); font-size: 12px; font-variant-numeric: tabular-nums; }
.tip .acts button { all: unset; cursor: pointer; padding: 3px 4px; color: var(--ph-link); }
.tip .acts .primary { padding: 3px 12px; border-radius: var(--ph-pill); background: var(--ph-accent); color: var(--ph-accent-fg); font-weight: 600; }
.ringed { box-shadow: 0 0 0 2px var(--ph-accent); }

/* dialogs over the pages: the keys, the log, the debug report, about */
.dialog { position: absolute; z-index: 25; inset: 12px; display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; text-align: left;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 24px rgb(0 0 0 / 30%); }
.dialog > header { all: unset; display: flex; align-items: center; gap: 8px; font-weight: 600; }
.dialog p { margin: 0; color: var(--ph-fg2); }
.dialog textarea, .dialog pre { flex: 1; min-height: 120px; margin: 0; overflow: auto; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre;
  color: var(--ph-fg); background: var(--ph-stage); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 8px; resize: none; }
.dialog .btns { display: flex; gap: 8px; justify-content: flex-end; }
.dialog .btn { all: unset; cursor: pointer; padding: 3px 12px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); }
.dialog .btn:hover { background: var(--ph-hover); }
.dialog.small { inset: auto 12px 12px auto; width: min(340px, calc(100% - 24px)); }
.keys { inset: 12px 12px auto auto; width: min(420px, calc(100% - 24px)); max-height: calc(100% - 24px); overflow: auto; }
.keys table { border-collapse: collapse; }
.keys td { padding: 2px 12px 2px 0; vertical-align: top; }
.keys kbd { font: 12px ui-monospace, monospace; padding: 0 5px; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); white-space: nowrap; }
.about a { color: var(--ph-link); }
.about .about-foot { margin-top: 8px; color: var(--ph-fg2); }
/* who made this, in Overleaf's pane (it looks native on purpose; it must never pass for Overleaf's) */
.byline { all: unset; display: none; position: absolute; left: 12px; bottom: 10px; z-index: 3; max-width: calc(100% - 24px); padding: 1px 8px;
  border-radius: var(--ph-pill); font-size: 11px; color: var(--ph-fg2); background: var(--ph-menu); border: 1px solid var(--ph-border);
  cursor: pointer; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.byline:hover { color: var(--ph-fg); }
.win.docked:not(.own-header) .byline { display: block; }
/* Overleaf's own settings, docked: the main file, the debug check, the latency's details */
.sheet-pop { right: 8px; top: 6px; width: min(340px, calc(100% - 16px)); padding: 12px; display: grid; gap: 8px; }
.sheet-pop label { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.sheet-pop select { font: inherit; color: var(--ph-fg); background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 2px 4px; max-width: 180px; }
.sheet-pop .links { display: flex; flex-wrap: wrap; gap: 12px; }
.sheet-pop .links button { all: unset; cursor: pointer; color: var(--ph-link); }
.details { margin: 0; font: 11px/1.4 ui-monospace, monospace; color: var(--ph-fg2); white-space: pre-wrap; }

/* docked: the panel fills the host's pane under its toolbar */
.win.docked { position: relative; left: auto !important; top: auto !important; width: 100% !important; height: 100% !important; max-width: none; max-height: none;
  min-width: 0; min-height: 0; resize: none; border: 0; border-radius: 0; box-shadow: none; }
.win.collapsed { resize: none; min-height: 0; min-width: 0; border-radius: var(--ph-pill); }
.win.collapsed > :not(header) { display: none; }
.win.collapsed header { border-bottom: 0; }
@media (prefers-reduced-motion: reduce) {
  .progress::after, .split.busy .go .icon, .steps .track.unknown i, .toast .track.unknown i, .diffbar .busy::before { animation: none; }
  .opening, .toast { transition: none; }
}
`;

// viewer/src/css.ts
var VIEWER_CSS = `
.slot { position: relative; contain: layout; background: #fff; }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
${PAGE_CSS}`;

// viewer/src/problems.ts
var esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
function place(file, line, col, text) {
  return `<a href="#" class="phx-place" data-file="${esc(file)}" data-line="${Number(line) || 1}" data-col="${Number(col) || 1}">${esc(text ?? `${file}${line ? `:${line}` : ""}${col ? `:${col}` : ""}`)}</a>`;
}
function problemHtml(p) {
  return item(p);
}
function asDiagnostic(p) {
  p = Object.assign({ notes: [], help: [], suggestions: [], file: null, line: null, col: null, excerpt: null, context: [], included: [], box: null }, p);
  const severity = p.severity === "fatal" || p.severity === "error" ? "error" : p.severity === "warning" ? "warning" : "info";
  return { severity, code: p.code, message: p.message, file: p.file ?? void 0, line: p.line ?? void 0, problem: p };
}
function item(p) {
  const sev = p.severity === "fatal" ? "error" : p.severity === "warning" || p.severity === "note" ? p.severity : "error";
  const where = p.file ? place(p.file, p.line, p.col) : "";
  let ex = "";
  if (p.excerpt) {
    const chars = [...p.excerpt.text];
    const a = chars.slice(0, p.excerpt.start).join("");
    const b = chars.slice(p.excerpt.start, p.excerpt.start + p.excerpt.len).join("");
    const c = chars.slice(p.excerpt.start + p.excerpt.len).join("");
    ex = `<pre class="phx-excerpt">${p.line ? `<span class="phx-ln">${Number(p.line)} \u2502 </span>` : ""}${esc(a)}<mark>${esc(b)}</mark>${esc(c)}</pre>`;
  }
  const ctx = p.context.length ? `<details class="phx-ctx"><summary>in ${p.context.map((m) => `<code>${esc(m.name)}</code>`).join(" \u2190 ")}</summary>${p.context.map((m) => `<pre><b>${esc(m.name)}</b> ${esc(m.before)}<mark>\u2038</mark>${esc(m.after)}</pre>`).join("")}</details>` : "";
  const inc = p.included.length ? `<div class="phx-inc">included from ${p.included.map((i) => place(i.file, i.line, 1)).join(", ")}</div>` : "";
  const box = p.box ? `<pre class="phx-excerpt">${esc(p.box.excerpt)}</pre>` : "";
  const list = (cls, xs) => xs.length ? `<ul class="${cls}">${xs.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>` : "";
  return `<li class="phx-${sev}"><div class="phx-msg"><span class="phx-sev">${esc(String(p.severity))}</span>${p.code ? `<code class="phx-code">${esc(p.code)}</code>` : ""}<span>${esc(p.message)}</span></div>${where ? `<div class="phx-where">${where}</div>` : ""}${ex}${box}${list("phx-notes", p.notes)}${ctx}${inc}${list("phx-help", [...p.help.length ? [p.help.join(" ").replace(/\s+/g, " ").trim()] : [], ...p.suggestions])}</li>`;
}
var PROBLEMS_CSS = `
.phx-problems { position: fixed; inset: 0; z-index: 10; background: rgb(0 0 0 / 45%); overflow: auto; padding: 48px 16px; }
.phx-card { max-width: 900px; margin: 0 auto; background: var(--ph-menu, #fff); color: var(--ph-fg, #1b222c); border-radius: var(--ph-radius, 4px);
  font: 13px/1.45 var(--ph-font, "Noto Sans", system-ui, sans-serif); }
.phx-card header { display: flex; align-items: baseline; gap: 10px; padding: 12px 16px 10px; border-bottom: 1px solid var(--ph-border, #e7e9ee); font-weight: 600; }
.phx-close { margin-left: auto; background: none; color: var(--ph-fg2, #495365); border: 1px solid var(--ph-border, #e7e9ee); border-radius: var(--ph-radius, 4px);
  padding: 1px 8px; font: inherit; font-size: 12px; cursor: pointer; }
.phx-card ol { list-style: none; margin: 0; padding: 0; }
.phx-card li { padding: 9px 14px 10px; border-bottom: 1px solid var(--ph-border, #e7e9ee); border-left: 3px solid var(--sev, var(--ph-fg2, #495365)); }
.phx-card li.phx-error { --sev: var(--ph-danger, #b83a33); }
.phx-card li.phx-warning { --sev: var(--ph-warning, #8f5514); }
.phx-card li.phx-note { --sev: var(--ph-info, #366cbf); }
.phx-msg { display: flex; flex-wrap: wrap; align-items: baseline; gap: 8px; font-weight: 600; }
.phx-sev { display: none; }
.phx-code { color: var(--ph-fg2, #495365); font: 400 12px ui-monospace, monospace; }
.phx-where { margin-top: 2px; font-size: 12px; }
.phx-place { color: var(--ph-link, #366cbf); text-decoration: none; }
.phx-place:hover { text-decoration: underline; }
.phx-excerpt, .phx-ctx pre { margin: 6px 0 0; padding: 0; overflow-x: auto; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre;
  color: var(--ph-fg2, #495365); background: none; }
.phx-excerpt mark, .phx-ctx mark { background: none; color: var(--sev, var(--ph-danger, #b83a33)); text-decoration: underline wavy; text-underline-offset: 3px; }
.phx-ln { opacity: .6; }
.phx-notes, .phx-help { margin: 6px 0 0; padding-left: 16px; color: var(--ph-fg2, #495365); }
.phx-ctx { margin-top: 6px; color: var(--ph-fg2, #495365); }
.phx-ctx summary { cursor: pointer; }
.phx-inc { margin-top: 6px; color: var(--ph-fg2, #495365); font-size: 12px; }
.phx-card footer { padding: 10px 16px; color: var(--ph-fg2, #495365); font-size: 12px; }
`;

// viewer/src/outline.ts
var esc2 = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
var Outline = class {
  el;
  /** The items in document order, flat, with their rows. */
  flat = [];
  marked = null;
  /** Folded parts, by their titles' path: kept when a build changes the outline. */
  folded = /* @__PURE__ */ new Set();
  place;
  constructor(root, place3) {
    this.place = place3;
    this.el = document.createElement("nav");
    this.el.className = "phx-outline";
    this.el.setAttribute("aria-label", "Contents");
    root.append(this.el);
    this.el.addEventListener("click", (ev) => {
      const t = ev.target;
      const li = t.closest("li");
      if (!li) return;
      ev.preventDefault();
      if (t.closest(".phx-fold")) {
        const key = li.dataset.path;
        if (li.classList.toggle("folded")) this.folded.add(key);
        else this.folded.delete(key);
        return;
      }
      const i = Number(li.dataset.i);
      const e = this.flat[i]?.e;
      if (e && e.p != null) this.place.goToPlace(e.p, e.y);
    });
  }
  get element() {
    return this.el;
  }
  /** A build's outline. */
  set(items) {
    this.flat = [];
    this.marked = null;
    if (!items.length) {
      this.el.innerHTML = `<p class="phx-none">No contents: the PDF has no outline (hyperref makes one from the sections).</p>`;
      return;
    }
    const list = (es, path, depth) => `<ul>${es.map((e) => {
      const i = this.flat.length;
      this.flat.push({ e, row: null });
      const p = `${path}/${e.t}`;
      const kids = e.k.length ? list(e.k, p, depth + 1) : "";
      const cls = [e.k.length ? "parent" : "", this.folded.has(p) ? "folded" : ""].filter(Boolean).join(" ");
      return `<li data-i="${i}" data-path="${esc2(p)}" class="${cls}"><div class="phx-row" style="--d:${depth}">${e.k.length ? `<span class="phx-fold" aria-hidden="true"></span>` : `<span class="phx-dot"></span>`}<a href="#" class="phx-t" title="${esc2(e.t)}">${esc2(e.t)}</a>${e.p != null ? `<span class="phx-p">${Number(e.p) + 1}</span>` : ""}</div>${kids}</li>`;
    }).join("")}</ul>`;
    this.el.innerHTML = list(items, "", 0);
    for (const li of this.el.querySelectorAll("li")) {
      const f = this.flat[Number(li.dataset.i)];
      if (f) f.row = li.querySelector(":scope > .phx-row");
    }
  }
  /** The page in view: the last item that starts on it or before is the one being read. */
  at(page) {
    let best = null;
    for (const { e, row } of this.flat) {
      if (e.p == null) continue;
      if (e.p > page) break;
      best = row;
    }
    if (best === this.marked) return;
    this.marked?.classList.remove("here");
    best?.classList.add("here");
    this.marked = best;
    if (best && best.offsetParent) {
      const r = best.getBoundingClientRect(), v = this.el.getBoundingClientRect();
      if (r.top < v.top || r.bottom > v.bottom) best.scrollIntoView({ block: "nearest" });
    }
  }
};
var OUTLINE_CSS = `
.phx-outline { overflow: auto; font: inherit; color: var(--ph-fg, #333); padding: 6px 0 24px; text-align: left; }
.phx-outline ul { list-style: none; margin: 0; padding: 0; }
.phx-outline li.folded > ul { display: none; }
.phx-row { display: flex; align-items: baseline; gap: 6px; padding: 3px 10px 3px calc(10px + var(--d) * 14px); border-radius: var(--ph-radius, 4px); margin: 0 6px; }
.phx-row:hover { background: var(--ph-hover, rgb(127 127 127 / 12%)); }
.phx-row.here { box-shadow: inset 2px 0 var(--ph-accent, #3e63dd); background: var(--ph-hover, rgb(62 99 221 / 14%)); }
.phx-row.here .phx-t { font-weight: 600; }
.phx-t { flex: 1; min-width: 0; color: inherit; text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
li.parent > .phx-row > .phx-t { font-weight: 500; }
.phx-p { color: var(--ph-fg2, #777); font-size: 12px; font-variant-numeric: tabular-nums; }
.phx-fold, .phx-dot { flex: none; width: 10px; text-align: center; color: var(--ph-fg2, #777); cursor: pointer; user-select: none; }
.phx-fold::before { content: "\u25BE"; }
li.folded > .phx-row .phx-fold::before { content: "\u25B8"; }
.phx-dot { cursor: default; }
.phx-none { color: var(--ph-fg2, #777); padding: 8px 16px; margin: 0; }
`;

// viewer/src/keys.ts
var KEYS = [
  ["j k  \u2193 \u2191", "scroll down, up"],
  ["h l", "scroll left, right"],
  ["Ctrl+d  Ctrl+u", "half a screen down, up"],
  ["Ctrl+b", "a screen up"],
  ["Ctrl+f  /", "find in the pages"],
  ["Enter  n / Shift+Enter  N", "next, previous match"],
  ["Space  Shift+Space", "next, previous page"],
  ["\u2192 \u2190  PgDn PgUp", "next, previous page"],
  ["gg  Home", "first page"],
  ["G  End", "last page"],
  ["12G  12gg", "page 12"],
  ["K J  + \u2212", "zoom in, out"],
  ["Ctrl+wheel  pinch", "zoom at the pointer"],
  ["Ctrl+ +  Ctrl+ \u2212  Ctrl+0", "zoom in, out, fit"],
  ["0  w", "fit the page's width"],
  ["f", "fit the whole page"],
  ["?", "these keys"],
  ["Esc", "close; forget a count"]
];
var LINE = 60;
function bindKeys(on, t) {
  let count = "";
  let g = false;
  let helping = false;
  const times = () => Math.max(1, Number(count) || 1);
  const page = (delta) => t.turn(delta);
  const by = (dx, dy) => t.scroller.scrollBy({ left: dx, top: dy, behavior: "auto" });
  const handler = (ev) => {
    const e = ev;
    const el = e.target;
    if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
    if (e.altKey || e.metaKey) return;
    const k = e.key;
    if (e.ctrlKey) {
      const h = t.scroller.clientHeight;
      const n2 = times();
      const act = {
        d: () => by(0, n2 * h / 2),
        u: () => by(0, -n2 * h / 2),
        b: () => by(0, -n2 * h)
      };
      if (!act[k]) return;
      e.preventDefault();
      act[k]();
      count = "";
      return;
    }
    if (/^[0-9]$/.test(k) && (count || k !== "0")) {
      count += k;
      e.preventDefault();
      return;
    }
    const n = times();
    const goToCount = (last) => t.goTo(count ? Math.min(t.pages, Number(count)) - 1 : last ? t.pages - 1 : 0);
    let done = true;
    if (g) {
      g = false;
      if (k === "g") goToCount(false);
      else done = false;
    } else
      switch (k) {
        case "j":
        case "ArrowDown":
          by(0, n * LINE);
          break;
        case "k":
        case "ArrowUp":
          by(0, -n * LINE);
          break;
        case "h":
          by(-n * LINE, 0);
          break;
        case "l":
          by(n * LINE, 0);
          break;
        case " ":
          page(e.shiftKey ? -n : n);
          break;
        case "ArrowRight":
        case "PageDown":
          page(n);
          break;
        case "ArrowLeft":
        case "PageUp":
          page(-n);
          break;
        case "Home":
          t.goTo(0);
          break;
        case "End":
          t.goTo(t.pages - 1);
          break;
        case "g":
          g = true;
          e.preventDefault();
          return;
        case "G":
          goToCount(true);
          break;
        case "K":
        case "+":
        case "=":
          t.zoom(1.1 ** n);
          break;
        case "J":
        case "-":
          t.zoom(1.1 ** -n);
          break;
        case "0":
        case "w":
          t.fitWidth();
          break;
        case "f":
          t.fitPage();
          break;
        case "?":
          helping = !helping;
          t.help?.(helping);
          break;
        case "Escape":
          if (helping) {
            helping = false;
            t.help?.(false);
          }
          break;
        default:
          done = false;
      }
    count = "";
    if (done) e.preventDefault();
  };
  on.addEventListener("keydown", handler);
  return () => on.removeEventListener("keydown", handler);
}

// viewer/src/report.ts
var REPORT_TO = "flinner@nand.sh";
function anonymizer(paths) {
  const pairs = [];
  paths.forEach((p, i) => {
    const ext = p.match(/\.[A-Za-z0-9]+$/)?.[0] ?? "";
    const alias = `file${i + 1}${ext}`;
    pairs.push([p, alias]);
    const base = p.replace(/^.*\//, "");
    if (base !== p) pairs.push([base, alias]);
    const stem = base.replace(/\.[A-Za-z0-9]+$/, "");
    if (stem.length >= 4 && stem !== base) pairs.push([stem, alias.replace(/\.[A-Za-z0-9]+$/, "")]);
  });
  pairs.sort((a, b) => b[0].length - a[0].length);
  const esc5 = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = pairs.length ? new RegExp(pairs.map(([p]) => `(?<![A-Za-z0-9_])${esc5(p)}(?![A-Za-z0-9_])`).join("|"), "g") : null;
  const map = new Map(pairs);
  return (s) => (re ? s.replace(re, (m) => map.get(m) ?? m) : s).replace(/`[^'\n]{1,80}'/g, "`\u2026'").replace(/\/home\/[^/\s]+\//g, "~/").replace(/\/Users\/[^/\s]+\//g, "~/");
}
var CONTENT = /* @__PURE__ */ new Set(["text", "files", "binaries", "expect", "detail", "glyphs", "g"]);
function clean(v, anon, depth = 0) {
  if (typeof v === "string") return anon(v.length > 400 ? v.slice(0, 400) + "\u2026" : v);
  if (typeof v !== "object" || v === null || depth > 4) return v;
  if (Array.isArray(v)) return v.slice(0, 40).map((x) => clean(x, anon, depth + 1));
  const out = {};
  for (const [k, x] of Object.entries(v)) {
    if (CONTENT.has(k)) out[k] = typeof x === "string" ? `(${x.length} chars, not included)` : "(not included)";
    else out[k] = clean(x, anon, depth + 1);
  }
  return out;
}
function report(r) {
  const anon = anonymizer(r.paths);
  const line = (e) => `${String(e.t).padStart(8)} ms  ${e.k.padEnd(16)} ${e.d === void 0 ? "" : JSON.stringify(clean(e.d, anon))}`;
  const exts = /* @__PURE__ */ new Map();
  for (const p of r.paths) {
    const x = p.match(/\.[A-Za-z0-9]+$/)?.[0] ?? "(none)";
    exts.set(x, (exts.get(x) ?? 0) + 1);
  }
  return [
    "PhiTeX Instant debug report (anonymized: no document text, no file names, no project id)",
    `extension ${r.version} \xB7 engine ${r.engine}`,
    `browser ${r.userAgent}`,
    `project: ${r.paths.length} files (${[...exts].map(([x, n]) => `${n} ${x}`).join(", ")})`,
    "",
    "errors:",
    ...r.errors.length ? r.errors.map((e) => "  " + anon(e)) : ["  (none)"],
    "",
    "status:",
    "  " + JSON.stringify(clean(r.status ?? {}, anon)),
    "",
    "diagnostics:",
    ...r.diagnostics.length ? r.diagnostics.slice(0, 40).map((d) => `  ${d.severity} ${d.code ?? ""}: ${anon(d.message)}${d.file ? ` (${anon(d.file)}${d.line ? `:${d.line}` : ""})` : ""}`) : ["  (none)"],
    "",
    "packages (TeX Live):",
    `  fetched: ${(r.packages?.done ?? []).join(", ") || "(none)"}`,
    `  failed: ${(r.packages?.failed ?? []).map((f) => `${f.name}${f.error ? ` (${anon(f.error)})` : ""}`).join(", ") || "(none)"}`,
    `  not in TeX Live: ${(r.packages?.unavailable ?? []).length}`,
    "",
    "trace (last 300 events):",
    ...r.trace.slice(-300).map(line)
  ].join("\n");
}

// viewer/src/opening-art.ts
var WORD = { "size": 95.52238805970148, "letters": [{ "ch": "\u03A6", "x": -7, "y": 85, "l": 0, "r": 71, "t": 20, "b": 85 }, { "ch": "T", "x": 55.514207299075906, "y": 64, "l": 59.514207299075906, "r": 128.5142072990759, "t": 0, "b": 64 }, { "ch": "E", "x": 116.0032076069846, "y": 85, "l": 119.0032076069846, "r": 186.0032076069846, "t": 20, "b": 85 }, { "ch": "X", "x": 176.1772889823344, "y": 64, "l": 179.1772889823344, "r": 256.1772889823344, "t": -1, "b": 65 }] };
var WOVEN = { "body": `<defs><clipPath id="\xA70" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA71" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA72" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA73" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA74" ><rect x="43.63" y="-870.22" width="35.34" height="1910.45" /></clipPath><clipPath id="\xA75" ><rect x="116" y="-870.22" width="57.31" height="1910.45" /></clipPath><clipPath id="\xA76" ><rect x="214.48" y="-891.22" width="47.67" height="1910.45" /></clipPath><mask id="\xA77" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA74)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g><g clip-path="url(#\xA75)"><text x="116" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">E</text></g><g clip-path="url(#\xA76)"><text x="176.18" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">X</text></g></mask><clipPath id="\xA78" ><rect x="20.7" y="44.88" width="22.93" height="32.48" /></clipPath><clipPath id="\xA79" ><rect x="79.39" y="-891.22" width="28.66" height="1910.45" /></clipPath><clipPath id="\xA710" ><rect x="176.18" y="-891.22" width="38.3" height="1910.45" /></clipPath><mask id="\xA711" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA78)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g><g clip-path="url(#\xA79)"><text x="55.51" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">T</text></g><g clip-path="url(#\xA710)"><text x="176.18" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">X</text></g></mask><clipPath id="\xA712" ><rect x="27.96" y="-10.52" width="51.01" height="85.97" /></clipPath><clipPath id="\xA713" ><rect x="116" y="-870.22" width="57.31" height="1910.45" /></clipPath><clipPath id="\xA714" ><rect x="214.48" y="-891.22" width="47.67" height="1910.45" /></clipPath><mask id="\xA715" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA712)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g><g clip-path="url(#\xA713)"><text x="116" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">E</text></g><g clip-path="url(#\xA714)"><text x="176.18" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">X</text></g></mask><clipPath id="\xA716" ><rect x="12.1" y="56.34" width="13.37" height="19.1" /></clipPath><clipPath id="\xA717" ><rect x="79.39" y="-891.22" width="28.66" height="1910.45" /></clipPath><clipPath id="\xA718" ><rect x="176.18" y="-891.22" width="85.97" height="1910.45" /></clipPath><mask id="\xA719" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA716)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g><g clip-path="url(#\xA717)"><text x="55.51" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">T</text></g><g clip-path="url(#\xA718)"><text x="176.18" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">X</text></g></mask><mask id="\xA720" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="43.63" y="-870.22" width="35.34" height="1910.45" fill="#000"/></mask><mask id="\xA721" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA720)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA722" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="20.7" y="44.88" width="22.93" height="32.48" fill="#000"/></mask><mask id="\xA723" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA722)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA724" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="27.96" y="-10.52" width="51.01" height="85.97" fill="#000"/></mask><mask id="\xA725" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA724)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA726" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="12.1" y="56.34" width="13.37" height="19.1" fill="#000"/></mask><mask id="\xA727" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA726)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask><mask id="\xA728" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></mask><mask id="\xA729" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="79.39" y="-891.22" width="28.66" height="1910.45" fill="#000"/></mask><mask id="\xA730" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA729)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA731" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></mask><mask id="\xA732" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="79.39" y="-891.22" width="28.66" height="1910.45" fill="#000"/></mask><mask id="\xA733" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA732)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask><mask id="\xA734" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="116" y="-870.22" width="57.31" height="1910.45" fill="#000"/></mask><mask id="\xA735" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA734)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA736" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></mask><mask id="\xA737" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="116" y="-870.22" width="57.31" height="1910.45" fill="#000"/></mask><mask id="\xA738" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA737)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA739" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></mask><mask id="\xA740" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="214.48" y="-891.22" width="47.67" height="1910.45" fill="#000"/></mask><mask id="\xA741" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA740)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA742" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="176.18" y="-891.22" width="38.3" height="1910.45" fill="#000"/></mask><mask id="\xA743" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA742)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA744" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="214.48" y="-891.22" width="47.67" height="1910.45" fill="#000"/></mask><mask id="\xA745" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA744)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA746" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="176.18" y="-891.22" width="85.97" height="1910.45" fill="#000"/></mask><mask id="\xA747" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA746)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask></defs><g mask="url(#\xA77)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#f58f2d" stroke-width="5.28"/></g><g mask="url(#\xA711)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#f0b225" stroke-width="4.07"/></g><g mask="url(#\xA715)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#d3c72a" stroke-width="2.86"/></g><g mask="url(#\xA719)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#00a8a4" stroke-width="1.65"/></g><g class="phx-l" data-ch="\u03A6"><g mask="url(#\xA727)"><g mask="url(#\xA725)"><g mask="url(#\xA723)"><g mask="url(#\xA721)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#FILL">\u03A6</text></g></g></g></g></g><g class="phx-l" data-ch="T"><g mask="url(#\xA733)"><g mask="url(#\xA731)"><g mask="url(#\xA730)"><g mask="url(#\xA728)"><text x="55.51" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#FILL">T</text></g></g></g></g></g><g class="phx-l" data-ch="E"><g mask="url(#\xA739)"><g mask="url(#\xA738)"><g mask="url(#\xA736)"><g mask="url(#\xA735)"><text x="116" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#FILL">E</text></g></g></g></g></g><g class="phx-l" data-ch="X"><g mask="url(#\xA747)"><g mask="url(#\xA745)"><g mask="url(#\xA743)"><g mask="url(#\xA741)"><text x="176.18" y="64" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#FILL">X</text></g></g></g></g></g>`, "x0": 0, "y0": -1, "x1": 262.1772889823344, "y1": 89 };
var MARK = { "body": `<defs><clipPath id="\xA70" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA71" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA72" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA73" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA74" ><rect x="43.63" y="-870.22" width="35.34" height="1910.45" /></clipPath><mask id="\xA75" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA74)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g></mask><clipPath id="\xA76" ><rect x="20.7" y="44.88" width="22.93" height="32.48" /></clipPath><mask id="\xA77" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA76)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g></mask><clipPath id="\xA78" ><rect x="27.96" y="-10.52" width="51.01" height="85.97" /></clipPath><mask id="\xA79" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA78)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g></mask><clipPath id="\xA710" ><rect x="12.1" y="56.34" width="13.37" height="19.1" /></clipPath><mask id="\xA711" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA710)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#000" stroke="#000" stroke-width="3.3" stroke-linejoin="miter" stroke-miterlimit="4">\u03A6</text></g></mask><mask id="\xA712" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="43.63" y="-870.22" width="35.34" height="1910.45" fill="#000"/></mask><mask id="\xA713" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA712)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA714" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="20.7" y="44.88" width="22.93" height="32.48" fill="#000"/></mask><mask id="\xA715" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA714)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA716" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="27.96" y="-10.52" width="51.01" height="85.97" fill="#000"/></mask><mask id="\xA717" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA716)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA718" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="12.1" y="56.34" width="13.37" height="19.1" fill="#000"/></mask><mask id="\xA719" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA718)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask></defs><g mask="url(#\xA75)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#f58f2d" stroke-width="5.28"/></g><g mask="url(#\xA77)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#f0b225" stroke-width="4.07"/></g><g mask="url(#\xA79)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#d3c72a" stroke-width="2.86"/></g><g mask="url(#\xA711)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#00a8a4" stroke-width="1.65"/></g><g class="phx-l" data-ch="\u03A6"><g mask="url(#\xA719)"><g mask="url(#\xA717)"><g mask="url(#\xA715)"><g mask="url(#\xA713)"><text x="-7" y="85" style="font: 700 95.52238805970148px 'CMU Serif'" fill="#FILL">\u03A6</text></g></g></g></g></g>`, "x0": 0, "y0": 20, "x1": 71, "y1": 89 };
var LOCKUP = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="3.000 2.300 555.496 94.984" role="img" aria-label="ACCL \u2013 Accelerated Connected Computing Lab" xmlns:c2pa="http://c2pa.org/manifest"><title>ACCL \u2013 Accelerated Connected Computing Lab</title><path fill="#1D3E72" fill-rule="evenodd" d="M97.00,3.00 L83.49,3.00 L34.66,70.21 L53.25,70.21 L81.96,30.70 L81.96,97.00 L97.00,97.00 Z"></path><path fill="#1D3E72" fill-rule="evenodd" d="M46.17,80.08 L33.88,97.00 L74.15,97.00 L79.61,89.48 L67.32,89.48 L79.61,72.56 L70.31,72.56 L58.02,89.48 L48.64,89.48 L55.47,80.08 L61.95,80.08 L67.41,72.56 L32.96,72.56 L15.20,97.00 L30.97,97.00 L36.44,89.48 L29.96,89.48 L36.79,80.08 Z"></path><path fill="#F58F2D" fill-rule="evenodd" d="M80.59,3.00 L71.29,3.00 L3.00,97.00 L12.30,97.00 Z"></path><path fill="#F0B225" fill-rule="evenodd" d="M68.39,3.00 L61.23,3.00 L3.00,83.14 L3.00,93.00 Z"></path><path fill="#D3C72A" fill-rule="evenodd" d="M58.32,3.00 L53.29,3.00 L3.00,72.21 L3.00,79.14 Z"></path><path fill="#00A8A4" fill-rule="evenodd" d="M50.38,3.00 L47.48,3.00 L3.00,64.22 L3.00,68.21 Z"></path><path fill="#1D3E72" d="M127.75767441860464 63.859534883720926Q117.8767441860465 63.859534883720926 113.19860465116278 61.10511627906976Q108.52046511627906 58.3506976744186 106.99023255813952 52.05488372093023Q105.46 45.75906976744186 105.46 33.08Q105.46 20.57581395348837 107.03395348837208 14.236279069767441Q108.60790697674418 7.8967441860465115 113.28604651162789 5.098604651162791Q117.96418604651161 2.300465116279071 127.75767441860464 2.300465116279071Q131.16790697674418 2.300465116279071 134.7967441860465 2.7376744186046515Q138.42558139534881 3.1748837209302323 140.78651162790698 3.8744186046511615V14.367441860465114Q133.35395348837207 13.580465116279072 130.90558139534883 13.580465116279072Q124.95953488372092 13.580465116279072 122.72976744186045 14.760930232558142Q120.49999999999999 15.941395348837212 119.88790697674418 19.570232558139534Q119.27581395348835 23.199069767441856 119.27581395348835 33.08Q119.27581395348835 42.96093023255814 119.88790697674418 46.58976744186047Q120.49999999999999 50.21860465116279 122.72976744186045 51.39906976744186Q124.95953488372092 52.579534883720925 130.90558139534883 52.579534883720925Q135.97720930232558 52.579534883720925 141.04883720930232 51.79255813953488V62.37302325581395Q138.60046511627905 62.985116279069764 134.84046511627906 63.42232558139534Q131.08046511627907 63.859534883720926 127.75767441860464 63.859534883720926Z M170.60418604651161 63.859534883720926Q160.72325581395347 63.859534883720926 156.04511627906976 61.10511627906976Q151.36697674418602 58.3506976744186 149.83674418604647 52.05488372093023Q148.30651162790696 45.75906976744186 148.30651162790696 33.08Q148.30651162790696 20.57581395348837 149.88046511627905 14.236279069767441Q151.45441860465115 7.8967441860465115 156.13255813953486 5.098604651162791Q160.81069767441858 2.300465116279071 170.60418604651161 2.300465116279071Q174.01441860465113 2.300465116279071 177.64325581395346 2.7376744186046515Q181.2720930232558 3.1748837209302323 183.63302325581392 3.8744186046511615V14.367441860465114Q176.20046511627905 13.580465116279072 173.75209302325578 13.580465116279072Q167.80604651162787 13.580465116279072 165.5762790697674 14.760930232558142Q163.34651162790695 15.941395348837212 162.73441860465113 19.570232558139534Q162.12232558139533 23.199069767441856 162.12232558139533 33.08Q162.12232558139533 42.96093023255814 162.73441860465113 46.58976744186047Q163.34651162790695 50.21860465116279 165.5762790697674 51.39906976744186Q167.80604651162787 52.579534883720925 173.75209302325578 52.579534883720925Q178.82372093023253 52.579534883720925 183.89534883720927 51.79255813953488V62.37302325581395Q181.44697674418603 62.985116279069764 177.68697674418604 63.42232558139534Q173.92697674418602 63.859534883720926 170.60418604651161 63.859534883720926Z M192.6395348837209 63.16V3.0H228.84046511627903V14.280000000000001H206.19302325581393V27.133953488372093H225.60511627906973V38.41395348837209H206.19302325581393V51.879999999999995H228.84046511627903V63.16Z M238.45906976744186 63.16V3.0H252.0125581395349V51.44279069767442H271.9493023255814V63.16Z M279.206976744186 63.16V3.0H315.4079069767442V14.280000000000001H292.7604651162791V27.133953488372093H312.1725581395349V38.41395348837209H292.7604651162791V51.879999999999995H315.4079069767442V63.16Z M354.3195348837209 63.16 347.3241860465116 42.08651162790697H338.58V63.16H325.0265116279069V3.0H349.77255813953485Q357.2051162790697 3.0 360.9651162790697 5.229767441860464Q364.7251162790697 7.459534883720927 365.9930232558139 11.569302325581393Q367.2609302325581 15.67906976744186 367.2609302325581 22.499534883720926Q367.2609302325581 28.882790697674416 365.86186046511625 33.38604651162791Q364.4627906976744 37.8893023255814 360.17813953488366 39.987906976744185L368.57255813953486 63.16ZM353.53255813953484 22.586976744186046Q353.53255813953484 18.652093023255816 353.0516279069767 17.03441860465116Q352.57069767441857 15.416744186046508 350.95302325581395 14.848372093023254Q349.33534883720927 14.280000000000001 345.48790697674417 14.280000000000001H338.58V30.89395348837209H345.400465116279Q349.24790697674416 30.89395348837209 350.86558139534884 30.281860465116278Q352.48325581395346 29.669767441860465 353.00790697674415 28.052093023255814Q353.53255813953484 26.434418604651164 353.53255813953484 22.586976744186046Z M409.2330232558139 63.16 406.17255813953483 51.09302325581395H388.4218604651162L385.5362790697674 63.16H371.89534883720927L387.9846511627906 3.0H407.046976744186L423.0488372093023 63.16ZM397.5158139534883 15.241860465116275H396.9911627906976L391.1325581395348 39.900465116279065H403.4618604651162Z M435.7279069767441 63.16V14.717209302325578H421.56232558139527V3.0H463.62186046511624V14.717209302325578H449.3688372093023V63.16Z M470.35488372093016 63.16V3.0H506.5558139534883V14.280000000000001H483.9083720930232V27.133953488372093H503.320465116279V38.41395348837209H483.9083720930232V51.879999999999995H506.5558139534883V63.16Z M516.1744186046511 3.0H535.0618604651163Q546.4293023255814 3.0 551.2823255813953 5.623255813953488Q556.1353488372092 8.246511627906976 557.3158139534883 14.01767441860465Q558.4962790697674 19.788837209302322 558.4962790697674 33.08Q558.4962790697674 46.37116279069767 557.3158139534883 52.14232558139534Q556.1353488372092 57.91348837209302 551.2823255813953 60.536744186046505Q546.4293023255814 63.16 535.0618604651163 63.16H516.1744186046511ZM534.7120930232558 51.879999999999995Q540.133488372093 51.879999999999995 541.9697674418604 51.005581395348834Q543.8060465116279 50.13116279069767 544.286976744186 46.67720930232558Q544.7679069767441 43.223255813953486 544.7679069767441 33.08Q544.7679069767441 22.93674418604651 544.2432558139535 19.482790697674417Q543.7186046511628 16.028837209302324 541.9260465116279 15.154418604651163Q540.133488372093 14.280000000000001 534.7120930232558 14.280000000000001H529.7279069767442V51.879999999999995Z"></path><path fill="#1D3E72" d="M114.30529069767441 97.28418604651162Q110.36220930232557 97.28418604651162 108.53276162790696 96.21848837209302Q106.70331395348836 95.15279069767442 106.08165697674417 92.59511627906977Q105.46 90.03744186046512 105.46 84.78Q105.46 79.52255813953488 106.08165697674417 76.98264534883721Q106.70331395348836 74.44273255813954 108.55052325581394 73.35927325581395Q110.39773255813952 72.27581395348837 114.30529069767441 72.27581395348837Q115.65517441860464 72.27581395348837 117.07610465116278 72.45343023255813Q118.49703488372091 72.6310465116279 119.3495930232558 72.87970930232558V76.50308139534883Q116.64982558139533 76.18337209302325 115.33546511627907 76.18337209302325Q112.67122093023255 76.18337209302325 111.6410465116279 76.69845930232557Q110.61087209302325 77.2135465116279 110.30892441860465 78.86537790697673Q110.00697674418603 80.51720930232558 110.00697674418603 84.78Q110.00697674418603 89.04279069767442 110.30892441860465 90.69462209302326Q110.61087209302325 92.34645348837209 111.6410465116279 92.86154069767443Q112.67122093023255 93.37662790697675 115.33546511627907 93.37662790697675Q117.39581395348836 93.37662790697675 119.45616279069766 93.05691860465116V96.71581395348838Q118.49703488372091 96.96447674418604 117.04058139534882 97.12433139534883Q115.58412790697673 97.28418604651162 114.30529069767441 97.28418604651162Z M124.0164297040169 84.78Q124.0164297040169 79.2028488372093 124.49599365750527 76.78726744186046Q124.97555761099363 74.37168604651163 126.82276691331921 73.32374999999999Q128.6699762156448 72.27581395348837 132.96829016913318 72.27581395348837Q137.26660412262154 72.27581395348837 139.1315750528541 73.32374999999999Q140.99654598308666 74.37168604651163 141.47610993657503 76.78726744186046Q141.9556738900634 79.2028488372093 141.9556738900634 84.78Q141.9556738900634 90.3571511627907 141.47610993657503 92.77273255813954Q140.99654598308666 95.18831395348838 139.1315750528541 96.23625Q137.26660412262154 97.28418604651162 132.96829016913318 97.28418604651162Q128.6699762156448 97.28418604651162 126.82276691331921 96.23625Q124.97555761099363 95.18831395348838 124.49599365750527 92.77273255813954Q124.0164297040169 90.3571511627907 124.0164297040169 84.78ZM137.40869714587737 84.78Q137.40869714587737 80.2685465116279 137.17779598308667 78.65223837209302Q136.94689482029597 77.03593023255814 136.11209830866807 76.6096511627907Q135.27730179704014 76.18337209302325 132.96829016913318 76.18337209302325Q130.6592785412262 76.18337209302325 129.84224365750526 76.6096511627907Q129.02520877378433 77.03593023255814 128.79430761099366 78.63447674418605Q128.56340644820295 80.23302325581395 128.56340644820295 84.78Q128.56340644820295 89.32697674418604 128.79430761099366 90.92552325581394Q129.02520877378433 92.52406976744186 129.84224365750526 92.9503488372093Q130.6592785412262 93.37662790697675 132.96829016913318 93.37662790697675Q135.27730179704014 93.37662790697675 136.11209830866807 92.9503488372093Q136.94689482029597 92.52406976744186 137.17779598308667 90.90776162790698Q137.40869714587737 89.2914534883721 137.40869714587737 84.78Z M160.58315010570823 97.0 152.1286152219873 81.54738372093023H151.98652219873148V97.0H147.6171617336152V72.56H152.23518498942917L160.37001057082452 87.2311046511628H160.51210359408032V72.56H164.88146405919662V97.0Z M184.11283562367862 97.0 175.6583007399577 81.54738372093023H175.51620771670187V97.0H171.1468472515856V72.56H175.76487050739956L183.8996960887949 87.2311046511628H184.0417891120507V72.56H188.411149577167V97.0Z M194.71205602536995 97.0V72.56H208.99240486257926V76.46755813953489H199.22350951374204V82.68412790697674H207.642521141649V86.62720930232558H199.22350951374204V93.09244186046512H208.99240486257926V97.0Z M222.6111020084566 97.28418604651162Q218.66802061310779 97.28418604651162 216.83857293868917 96.21848837209302Q215.00912526427055 95.15279069767442 214.38746828752636 92.59511627906977Q213.7658113107822 90.03744186046512 213.7658113107822 84.78Q213.7658113107822 79.52255813953488 214.38746828752636 76.98264534883721Q215.00912526427055 74.44273255813954 216.85633456659616 73.35927325581395Q218.70354386892174 72.27581395348837 222.6111020084566 72.27581395348837Q223.96098572938683 72.27581395348837 225.38191596194497 72.45343023255813Q226.80284619450313 72.6310465116279 227.655404334038 72.87970930232558V76.50308139534883Q224.95563689217755 76.18337209302325 223.64127642706126 76.18337209302325Q220.97703224101474 76.18337209302325 219.9468578224101 76.69845930232557Q218.91668340380545 77.2135465116279 218.61473572938684 78.86537790697673Q218.31278805496825 80.51720930232558 218.31278805496825 84.78Q218.31278805496825 89.04279069767442 218.61473572938684 90.69462209302326Q218.91668340380545 92.34645348837209 219.9468578224101 92.86154069767443Q220.97703224101474 93.37662790697675 223.64127642706126 93.37662790697675Q225.70162526427058 93.37662790697675 227.76197410147986 93.05691860465116V96.71581395348838Q226.80284619450313 96.96447674418604 225.34639270613104 97.12433139534883Q223.88993921775895 97.28418604651162 222.6111020084566 97.28418604651162Z M237.04683403805493 97.0V76.6096511627907H230.97235729386887V72.56H247.63276427061308V76.6096511627907H241.55828752642702V97.0Z M251.97989164904854 97.0V72.56H266.2602404862578V76.46755813953489H256.49134513742064V82.68412790697674H264.9103567653276V86.62720930232558H256.49134513742064V93.09244186046512H266.2602404862578V97.0Z M271.67306553911203 72.56H279.02637949260037Q283.5378329809725 72.56 285.438327167019 73.57241279069768Q287.3388213530655 74.58482558139535 287.81838530655386 76.91159883720931Q288.29794926004223 79.23837209302326 288.29794926004223 84.78Q288.29794926004223 90.32162790697674 287.81838530655386 92.64840116279069Q287.3388213530655 94.97517441860465 285.438327167019 95.98758720930232Q283.5378329809725 97.0 279.02637949260037 97.0H271.67306553911203ZM278.81323995771663 93.09244186046512Q281.40643763213524 93.09244186046512 282.3122806553911 92.70168604651163Q283.21812367864686 92.31093023255814 283.4845480972515 90.74790697674419Q283.7509725158562 89.18488372093023 283.7509725158562 84.78Q283.7509725158562 80.37511627906977 283.4845480972515 78.81209302325581Q283.21812367864686 77.24906976744185 282.3122806553911 76.85831395348836Q281.40643763213524 76.46755813953489 278.81323995771663 76.46755813953489H276.1845190274841V93.09244186046512Z  M311.09493657505277 97.28418604651162Q307.15185517970394 97.28418604651162 305.32240750528536 96.21848837209302Q303.4929598308667 95.15279069767442 302.8713028541225 92.59511627906977Q302.2496458773783 90.03744186046512 302.2496458773783 84.78Q302.2496458773783 79.52255813953488 302.8713028541225 76.98264534883721Q303.4929598308667 74.44273255813954 305.34016913319226 73.35927325581395Q307.18737843551787 72.27581395348837 311.09493657505277 72.27581395348837Q312.444820295983 72.27581395348837 313.8657505285411 72.45343023255813Q315.28668076109926 72.6310465116279 316.13923890063415 72.87970930232558V76.50308139534883Q313.4394714587737 76.18337209302325 312.1251109936574 76.18337209302325Q309.4608668076109 76.18337209302325 308.43069238900625 76.69845930232557Q307.4005179704016 77.2135465116279 307.098570295983 78.86537790697673Q306.7966226215644 80.51720930232558 306.7966226215644 84.78Q306.7966226215644 89.04279069767442 307.098570295983 90.69462209302326Q307.4005179704016 92.34645348837209 308.43069238900625 92.86154069767443Q309.4608668076109 93.37662790697675 312.1251109936574 93.37662790697675Q314.1854598308667 93.37662790697675 316.24580866807605 93.05691860465116V96.71581395348838Q315.28668076109926 96.96447674418604 313.83022727272714 97.12433139534883Q312.3737737843551 97.28418604651162 311.09493657505277 97.28418604651162Z M320.80607558139525 84.78Q320.80607558139525 79.2028488372093 321.28563953488367 76.78726744186046Q321.76520348837204 74.37168604651163 323.6124127906976 73.32374999999999Q325.4596220930232 72.27581395348837 329.75793604651153 72.27581395348837Q334.0562499999999 72.27581395348837 335.9212209302325 73.32374999999999Q337.78619186046507 74.37168604651163 338.26575581395343 76.78726744186046Q338.7453197674418 79.2028488372093 338.7453197674418 84.78Q338.7453197674418 90.3571511627907 338.26575581395343 92.77273255813954Q337.78619186046507 95.18831395348838 335.9212209302325 96.23625Q334.0562499999999 97.28418604651162 329.75793604651153 97.28418604651162Q325.4596220930232 97.28418604651162 323.6124127906976 96.23625Q321.76520348837204 95.18831395348838 321.28563953488367 92.77273255813954Q320.80607558139525 90.3571511627907 320.80607558139525 84.78ZM334.19834302325575 84.78Q334.19834302325575 80.2685465116279 333.96744186046504 78.65223837209302Q333.73654069767434 77.03593023255814 332.90174418604647 76.6096511627907Q332.06694767441854 76.18337209302325 329.75793604651153 76.18337209302325Q327.4489244186046 76.18337209302325 326.6318895348836 76.6096511627907Q325.8148546511627 77.03593023255814 325.583953488372 78.63447674418605Q325.3530523255813 80.23302325581395 325.3530523255813 84.78Q325.3530523255813 89.32697674418604 325.583953488372 90.92552325581394Q325.8148546511627 92.52406976744186 326.6318895348836 92.9503488372093Q327.4489244186046 93.37662790697675 329.75793604651153 93.37662790697675Q332.06694767441854 93.37662790697675 332.90174418604647 92.9503488372093Q333.73654069767434 92.52406976744186 333.96744186046504 90.90776162790698Q334.19834302325575 89.2914534883721 334.19834302325575 84.78Z M363.37622621564475 97.0V79.13180232558139H363.19860993657494L357.8701215644819 97.0H354.17570295983074L348.8827378435517 79.13180232558139H348.70512156448194V97.0H344.40680761099355V72.56H350.97860993657497L356.02291226215635 89.61116279069768H356.2360517970401L361.2803541226215 72.56H367.745586680761V97.0Z M374.08201638477794 72.56H383.60224894291747Q386.3730628964058 72.56 387.8472780126849 73.50136627906977Q389.321493128964 74.44273255813954 389.83658033826634 76.16561046511629Q390.35166754756864 77.88848837209302 390.35166754756864 80.6593023255814Q390.35166754756864 83.32354651162791 389.85434196617325 85.04642441860466Q389.3570163847779 86.76930232558139 387.8650396405919 87.78171511627906Q386.3730628964058 88.79412790697674 383.53120243128956 88.79412790697674H378.59346987315V97.0H374.08201638477794ZM382.0392256871035 84.88656976744186Q383.81538847780115 84.88656976744186 384.57913847780117 84.5490988372093Q385.3428884778012 84.21162790697674 385.5737896405919 83.37683139534883Q385.8046908033826 82.54203488372093 385.8046908033826 80.6593023255814Q385.8046908033826 78.77656976744186 385.5737896405919 77.94177325581396Q385.3428884778012 77.10697674418604 384.5969001057082 76.78726744186046Q383.85091173361513 76.46755813953489 382.1102721987314 76.46755813953489H378.59346987315V84.88656976744186Z M411.9986205073995 72.56V86.16540697674418Q411.9986205073995 90.99656976744185 411.4657716701902 93.21677325581395Q410.93292283298086 95.43697674418604 409.19228329809715 96.36058139534883Q407.45164376321344 97.28418604651162 403.57960887949247 97.28418604651162Q399.70757399577155 97.28418604651162 397.9491728329808 96.36058139534883Q396.19077167019014 95.43697674418604 395.65792283298083 93.19901162790697Q395.1250739957716 90.96104651162791 395.1250739957716 86.16540697674418V72.56H399.6720507399576V87.58633720930233Q399.6720507399576 90.39267441860466 399.8851902748413 91.49389534883721Q400.09832980972504 92.59511627906977 400.86207980972506 92.98587209302326Q401.6258298097251 93.37662790697675 403.57960887949247 93.37662790697675Q405.5333879492599 93.37662790697675 406.29713794925993 92.98587209302326Q407.06088794925995 92.59511627906977 407.2562658562367 91.51165697674418Q407.45164376321344 90.4281976744186 407.45164376321344 87.58633720930233V72.56Z M422.20708509513736 97.0V76.6096511627907H416.1326083509513V72.56H432.79301532769546V76.6096511627907H426.7185385835094V97.0Z M437.14014270613103 97.0V72.56H441.687119450317V97.0Z M460.954014270613 97.0 452.4994793868921 81.54738372093023H452.3573863636363V97.0H447.98802589852V72.56H452.60604915433396L460.7408747357293 87.2311046511628H460.8829677589851V72.56H465.2523282241014V97.0Z M479.6525369978858 86.9113953488372V83.11040697674419H487.39660676532765V96.5381976744186Q485.8691067653277 96.85790697674419 483.8442811839323 97.07104651162791Q481.81945560253695 97.28418604651162 480.22090909090906 97.28418604651162Q476.27782769556023 97.28418604651162 474.3240486257928 96.25401162790698Q472.37026955602533 95.22383720930233 471.64204281183925 92.63063953488373Q470.9138160676532 90.03744186046512 470.9138160676532 84.78Q470.9138160676532 79.59360465116279 471.65980443974627 76.98264534883721Q472.40579281183926 74.37168604651163 474.39509513742064 73.32374999999999Q476.3843974630021 72.27581395348837 480.3630021141649 72.27581395348837Q481.99707188160676 72.27581395348837 483.96861257928117 72.48895348837209Q485.9401532769556 72.70209302325581 487.2900369978858 73.02180232558139V76.68069767441861Q483.73771141649047 76.18337209302325 481.99707188160676 76.18337209302325Q478.7999788583509 76.18337209302325 477.5744265327695 76.6096511627907Q476.34887420718815 77.03593023255814 475.9048335095137 78.7410465116279Q475.46079281183927 80.44616279069767 475.46079281183927 84.78Q475.46079281183927 88.75860465116278 475.78050211416485 90.49924418604651Q476.1002114164904 92.23988372093024 477.09486257928114 92.86154069767443Q478.08951374207186 93.4831976744186 480.3274788583509 93.4831976744186Q481.85497885835093 93.4831976744186 483.0272463002114 93.30558139534884V86.9113953488372Z "></path><path fill="#F58F2D" d="M502.84028012684985 97.0V72.56H505.6821405919661V94.4068023255814H515.3444661733615V97.0Z M534.6468842494714 97.0 532.4799656448203 90.32162790697674H523.2439191331923L521.1125237843552 97.0H518.2351400634249L526.0502563424947 72.56H529.7801982029598L537.5953144820296 97.0ZM527.9685121564481 75.68604651162791H527.7908958773784L523.9543842494714 87.79947674418605H531.7339772727272Z M542.723953488372 72.56H552.2086627906976Q555.4412790697673 72.56 556.8444476744185 74.12302325581396Q558.2476162790697 75.68604651162791 558.2476162790697 78.67Q558.2476162790697 81.15662790697674 557.3417732558139 82.57755813953489Q556.4359302325581 83.99848837209302 554.4111046511628 84.42476744186047V84.56686046511628Q556.7556395348837 85.02866279069767 557.6259593023256 86.53840116279069Q558.4962790697674 88.04813953488372 558.4962790697674 90.57029069767442Q558.4962790697674 93.55424418604652 557.1108720930232 95.27712209302325Q555.725465116279 97.0 552.0665697674418 97.0H542.723953488372ZM555.4057558139534 79.23837209302326Q555.4057558139534 77.6753488372093 555.0682848837209 76.78726744186046Q554.7308139534883 75.89918604651163 553.8604941860465 75.49066860465116Q552.9901744186046 75.0821511627907 551.3916279069767 75.0821511627907H545.5658139534884V83.35906976744187H551.3916279069767Q553.416453488372 83.35906976744187 554.4111046511628 82.36441860465116Q555.4057558139534 81.36976744186046 555.4057558139534 79.23837209302326ZM555.6188953488372 90.21505813953489Q555.6188953488372 87.835 554.7130523255814 86.85811046511628Q553.8072093023255 85.88122093023256 551.5337209302326 85.88122093023256H545.5658139534884V94.51337209302325H551.5337209302326Q553.8782558139534 94.51337209302325 554.7485755813952 93.5542441860465Q555.6188953488372 92.59511627906977 555.6188953488372 90.21505813953489Z"></path></svg>';
var SEEDS = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="22.23 15.5 58.14 55.96" role="img" aria-label="KAUST"><g id="seeds"><path fill="#f09021" d="M60.56,49.15c.87-1.19,1.3-2.73,1.3-4.62,0-1.14-.16-2.14-.43-3.03-.98.94-1.61,2.1-1.61,3.51,0,1.15.24,2.62.75,4.14Z"></path><path fill="#f09021" d="M71.49,38.22c.3,1.66.45,3.34.45,4.98,0,6.18-1.2,10.6-3.04,13.76.31.03.62.06.95.06,8.65,0,10.52-8.05,10.52-11.55,0-5.6-5.73-7.1-8.88-7.25Z"></path><path fill="#f09021" d="M62.79,44.53c0,2.35-.66,4.2-1.87,5.6.33.81.74,1.61,1.22,2.37,1.7-1.95,2.56-4.75,2.56-8.41,0-1.54-.25-2.97-.71-4.28-.65.3-1.26.65-1.8,1.05.37,1.06.6,2.27.6,3.68Z"></path><path fill="#f09021" d="M65.79,44.08c0,4.42-1.2,7.37-3,9.32.46.58.97,1.12,1.54,1.6,1.91-2.4,3.18-6,3.18-11.37,0-1.64-.22-3.29-.64-4.86-.62.16-1.25.36-1.87.59.5,1.45.79,3.02.79,4.71Z"></path><path fill="#f09021" d="M68.87,43.64c0,5.74-1.4,9.58-3.43,12.16.57.34,1.19.62,1.86.83,1.88-3,3.08-7.31,3.08-13.43,0-.86-.07-2.73-.49-4.94-.53.04-1.11.12-1.71.23.46,1.68.7,3.43.7,5.15Z"></path><path fill="#efb51d" d="M51.58,30.65c4.8,0,8.75,1.82,11.27,4.86.53-.71.96-1.48,1.32-2.23-2.28-2.86-5.77-4.77-10.6-4.77-2.54,0-7.24.61-11.38,2.18.07.5.24,1,.49,1.49,2.94-1.06,6.25-1.53,8.9-1.53Z"></path><path fill="#efb51d" d="M65.45,26.99c-.33-4.34-2.63-11.49-10.61-11.49-6.17,0-9.89,5.83-11.59,10.24,4.7-1.47,9.52-2.06,12.33-2.06,4.18,0,7.41,1.28,9.87,3.31Z"></path><path fill="#efb51d" d="M44.69,34.61c1.45-.33,3.1-.51,4.91-.51,2.7,0,7.83.48,10.83,3.74.63-.42,1.2-.91,1.7-1.45-2.31-2.9-6.02-4.65-10.54-4.65-2.48,0-5.57.42-8.32,1.36.39.53.87,1.04,1.42,1.52Z"></path><path fill="#efb51d" d="M59.57,38.34c-2.8-2.88-7.48-3.31-9.96-3.31-1.45,0-2.78.12-3.97.34,3.06,2.26,7.45,3.8,10.29,3.8,1.39,0,2.6-.32,3.64-.83Z"></path><path fill="#efb51d" d="M53.58,27.15c4.98,0,8.66,1.93,11.17,4.79.41-1.14.64-2.17.72-2.91-2.19-2.22-5.33-3.79-9.88-3.79-2.81,0-8.08.68-12.95,2.36-.18.61-.3,1.14-.38,1.6,4.02-1.45,8.44-2.05,11.33-2.05Z"></path><path fill="#cccd2b" d="M32.77,39.19c0-3.61,2.3-6.24,5.46-8.12-.46-.3-.95-.55-1.45-.72-2.97,1.94-5.05,4.51-5.05,7.85,0,2.97,1.1,6.79,3.07,10.54.33-.03.64-.09.94-.18-1.9-3.33-2.97-6.73-2.97-9.36Z"></path><path fill="#cccd2b" d="M35.35,40.19c0-3.12,2.01-5.35,4.8-6.88-.23-.49-.5-.94-.82-1.3-2.98,1.65-5.19,3.98-5.19,7.18,0,2.46,1,5.65,2.78,8.76.29-.21.57-.45.81-.73-1.52-2.5-2.38-5.05-2.38-7.03Z"></path><path fill="#cccd2b" d="M34.71,29.92c-.4-.04-.77-.05-1.11-.05-3.62,0-11.37,2.99-11.37,8.5,0,5.91,6.57,9.56,10.85,10.27-1.86-3.74-2.9-7.5-2.9-10.45,0-3.46,1.82-6.18,4.53-8.27Z"></path><path fill="#cccd2b" d="M37.81,41.19c0-2.07,1.08-3.79,3-5.02-.04-.69-.14-1.31-.29-1.86-2.39,1.34-4.08,3.26-4.08,5.88,0,1.74.7,3.94,1.97,6.14.25-.38.46-.8.67-1.23-.81-1.41-1.27-2.79-1.27-3.92Z"></path><path fill="#cccd2b" d="M38.74,41.19c0,.83.28,1.82.78,2.85.94-2.55,1.27-5.44,1.33-6.77-1.35,1.02-2.1,2.35-2.1,3.92Z"></path><path fill="#00a6aa" d="M52.58,53.09c-4.28,0-7.93-1.88-10.53-4.29-.48.19-.96.41-1.43.64,2.99,3.49,7.4,6.35,12.65,6.35,2.54,0,4.65-.45,6.34-1.33-.34-.93-.79-1.73-1.34-2.43-1.53.68-3.42,1.05-5.7,1.05Z"></path><path fill="#00a6aa" d="M35.62,52.96c-1.83,1.82-3.02,3.92-3.02,5.97,0,8.02,12.48,12.53,17.32,12.53,5.73,0,8.41-3.63,9.57-7.48-1.73.43-3.41.58-4.88.58-7.99,0-14.66-5.3-18.99-11.6Z"></path><path fill="#00a6aa" d="M47.98,47.54c-1.61,0-3.32.34-4.98.91,2.42,2.1,5.72,3.71,9.58,3.71,2.03,0,3.69-.31,5.06-.86-3.02-3.14-7.79-3.76-9.66-3.76Z"></path><path fill="#00a6aa" d="M53.27,56.89c-5.66,0-10.4-3.14-13.58-6.94-.37.2-.72.41-1.08.63,3.52,4.79,8.89,8.8,15.33,8.8,2.08,0,4.3-.32,6.32-1.23-.02-.96-.13-1.83-.33-2.62-2.08,1.02-4.46,1.37-6.67,1.37Z"></path><path fill="#00a6aa" d="M53.95,60.75c-6.9,0-12.65-4.31-16.41-9.43-.28.21-.53.42-.79.64,4.06,5.98,10.33,11.04,17.87,11.04,1.73,0,3.54-.21,5.29-.74.18-.91.28-1.81.33-2.64-2.2.87-4.45,1.12-6.29,1.12Z"></path></g></svg>';

// viewer/src/opening.ts
var NS = "http://www.w3.org/2000/svg";
var f2 = (v) => +v.toFixed(2);
var clamp01 = (u) => Math.min(1, Math.max(0, u));
var easeIO = (u) => (u = clamp01(u), u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2);
var easeOutExpo = (u) => u >= 1 ? 1 : 1 - 2 ** (-10 * Math.max(0, u));
var easeBack = (u, k) => (u = clamp01(u) - 1, 1 + (k + 1) * u * u * u + k * u * u);
var ease2 = (u) => (u = clamp01(u), u < 0.5 ? 2 * u * u : 1 - (-2 * u + 2) ** 2 / 2);
var OFF = 1.01;
var OPEN = { draw: 700, stagger: 110, sweep: 1e3, rest: 300, cycle: 1600, settle: 500, dive: 1100, fade: 250, into: [0.3, 0.42], deep: 5 };
var PHI_COUNTER = "M228 344Q228 316 233 294Q238 272 252.5 249Q267 226 298 210.5Q329 195 375 190V495Q288 485 258 444Q228 403 228 344Z";
var BY = { h: 18, faint: "#8d96a5", kaust: "#80715e" };
var SANS = `"Noto Sans", system-ui, sans-serif`;
var uid = 0;
var art = (a, fill, tag) => a.body.replace(/§/g, `phx${tag}-`).replace(/#FILL/g, fill);
var svgOf = (markup) => {
  const t = document.createElement("template");
  t.innerHTML = markup;
  return t.content.firstElementChild;
};
function byline(svg, word) {
  const h = BY.h;
  const ctx = document.createElement("canvas").getContext("2d");
  const sans = (px, wt) => (ctx.font = `${wt} ${px}px ${SANS}`, (s) => ctx.measureText(s).width);
  const byW = sans(0.55 * h, 400)("by"), kW = sans(0.72 * h, 700);
  const letters = [..."KAUST"], track = 0.05 * 0.72 * h, adv = letters.map((c) => kW(c) + track);
  const lockup = svgOf(LOCKUP), seeds = svgOf(SEEDS);
  const vb = lockup.viewBox.baseVal, lw = h * vb.width / vb.height, sv = seeds.viewBox.baseVal, sh = 1.1 * h, sw = sh * sv.width / sv.height;
  const gap = 0.45 * h, sep = 0.9 * h;
  const width = byW + gap + lw + sep * 2 + sw + gap * 0.6 + adv.reduce((a, b) => a + b, 0);
  const x0 = (word.x0 + word.x1) / 2 - width / 2, top = word.y1 + 0.9 * h, mid = top + h / 2;
  const g = document.createElementNS(NS, "g"), wid = `phx-by${uid++}`;
  svg.append(g);
  let x = x0;
  g.insertAdjacentHTML("beforeend", `<text x="${f2(x)}" y="${f2(mid + 0.19 * h)}" fill="${BY.faint}" style="font: 400 ${f2(0.55 * h)}px ${SANS}">by</text>`);
  const byT = g.lastElementChild;
  x += byW + gap;
  for (const [k, v] of [["x", x], ["y", top], ["width", lw], ["height", h]]) lockup.setAttribute(k, String(f2(v)));
  g.append(lockup);
  x += lw + sep;
  g.insertAdjacentHTML("beforeend", `<rect x="${f2(x)}" y="${f2(top)}" width="${f2(0.1 * h)}" height="${f2(h)}" fill="${BY.faint}"/>`);
  const rule = g.lastElementChild;
  x += sep;
  for (const [k, v] of [["x", x], ["y", mid - sh / 2], ["width", sw], ["height", sh]]) seeds.setAttribute(k, String(f2(v)));
  g.append(seeds);
  x += sw + gap * 0.6;
  const ks = letters.map((c, i) => {
    g.insertAdjacentHTML("beforeend", `<text x="${f2(x)}" y="${f2(mid + 0.26 * h)}" fill="${BY.kaust}" style="font: 700 ${f2(0.72 * h)}px ${SANS}">${c}</text>`);
    x += adv[i];
    return g.lastElementChild;
  });
  const paths = [...lockup.querySelectorAll("path")], A = paths.slice(0, 2), stripes = paths.slice(2, 6), words2 = paths.slice(6);
  const up = [77.59, -94], n = Math.hypot(up[0], up[1]), D = 150, from2 = [-up[0] / n * D, -up[1] / n * D];
  const trails = stripes.map(
    (s) => [0.4, 0.22, 0.1].map((op) => {
      const c = s.cloneNode();
      c.dataset.op = String(op);
      s.before(c);
      return c;
    })
  );
  const outline = A.map((a) => {
    const c = a.cloneNode();
    for (const [k, v] of [["fill", "none"], ["stroke", "#1D3E72"], ["stroke-width", "1.1"], ["stroke-linejoin", "round"], ["pathLength", "1"], ["stroke-dasharray", "1 2"]]) c.setAttribute(k, v);
    a.after(c);
    return c;
  });
  const wg = document.createElementNS(NS, "g");
  words2[0]?.before(wg);
  words2.forEach((w) => wg.append(w));
  lockup.insertAdjacentHTML("afterbegin", `<defs><clipPath id="${wid}"><rect x="97.5" y="-50" width="1000" height="200"/></clipPath></defs>`);
  const clipped = document.createElementNS(NS, "g");
  wg.before(clipped);
  clipped.append(wg);
  clipped.setAttribute("clip-path", `url(#${wid})`);
  const at = (e, dx, dy) => e.setAttribute("transform", `translate(${f2(dx)} ${f2(dy)})`);
  return {
    x0,
    x1: x0 + width,
    y1: top + h,
    frame(t) {
      byT.setAttribute("opacity", String(f2(clamp01(t / 300))));
      stripes.forEach((s, i) => {
        const t0 = 50 + i * 70, k = easeBack((t - t0) / 950, 1.15);
        at(s, from2[0] * (1 - k), from2[1] * (1 - k));
        s.setAttribute("opacity", t < t0 ? "0" : "1");
        trails[i].forEach((c, j) => {
          const tj = t0 + (j + 1) * 40, kj = easeOutExpo((t - tj) / 900);
          at(c, from2[0] * (1 - kj), from2[1] * (1 - kj));
          c.setAttribute("opacity", t < tj ? "0" : String(f2(Number(c.dataset.op) * (1 - clamp01((t - tj - 350) / 300)))));
        });
      });
      outline.forEach((o) => {
        o.setAttribute("stroke-dashoffset", String(f2(1 - ease2((t - 300) / 750))));
        o.setAttribute("opacity", String(f2(t < 300 ? 0 : 1 - clamp01((t - 1050) / 300))));
      });
      A.forEach((a) => a.setAttribute("opacity", String(f2(clamp01((t - 850) / 350)))));
      words2.forEach((w, i) => at(w, -470 * (1 - easeOutExpo((t - 800 - i * 45) / 1100)), 0));
      rule.setAttribute("opacity", t < 900 ? "0" : "1");
      rule.setAttribute("transform", `translate(0 ${f2(mid)}) scale(1 ${f2(ease2((t - 900) / 300))}) translate(0 ${f2(-mid)})`);
      const sk = easeBack((t - 1e3) / 550, 2.6), cx = Number(seeds.getAttribute("x")) + sw / 2, by = mid + sh / 2;
      seeds.setAttribute("opacity", String(f2(clamp01((t - 1e3) / 200))));
      seeds.setAttribute("transform", `translate(${f2(cx)} ${f2(by + 0.5 * h * (1 - sk))}) scale(${f2(0.2 + 0.8 * sk)}) translate(${f2(-cx)} ${f2(-by)})`);
      ks.forEach((k, i) => k.setAttribute("opacity", t >= 1150 + i * 60 ? "1" : "0"));
    }
  };
}
function opening(box, fill, bg) {
  const tag = String(uid++);
  const w = WORD, size = w.size, phi = w.letters[0];
  box.insertAdjacentHTML(
    "beforeend",
    `<svg class="phx-open" preserveAspectRatio="none" aria-hidden="true">
    <defs><mask id="phx-hole${tag}" maskUnits="userSpaceOnUse" x="-1e5" y="-1e5" width="2e5" height="2e5"><rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="#fff"/>
      <path class="phx-hole" d="${PHI_COUNTER}" fill="#000" opacity="0" transform="translate(${f2(phi.x)} ${f2(phi.y)}) scale(${size / 1e3} ${-size / 1e3})"/></mask></defs>
    <rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="${bg}" mask="url(#phx-hole${tag})"/>${art(WOVEN, fill, tag)}</svg>`
  );
  const el = box.lastElementChild, hole = el.querySelector(".phx-hole");
  const by = byline(el, WOVEN);
  const word = { x0: Math.min(WOVEN.x0, by.x0), y0: WOVEN.y0, x1: Math.max(WOVEN.x1, by.x1), y1: by.y1 };
  const rest = [...el.querySelectorAll(".phx-l")].filter((g) => g.dataset.ch !== "\u03A6").map((g) => [g, w.letters.find((l) => l.ch === g.dataset.ch)]);
  const tmp = document.createElementNS(NS, "svg");
  tmp.style.cssText = "position:absolute;width:0;height:0;visibility:hidden";
  document.body.append(tmp);
  tmp.innerHTML = art(MARK, fill, `${tag}m`);
  const S = [0, 1, 2, 3].map((j) => {
    const ps = [...el.querySelectorAll(`.phx-s${j}`)];
    const full = ps[0].getTotalLength(), part = tmp.querySelector(`.phx-s${j}`).getTotalLength();
    ps.forEach((p) => p.style.strokeDasharray = "1 2");
    return { ps, full, f: part / full };
  });
  tmp.remove();
  const W = () => box.clientWidth || 1, H = () => box.clientHeight || 1;
  const fit = (r, fw, fh) => ({
    cx: (r.x0 + r.x1) / 2,
    cy: (r.y0 + r.y1) / 2,
    s: Math.max((r.x1 - r.x0) / (fw * W()), (r.y1 - r.y0) / (fh * H()))
  });
  const P = { x: phi.x + OPEN.into[0] * size, y: phi.y - OPEN.into[1] * size };
  const deep = () => OPEN.deep / W();
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const T1 = OPEN.draw + 3 * OPEN.stagger, T2 = T1 + 200 + OPEN.sweep + 1.5 * OPEN.stagger, T3 = T2 + OPEN.rest;
  let doneAt = null, settled = null, end = false, raf = 0;
  let finish = () => {
  };
  const t0 = performance.now();
  const flow = (j, t) => {
    const u = (t - T3 - j * OPEN.stagger) / OPEN.cycle;
    if (u < 0) return 0;
    const v = u % 1;
    return v < 0.5 ? -OFF * easeIO(v * 2) : OFF * (1 - easeIO(v * 2 - 1));
  };
  const frame = (t) => {
    if (still) t = doneAt === null ? T3 : Math.max(t, (settled ??= doneAt) + OPEN.dive);
    let head = phi.r;
    S.forEach((st, j) => {
      let o;
      if (t < T1 + 200) o = OFF - (OFF - (1 - st.f)) * easeIO((t - j * OPEN.stagger) / OPEN.draw);
      else if (t < T3) o = (1 - st.f) * (1 - easeIO((t - T1 - 200 - j * OPEN.stagger / 2) / OPEN.sweep));
      else if (doneAt === null || t < doneAt) o = flow(j, t);
      else {
        const o0 = flow(j, doneAt), u = easeIO((t - doneAt - j * OPEN.stagger / 2) / OPEN.settle);
        o = o0 <= 0 ? u < 0.5 ? o0 + (-OFF - o0) * u * 2 : OFF * (2 - u * 2) : o0 * (1 - u);
      }
      st.ps.forEach((p) => p.style.strokeDashoffset = String(o));
      if (o >= 0 && o < 1) head = Math.max(head, st.ps[0].getPointAtLength((1 - o) * st.full).x);
    });
    rest.forEach(([g, l]) => g.style.opacity = t < T3 ? String(f2(clamp01((head - Math.max(l.l, phi.r)) / (0.3 * (l.r - l.l))))) : "1");
    const cm = fit(MARK, 0.45, 0.3), cw = fit(word, 0.84, 0.4);
    const k0 = easeIO((t - T1 - 200) / (0.75 * OPEN.sweep));
    let c = t < T1 + 200 ? cm : { cx: cm.cx + (cw.cx - cm.cx) * k0, cy: cm.cy + (cw.cy - cm.cy) * k0, s: cm.s + (cw.s - cm.s) * k0 };
    if (doneAt !== null && t >= (settled ??= doneAt + OPEN.settle + 2 * OPEN.stagger)) {
      const u = Math.min(1, (t - settled) / OPEN.dive), s = cw.s * Math.exp(Math.log(deep() / cw.s) * u ** 2.2), k = (s - deep()) / (cw.s - deep());
      c = { cx: P.x + (cw.cx - P.x) * k, cy: P.y + (cw.cy - P.y) * k, s };
      hole.setAttribute("opacity", String(f2(Math.min(1, u * 4))));
      el.style.opacity = String(1 - Math.max(0, (t - settled - OPEN.dive) / OPEN.fade));
      if (t >= settled + OPEN.dive + OPEN.fade) end = true;
    }
    by.frame(still ? 1e9 : t - (T2 - 300));
    el.setAttribute("viewBox", [c.cx - W() * c.s / 2, c.cy - H() * c.s / 2, W() * c.s, H() * c.s].map(f2).join(" "));
  };
  const tick = () => {
    frame(performance.now() - t0);
    if (end) return void (el.remove(), finish());
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return {
    done() {
      doneAt ??= Math.max(performance.now() - t0, T3);
      return new Promise((ok) => finish = ok);
    },
    stop() {
      cancelAnimationFrame(raf);
      el.remove();
      finish();
    }
  };
}

// viewer/src/panel.ts
var STALE_GRACE_MS = 1e3;
var OVERLEAF_WORDS = {
  badge: "Unofficial \xB7 experimental",
  badgeTitle: "An unofficial extension, not part of Overleaf. PhiTeX runs LaTeX (pdfTeX) in your browser; Overleaf's PDF is the real one.",
  byline: "Unofficial PhiTeX extension \xB7 experimental",
  about: `<b>\u26A1 Instant is not part of Overleaf.</b> It is added by the <b>unofficial PhiTeX</b> browser extension, not made,
      endorsed or supported by Overleaf: an experimental incremental TeX engine that runs entirely in your browser. Nothing is
      sent anywhere; Overleaf's own PDF is on the <b>PDF</b> tab.
      <div class="about-foot">LaTeX, with TeX Live's packages fetched as needed. Free software (AGPL-3.0-only,
      <a id="license" target="_blank" rel="noopener">full license</a>), provided as is, without any warranty.
      To turn it off: <code>chrome://extensions</code>.</div>`,
  preparing: "Getting ready for instant edits\u2026 {s} s (pages still scroll)",
  updating: "Updating the other pages\u2026",
  license: "LICENSE.txt",
  realPdf: "Overleaf's PDF is the real one.",
  stopped: "Open \u24D8 diagnostics for the details. Overleaf's PDF is one click away (PDF).",
  notReady: "Use Overleaf's PDF for it",
  openFailed: "Overleaf's PDF is one click away. Reload the tab to try again.",
  reading: "Your project's files stay in this browser: read from Overleaf, typeset here.",
  keptIn: "this browser",
  steps: ["Reading the project", "Fetching packages", "Typesetting"]
};
function safeColor(c, or) {
  return typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c) ? c : or;
}
var DEFAULTS = { x: null, y: null, w: 440, h: 620, zoom: "fit", collapsed: false, details: false };
function pageBox(d, cssWidth, dpr = globalThis.devicePixelRatio || 1) {
  const snap = (x) => Math.floor(x * dpr) / dpr;
  return [snap(cssWidth), snap(d.h * cssWidth / d.w)];
}
var esc3 = (t) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
var PATHS = {
  preview: "M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z",
  chevron_left: "M15.41 7.41 14 6l-6 6 6 6 1.41-1.41L10.83 12z",
  chevron_right: "M10 6 8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z",
  bug_report: "M20 8h-2.81c-.45-.78-1.07-1.45-1.82-1.96L17 4.41 15.59 3l-2.17 2.17C12.96 5.06 12.49 5 12 5s-.96.06-1.41.17L8.41 3 7 4.41l1.62 1.63C7.88 6.55 7.26 7.22 6.81 8H4v2h2.09c-.05.33-.09.66-.09 1v1H4v2h2v1c0 .34.04.67.09 1H4v2h2.81c1.04 1.79 2.97 3 5.19 3s4.15-1.21 5.19-3H20v-2h-2.09c.05-.33.09-.66.09-1v-1h2v-2h-2v-1c0-.34-.04-.67-.09-1H20V8zm-6 8h-4v-2h4v2zm0-4h-4v-2h4v2z",
  close_fullscreen: "M19 13H5v-2h14v2z",
  open_in_full: "M21 11V3h-8l3.29 3.29-10 10L3 13v8h8l-3.29-3.29 10-10z",
  sync: "M12 4V1L8 5l4 4V6c3.31 0 6 2.69 6 6 0 1.01-.25 1.97-.7 2.8l1.46 1.46C19.54 15.03 20 13.57 20 12c0-4.42-3.58-8-8-8zm0 14c-3.31 0-6-2.69-6-6 0-1.01.25-1.97.7-2.8L5.24 7.74C4.46 8.97 4 10.43 4 12c0 4.42 3.58 8 8 8v3l4-4-4-4v3z",
  download: "M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z",
  error: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z",
  warning: "M1 21h22L12 2 1 21zm12-3h-2v-2h2v2zm0-4h-2v-4h2v4z",
  info: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z",
  refresh: "M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z",
  expand_more: "M16.59 8.59 12 13.17 7.41 8.59 6 10l6 6 6-6z",
  more_horiz: "M6 10c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm12 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm-6 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z",
  search: "M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z",
  compare: "M9.01 14H2v2h7.01v3L13 15l-3.99-4v3zm5.98-1v-3H22V8h-7.01V5L11 9l3.99 4z",
  close: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  log: "M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
  toc: "M3 9h14V7H3v2zm0 4h14v-2H3v2zm0 4h14v-2H3v2zm16 0h2v-2h-2v2zm0-10v2h2V7h-2zm0 6h2v-2h-2v2z",
  dark: "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z",
  keyboard: "M20 5H4c-1.1 0-1.99.9-1.99 2L2 17c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm-9 3h2v2h-2V8zm0 3h2v2h-2v-2zM8 8h2v2H8V8zm0 3h2v2H8v-2zm-1 2H5v-2h2v2zm0-3H5V8h2v2zm9 7H8v-2h8v2zm0-4h-2v-2h2v2zm0-3h-2V8h2v2zm3 3h-2v-2h2v2zm0-3h-2V8h2v2z",
  tips: "M9 21c0 .55.45 1 1 1h4c.55 0 1-.45 1-1v-1H9v1zm3-19C8.14 2 5 5.14 5 9c0 2.38 1.19 4.47 3 5.74V17c0 .55.45 1 1 1h6c.55 0 1-.45 1-1v-2.26c1.81-1.27 3-3.36 3-5.74 0-3.86-3.14-7-7-7z"
};
var icon = (name, size = 18) => `<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`;
var ZOOMS = [["fit", "Page width"], ["page", "Whole page"], ["0.5", "50%"], ["0.75", "75%"], ["1", "100%"], ["1.25", "125%"], ["1.5", "150%"], ["2", "200%"], ["3", "300%"]];
var TIPS = [
  { id: "compare", at: "#b-compare", title: "Compare versions", text: "See what changed since an earlier version, marked as latexdiff marks it: additions in blue, deletions struck out.", fresh: true },
  { id: "timing", at: "#st", title: "How long it took", text: "Click the status for the last build's times and how long a keystroke takes to reach the page." },
  { id: "rebuild", at: "#b-rec", title: "Clean rebuild", text: "Builds run by themselves as you type. Start over from a clean slate here, or clear what PhiTeX keeps." },
  { id: "more", at: "#b-more", title: "Dark PDF and more", text: "The pages in dark mode, the log, the contents and the keys are in this menu." }
];
var Panel = class {
  root;
  host;
  docked = null;
  win;
  prefs = { ...DEFAULTS };
  store;
  at = 0;
  last = null;
  url = null;
  lastStatus = null;
  listeners = [];
  speed;
  debugOn = false;
  ev;
  /** A page that came while text in the shown one was selected: shown once the selection ends. */
  viewer;
  /** The host's look, and whether the panel has a bar of its own (VS Code, the watch, floating). */
  kind;
  ownBar;
  $ = (sel) => this.root.querySelector(sel);
  words;
  constructor(ev, store, opts = {}) {
    this.store = store;
    this.ev = ev;
    const w = this.words = { ...OVERLEAF_WORDS, ...opts.words };
    const vscode = /\bvscode-/.test(document.body?.className ?? "");
    this.kind = opts.host ?? (vscode ? "vscode" : "overleaf");
    this.ownBar = !!opts.header;
    const host2 = document.createElement("phitex-preview");
    this.root = host2.attachShadow({ mode: "open" });
    const item2 = (id, name, label, sub = "", cls = "") => `<button type="button" role="menuitem" id="${id}"${cls ? ` class="${cls}"` : ""}>${name ? `<span class="icon">${name}</span>` : '<span class="check"></span>'}<span>${label}</span>${sub ? `<span class="sub">${sub}</span>` : ""}</button>`;
    this.root.innerHTML = `<style>${VIEWER_CSS}${PROBLEMS_CSS}${OUTLINE_CSS}${PRINT_CSS}</style>
<div class="win ${this.kind}" role="dialog" aria-label="PhiTeX preview">
  <header><span>PhiTeX</span><span class="grow"></span><button class="ib" id="min" title="Collapse (Alt+Shift+P)" aria-label="Collapse"><span class="icon">close_fullscreen</span></button></header>
  <div class="bar" role="toolbar" aria-label="PhiTeX">
    <span class="split" id="split"><button type="button" class="go" id="go" title="Build again with the files as they are (builds run by themselves as you type)"><span class="icon">refresh</span><span class="wide">Rebuild</span></button><button type="button" class="more" id="b-rec" aria-haspopup="menu" aria-expanded="false" title="More ways to build" aria-label="More ways to build"><span class="icon">expand_more</span></button></span>
    <button type="button" class="status" id="st" aria-haspopup="dialog" aria-expanded="false" title="The last build's times"><b id="sttext">Starting\u2026</b><span id="stmore"></span></button>
    <span class="grow"></span>
    <span class="find" id="find" hidden><input id="findq" type="search" placeholder="Find" spellcheck="false" aria-label="Find in the pages"><span id="findn"></span></span>
    <button type="button" class="ib wide" id="b-find" title="Find in the pages (Ctrl+F)" aria-label="Find"><span class="icon">search</span></button>
    <button type="button" class="ib wide" id="b-compare" title="Compare with an earlier version (latexdiff)" aria-label="Compare"><span class="icon">compare</span></button>
    <button type="button" class="ib wide" id="pdf" title="Download the PDF" aria-label="Download the PDF"><span class="icon">download</span></button>
    <span class="pageno"><input id="pgin" value="\u2013" aria-label="Page" inputmode="numeric" autocomplete="off"><span>/&nbsp;<span id="pgn">0</span></span></span>
    <button type="button" class="ib zoomb wide" id="b-zoom" aria-haspopup="menu" aria-expanded="false" title="Zoom"><span id="zl">Page width</span><span class="icon">expand_more</span></button>
    <button type="button" class="ib probs" id="b-probs" aria-haspopup="dialog" aria-expanded="false" title="Problems" hidden><span id="psev"></span><span id="pcount"></span></button>
    <button type="button" class="ib" id="b-more" aria-haspopup="menu" aria-expanded="false" title="More" aria-label="More"><span class="icon">more_horiz</span></button>
    <span class="progress" id="prog"></span>
    <div class="menu" id="m-rec" role="menu" hidden style="left:6px;top:calc(100% + 4px)">
      ${item2("r-rebuild", "", "Rebuild")}
      <button type="button" role="menuitem" id="r-clean"><span class="check"></span><span>Clean rebuild<span class="desc">Start over: the project read again, built from the start</span></span></button>
      <hr class="if-compare"><button type="button" role="menuitem" id="r-compare" class="if-compare"><span class="check"></span><span>Compare with an earlier version\u2026<span class="desc">latexdiff against a saved version</span></span></button>
      <hr class="if-cache"><button type="button" role="menuitem" id="r-cache" class="if-cache"><span class="check"></span><span>Clear cache\u2026<span class="desc">The packages and formats PhiTeX keeps</span></span></button>
    </div>
    <div class="menu timing" id="m-time" role="dialog" aria-label="Timing" hidden style="left:6px;top:calc(100% + 4px)"></div>
    <div class="menu" id="m-zoom" role="menu" hidden style="right:60px;top:calc(100% + 4px);min-width:170px">
      ${ZOOMS.map(([z, t], i) => `${i === 2 ? "<hr>" : ""}<button type="button" role="menuitemradio" data-zoom="${z}"><span class="check"></span>${t}</button>`).join("")}
    </div>
    <div class="menu" id="m-more" role="menu" hidden style="right:6px;top:calc(100% + 4px);min-width:250px">
      ${item2("x-pdf", "download", "Download PDF")}
      ${item2("x-log", "log", "View log")}
      ${item2("x-contents", "toc", "Contents", "t")}
      ${item2("x-find", "search", "Find", "Ctrl+F", "narrow")}
      ${item2("x-compare", "compare", "Compare\u2026", "", "narrow")}
      <hr>
      <div class="mi"><span class="icon">dark</span>Dark PDF</div>
      <div class="radios" id="darkpdf" role="group" aria-label="Dark PDF"><button type="button" data-dark="off">Off</button><button type="button" data-dark="on">On</button><button type="button" data-dark="theme">Match theme</button></div>
      <button type="button" role="menuitemcheckbox" id="dbg" aria-checked="false"><span class="check"></span><span>Debug check<span class="desc">Compare with a fresh build every 5 s</span></span></button>
      <hr>
      ${item2("x-tips", "tips", "What's new")}
      ${item2("x-keys", "keyboard", "Keyboard shortcuts", "?")}
      ${item2("x-about", "info", "About PhiTeX")}
    </div>
    <div class="tip" id="tip" role="dialog" aria-live="polite" hidden></div>
  </div>
  <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
  <div class="body">
    <nav class="side" id="side" aria-label="Contents"></nav>
    <div class="stage" id="stage"><div class="strip gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">\u2715</button></div><div class="strip approx" id="approx"></div><div class="strip banner" id="banner"></div><div class="empty" id="empty" hidden></div><div class="viewer" id="viewer"></div></div>
    <div class="opening" id="opening" hidden><div class="sheet"></div><div class="steps" aria-live="polite"><div class="what"></div><div class="track unknown"><i></i></div><div class="detail"></div></div></div>
    <div class="menu diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="menu logs" id="logs" role="dialog" aria-label="Problems" hidden><header><span id="logshead">Problems</span><span class="grow"></span><button type="button" class="ib" id="logsx" title="Close (Esc)" aria-label="Close"><span class="icon">close</span></button></header><div class="diags" id="diags" role="list"></div></div>
    <div class="menu sheet-pop" id="sheet" role="dialog" aria-label="PhiTeX settings" hidden>
      <label>Main file <select id="main" title="The file PhiTeX typesets"></select></label>
      <label id="dbglabel">Debug check <input type="checkbox" id="dbg2"></label>
      <div class="links"><button type="button" id="reload">Reload the files</button><button type="button" id="tour">Take the tour</button><button type="button" id="news">What's new</button></div>
      <pre class="details" id="details"></pre>
    </div>
    <div class="toast" id="toast" role="status" aria-live="polite" hidden></div>
    <div class="dialog keys" id="keyhelp" role="dialog" aria-label="Keys" hidden></div>
    <div class="dialog small about" id="about" role="dialog" aria-label="About the PhiTeX preview" hidden><header>About PhiTeX<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><div>${w.about}</div></div>
    <div class="dialog" id="logview" role="dialog" aria-label="The build's log" hidden><header>The last build's log<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><pre id="logtext"></pre></div>
    <div class="dialog" id="report" role="dialog" aria-label="Debug report" hidden>
      <header>Debug report</header>
      <p>Anonymized: no document text, nothing you typed, no file names (file1.tex, \u2026), no project id. Read it before you send it.</p>
      <textarea id="reporttext" readonly spellcheck="false"></textarea>
      <div class="btns"><button type="button" class="btn" id="reportcopy">Copy</button><button type="button" class="btn" id="reportmail">Email</button><button type="button" class="btn" id="reportclose">Close</button></div>
    </div>
    <div class="offpill" id="offpill">Disconnected</div>
    <button type="button" class="byline" id="byline" title="About this preview">${w.byline}</button>
  </div>
</div>`.replace(/<span class="icon"(?: aria-hidden="true")?>(\w+)<\/span>/g, (_, n) => icon(n, n === "expand_more" || n === "refresh" ? 16 : 18));
    this.host = host2;
    host2.style.display = "none";
    (document.body ?? document.documentElement).append(host2);
    this.win = this.$(".win");
    const offer = (sel, on) => this.root.querySelectorAll(sel).forEach((e) => e.hidden = !on);
    offer(".if-compare, #b-compare, #x-compare", !!ev.onCompare);
    offer(".if-cache", !!ev.onClearCache);
    offer("#x-log", !!ev.onLog);
    offer("#x-contents", !!opts.outline);
    offer("#x-keys", !!opts.keys);
    offer("#b-find, #x-find", !!ev.onFind);
    offer("#dbg, #dbglabel", !!ev.onDebug);
    offer("#tour", !!ev.onTour);
    offer("#news", !!ev.onNews);
    const menus = [["#b-rec", "#m-rec"], ["#st", "#m-time"], ["#b-zoom", "#m-zoom"], ["#b-more", "#m-more"], ["#b-probs", "#logs"]];
    this.menus = menus;
    for (const [b, m] of menus)
      this.$(b).addEventListener("click", (e) => {
        e.stopPropagation();
        if (m === "#logs") return this.diagnostics();
        this.menu(m);
      });
    this.root.addEventListener("click", (e) => {
      const t = e.target;
      if (t.closest("[data-close]")) t.closest(".dialog").hidden = true;
      if (!t.closest(".menu, .tip")) this.menu(null);
    });
    this.$("#go").onclick = () => this.rebuild();
    this.$("#r-rebuild").onclick = () => (this.menu(null), this.rebuild());
    this.$("#r-clean").onclick = () => (this.menu(null), ev.onClean());
    this.$("#r-compare").onclick = this.$("#b-compare").onclick = this.$("#x-compare").onclick = () => (this.menu(null), ev.onCompare?.());
    this.$("#r-cache").onclick = () => (this.menu(null), ev.onClearCache?.());
    this.$("#pdf").onclick = this.$("#x-pdf").onclick = () => (this.menu(null), ev.onPdf());
    this.$("#x-log").onclick = () => (this.menu(null), void this.showLog());
    this.$("#x-contents").onclick = () => (this.menu(null), this.side());
    this.$("#x-find").onclick = this.$("#b-find").onclick = () => (this.menu(null), this.find(true));
    this.$("#x-tips").onclick = () => (this.menu(null), this.tips(true));
    this.$("#x-keys").onclick = () => {
      this.menu(null);
      this.$("#keyhelp").hidden = false;
    };
    this.$("#x-about").onclick = this.$("#byline").onclick = (e) => {
      e.stopPropagation();
      this.menu(null);
      this.$("#about").hidden = !this.$("#about").hidden;
    };
    this.$("#m-zoom").onclick = (e) => {
      const z = e.target.closest("[data-zoom]")?.dataset.zoom;
      if (z) this.menu(null), this.zoomTo(z);
    };
    this.$("#darkpdf").onclick = (e) => {
      const d = e.target.closest("[data-dark]")?.dataset.dark;
      if (d) this.darkPdf(d);
    };
    this.$("#logsx").onclick = () => this.diagnostics(false);
    const pg = this.$("#pgin");
    pg.onfocus = () => pg.select();
    pg.onkeydown = (e) => {
      if (e.key !== "Enter") return;
      const k = Number(pg.value) - 1;
      if (Number.isInteger(k) && k >= 0 && k < this.viewer.pages) this.viewer.goTo(k);
      pg.blur();
    };
    pg.onblur = () => this.nav();
    this.$("#dbg").onclick = () => this.toggleDebug();
    this.$("#dbg2").onchange = () => this.toggleDebug();
    this.$("#reload").onclick = () => (this.sheet(false), ev.onReload());
    const license = this.$("#license");
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    this.$("#news").onclick = () => {
      this.sheet(false);
      ev.onNews?.();
    };
    this.$("#tour").onclick = () => {
      this.sheet(false);
      ev.onTour?.();
    };
    this.$("#main").onchange = (e) => ev.onMain(e.target.value);
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#diags").onclick = (e) => {
      const at = e.target.closest(".phx-place[data-file]");
      if (at) {
        e.preventDefault();
        return ev.onGoto(at.dataset.file, Number(at.dataset.line) || 1);
      }
      const d = e.target.closest(".diag[data-line]");
      if (d) ev.onGoto(d.dataset.file, Number(d.dataset.line));
    };
    this.$("#stage").addEventListener("pointerdown", () => {
      this.menu(null);
      this.diagnostics(false);
      this.sheet(false);
      this.$("#about").hidden = true;
    });
    this.root.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const open = this.root.querySelector(".dialog:not([hidden])");
      if (open) open.hidden = true;
      else if (this.menus.some(([, m]) => !this.$(m).hidden)) this.menu(null);
      else this.escapeProblems();
    });
    this.$("#gonex").onclick = () => this.dismissOffline();
    addEventListener("keydown", (e) => e.key === "Escape" && this.$("#stage").classList.contains("offline") && this.dismissOffline());
    window.addEventListener("keydown", (e) => {
      if (e.altKey && e.shiftKey && e.code === "KeyP") {
        e.preventDefault();
        if (!ev.onShortcut?.()) this.collapse(!this.prefs.collapsed);
      }
    });
    this.$("#stage").addEventListener("wheel", (e) => e.ctrlKey && this.ownZoom(), { capture: true, passive: true });
    this.drag();
    this.viewer = new Viewer(this.$("#viewer"), this.$("#stage"), {
      need: (k) => ev.onNeed?.(k),
      inView: (k) => {
        this.at = k;
        this.contents?.at(k);
        this.nav();
        ev.onPage(k);
      },
      scale: () => this.scale(),
      box: (d, w2) => pageBox(d, w2),
      dbl: (k, x, y) => ev.onSyncSource?.(k, x, y),
      selected: (sel) => ev.onSelectPage?.(sel),
      link: ev.onLink ? (u) => ev.onLink(u) : void 0
    });
    this.extras(opts);
    this.darkPdf(this.darkPref(), false);
    window.addEventListener("resize", () => this.place());
    new ResizeObserver(() => {
      this.fitSheet();
      if (this.docked) return this.prefs.zoom === "fit" || this.zoomNow ? this.redraw() : void 0;
      if (this.prefs.collapsed) return;
      const r = this.win.getBoundingClientRect();
      if (r.width < 100 || r.height < 100) return;
      if (Math.abs(r.width - this.prefs.w) + Math.abs(r.height - this.prefs.h) > 2) {
        this.prefs.w = Math.round(r.width);
        this.prefs.h = Math.round(r.height);
        this.save();
        if (this.prefs.zoom === "fit") this.redraw();
      }
    }).observe(this.win);
    this.place();
    this.zoomShown();
    this.renderStatus();
    store?.load().then((p) => {
      this.prefs = { ...DEFAULTS, ...p };
      if (this.prefs.w < 300 || this.prefs.h < 180) Object.assign(this.prefs, { w: DEFAULTS.w, h: DEFAULTS.h });
      this.place();
    });
  }
  /** The menus and popovers by their buttons. */
  menus = [];
  /** Open the menu `m` (closing the others; open already: closed), or close all (null). */
  menu(m) {
    for (const [b, el] of this.menus) {
      const on = el === m && this.$(el).hidden;
      if (el === "#logs") continue;
      this.$(el).hidden = !on;
      this.$(b).setAttribute("aria-expanded", String(on));
      if (on && el === "#m-time") this.timing();
    }
    if (m !== "#logs" && m !== null) this.diagnostics(false);
  }
  /** Rebuild: the host's (the files as they are), or its reload. */
  rebuild() {
    (this.ev.onRebuild ?? this.ev.onReload).call(this.ev);
  }
  /**
  * The place on the pages for the source (Overleaf's ← arrow), as
  * `toSource` takes it: the last single click or selection on the pages
  * while still in view, else the top of the view; null with no page shown.
  */
  spot() {
    return this.viewer.spot();
  }
  /**
   * Dock into `pane` (after `after`, filling the rest of it), or float
   * (null): the fallback when the host's layout is not found.
   */
  dock(pane2, below) {
    const h = this.host.style;
    if (pane2) {
      const top = below ? Math.max(0, below.getBoundingClientRect().bottom - pane2.getBoundingClientRect().top) : 0;
      const cs = getComputedStyle(pane2);
      if (cs.position === "static") pane2.style.position = "relative";
      if (this.win.style.getPropertyValue("--pane") !== cs.backgroundColor) this.win.style.setProperty("--pane", cs.backgroundColor);
      this.win.classList.toggle("pdf-dark", pane2.classList.contains("pdf-dark-mode"));
      this.win.classList.toggle("light", document.body.dataset.theme === "light");
      this.applyDark();
      if (h.top !== `${top}px` || h.position !== "absolute") Object.assign(h, { position: "absolute", left: "0", right: "0", bottom: "0", top: `${top}px`, zIndex: "12", flexDirection: "column" });
    }
    if (pane2 === this.docked && this.host.isConnected) return;
    this.docked = pane2;
    if (pane2) {
      pane2.append(this.host);
      this.win.classList.add("docked");
      this.win.classList.remove("collapsed");
      this.win.setAttribute("role", "region");
    } else {
      (document.body ?? document.documentElement).append(this.host);
      Object.assign(h, { position: "", left: "", right: "", bottom: "", top: "", zIndex: "" });
      this.win.classList.remove("docked");
      this.win.setAttribute("role", "dialog");
      this.place();
    }
    this.redraw();
  }
  /** Shown or not (docked: the host's own PDF is showing instead). */
  shown(on) {
    const display = on ? this.docked ? "flex" : "" : "none";
    if (this.host.style.display === display && this.wasShown === on) return;
    const was = this.wasShown;
    this.wasShown = on;
    this.host.style.display = display;
    if (on && !was) this.redraw();
    if (on && !this.last) this.step(this.stepNow || this.words.steps[0], void 0, "");
  }
  wasShown;
  save() {
    this.store?.save(this.prefs);
  }
  place() {
    if (this.docked) return;
    const p = this.prefs, s = this.win.style;
    const x = p.x ?? window.innerWidth - p.w - 16, y = p.y ?? window.innerHeight - p.h - 16;
    s.left = `${Math.max(0, Math.min(x, window.innerWidth - 120))}px`;
    s.top = `${Math.max(0, Math.min(y, window.innerHeight - 40))}px`;
    this.zoomShown();
    this.collapse(p.collapsed, false);
  }
  collapse(on, save2 = true) {
    this.prefs.collapsed = on;
    this.win.classList.toggle("collapsed", on);
    const b = this.$("#min");
    b.title = on ? "Expand (Alt+Shift+P)" : "Collapse (Alt+Shift+P)";
    b.innerHTML = icon(on ? "open_in_full" : "close_fullscreen");
    this.win.style.width = on ? "" : `${this.prefs.w}px`;
    this.win.style.height = on ? "" : `${this.prefs.h}px`;
    if (!on) this.redraw();
    if (save2) this.save();
  }
  drag() {
    this.$("header").onpointerdown = (d) => {
      if (d.target.closest("button, .chip")) return;
      const r = this.win.getBoundingClientRect();
      const move = (m) => {
        this.prefs.x = Math.max(0, Math.min(r.left + m.clientX - d.clientX, window.innerWidth - 120));
        this.prefs.y = Math.max(0, Math.min(r.top + m.clientY - d.clientY, window.innerHeight - 40));
        this.win.style.left = `${this.prefs.x}px`;
        this.win.style.top = `${this.prefs.y}px`;
      };
      window.addEventListener("pointermove", move);
      window.addEventListener(
        "pointerup",
        () => {
          window.removeEventListener("pointermove", move);
          this.save();
        },
        { once: true }
      );
    };
  }
  /** Packages on their way after the first page, and a build running on: said in the bar, or in the corner. */
  pkgNow;
  buildSince = 0;
  actTick;
  prepSince = 0;
  /** A build has ended with pages: a later one is the quiet "updating", never "getting ready". */
  built = false;
  /** A build of the core's (`on`) or its end: past 400 ms the status says so. */
  preparing(on) {
    if (!this.words.preparing) return;
    if (!on && (this.last || this.viewer.pages > 0)) this.built = true;
    this.prepSince = on ? this.prepSince || performance.now() : 0;
    this.activity();
  }
  /** A streamed open's pages so far (before the first: the sheet's step; after: the status's page). */
  streamed;
  streaming(s) {
    this.streamed = s ? s.pages : void 0;
    if (s && this.firstPending()) this.step(this.words.steps[2], void 0, `Page ${s.pages + 1}`);
    this.activity();
  }
  busy(on) {
    if (!on && (this.last || this.viewer.pages > 0)) this.built = true;
    this.buildSince = on ? this.buildSince || performance.now() : 0;
    this.activity();
  }
  /** The bar's status, the progress line, and in a host without the bar the corner's note: never silent while the writer waits. */
  activity() {
    const p = this.pkgNow;
    const now = performance.now();
    const prep = !!this.prepSince && now - this.prepSince > 400;
    const long = !!this.buildSince && now - this.buildSince > 400 || prep;
    if (long || this.buildSince || this.prepSince || p) this.actTick ??= setInterval(() => this.activity(), 250);
    else clearInterval(this.actTick), this.actTick = void 0;
    this.renderStatus();
    if (!this.last && this.viewer.pages === 0) return;
    if (p) {
      const names = p.names.slice(0, 3).join(", ") + (p.names.length > 3 ? ` and ${p.names.length - 3} more` : "");
      return this.toast("pkgs", { title: `Fetching ${names}`, detail: `${p.done} of ${p.total} files${known(p.source) ? ` \xB7 from ${p.source}` : ""}`, pct: p.pct, icon: "download" });
    }
    if (this.toastKind === "pkgs") this.toast("pkgs", null);
    if (this.ownBar) return;
    if (long && this.words.preparing) {
      const s = (now - (this.buildSince || this.prepSince)) / 1e3 | 0;
      const what = this.built ? this.words.updating ?? "Updating the other pages\u2026" : this.streamed ? `Typesetting page ${this.streamed + 1}\u2026` : "Typesetting\u2026";
      this.toast("build", { title: what, detail: s >= 2 ? `${s} s` : "", pct: null, icon: "refresh" });
    } else if (this.toastKind === "build") this.toast("build", null);
  }
  /** The bar's status: what is running, or how the last build went and how fast. */
  renderStatus() {
    const st = this.$("#st"), text = this.$("#sttext"), more = this.$("#stmore");
    const now = performance.now();
    const running = !!this.buildSince && now - this.buildSince > 400 || !!this.prepSince && now - this.prepSince > 400 || !!this.pkgNow;
    const errs = this.lastStatus?.diagnostics?.filter((d) => d.severity === "error").length ?? 0;
    let t, m = "", err = false;
    if (this.offline) t = "Disconnected", err = true;
    else if (this.pkgNow) t = "Fetching packages\u2026", m = `${this.pkgNow.done} of ${this.pkgNow.total}`;
    else if (running) {
      const s = (now - (this.buildSince || this.prepSince)) / 1e3;
      t = this.built ? "Building\u2026" : this.streamed ? `Typesetting page ${this.streamed + 1}\u2026` : "Building\u2026";
      m = s >= 1 ? `${s < 10 ? s.toFixed(1) : Math.round(s)} s` : "";
    } else if (this.warm) t = "Preparing instant typing\u2026", m = `${Math.round((Date.now() - this.warm.since) / 1e3)} s`;
    else if (!this.last && this.viewer.pages === 0 && !this.$("#empty").hidden) t = errs ? `Stopped with ${errs} error${errs === 1 ? "" : "s"}` : "No page yet", err = !!errs;
    else if (!this.last && this.viewer.pages === 0) t = this.stepNow || "Opening\u2026";
    else if (errs) t = `Built with ${errs} error${errs === 1 ? "" : "s"}`, err = true;
    else t = "Built", m = this.speed ? `${this.speed.ms < 10 ? this.speed.ms.toFixed(1) : Math.round(this.speed.ms)} ms` : "";
    text.textContent = t;
    more.textContent = m ? `\xB7 ${m}` : "";
    st.classList.toggle("err", err);
    this.$("#prog").classList.toggle("on", running || !this.last && this.viewer.pages === 0 && !this.offline && this.$("#empty").hidden === true);
    this.$("#split").classList.toggle("busy", running);
  }
  /** Packages still on their way before the first page, so a stop reads as one only once they are in. */
  pkgBusy = false;
  /**
   * Before any page: a build that stopped at an error says so, in place of
   * the sheet (a stopped job's PDF is unfinished: no page to draw).
   */
  stopped() {
    if (this.engineNow && !this.engineNow.ready) return this.engineCard(this.engineNow.engine, null);
    if (this.last || this.pkgBusy) return false;
    const e = this.lastStatus?.diagnostics?.find((d) => d.severity === "error");
    if (!e) return false;
    const needed = errorNeeds(e.message);
    if (needed) return this.engineCard(needed, e.message);
    const card = this.card();
    if (e.code === "engine") {
      card.querySelector("h3").textContent = "PhiTeX's engine stopped";
      card.querySelector(".count").textContent = e.message;
      card.querySelector(".note").textContent = "Your document is probably fine. Save again to retry.";
      if (e.detail) card.append(foldedDetail(e.detail, "Engine details"));
      this.reportLink(card);
      return true;
    }
    card.querySelector("h3").textContent = "The build stopped before a page could be shown";
    card.querySelector(".count").textContent = e.message + (e.file && e.line ? ` (${e.file}:${e.line})` : "");
    card.querySelector(".note").textContent = this.words.stopped;
    this.reportLink(card);
    return true;
  }
  /** The card in place of the pages (the sheet gone): its heading, its line, its note. */
  card() {
    this.stopSheet();
    const empty = this.$("#empty");
    empty.innerHTML = `<div class="card"><h3></h3><div class="count"></div><div class="note"></div></div>`;
    empty.hidden = false;
    queueMicrotask(() => this.renderStatus());
    return empty.querySelector(".card");
  }
  /** "Copy debug info" under a card, if reports are wired. */
  reportLink(card) {
    if (!this.ev.onReport) return;
    const b = document.createElement("button");
    b.className = "reportbtn";
    b.textContent = "Copy debug info\u2026";
    b.onclick = () => void this.report();
    card.append(b);
  }
  /** The engine the session runs, as it last said. */
  engineNow;
  /** A XeLaTeX project shown approximated with pdfLaTeX (fonts substituted). */
  approx = false;
  engine(e) {
    this.engineNow = e;
    this.approx = !!e.approx;
    const a = this.$("#approx");
    a.classList.toggle("on", this.approx);
    a.textContent = `Approximate: needs ${ENGINES[e.engine].label}, shown with pdfLaTeX (fonts substituted, breaks may differ). ${this.words.realPdf}`;
    if (!e.ready) this.stopped();
    else if (this.$("#empty").querySelector(".engine")) this.$("#empty").hidden = true;
  }
  /**
   * The project needs `want` (its preamble, or `error`, a build that
   * stopped, says so): which engine to run it with, as buttons.
   */
  engineCard(want, error) {
    const ready = ENGINES[want].ready;
    const card = this.card();
    card.classList.add("engine");
    card.style.setProperty("--sev", "var(--ph-info)");
    card.insertAdjacentHTML("beforeend", `<div class="btns"></div>`);
    card.querySelector("h3").textContent = error ? `This project needs ${want === "lualatex" ? "LuaLaTeX" : "XeLaTeX or LuaLaTeX"}` : `This project runs with ${ENGINES[want].label}`;
    card.querySelector(".count").textContent = error ?? "Its preamble loads a package that pdfLaTeX can't run (fontspec, unicode-math, polyglossia, \u2026), or the engine was chosen for it.";
    card.querySelector(".note").textContent = ready ? `Run it with ${ENGINES[want].label}?` : `\u26A1 Instant doesn't run ${ENGINES[want].label} yet: only pdfLaTeX. ${this.words.notReady}, or try pdfLaTeX if the project can do without those packages.`;
    const btns = card.querySelector(".btns");
    const offer = this.engineNow?.engine === want ? ["pdflatex"] : want === "xelatex" ? ["xelatex", "lualatex"] : [want];
    for (const e of offer) {
      const b = document.createElement("button");
      b.className = "btn";
      b.textContent = e === "pdflatex" ? "Try pdfLaTeX anyway" : `Use ${ENGINES[e].label}${ENGINES[e].ready ? "" : " (coming)"}`;
      b.title = e === "pdflatex" ? "For this project: pdfLaTeX, whatever its preamble loads" : `For this project from now on (Settings \u2192 Engine for every project)`;
      b.onclick = () => this.ev.onEngine?.(e);
      btns.append(b);
    }
    return true;
  }
  /** The level and line a host's toolbar shows (ViewState's chip). */
  chip = { level: "ok", text: "starting\u2026", title: "Status" };
  status(s) {
    this.lastStatus = s;
    queueMicrotask(() => this.stopped());
    const diags = s.diagnostics ?? [];
    const worst = diags[0]?.severity;
    const partial = !!s.pending || s.pages === 0;
    this.chip = {
      level: worst === "error" ? "error" : partial || worst === "warning" ? "warning" : "ok",
      text: `${s.pages} page${s.pages === 1 ? "" : "s"}${s.pending ? " \xB7 partial" : ""}`,
      title: (s.pending ? `${s.pending} parts not read (out of fuel or unsupported). ` : "") + (diags.length ? `${diags.length} diagnostic${diags.length === 1 ? "" : "s"}` : "No problems found") + (s.file ? ` \xB7 editing ${s.file}` : "")
    };
    const list = this.$("#diags");
    list.innerHTML = "";
    for (const d of diags.slice(0, 50)) {
      if (d.problem) {
        const card = document.createElement("ol");
        card.className = "phx-card phx-inline";
        card.setAttribute("role", "listitem");
        card.innerHTML = problemHtml(d.problem);
        list.append(card);
        continue;
      }
      const row = document.createElement("div");
      row.className = `diag ${d.severity}`;
      row.setAttribute("role", "listitem");
      if (d.file && d.line) {
        row.dataset.file = d.file;
        row.dataset.line = String(d.line);
        row.title = `Go to ${d.file}:${d.line}`;
      }
      row.innerHTML = `<span class="where"></span><div class="text"></div>`;
      row.querySelector(".text").textContent = d.message;
      if (d.detail && d.code === "engine") row.append(foldedDetail(d.detail, "Engine details"));
      else if (d.detail) {
        const pre = document.createElement("pre");
        pre.className = "detail";
        pre.textContent = d.detail;
        row.append(pre);
      }
      row.querySelector(".where").textContent = d.file ? `${d.file}${d.line ? `, line ${d.line}` : ""}` : "";
      list.append(row);
    }
    if (diags.length > 50) {
      const more = document.createElement("div");
      more.className = "diag info";
      more.textContent = `\u2026 and ${diags.length - 50} more`;
      list.append(more);
    }
    if (this.ev.onReport) {
      const b = document.createElement("button");
      b.className = "reportbtn";
      b.textContent = "Report a problem\u2026";
      b.onclick = (e) => (e.stopPropagation(), void this.report());
      list.append(b);
    }
    const count = (sev) => diags.filter((d) => d.severity === sev).length;
    const errors = count("error"), warnings = count("warning");
    const pb = this.$("#b-probs");
    pb.hidden = !errors && !warnings;
    pb.className = `ib probs ${errors ? "error" : "warning"}`;
    pb.title = `${errors} error${errors === 1 ? "" : "s"}, ${warnings} warning${warnings === 1 ? "" : "s"}`;
    this.$("#psev").innerHTML = icon(errors ? "error" : "warning", 16);
    this.$("#pcount").textContent = String(errors || warnings);
    this.$("#logshead").textContent = diags.length ? [errors && `${errors} error${errors === 1 ? "" : "s"}`, warnings && `${warnings} warning${warnings === 1 ? "" : "s"}`].filter(Boolean).join(", ") || "Notes" : "No problems";
    const nopages = s.pages === 0 && !this.last;
    const key = this.problemsKey();
    if (key !== this.shownKey) {
      this.shownKey = key;
      this.diagnostics(nopages && errors > 0 && key !== this.dismissed);
    }
    if (!diags.length) this.diagnostics(false);
    this.stale(s);
    this.renderStatus();
    queueMicrotask(() => this.emit());
  }
  /** The reading step's last state, and when the sheet last changed (redrawn at most every 50 ms). */
  readNow;
  readDrawn = 0;
  readTimer;
  loadDone = -1;
  loadPct = 0;
  /**
   * Reading the project, before the packages: what is happening (the
   * download from Overleaf, each file unpacked, each figure handed to the
   * engine), the file in hand and how far, so the first seconds read as work.
   */
  reading(r) {
    if (this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readNow = r;
    const wait = 50 - (performance.now() - this.readDrawn);
    if (wait > 0) {
      this.readTimer ??= setTimeout(() => (this.readTimer = void 0, this.drawReading()), wait);
      return;
    }
    this.drawReading();
  }
  drawReading() {
    const r = this.readNow;
    if (!r || this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readDrawn = performance.now();
    const pct = r.pct ?? (r.total ? Math.round(100 * (r.done ?? 0) / r.total) : void 0);
    const count = r.total ? `${r.done ?? 0} of ${r.total}` : "";
    this.step(r.h, pct, [r.detail, count].filter(Boolean).join(" \xB7 "));
  }
  packages(p) {
    const done = p.done?.length ?? 0;
    const total = done + p.loading.length;
    if (done !== this.loadDone) this.loadDone = done;
    const pct = this.loadPct = Math.max(this.loadPct, total ? Math.round(100 * done / total) : 100);
    if (!this.last) {
      if (!p.loading.length && !p.building) {
        this.pkgBusy = false;
        if (!this.stopped()) this.step(this.words.steps[2], void 0, `with ${done} package file${done === 1 ? "" : "s"}`);
        return;
      }
      this.pkgBusy = true;
      if (p.loading.length) {
        const name = p.loading[0];
        this.step(this.words.steps[1], pct, `${name} \xB7 ${done} of ${total}${p.unavailable.length ? ` \xB7 ${p.unavailable.length} not found` : ""}`);
      } else this.step(this.words.steps[2], void 0, `with ${done} package file${done === 1 ? "" : "s"}; a build may ask for more`);
      return;
    }
    this.pkgNow = p.loading.length ? { names: p.loading, done, total, pct, source: p.source, failed: p.failed ?? [] } : void 0;
    if (p.failed?.length && !p.loading.length) {
      const f = p.failed[0];
      this.toast("pkgs", { title: `Couldn't fetch ${f.name}`, detail: `${f.error}. Asked again at the next build.`, pct: null, icon: "warning", err: true });
    }
    this.activity();
  }
  /**
   * The first page's step `k` (0, 1, 2 of the words' steps), saying `text`
   * (the host's own phases: the CLI's build); nothing once a page is shown.
   */
  loadStep(k, text) {
    if (this.last) return;
    this.step(this.words.steps[k] ?? "", void 0, text);
  }
  // ---- The first page's sheet: the opening playing on it, the step under it ----
  /** The opening on the sheet (null: none playing), and whether it has played out once. */
  anim = null;
  opened = false;
  ending = false;
  /** The step the sheet says now (the status says it too). */
  stepNow = "";
  /** The zoom the opening set (the whole page, so the page that arrives keeps the sheet's height), until the user zooms. */
  zoomNow = null;
  /** Before any page and no card: the sheet is (or now goes) up. */
  firstPending() {
    return !this.last && this.viewer.pages === 0 && this.$("#empty").hidden === true;
  }
  /** The sheet's step: what, how far (undefined: not known), and the detail; the sheet put up first if it isn't. */
  step(what, pct, detail = "") {
    if (!this.firstPending()) return;
    this.sheetUp();
    const el = this.$("#opening");
    this.stepNow = what;
    el.querySelector(".what").textContent = what;
    el.querySelector(".detail").textContent = detail;
    const track = el.querySelector(".track");
    track.classList.toggle("unknown", pct === void 0);
    track.querySelector("i").style.width = pct === void 0 ? "" : `${pct}%`;
    this.renderStatus();
  }
  /** The sheet up, the opening playing on it (once per panel: a later rebuild has pages). */
  sheetUp() {
    if (this.opened || this.anim || !this.wasShown) return;
    const el = this.$("#opening");
    el.hidden = false;
    el.classList.remove("in");
    this.win.classList.add("sheet-up");
    if (this.prefs.zoom === "fit") this.zoomNow = "page";
    this.fitSheet();
    const dark = this.win.classList.contains("dark-pdf") || this.win.classList.contains("docked") && this.win.classList.contains("pdf-dark");
    this.anim = opening(el.querySelector(".sheet"), dark ? "#e4e6ea" : "#1d3e72", dark ? "#2c2e33" : "#ffffff");
    this.animSince = performance.now();
  }
  /** When the opening began: a page there within 400 ms (a project already built) is shown at once, no opening. */
  animSince = 0;
  /** The sheet as the first page will be: an A4 page, whole in the stage, where the viewer puts page 1. */
  fitSheet() {
    const el = this.$("#opening");
    if (el.hidden || this.ending) return;
    const st = this.$("#stage");
    const room = { w: st.clientWidth - 24, h: st.clientHeight - 24 };
    let h = room.h, w = h * 210 / 297;
    if (w > room.w) w = room.w, h = w * 297 / 210;
    Object.assign(el.style, { left: `${st.offsetLeft + (st.clientWidth - w) / 2}px`, top: `${st.offsetTop + 12}px`, width: `${w}px`, height: `${h}px` });
  }
  /**
   * The first page is in: the sheet eases to the page's box (its height
   * kept, the zoom being the whole page: only its width changes), the logo
   * settles and dives into the page beneath, and the sheet is gone.
   */
  sheetDown() {
    if (!this.anim || this.ending) return;
    if (performance.now() - this.animSince < 400) return this.stopSheet();
    this.ending = true;
    const el = this.$("#opening");
    requestAnimationFrame(() => {
      const slot = this.$("#viewer").querySelector(".slot");
      if (slot) {
        const r = slot.getBoundingClientRect(), b = this.$(".body").getBoundingClientRect();
        Object.assign(el.style, { left: `${r.left - b.left}px`, top: `${r.top - b.top}px`, width: `${r.width}px`, height: `${r.height}px` });
      }
      el.classList.add("in");
      setTimeout(() => {
        void this.anim?.done().then(() => this.stopSheet());
      }, matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 300);
    });
  }
  /** The sheet gone (played out, or a card in its place). */
  stopSheet() {
    const was = !!this.anim;
    this.anim?.stop();
    this.anim = null;
    this.ending = false;
    this.$("#opening").hidden = true;
    this.win.classList.remove("sheet-up");
    if (was) {
      this.opened = true;
      this.stepNow = "";
      this.renderStatus();
      setTimeout(() => this.tips(false), 600);
    }
  }
  /** The zoom the user has from here: the opening's whole page made theirs (a gesture starts from it), the sheet's ease stopped. */
  ownZoom() {
    if (!this.zoomNow) return;
    this.prefs.zoom = this.zoomNow;
    this.zoomNow = null;
  }
  // ---- The corner's note ----
  toastKind = "";
  toastTimer;
  /** A note in the corner, of `kind` (a later one replaces it), or that kind's gone (null). */
  toast(kind, t) {
    const el = this.$("#toast");
    if (!t) {
      if (this.toastKind !== kind) return;
      this.toastKind = "";
      el.classList.add("out");
      clearTimeout(this.toastTimer);
      this.toastTimer = setTimeout(() => !this.toastKind && (el.hidden = true), 300);
      return;
    }
    clearTimeout(this.toastTimer);
    this.toastKind = kind;
    el.className = `toast${t.err ? " err" : ""}`;
    el.hidden = false;
    el.innerHTML = `<div class="t">${t.icon ? icon(t.icon, 16) : ""}<span></span></div><div class="d"></div>` + (t.pct === null ? "" : `<div class="track"><i style="width:${t.pct}%"></i></div>`);
    el.querySelector(".t span").textContent = t.title;
    el.querySelector(".d").textContent = t.detail ?? "";
    if (t.ms) this.toastTimer = setTimeout(() => this.toast(kind, null), t.ms);
  }
  /** Keep the last good page when a build ships nothing (or not this page): dimmed, with why. */
  /** When the page first went missing (a build that shipped none); its dimming waits. */
  staleSince = 0;
  staleTimer;
  /**
   * Dim the last good page when a build ships none, but only once that has
   * lasted STALE_GRACE_MS: typing `\section{B…` is an error at every
   * keystroke until the `}`, and the page shouldn't flash grey for each.
   */
  stale(s) {
    const stage = this.$("#stage");
    const gone = (s.pages === 0 || this.at >= s.pages) && !!this.last;
    clearTimeout(this.staleTimer);
    if (!gone) {
      this.staleSince = 0;
      stage.classList.remove("stale");
      return;
    }
    const now = performance.now();
    this.staleSince ||= now;
    const left = STALE_GRACE_MS - (now - this.staleSince);
    if (left > 0) {
      this.staleTimer = setTimeout(() => this.lastStatus && this.stale(this.lastStatus), left);
      return;
    }
    stage.classList.add("stale");
    const why = s.diagnostics?.find((d) => d.severity !== "info" && d.code !== "no-pages");
    this.$("#banner").textContent = `Showing the last complete render: ${why ? why.message + (why.line ? ` (${why.file}:${why.line})` : "") : "this build shipped no page"}.`;
  }
  /**
   * The connection to the engine lost (`up` false: the watch stopped or
   * exited, the socket closed, the core trapped) or back: while it is lost
   * the last pages stay, dimmed and inert, under a banner saying so and why.
   */
  connection(up, why) {
    const was = this.offline;
    this.offline = !up;
    this.renderStatus();
    if (up) {
      this.$("#stage").classList.remove("offline");
      this.win.classList.remove("offline-quiet");
      return;
    }
    const text = `Disconnected from the engine: the pages are the last ones built${why ? ` (${why})` : ""}.`;
    this.$("#gonewhy").textContent = text;
    this.$("#offpill").title = text;
    if (!was || !this.win.classList.contains("offline-quiet")) this.$("#stage").classList.add("offline");
  }
  /** The disconnection's banner dismissed (✕ or Esc): the pages interactive again, a small pill saying so. */
  dismissOffline() {
    if (!this.offline) return;
    this.$("#stage").classList.remove("offline");
    this.win.classList.add("offline-quiet");
    const v = this.$("#viewer");
    this.$("#offpill").style.right = `${Math.max(0, v.offsetWidth - v.clientWidth) + 8}px`;
  }
  offline = false;
  /** Follow what the panel shows (a host toolbar's controls). */
  onState(cb) {
    this.listeners.push(cb);
    cb(this.state());
  }
  state() {
    return {
      page: this.at,
      pages: this.lastStatus?.pages ?? 0,
      zoom: this.zoomNow ?? this.prefs.zoom,
      debug: this.debugOn,
      level: this.chip.level,
      chip: this.chip.text,
      chipTitle: this.chip.title,
      speed: this.speed,
      sheet: !this.$("#sheet").hidden,
      // (the badge counts what needs a look: info is listed, not counted)
      diagCount: this.lastStatus?.diagnostics?.filter((d) => d.severity !== "info").length ?? 0,
      diagWorst: this.lastStatus?.diagnostics?.[0]?.severity ?? "",
      percent: Math.round(this.scale() * 100)
    };
  }
  emit() {
    const s = this.state();
    for (const f of this.listeners) f(s);
  }
  /**
   * The zoom, 1 being 100% (a PDF point at 96/72 CSS pixels). Fit is the
   * page's width, as Overleaf's viewer fits it: (clientWidth − 40) / the
   * page's width, 40 being its scrollbar's padding; the whole page, its
   * height in the stage's (12 px above and below) too.
   */
  /** Device pixels per PDF point at the current zoom: the size pages are drawn at. */
  pixelsPerPoint() {
    return 96 / 72 * this.scale() * (window.devicePixelRatio || 1);
  }
  scale() {
    const z = this.zoomNow ?? this.prefs.zoom;
    if (z !== "fit" && z !== "page") return Number(z);
    const st = this.$("#stage");
    const width = Math.max(st.clientWidth - 40, 200) / ((this.last?.w ?? 595) * (96 / 72));
    if (z === "fit") return width;
    return Math.min(width, Math.max(st.clientHeight - 24, 100) / ((this.last?.h ?? 842) * (96 / 72)));
  }
  goPage(k) {
    this.viewer.goTo(k);
  }
  /** One zoom step in or out, from where it is (as Overleaf's − +). */
  zoomStep(dir) {
    const steps = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];
    const now = this.scale();
    const next = dir > 0 ? steps.find((z) => z > now + 0.01) : [...steps].reverse().find((z) => z < now - 0.01);
    this.zoomTo(String(next ?? now));
  }
  /** The contents beside the pages (PanelOptions.outline). */
  contents;
  /** Whether the contents are shown: the session asks for the outline after a build only then. */
  outlineWanted() {
    return !!this.contents && this.win.classList.contains("side-on");
  }
  /** The document's outline (its PDF's bookmarks), from the host after each build. */
  outline(items) {
    this.contents?.set(items);
    this.contents?.at(this.at);
    this.noOutline(!items.length);
  }
  /** No outline (none yet, or a PDF without one): the column hidden, the menu's Contents saying so. */
  noOutline(none) {
    const was = this.win.classList.contains("side-empty");
    this.win.classList.toggle("side-empty", none);
    if (was !== none && this.win.classList.contains("side-on") && this.prefs.zoom === "fit") this.redraw();
    const label = this.$("#x-contents").querySelector("span:not(.sub)");
    if (label) label.textContent = none ? "Contents (none)" : "Contents";
    this.$("#x-contents").title = none ? "This PDF has no outline: hyperref makes one from the sections" : "";
  }
  /** The contents shown or hidden (remembered on this machine). */
  side(on = !this.win.classList.contains("side-on")) {
    if (!this.contents) return;
    this.win.classList.toggle("side-on", on);
    if (on) this.ev.onContents?.();
    try {
      localStorage.setItem("phitex.side", on ? "1" : "0");
    } catch {
    }
    if (this.prefs.zoom === "fit") this.redraw();
  }
  /** A button of the host's in the bar, before find (the CLI's own). */
  headerButton(label, title, onclick) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "ib wide";
    b.title = title;
    b.textContent = label;
    b.onclick = () => onclick(b);
    this.$("#b-find").before(b);
    return b;
  }
  /** The contents and the keys, as the host asked (PanelOptions). */
  extras(opts) {
    if (opts.header) this.win.classList.add("own-header");
    this.findAndZoom(!!opts.keys);
    if (opts.outline) {
      this.win.classList.add("has-side");
      this.contents = new Outline(this.$("#side"), { goToPlace: (k, top) => this.viewer.goToPlace(k, top) });
      let on = true;
      try {
        on = localStorage.getItem("phitex.side") !== "0";
      } catch {
      }
      this.win.classList.toggle("side-on", on);
      this.noOutline(true);
    }
    if (!opts.keys) return;
    const help = this.$("#keyhelp");
    const extra = [["e", "the build's errors and warnings"], ...opts.outline ? [["t", "the contents beside the pages"]] : []];
    help.innerHTML = `<header>Keyboard shortcuts<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><table>${[...KEYS, ...extra].map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
    addEventListener(
      "keydown",
      (e) => {
        if (e.ctrlKey || e.altKey || e.metaKey || e.target?.closest?.("input, textarea, select, [contenteditable=true]")) return;
        if (e.key === "Escape" && this.escapeProblems()) {
        } else if (e.key === "e") this.diagnostics();
        else if (e.key === "t" && this.contents) this.side();
        else return;
        e.preventDefault();
        e.stopImmediatePropagation();
      },
      { capture: true }
    );
    const stage = this.$("#stage");
    const pageH = () => {
      return (this.last?.h ?? 792) * (96 / 72);
    };
    const panel2 = this;
    bindKeys(window, {
      get pages() {
        return panel2.viewer.pages;
      },
      get page() {
        return panel2.viewer.page;
      },
      goTo: (k) => this.viewer.goTo(k),
      turn: (n) => this.viewer.turn(n),
      scroller: stage,
      zoom: (f) => this.zoomTo(String(Math.min(5, Math.max(0.25, this.scale() * f)))),
      fitWidth: () => this.zoomTo("fit"),
      fitPage: () => this.zoomTo(String(Math.max(0.2, Math.min((stage.clientHeight - 24) / pageH(), this.scale())))),
      help: (show) => {
        help.hidden = !show;
      }
    });
  }
  /** The problems last closed by hand (not opened by themselves again until they change), and those listed. */
  dismissed = "";
  shownKey = "";
  problemsKey() {
    return JSON.stringify((this.lastStatus?.diagnostics ?? []).map((d) => [d.severity, d.message, d.file, d.line]));
  }
  /** The problems' list opened or closed. */
  diagnostics(on = this.$("#logs").hidden === true) {
    const was = !this.$("#logs").hidden;
    if (on && !(this.lastStatus?.diagnostics ?? []).length && !was) on = false;
    if (on) this.menu(null);
    this.$("#logs").hidden = !on;
    this.$("#b-probs").setAttribute("aria-expanded", String(on));
    this.win.classList.toggle("diags-open", on);
    if (was && !on) this.dismissed = this.problemsKey();
    if (was !== on) this.emit();
  }
  /** Esc on the problems: the list closed; false when it wasn't open. */
  escapeProblems() {
    if (this.$("#logs").hidden) return false;
    this.diagnostics(false);
    return true;
  }
  prev() {
    this.viewer.goTo(this.at - 1);
  }
  next() {
    this.viewer.goTo(this.at + 1);
  }
  clean() {
    this.ev.onClean();
  }
  pdf() {
    this.ev.onPdf();
  }
  zoomTo(z) {
    this.zoomNow = null;
    this.prefs.zoom = z;
    this.zoomShown();
    this.save();
    this.redraw();
    this.emit();
  }
  /** The zoom menu at the zoom now: a zoom not on it (a pinch's) by its percentage. */
  zoomShown() {
    const z = this.zoomNow ?? this.prefs.zoom;
    const named = ZOOMS.find(([v]) => v === z);
    this.$("#zl").textContent = named ? named[1] : `${Math.round(Number(z) * 100)}%`;
    this.root.querySelectorAll("#m-zoom [data-zoom]").forEach((b) => {
      const on = b.dataset.zoom === z;
      b.setAttribute("aria-checked", String(on));
      b.querySelector(".check").textContent = on ? "\u2713" : "";
    });
  }
  /**
  * A zoom gesture under way (Ctrl+wheel, a pinch): the zoom it started
  * at, the factor so far, the place under the pointer when it began
  * (client pixels, and on its page), and when it ends.
  */
  gesture;
  /**
   * Zoom by `f` keeping the point (`px`, `py`, from the stage's top left)
   * where it is, as pdf.js's and hayro's viewers do: while the gesture goes
   * on, the pages as they are scaled by one transform around the pointer
   * (no layout, no drawing: the pages, their marks and their text scale
   * together); once it rests (250 ms, longer than a wheel's notches are
   * apart), the zoom set once, the place kept under the pointer by its
   * page, and the pages drawn once at it.
   */
  zoomAt(f, px, py) {
    const stage = this.$("#stage"), view = this.$("#viewer");
    const r = stage.getBoundingClientRect();
    let g = this.gesture;
    if (!g) {
      const x = r.left + px, y = r.top + py;
      const v = view.getBoundingClientRect();
      view.style.transformOrigin = `${x - v.left}px ${y - v.top}px`;
      g = this.gesture = { from: this.scale(), f: 1, x, y, at: this.viewer.pointAt(x, y) };
    }
    g.f = Math.min(8, Math.max(0.25, g.from * g.f * f)) / g.from;
    view.style.transform = `scale(${g.f})`;
    view.style.willChange = "transform";
    clearTimeout(g.end);
    g.end = setTimeout(() => this.zoomEnd(), 250);
  }
  /** The zoom gesture rested: the pages at the zoom, the place under the pointer kept, drawn once. */
  zoomEnd() {
    const g = this.gesture;
    if (!g) return;
    this.gesture = void 0;
    const view = this.$("#viewer");
    view.style.transform = "";
    view.style.willChange = "";
    this.prefs.zoom = String(Number((g.from * g.f).toFixed(4)));
    this.zoomShown();
    this.$("#stage").classList.remove("fit");
    this.viewer.redraw(true);
    if (g.at) this.viewer.keepAt(g.at, g.x, g.y);
    this.viewer.redraw();
    this.save();
    this.emit();
  }
  /** The find field's matches, the one shown, and the search asked last. */
  hits = [];
  hit = 0;
  findSeq = 0;
  findTimer;
  /** The pointer over the panel (in a host's page, its keys only then). */
  hover = false;
  /** The find field opened (its text selected) or closed (the marks gone). */
  find(on) {
    const box = this.$("#find");
    const q = this.$("#findq");
    box.hidden = !on;
    if (on) {
      q.focus();
      q.select();
      if (q.value) this.search(q.value);
      return;
    }
    this.findSeq++;
    this.hits = [];
    this.viewer.found([], 0);
    q.blur();
  }
  search(q) {
    const seq = ++this.findSeq;
    const n = this.$("#findn");
    if (!q.trim()) {
      this.hits = [];
      this.viewer.found([], 0);
      n.textContent = "";
      return;
    }
    void this.ev.onFind?.(q).then((hits) => {
      if (seq !== this.findSeq) return;
      this.hits = hits;
      const at = hits.findIndex((h) => h.k >= this.viewer.page);
      this.hit = at < 0 ? 0 : at;
      this.showHit();
    });
  }
  showHit() {
    const n = this.$("#findn");
    n.textContent = this.hits.length ? `${this.hit + 1} / ${this.hits.length}` : "0 / 0";
    n.classList.toggle("none", !this.hits.length);
    this.viewer.found(this.hits, this.hit);
  }
  /** Matches found by the host (a diff's change): marked, `cur` brought into view; [] clears. */
  reveal(hits, cur = 0) {
    this.viewer.found(hits, cur);
  }
  /** The next match (`dir` 1) or the one before (−1), round the ends. */
  findNext(dir) {
    if (!this.hits.length) return;
    this.hit = (this.hit + dir + this.hits.length) % this.hits.length;
    this.showHit();
  }
  /** Find, and zoom by the wheel and its keys: in every host (`all`: the page's keys are the panel's). */
  findAndZoom(all) {
    const stage = this.$("#stage");
    this.win.addEventListener("pointerenter", () => this.hover = true);
    this.win.addEventListener("pointerleave", () => this.hover = false);
    stage.addEventListener(
      "wheel",
      (e) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        const r = stage.getBoundingClientRect();
        const notch = e.deltaMode !== 0 || Math.abs(e.deltaY) >= 50;
        const f = notch ? e.deltaY < 0 ? 1.1 : 1 / 1.1 : Math.exp(-e.deltaY / 100);
        this.zoomAt(f, e.clientX - r.left, e.clientY - r.top);
      },
      { passive: false }
    );
    const q = this.$("#findq");
    q.addEventListener("input", () => {
      clearTimeout(this.findTimer);
      this.findTimer = setTimeout(() => this.search(q.value), 120);
    });
    q.addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.findNext(e.shiftKey ? -1 : 1);
      else if (e.key === "Escape") this.find(false);
      else return;
      e.preventDefault();
      e.stopPropagation();
    });
    addEventListener(
      "keydown",
      (e) => {
        const ours = all || this.hover || this.root.activeElement != null;
        if (!ours || e.altKey || e.metaKey) return;
        const typing = e.target?.closest?.("input, textarea, select, [contenteditable=true]") && e.composedPath()[0] !== q;
        if (e.ctrlKey) {
          const k = e.key;
          if (k === "f" && this.ev.onFind) this.find(true);
          else if (k === "=" || k === "+") this.zoomStep(1);
          else if (k === "-") this.zoomStep(-1);
          else if (k === "0") this.zoomTo("fit");
          else return;
        } else if (typing || e.composedPath()[0] === q) return;
        else if (e.key === "/" && this.ev.onFind) this.find(true);
        else if ((e.key === "n" || e.key === "N") && !this.$("#find").hidden && this.hits.length) this.findNext(e.key === "n" ? 1 : -1);
        else if (e.key === "Escape" && !this.$("#find").hidden) this.find(false);
        else return;
        e.preventDefault();
        e.stopImmediatePropagation();
      },
      { capture: true }
    );
  }
  toggleDebug() {
    this.debugOn = !this.debugOn;
    this.$("#dbg").setAttribute("aria-checked", String(this.debugOn));
    this.$("#dbg").querySelector(".check").textContent = this.debugOn ? "\u2713" : "";
    this.$("#dbg2").checked = this.debugOn;
    this.ev.onDebug?.(this.debugOn);
    this.msg(this.debugOn ? "Debug check on" : "Debug check off");
    this.emit();
  }
  /** The settings (docked under a host's toolbar: the main file, the debug check, reload, the latency's details). */
  sheet(on = this.$("#sheet").hidden) {
    this.$("#sheet").hidden = !(on && !!this.docked);
    this.emit();
  }
  /** Kept for hosts that call it: the speed is the bar's status now. */
  speedChip(_on) {
  }
  /** A repaint reached the screen, `ms` after its keystroke: the status says how fast. */
  painted(ms) {
    this.speed = { ms, at: performance.now() };
    this.renderStatus();
    this.emit();
  }
  // ---- Dark PDF: the user's choice, Off by default (a static filter: it inverts figures too) ----
  darkPref() {
    try {
      const d = localStorage.getItem("phitex.darkpdf");
      return d === "on" || d === "theme" ? d : "off";
    } catch {
      return "off";
    }
  }
  dark = "off";
  darkQuery = matchMedia("(prefers-color-scheme: dark)");
  /** Dark pages off, on, or as the host's theme is; remembered on this machine. */
  darkPdf(d, save2 = true) {
    this.dark = d;
    if (save2)
      try {
        localStorage.setItem("phitex.darkpdf", d);
      } catch {
      }
    this.root.querySelectorAll("#darkpdf [data-dark]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.dark === d)));
    this.darkQuery.onchange = () => this.applyDark();
    this.applyDark();
  }
  /** Whether the host's theme is dark: Overleaf's (dock() says), VS Code's (its body's class), the system's. */
  hostDark() {
    if (this.kind === "overleaf") return !this.win.classList.contains("light");
    if (this.kind === "vscode") return !/\bvscode-light\b/.test(document.body.className);
    return this.darkQuery.matches;
  }
  applyDark() {
    this.win.classList.toggle("dark-pdf", this.dark === "on" || this.dark === "theme" && this.hostDark());
  }
  // ---- Feature tips, each once (own-bar hosts; Overleaf's toolbar has the extension's) ----
  tipAt = -1;
  tipList = [];
  /** The tips not seen yet (`all`: every one, What's new), from the first. */
  tips(all) {
    if (!this.ownBar || this.prefs.collapsed) return;
    let seen = [];
    try {
      seen = JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]");
    } catch {
    }
    const offered = TIPS.filter((t) => !(t.id === "compare" && !this.ev.onCompare));
    this.tipList = all ? offered : offered.filter((t) => !seen.includes(t.id));
    this.tip(0);
  }
  tip(i) {
    const el = this.$("#tip");
    this.root.querySelectorAll(".ringed").forEach((e) => e.classList.remove("ringed"));
    this.tipAt = i;
    const t = this.tipList[i];
    if (!t) return void (el.hidden = true);
    try {
      const seen = new Set(JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]"));
      seen.add(t.id);
      localStorage.setItem("phitex.tips.seen", JSON.stringify([...seen]));
    } catch {
    }
    let target = this.$(t.at);
    if (!target || !target.offsetParent) target = this.$("#b-more");
    target.classList.add("ringed");
    el.innerHTML = `<h4>${t.fresh ? '<span class="new">New</span>' : ""}${esc3(t.title)}</h4><p>${esc3(t.text)}</p><div class="acts"><span class="n">${i + 1} of ${this.tipList.length}</span><button type="button" data-t="skip">Skip</button><button type="button" class="primary" data-t="next">${i + 1 < this.tipList.length ? "Next" : "Got it"}</button></div>`;
    el.hidden = false;
    const bar2 = this.$(".bar").getBoundingClientRect(), r = target.getBoundingClientRect(), w = el.offsetWidth;
    const cx = r.left + r.width / 2 - bar2.left, left = Math.max(8, Math.min(bar2.width - w - 8, cx - w / 2));
    Object.assign(el.style, { left: `${left}px`, top: `${r.bottom - bar2.top + 9}px` });
    el.style.setProperty("--ax", `${cx - left}px`);
    el.onclick = (e) => {
      e.stopPropagation();
      const a = e.target.closest("[data-t]")?.dataset.t;
      if (a) this.tip(a === "next" ? i + 1 : -1);
    };
  }
  /**
  * The diff's bar over the page (null: none): against which version, the
  * changes and the one at, and its controls. `showing` "diff" or "current":
  * which of the two the page is (toggled with "d").
  */
  diffBar(d, on) {
    const el = this.$("#diffbar");
    if (!d) {
      el.className = "diffbar";
      el.innerHTML = "";
      return;
    }
    const esc5 = (t) => t.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
    const n = d.busy ? `<span class="n busy">${esc5(d.busy)}</span>` : `<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    el.innerHTML = `<span class="what" title="${esc5(d.title)}">Diff vs ${esc5(d.title)} <small>${esc5(d.when)}</small></span><button data-a="prev" title="Previous change (Shift+N)" ${d.busy || !d.changes ? "disabled" : ""}>\u2039</button>${n}<button data-a="next" title="Next change (N)" ${d.busy || !d.changes ? "disabled" : ""}>\u203A</button><span class="sep"></span><button data-a="toggle" class="${d.showing === "current" ? "cur" : ""}" title="Show the ${d.showing === "diff" ? "current version" : "diff"} (D)">${d.showing === "diff" ? "Current" : "Diff"}</button><button data-a="pdf" title="Download the diff PDF" ${d.busy ? "disabled" : ""}>\u2B07 PDF</button><button data-a="tex" title="Download diff.tex" ${d.busy ? "disabled" : ""}>\u2B07 .tex</button><button data-a="settings" title="Diff style: colors, markup" aria-label="Diff style">\u2699</button><button data-a="close" title="Stop comparing" aria-label="Stop comparing">\u2715</button>`;
    el.className = "diffbar on";
    el.onclick = (e) => {
      const a = e.target.closest("button[data-a]")?.dataset.a;
      if (a) on?.[a]?.();
    };
  }
  /**
   * The diff's style, a popover under the bar (null: shut): latexdiff's
   * markup and subtype, and the two colors, `change` called on each pick.
   */
  diffSettings(look2, change, defaults2) {
    const el = this.$("#diffset");
    if (!look2) {
      el.className = "diffset";
      return;
    }
    look2 = { ...look2, add_color: safeColor(look2.add_color, "#0000ff"), del_color: safeColor(look2.del_color, "#ff0000") };
    const MARKUP = [
      ["underline", "Underline"],
      ["ctraditional", "Color + font"],
      ["traditional", "Font"],
      ["cfont", "Color + size"],
      ["fontstrike", "Strike"],
      ["bold", "Bold"],
      ["changebar", "Change bars"],
      ["culinechbar", "Underline + bars"],
      ["invisible", "Hide deletions"]
    ];
    const SUB = [["safe", "Plain"], ["color", "Colored"], ["marker", "Margin marks"]];
    const PAIRS = [
      ["#0000ff", "#ff0000", "Blue / red"],
      ["#15803d", "#b91c1c", "Green / red"],
      ["#0072b2", "#d55e00", "Colorblind-safe"],
      ["#7e22ce", "#6b7280", "Purple / gray"]
    ];
    const seg = (key, opts) => `<div class="opts">${opts.map(([v, t]) => `<button data-k="${key}" data-v="${v}" aria-pressed="${look2[key] === v}">${t}</button>`).join("")}</div>`;
    const deco = (add) => {
      const c = add ? look2.add_color : look2.del_color;
      if (look2.markup === "invisible" && !add) return "display:none";
      const strike = !add && /^(underline|fontstrike|culinechbar)$/.test(look2.markup);
      const line = add && /^(underline|culinechbar)$/.test(look2.markup) ? `text-decoration: underline wavy ${c}; text-underline-offset: 3px;` : "";
      return `color:${c};${strike ? `text-decoration: line-through ${c};` : ""}${line}${look2.markup === "bold" && add ? "font-weight:700;" : ""}`;
    };
    el.innerHTML = `<h4>Style</h4>${seg("markup", MARKUP)}<h4>Colors</h4><div class="opts">${PAIRS.map(([a, d, t]) => `<button class="pair" data-pair="${a},${d}" aria-pressed="${look2.add_color === a && look2.del_color === d}"><i style="background:${a}"></i><i style="background:${d}"></i>${t}</button>`).join("")}</div><div class="row"><span>Added</span><input type="color" data-c="add_color" value="${look2.add_color}"><span>Deleted</span><input type="color" data-c="del_color" value="${look2.del_color}"></div><h4>Mode</h4>${seg("subtype", SUB)}<div class="sample">The results are <span style="${deco(false)}">good</span> <span style="${deco(true)}">excellent</span>.</div><div class="foot"><small>As latexdiff's --type, --subtype</small><button data-reset="1">Reset</button></div>`;
    el.className = "diffset on";
    const set = (l) => {
      const next = { ...look2, ...l };
      change?.(next);
      this.diffSettings(next, change, defaults2);
    };
    el.onclick = (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      if (b.dataset.reset && defaults2) return set(defaults2);
      if (b.dataset.pair) {
        const [a, d] = b.dataset.pair.split(",");
        return set({ add_color: a, del_color: d });
      }
      if (b.dataset.k) set({ [b.dataset.k]: b.dataset.v });
    };
    el.onchange = (e) => {
      const i = e.target;
      if (i.dataset.c) set({ [i.dataset.c]: i.value });
    };
  }
  /** Between the first paint and instant typing (since when); the status says so, or the corner where there is no bar. */
  warm = null;
  warmTimer;
  /** Startup: the first paint is in, instant typing still being prepared (the time going); then "ready", briefly. */
  warmup(w) {
    clearInterval(this.warmTimer);
    if (w.state === "ready") {
      this.warm = null;
      this.renderStatus();
      if (!this.ownBar) this.toast("warm", { title: "Instant typing ready", pct: null, ms: 2500 });
      return;
    }
    this.warm = { since: w.since ?? Date.now() };
    const tick = () => {
      this.renderStatus();
      if (this.ownBar || !this.warm) return;
      const s = Math.round((Date.now() - this.warm.since) / 1e3);
      this.toast("warm", { title: "Preparing instant typing\u2026", detail: `${s} s${w.slow ? " \xB7 edits are slower until it is ready" : ""}`, pct: null });
    };
    tick();
    this.warmTimer = setInterval(tick, 1e3);
  }
  /** The last build's numbers, as the session says them: a line, and the lines behind it. */
  lat = null;
  latency(summary, details) {
    this.lat = { summary, details: details ?? this.lat?.details };
    this.$("#details").textContent = details ?? summary;
    if (!this.$("#m-time").hidden) this.timing();
  }
  /** The timing popover: the last build's line, its parts as the session lists them, the last message. */
  timing() {
    const el = this.$("#m-time");
    const rows = (this.lat?.details ?? "").split("\n").filter(Boolean).map((l) => l.trim().split(/\s{2,}/)).map(([k, ...v]) => `<tr><td>${esc3(k)}</td><td>${esc3(v.join(" "))}</td></tr>`).join("");
    const ms = this.speed ? `${this.speed.ms < 10 ? this.speed.ms.toFixed(1) : Math.round(this.speed.ms)} ms` : "";
    el.innerHTML = `<h3>${ms ? `Keystroke to page: ${ms}` : "The last build"}</h3>` + (rows ? `<table>${rows}</table>` : "") + `<div class="foot"></div><div class="foot msg${this.msgErr ? " err" : ""}"></div>`;
    el.querySelector(".foot").textContent = this.lat?.summary ?? "No build yet.";
    el.querySelector(".msg").textContent = this.msgText;
  }
  /** The last message (connected, files read, a debug check's result), shown in the timing popover. */
  msgText = "";
  msgErr = false;
  msg(t, err = false) {
    this.msgText = t;
    this.msgErr = err;
    if (!this.$("#m-time").hidden) this.timing();
  }
  error(t) {
    this.msg(t, true);
    if (!this.last && !this.pkgBusy) {
      const card = this.card();
      card.querySelector("h3").textContent = "Couldn't open the project";
      card.querySelector(".note").textContent = this.words.openFailed;
      card.querySelector(".count").textContent = t;
      if (this.ev.onReport) {
        const b = document.createElement("button");
        b.className = "reportbtn";
        b.textContent = "Report a problem\u2026";
        b.onclick = () => void this.report();
        card.append(b);
      }
      return;
    }
    this.toast("msg", { title: "Something went wrong", detail: t, pct: null, icon: "error", err: true, ms: 8e3 });
    if (this.ev.onReport) {
      const el = this.$("#toast");
      el.style.cursor = "pointer";
      el.title = "Click for a debug report to send";
      el.onclick = () => (el.onclick = null, el.style.cursor = "", el.title = "", void this.report());
    }
  }
  check(r) {
    if (r.ok) this.msg(`\u2713 incremental = fresh build (${r.ms.toFixed(1)} ms)`);
    else this.error(`\u2717 mismatch vs fresh build: ${r.mismatch}`);
  }
  files(names, _skipped, took) {
    this.msg(`${names.length} file${names.length === 1 ? "" : "s"} \xB7 ${took} \xB7 all local`);
  }
  mains(names, main) {
    const s = this.$("#main");
    s.innerHTML = "";
    for (const n of names) s.append(new Option(n, n, n === main, n === main));
  }
  /** The last build's log (the host's), in a dialog over the pages. */
  async showLog() {
    const box = this.$("#logview"), pre = this.$("#logtext");
    pre.textContent = "Reading the log\u2026";
    box.hidden = false;
    const text = await this.ev.onLog?.().catch((e) => `Couldn't read the log: ${e instanceof Error ? e.message : String(e)}`);
    pre.textContent = text ?? "No log yet: the first build hasn't finished.";
  }
  redraw() {
    this.viewer?.redraw();
  }
  /** The page box, for the page in view. */
  nav() {
    const n = this.viewer.pages;
    const k = this.at;
    const pg = this.$("#pgin");
    if (this.root.activeElement !== pg) pg.value = n ? String(k + 1) : "\u2013";
    this.$("#pgn").textContent = String(n);
    queueMicrotask(() => this.emit());
  }
  /** The pages there are, by hash (the viewer draws the changed ones it shows). */
  layout(hashes) {
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes);
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
    if (hashes.length) this.sheetDown();
    this.nav();
  }
  /** The editor to a page's source (a double-click on it). */
  goto(file, from2, to, focus) {
    this.ev.onGotoRange?.(file, from2, to, focus);
  }
  /** The editor's selection on the pages ([] clears). */
  marks(marks) {
    this.viewer.marks(marks);
  }
  /** Collaborators' cursors on the pages ([] clears). */
  others(marks) {
    this.viewer.others(marks);
  }
  /** The debug report, shown whole before it is copied or emailed. */
  async report() {
    const text = await this.ev.onReport?.() ?? "";
    const box = this.$("#report");
    this.$("#reporttext").value = text;
    box.hidden = false;
    const copy = () => navigator.clipboard.writeText(text).catch(() => this.$("#reporttext").select());
    this.$("#reportcopy").onclick = () => void copy().then(() => this.$("#reportcopy").textContent = "Copied \u2713");
    const mail = this.$("#reportmail");
    mail.hidden = !REPORT_TO;
    mail.onclick = () => void copy().then(() => {
      const body = "Please paste the debug report here (it is on your clipboard), and say what you were doing:\n\n";
      window.open(`mailto:${REPORT_TO}?subject=${encodeURIComponent("PhiTeX Instant debug report")}&body=${encodeURIComponent(body)}`);
    });
    this.$("#reportclose").onclick = () => box.hidden = true;
  }
  /** Highlight `boxes` on page `k` (the source the editor is at). */
  mark(k, boxes2, scroll = true) {
    this.viewer.mark(k, boxes2, scroll);
  }
  page(img, k, n, hash) {
    if (!img) {
      if (!this.last && !this.anim) {
        const empty = this.$("#empty");
        empty.textContent = n ? "This page is empty." : "No page shipped yet.";
        empty.hidden = false;
      }
      if (this.lastStatus) this.stale({ ...this.lastStatus, pages: n });
      return;
    }
    this.last = img;
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.$("#stage").classList.toggle("fit", (this.zoomNow ?? this.prefs.zoom) === "fit");
    if (this.viewer.pages <= k) this.viewer.layout(Array.from({ length: Math.max(n, k + 1) }, (_, i) => i === k && hash || `?${i}`));
    this.viewer.set(k, img, hash ?? null);
    this.sheetDown();
    this.renderStatus();
    this.nav();
  }
};
function foldedDetail(text, label) {
  const d = document.createElement("details");
  d.className = "folded";
  const sum = document.createElement("summary");
  sum.textContent = label;
  const pre = document.createElement("pre");
  pre.className = "detail";
  pre.textContent = text;
  d.append(sum, pre);
  return d;
}
function known(source2) {
  return !!source2 && source2 !== "none";
}

// viewer/src/edits.ts
function utf8Len(s, from2 = 0, to = s.length) {
  let n = 0;
  for (let i = from2; i < to; i++) {
    const c = s.charCodeAt(i);
    if (c < 128) n += 1;
    else if (c < 2048) n += 2;
    else if (c >= 55296 && c <= 56319 && i + 1 < to) {
      const d = s.charCodeAt(i + 1);
      if (d >= 56320 && d <= 57343) {
        n += 4;
        i++;
      } else n += 3;
    } else n += 3;
  }
  return n;
}
function byteOffset(s, pos) {
  if (pos > 0 && pos < s.length) {
    const c = s.charCodeAt(pos);
    const p = s.charCodeAt(pos - 1);
    if (c >= 56320 && c <= 57343 && p >= 55296 && p <= 56319) pos--;
  }
  return utf8Len(s, 0, pos);
}
function apply(s, e) {
  return s.slice(0, e.from) + e.text + s.slice(e.to);
}
function merge(before, a, b) {
  const aEnd = a.from + a.text.length;
  if (b.to < a.from || b.from > aEnd) return null;
  const from2 = Math.min(a.from, b.from);
  const endAfterA = Math.max(aEnd, b.to);
  const to = endAfterA <= aEnd ? a.to : endAfterA - (a.text.length - (a.to - a.from));
  const afterA = apply(before.slice(from2, to), { from: a.from - from2, to: a.to - from2, text: a.text });
  const text = apply(afterA, { from: b.from - from2, to: b.to - from2, text: b.text });
  return { from: from2, to, text };
}
var Batch = class {
  edits = [];
  /** The text before the first queued edit. */
  base;
  /** The text after all of them. */
  text;
  constructor(text) {
    this.base = text;
    this.text = text;
  }
  get size() {
    return this.edits.length;
  }
  push(e) {
    const last = this.edits.at(-1);
    if (last) {
      const before = this.befores.at(-1);
      const m = merge(before, last, e);
      if (m) {
        this.edits[this.edits.length - 1] = m;
        this.text = apply(this.text, e);
        return;
      }
    }
    this.befores.push(this.text);
    this.edits.push(e);
    this.text = apply(this.text, e);
  }
  befores = [];
  /** The queued edits in bytes, in order, each against the text before it. */
  take() {
    const out = [];
    let s = this.base;
    for (const e of this.edits) {
      const start = byteOffset(s, e.from);
      out.push({ start, end: start + utf8Len(s, e.from, e.to), text: e.text });
      s = apply(s, e);
    }
    this.edits = [];
    this.befores = [];
    this.base = this.text;
    return out;
  }
};
function charOffset(s, byte) {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    if (n >= byte) return i;
    const c = s.charCodeAt(i);
    if (c < 128) n += 1;
    else if (c < 2048) n += 2;
    else if (c >= 55296 && c <= 56319 && i + 1 < s.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return s.length;
}

// viewer/src/sync.ts
function glyphs(json, path) {
  if (!json?.n || !json.file || !json.start || !json.end || !json.synth || !json.x || !json.y) return [];
  const files = json.files.map(path);
  const words2 = (b64) => {
    const b = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    return new DataView(b.buffer);
  };
  const [file, start, end, synth, x, y] = [json.file, json.start, json.end, json.synth, json.x, json.y].map(words2);
  const out = [];
  for (let i = 0; i < json.n; i++) {
    const f = file.getUint32(4 * i, true);
    const s = 4 * (i >> 5) < synth.byteLength ? synth.getUint32(4 * (i >> 5), true) >>> (i & 31) & 1 : 0;
    out.push({ x: x.getFloat32(4 * i, true), y: y.getFloat32(4 * i, true), file: f === 4294967295 ? null : files[f] ?? null, start: start.getUint32(4 * i, true), end: end.getUint32(4 * i, true), synth: s === 1 });
  }
  return out;
}
function nearest(gs, x, y) {
  let best = null, d = Infinity;
  for (const g of gs) {
    if (!g.file) continue;
    const dy = g.y - 3 - y, dx = g.x + 2.5 - x;
    const e = dx * dx + 9 * dy * dy + (g.synth ? 400 : 0);
    if (e < d) d = e, best = g;
  }
  return best;
}
function from(gs, file, lo, hi) {
  return gs.filter((g) => g.file === file && !g.synth && g.start < hi && g.end > lo);
}
function boxes(gs, all) {
  const next = /* @__PURE__ */ new Map();
  for (let i = 0; i < all.length; i++) {
    const n = all[i + 1];
    next.set(all[i], n && Math.abs(n.y - all[i].y) < 0.5 && n.x > all[i].x && n.x - all[i].x < 15 ? n.x - all[i].x : 5);
  }
  const lines = /* @__PURE__ */ new Map();
  for (const g of gs) {
    const k = Math.round(g.y * 2) / 2;
    const r = lines.get(k), e = g.x + (next.get(g) ?? 5);
    lines.set(k, r ? [Math.min(r[0], g.x), Math.max(r[1], e)] : [g.x, e]);
  }
  return [...lines].map(([y, [a, b]]) => [a - 1, y - 8, b - a + 2, 11]);
}
function lineAt(hits, at) {
  let best = null;
  for (const g of hits) if (!best || Math.abs(g.start - at) < Math.abs(best.start - at)) best = g;
  return best ? hits.filter((g) => Math.abs(g.y - best.y) < 0.5) : [];
}
function wordNear(text, at, lo, hi) {
  const here = wordBytes(text, at);
  if (here) return here;
  const w = (c) => c >= 128 || c >= 48 && c <= 57 || c >= 65 && c <= 90 || c >= 97 && c <= 122 || c === 39 || c === 45;
  for (let i = Math.min(at, text.length) - 1; i >= lo; i--) if (w(text[i])) return wordBytes(text, i);
  for (let i = at; i < Math.min(hi, text.length); i++) if (w(text[i])) return wordBytes(text, i);
  return null;
}
function glyphNear(hits, at) {
  let before = null, after = null;
  for (const g of hits) {
    if (g.start <= at) {
      if (!before || g.start > before.start) before = g;
    } else if (!after || g.start < after.start) after = g;
  }
  return before ?? after;
}
function wordBytes(text, at) {
  const w = (c) => c >= 128 || c >= 48 && c <= 57 || c >= 65 && c <= 90 || c >= 97 && c <= 122 || c === 39 || c === 45;
  let lo = at, hi = at;
  while (lo > 0 && w(text[lo - 1])) lo--;
  while (hi < text.length && w(text[hi])) hi++;
  return lo < hi ? [lo, hi] : null;
}

// viewer/src/diagnostics.ts
function lineAt2(text, i) {
  let n = 1;
  for (let k = 0; k < i; k++) if (text.charCodeAt(k) === 10) n++;
  return n;
}
function scan(text) {
  const open = [];
  const stray = [];
  const dollars = [];
  let paraDollars = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (c === "%") {
      while (i < text.length && text[i] !== "\n") i++;
      continue;
    }
    if (c === "{") open.push(i);
    else if (c === "}") {
      if (open.length) open.pop();
      else stray.push(i);
    } else if (c === "$") {
      if (text[i + 1] === "$") i++;
      paraDollars.push(i);
    } else if (c === "\n" && /^[ \t]*\n/.test(text.slice(i + 1, i + 40))) {
      if (paraDollars.length % 2) dollars.push(paraDollars.at(-1));
      paraDollars = [];
    }
  }
  if (paraDollars.length % 2) dollars.push(paraDollars.at(-1));
  return { openBraces: open, strayBraces: stray, openDollars: dollars };
}
var LATEX = /\\(documentclass|usepackage|begin\{[A-Za-z*]+\}|end\{[A-Za-z*]+\}|section|subsection|maketitle|cite|ref|label|emph|textbf|textit)\b/g;
function diagnose(files, main, build) {
  const out = [];
  for (const [file, text] of Object.entries(files)) {
    if (!/\.(tex|sty|def)$/i.test(file)) continue;
    const s = scan(text);
    if (s.openBraces.length) {
      const at = s.openBraces[0];
      out.push({
        severity: "warning",
        code: "unclosed-brace",
        message: `{ never closed${s.openBraces.length > 1 ? ` (${s.openBraces.length} open)` : ""}: the group runs to the end of the file`,
        file,
        line: lineAt2(text, at)
      });
    }
    for (const at of s.strayBraces.slice(0, 3))
      out.push({ severity: "warning", code: "stray-brace", message: "} with no { to close", file, line: lineAt2(text, at) });
    for (const at of s.openDollars.slice(0, 3))
      out.push({ severity: "warning", code: "unclosed-math", message: "$ never closed in this paragraph", file, line: lineAt2(text, at) });
    for (const m of text.matchAll(/(^|[^\\])\\[gex]?def\s*\\([A-Za-z@]+)([^{\n%]*)$/gm))
      out.push({ severity: "warning", code: "def-no-body", message: `\\${m[2]} is defined with no { body } on its line`, file, line: lineAt2(text, m.index + m[1].length) });
    const ifs = [...text.matchAll(/\\if[a-z]*\b/g)];
    const fis = (text.match(/\\fi\b/g) ?? []).length;
    if (ifs.length > fis && !/\\[gex]?def/.test(text))
      out.push({ severity: "info", code: "unclosed-if", message: `${ifs.length - fis} \\if\u2026 without \\fi`, file, line: lineAt2(text, ifs.at(-1).index) });
    for (const m of text.matchAll(/\\input\s*\{?([^\s}\\%]+)/g)) {
      const n = m[1];
      if (!(n in files) && !(`${n}.tex` in files) && !/\.(aux|bbl|ind|toc)$/.test(n) && !n.startsWith("\\"))
        out.push({ severity: "warning", code: "missing-file", message: `\\input ${n}: no such file in the project`, file, line: lineAt2(text, m.index) });
    }
    const latex = build?.latex ? [] : [...new Set(text.match(LATEX) ?? [])];
    if (latex.length) {
      const first = text.search(LATEX);
      out.push({
        severity: "error",
        code: "latex",
        message: `LaTeX is not supported yet (dropped): ${latex.slice(0, 5).join(" ")}${latex.length > 5 ? " \u2026" : ""}`,
        file,
        line: lineAt2(text, first)
      });
    }
    let nonAscii = 0;
    let firstNon = -1;
    for (let i = 0; i < text.length; i++)
      if (text.charCodeAt(i) > 127) {
        nonAscii++;
        if (firstNon < 0) firstNon = i;
      }
    if (nonAscii && !build?.latex)
      out.push({ severity: "info", code: "non-ascii", message: `${nonAscii} non-ASCII characters: PhiTeX's fonts may not typeset them`, file, line: lineAt2(text, firstNon) });
  }
  if (main && files[main] !== void 0 && !/\\(bye|end|enddocument)\b/.test(files[main]))
    out.push({ severity: "info", code: "no-end", message: "no \\bye: the last page ships at the end of the file anyway", file: main });
  for (const f of build?.packageErrors ?? [])
    out.push({ severity: "error", code: "package-download", message: `couldn't download ${f.name}: ${f.error}` });
  if (build?.texError) {
    const e = build.texError;
    const stuck = e.missing && build.unavailable?.includes(e.missing);
    const failed = e.missing && build.packageErrors?.some((f) => f.name === e.missing);
    out.push({
      severity: "error",
      code: e.missing ? "tex-missing-file" : "tex-error",
      message: e.missing ? `${e.missing} not found${stuck ? ` in the project or ${build.packageSource ?? "the package source"}` : failed ? " (its download failed, above)" : " (still downloading?)"}: the build stopped here` : e.message,
      file: e.file ?? void 0,
      line: e.line ?? void 0,
      detail: e.context
    });
  }
  if (build) {
    if (build.pending)
      out.push({ severity: "warning", code: "pending", message: `${build.pending} part${build.pending === 1 ? "" : "s"} not read: out of fuel (a loop?) or unsupported` });
    if (build.undefinedNames?.length) out.push({ severity: "warning", code: "undefined", message: `undefined: ${build.undefinedNames.join(" ")}` });
    const optional = [...new Set(build.unavailable?.filter((n) => n !== build.texError?.missing).map((n) => n.replace(/(\.[a-z]+)\.tex$/, "$1")))];
    if (optional.length) {
      const names = optional.slice(0, 6).join(" ") + (optional.length > 6 ? " \u2026" : "");
      out.push({
        severity: "info",
        code: "package-unavailable",
        message: build.packageSource ? `looked for and not found in ${build.packageSource} (usually optional): ${names}` : `packages are not downloaded yet: ${names}`
      });
    }
    if (build.pages === 0) out.push({ severity: "warning", code: "no-pages", message: "no page shipped yet" });
  }
  const rank = { error: 0, warning: 1, info: 2 };
  return out.sort((a, b) => rank[a.severity] - rank[b.severity]);
}

// viewer/src/packages.ts
var DELIVERED = "\0delivered";
var noPackages = { label: "none", resolve: async () => null };
function isPackageFile(name) {
  return /\.(sty|cls|clo|def|cfg|fd|ldf|ltx|tex|dfu|sto|lbx|bbx|cbx|dbx|mld|tikz|pgf|code\.tex|lua|ini|cmap|fontspec|tfm|vf|pfb|enc|map)$/i.test(name) && !name.includes("/");
}
function referenced(text) {
  const out = /* @__PURE__ */ new Set();
  const body = text.replace(/(^|[^\\])%.*$/gm, "$1");
  for (const m of body.matchAll(/\\(documentclass|LoadClass(?:WithOptions)?|usepackage|RequirePackage(?:WithOptions)?)\s*(?:\[[^\]]*\]\s*)?\{([^}]*)\}/g)) {
    const ext = /class/i.test(m[1]) ? ".cls" : ".sty";
    for (const n of m[2].split(",")) {
      const name = n.trim();
      if (/^[A-Za-z0-9@_.-]+$/.test(name)) out.add(name + ext);
    }
  }
  if (/\\DeclareFontShape/.test(body)) {
    for (const m of body.matchAll(/<[^>]*>\s*(?:\\[A-Za-z@]+\s*|s\*\s*(?:\[[^\]]*\]\s*)?)*([A-Za-z][A-Za-z0-9-]*)(?![A-Za-z0-9-]*\/)/g))
      if (!/^(s?sub|gen|genb|sgen|fixed|sfixed|error|serror|warning)$/.test(m[1])) out.add(m[1] + ".tfm");
  }
  for (const m of body.matchAll(/\\(?:input|InputIfFileExists|@input)\s*\{?([A-Za-z0-9@_-]+\.(?:sty|cls|clo|def|cfg|fd|ldf|tex|dfu))\}?/g)) out.add(m[1]);
  return [...out];
}

// viewer/src/session.ts
var preambleOf = (t) => {
  const end = t?.indexOf("\\begin{document}") ?? -1;
  return end < 0 ? t ?? "" : t.slice(0, end);
};
var usesMinted = (t) => /\\(usepackage|RequirePackage)\s*(\[[^\]]*\])?\s*\{[^}]*\bminted\b/.test(t);
var WATCH2 = /* @__PURE__ */ new Set(["page", "progress", "diagnostics", "settled", "superseded", "sync"]);
function place2(t, pos) {
  const at = byteOffset(t, pos);
  const lo = t.lastIndexOf("\n", pos - 1) + 1, hiC = t.indexOf("\n", pos);
  const line = [byteOffset(t, lo), byteOffset(t, hiC < 0 ? t.length : hiC)];
  return { at, word: wordNear(new TextEncoder().encode(t), at, line[0], line[1]), line };
}
function traceReq(r) {
  const o = {};
  for (const [k, v] of Object.entries(r)) {
    if (k === "op" || k === "id") continue;
    o[k] = typeof v === "string" && v.length > 40 ? `${v.length} chars` : v && typeof v === "object" ? `${Object.keys(v).length} keys` : v;
  }
  return o;
}
function traceRes(r) {
  const j = r.json ?? {};
  return {
    ok: r.ok,
    ...r.error ? { error: r.error } : {},
    ...typeof j.how === "string" ? { how: j.how.split("\n")[0].slice(0, 140) } : {},
    ...j.pages !== void 0 ? { pages: Array.isArray(j.pages) ? j.pages.length : j.pages } : {},
    ...j.history !== void 0 ? { history: j.history } : {},
    ...Array.isArray(j.missing) && j.missing.length ? { missing: j.missing.length } : {},
    ...j.error ? { texError: typeof j.error === "string" ? j.error : j.error.message } : {},
    ...j.build_ms !== void 0 ? { build_ms: Math.round(j.build_ms) } : {},
    ...r.pdf ? { pdf: r.pdf.length } : {},
    // (a page from the draw worker: its draw time, and whether it was drawn ahead)
    ...j.draw_ms !== void 0 ? { drawer: true, draw_ms: Math.round(j.draw_ms), kept: j.kept } : {}
  };
}
var defaults = { fuel: 1e5, dpi: 96, checkEveryMs: 5e3 };
var image = (r) => r.canvas && r.size ? { canvas: r.canvas, w: r.size[0], h: r.size[1], links: r.links } : null;
function guessMain(files) {
  const names = Object.keys(files).filter((n) => /\.tex$/i.test(n));
  const score = (n) => {
    const t = files[n];
    let s = 0;
    if (/(^|\n)[^%\n]*\\(bye|end\{document\})/.test(t)) s += 4;
    if (/(^|\n)\s*\\documentclass/.test(t)) s += 2;
    if (/^main\.tex$/i.test(n)) s += 1;
    if (!n.includes("/")) s += 1;
    return s;
  };
  return names.sort((a, b) => score(b) - score(a))[0] ?? null;
}
function diff(old, text) {
  if (old === text) return null;
  let a = 0;
  while (a < old.length && a < text.length && old[a] === text[a]) a++;
  let z = 0;
  while (z < old.length - a && z < text.length - a && old[old.length - 1 - z] === text[text.length - 1 - z]) z++;
  if (a > 0 && /[\udc00-\udfff]/.test(old[a] ?? "")) a--;
  if (z > 0 && /[\udc00-\udfff]/.test(old[old.length - z] ?? "")) z--;
  return { from: a, to: old.length - z, text: text.slice(a, text.length - z) };
}
var PreviewSession = class {
  o;
  /** Each file's text as sent to the core (queued edits: in `batches`). */
  files = {};
  batches = /* @__PURE__ */ new Map();
  main = null;
  open = null;
  page = 0;
  pages = 0;
  opened = false;
  debug = false;
  /** When each file's queued edits began (the oldest keystroke not sent). */
  since = /* @__PURE__ */ new Map();
  scheduled = false;
  /** Sends are chained: the core gets the edits in the order they were made. */
  chain = Promise.resolve();
  host;
  core;
  sink;
  /**
   * What happened, in order (dev: scripts/mock-run.mjs reads it): each core
   * request and its reply, package fetches, layouts. A ring of the last 3000,
   * times in ms since the session began; never document text, only sizes.
   */
  trace = [];
  /** The core fetches what it lacks from Shelf itself (its open reply says so). */
  coreFetches = false;
  /** Shelf packs the core fetched in the build under way (shown as loading until it ends). */
  inBuild = [];
  tellPackages() {
    this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...this.pkg.done ?? []], failed: [...this.pkg.failed ?? []] });
  }
  /**
   * The core fetches a pack mid-build (the job goes on with it), as its
   * progress says (`phase: "fetching"`): the view says which. (A pack it
   * could not get is the build's trouble, said by the host, not here.)
   */
  onFetching(e) {
    this.tr("package: core fetches", e);
    this.pkg.source = this.source.label;
    const prev = this.inBuild.at(-1);
    if (prev) this.pkg.loading = this.pkg.loading.filter((n) => n !== prev);
    if (prev) this.pkg.done = [...this.pkg.done ?? [], prev];
    this.inBuild.push(e.pack);
    this.pkg.loading = [...this.pkg.loading, e.pack];
    this.tellPackages();
  }
  /** Core requests sent and not answered yet; of them, ones that may build. */
  inflight = 0;
  building = 0;
  t0 = 0;
  tr(k, d) {
    this.t0 ||= this.now();
    this.trace.push({ t: Math.round(this.now() - this.t0), k, d });
    if (this.trace.length > 3e3) this.trace.shift();
  }
  constructor(host2, core, sink, opts = {}) {
    this.host = host2;
    core.onEvent?.((e) => {
      if (e.event === "follow") return this.followBytes(e.file, e.at, e.word, e.line);
      if (e.event === "select") return void this.selectBytes(e.file, e.from, e.to);
      if (this.o.remote && "event" in e && WATCH2.has(e.event)) return this.onWatch(e);
      if (e.event === "building" || e.event === "preparing") return this.sink.preparing?.(e.on);
      if (e.event === "switched") {
        this.tr("core: the SSA worker took over", {});
        this.glyphCache.clear();
        if (this.opened) this.chain = this.chain.then(() => this.layout()).then(() => this.statusSoon());
        return;
      }
      if (e.event === "settled") {
        if (this.opened && !this.busy) this.chain = this.chain.then(() => this.layout()).then(() => this.statusSoon());
        return;
      }
      if (e.event === "page" || e.event === "progress" || e.event === "done") this.onStream(e);
    });
    this.core = {
      request: async (r) => {
        const t0 = this.now();
        this.tr(`\u2192 ${r.op}`, traceReq(r));
        this.inflight++;
        const builds = r.op === "edit" || r.op === "status" || r.op === "open";
        if (builds && this.building++ === 0) this.sink.busy?.(true);
        if (r.op === "open" || r.op === "set_file") this.glyphCache.clear();
        if (r.op === "edit") this.mapGlyphs(r.file, r.start, r.end, new TextEncoder().encode(r.text).length);
        const res = await core.request(r).finally(() => {
          this.inflight--;
          if (builds && --this.building === 0) this.sink.busy?.(false);
          if (builds && this.inBuild.length) {
            this.pkg.done = [...this.pkg.done ?? [], ...this.inBuild.filter((n) => this.pkg.loading.includes(n))];
            this.pkg.loading = this.pkg.loading.filter((n) => !this.inBuild.includes(n));
            this.inBuild = [];
            this.tellPackages();
          }
        });
        this.tr(`\u2190 ${r.op}`, { ms: Math.round(this.now() - t0), ...traceRes(res) });
        if (res.error) this.errorsSeen = [...this.errorsSeen.slice(-19), `${r.op}: ${res.error}`];
        const lost = res.json?.error === "no such handle" || /^(core trapped|PDF driver failed)/.test(res.error ?? "");
        if (res.ok && (r.op === "edit" || r.op === "status")) this.traps = 0;
        if (lost && this.opened && r.op !== "open") {
          this.opened = false;
          this.traps++;
          const why = res.error ?? "the core restarted";
          if (this.traps <= 1) {
            this.sink.error(`${why}; reopening`);
            this.sink.connection?.(false, `${why}; reopening`);
            this.chain = this.chain.then(() => this.reopen());
          } else {
            this.sink.error(`${why}, again: the preview stops here (reload the page to retry)`);
            this.sink.connection?.(false, `${why}, again: reload the page to retry`);
          }
        }
        return res;
      },
      onLost: core.onLost?.bind(core)
    };
    this.sink = sink;
    this.o = { ...defaults, ...opts };
    host2.onOpen((f0, t) => {
      const f = this.known(f0);
      this.open = f;
      this.resending.delete(f);
      this.sync(f, t);
      this.status();
    });
    host2.onChanges((f0, es, len) => {
      const f = this.known(f0);
      this.queue(f, es);
      if (len !== void 0) this.agree(f, len, es);
    });
    core.onLost?.(() => {
      this.opened = false;
      this.sink.error("core lost; reopening");
      this.sink.connection?.(false, "the engine stopped; reopening");
      this.reopen();
    });
  }
  /** Files whose whole text was asked for again (host.resend), until it comes (onOpen). */
  resending = /* @__PURE__ */ new Set();
  /**
   * The session's copy of `file` (its queued edits in) against the
   * editor's length after `edits`: on a difference the copy has drifted
   * (edits lost or applied twice, as an Overleaf history restore's were),
   * so it is said and the whole text asked for again, sync() diffing it in.
   * Once until it comes.
   */
  agree(file, len, edits) {
    const have = this.text(file)?.length ?? 0;
    if (have === len || this.resending.has(file)) return;
    this.tr("editor and session disagree", { file, editor: len, session: have, edits: edits.slice(-5) });
    if (!this.host.resend) return;
    this.resending.add(file);
    this.host.resend(file);
  }
  /**
   * The project's file the editor's name is: itself, else the one file
   * whose path ends with it (a name read from Overleaf's page without its
   * folders: edits to a new, empty file the build never reads, the page
   * never changing while the chip said they were built).
   */
  known(f) {
    if (f in this.files) return f;
    const like = Object.keys(this.files).filter((p) => p.endsWith("/" + f));
    if (like.length === 1) {
      this.tr("file: named by its path", { from: f, to: like[0] });
      return like[0];
    }
    return f;
  }
  /** The pages' request: -1, the PDF (the host draws its pages with MuPDF). */
  dpi() {
    return -1;
  }
  now() {
    return (this.o.now ?? (() => performance.now()))();
  }
  async start() {
    if (this.o.remote) {
      this.opened = true;
      return this.watchJoined();
    }
    const files = await this.host.loadProject();
    for (const [f, t] of Object.entries(files)) if (!(f in this.files)) this.files[f] = t;
    this.main = this.host.mainFile?.(this.files) ?? guessMain(this.files);
    this.sink.mains?.(Object.keys(this.files).filter((f) => f.endsWith(".tex")), this.main);
    await this.reopen();
    this.host.ready?.();
    if (this.o.checkEveryMs > 0) this.checkTimer = setInterval(() => this.debug && this.check(), this.o.checkEveryMs);
  }
  checkTimer;
  /** The session ends (its view closed): its timers stop and it sends nothing more; the core's side is its transport's to close. */
  stop() {
    clearInterval(this.checkTimer);
    if (this.statusTimer) clearTimeout(this.statusTimer);
    this.opened = false;
    this.paused = true;
  }
  /** Files changed outside the editor (e.g. a project reloaded): each diffed in. */
  refresh(files) {
    for (const [f, t] of Object.entries(files)) if (f !== this.open) this.sync(f, t);
  }
  setMain(m) {
    this.main = m;
    return this.reopen();
  }
  /**
   * Clean recompile: the project's files as Overleaf has them now, packages
   * that failed or were not found asked for again, and a fresh core session
   * (discovery, first paint, then the incremental program), nothing kept from
   * the last job but the packages already fetched.
   */
  async clean(files) {
    for (const [f, t] of Object.entries(files)) if (f !== this.open) this.files[f] = t;
    for (const n of [...this.pkg.unavailable, ...(this.pkg.failed ?? []).map((f) => f.name)]) this.asked.delete(n);
    this.pkg.unavailable = [];
    this.pkg.failed = [];
    this.texError = void 0;
    this.hashes = [];
    return this.reopen();
  }
  /** Whether the open's sources used minted (the worker then has its runner). */
  minted = false;
  /** A fresh core session from the texts with every queued edit in. */
  async reopen() {
    if (!this.main) return this.sink.error("no main .tex file found");
    for (const [f, b] of this.batches) this.files[f] = b.text;
    this.batches.clear();
    const t = this.now();
    const want = this.o.engine?.(this.files[this.main]) ?? "pdflatex";
    const shims = !ENGINES[want].ready && want === "xelatex" ? await this.o.shims?.(this.files[this.main]) : null;
    const engine = shims ? "pdflatex" : want;
    this.engine = engine;
    this.wanted = want;
    this.preamble = preambleOf(this.files[this.main]);
    this.sink.engine?.({ engine: want, ready: ENGINES[engine].ready, approx: !!shims });
    if (!ENGINES[engine].ready) {
      this.opened = false;
      this.tr("engine", { engine, ready: false });
      return;
    }
    await this.prefetch(["pdftex.map", ...Object.values(this.files).flatMap(referenced)]);
    this.minted = Object.values(this.files).some(usesMinted);
    const r = await this.core.request({ op: "open", main: this.main, files: { ...this.pkgFiles, ...shims, ...this.files }, fuel: this.o.fuel, engine, workers: this.o.workers });
    if (!r.ok) return this.sink.error((typeof r.json?.error === "string" ? r.json.error : void 0) ?? r.error ?? "open failed");
    this.sink.connection?.(true);
    if (r.json.building) {
      this.tr("open: streaming", {});
      this.opened = true;
      this.sink.busy?.(true);
      this.sink.streaming?.({ pages: 0, phase: r.json.phase ?? "typesetting" });
      const d = await new Promise((res) => this.streamEnd = res);
      this.sink.busy?.(false);
      this.sink.streaming?.(null);
      if (!d.ok || !d.json) return this.sink.error(d.error ?? "the build stopped");
      r.json = d.json;
    }
    this.opened = true;
    const latex = r.json.engine === "phitex";
    if (latex !== this.latex) this.lastWarned = 0;
    this.latex = latex;
    this.coreFetches = !!r.json.fetches;
    this.warming = typeof r.json.how === "string" && r.json.how.startsWith("plain") ? this.now() : 0;
    this.warmSlow = false;
    if (this.warming) this.sink.warmup?.({ state: "preparing", since: Date.now() });
    this.setPages(r.json.pages);
    this.noteError(r.json);
    this.status(r.json.pending, r.json.undefined_names);
    this.fetchPackages(r.json.missing);
    this.sink.latency(`opened in ${r.json.build_ms.toFixed(1)} ms (round trip ${(this.now() - t).toFixed(1)} ms)`);
    await (this.sink.layout ? this.layout(Array.isArray(r.json.hashes) ? r.json.hashes : void 0) : this.showPage());
    this.pkg.building = false;
    this.tellPackages();
    this.statusSoon();
  }
  /** A streamed open's end, awaited by `open`. */
  streamEnd;
  /** The streamed open's first page was traced. */
  streamFirst = false;
  onStream(e) {
    if (e.event === "done") {
      this.tr("open: streamed", { ok: e.ok });
      this.streamFirst = false;
      this.streamEnd?.(e);
      this.streamEnd = void 0;
      return;
    }
    if (!this.streamEnd) return;
    if (e.event === "progress") return this.sink.streaming?.({ pages: e.pages, phase: e.phase });
    if (!this.streamFirst) {
      this.streamFirst = true;
      this.tr("open: first page", { pages: e.pages });
    }
    this.setPages(e.pages);
    this.sink.streaming?.({ pages: e.pages, phase: "typesetting" });
    if (this.sink.layout) this.chain = this.chain.then(() => this.layout(e.hashes));
  }
  /** When the plain first paint came, while the SSA program is still to come (0: not warming). */
  warming = 0;
  warmSlow = false;
  /** A status said how the last build was: past a plain one, the SSA program is there. */
  warmCheck(how) {
    if (!this.warming || typeof how !== "string" || how.startsWith("plain")) return;
    this.tr("warmup: ready", { ms: Math.round(this.now() - this.warming) });
    this.warming = 0;
    this.sink.warmup?.({ state: "ready" });
  }
  lastWarned = 0;
  /** When the core trapped, in the last minute. */
  /** Traps since the last build that came back whole. */
  traps = 0;
  /** The engine the project runs with (its packages are resolved for it: which tree's file). */
  engine = "pdflatex";
  /** The engine the main file asked for at the last open (before the shims' choice), and its preamble then. */
  wanted;
  preamble;
  /** The core runs LaTeX (the open reply's `engine`; until it says, as phitex's, the default build). */
  latex = true;
  diags = [];
  /** The last build's first TeX error (the core's status), if it had one. */
  texError;
  noteError(j) {
    if (j.history === void 0) return;
    this.texError = j.error && typeof j.error === "object" ? j.error : void 0;
  }
  undefinedNames = [];
  /** The last status scan's pending count (edits keep it until the next scan). */
  pending;
  status(pending, undefinedNames) {
    if (this.o.remote) return this.sink.status({ pages: this.pages, main: this.main, file: null, diagnostics: this.watchDiags });
    this.pending = pending;
    if (undefinedNames) this.undefinedNames = undefinedNames;
    if (this.now() - this.lastWarned > 250 || undefinedNames) {
      const texts = {};
      for (const f of Object.keys(this.files)) texts[f] = this.text(f);
      this.diags = diagnose(texts, this.main, {
        pages: this.pages,
        pending,
        undefinedNames: this.undefinedNames,
        unavailable: this.pkg.unavailable,
        packageSource: this.source === noPackages ? void 0 : this.source.label,
        latex: this.latex,
        texError: this.texError,
        packageErrors: this.pkg.failed
      });
      this.lastWarned = this.now();
    }
    this.sink.status({ pages: this.pages, pending, main: this.main, file: this.open, diagnostics: this.diags });
  }
  /** Packages the core was given (kept apart from the project's files: never diagnosed, never a main). */
  pkgFiles = {};
  /** Every package name asked for once: not asked again. */
  asked = /* @__PURE__ */ new Set();
  pkg = { loading: [], unavailable: [], source: "", done: [], building: false };
  get source() {
    return this.o.packages ?? noPackages;
  }
  /**
   * The core read `missing` and found nothing: each package among them
   * fetched (once), then set as a file (a rebuild each: the core never
   * looks for a missing file again). What they read in turn shows up in
   * the next status, and is fetched then.
   */
  fetchPackages(missing, guessed = []) {
    if (this.coreFetches) {
      const none = (missing ?? []).filter((n) => isPackageFile(n) && !this.pkg.unavailable.includes(n));
      if (none.length) this.pkg.unavailable.push(...none), this.tellPackages();
      return;
    }
    const wanted = new Set(missing ?? []);
    missing = [...wanted, ...guessed];
    const want = [...new Set(missing ?? [])].filter((n) => isPackageFile(n) && !(n in this.files) && !this.asked.has(n));
    if (!want.length) return;
    for (const n of want) this.asked.add(n);
    const src = this.source;
    this.pkg.source = src.label;
    const tell = () => this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...this.pkg.done ?? []], failed: [...this.pkg.failed ?? []] });
    if (src === noPackages) {
      this.pkg.unavailable.push(...want);
      tell();
      return this.status(this.pending, this.undefinedNames);
    }
    this.pkg.loading.push(...want);
    tell();
    const got = [];
    this.tr("packages: want", want);
    const fetchOne = async (n) => {
      const t = await src.resolve(n, this.engine).catch((e) => this.failed(n, e));
      this.tr(t === null ? "package: none" : "package: got", n);
      got.push([n, t]);
      this.pkg.loading = this.pkg.loading.filter((m) => m !== n);
      if (t !== null) {
        this.pkg.done.push(n);
        this.pkg.failed = this.pkg.failed?.filter((f) => f.name !== n);
      }
      const more = t === null || t === DELIVERED ? [] : referenced(t).filter((m) => isPackageFile(m) && !(m in this.files) && !this.asked.has(m));
      for (const m of more) this.asked.add(m);
      this.pkg.loading.push(...more);
      tell();
      await Promise.all(more.map(fetchOne));
    };
    void Promise.all(want.map(fetchOne)).then(() => {
      for (const [n, t] of got) {
        if (t === null) {
          if (wanted.has(n) && !this.pkg.failed?.some((f) => f.name === n)) this.pkg.unavailable.push(n);
          continue;
        }
        if (t === DELIVERED) continue;
        this.pkgFiles[n] = t;
        if (this.opened) this.chain = this.chain.then(() => this.core.request({ op: "set_file", file: n, text: t }).then(() => void 0));
      }
      const any = got.some(([, t]) => t !== null);
      this.pkg.building = any;
      tell();
      this.status(this.pending, this.undefinedNames);
      if (any)
        this.chain = this.chain.then(() => this.layout()).then(() => {
          this.pkg.building = false;
          tell();
          this.statusSoon();
        });
    });
  }
  /** After the core's discovery pass: what it asked for is here (or nowhere), so the first paint may build. Once per open. */
  /** A fetch that failed: kept with why (shown), and askable again. */
  failed(n, e) {
    this.asked.delete(n);
    const error = e instanceof Error ? e.message : String(e);
    this.pkg.failed = [...(this.pkg.failed ?? []).filter((f) => f.name !== n), { name: n, error }];
    return null;
  }
  /** Fetch `names` (and what they load, by the scan) into pkgFiles, before a session opens. */
  async prefetch(names) {
    const src = this.source;
    const want = [...new Set(names)].filter((n) => isPackageFile(n) && !(n in this.files) && !this.asked.has(n));
    if (src === noPackages || !want.length) return;
    this.pkg.source = src.label;
    const tell = () => this.sink.packages?.({ ...this.pkg, loading: [...this.pkg.loading], unavailable: [...this.pkg.unavailable], done: [...this.pkg.done ?? []], failed: [...this.pkg.failed ?? []] });
    for (const n of want) this.asked.add(n);
    this.pkg.loading.push(...want);
    tell();
    const one = async (n) => {
      const t = await src.resolve(n, this.engine).catch((e) => this.failed(n, e));
      this.pkg.loading = this.pkg.loading.filter((m) => m !== n);
      if (t !== null) this.pkg.done.push(n);
      if (t !== null && t !== DELIVERED) this.pkgFiles[n] = t;
      const more = t === null || t === DELIVERED ? [] : referenced(t).filter((m) => isPackageFile(m) && !(m in this.files) && !this.asked.has(m));
      for (const m of more) this.asked.add(m);
      this.pkg.loading.push(...more);
      tell();
      await Promise.all(more.map(one));
    };
    const t0 = this.now();
    this.tr("prefetch: start", want.length);
    await Promise.all(want.map(one));
    this.tr("prefetch: done", { files: this.pkg.done?.length ?? 0, ms: Math.round(this.now() - t0) });
    this.pkg.building = true;
    tell();
  }
  /** The page count of the last build; the page in view kept inside it
   * (a build with no page, an error, keeps the page: it comes back). */
  setPages(n) {
    this.pages = n;
    if (n > 0 && this.page >= n) this.page = n - 1;
  }
  /** The page in view (the one an edit paints first). */
  setPage(p) {
    this.page = Math.max(0, Math.min(p, Math.max(this.pages - 1, 0)));
    void this.peerPages();
    return this.sink.layout ? Promise.resolve() : this.showPage();
  }
  /** The pages' hashes, as the core has them now. */
  hashes = [];
  /** Tell the view which pages there are (by hash). */
  /** `given`: the hashes a reply carried (the open's), else asked for. */
  async layout(given) {
    if (!this.sink.layout) return;
    if (given) this.hashes = given;
    else {
      const r = await this.core.request({ op: "pages" });
      if (!r.ok || !r.json?.pages) return;
      this.hashes = r.json.pages;
    }
    this.tr("layout", { pages: this.hashes.length });
    this.setPages(this.hashes.length);
    if (this.hashes.length) this.sink.layout(this.hashes);
    this.drawPeers();
    void this.peerPages();
    if (this.sink.goto && !this.noOriginsPrefetch) this.glyphsAll();
  }
  /** Everything again, for a view that just joined (a detached PDF tab). */
  async resync() {
    if (!this.opened) return;
    if (this.o.remote) return this.watchJoined();
    this.sink.mains?.(Object.keys(this.files).filter((f) => f.endsWith(".tex")), this.main);
    this.status(this.pending);
    await (this.sink.layout ? this.layout() : this.showPage());
  }
  /** Page `k`, for a view that wants it. */
  async fetch(k) {
    if (!this.opened) return;
    const r = await this.core.request({ op: "png", page: k, dpi: this.dpi() });
    const img = image(r);
    if (img) this.sink.page(img, k, this.pages, r.json?.hash ?? this.hashes[k] ?? null);
  }
  async showPage() {
    if (!this.opened) return;
    if (this.sink.layout) return this.layout();
    const r = await this.core.request({ op: "png", page: this.page, dpi: this.dpi() });
    this.sink.page(image(r), this.page, this.pages);
  }
  /** (debugging: no glyph-origin requests after layouts) */
  noOriginsPrefetch = false;
  /** The errors replies carried (traps, panics), the last 20. */
  errorsSeen = [];
  /** An anonymized debug report (report.ts) for the user to send. */
  report(env) {
    const st = this.trace.filter((e) => e.k === "\u2190 status").at(-1)?.d;
    return report({
      ...env,
      paths: Object.keys(this.files),
      trace: this.trace,
      diagnostics: this.diags ?? [],
      status: st,
      packages: this.pkg,
      // (TeX's error by its message only: its context is the document's lines)
      errors: [...this.errorsSeen, ...this.texError ? [`TeX: ${this.texError.message}`] : []]
    });
  }
  /** Page `k`'s glyphs with their sources, as the core has them now (kept by the page's hash). */
  glyphCache = /* @__PURE__ */ new Map();
  /**
   * An edit replaced bytes [start, end) of `file` with `len` bytes: every
   * cached glyph's source range moved with it. A page the edit changed has a
   * new hash, so its glyphs are asked again (the cache is by page hash).
   */
  mapGlyphs(file, start, end, len) {
    const map = (p) => p < start ? p : p >= end ? p + len - (end - start) : start + len;
    for (const [key, gs] of this.glyphCache) {
      if (end > start && gs.some((g) => g.file === file && g.start < end && g.end > start)) {
        this.glyphCache.delete(key);
        continue;
      }
      for (const g of gs) {
        if (g.file !== file) continue;
        const [a, b] = [map(g.start), map(g.end)];
        g.start = a;
        g.end = Math.max(a, b);
      }
    }
  }
  /** Page `k`'s glyphs if they are here, at once (no request, no waiting on a build). */
  cached(k) {
    return this.glyphCache.get(`${k}:${this.hashes[k] ?? ""}`);
  }
  /** Every page's glyphs asked for, the one read first, one at a time: after each build, so lookups are local. */
  glyphsAll() {
    const gen = ++this.glyphGen;
    const n = this.hashes.length;
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    void (async () => {
      for (const k of order) {
        if (gen !== this.glyphGen) return;
        if (k >= 0 && k < n && !this.cached(k)) await this.glyphsOf(k);
      }
    })();
  }
  glyphGen = 0;
  glyphsOf(k) {
    const key = `${k}:${this.hashes[k] ?? ""}`;
    const have = this.glyphCache.get(key);
    if (have) return Promise.resolve(have);
    let p = this.glyphsAsked.get(key);
    if (!p) {
      p = this.glyphsFetch(k, key).finally(() => this.glyphsAsked.delete(key));
      this.glyphsAsked.set(key, p);
    }
    return p;
  }
  glyphsAsked = /* @__PURE__ */ new Map();
  async glyphsFetch(k, key) {
    const r = await this.core.request({ op: "origins", page: k });
    if (!r.ok || !r.json) return [];
    const path = (n) => {
      n = n.replace(/^\.\//, "");
      if (this.o.remote) return n;
      return n in this.files ? n : n + ".tex" in this.files ? n + ".tex" : null;
    };
    const gs = glyphs(r.json, path);
    if (!gs.length) return gs;
    if (this.glyphCache.size > 4 * Math.max(this.hashes.length, 100)) this.glyphCache.clear();
    this.glyphCache.set(key, gs);
    if (this.peers.length) this.drawPeers();
    return gs;
  }
  /** A double-click at (x, y) on page `k` (PDF points from its top left): the editor to its source. */
  async toSource(k, x, y) {
    if (!this.opened) return;
    const g = nearest(this.cached(k) ?? await this.glyphsOf(k), x, y);
    this.tr("sync\u2192source", { k, x, y, g });
    if (!g?.file) return;
    if (this.o.remote) return this.o.remote.source(g.file, g.start, g.end);
    const t = this.files[g.file];
    this.sink.goto?.(g.file, charOffset(t, g.start), charOffset(t, g.end));
  }
  /**
   * The editor selected [from, to) of `file` (UTF-16): the glyphs that came
   * from it, on every page, highlighted (an empty selection clears them).
   */
  async selectSource(file, from2, to) {
    if (!this.opened || !(file in this.files)) return;
    const t = this.files[file];
    return this.selectBytes(file, byteOffset(t, from2), byteOffset(t, to));
  }
  /**
   * Bytes [a, b) of `file`'s text selected (the glyph origins' offsets):
   * the glyphs that came from them, on every page, highlighted (an empty
   * selection clears them). [`selectSource`]'s, and an LSP editor's
   * (`{event: "select"}`).
   */
  async selectBytes(file, a, b) {
    if (!this.opened) return;
    if (a === b) return this.sink.marks?.([]);
    const out = [];
    for (let k = 0; k < Math.max(this.hashes.length, this.pages); k++) {
      const all = this.cached(k) ?? await this.glyphsOf(k);
      const hit = all.filter((g) => g.file === file && !g.synth && g.start < b && g.end > a);
      if (hit.length) out.push({ k, boxes: boxes(hit, all) });
    }
    this.tr("select\u2192page", { file, a, b, pages: out.map((m) => m.k) });
    this.sink.marks?.(out);
  }
  /**
   * Text selected on the pages ([x, y top, w, h] boxes in PDF points, by
   * page): the editor selects the source it came from (the file most of it
   * came from, from its first glyph's start to its last's end).
   */
  async selectPage(sel) {
    if (!this.opened || !sel.length) return;
    const by = /* @__PURE__ */ new Map();
    for (const { k, rects } of sel) {
      for (const g of this.cached(k) ?? await this.glyphsOf(k)) {
        if (!g.file || g.synth) continue;
        const [gx, gy] = [g.x + 1, g.y - 2];
        if (!rects.some(([x, y, w, h]) => gx >= x && gx <= x + w && gy >= y && gy <= y + h)) continue;
        const r = by.get(g.file) ?? { lo: Infinity, hi: -Infinity, n: 0 };
        by.set(g.file, { lo: Math.min(r.lo, g.start), hi: Math.max(r.hi, g.end), n: r.n + 1 });
      }
    }
    const best = [...by].sort((x, y) => y[1].n - x[1].n)[0];
    this.tr("select\u2192source", { pages: sel.map((s) => s.k), file: best?.[0], glyphs: best?.[1].n ?? 0 });
    if (!best) return;
    const [file, { lo, hi }] = best;
    if (this.o.remote) return void this.o.remote.select?.(file, lo, hi);
    const t = this.files[file];
    if (t === void 0) return;
    this.sink.goto?.(file, charOffset(t, lo), charOffset(t, hi), false);
  }
  /**
   * The editor's cursor at `pos` (UTF-16) in `file`, as it moves: the word it
   * is in highlighted on the page, at once, from the glyphs already here
   * (none asked for: a page not yet here is skipped); scrolled to only when
   * the cursor changed line.
   */
  follow(file, pos) {
    if (!this.opened || !(file in this.files) || !this.sink.mark) return;
    const { at, word, line } = place2(this.files[file], pos);
    this.followBytes(file, at, word, line);
  }
  /**
   * Collaborators' cursors (the host's, Overleaf's: never the user's own),
   * each at `pos` (UTF-16) in `file`: the word there marked on its page,
   * by follow's rule, in the collaborator's hue with their label, never
   * scrolling, apart from the user's own mark. From the glyphs here; a
   * cursor on a page without them is shown once they come (the pages
   * around the one in view asked for). Each call is the whole set: []
   * clears.
   */
  others(file, others) {
    this.peers = others.map((o) => ({ file, ...o }));
    this.drawPeers();
    void this.peerPages();
  }
  peers = [];
  /** The collaborators' marks, from the glyphs already here. */
  drawPeers() {
    if (!this.sink.others || !this.opened) return;
    const out = [];
    for (const p of this.peers) {
      const t = this.files[p.file];
      if (t === void 0) continue;
      const { at, word, line } = place2(t, p.pos);
      for (let k = 0; k < this.hashes.length; k++) {
        const all = this.cached(k);
        const hit = all ? this.wordOn(all, p.file, at, word, line) : [];
        if (!all || !hit.length) continue;
        out.push({ k, boxes: boxes(hit, all), label: p.label, hue: p.hue });
        break;
      }
    }
    this.sink.others(out);
  }
  /** The glyphs of the pages around the one in view asked for, then the collaborators drawn again. */
  async peerPages() {
    if (!this.peers.length || !this.sink.others) return;
    const ks = [this.page, this.page + 1, this.page - 1].filter((k) => k >= 0 && k < this.hashes.length && !this.cached(k));
    if (!ks.length) return;
    await Promise.all(ks.map((k) => this.glyphsOf(k)));
    this.drawPeers();
  }
  /**
   * The glyphs to mark for a cursor at byte `at` of `file`, in word `w`
   * (bytes, none: not in a word) of line `line` (bytes), among a page's
   * glyphs `all` (none: not on that page): a word, never the whole line,
   * while typing: the cursor's word, else, not typeset yet (just typed),
   * the typeset word nearest it on its line, the glyph's own; the line
   * only with no word near at all.
   */
  wordOn(all, file, at, w, line) {
    const onLine = from(all, file, line[0], line[1]);
    if (!onLine.length) return [];
    let word = w ? from(onLine, file, w[0], w[1]) : [];
    if (!word.length) {
      const g = glyphNear(onLine, at);
      const t = this.files[file];
      const gw = g && t !== void 0 ? wordBytes(new TextEncoder().encode(t), g.start) : null;
      word = gw ? from(onLine, file, gw[0], gw[1]) : g ? onLine.filter((x) => x.start === g.start) : [];
    }
    return word.length ? word : lineAt(onLine, at);
  }
  /**
   * The cursor at byte `at` of `file`'s text (the glyph origins' offsets),
   * in word `w` (bytes, none: not in a word) of line `line` (bytes): the
   * word highlighted on the page, else the line's glyphs nearest it, from
   * the glyphs already here (a page not yet here asked for, then shown if
   * the cursor is still there); scrolled to only when the cursor changed
   * line. [`follow`]'s, and an LSP editor's (`{event: "follow"}`).
   */
  followBytes(file, at, w, line) {
    if (!this.opened || !this.sink.mark) return;
    const [a, b] = line;
    const key = `${file}:${a}`;
    const scroll = key !== this.followLine;
    this.followLine = key;
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    for (const k of order) {
      const all = this.cached(k);
      if (!all) continue;
      const hit = this.wordOn(all, file, at, w, line);
      if (!hit.length) continue;
      this.followWant = "";
      this.sink.mark(k, boxes(hit, all), scroll);
      return;
    }
    const want = `${file}:${at}`;
    this.followWant = want;
    void (async () => {
      for (const k of order) {
        if (this.followWant !== want) return;
        if (k >= this.hashes.length || this.cached(k)) continue;
        const all = await this.glyphsOf(k);
        if (from(all, file, a, b).length) {
          if (this.followWant === want) {
            this.followLine = "";
            this.followBytes(file, at, w, line);
          }
          return;
        }
      }
    })();
  }
  followLine = "";
  followWant = "";
  /** The last build's transcript, the job's `.log` (the viewer's "View log"; none: the core has none yet). */
  async log() {
    if (!this.opened) return null;
    for (let i = 0; i < 50; i++) {
      const r = await this.core.request({ op: "log" });
      if (!r.ok) return null;
      if (r.json?.pending) {
        await new Promise((go) => setTimeout(go, 100));
        continue;
      }
      const t = r.json?.text;
      return typeof t === "string" && t ? t : null;
    }
    return null;
  }
  /**
   * The boxes of [`start`, `end`) (bytes, as a problem's range) of `file` on
   * the pages: the culprit under a problem's wavy underline. From the pages'
   * glyphs, the page in view first and the rest asked for as needed; [] where
   * none of it is typeset.
   */
  async boxesOf(file, start, end) {
    if (!this.opened) return [];
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    const out = [];
    for (const k of order) {
      if (k >= this.hashes.length) continue;
      const all = this.cached(k) ?? await this.glyphsOf(k);
      const hit = from(all, file, start, Math.max(end, start + 1));
      if (hit.length) out.push({ k, boxes: boxes(hit, all) });
    }
    return out.sort((x, y) => x.k - y.k);
  }
  /** The place `pos` (UTF-16) in `file`: its line's glyphs highlighted on the page (the page in view first). */
  async toPage(file, pos) {
    if (!this.opened || !(file in this.files)) return;
    const t = this.files[file];
    const lo = t.lastIndexOf("\n", pos - 1) + 1, hiC = t.indexOf("\n", pos);
    const [a, b] = [byteOffset(t, lo), byteOffset(t, hiC < 0 ? t.length : hiC)];
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    for (const k of order) {
      const all = this.cached(k) ?? await this.glyphsOf(k);
      const hit = lineAt(from(all, file, a, b), byteOffset(t, pos));
      if (hit.length) {
        this.tr("sync\u2192page", { file, pos, k, glyphs: hit.length });
        this.sink.mark?.(k, boxes(hit, all));
        return;
      }
    }
    this.tr("sync\u2192page", { file, pos, k: -1 });
  }
  async pdf() {
    this.flush();
    await this.chain;
    const t = this.now();
    const r = await this.core.request({ op: "pdf" });
    this.sink.latency(`PDF made in ${(this.now() - t).toFixed(1)} ms`);
    return r.pdf;
  }
  /** `file` is now `text`: only the differing middle is sent. */
  sync(file, text) {
    const old = this.batches.get(file)?.text ?? this.files[file];
    if (old === void 0) {
      this.files[file] = text;
      if (this.opened) this.chain = this.chain.then(() => this.core.request({ op: "set_file", file, text }).then(() => void 0));
      return;
    }
    const e = diff(old, text);
    if (e) this.queue(file, [e]);
  }
  queue(file, edits) {
    if (!edits.length) return;
    let b = this.batches.get(file);
    if (!b) this.batches.set(file, b = new Batch(this.files[file] ?? ""));
    if (!(file in this.files)) this.files[file] = "";
    if (!b.size) this.since.set(file, this.now());
    for (const e of edits) b.push(e);
    this.keystrokes += edits.length;
    if (!this.scheduled) {
      this.scheduled = true;
      const f = () => this.flush();
      if (this.o.schedule) this.o.schedule(f);
      else setTimeout(f, 0);
    }
  }
  /** A send loop is running (it takes what is queued when each build ends). */
  busy = false;
  /** Edits the host gave, and builds they took (the batching's effect). */
  keystrokes = 0;
  builds = 0;
  /**
   * Send the queued edits, in order; the last of a round painted first
   * (edit_view). Backpressure: at most one round in flight. What is typed
   * meanwhile merges into the batches, and goes as one edit when the core
   * is free: a fast core builds every keystroke, a slow one the last N
   * together, never a state already stale.
   */
  /** Paused (the extension turned off): edits keep merging in their batches, sent on resume. */
  paused = false;
  pause(on) {
    this.paused = on;
    if (!on) void this.flush();
  }
  flush() {
    this.scheduled = false;
    if (!this.opened || this.busy || this.paused) return this.chain;
    this.busy = true;
    this.sink.busy?.(true);
    this.chain = this.chain.then(async () => {
      try {
        for (; ; ) {
          const round = [...this.batches].filter(([, b]) => b.size);
          if (!round.length) break;
          for (const [file, b] of round) if (!await this.send(file, b)) return;
        }
        await this.layout();
        this.statusSoon();
      } finally {
        this.busy = false;
        this.sink.busy?.(false);
        if ([...this.batches.values()].some((b) => b.size)) setTimeout(() => void this.flush(), 0);
      }
    });
    return this.chain;
  }
  statusTimer = null;
  /**
   * The pending/undefined scan flattens the whole program (3 ms at 180 KB,
   * ten times an edit): once the edits have settled, at most every 300 ms.
   */
  /** `phitex watch`'s last build's problems (Options.remote), as diagnostics. */
  watchDiags = [];
  /** A build of the watch's under way (its progress told, not yet settled). */
  watchBuilding = false;
  watchBusy(on) {
    if (on === this.watchBuilding) return;
    this.watchBuilding = on;
    this.sink.busy?.(on);
    if (!on) this.sink.streaming?.(null);
  }
  /** The watch joined (or joined again): its pages, outline and last problems. */
  async watchJoined() {
    await this.layout();
    this.status();
    void this.watchOutline();
    const r = await this.core.request({ op: "diagnostics" });
    if (r.ok) this.onWatch({ event: "diagnostics", items: r.json?.items ?? [] });
  }
  /** The document's outline asked for now and given to the view (it shows the contents). */
  async outline() {
    if (!this.sink.outline) return;
    const r = await this.core.request({ op: "outline" });
    if (r.ok) this.sink.outline(r.json?.items ?? []);
  }
  /** After a build: the outline, only while the view shows it (else asked when it does). */
  async watchOutline() {
    if (!this.sink.outline || this.sink.outlineWanted?.() === false) return;
    const r = await this.core.request({ op: "outline" });
    if (r.ok) this.sink.outline(r.json?.items ?? []);
  }
  onWatch(e) {
    switch (e.event) {
      case "page": {
        this.watchBusy(true);
        const hs = [...this.hashes];
        while (hs.length < Math.max(e.k + 1, e.pages ?? 0)) hs.push("");
        hs[e.k] = e.hash;
        this.hashes = hs;
        this.setPages(hs.length);
        this.sink.layout?.(hs);
        this.drawPeers();
        return;
      }
      case "progress":
        this.watchBusy(true);
        if (e.phase === "fetching" && e.pack) return this.onFetching({ pack: e.name ?? e.pack, name: e.pack });
        if (!this.hashes.length && (this.pkg.loading.length || !this.pkg.building)) {
          this.pkg.done = [...this.pkg.done ?? [], ...this.pkg.loading];
          this.pkg.loading = [];
          this.inBuild = [];
          this.pkg.building = true;
          this.tellPackages();
        }
        this.sink.streaming?.({ pages: e.pages ?? 0, phase: e.phase ?? "typesetting" });
        return;
      case "superseded":
        this.watchBusy(true);
        return;
      case "diagnostics":
        this.watchDiags = e.items.map(asDiagnostic);
        this.status();
        return;
      case "settled":
        if (!e.settling) this.watchBusy(false), this.sink.preparing?.(false);
        this.chain = this.chain.then(() => this.layout()).then(() => (this.status(), this.watchOutline()));
        return;
      case "sync":
        void this.syncTo(e.file, e.lo, e.hi, e.at, e.page);
        return;
    }
  }
  /** Forward search: bytes [lo, hi) of `file` (a line), the glyphs from it nearest `at`'s highlighted, the page in view tried first. */
  async syncTo(file, lo, hi, at, hint) {
    const n = this.hashes.length;
    const first = hint ?? this.page;
    for (const k of [first, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== first)]) {
      const all = this.cached(k) ?? await this.glyphsOf(k);
      const hit = lineAt(from(all, file, lo, hi), at);
      if (hit.length) return this.sink.mark?.(k, boxes(hit, all), true);
    }
  }
  statusSoon() {
    if (this.statusTimer) return;
    this.statusTimer = setTimeout(async () => {
      this.statusTimer = null;
      if (this.busy || !this.opened) return this.statusSoon();
      const r = await this.core.request({ op: "status" });
      if (r.ok && r.json) {
        this.warmCheck(r.json.how);
        this.setPages(r.json.pages);
        this.noteError(r.json);
        this.status(r.json.pending, r.json.undefined_names);
        this.fetchPackages(r.json.missing);
      }
    }, 300);
  }
  async send(file, b) {
    if (this.warming && !this.warmSlow) {
      this.warmSlow = true;
      this.sink.warmup?.({ state: "preparing", since: Date.now() - (this.now() - this.warming), slow: true });
    }
    const t0 = this.since.get(file) ?? this.now();
    const tSend = this.now();
    const edits = b.take();
    this.files[file] = b.text;
    if (!this.minted && usesMinted(b.text)) {
      this.tr("minted: open again, with its runner", {});
      await this.reopen();
      return false;
    }
    if (file === this.main && this.o.engine) {
      const pre = preambleOf(b.text);
      if (pre !== this.preamble) {
        this.preamble = pre;
        const want = this.o.engine(b.text);
        if (want !== this.wanted) {
          this.tr("engine: open again", { from: this.wanted, to: want });
          await this.reopen();
          return false;
        }
      }
    }
    for (const [i, e] of edits.entries()) {
      const last = i === edits.length - 1;
      this.builds++;
      const r = await this.core.request({ op: "edit", file, ...e, page: last ? this.page : -1, dpi: this.dpi() });
      if (!r.ok) {
        this.sink.error((typeof r.json?.error === "string" ? r.json.error : void 0) ?? r.error ?? "edit failed");
        if (r.error?.includes("trapped")) await this.reopen();
        return false;
      }
      if (!last) continue;
      const tBack = this.now();
      const j = r.json;
      this.setPages(j.pages);
      this.noteError(j);
      this.status(this.pending);
      const img = image(r);
      if (img) this.sink.page(img, this.page, this.pages, j.painted_hash ?? null);
      else if (!this.sink.layout) await this.showPage();
      const e2e = this.now() - t0;
      this.sink.painted?.(e2e);
      this.sink.latency(
        `${e2e.toFixed(0)} ms keystroke\u2192page \xB7 core ${j.paint_ms.toFixed(1)} ms \xB7 ${this.keystrokes} edits \u2192 ${this.builds} builds`,
        [
          `first paint      ${j.paint_ms.toFixed(2)} ms (in the core: edit_view, the page painted)`,
          `fixed point      ${j.total_ms.toFixed(2)} ms (every page, the .aux/.bbl loops)${j.wrong ? " \xB7 first paint was superseded" : ""}`,
          `keystroke\u2192page   ${e2e.toFixed(1)} ms = queued ${(tSend - t0).toFixed(1)} + round trip ${(tBack - tSend).toFixed(1)} + paint ${(this.now() - tBack).toFixed(1)}`,
          `rebuilt          ${j.stats.rebuilt} of ${j.stats.rebuilt + j.stats.reused} chunks, ${j.stats.passes} pass${j.stats.passes === 1 ? "" : "es"}`,
          `batching         ${this.keystrokes} edits \u2192 ${this.builds} builds (one in flight; the rest merged)`
        ].filter(Boolean).join("\n")
      );
    }
    return true;
  }
  /** PhiTeX's invariant: the incremental state = a fresh build of the same text. */
  async check() {
    if (!this.opened) return;
    await this.flush();
    const file = this.open ?? this.main;
    const r = await this.core.request({ op: "check", file, expect: this.files[file] });
    this.sink.check?.(r.json);
    return r.json;
  }
  /** The texts the session has sent (tests). */
  text(file) {
    return this.batches.get(file)?.text ?? this.files[file];
  }
  /** Every project file's text as it is now (queued edits in): a compare's new side. */
  texts() {
    const out = {};
    for (const f of Object.keys(this.files)) out[f] = this.text(f);
    return out;
  }
};

// viewer/src/text.ts
function fold(c) {
  const f = c.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  return f.length === 1 ? f : c.toLowerCase().length === 1 ? c.toLowerCase() : c;
}
var COMBINING = {
  "\xA8": "\u0308",
  "\xB4": "\u0301",
  "`": "\u0300",
  "\u02C6": "\u0302",
  "^": "\u0302",
  "\u02DC": "\u0303",
  "~": "\u0303",
  "\xAF": "\u0304",
  "\u02C9": "\u0304",
  "\u02D8": "\u0306",
  "\u02D9": "\u0307",
  "\u02DA": "\u030A",
  "\xB8": "\u0327",
  "\u02C7": "\u030C",
  "\u02DD": "\u030B",
  "\u02DB": "\u0328"
};
function compose(cs) {
  const out = cs.map((c) => ({ ...c }));
  const over = (a, b) => b !== void 0 && !COMBINING[b.c] && b.c.trim() !== "" && (a.x + a.r) / 2 >= b.x - 0.5 && (a.x + a.r) / 2 <= b.r + 0.5;
  for (let k = 0; k < out.length; k++) {
    const a = out[k];
    const mark = COMBINING[a.c];
    if (!mark) continue;
    const on = over(a, out[k + 1]) ? k + 1 : over(a, out[k - 1]) ? k - 1 : -1;
    if (on < 0) continue;
    out[on].c = (out[on].c + mark).normalize("NFC");
    a.c = "";
  }
  return out.filter((c) => c.c !== "");
}
function composeLine(l) {
  if (!l.xs || !l.rs) return l;
  const units = Array.from({ length: l.text.length }, (_, i) => l.text[i]);
  const cs = compose(units.map((c, i) => ({ c, x: l.xs[i] ?? l.x, r: l.rs[i] ?? l.x + l.w })));
  const text = cs.map((c) => c.c).join("");
  const xs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.x));
  const rs = cs.flatMap((c) => Array.from({ length: c.c.length }, () => c.r));
  return { ...l, text, xs, rs };
}
function search(pages2, q, max = 1e3) {
  const needle = Array.from(q.replace(/\s+/g, " ").trim(), fold).join("");
  if (!needle) return [];
  const out = [];
  for (let k = 0; k < pages2.length && out.length < max; k++) {
    const lines = pages2[k];
    let hay = "";
    const at = [];
    const col = [];
    lines.forEach((l, i) => {
      if (i) hay += " ", at.push(-1), col.push(-1);
      for (let j = 0; j < l.text.length; j++) hay += fold(l.text[j]), at.push(i), col.push(j);
    });
    for (let from2 = hay.indexOf(needle); from2 >= 0 && out.length < max; from2 = hay.indexOf(needle, from2 + needle.length)) {
      const boxes2 = [];
      let cur = -1, x = 0, r = 0;
      const flush = () => {
        if (cur < 0) return;
        const l = lines[cur];
        boxes2.push([x, l.y - l.size * 0.78, r - x, l.size * 1.02]);
      };
      for (let u = from2; u < from2 + needle.length; u++) {
        const i = at[u];
        if (i < 0) continue;
        const l = lines[i], xl = l.xs?.[col[u]] ?? l.x, xr = l.rs?.[col[u]] ?? l.x + l.w;
        if (i !== cur) flush(), cur = i, x = xl, r = xr;
        else x = Math.min(x, xl), r = Math.max(r, xr);
      }
      flush();
      if (boxes2.length) out.push({ k, boxes: boxes2 });
    }
  }
  return out;
}

// viewer/src/strips.ts
var Strips = class {
  /** The bitmaps kept, by key (the oldest dropped past `max`). */
  kept = /* @__PURE__ */ new Map();
  max = 4e3;
  src;
  /** Where the last frame's numbers are shown (none: not shown). */
  chip;
  constructor(src, chip2 = null) {
    this.src = src;
    this.chip = chip2;
  }
  /** Each page's bands' keys as last shown (what a frame of it most likely keeps). */
  shown = /* @__PURE__ */ new Map();
  /** Page `k` at `scale` pixels a point: its element (canvas and text), size and links. */
  async page(k, scale) {
    const t0 = performance.now();
    let f = null;
    let t1 = t0;
    if (this.src.frameWith) {
      const have = (this.shown.get(k) ?? []).filter((key) => this.kept.has(key));
      const buf = await this.src.frameWith(k, scale, have);
      if (!buf || buf.byteLength < 4) return null;
      const n = new DataView(buf).getUint32(0, true);
      f = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, n)));
      t1 = performance.now();
      await this.take(f.sent ?? [], buf, 4 + n);
    } else {
      f = await this.src.frame(k, scale);
      if (!f) return null;
      t1 = performance.now();
    }
    if (!f) return null;
    this.shown.set(k, f.bands.map((b) => b[0]));
    const want = [...new Set(f.bands.map((b) => b[0]).filter((key) => !this.kept.has(key)))];
    if (want.length) await this.fetch(want);
    const t2 = performance.now();
    const c = document.createElement("canvas");
    c.width = f.pw;
    c.height = f.ph;
    const g = c.getContext("2d");
    g.fillStyle = "#fff";
    g.fillRect(0, 0, f.pw, f.ph);
    for (const [key, y] of f.bands) {
      const b = this.kept.get(key);
      if (b) g.drawImage(b, 0, y);
    }
    const t3 = performance.now();
    if (this.chip)
      this.chip.textContent = `p${k + 1}: ${f.drawn} drawn, ${f.kept} kept \xB7 watch ${(f.us.all / 1e3).toFixed(1)} ms (probe ${(f.us.probe / 1e3).toFixed(1)}, draw ${(f.us.draw / 1e3).toFixed(1)}) \xB7 fetch ${(t1 - t0).toFixed(1)}+${(t2 - t1).toFixed(1)} \xB7 compose ${(t3 - t2).toFixed(1)} ms`;
    const el = document.createElement("div");
    el.className = "phx-page";
    el.append(c, textLayer(f.text.map(composeLine), f.w, f.h));
    return { el, w: f.w, h: f.h, links: f.links };
  }
  /** The strips of `keys`, made bitmaps and kept. */
  async fetch(keys) {
    const buf = await this.src.strips(keys);
    if (buf) await this.take(keys, buf, 0);
  }
  /** The strips of `keys` in `buf` from byte `at` (each its size, then its pixels), made bitmaps and kept. */
  async take(keys, buf, at) {
    const dv = new DataView(buf);
    const made = [];
    for (const key of keys) {
      if (at + 8 > buf.byteLength) break;
      const w = dv.getUint32(at, true), h = dv.getUint32(at + 4, true);
      at += 8;
      if (w && h) {
        const px = new Uint8ClampedArray(buf, at, w * h * 4);
        made.push(
          createImageBitmap(new ImageData(px, w, h)).then((b) => {
            this.kept.set(key, b);
          })
        );
      }
      at += w * h * 4;
    }
    await Promise.all(made);
    for (const k of this.kept.keys()) {
      if (this.kept.size <= this.max) break;
      this.kept.get(k)?.close();
      this.kept.delete(k);
    }
  }
};
var SVG = "http://www.w3.org/2000/svg";
function textLayer(lines, w, h) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-text");
  svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  svg.setAttribute("preserveAspectRatio", "none");
  for (const l of lines) {
    const t = document.createElementNS(SVG, "text");
    t.setAttribute("x", l.xs ? l.xs.map((x) => x.toFixed(2)).join(" ") : String(l.x));
    t.setAttribute("y", String(l.y));
    t.setAttribute("font-size", String(l.size));
    t.textContent = l.text;
    svg.append(t);
  }
  return svg;
}

// viewer/src/compare.ts
var LATEXDIFF_LOOK = { markup: "underline", subtype: "safe", add_color: "#0000ff", del_color: "#ff0000" };
var DiffControls = class {
  state = null;
  panel;
  run;
  actions;
  lookOpen = false;
  constructor(panel2, run) {
    this.panel = panel2;
    this.run = run;
    this.actions = {
      prev: () => this.step(-1),
      next: () => this.step(1),
      toggle: () => {
        const st = this.state;
        if (!st || st.busy) return;
        st.showing = st.showing === "diff" ? "current" : "diff";
        run.show(st.showing);
        this.bar();
      },
      pdf: () => run.download("pdf"),
      tex: () => run.download("tex"),
      settings: async () => {
        if (this.lookOpen) return this.closeLook();
        this.lookOpen = true;
        panel2.diffSettings(await run.look(), (l) => run.restyle(l), LATEXDIFF_LOOK);
      },
      close: () => this.stop()
    };
  }
  closeLook() {
    this.lookOpen = false;
    this.panel.diffSettings(null);
  }
  /** A compare is on (its bar shown). */
  get active() {
    return !!this.state;
  }
  bar() {
    this.panel.diffBar(this.state, this.actions);
  }
  step(d) {
    const st = this.state;
    if (!st || st.busy || !st.changes) return;
    st.at = (Math.max(st.at, d > 0 ? 0 : 1) - 1 + d + st.changes) % st.changes + 1;
    if (st.showing === "current") this.actions.toggle();
    this.run.goto(st.at);
    this.bar();
  }
  /** Compare with `v`, named `title`, `when` it was ("3 h ago · v42"). */
  async start(v, title, when) {
    this.state = { title, when, changes: 0, at: 0, showing: "diff", busy: "Fetching that version\u2026" };
    this.bar();
    try {
      const r = await this.run.start(
        v,
        (busy2) => {
          if (this.state) this.state.busy = busy2, this.bar();
        },
        (n) => {
          const st = this.state;
          if (!st || st.busy) return;
          st.changes = n;
          st.at = Math.min(st.at, n);
          this.bar();
        }
      );
      if (!this.state) return;
      this.state.busy = void 0;
      this.state.changes = r.changes;
    } catch (err) {
      if (!this.state) return;
      this.state.busy = `Couldn't compare: ${err.message ?? err}`;
    }
    this.bar();
  }
  stop() {
    this.closeLook();
    this.state = null;
    this.run.stop();
    this.bar();
  }
  /** A key while comparing (not in an editor or a field): handled, or false. */
  key(e) {
    if (!this.state || e.ctrlKey || e.metaKey || e.altKey) return false;
    const t = e.target;
    if (t?.closest?.("input, textarea, [contenteditable=true], .cm-editor")) return false;
    if (e.key === "d" || e.key === "D") this.actions.toggle();
    else if (e.key === "n") this.step(1);
    else if (e.key === "N") this.step(-1);
    else if (e.key === "Escape") this.lookOpen ? this.closeLook() : this.stop();
    else return false;
    e.preventDefault();
    return true;
  }
};

// viewer/src/commits.ts
function layout(cs) {
  const active = [];
  const rows = [];
  let pending = [];
  let width = 1;
  for (const c of cs) {
    let lane = active.indexOf(c.id);
    if (lane < 0) {
      lane = active.indexOf(null);
      if (lane < 0) lane = active.push(null) - 1;
    }
    if (rows.length) rows[rows.length - 1].down = pending.map(([x, s]) => [x, active[s] === c.id ? lane : s]);
    for (let s = 0; s < active.length; s++) if (active[s] === c.id) active[s] = null;
    const by = active.map((id) => id !== null);
    const from2 = /* @__PURE__ */ new Map();
    c.parents.forEach((p, i) => {
      let s = active.indexOf(p);
      if (s < 0) {
        s = i === 0 && active[lane] === null ? lane : active.indexOf(null);
        if (s < 0) s = active.push(null) - 1;
        active[s] = p;
      }
      from2.set(s, lane);
    });
    while (active.length && active[active.length - 1] === null) active.pop();
    pending = [];
    active.forEach((id, s) => {
      if (id === null) return;
      if (by[s]) pending.push([s, s]);
      const f = from2.get(s);
      if (f !== void 0 && !(by[s] && f === s)) pending.push([f, s]);
    });
    width = Math.max(width, lane + 1, active.length);
    rows.push({ lane, down: [] });
  }
  if (rows.length) rows[rows.length - 1].down = pending;
  return { rows, width };
}
function ago(t, now = Date.now() / 1e3) {
  const s = Math.max(0, now - t);
  const units = [[31536e3, "year"], [2592e3, "month"], [86400, "day"], [3600, "hour"], [60, "minute"]];
  for (const [n, u] of units)
    if (s >= n) {
      const k = Math.floor(s / n);
      return `${k} ${u}${k === 1 ? "" : "s"} ago`;
    }
  return "just now";
}
var esc4 = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
var ROW = 24;
var LANE = 12;
var COLORS = ["#3e63dd", "#e5484d", "#30a46c", "#f5a524", "#8e4ec6", "#12a594", "#d6409f", "#978365"];
var COMMITS_CSS = `
.phx-commits { font: 12px system-ui, sans-serif; overflow: auto; }
.phx-commits .row { display: flex; align-items: center; height: ${ROW}px; cursor: pointer; white-space: nowrap; }
.phx-commits .row:hover { background: rgba(127,127,127,.12); }
.phx-commits .row.inrange { background: rgba(62,99,221,.10); }
.phx-commits .row.from, .phx-commits .row.to { background: rgba(62,99,221,.22); }
.phx-commits svg { flex: none; overflow: visible; }
.phx-commits .txt { overflow: hidden; text-overflow: ellipsis; padding-left: 4px; }
.phx-commits .sha { font: 11px ui-monospace, monospace; opacity: .65; margin-right: 6px; }
.phx-commits .who { opacity: .65; margin-left: 6px; }
.phx-commits .pill { display: inline-block; padding: 0 6px; margin-right: 4px; border-radius: 999px; font-size: 10.5px; line-height: 16px; }
.phx-commits .pill.head { background: #3e63dd; color: #fff; }
.phx-commits .pill.branch { background: rgba(48,164,108,.2); color: #18794e; }
.phx-commits .pill.remote { background: rgba(127,127,127,.18); }
.phx-commits .pill.tag { background: rgba(245,165,36,.25); color: #915930; }
.phx-commits .end { font-size: 10px; font-weight: 600; color: #3e63dd; margin-right: 4px; }
.phx-commits .note { padding: 6px 10px; opacity: .7; }
.phx-commits .note.err { color: #e5484d; opacity: 1; }
.phx-commits button { font: inherit; border: 1px solid rgba(127,127,127,.4); background: none; color: inherit; border-radius: 5px; padding: 2px 8px; cursor: pointer; }
`;
var KINDS = /* @__PURE__ */ new Set(["head", "branch", "remote", "tag"]);
var Commits = class {
  el;
  host;
  commits = [];
  more = true;
  loading = false;
  error = "";
  /** The pair chosen (ids; `to` null: the working tree). */
  from = null;
  to = null;
  page = 200;
  constructor(root, host2) {
    this.host = host2;
    this.el = document.createElement("div");
    this.el.className = "phx-commits";
    root.append(this.el);
    this.el.addEventListener("click", (e) => this.click(e));
    this.el.addEventListener("dblclick", (e) => {
      const c = this.at(e.target);
      if (c && c.parents.length) this.choose(c.parents[0], c.id);
    });
    this.render();
  }
  get element() {
    return this.el;
  }
  /** Mark the pair shown (as the host has it: a revision, a commit id or its prefix; `to` null: the working tree). */
  mark(from2, to) {
    this.from = from2;
    this.to = to;
    this.render();
  }
  /** The commits: the first page now, more as asked. */
  async load() {
    if (this.loading || !this.more) return;
    this.loading = true;
    this.render();
    const got = await this.host.load(this.commits.length, this.page);
    this.loading = false;
    if (typeof got === "string") {
      this.error = got;
      this.more = false;
    } else {
      this.commits.push(...got);
      this.more = got.length === this.page;
    }
    this.render();
  }
  /** The history changed (a commit made, a branch moved): read again. */
  reload() {
    this.commits = [];
    this.more = true;
    this.error = "";
    return this.load();
  }
  choose(from2, to) {
    this.from = from2;
    this.to = to;
    this.render();
    this.host.pick(from2, to);
  }
  at(t) {
    const row = t.closest(".row[data-i]");
    return row ? this.commits[Number(row.dataset.i)] : void 0;
  }
  /** The row of revision `rev` (a commit id or its prefix, a ref's name, HEAD), else -1. */
  find(rev) {
    if (!rev) return -1;
    if (rev === "HEAD") return this.commits.findIndex((c) => c.refs.some((r) => r.kind === "head"));
    return this.commits.findIndex((c) => c.id.startsWith(rev) || c.refs.some((r) => r.name === rev));
  }
  click(e) {
    const t = e.target;
    if (t.closest("[data-more]")) return void this.load();
    const row = t.closest(".row");
    if (!row || e.detail > 1) return;
    const c = this.at(t);
    if (e.shiftKey) {
      if (!c) return this.choose(this.from ?? "HEAD", null);
      const fi2 = this.find(this.from), ti2 = this.commits.indexOf(c);
      const from2 = fi2 > ti2 ? this.commits[fi2].id : c.parents[0] ?? c.id;
      return this.choose(from2, c.id);
    }
    if (!c) return;
    const ti = this.find(this.to), fi = this.commits.indexOf(c);
    this.choose(c.id, this.to && ti >= 0 && ti < fi ? this.commits[ti].id : null);
  }
  render() {
    const scroll = this.el.scrollTop;
    this.el.innerHTML = this.html();
    this.el.scrollTop = scroll;
  }
  html() {
    if (this.error) return `<div class="note err">${esc4(this.error)}</div>`;
    if (!this.commits.length) return `<div class="note">${this.loading ? "reading the history\u2026" : "no commits"}</div>`;
    const { rows, width } = layout(this.commits);
    const W = (width + 1) * LANE;
    const x = (l) => LANE / 2 + l * LANE + 2;
    const fi = this.find(this.from);
    const ti = this.to ? this.find(this.to) : -1;
    const tree = this.from !== null && !this.to;
    const top = `<div class="row${tree ? " to" : ""}" title="Shift-click: diff the working tree"><svg width="${W}" height="${ROW}"><circle cx="${x(0)}" cy="${ROW / 2}" r="4" fill="none" stroke="#888" stroke-dasharray="2 2"/><line x1="${x(0)}" y1="${ROW / 2 + 4}" x2="${x(0)}" y2="${ROW * 1.5}" stroke="#888" stroke-dasharray="2 2"/></svg><span class="txt">${tree ? '<span class="end">TO</span>' : ""}<i>the working tree</i></span></div>`;
    const lines = this.commits.map((c, i) => {
      const r = rows[i];
      const col = COLORS[r.lane % COLORS.length];
      const svg = r.down.map(([a, b]) => `<path d="M${x(a)} ${ROW / 2} C${x(a)} ${ROW} ${x(b)} ${ROW} ${x(b)} ${ROW * 1.5}" stroke="${COLORS[b % COLORS.length]}" fill="none" stroke-width="1.6"/>`);
      svg.push(c.parents.length > 1 ? `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4.5" fill="Canvas" stroke="${col}" stroke-width="2"/>` : `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4" fill="${col}"/>`);
      const pills = c.refs.map((f) => `<span class="pill ${KINDS.has(f.kind) ? f.kind : "branch"}">${esc4(f.name)}</span>`).join("");
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? '<span class="end">FROM</span>' : i === ti ? '<span class="end">TO</span>' : "";
      const title = `${c.subject}
${c.author}, ${new Date(c.time * 1e3).toLocaleString()}
click: From \xB7 shift-click: To \xB7 double-click: against its first parent`;
      return `<div class="row${cls}" data-i="${i}" title="${esc4(title)}"><svg width="${W}" height="${ROW}">${svg.join("")}</svg><span class="txt">${end}<span class="sha">${esc4(c.short)}</span>${pills}${c.parents.length > 1 ? "<i>merge:</i> " : ""}${esc4(c.subject)}<span class="who">${esc4(c.author)}, ${ago(c.time)}</span></span></div>`;
    });
    const more = this.more ? `<div class="note"><button type="button" data-more>${this.loading ? "reading\u2026" : "more\u2026"}</button></div>` : "";
    return top + lines.join("") + more;
  }
};

// crates/cli/viewer/cli.ts
var Socket = class {
  ws;
  /** The wait before the next try (doubled to 30 s while the watch is away). */
  wait = 1e3;
  next = 1;
  waiting = /* @__PURE__ */ new Map();
  events = [];
  onOpen = () => {
  };
  onClose = () => {
  };
  /**
   * Join the watch: first asked over HTTP whether it is there, so a tab
   * left open after its watch ended never opens WebSockets to a dead port
   * (Firefox opens one WebSocket at a time to an address, and delays
   * those after failures: such a tab would hold back every other's).
   */
  async connect() {
    if (document.hidden) return void addEventListener("visibilitychange", () => void this.connect(), { once: true });
    if (!await this.there()) return this.again();
    this.open();
  }
  /** Whether the watch answers HTTP here. */
  async there() {
    try {
      return (await fetch(new URL("ping", location.href), { cache: "no-store" })).ok;
    } catch {
      return false;
    }
  }
  again() {
    setTimeout(() => void this.connect(), this.wait);
    this.wait = Math.min(this.wait * 2, 3e4);
  }
  open() {
    const url = new URL("ws", location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    this.ws = new WebSocket(url);
    this.ws.onopen = () => (this.wait = 1e3, this.onOpen());
    this.ws.onmessage = (m) => {
      const r = JSON.parse(m.data);
      if (r.event) return this.events.forEach((f) => f(r));
      this.waiting.get(r.id)?.(r);
      this.waiting.delete(r.id);
    };
    this.ws.onclose = () => {
      for (const f of this.waiting.values()) f({ ok: false, error: "closed" });
      this.waiting.clear();
      this.onClose();
      this.again();
    };
  }
  request(op) {
    const id = this.next++;
    return new Promise((ok) => {
      this.waiting.set(id, ok);
      if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ id, ...op }));
      else this.waiting.delete(id), ok({ ok: false, error: "not connected" });
    });
  }
  onEvent(cb) {
    this.events.push(cb);
  }
};
var Pages = class {
  sock;
  /** A change on its way to the screen: ms from its save to `settled`, and when that came. */
  change = null;
  /** Every page's text, for finding (asked again after each build). */
  texts = null;
  constructor(sock2) {
    this.sock = sock2;
    sock2.onEvent((e) => {
      if (e.event !== "settled") return;
      this.texts = null;
      const since = e.since_ms;
      this.change = typeof since === "number" ? { since, at: performance.now() } : null;
    });
  }
  async request(op) {
    const o = op;
    if (o.op === "png") {
      const p = await strips.page(o.page ?? 0, panel.pixelsPerPoint()).catch(() => null);
      if (!p) return { ok: false, json: { error: "no such page" } };
      this.painted();
      return { ok: true, canvas: p.el, size: [p.w, p.h], links: p.links };
    }
    return this.sock.request(op);
  }
  /**
   * The first page drawn after a change: on the screen at the next frame;
   * the ⚡ chip and the watch's status line say how long from the save.
   */
  painted() {
    const c = this.change;
    if (!c) return;
    this.change = null;
    requestAnimationFrame(() => {
      const ms = c.since + performance.now() - c.at;
      panel.painted(ms);
      void this.sock.request({ op: "painted", ms });
    });
  }
  /** Where `q` is in the build's pages (their text as the watch reads it). */
  async find(q) {
    this.texts ??= this.sock.request({ op: "text" }).then(
      (r) => r.ok ? (r.json?.pages ?? []).map((lines) => lines.map(composeLine)) : []
    );
    return search(await this.texts, q);
  }
  onEvent(cb) {
    this.sock.onEvent(cb);
  }
};
var stripSource = {
  async frame(k, scale) {
    const r = await fetch(new URL(`frame/${k}/${Math.round(scale * 1e3)}`, location.href), { cache: "no-store" });
    return r.ok ? await r.json() : null;
  },
  async strips(keys) {
    const r = await fetch(new URL(`strips/${keys.join(",")}`, location.href), { cache: "no-store" });
    return r.ok ? r.arrayBuffer() : null;
  }
};
var chip;
if (location.hash === "#debug") {
  chip = document.createElement("div");
  chip.style.cssText = "position:fixed;right:8px;bottom:32px;z-index:16;font:11px/1.3 ui-monospace,monospace;background:rgba(20,24,32,.82);color:#e8eef8;padding:4px 7px;border-radius:5px;pointer-events:none";
  chip.textContent = "strips";
  document.body.append(chip);
}
var strips = new Strips(stripSource, chip ?? null);
var sock = new Socket();
var pages = new Pages(sock);
var source = (file, line, col = 1) => void sock.request({ op: "source", file, line, col });
var panel = new Panel(
  {
    onPage: () => {
    },
    onNeed: (k) => void session.fetch(k),
    // (the build's PDF, as the watch wrote it)
    onPdf: () => {
      const a = document.createElement("a");
      a.href = new URL("doc.pdf", location.href).href;
      a.download = "";
      a.click();
    },
    onMain: () => {
    },
    onReload: () => void session.resync(),
    onClean: () => void session.resync(),
    // (the watch builds again from the files on disk, as `r` in its terminal)
    onRebuild: () => void sock.request({ op: "rebuild" }),
    onCompare: () => openPick(),
    onLog: () => session.log(),
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
    onContents: () => void session.outline(),
    onFind: (q) => pages.find(q)
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
      }
    }
  },
  {
    host: "watch",
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
      steps: ["Connecting to phitex watch", "Preparing the format", "Typesetting"],
      realPdf: ""
    }
  }
);
var pane = document.createElement("div");
pane.style.cssText = "position:fixed;inset:0";
document.body.append(pane);
panel.dock(pane);
panel.shown(true);
var host = { loadProject: async () => ({}), onOpen() {
}, onChanges() {
} };
var session = new PreviewSession(host, pages, panel, {
  checkEveryMs: 0,
  remote: {
    source: (file, start, end) => void sock.request({ op: "source", file, start, end }),
    select: (file, start, end) => void sock.request({ op: "select", file, start, end })
  }
});
var diffState = {};
var diffWait;
var counted;
sock.onEvent((e) => {
  if (e.event !== "diff") return;
  diffState = e.diff ?? {};
  const n = diffState.changes?.length;
  if (n === void 0) return;
  if (diffWait) diffWait(diffState), diffWait = void 0;
  else counted?.(n);
});
var ask = async (op) => {
  const r = await sock.request(op);
  if (!r.ok) throw new Error(r.error ?? "the watch refused");
};
var save = (name) => {
  const a = document.createElement("a");
  a.href = new URL(name, location.href).href;
  a.download = name;
  a.click();
};
var words = (src) => src.replace(/\\[a-zA-Z@]+\*?/g, " ").replace(/[{}$\\%&~^_#\[\]]/g, " ").split(/\s+/).filter(Boolean).slice(0, 5).join(" ");
var lastAt = { k: -1, page: 0 };
var gotoChange = async (k) => {
  const c = (diffState.changes ?? [])[k - 1];
  if (!c) return;
  const q = words(c.new_text || "") || words(c.old_text || "");
  if (!q) return;
  const hits = await pages.find(q);
  if (!hits.length) return void panel.msg(`change ${k}: not found on the pages`);
  const from2 = k > lastAt.k ? lastAt.page : 0;
  const at = Math.max(0, hits.findIndex((h) => h.k >= from2));
  lastAt = { k, page: hits[at].k };
  panel.reveal(hits.slice(at, at + 1));
};
var runner = {
  async start(v, say, count) {
    counted = count;
    say("Diffing\u2026");
    const got = new Promise((ok) => diffWait = ok);
    await ask({ op: "diff_start", rev: v.from, ...v.to ? { to: v.to } : {} });
    say("Typesetting the diff\u2026");
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
  stop: () => void ask({ op: "diff_stop" })
};
var controls = new DiffControls(panel, runner);
addEventListener("keydown", (e) => controls.key(e));
document.head.append(Object.assign(document.createElement("style"), {
  textContent: COMMITS_CSS + `
#phx-pick, #phx-bar, #phx-con { --bg: #ffffff; --fg: #1b222c; --fg2: #5d6778; --line: #e3e6ec; --hover: #eef1f6; --danger: #c23a2f; --warning: #a3620a; --accent: #1d3e72;
  font: 13px/1.4 "Noto Sans", system-ui, sans-serif; }
@media (prefers-color-scheme: dark) {
  #phx-pick, #phx-bar, #phx-con { --bg: #1b212b; --fg: #e7e9ee; --fg2: #98a1b0; --line: #262d38; --hover: #222935; --danger: #f0786d; --warning: #e5a54b; --accent: #8fb3ec; color-scheme: dark; }
}
#phx-pick { position: fixed; z-index: 20; top: 44px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; overflow: auto;
  background: var(--bg); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 6px 20px rgb(0 0 0 / 25%); }
#phx-pick header { display: flex; align-items: baseline; gap: 10px; padding: 10px 12px; border-bottom: 1px solid var(--line); position: sticky; top: 0; background: var(--bg); }
#phx-pick header small { color: var(--fg2); font-size: 12px; }
#phx-pick header button { margin-left: auto; border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; }
#phx-pick header button:hover { background: var(--hover); }`
}));
var pick;
var graph;
var closePick = () => {
  if (pick) pick.hidden = true;
};
function openPick() {
  if (pick && !pick.hidden) return closePick();
  if (!pick) {
    pick = document.createElement("div");
    pick.id = "phx-pick";
    pick.innerHTML = `<header><b>Compare with an earlier version</b><small>Click: from \xB7 Shift+click: to (else the files as they are) \xB7 Double-click: a commit's own changes</small><button type="button">Close</button></header>`;
    pick.querySelector("button").onclick = closePick;
    document.body.append(pick);
    graph = new Commits(pick, {
      load: async (skip, limit) => {
        const r = await sock.request({ op: "diff_log", skip, limit });
        return r.ok ? r.json?.commits ?? [] : r.error ?? "no git repository here";
      },
      pick: (from2, to) => {
        closePick();
        const short = (id) => id.slice(0, 7);
        void controls.start({ from: from2, to }, short(from2), to ? `to ${short(to)}` : "working tree");
      }
    });
    void graph.load();
  }
  pick.hidden = false;
}
addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pick && !pick.hidden) closePick(), e.stopImmediatePropagation();
}, { capture: true });
var started = false;
sock.onOpen = () => {
  panel.msg("connected to phitex watch");
  panel.connection(true);
  if (started) return void session.resync();
  started = true;
  void session.start();
};
var unreached = async () => {
  if (started) return;
  if (!await sock.there()) return panel.error("phitex watch isn't answering here: is it still running? (trying again)");
  if (sock.ws?.readyState === WebSocket.CONNECTING)
    return panel.error(
      "phitex watch answers, but the browser hasn't opened the WebSocket to it yet. Firefox opens one at a time to this machine and waits longer after failures: another tab connecting to 127.0.0.1 (an old phitex viewer?) holds it back. Closing such tabs lets it through."
    );
  panel.error("phitex watch answers, but the browser refused the WebSocket to it (trying again): a browser add-on blocking connections to 127.0.0.1? Try a private window, or allow 127.0.0.1 in the add-on.");
};
sock.onClose = () => {
  if (!started) return void unreached();
  panel.msg("not connected to phitex watch (retrying)", true);
  panel.connection(false, "phitex watch stopped or the connection was lost; reconnecting");
  status = { kind: "status", text: "not connected to phitex watch" };
  busy = "";
  showBar();
};
setTimeout(() => void unreached(), 3e3);
void sock.connect();
document.head.append(Object.assign(document.createElement("style"), {
  textContent: `
#phx-bar { position: fixed; left: 0; right: 0; bottom: 0; height: 24px; z-index: 15; display: flex; align-items: center; gap: 10px;
  padding: 0 10px; font-size: 12px; font-variant-numeric: tabular-nums; background: var(--bg); color: var(--fg2); border-top: 1px solid var(--line);
  cursor: pointer; user-select: none; white-space: nowrap; overflow: hidden; }
#phx-bar:hover { color: var(--fg); }
#phx-bar .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg2); flex: none; }
#phx-bar .dot.ok { background: #2e9d63; } #phx-bar .dot.bad { background: var(--danger); }
#phx-bar .dot.busy { background: #f58f2d; animation: phx-pulse .8s ease-in-out infinite alternate; }
@keyframes phx-pulse { from { opacity: .35; } }
@media (prefers-reduced-motion: reduce) { #phx-bar .dot.busy { animation: none; } }
#phx-bar .text { overflow: hidden; text-overflow: ellipsis; flex: 1; }
#phx-bar .new { flex: none; color: var(--danger); font-weight: 600; }
#phx-bar .new.warn { color: var(--warning); }
#phx-bar .hint { flex: none; color: var(--fg2); }
#phx-con { position: fixed; left: 0; right: 0; bottom: 24px; height: 38vh; z-index: 15; display: flex; flex-direction: column;
  background: var(--bg); color: var(--fg); border-top: 1px solid var(--line); box-shadow: 0 -4px 16px rgb(0 0 0 / 12%); }
#phx-con[hidden] { display: none; }
#phx-con header { display: flex; align-items: baseline; gap: 10px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
#phx-con header b { font-weight: 600; }
#phx-con header small { color: var(--fg2); }
#phx-con header button { border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; }
#phx-con header button:hover { background: var(--hover); }
#phx-con header button:first-of-type { margin-left: auto; }
#phx-con .log { flex: 1; overflow: auto; margin: 0; padding: 8px 10px; font: 12px/1.5 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre-wrap; word-break: break-word; }
#phx-con .error { color: var(--danger); } #phx-con .warning { color: var(--warning); } #phx-con .note, #phx-con .dim { color: var(--fg2); }
#phx-con .ok { color: var(--fg); } #phx-con .busy { color: var(--fg2); }
}`
}));
pane.style.bottom = "24px";
var bar = document.createElement("div");
bar.id = "phx-bar";
bar.title = "phitex watch's status: click (or `) for everything it printed";
bar.innerHTML = `<span class="dot"></span><span class="text">phitex watch</span><span class="new" hidden></span><span class="hint">Terminal (\`)</span>`;
var con = document.createElement("div");
con.id = "phx-con";
con.hidden = true;
con.innerHTML = `<header><b>Terminal</b><small>what phitex watch printed</small><button type="button" data-a="rebuild" title="Build again (r in the terminal)">Rebuild</button><button type="button" data-a="clear">Clear</button><button type="button" data-a="close">Close</button></header><pre class="log"></pre>`;
document.body.append(con, bar);
var log = con.querySelector(".log");
var dot = bar.querySelector(".dot");
var barText = bar.querySelector(".text");
var badge = bar.querySelector(".new");
var status = { kind: "status", text: "" };
var busy = "";
var unseen = { errors: 0, warnings: 0 };
var showBar = () => {
  dot.className = "dot" + (busy ? " busy" : status.ok === true ? " ok" : status.ok === false ? " bad" : "");
  barText.textContent = busy ? `${busy}\u2026` : status.text || "phitex watch";
  const n = unseen.errors + unseen.warnings;
  badge.hidden = n === 0 || !con.hidden;
  badge.className = "new" + (unseen.errors ? "" : " warn");
  badge.textContent = unseen.errors ? `${unseen.errors} error${unseen.errors > 1 ? "s" : ""}` : `${n} warning${n > 1 ? "s" : ""}`;
};
var look = (s) => {
  const t = s.text ?? "";
  if (s.kind !== "line") return s.kind;
  if (/^(error|✗|failed:)/.test(t)) return "error";
  if (/^warning/.test(t)) return "warning";
  if (/^(✓|ok:|finished:)/.test(t)) return "ok";
  if (/^(phitex:|viewer:|\s)/.test(t)) return "dim";
  return "";
};
var said = (s, live) => {
  if (s.kind === "status") return void (status = s, showBar());
  if (s.kind === "busy") {
    busy = s.text ?? "";
    const parts = busy.split(" \xB7 ");
    if (parts.length >= 3) {
      const phase = parts[1];
      panel.loadStep(phase === "format" ? 1 : 2, `${phase} \xB7 ${parts.slice(2).join(" \xB7 ")}`);
    }
    return void showBar();
  }
  if (s.kind === "event") return;
  const at = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  const line = document.createElement("div");
  line.className = look(s);
  line.textContent = s.kind === "note" ? `phitex: ${s.text}` : s.kind === "error" || s.kind === "warning" ? `${s.kind}: ${s.text}` : s.text ?? "";
  log.append(line);
  while (log.childElementCount > 2e3) log.firstElementChild.remove();
  if (at) log.scrollTop = log.scrollHeight;
  if (live && con.hidden) {
    if (line.className === "error") unseen.errors++;
    else if (line.className === "warning") unseen.warnings++;
    showBar();
  }
};
var toggle = (open = con.hidden) => {
  con.hidden = !open;
  if (open) unseen = { errors: 0, warnings: 0 }, log.scrollTop = log.scrollHeight;
  showBar();
};
bar.onclick = () => toggle();
con.querySelector("header").onclick = (e) => {
  const a = e.target.dataset?.a;
  if (a === "close") toggle(false);
  if (a === "clear") log.replaceChildren();
  if (a === "rebuild") void sock.request({ op: "rebuild" });
};
addEventListener("keydown", (e) => {
  const t = e.target;
  if (t?.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t?.tagName ?? "")) return;
  if (e.key === "`" && !e.ctrlKey && !e.metaKey && !e.altKey) toggle(), e.preventDefault();
  else if (e.key === "Escape" && !con.hidden) toggle(false);
});
sock.onEvent((e) => {
  if (e.event === "console") said(e.item, true);
});
var backlog = async () => {
  const r = await sock.request({ op: "console" });
  if (!r.ok) return;
  log.replaceChildren();
  for (const s of r.json?.items ?? []) said(s, false);
  if (r.json?.status) said(r.json.status, false);
};
var opened = sock.onOpen;
sock.onOpen = () => (opened(), void backlog());
showBar();
