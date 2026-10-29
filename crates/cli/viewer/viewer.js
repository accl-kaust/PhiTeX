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
          this.measure();
          for (const k of this.visible) if (this.slots[k]) this.snap(this.slots[k]);
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
    const marks = [...s.el.querySelectorAll(".mark")];
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
  snap(s) {
    const wrap = s.img?.canvas;
    const c = wrap?.querySelector("canvas");
    if (!wrap || !c || !c.width || !c.height) return;
    const dpr = window.devicePixelRatio || 1;
    wrap.style.width = `${c.width / dpr}px`;
    wrap.style.height = `${c.height / dpr}px`;
    const r = s.el.getBoundingClientRect();
    const fx = r.left * dpr - Math.round(r.left * dpr);
    const fy = r.top * dpr - Math.round(r.top * dpr);
    wrap.style.transform = fx || fy ? `translate(${-fx / dpr}px, ${-fy / dpr}px)` : "";
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
  /** The page most on screen, measured now (the observer only reports crossings of its thresholds, in a root grown by its margin). */
  measure() {
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
var TOKENS = `
  --paper: #f1f1ef; --paper-2: #e6e6e3; --sheet: #ffffff;
  --ink: #141414; --ink-2: #4a4a48; --ink-3: #8a8a86;
  --rule: #c9c9c5; --rule-2: #dededa;
  --mark: #d71920; --caution: #8a5a00;
  --sans: "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  --serif: "Latin Modern Roman", "CMU Serif", "Computer Modern", Georgia, serif;
`;
var TOKENS_DARK = `
  --paper: #121212; --paper-2: #1c1c1c; --sheet: #ffffff;
  --ink: #e8e8e4; --ink-2: #a9a9a4; --ink-3: #6e6e6a;
  --rule: #3a3a38; --rule-2: #2a2a28;
  --mark: #ff4a3d; --caution: #d9a441;
`;
var CROP = `
.slot::after { content: ""; position: absolute; inset: -14px; pointer-events: none;
  --c: var(--ink-3, #8a8a86);
  background:
    linear-gradient(var(--c), var(--c)) left 14px top 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) left 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px top 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) right 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) left 14px bottom 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) left 0 bottom 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px bottom 0 / 1px 10px,
    linear-gradient(var(--c), var(--c)) right 0 bottom 14px / 10px 1px;
  background-repeat: no-repeat; }
`;
var PAGE_CSS = CROP;
var PRINT_CSS = `
.win { ${TOKENS} }
@media (prefers-color-scheme: dark) { .win:not(.light) { ${TOKENS_DARK} color-scheme: dark; } }
.win.dark { ${TOKENS_DARK} color-scheme: dark; }
.pkgs .icon { color: var(--ink-2); }

.win, .win.docked { background: var(--paper); color: var(--ink); border: 1px solid var(--rule); border-radius: 0; box-shadow: none;
  font: 12px/1.45 var(--sans); font-variant-numeric: tabular-nums; }
.win.docked { border: 0; }
.win *:focus-visible { outline: 1px solid var(--mark); outline-offset: 1px; }

/* header: one thin bar, ink on paper, a hairline under it */
header, .win.docked header { height: 30px; padding: 0 6px 0 10px; gap: 6px; background: var(--paper); color: var(--ink);
  border-bottom: 1px solid var(--rule); }
.title { font: 600 11px var(--sans); letter-spacing: .08em; text-transform: uppercase; margin: 0 8px 0 0; }
.badge, .win.docked .badge { background: none; color: var(--ink-3); border-radius: 0; padding: 0; font: 400 11px var(--sans); font-variant: small-caps; letter-spacing: .04em; }
header > .icon:first-child { display: none; }
.chip, .win.docked .chip { background: none; border-radius: 0; padding: 0 6px; height: 30px; gap: 6px; font: 11px var(--mono); color: var(--ink-2);
  border-left: 1px solid var(--rule-2); border-right: 1px solid var(--rule-2); }
.dot { width: 6px; height: 6px; border-radius: 0; background: var(--ink-3); }
.chip.ok .dot { background: var(--ink); } .chip.warning .dot { background: var(--caution); } .chip.error .dot { background: var(--mark); }
.chip.busy .dot { animation: phx-blink .8s steps(2) infinite; }
@keyframes phx-blink { 50% { opacity: 0; } }
button.ib { border-radius: 0; width: 26px; height: 26px; color: var(--ink-2); }
header button.ib:hover, .bar button.ib:hover, .win.docked header button.ib:hover { background: var(--paper-2); color: var(--ink); }
button.ib[aria-pressed=true], .win.docked header button.ib[aria-pressed=true] { background: none; color: var(--mark); }
button.ib.txt { width: auto; padding: 0 8px; font: 10px var(--sans); letter-spacing: .1em; text-transform: uppercase; }
.win.no-footer > footer, .win.docked.own-header.no-footer > footer { display: none; }
.find { display: inline-flex; align-items: center; gap: 6px; margin-right: 8px; }
.find[hidden] { display: none; }
.find input { width: 180px; height: 20px; padding: 0 6px; font: 11px var(--mono); color: var(--ink); background: var(--sheet); border: 1px solid var(--ink);
  border-radius: 0; outline: none; }
.find input::placeholder { color: var(--ink-3); font-family: var(--sans); letter-spacing: .04em; }
#findn { font: 11px var(--mono); color: var(--ink-2); min-width: 52px; }
#findn.none { color: var(--mark); }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); outline: 0; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); outline: 1px solid #141414; }
.speedchip:empty { display: none !important; }
.pno { font: 11px var(--mono); color: var(--ink-2); min-width: 48px; }
.bar { background: var(--paper); color: var(--ink-2); border-bottom: 1px solid var(--rule); padding: 4px 10px; }
.bar label { font: 11px var(--sans); font-variant: small-caps; letter-spacing: .04em; }
select { font: 11px var(--mono); color: var(--ink); background: var(--paper); border: 1px solid var(--rule); border-radius: 0; padding: 1px 4px; }
button.btn { color: var(--ink); background: none; border: 1px solid var(--ink); border-radius: 0; padding: 2px 10px 2px 6px; font: 600 11px var(--sans);
  letter-spacing: .04em; text-transform: uppercase; }
button.btn:hover { background: var(--ink); color: var(--paper); }

/* the problems' line and list: rules, not cards */
.sum, .win.docked .sum, .win.docked.light .sum { background: var(--paper); color: var(--ink-2); border-bottom: 1px solid var(--rule); height: 24px; font: 11px var(--sans); }
.sum:hover, .win.docked .sum:hover, .win.docked.light .sum:hover { background: var(--paper-2); }
.sum .first, .win.docked .sum .first, .win.docked.light .sum .first { color: var(--ink); }
.sum .n.error, .win.docked .sum .n.error, .win.docked.light .sum .n.error { color: var(--mark); }
.sum .n.warning, .win.docked .sum .n.warning, .win.docked.light .sum .n.warning { color: var(--caution); }
.sum .n.info, .win.docked .sum .n.info, .win.docked.light .sum .n.info { color: var(--ink-2); }
.diags { background: var(--paper); border-bottom: 1px solid var(--ink); box-shadow: none; }
.diag { border-top: 1px solid var(--rule-2); font: 12px var(--sans); }
.diag[data-line]:hover { background: var(--paper-2); }
.diag.error .icon { color: var(--mark); } .diag.warning .icon { color: var(--caution); } .diag.info .icon { color: var(--ink-3); }
.diag .where { font: 11px var(--mono); color: var(--ink-3); }
.diag .detail, .folded .detail { font: 11px/1.4 var(--mono); background: none; border-left: 1px solid var(--rule); border-radius: 0; padding: 2px 8px; }

/* the stage: pages on paper, crop marks for a frame, no shadows */
.stage, .win.docked .stage { background: var(--paper-2); padding: 24px; }
.slot, .stage .slot, .win.docked .slot, .win.docked .stage .slot { margin: 28px auto; background: var(--sheet); box-shadow: none; border-radius: 0; }
.win.docked.pdf-dark .slot { box-shadow: none; }
.stage.stale .slot { opacity: .5; filter: grayscale(1); }
.banner, .approx { background: var(--paper); color: var(--caution); border-bottom: 1px solid var(--rule); font: 11px var(--sans); }
.slot .mark { border-radius: 0; background: rgb(255 214 0 / 45%); box-shadow: none; outline: 1px solid rgb(180 140 0 / 70%); animation: none; }

/* the loading card: a blank proof with crop marks, its steps in small caps, a rule filling */
.empty, .win.docked .empty, .win.docked.light .empty { color: var(--ink-2); padding: 48px 12px; font: 12px var(--sans); }
.load, .win.docked .load, .win.docked.light .load, .win:not(.docked) .load { max-width: 340px; color: var(--ink-2); position: relative; padding: 22px 24px;
  background: var(--sheet); color: #4a4a48; }
.load::after { content: ""; position: absolute; inset: -14px; pointer-events: none; --c: var(--ink-3);
  background:
    linear-gradient(var(--c), var(--c)) left 14px top 0 / 1px 10px, linear-gradient(var(--c), var(--c)) left 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px top 0 / 1px 10px, linear-gradient(var(--c), var(--c)) right 0 top 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) left 14px bottom 0 / 1px 10px, linear-gradient(var(--c), var(--c)) left 0 bottom 14px / 10px 1px,
    linear-gradient(var(--c), var(--c)) right 14px bottom 0 / 1px 10px, linear-gradient(var(--c), var(--c)) right 0 bottom 14px / 10px 1px;
  background-repeat: no-repeat; }
.load h3 { font: 400 17px/1.3 var(--serif); color: #141414; margin: 0 0 14px; }
.load .steps { gap: 0; margin: 0 0 16px; font: 10px var(--sans); letter-spacing: .1em; text-transform: uppercase; color: #8a8a86; }
.load .steps span { opacity: 1; gap: 0; }
.load .steps span::before { display: none; }
.load .steps span.ok { color: #4a4a48; }
.load .steps span.now { color: #d71920; font-weight: 600; }
.load .steps i { flex: 1; min-width: 12px; margin: 0 8px; height: 1px; background: #c9c9c5; opacity: 1; }
.load .bar, .load .bar.busy { height: 1px; border-radius: 0; background: #dededa; }
.load .bar i { background: #141414; border-radius: 0; transition: width .15s linear; }
.load .bar:not(.busy) i { background-image: none; animation: none; }
.load .bar.busy i { width: 20% !important; animation: phx-carriage 1.4s linear infinite; }
@keyframes phx-carriage { from { transform: translateX(-100%); } to { transform: translateX(500%); } }
.load .alive { font: 11px var(--mono); color: #4a4a48; margin-top: 10px; }
.load .alive::before { display: none; }
.load .count { font: 11px var(--mono); }
.load .names span { border-radius: 0; background: none; border: 1px solid #dededa; font: 10px var(--mono); color: #4a4a48; }
.load .names span.ok { color: #141414; }
.load .names span.ing { animation: none; border-color: #d71920; }
.load .note { font: 11px var(--sans); color: #8a8a86; }
.load .btns .btn, .report .btn, .diags .reportbtn { border-radius: 0; border: 1px solid currentColor; font: 600 11px var(--sans); text-transform: uppercase; letter-spacing: .04em; }

/* overlays: flat sheets with a rule, no blur, no shadow */
.pkgs, .warmup, .diffbar, .diffset, .keyhelp, .about, .report, .win.docked.sheet-open > .bar {
  border-radius: 0 !important; box-shadow: none !important; backdrop-filter: none !important;
  background: var(--paper) !important; color: var(--ink) !important; border: 1px solid var(--ink) !important; font-family: var(--sans); }
.pkgs { border-left: 2px solid var(--mark) !important; }
.pkgs .mini, .pkgs .mini i { border-radius: 0; height: 1px; } .pkgs .mini i { background: var(--ink); }
.warmup .dot, .diffbar .busy::before { border-radius: 0; width: 6px; height: 6px; background: var(--mark); animation: phx-blink .8s steps(2) infinite; }
.warmup.ready { background: var(--paper) !important; color: var(--ink) !important; }
.warmup .slow { color: var(--caution); }
.diffbar button, .diffset button { border-radius: 0; color: var(--ink); }
.diffbar button:hover:not(:disabled), .diffset button:hover { background: var(--paper-2); }
.diffbar button.cur, .diffset button[aria-pressed=true] { background: var(--ink); color: var(--paper); }
.diffbar .sep { background: var(--rule); }
.diffset h4 { color: var(--ink-3); }
.diffset .sample { border-radius: 0; font-family: var(--serif); }
.diffset .pair i { border-radius: 0; }
.keyhelp kbd { font: 11px var(--mono); background: none; border: 1px solid var(--rule); border-radius: 0; }
.about a { color: var(--mark); }
.about .about-foot { color: var(--ink-3); }

/* the time to the screen and who made this: set like a folio, not pills */
.speedchip, .win.docked .speedchip { height: auto; padding: 2px 6px; border-radius: 0; background: var(--paper); color: var(--ink);
  border: 1px solid var(--rule); box-shadow: none; font: 11px var(--mono); }
.speedchip.slow { color: var(--caution); background: var(--paper); }
.speedchip.flash { animation: phx-tick .4s linear; }
@keyframes phx-tick { from { border-color: var(--mark); } }
.win.docked .byline, .win.docked.light .byline { border-radius: 0; background: var(--paper); color: var(--ink-3); backdrop-filter: none;
  font: 10px/20px var(--sans); height: 20px; letter-spacing: .1em; text-transform: uppercase; border: 1px solid var(--rule-2); }
.win.docked .byline:hover { color: var(--ink); background: var(--paper); }

footer { background: var(--paper); color: var(--ink-3); border-top: 1px solid var(--rule); font: 11px var(--mono); height: 22px; }
footer .msg.err { color: var(--mark); }
.details { font: 11px var(--mono); color: var(--ink-2); border-top: 1px solid var(--rule); }
.side { background: var(--paper); color: var(--ink); border-right: 1px solid var(--rule); }
.win.collapsed, .win.collapsed header { border-radius: 0; }
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
.phx-problems { position: fixed; inset: 0; z-index: 10; background: rgb(18 18 18 / 55%); overflow: auto; padding: 48px 16px; }
.phx-card { max-width: 900px; margin: 0 auto; background: #fff; color: #141414; border-radius: 0; border-left: 2px solid #d71920;
  box-shadow: none; font: 13px/1.5 "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif; }
.phx-card header { display: flex; align-items: baseline; gap: 10px; padding: 14px 20px 10px; border-bottom: 1px solid #141414; font-size: 13px; }
.phx-card header b { color: #141414; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; font-size: 11px; }
.phx-close { margin-left: auto; background: none; color: #4a4a48; border: 1px solid #c9c9c5; border-radius: 0; padding: 1px 8px;
  font: 11px "IBM Plex Mono", ui-monospace, monospace; cursor: pointer; }
.phx-close:hover { border-color: #141414; color: #141414; }
.phx-card ol { list-style: none; margin: 0; padding: 0; }
.phx-card li { padding: 14px 20px; border-bottom: 1px solid #dededa; }
.phx-msg { display: flex; flex-wrap: wrap; align-items: baseline; gap: 10px; font-size: 14px; }
.phx-sev { font-size: 10px; letter-spacing: .1em; text-transform: uppercase; font-weight: 600; padding: 0; border-radius: 0; background: none; }
.phx-error .phx-sev { color: #d71920; }
.phx-warning .phx-sev { color: #8a5a00; }
.phx-note .phx-sev { color: #8a8a86; }
.phx-code { color: #8a8a86; font: 11px "IBM Plex Mono", ui-monospace, monospace; }
.phx-where { margin-top: 2px; font: 11px "IBM Plex Mono", ui-monospace, monospace; color: #4a4a48; }
.phx-place { color: #141414; text-decoration: underline; text-decoration-color: #c9c9c5; text-underline-offset: 2px; }
.phx-place:hover { text-decoration-color: #d71920; }
.phx-excerpt, .phx-ctx pre { margin: 10px 0 0; padding: 6px 0 6px 12px; background: none; border-left: 1px solid #c9c9c5; border-radius: 0; overflow-x: auto;
  font: 12px/1.5 "IBM Plex Mono", ui-monospace, monospace; white-space: pre; color: #141414; }
.phx-excerpt mark, .phx-ctx mark { background: none; color: #d71920; text-decoration: underline 1.5px #d71920; text-underline-offset: 3px; }
.phx-ln { color: #8a8a86; }
.phx-notes, .phx-help { margin: 8px 0 0; padding-left: 16px; color: #4a4a48; }
.phx-help li::marker { content: "\u2192 "; color: #d71920; }
.phx-ctx { margin-top: 8px; color: #4a4a48; }
.phx-ctx summary { cursor: pointer; }
.phx-ctx code { color: #141414; }
.phx-inc { margin-top: 6px; color: #8a8a86; font-size: 12px; }
.phx-card footer { padding: 10px 20px; color: #8a8a86; font-size: 11px; }
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
  constructor(root, place2) {
    this.place = place2;
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
.phx-outline { overflow: auto; font: 13px/1.35 system-ui, sans-serif; color: var(--ink, #333); padding: 8px 0 24px; }
.phx-outline ul { list-style: none; margin: 0; padding: 0; }
.phx-outline li.folded > ul { display: none; }
.phx-row { display: flex; align-items: baseline; gap: 6px; padding: 3px 10px 3px calc(10px + var(--d) * 14px); border-radius: 4px; margin: 0 6px; }
.phx-row:hover { background: rgba(127, 127, 127, 0.12); }
.phx-row.here { background: rgba(62, 99, 221, 0.14); }
.phx-row.here .phx-t { color: #3e63dd; font-weight: 600; }
.phx-t { flex: 1; min-width: 0; color: inherit; text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
li.parent > .phx-row > .phx-t { font-weight: 500; }
.phx-p { color: var(--dim, #777); font-size: 11px; font-variant-numeric: tabular-nums; }
.phx-fold, .phx-dot { flex: none; width: 10px; text-align: center; color: var(--dim, #777); cursor: pointer; user-select: none; }
.phx-fold::before { content: "\u25BE"; }
li.folded > .phx-row .phx-fold::before { content: "\u25B8"; }
.phx-dot { cursor: default; }
.phx-none { color: var(--dim, #777); padding: 8px 16px; margin: 0; }
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
  const esc4 = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = pairs.length ? new RegExp(pairs.map(([p]) => `(?<![A-Za-z0-9_])${esc4(p)}(?![A-Za-z0-9_])`).join("|"), "g") : null;
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
  license: "LICENSE.txt",
  realPdf: "Overleaf's PDF is the real one.",
  stopped: "Open \u24D8 diagnostics for the details. Overleaf's PDF is one click away (PDF).",
  notReady: "Use Overleaf's PDF for it",
  openFailed: "Overleaf's PDF is one click away. Reload the tab to try again.",
  reading: "Your project's files stay in this browser: read from Overleaf, typeset here.",
  keptIn: "this browser",
  steps: ["Project", "Packages", "Typesetting"]
};
function safeColor(c, or) {
  return typeof c === "string" && /^#[0-9a-fA-F]{6}$/.test(c) ? c : or;
}
var DEFAULTS = { x: null, y: null, w: 440, h: 620, zoom: "fit", collapsed: false, details: false };
function pageBox(d, cssWidth, dpr = globalThis.devicePixelRatio || 1) {
  const snap = (x) => Math.floor(x * dpr) / dpr;
  return [snap(cssWidth), snap(d.h * cssWidth / d.w)];
}
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
  info: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z"
};
var icon = (name, size = 18) => `<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`;
var stepsHtml = (names, state) => `<div class="steps">${names.map((n, k) => `<span class="${state[k]}">${n}</span>`).join("<i></i>")}</div>`;
var CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
.win {
  --dark: var(--bg-dark-primary, #1b222c); --dark2: var(--bg-dark-secondary, #2f3a4c); --dark3: var(--bg-dark-tertiary, #495365);
  --light: var(--bg-light-primary, #fff); --light2: var(--bg-light-secondary, #f4f5f6); --light3: var(--bg-light-tertiary, #e7e9ee);
  --accent: var(--bg-accent-01, #098842); --accent2: var(--bg-accent-02, #1e6b41);
  --fg: var(--content-primary, #1b222c); --fg2: var(--content-secondary, #495365); --fg-dark: var(--content-primary-dark, #f4f5f6);
  --fg2-dark: var(--content-placeholder-dark, #8d96a5);
  --danger: var(--content-danger, #b83a33); --warn: var(--content-warning, #8f5514); --info: var(--content-info, #366cbf);
  --ok-dark: var(--content-positive-dark, #53b57f); --warn-dark: var(--content-warning-dark, #de8014); --danger-dark: var(--content-danger-dark, #e36d66);
  --divider: var(--border-divider, #e7e9ee); --r: var(--border-radius-base, 4px); --r2: var(--border-radius-medium, 8px);
  position: fixed; z-index: 2147483000; display: flex; flex-direction: column;
  min-width: 300px; min-height: 180px; max-width: calc(100vw - 16px); max-height: calc(100vh - 16px);
  resize: both; overflow: hidden;
  background: var(--light); color: var(--fg); border: 1px solid var(--dark3); border-radius: var(--r2);
  box-shadow: 0 8px 28px rgba(27,34,44,.35);
  font: 13px/1.4 "Noto Sans", system-ui, sans-serif;
}
.icon { display: block; flex: none; }
header { display: flex; align-items: center; gap: 2px; height: 40px; padding: 0 6px 0 10px; background: var(--dark); color: var(--fg-dark);
  cursor: grab; user-select: none; flex: none; }
header:active { cursor: grabbing; }
.title { font-weight: 600; font-size: 14px; margin: 0 6px 0 4px; white-space: nowrap; }
.badge { font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 9999px; background: var(--dark2); color: var(--fg2-dark); white-space: nowrap; }
.chip { display: inline-flex; align-items: center; gap: 5px; font-size: 12px; padding: 2px 8px; border-radius: 9999px; background: var(--dark2);
  white-space: nowrap; cursor: pointer; margin-right: 4px; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--fg2-dark); }
.chip.ok .dot { background: var(--ok-dark); } .chip.warning .dot { background: var(--warn-dark); } .chip.error .dot { background: var(--danger-dark); }
.chip.busy .dot { animation: pulse 1s infinite; } @keyframes pulse { 50% { opacity: .3; } }
.grow { flex: 1; }
.group { display: inline-flex; align-items: center; }
button.ib { all: unset; display: inline-grid; place-items: center; width: 28px; height: 28px; border-radius: var(--r); color: inherit; cursor: pointer; }
header button.ib:hover { background: var(--dark2); } .bar button.ib:hover { background: var(--light3); }
button.ib:focus-visible { outline: 2px solid var(--info); }
button.ib[aria-pressed=true] { background: var(--dark3); }
button.ib:disabled { opacity: .35; cursor: default; }
.pno { font-size: 12px; min-width: 44px; text-align: center; font-variant-numeric: tabular-nums; }
.bar { display: flex; align-items: center; gap: 8px; padding: 6px 10px; background: var(--light2); border-bottom: 1px solid var(--divider);
  color: var(--fg2); flex: none; flex-wrap: wrap; }
.bar label { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; }
select { font: inherit; font-size: 12px; color: var(--fg); background: var(--light); border: 1px solid var(--border-primary, #677283);
  border-radius: var(--r); padding: 2px 4px; max-width: 140px; }
button.btn { font: inherit; font-size: 12px; font-weight: 600; color: #fff; background: var(--accent); border: 0; border-radius: 9999px;
  padding: 4px 12px 4px 8px; cursor: pointer; display: inline-flex; align-items: center; gap: 4px; }
button.btn:hover { background: var(--accent2); }

.sum { display: flex; align-items: center; gap: 8px; height: 28px; padding: 0 10px; font-size: 12px; color: var(--fg2); flex: none;
  border-bottom: 1px solid var(--divider); cursor: pointer; white-space: nowrap; overflow: hidden; user-select: none; }
.sum:hover { background: var(--light2); }
.sum .n { display: inline-flex; align-items: center; gap: 2px; font-weight: 600; }
.sum .n.error { color: var(--danger); } .sum .n.warning { color: var(--warn); } .sum .n.info { color: var(--info); }
.sum .first { overflow: hidden; text-overflow: ellipsis; color: var(--fg); }
.sum .caret { margin-left: auto; transition: transform .15s; }
.win.diags-open .sum .caret { transform: rotate(90deg); }
.body { position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column; }
/* (the contents beside the pages: PanelOptions.outline) */
.win.has-side .body { flex-direction: row; }
.side { display: none; flex: none; width: 240px; overflow: auto; background: var(--light); border-right: 1px solid var(--light3); color: var(--fg); }
.win.side-on .side { display: block; }
.keyhelp { position: absolute; z-index: 6; top: 12px; right: 12px; padding: 10px 14px; border-radius: 10px; background: rgb(27 34 44 / 96%); color: #fff; font-size: 12px; box-shadow: 0 8px 24px rgba(0,0,0,.4); }
.keyhelp td { padding: 1px 8px 1px 0; vertical-align: top; }
.keyhelp kbd { font: 11px ui-monospace, monospace; background: rgb(255 255 255 / 12%); border-radius: 4px; padding: 1px 5px; }
/* (a problem told whole, in the drawer: problems.ts's card, as a row) */
.diags .phx-inline { margin: 0; padding: 0; border-radius: 0; border-top: 0; box-shadow: none; max-width: none; list-style: none; }
.diags .phx-inline > li { border-bottom: 1px solid #2c2e33; }
.diags { position: relative; flex: none; max-height: 40%; overflow: auto; background: var(--light);
  border-bottom: 1px solid var(--divider); display: none; }
/* (a build with no pages: the list may take the room the pages would) */
.win.nopages.diags-open .diags { max-height: none; flex: 1 1 auto; }
.win.diags-open .diags { display: block; }
.diag { display: flex; gap: 6px; align-items: flex-start; padding: 5px 10px; font-size: 12px; border-top: 1px solid var(--divider); }
.diag:first-child { border-top: 0; }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--light2); }
.diag .icon { margin-top: 1px; }
.folded { margin-top: 6px; text-align: left; font-size: 11px; }
.folded summary { cursor: pointer; color: var(--fg2); }
.diag .detail, .folded .detail { margin: 4px 0 0; padding: 4px 6px; font: 11px/1.35 "DM Mono", monospace; white-space: pre-wrap; overflow-wrap: anywhere; background: var(--divider); border-radius: 4px; max-height: 12em; overflow: auto; }
.diag.error .icon { color: var(--danger); } .diag.warning .icon { color: var(--warn); } .diag.info .icon { color: var(--info); }
.diag .where { color: var(--fg2); white-space: nowrap; margin-left: auto; padding-left: 8px; font-family: "DM Mono", monospace; font-size: 11px; }
.stage { position: relative; flex: 1; overflow: auto; background: var(--light3); padding: 12px; text-align: center; min-height: 0; }
.stage .slot { background: #fff; box-shadow: 0 1px 3px rgba(27,34,44,.25); border-radius: 2px; }
.stage img { max-width: 100%; }
/* a page: MuPDF's canvas, and its text over it, transparent (selected, copied, found) */
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: rgb(0 100 255 / 25%); }
.stage.stale .slot canvas { opacity: .45; filter: grayscale(1); }
.banner { position: sticky; top: -12px; z-index: 1; display: none; margin: -12px -12px 10px; padding: 6px 10px; text-align: left; font-size: 12px;
  background: var(--bg-warning-03, #fcf1e3); color: var(--warn); border-bottom: 1px solid var(--divider); }
.stage.stale .banner { display: block; }
/* (the engine's connection lost: the last pages kept, dimmed, still, with why) */
.gone { position: sticky; top: -12px; z-index: 2; display: none; margin: -12px -12px 10px; padding: 8px 12px; text-align: left; font-size: 12px; font-weight: 600;
  background: var(--bg-danger-03, #fbeceb); color: var(--danger); border-bottom: 1px solid var(--divider); }
.stage.offline .gone { display: flex; align-items: baseline; gap: 8px; }
.gone .x { margin-left: auto; flex: none; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; font-size: 14px; line-height: 1; padding: 0 2px; }
.stage.offline .slot { opacity: .4; pointer-events: none; }
/* (dismissed: the pages back to normal, the state still shown, small) */
.offpill { display: none; position: absolute; top: 8px; right: 14px; z-index: 3; padding: 2px 8px; border-radius: 9999px; font-size: 11px; font-weight: 600;
  background: var(--light); color: var(--danger); border: 1px solid var(--danger); pointer-events: none; }
.win.offline-quiet .offpill { display: block; }
.approx { position: sticky; top: -12px; z-index: 1; display: none; margin: -12px -12px 10px; padding: 6px 10px; text-align: left; font-size: 12px; background: #fff4d6; color: #5c4400; border-bottom: 1px solid #e8c766; }
.win.docked .approx { margin: 0; top: 0; }
.stage.fit { overflow-x: hidden; }
.empty { color: var(--fg2); padding: 40px 12px; font-size: 13px; }
footer { display: flex; align-items: center; gap: 8px; height: 26px; padding: 0 10px; background: var(--light2); border-top: 1px solid var(--divider);
  color: var(--fg2); font-size: 11px; font-variant-numeric: tabular-nums; flex: none; white-space: nowrap; overflow: hidden; }
footer .lat { cursor: pointer; overflow: hidden; text-overflow: ellipsis; }
footer .msg { overflow: hidden; text-overflow: ellipsis; }
footer .msg.err { color: var(--danger); }
.details { display: none; flex-basis: 100%; padding: 6px 0 0; font-size: 11px; color: var(--fg2); border-top: 1px solid var(--divider);
  white-space: pre-wrap; text-align: left; font-family: "DM Mono", monospace; }
.win.details-open .details { display: block; }
/* docked: the preview fills the host's pane, in place of its PDF viewer */
.win.docked { position: relative; left: auto !important; top: auto !important; width: 100% !important; height: 100% !important;
  max-width: none; max-height: none; min-width: 0; min-height: 0; resize: none; border: 0; border-radius: 0; box-shadow: none; }
.win.docked header { height: 34px; background: var(--light2); color: var(--fg); border-bottom: 1px solid var(--divider); cursor: default; }
.win.docked header button.ib:hover { background: var(--light3); } .win.docked header button.ib[aria-pressed=true] { background: var(--light3); }
.win.docked .chip { background: var(--light3); }
.win.docked .badge { background: var(--light3); color: var(--fg2); }
.win.docked .hide-docked { display: none; }
.win:not(.docked) .show-docked { display: none; }
.win.docked { background: var(--pdf-bg, var(--pane, #2f3a4c)); border: 0; }
/* Overleaf's "dark mode PDF preview" (.pdf-dark-mode on the pane): its own filter, no shadow */
.win.docked.pdf-dark .stage .slot canvas { filter: invert(95%) hue-rotate(180deg) brightness(90%) contrast(90%); box-shadow: none; }
.win.docked.light .sum { background: var(--light2); color: var(--fg2); border-bottom-color: var(--divider); }
.win.docked.light .sum:hover { background: var(--light3); } .win.docked.light .sum .first { color: var(--fg); }
.win.docked.light .sum .n.error { color: var(--danger); } .win.docked.light .sum .n.warning { color: var(--warn); } .win.docked.light .sum .n.info { color: var(--info); }
.win.docked.light .empty { color: var(--fg2); }
.win.docked > header, .win.docked > footer { display: none; }
/* (a host with no toolbar of its own: the panel's header and footer kept, PanelOptions.header) */
.win.docked.own-header > header, .win.docked.own-header > footer { display: flex; }
.win.docked > .bar { display: none; }
.win.docked.sheet-open > .bar { display: flex; position: absolute; z-index: 4; top: 6px; right: 8px; width: min(340px, calc(100% - 16px));
  flex-direction: column; align-items: stretch; gap: 8px; padding: 12px; border-radius: var(--r2); border: 1px solid var(--divider);
  background: var(--light); box-shadow: 0 8px 24px rgba(27,34,44,.3); animation: sheet .14s ease-out; }
.win.docked.sheet-open > .bar label { justify-content: space-between; } .win.docked.sheet-open > .bar .grow { display: none; }
.win.docked.sheet-open .details { display: block; }
@keyframes sheet { from { opacity: 0; transform: translateY(-4px); } }
.win.docked > .sum { display: none; }
.win.docked .diags { max-height: 40%; }
.pkgs { display: none; position: absolute; z-index: 4; left: 50%; transform: translateX(-50%); top: 10px; max-width: calc(100% - 24px); padding: 8px 12px; font-size: 13px; border-radius: 8px;
  background: var(--light); color: var(--fg); border: 1px solid var(--divider); border-left: 3px solid var(--accent); box-shadow: 0 4px 14px rgba(0,0,0,.28); }
.pkgs.on { display: flex; gap: 8px; align-items: center; }
.pkgs > span:not(.mini) { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pkgs .mini.busy i { width: 35% !important; animation: phx-busy 1.1s ease-in-out infinite; }
.pkgs .icon { color: var(--info); }
/* the loading card: packages downloading, then the first build */
.load { max-width: 360px; margin: 0 auto; text-align: left; color: var(--fg2-dark); }
.win.docked.light .load, .win:not(.docked) .load { color: var(--fg2); }
.load h3 { margin: 0 0 10px; font: 600 14px "Noto Sans", system-ui, sans-serif; color: inherit; }
.load .bar { display: block; height: 6px; padding: 0; border: 0; border-radius: 9999px; background: rgb(127 127 127 / 25%); overflow: hidden; }
.load .bar i { display: block; height: 100%; background: var(--accent); border-radius: inherit; transition: width .25s ease; }
.load .bar.busy i { width: 35% !important; animation: phx-busy 1.1s ease-in-out infinite; }
@keyframes phx-busy { from { transform: translateX(-100%); } to { transform: translateX(290%); } }
.load .count { margin: 8px 0 10px; font-size: 12px; font-variant-numeric: tabular-nums; }
.load .names { display: flex; flex-wrap: wrap; gap: 4px; min-height: 22px; }
.load .names span { font: 11px "DM Mono", monospace; padding: 1px 6px; border-radius: 4px; background: rgb(127 127 127 / 18%); }
.load .names span.ing { animation: phx-pulse 1s ease-in-out infinite alternate; }
.load .names span.ok { color: var(--accent); }
@keyframes phx-pulse { from { opacity: .45; } to { opacity: 1; } }
.load .note { margin-top: 12px; font-size: 11px; opacity: .8; }
.load .steps { display: flex; gap: 6px; margin: 0 0 12px; font-size: 11px; }
.load .steps span { display: flex; align-items: center; gap: 4px; opacity: .5; }
.load .steps span::before { content: ""; width: 8px; height: 8px; border-radius: 50%; border: 1.5px solid currentColor; box-sizing: border-box; }
.load .steps span.now { opacity: 1; font-weight: 600; }
.load .steps span.now::before { background: var(--accent); border-color: var(--accent); animation: phx-pulse .8s ease-in-out infinite alternate; }
.load .steps span.ok { opacity: .8; }
.load .steps span.ok::before { background: currentColor; }
.load .steps i { flex: 1; align-self: center; height: 1px; background: currentColor; opacity: .25; font-style: normal; }
.load .bar:not(.busy) i { background-image: linear-gradient(110deg, transparent 30%, rgb(255 255 255 / 35%) 50%, transparent 70%); background-size: 200% 100%; animation: phx-shine 1.4s linear infinite; }
@keyframes phx-shine { from { background-position: 150% 0; } to { background-position: -50% 0; } }
.load .alive { margin-top: 8px; font-size: 12px; display: flex; gap: 6px; align-items: center; }
.load .alive::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
@media (prefers-reduced-motion: reduce) { .load .bar i, .load .steps span.now::before, .load .alive::before { animation: none !important; } }
.pkgs .mini { flex: none; width: 80px; height: 4px; border-radius: 9999px; background: rgb(127 127 127 / 25%); overflow: hidden; }
.pkgs .mini i { display: block; height: 100%; background: var(--accent); }
.speedchip { display: none; position: absolute; right: 16px; bottom: 14px; z-index: 3; pointer-events: none; white-space: nowrap;
  height: 24px; padding: 0 10px; border-radius: 9999px; align-items: center; gap: 4px; font-size: 12px; font-weight: 600;
  font-variant-numeric: tabular-nums; color: #fff; background: var(--accent); box-shadow: 0 4px 12px rgba(0,0,0,.3); opacity: 0; }
.win.docked .speedchip { display: inline-flex; }
.win.docked.nospeed .speedchip { display: none; }
/* who made this, always said (it looks native on purpose; it must never pass for Overleaf's) */
.byline { display: none; position: absolute; left: 14px; bottom: 14px; z-index: 3; all: unset; }
.win.docked .byline { display: inline-block; position: absolute; left: 14px; bottom: 14px; z-index: 3; height: 22px; padding: 0 9px;
  max-width: calc(100% - 160px); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; box-sizing: border-box; /* clear of the \u26A1 chip, now always on */
  border-radius: 9999px; font: 600 11px/22px "Noto Sans", system-ui, sans-serif; color: var(--fg2-dark); background: rgb(27 34 44 / 55%);
  backdrop-filter: blur(4px); cursor: pointer; }
.win.docked .byline:hover { color: var(--fg-dark); background: rgb(27 34 44 / 80%); }
.win.docked.light .byline { color: var(--fg2); background: rgb(255 255 255 / 75%); }
.about { display: none; position: absolute; left: 14px; bottom: 44px; z-index: 4; width: min(320px, calc(100% - 28px)); padding: 12px;
  border-radius: var(--r2); background: var(--dark); color: var(--fg-dark); font-size: 12px; line-height: 1.5; box-shadow: 0 8px 24px rgba(0,0,0,.35); }
.about.open { display: block; }
.about .about-foot { margin-top: 8px; color: var(--fg2-dark); }
.about code { font-size: 11px; }
.about a { color: var(--ok-dark); }
.speedchip.slow { background: var(--bg-warning-01, #8f5514); }
/* the chip: always there (the setting on), its number updated in place, a brief flash on each repaint */
.speedchip { opacity: 1; transition: background-color .4s; }
.speedchip.flash { animation: chipflash .5s ease-out; }
@keyframes chipflash { 0% { filter: brightness(1.45); } 100% { filter: none; } }
@media (prefers-reduced-motion: reduce) { .speedchip.flash { animation: none; } }
/* startup: the first paint is in, the SSA program (instant typing) still coming */
.warmup { display: none; position: absolute; left: 12px; right: 12px; margin: 0 auto; width: fit-content; top: 10px; z-index: 3; pointer-events: none;
  min-height: 24px; box-sizing: border-box; padding: 4px 12px; border-radius: 12px; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; line-height: 16px; text-align: center;
  font-variant-numeric: tabular-nums; color: #fff; background: rgb(27 34 44 / 82%); box-shadow: 0 4px 12px rgba(0,0,0,.3); backdrop-filter: blur(4px); }
.warmup.on { display: inline-flex; }
/* a diff shown: what against, the changes, and its controls (a pill over the page, above the warmup) */
.diffbar { display: none; position: absolute; left: 12px; right: 12px; margin: 0 auto; width: fit-content; max-width: calc(100% - 24px); top: 10px; z-index: 4;
  box-sizing: border-box; padding: 3px 4px 3px 12px; border-radius: 14px; align-items: center; gap: 4px; font-size: 12px; font-weight: 600; line-height: 16px;
  font-variant-numeric: tabular-nums; color: #fff; background: rgb(27 34 44 / 88%); box-shadow: 0 4px 12px rgba(0,0,0,.3); backdrop-filter: blur(4px); }
.diffbar.on { display: inline-flex; }
.diffbar.on ~ .warmup, .diffbar.on ~ .pkgs { top: 46px; }
.diffbar .what { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.diffbar .what small { font-weight: 400; opacity: .75; }
.diffbar .n { white-space: nowrap; padding: 0 4px; }
.diffbar .busy::before { content: ""; display: inline-block; width: 7px; height: 7px; margin-right: 6px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
.diffbar button { all: unset; cursor: pointer; padding: 2px 7px; border-radius: 10px; color: #fff; white-space: nowrap; }
.diffbar button:hover:not(:disabled) { background: rgb(255 255 255 / 16%); }
.diffbar button:disabled { opacity: .4; cursor: default; }
.diffbar button.cur { background: rgb(255 255 255 / 22%); }
.diffset { display: none; position: absolute; top: 44px; left: 50%; transform: translateX(-50%); z-index: 5; width: 300px; max-width: calc(100% - 24px);
  box-sizing: border-box; padding: 12px; border-radius: 12px; font-size: 12px; color: #fff; background: rgb(27 34 44 / 96%); box-shadow: 0 8px 24px rgba(0,0,0,.4); }
.diffset.on { display: block; }
.diffset h4 { margin: 0 0 6px; font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .04em; color: rgb(255 255 255 / 60%); }
.diffset h4:not(:first-child) { margin-top: 12px; }
.diffset .opts { display: flex; flex-wrap: wrap; gap: 4px; }
.diffset button { all: unset; cursor: pointer; padding: 3px 9px; border-radius: 10px; background: rgb(255 255 255 / 10%); color: #fff; }
.diffset button:hover { background: rgb(255 255 255 / 18%); }
.diffset button[aria-pressed=true] { background: var(--accent); }
.diffset .pair { display: flex; align-items: center; gap: 6px; padding: 3px 6px 3px 3px; }
.diffset .pair i { display: inline-block; width: 12px; height: 12px; border-radius: 50%; box-shadow: inset 0 0 0 1px rgb(255 255 255 / 30%); }
.diffset .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 6px; }
.diffset input[type=color] { width: 30px; height: 22px; padding: 0; border: 0; background: none; cursor: pointer; }
.diffset .sample { margin-top: 10px; padding: 6px 8px; border-radius: 6px; background: #fff; color: #1b222c; font: 13px/1.4 "Noto Serif", serif; }
.diffset .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 12px; }
.diffset .foot small { opacity: .6; }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px 2px; background: rgb(255 255 255 / 25%); }
.pkgs.on ~ .warmup { top: 56px; } /* below the strip, both said */
.warmup .dot { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--accent); animation: phx-pulse .6s ease-in-out infinite alternate; }
.warmup .slow { color: #ffd27a; font-weight: 500; }
.warmup.ready { background: var(--accent); }
.warmup.ready .dot { display: none; }
@media (prefers-reduced-motion: reduce) { .warmup .dot { animation: none; } }
.win.docked .sum { background: var(--dark2); color: var(--fg2-dark); border-bottom: 1px solid var(--dark); height: 26px; }
.win.docked .sum:hover { background: var(--dark3); }
.win.docked .sum .first { color: var(--fg-dark); }
.win.docked .sum .n.error { color: var(--danger-dark); } .win.docked .sum .n.warning { color: var(--warn-dark); } .win.docked .sum .n.info { color: #97b6e5; }
/* as Overleaf's viewer: the scrollbar always there, pages 12 px apart and centered, and the
   viewer min-height 100% with the first page's margin collapsing through it (so it scrolls by 12 px, as native) */
.win.docked .viewer { min-height: 100%; }
.win.docked .stage { background: transparent; padding: 0; overflow-y: scroll; overflow-x: auto; }
.stage { overscroll-behavior: contain; will-change: scroll-position; }
/* (the slots' own rules, the raster's among them: the renderer's VIEWER_CSS, before these) */
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgba(27,34,44,.25); }
.load .btns { display: flex; gap: 8px; justify-content: center; margin-top: 10px; }
.load .btns .btn { cursor: pointer; padding: 4px 12px; border-radius: 9999px; border: 1px solid currentColor; background: none; color: inherit; font: inherit; font-weight: 600; }
.report { display: none; position: absolute; inset: 12px; z-index: 6; padding: 12px; flex-direction: column; gap: 6px; background: var(--bg, #fff); color: var(--fg, #1b222c); border-radius: 8px; box-shadow: 0 4px 24px rgba(0,0,0,.35); font-size: 12px; }
.report.on { display: flex; }
.report p { margin: 0; opacity: .8; }
.report textarea { flex: 1; min-height: 160px; font: 11px/1.35 ui-monospace, monospace; white-space: pre; resize: none; }
.report .btns { display: flex; gap: 8px; justify-content: flex-end; }
.report .btn, .diags .reportbtn { cursor: pointer; padding: 3px 12px; border-radius: 9999px; border: 1px solid currentColor; background: none; color: inherit; font: inherit; font-weight: 600; }
.diags .reportbtn { margin: 6px 8px; }
.slot .mark { position: absolute; background: rgba(255, 221, 0, .55); mix-blend-mode: multiply; border-radius: 3px; pointer-events: none;
  box-shadow: 0 0 0 1.5px rgba(232, 172, 0, .9), 0 0 10px 1px rgba(255, 200, 0, .55);
  transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; animation: phitex-mark-in .18s ease-out; }
@keyframes phitex-mark-in { from { opacity: 0; transform: scale(1.15); } }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; animation: none; }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; box-shadow: none !important; border-radius: 0 !important; }
.win.docked .slot { box-shadow: rgba(35,40,47,.05) 0 5px 5px, rgba(35,40,47,.03) 0 3px 14px, rgba(35,40,47,.08) 0 8px 10px; }
.win.docked.pdf-dark .slot { filter: invert(95%) hue-rotate(180deg) brightness(90%) contrast(90%); box-shadow: none; }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.win.docked .stage .slot { border-radius: 0;
  box-shadow: rgba(35,40,47,.05) 0 5px 5px, rgba(35,40,47,.03) 0 3px 14px, rgba(35,40,47,.08) 0 8px 10px; }
.win.docked .banner { margin: 0; top: 0; }
.win.docked .empty { color: var(--fg2-dark); }
.win.collapsed { resize: none; min-height: 0; min-width: 0; border-radius: 9999px; }
.win.collapsed > :not(header) { display: none; }
.win.collapsed header { border-radius: 9999px; padding-right: 4px; }
.win.collapsed .hide-collapsed { display: none; }
`;
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
  $ = (sel) => this.root.querySelector(sel);
  words;
  constructor(ev, store, opts = {}) {
    this.store = store;
    this.ev = ev;
    const w = this.words = { ...OVERLEAF_WORDS, ...opts.words };
    const host2 = document.createElement("phitex-preview");
    this.root = host2.attachShadow({ mode: "open" });
    this.root.innerHTML = `<style>${VIEWER_CSS}${PROBLEMS_CSS}${OUTLINE_CSS}${CSS}${PRINT_CSS}</style>
<div class="win" role="dialog" aria-label="PhiTeX preview">
  <header>
    <span class="icon" aria-hidden="true">preview</span>
    <span class="title">PhiTeX</span>
    <span class="badge hide-collapsed" title="${w.badgeTitle}">${w.badge}</span>
    <span class="grow"></span>
    <span class="find" id="find" hidden><input id="findq" type="search" placeholder="Find" spellcheck="false" aria-label="Find in the pages"><span id="findn"></span></span>
    <span class="chip" id="chip" title="Status"><span class="dot"></span><span id="chiptext">starting\u2026</span></span>
    <span class="group hide-collapsed">
      <button class="ib" id="prev" title="Previous page" aria-label="Previous page"><span class="icon">chevron_left</span></button>
      <span class="pno" id="pno">\u2013</span>
      <button class="ib" id="next" title="Next page" aria-label="Next page"><span class="icon">chevron_right</span></button>
      <button class="ib" id="dbg" title="Debug: check against a fresh build every 5 s" aria-pressed="false"><span class="icon">bug_report</span></button>
    </span>
    <button class="ib hide-docked" id="min" title="Collapse (Alt+Shift+P)" aria-label="Collapse"><span class="icon">close_fullscreen</span></button>
  </header>
  <div class="bar">
    <label>Main <select id="main" title="The file PhiTeX typesets"></select></label>
    <label>Zoom <select id="zoom"><option value="fit">Fit</option><option value="0.75">75%</option><option value="1">100%</option><option value="1.5">150%</option><option value="2">200%</option></select></label>

    <span class="grow"></span>
    <label class="show-docked" title="Check against a fresh build every 5 s"><input type="checkbox" id="dbg2"> Debug check</label>
    <button class="ib show-docked" id="tour" title="Take the tour" aria-label="Take the tour" style="width:auto;padding:0 6px;font-size:12px;color:var(--info)">Take the tour</button>
    <button class="ib show-docked" id="news" title="What's new" aria-label="What's new" style="width:auto;padding:0 6px;font-size:12px;color:var(--info)">What's new</button>
    <button class="ib" id="reload" title="Fetch the project's files again" aria-label="Reload files"><span class="icon">sync</span></button>
    <button class="btn" id="pdf" title="PhiTeX's PDF, made locally"><span class="icon">download</span>PDF</button>
    <div class="details" id="details"></div>
  </div>
  <div class="sum" id="sum" role="button" aria-expanded="false" tabindex="0" title="Diagnostics (click to list)"></div>
  <div class="diags" id="diags" role="list" aria-label="Diagnostics"></div>
  <div class="body">
    <div class="report" id="report" role="dialog" aria-label="Debug report">
      <b>Debug report</b>
      <p>Anonymized: no document text, nothing you typed, no file names (file1.tex, \u2026), no project id. Read it before you send it.</p>
      <textarea id="reporttext" readonly spellcheck="false"></textarea>
      <div class="btns"><button class="btn" id="reportcopy">Copy</button><button class="btn" id="reportmail">Email</button><button class="btn" id="reportclose">Close</button></div>
    </div>
    <div class="speedchip" id="speedchip" aria-live="off" title="From the keystroke to the page on screen"></div>
    <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
    <div class="diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="pkgs" id="pkgs" role="status" aria-live="polite"></div>
    <div class="warmup" id="warmup" role="status" aria-live="polite"></div>
    <button class="byline" id="byline" title="About this preview">${w.byline}</button>
    <div class="about" id="about" role="dialog" aria-label="About the PhiTeX preview">
      ${w.about}
    </div>
    <nav class="side" id="side" aria-label="Contents"></nav>
    <div class="keyhelp" id="keyhelp" role="dialog" aria-label="Keys" hidden></div>
    <div class="offpill" id="offpill">\u25CF disconnected</div>
    <div class="stage" id="stage"><div class="gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">\u2715</button></div><div class="approx" id="approx"></div><div class="banner" id="banner"></div><div class="empty" id="empty"><div class="load">${stepsHtml(w.steps, ["now", "", ""])}<h3>${w.steps[0]}\u2026</h3><div class="bar busy"><i></i></div></div></div><div class="viewer" id="viewer"></div></div>
  </div>
  <footer><span class="lat" id="lat" title="Click for details">\u2013</span><span class="grow"></span><span class="msg" id="msg">all local</span></footer>
</div>`.replace(/<span class="icon"(?: aria-hidden="true")?>(\w+)<\/span>/g, (_, n) => icon(n, n === "download" ? 16 : 18));
    this.host = host2;
    host2.style.display = "none";
    (document.body ?? document.documentElement).append(host2);
    this.win = this.$(".win");
    this.$("#prev").onclick = () => this.prev();
    this.$("#next").onclick = () => this.next();
    this.$("#pdf").onclick = () => ev.onPdf();
    this.$("#reload").onclick = () => ev.onReload();
    const dbg = this.$("#dbg");
    dbg.onclick = () => this.toggleDebug();
    this.$("#dbg2").onchange = () => this.toggleDebug();
    const license = this.$("#license");
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    if (!ev.onTour) this.$("#tour").style.display = "none";
    if (!ev.onDebug) for (const id of ["#dbg", "#dbg2"]) (this.$(id).closest("label") ?? this.$(id)).style.display = "none";
    if (!ev.onNews) this.$("#news").style.display = "none";
    this.$("#byline").onclick = (e) => {
      e.stopPropagation();
      this.$("#about").classList.toggle("open");
    };
    this.$("#stage").addEventListener("pointerdown", () => this.$("#about").classList.remove("open"));
    this.$("#news").onclick = () => {
      this.sheet(false);
      ev.onNews?.();
    };
    this.$("#tour").onclick = () => {
      this.sheet(false);
      ev.onTour?.();
    };
    this.$("#main").onchange = (e) => ev.onMain(e.target.value);
    this.$("#zoom").onchange = (e) => this.zoomTo(e.target.value);
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#chip").onclick = () => this.prefs.collapsed && this.collapse(false);
    this.$("#lat").onclick = () => {
      this.prefs.details = !this.prefs.details;
      this.win.classList.toggle("details-open", this.prefs.details);
      this.save();
    };
    this.$("#diags").onclick = (e) => {
      const at = e.target.closest(".phx-place[data-file]");
      if (at) {
        e.preventDefault();
        return ev.onGoto(at.dataset.file, Number(at.dataset.line) || 1);
      }
      const d = e.target.closest(".diag[data-line]");
      if (d) ev.onGoto(d.dataset.file, Number(d.dataset.line));
    };
    const sum = this.$("#sum");
    sum.onclick = () => this.diagnostics();
    sum.onkeydown = (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), sum.click());
    this.$("#stage").addEventListener("pointerdown", () => {
      this.diagsOpen(false);
      this.sheet(false);
    });
    this.root.addEventListener("keydown", (e) => e.key === "Escape" && this.diagsOpen(false));
    this.$("#gonex").onclick = () => this.dismissOffline();
    addEventListener("keydown", (e) => e.key === "Escape" && this.$("#stage").classList.contains("offline") && this.dismissOffline());
    window.addEventListener("keydown", (e) => {
      if (e.altKey && e.shiftKey && e.code === "KeyP") {
        e.preventDefault();
        if (!ev.onShortcut?.()) this.collapse(!this.prefs.collapsed);
      }
    });
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
    window.addEventListener("resize", () => this.place());
    new ResizeObserver(() => {
      if (this.docked) return this.prefs.zoom === "fit" ? this.redraw() : void 0;
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
    store?.load().then((p) => {
      this.prefs = { ...DEFAULTS, ...p };
      if (this.prefs.w < 300 || this.prefs.h < 180) Object.assign(this.prefs, { w: DEFAULTS.w, h: DEFAULTS.h });
      this.place();
    });
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
    this.$("#zoom").value = p.zoom;
    this.win.classList.toggle("details-open", p.details);
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
  /** What the strip over the page says: packages on their way, or a build that is taking a while. */
  pkgNow;
  buildSince = 0;
  actTick;
  prepSince = 0;
  /** The core readying its next rebuild: past 400 ms the strip says so, not "Typesetting". */
  preparing(on) {
    if (!this.words.preparing) return;
    this.prepSince = on ? this.prepSince || performance.now() : 0;
    this.activity();
  }
  /** A build started (`on`) or ended: past 400 ms the strip says so, with its time. */
  /** A streamed open's pages so far (the strip: which page it is on). */
  streamed;
  streaming(s) {
    this.streamed = s ? s.pages : void 0;
    this.activity();
  }
  busy(on) {
    this.$("#chip").classList.toggle("busy", on);
    this.buildSince = on ? this.buildSince || performance.now() : 0;
    this.activity();
  }
  /** The strip over the page: never silent while the writer waits. */
  activity() {
    const el = this.$("#pkgs");
    const p = this.pkgNow;
    const prep = this.prepSince && performance.now() - this.prepSince > 400;
    const long = !!this.words.preparing && (this.buildSince && performance.now() - this.buildSince > 400 || prep);
    const on = (!!this.last || this.viewer.pages > 0) && (!!p || !!long);
    el.classList.toggle("on", on);
    if (on || this.buildSince || this.prepSince) this.actTick ??= setInterval(() => this.activity(), 250);
    else clearInterval(this.actTick), this.actTick = void 0;
    if (!on) return;
    if (p) {
      const names = p.names.slice(0, 3).join(", ") + (p.names.length > 3 ? ` and ${p.names.length - 3} more` : "");
      el.innerHTML = `${icon("download", 16)}<span></span><span class="mini"><i style="width:${p.pct}%"></i></span>`;
      el.querySelector("span").textContent = `Fetching ${names}${known(p.source) ? ` from ${p.source}` : ""} (${p.done} of ${p.total})\u2026`;
    } else if (prep) {
      const s = (performance.now() - this.prepSince) / 1e3;
      el.innerHTML = `${icon("sync", 16)}<span></span><span class="mini busy"><i></i></span>`;
      el.querySelector("span").textContent = this.words.preparing.replace("{s}", s < 10 ? s.toFixed(1) : String(Math.round(s)));
    } else {
      const s = (performance.now() - this.buildSince) / 1e3;
      el.innerHTML = `${icon("sync", 16)}<span></span><span class="mini busy"><i></i></span>`;
      const page = this.streamed ? ` page ${this.streamed + 1}` : "";
      el.querySelector("span").textContent = `Typesetting\u2026${page} ${s < 10 ? s.toFixed(1) : Math.round(s)} s`;
    }
  }
  /** Packages still on their way (the loading card's), so a stop reads as one only once they are in. */
  pkgBusy = false;
  /**
   * Before any page: a build that stopped at an error says so, in place of
   * "Typesetting…" (a stopped job's PDF is unfinished: no page to draw).
   */
  stopped() {
    if (this.engineNow && !this.engineNow.ready) return this.engineCard(this.engineNow.engine, null);
    if (this.last || this.pkgBusy) return false;
    const e = this.lastStatus?.diagnostics?.find((d) => d.severity === "error");
    if (!e) return false;
    const needed = errorNeeds(e.message);
    if (needed) return this.engineCard(needed, e.message);
    const empty = this.$("#empty");
    empty.innerHTML = `<div class="load"><h3></h3><div class="count"></div><div class="note"></div></div>`;
    if (e.code === "engine") {
      empty.querySelector("h3").textContent = "PhiTeX's engine stopped";
      empty.querySelector(".count").textContent = e.message;
      empty.querySelector(".note").textContent = "Your document is probably fine. Save again to retry.";
      if (e.detail) empty.querySelector(".load").append(foldedDetail(e.detail, "Engine details"));
      this.reportLink(empty.querySelector(".load"));
      empty.style.display = "";
      return true;
    }
    empty.querySelector("h3").textContent = "The build stopped before a page could be shown";
    empty.querySelector(".count").textContent = e.message + (e.file && e.line ? ` (${e.file}:${e.line})` : "");
    empty.querySelector(".note").textContent = this.words.stopped;
    this.reportLink(empty.querySelector(".load"));
    empty.style.display = "";
    return true;
  }
  /** "Copy debug info" under a card, if reports are wired. */
  reportLink(card) {
    if (!this.ev.onReport) return;
    const b = document.createElement("button");
    b.className = "reportbtn";
    b.textContent = "Copy debug info\u2026";
    b.style.cssText = "margin-top:10px;cursor:pointer;padding:3px 12px;border-radius:9999px;border:1px solid currentColor;background:none;color:inherit;font:inherit;font-weight:600";
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
    a.style.display = this.approx ? "block" : "none";
    a.textContent = `Approximate: needs ${ENGINES[e.engine].label}, shown with pdfLaTeX (fonts substituted, breaks may differ). ${this.words.realPdf}`;
    if (!e.ready) this.stopped();
    else if (this.$("#empty").querySelector(".engine")) this.$("#empty").style.display = "none";
  }
  /**
   * The project needs `want` (its preamble, or `error`, a build that
   * stopped, says so): which engine to run it with, as buttons.
   */
  engineCard(want, error) {
    const empty = this.$("#empty");
    const ready = ENGINES[want].ready;
    empty.innerHTML = `<div class="load engine"><h3></h3><div class="count"></div><div class="note"></div><div class="btns"></div></div>`;
    empty.querySelector("h3").textContent = error ? `This project needs ${want === "lualatex" ? "LuaLaTeX" : "XeLaTeX or LuaLaTeX"}` : `This project runs with ${ENGINES[want].label}`;
    empty.querySelector(".count").textContent = error ?? "Its preamble loads a package that pdfLaTeX can't run (fontspec, unicode-math, polyglossia, \u2026), or the engine was chosen for it.";
    empty.querySelector(".note").textContent = ready ? `Run it with ${ENGINES[want].label}?` : `\u26A1 Instant doesn't run ${ENGINES[want].label} yet: only pdfLaTeX. ${this.words.notReady}, or try pdfLaTeX if the project can do without those packages.`;
    const btns = empty.querySelector(".btns");
    const offer = this.engineNow?.engine === want ? ["pdflatex"] : want === "xelatex" ? ["xelatex", "lualatex"] : [want];
    for (const e of offer) {
      const b = document.createElement("button");
      b.className = "btn";
      b.textContent = e === "pdflatex" ? "Try pdfLaTeX anyway" : `Use ${ENGINES[e].label}${ENGINES[e].ready ? "" : " (coming)"}`;
      b.title = e === "pdflatex" ? "For this project: pdfLaTeX, whatever its preamble loads" : `For this project from now on (Settings \u2192 Engine for every project)`;
      b.onclick = () => this.ev.onEngine?.(e);
      btns.append(b);
    }
    empty.style.display = "";
    return true;
  }
  status(s) {
    this.lastStatus = s;
    queueMicrotask(() => this.stopped());
    const diags = s.diagnostics ?? [];
    const worst = diags[0]?.severity;
    const chip2 = this.$("#chip");
    const partial = !!s.pending || s.pages === 0;
    chip2.className = `chip ${worst === "error" ? "error" : partial || worst === "warning" ? "warning" : "ok"}`;
    this.$("#chiptext").textContent = `${s.pages} page${s.pages === 1 ? "" : "s"}${s.pending ? " \xB7 partial" : ""}`;
    chip2.title = (s.pending ? `${s.pending} parts not read (out of fuel or unsupported). ` : "") + (diags.length ? `${diags.length} diagnostic${diags.length === 1 ? "" : "s"}` : "No problems found") + (s.file ? ` \xB7 editing ${s.file}` : "");
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
      row.innerHTML = `${icon(d.severity, 16)}<span class="text"></span><span class="where"></span>`;
      row.querySelector(".text").textContent = d.message;
      if (d.detail && d.code === "engine") row.querySelector(".text").append(foldedDetail(d.detail, "Engine details"));
      else if (d.detail) {
        const pre = document.createElement("pre");
        pre.className = "detail";
        pre.textContent = d.detail;
        row.querySelector(".text").append(pre);
      }
      row.querySelector(".where").textContent = d.file ? `${d.file}${d.line ? `:${d.line}` : ""}` : "";
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
    const sum = this.$("#sum");
    sum.style.display = diags.length ? "" : "none";
    if (!diags.length) this.diagsOpen(false);
    const count = (sev) => diags.filter((d) => d.severity === sev).length;
    sum.innerHTML = ["error", "warning", "info"].filter((k) => count(k)).map((k) => `<span class="n ${k}">${icon(k, 14)}${count(k)}</span>`).join("") + `<span class="first"></span>${icon("chevron_right", 16).replace('class="icon"', 'class="icon caret"')}`;
    if (diags[0]) sum.querySelector(".first").textContent = diags[0].message;
    const errs = JSON.stringify(diags.filter((d) => d.problem && d.severity === "error").map((d) => [d.message, d.file, d.line]));
    if (errs !== "[]" && errs !== this.errsClosed) this.diagsOpen(true);
    this.win.classList.toggle("nopages", s.pages === 0 && !this.last);
    if (errs === "[]") this.errsClosed = "";
    this.stale(s);
    queueMicrotask(() => this.emit());
  }
  /**
   * Packages downloading: said plainly, so a slow first build reads as the
   * download's (TeX Live's files, not the project's, not Overleaf's).
   */
  /** The loading card's clock: when loading began, the last file that arrived, its ticker. */
  loadT0 = 0;
  loadLast = 0;
  loadDone = -1;
  loadTick;
  loadPct = 0;
  /** The card's live line: time so far and the last arrival, so a long fetch reads as working, not stuck. */
  alive() {
    const el = this.$("#empty").querySelector(".alive");
    if (!el || this.last) {
      clearInterval(this.loadTick);
      this.loadTick = void 0;
      return;
    }
    const now = performance.now(), s = Math.round((now - this.loadT0) / 1e3), quiet = (now - this.loadLast) / 1e3;
    const fetching = el.dataset.phase === "fetch";
    if (el.dataset.phase === "read") {
      el.textContent = `Reading \xB7 ${s} s`;
      return;
    }
    el.textContent = fetching ? quiet < 4 ? `Still fetching \xB7 ${s} s \xB7 files arriving` : `Still fetching \xB7 ${s} s \xB7 a large package (TikZ, fonts) is on its way` : `${this.words.steps[2]} \xB7 ${s} s`;
    if (s >= 20 && fetching) el.textContent += " \xB7 only the first time: next visits reuse them";
  }
  /** The reading step's last state, and when its card last changed (redrawn at most every 50 ms). */
  readNow;
  readDrawn = 0;
  readTimer;
  /**
   * Reading the project, before the packages: what is happening (the
   * download from Overleaf, each file unpacked, each figure handed to the
   * engine), the file in hand and how far, so the first seconds read as work.
   */
  reading(r) {
    if (this.last || this.pkgBusy || this.loadDone >= 0) return;
    this.readNow = r;
    this.loadT0 ||= performance.now();
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
    const empty = this.$("#empty");
    let card = empty.querySelector(".load.reading");
    if (!card) {
      empty.innerHTML = `<div class="load reading">${stepsHtml(this.words.steps, ["now", "", ""])}<h3></h3><div class="bar busy"><i></i></div><div class="alive" data-phase="read"></div><div class="count"></div><div class="names"></div><div class="note">${this.words.reading}</div></div>`;
      card = empty.querySelector(".load.reading");
    }
    card.querySelector("h3").textContent = r.h;
    const pct = r.pct ?? (r.total ? Math.round(100 * (r.done ?? 0) / r.total) : void 0);
    const bar2 = card.querySelector(".bar");
    bar2.classList.toggle("busy", pct === void 0);
    bar2.querySelector("i").style.width = pct === void 0 ? "" : `${pct}%`;
    card.querySelector(".count").textContent = r.total ? `${r.done ?? 0} of ${r.total}` : "";
    const names = card.querySelector(".names");
    names.innerHTML = "";
    if (r.detail) {
      const c = document.createElement("span");
      c.className = "ing";
      c.textContent = r.detail;
      names.append(c);
    }
    this.alive();
    this.loadTick ??= setInterval(() => this.alive(), 1e3);
  }
  packages(p) {
    const done = p.done?.length ?? 0;
    const total = done + p.loading.length;
    const now = performance.now();
    if (!this.loadT0) this.loadT0 = now;
    if (done !== this.loadDone) {
      this.loadDone = done;
      this.loadLast = now;
    }
    const pct = this.loadPct = Math.max(this.loadPct, total ? Math.round(100 * done / total) : 100);
    if (!this.last) {
      const empty = this.$("#empty");
      if (!p.loading.length && !p.building) {
        this.pkgBusy = false;
        if (!this.stopped()) {
          empty.innerHTML = `<div class="load">${stepsHtml(this.words.steps, ["ok", "ok", "now"])}<h3>${this.words.steps[2]}\u2026</h3><div class="bar busy"><i></i></div><div class="alive" data-phase="tex"></div></div>`;
          this.loadT0 ||= performance.now();
          this.alive();
          this.loadTick ??= setInterval(() => this.alive(), 1e3);
        }
        return;
      }
      const recent = [...(p.done ?? []).slice(-10).map((n) => [n, "ok"]), ...p.loading.slice(0, 8).map((n) => [n, "ing"])];
      const fetch2 = p.loading.length > 0;
      this.pkgBusy = true;
      empty.innerHTML = `<div class="load">${stepsHtml(this.words.steps, ["ok", fetch2 ? "now" : "ok", fetch2 ? "" : "now"])}<h3></h3><div class="bar${fetch2 ? "" : " busy"}"><i style="width:${pct}%"></i></div><div class="alive" data-phase="${fetch2 ? "fetch" : "tex"}"></div><div class="count"></div><div class="names"></div><div class="note"></div></div>`;
      this.alive();
      this.loadTick ??= setInterval(() => this.alive(), 1e3);
      empty.querySelector("h3").textContent = p.loading.length ? "Fetching LaTeX packages\u2026" : "Typesetting\u2026";
      empty.querySelector(".count").textContent = p.loading.length ? `${done} of ${total} files${p.unavailable.length ? ` \xB7 ${p.unavailable.length} not found` : ""}` : `with ${done} package file${done === 1 ? "" : "s"}; a build may ask for more`;
      const names = empty.querySelector(".names");
      for (const [n, k] of recent) {
        const c = document.createElement("span");
        c.className = k;
        c.textContent = n;
        names.append(c);
      }
      empty.querySelector(".note").textContent = `Downloaded${known(p.source) ? ` from ${p.source}` : ""} once, then kept in ${this.words.keptIn}. Only package names are sent, never your files.`;
      return;
    }
    this.pkgNow = p.loading.length ? { names: p.loading, done, total, pct, source: p.source } : void 0;
    this.activity();
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
    const chip2 = this.$("#chip");
    return {
      page: this.at,
      pages: this.lastStatus?.pages ?? 0,
      zoom: this.prefs.zoom,
      debug: this.debugOn,
      level: ["error", "warning", "ok"].find((c) => chip2.classList.contains(c)) ?? "ok",
      chip: this.$("#chiptext").textContent ?? "",
      chipTitle: chip2.title,
      speed: this.speed,
      sheet: this.win.classList.contains("sheet-open"),
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
   * page's width, 40 being its scrollbar's padding.
   */
  /** Device pixels per PDF point at the current zoom: the size pages are drawn at. */
  pixelsPerPoint() {
    return 96 / 72 * this.scale() * (window.devicePixelRatio || 1);
  }
  scale() {
    if (this.prefs.zoom !== "fit") return Number(this.prefs.zoom);
    const pageW = (this.last?.w ?? 612) * (96 / 72);
    return Math.max(this.$("#stage").clientWidth - 80, 200) / pageW;
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
  /** The document's outline (its PDF's bookmarks), from the host after each build. */
  outline(items) {
    this.contents?.set(items);
    this.contents?.at(this.at);
  }
  /** The contents shown or hidden (remembered on this machine). */
  side(on = !this.win.classList.contains("side-on")) {
    if (!this.contents) return;
    this.win.classList.toggle("side-on", on);
    try {
      localStorage.setItem("phitex.side", on ? "1" : "0");
    } catch {
    }
    if (this.prefs.zoom === "fit") this.redraw();
  }
  /** A button of the host's in the header, before the page controls (the CLI's Compare). */
  /**
   * The loading card at step `k` (0, 1, 2 of the words' steps), saying
   * `text` (the host's own phases: the CLI's build); nothing once a page
   * is shown.
   */
  loadStep(k, text) {
    if (this.last) return;
    clearInterval(this.loadTick);
    this.loadTick = void 0;
    const empty = this.$("#empty");
    let card = empty.querySelector(".load");
    if (!card || card.querySelector(".engine")) {
      empty.innerHTML = `<div class="load">${stepsHtml(this.words.steps, ["", "", ""])}<h3></h3><div class="bar busy"><i></i></div><div class="alive"></div></div>`;
      empty.style.display = "";
      card = empty.querySelector(".load");
    }
    card.querySelectorAll(".steps span").forEach((s, i) => s.className = i < k ? "ok" : i === k ? "now" : "");
    card.querySelector("h3").textContent = `${this.words.steps[k]}\u2026`;
    let alive = card.querySelector(".alive");
    if (!alive) card.append(alive = Object.assign(document.createElement("div"), { className: "alive" }));
    alive.textContent = text;
    delete alive.dataset.phase;
  }
  headerButton(label, title, onclick) {
    const b = document.createElement("button");
    b.className = "ib txt";
    b.title = title;
    b.textContent = label;
    b.onclick = () => onclick(b);
    this.$("header .grow").after(b);
    return b;
  }
  /** The contents and the keys, as the host asked (PanelOptions). */
  extras(opts) {
    if (opts.header) this.win.classList.add("own-header");
    if (opts.footer === false) this.win.classList.add("no-footer");
    this.findAndZoom(!!opts.keys);
    if (opts.outline) {
      this.win.classList.add("has-side");
      this.contents = new Outline(this.$("#side"), { goToPlace: (k, top) => this.viewer.goToPlace(k, top) });
      const b = document.createElement("button");
      b.className = "ib";
      b.title = "Contents (t)";
      b.setAttribute("aria-label", "Contents");
      b.textContent = "\u2630";
      b.onclick = () => this.side();
      this.$("header .title").before(b);
      let on = true;
      try {
        on = localStorage.getItem("phitex.side") !== "0";
      } catch {
      }
      this.win.classList.toggle("side-on", on);
    }
    if (!opts.keys) return;
    const help = this.$("#keyhelp");
    const extra = [["e", "the build's errors and warnings"], ...opts.outline ? [["t", "the contents beside the pages"]] : []];
    help.innerHTML = `<b>Keys</b><table>${[...KEYS, ...extra].map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
    addEventListener(
      "keydown",
      (e) => {
        if (e.ctrlKey || e.altKey || e.metaKey || e.target?.closest?.("input, textarea, select, [contenteditable=true]")) return;
        const drawer = this.win.classList.contains("diags-open");
        if (e.key === "Escape" && drawer) this.diagnostics(false);
        else if (e.key === "e") this.diagnostics();
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
  /** The errors last closed by hand (the drawer opens again when they change). */
  errsClosed = "";
  /** The diagnostics drawer. */
  diagnostics(on = !this.win.classList.contains("diags-open")) {
    if (!on) this.errsClosed = JSON.stringify((this.lastStatus?.diagnostics ?? []).filter((d) => d.problem && d.severity === "error").map((d) => [d.message, d.file, d.line]));
    this.diagsOpen(on);
    this.emit();
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
    this.prefs.zoom = z;
    this.zoomShown();
    this.save();
    this.redraw();
    this.emit();
  }
  /** The zoom menu at the zoom now: a zoom not on it (a pinch's) as its own entry. */
  zoomShown() {
    const sel = this.$("#zoom");
    const z = this.prefs.zoom;
    let own = sel.querySelector("option.own");
    if ([...sel.options].some((o) => o.value === z && !o.classList.contains("own"))) own?.remove();
    else {
      own ??= sel.appendChild(Object.assign(document.createElement("option"), { className: "own" }));
      own.value = z;
      own.textContent = `${Math.round(Number(z) * 100)}%`;
    }
    sel.value = z;
  }
  pinchEnd;
  /**
   * Zoom by `f` keeping the point (`px`, `py`, from the stage's top left)
   * where it is: the boxes at once, the pages drawn sharp again once the
   * gesture rests.
   */
  zoomAt(f, px, py) {
    const stage = this.$("#stage");
    const old = this.scale();
    const next = Math.min(8, Math.max(0.25, old * f));
    if (Math.abs(next - old) < 1e-4) return;
    const cx = stage.scrollLeft + px, cy = stage.scrollTop + py;
    this.prefs.zoom = String(Number(next.toFixed(4)));
    this.$("#stage").classList.remove("fit");
    this.viewer.redraw(true);
    stage.scrollLeft = cx * next / old - px;
    stage.scrollTop = cy * next / old - py;
    this.zoomShown();
    clearTimeout(this.pinchEnd);
    this.pinchEnd = setTimeout(() => {
      this.save();
      this.viewer.redraw();
      this.emit();
    }, 150);
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
        const d = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
        this.zoomAt(Math.exp(-Math.max(-60, Math.min(60, d)) * 4e-3), e.clientX - r.left, e.clientY - r.top);
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
    this.$("#dbg").setAttribute("aria-pressed", String(this.debugOn));
    this.$("#dbg2").checked = this.debugOn;
    this.ev.onDebug?.(this.debugOn);
    this.msg(this.debugOn ? "debug check on" : "debug check off");
    this.emit();
  }
  /** The settings sheet (docked: main file, view, reload, the latency details). */
  sheet(on = !this.win.classList.contains("sheet-open")) {
    this.win.classList.toggle("sheet-open", on && !!this.docked);
    this.emit();
  }
  /** The ⚡ chip shown or not (the popup's setting). */
  speedChip(on) {
    this.win.classList.toggle("nospeed", !on);
  }
  /** A repaint reached the screen, `ms` after its keystroke: the ⚡ chip says how fast. */
  painted(ms) {
    this.speed = { ms, at: performance.now() };
    const chip2 = this.$("#speedchip");
    chip2.textContent = `${ms < 10 ? ms.toFixed(1) : Math.round(ms)} ms`;
    chip2.classList.toggle("slow", ms > 100);
    chip2.classList.remove("flash");
    void chip2.offsetWidth;
    chip2.classList.add("flash");
    this.emit();
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
    const esc4 = (t) => t.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
    const n = d.busy ? `<span class="n busy">${esc4(d.busy)}</span>` : `<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    el.innerHTML = `<span class="what" title="${esc4(d.title)}">Diff vs ${esc4(d.title)} <small>${esc4(d.when)}</small></span><button data-a="prev" title="Previous change (Shift+N)" ${d.busy || !d.changes ? "disabled" : ""}>\u2039</button>${n}<button data-a="next" title="Next change (N)" ${d.busy || !d.changes ? "disabled" : ""}>\u203A</button><span class="sep"></span><button data-a="toggle" class="${d.showing === "current" ? "cur" : ""}" title="Show the ${d.showing === "diff" ? "current version" : "diff"} (D)">${d.showing === "diff" ? "Current" : "Diff"}</button><button data-a="pdf" title="Download the diff PDF" ${d.busy ? "disabled" : ""}>\u2B07 PDF</button><button data-a="tex" title="Download diff.tex" ${d.busy ? "disabled" : ""}>\u2B07 .tex</button><button data-a="settings" title="Diff style: colors, markup" aria-label="Diff style">\u2699</button><button data-a="close" title="Stop comparing" aria-label="Stop comparing">\u2715</button>`;
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
  warmTimer;
  /** Startup: the first paint is in, instant typing still being prepared (a pill over the page, the time going); then "ready", briefly. */
  warmup(w) {
    const el = this.$("#warmup");
    clearInterval(this.warmTimer);
    if (w.state === "ready") {
      el.className = "warmup on ready";
      el.innerHTML = `<span class="dot"></span>Instant typing ready`;
      this.warmTimer = setTimeout(() => el.className = "warmup", 2500);
      return;
    }
    const since = w.since ?? Date.now();
    const tick = () => {
      const s = Math.round((Date.now() - since) / 1e3);
      el.innerHTML = `<span class="dot"></span><span>Preview ready \xB7 preparing instant typing\u2026 ${s} s${w.slow ? ` <span class="slow">\xB7 edits slower until ready</span>` : ""}</span>`;
    };
    el.className = "warmup on";
    tick();
    this.warmTimer = setInterval(tick, 1e3);
  }
  /** The diagnostics list: a drawer above the pages, pushing them down; it never covers them. */
  diagsOpen(on) {
    this.win.classList.toggle("diags-open", on);
    this.$("#sum").setAttribute("aria-expanded", String(on));
  }
  latency(summary, details) {
    this.$("#lat").textContent = summary;
    if (details) this.$("#details").textContent = details;
  }
  msg(t, err = false) {
    const m = this.$("#msg");
    m.textContent = t;
    m.className = `msg${err ? " err" : ""}`;
    m.title = t;
    m.onclick = null;
    m.style.cursor = "";
  }
  error(t) {
    this.msg(t, true);
    if (!this.last && !this.pkgBusy) {
      clearInterval(this.loadTick);
      this.loadTick = void 0;
      const empty = this.$("#empty");
      empty.innerHTML = `<div class="load"><h3>Couldn't open the project</h3><div class="count"></div><div class="note"></div></div>`;
      empty.querySelector(".note").textContent = this.words.openFailed;
      empty.querySelector(".count").textContent = t;
      if (this.ev.onReport) {
        const b = document.createElement("button");
        b.className = "reportbtn";
        b.textContent = "Report a problem\u2026";
        b.onclick = () => void this.report();
        empty.querySelector(".load").append(b);
      }
    }
    if (this.ev.onReport) {
      const m = this.$("#msg");
      m.title = `${t} \u2014 click for a debug report to send`;
      m.style.cursor = "pointer";
      m.onclick = () => void this.report();
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
  redraw() {
    this.viewer?.redraw();
  }
  /** ⌃ ⌄ and the page number, for the page in view. */
  nav() {
    const n = this.viewer.pages;
    const k = this.at;
    this.$("#pno").textContent = n ? `${k + 1} / ${n}` : "\u2013 / 0";
    this.$("#prev").disabled = k <= 0;
    this.$("#next").disabled = k >= n - 1;
    queueMicrotask(() => this.emit());
  }
  /** The pages there are, by hash (the viewer draws the changed ones it shows). */
  layout(hashes) {
    this.$("#empty").style.display = "none";
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes);
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
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
  /** The debug report, shown whole before it is copied or emailed. */
  async report() {
    const text = await this.ev.onReport?.() ?? "";
    const box = this.$("#report");
    this.$("#reporttext").value = text;
    box.classList.add("on");
    const copy = () => navigator.clipboard.writeText(text).catch(() => this.$("#reporttext").select());
    this.$("#reportcopy").onclick = () => void copy().then(() => this.$("#reportcopy").textContent = "Copied \u2713");
    const mail = this.$("#reportmail");
    mail.style.display = REPORT_TO ? "" : "none";
    mail.onclick = () => void copy().then(() => {
      const body = "Please paste the debug report here (it is on your clipboard), and say what you were doing:\n\n";
      window.open(`mailto:${REPORT_TO}?subject=${encodeURIComponent("PhiTeX Instant debug report")}&body=${encodeURIComponent(body)}`);
    });
    this.$("#reportclose").onclick = () => box.classList.remove("on");
  }
  /** Highlight `boxes` on page `k` (the source the editor is at). */
  mark(k, boxes2, scroll = true) {
    this.viewer.mark(k, boxes2, scroll);
  }
  page(img, k, n, hash) {
    if (!img) {
      if (!this.last) {
        this.$("#empty").textContent = n ? "This page is empty." : "No page shipped yet.";
        this.$("#empty").style.display = "";
      }
      if (this.lastStatus) this.stale({ ...this.lastStatus, pages: n });
      return;
    }
    this.last = img;
    this.$("#empty").style.display = "none";
    this.$("#stage").classList.remove("stale");
    this.$("#stage").classList.toggle("fit", this.prefs.zoom === "fit");
    if (this.viewer.pages <= k) this.viewer.layout(Array.from({ length: Math.max(n, k + 1) }, (_, i) => i === k && hash || `?${i}`));
    this.viewer.set(k, img, hash ?? null);
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
var WATCH = /* @__PURE__ */ new Set(["page", "progress", "diagnostics", "settled", "superseded", "sync"]);
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
      if (this.o.remote && "event" in e && WATCH.has(e.event)) return this.onWatch(e);
      if (e.event === "preparing") return this.sink.preparing?.(e.on);
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
      this.sync(f, t);
      this.status();
    });
    host2.onChanges((f, es) => this.queue(this.known(f), es));
    core.onLost?.(() => {
      this.opened = false;
      this.sink.error("core lost; reopening");
      this.sink.connection?.(false, "the engine stopped; reopening");
      this.reopen();
    });
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
  async glyphsOf(k) {
    const key = `${k}:${this.hashes[k] ?? ""}`;
    const have = this.glyphCache.get(key);
    if (have) return have;
    const r = await this.core.request({ op: "origins", page: k });
    if (!r.ok || !r.json) return [];
    const path = (n) => {
      n = n.replace(/^\.\//, "");
      if (this.o.remote) return n;
      return n in this.files ? n : n + ".tex" in this.files ? n + ".tex" : null;
    };
    const gs = glyphs(r.json, path);
    if (this.glyphCache.size > 4 * Math.max(this.hashes.length, 100)) this.glyphCache.clear();
    this.glyphCache.set(key, gs);
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
    const t = this.files[file];
    const at = byteOffset(t, pos);
    const lo = t.lastIndexOf("\n", pos - 1) + 1, hiC = t.indexOf("\n", pos);
    const line = [byteOffset(t, lo), byteOffset(t, hiC < 0 ? t.length : hiC)];
    this.followBytes(file, at, wordBytes(new TextEncoder().encode(t), at), line);
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
      const onLine = from(all, file, a, b);
      if (!onLine.length) continue;
      const word = w ? from(onLine, file, w[0], w[1]) : [];
      const hit = word.length ? word : lineAt(onLine, at);
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
  async watchOutline() {
    if (!this.sink.outline) return;
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
var esc3 = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
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
    if (this.error) return `<div class="note err">${esc3(this.error)}</div>`;
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
      const pills = c.refs.map((f) => `<span class="pill ${KINDS.has(f.kind) ? f.kind : "branch"}">${esc3(f.name)}</span>`).join("");
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? '<span class="end">FROM</span>' : i === ti ? '<span class="end">TO</span>' : "";
      const title = `${c.subject}
${c.author}, ${new Date(c.time * 1e3).toLocaleString()}
click: From \xB7 shift-click: To \xB7 double-click: against its first parent`;
      return `<div class="row${cls}" data-i="${i}" title="${esc3(title)}"><svg width="${W}" height="${ROW}">${svg.join("")}</svg><span class="txt">${end}<span class="sha">${esc3(c.short)}</span>${pills}${c.parents.length > 1 ? "<i>merge:</i> " : ""}${esc3(c.subject)}<span class="who">${esc3(c.author)}, ${ago(c.time)}</span></span></div>`;
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
var chip = document.createElement("div");
chip.style.cssText = "position:fixed;right:8px;bottom:8px;z-index:9;font:11px/1.3 ui-monospace,monospace;background:rgba(20,24,32,.82);color:#e8eef8;padding:4px 7px;border-radius:5px;pointer-events:none";
chip.textContent = "strips";
document.body.append(chip);
var strips = new Strips(stripSource, chip);
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
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
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
      steps: ["Connected", "Format", "Building"],
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
  remote: { source: (file, start, end) => void sock.request({ op: "source", file, start, end }) }
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
#phx-pick { position: fixed; z-index: 20; top: 36px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; overflow: auto;
  background: #f1f1ef; color: #141414; border: 1px solid #141414; border-radius: 0; box-shadow: none; font: 12px "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif; }
#phx-pick header { display: flex; align-items: baseline; gap: 10px; padding: 8px 12px; border-bottom: 1px solid #c9c9c5; }
#phx-pick header b { font-size: 10px; letter-spacing: .1em; text-transform: uppercase; }
#phx-pick header small { color: #8a8a86; }
#phx-pick header button { margin-left: auto; border: 1px solid #c9c9c5; background: none; border-radius: 0; padding: 0 8px; cursor: pointer; font: 11px ui-monospace, monospace; }
@media (prefers-color-scheme: dark) { #phx-pick { background: #121212; color: #e8e8e4; border-color: #e8e8e4; } #phx-pick header { border-color: #3a3a38; } #phx-pick header button { border-color: #3a3a38; color: #e8e8e4; } }`
}));
var pick;
var graph;
var closePick = () => {
  if (pick) pick.hidden = true;
};
panel.headerButton("Compare", "Compare with a past version: the commit graph (click From, shift-click To, double-click a commit against its parent)", () => {
  if (pick && !pick.hidden) return closePick();
  if (!pick) {
    pick = document.createElement("div");
    pick.id = "phx-pick";
    pick.innerHTML = `<header><b>Compare</b><small>click: from \xB7 shift-click: to (else the files as they are) \xB7 double-click: a commit's own changes</small><button type="button">Esc</button></header>`;
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
});
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
#phx-bar, #phx-con { --paper: #f1f1ef; --paper-2: #e6e6e3; --ink: #141414; --ink-2: #4a4a48; --ink-3: #8a8a86; --rule: #c9c9c5; --mark: #d71920; --caution: #8a5a00;
  --mono: "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace; --sans: "IBM Plex Sans", "Helvetica Neue", Helvetica, Arial, sans-serif; }
#phx-bar { position: fixed; left: 0; right: 0; bottom: 0; height: 24px; z-index: 15; display: flex; align-items: center; gap: 10px;
  padding: 0 10px; font: 11px var(--mono); font-variant-numeric: tabular-nums; background: var(--paper); color: var(--ink-2); border-top: 1px solid var(--rule);
  cursor: pointer; user-select: none; white-space: nowrap; overflow: hidden; }
#phx-bar .dot { width: 6px; height: 6px; background: var(--ink-3); flex: none; }
#phx-bar .dot.ok { background: var(--ink); } #phx-bar .dot.bad { background: var(--mark); }
#phx-bar .dot.busy { background: var(--mark); animation: phx-blink .8s steps(2) infinite; }
@keyframes phx-blink { 50% { opacity: 0; } }
#phx-bar .text { overflow: hidden; text-overflow: ellipsis; flex: 1; }
#phx-bar .new { flex: none; padding: 0; color: var(--mark); }
#phx-bar .new.warn { color: var(--caution); }
#phx-bar .hint { flex: none; color: var(--ink-3); font-family: var(--sans); font-size: 10px; letter-spacing: .1em; text-transform: uppercase; }
#phx-bar:hover .hint { color: var(--ink); }
#phx-con { position: fixed; left: 0; right: 0; bottom: 24px; height: 38vh; z-index: 15; display: flex; flex-direction: column;
  background: var(--paper); color: var(--ink); border-top: 1px solid var(--ink); }
#phx-con[hidden] { display: none; }
#phx-con header { display: flex; align-items: baseline; gap: 10px; padding: 5px 10px; font: 11px var(--sans); border-bottom: 1px solid var(--rule); }
#phx-con header b { font-weight: 600; letter-spacing: .1em; text-transform: uppercase; font-size: 10px; }
#phx-con header small { color: var(--ink-3); }
#phx-con header button { border: 1px solid var(--rule); background: none; color: var(--ink-2); border-radius: 0; padding: 0 8px; cursor: pointer;
  font: 10px var(--sans); letter-spacing: .08em; text-transform: uppercase; }
#phx-con header button:hover { border-color: var(--ink); color: var(--ink); }
#phx-con header button:first-of-type { margin-left: auto; }
#phx-con .log { flex: 1; overflow: auto; margin: 0; padding: 8px 10px; font: 12px/1.5 var(--mono); white-space: pre-wrap; word-break: break-word; }
#phx-con .error { color: var(--mark); } #phx-con .warning { color: var(--caution); } #phx-con .note, #phx-con .dim { color: var(--ink-3); }
#phx-con .ok { color: var(--ink); } #phx-con .busy { color: var(--ink-2); }
@media (prefers-color-scheme: dark) {
  #phx-bar, #phx-con { --paper: #121212; --paper-2: #1c1c1c; --ink: #e8e8e4; --ink-2: #a9a9a4; --ink-3: #6e6e6a; --rule: #3a3a38; --mark: #ff4a3d; --caution: #d9a441; }
}`
}));
pane.style.bottom = "24px";
var bar = document.createElement("div");
bar.id = "phx-bar";
bar.title = "phitex watch's status: click (or `) for everything it printed";
bar.innerHTML = `<span class="dot"></span><span class="text">phitex watch</span><span class="new" hidden></span><span class="hint">\` Terminal</span>`;
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
