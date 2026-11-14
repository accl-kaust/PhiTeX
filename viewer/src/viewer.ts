// The pages, continuous: every page stacked in the scroll view, as Overleaf's
// viewer shows them. Only the pages near the view are drawn, and a
// page is drawn again only when its box changed (each page's hash, from the
// core). The page most in view is the one an edit paints first.

import type { Link, PageImage } from "./types.ts";
import type { CiteAnswer } from "./session.ts";
import { LAYER_LINES } from "./text.ts";
import { waspLength, waspOff, waspOn } from "./wasp.ts";

export interface ViewerHost {
  /**
   * Draw page `k` (the viewer wants it: in view, and not drawn at its hash
   * or at the zoom); a promise that settles once it is drawn (set) or not,
   * so the viewer asks for a page no more than once at a time.
   */
  need(k: number): void | Promise<void>;
  /** The page most in view changed. */
  inView(k: number): void;
  /** Ctrl+click (⌘ on a Mac) or a middle click on a link: the same viewer in a new tab, at page `k`, `top` points down it (absent: such a click follows the link here). */
  openAt?(k: number, top: number | null): void;
  /**
   * How the mouse's back button and Alt+← go Back after a link: "history",
   * a page in a browser's tab (the watch, the demo): each link followed an
   * entry of the tab's history, its back (any: the mouse's button, which
   * Firefox gives the page no event for, Alt+←, the toolbar's) is Back;
   * "buttons", a page with no history of its own (VS Code's webview): the
   * button's own events. None (Overleaf): they stay the browser's.
   */
  backWith?: "history" | "buttons";
  /** CSS pixels per PDF point at the current zoom (1 is 100%). */
  scale(): number;
  /** Text selected on the pages: its boxes by page, in PDF points from each page's top left. */
  selected?(sel: { k: number; rects: [number, number, number, number][] }[]): void;
  /** A double-click at (x, y) on page `k`, in PDF points from its top left. */
  dbl?(k: number, x: number, y: number): void;
  /** A click on a collaborator's name: their cursor, at `pos` (UTF-16) in `file`. */
  peer?(file: string, pos: number): void;
  /** A click on a link to a web address (http, https or mailto): opened by the host (absent: in a new tab). */
  link?(uri: string): void;
  /**
   * What the citation under the pointer cites: every entry of its \cite
   * (from the source: the link's place, page `from.k` at `from.x`,
   * `from.y`, mapped back to its \cite{a,b,c}, ranges and all), in the
   * order cited. Null: not a citation (a \ref's link shows the page).
   */
  cited?(from: { k: number; x: number; y: number }, to: number, top: number | null): Cited[] | null;
  /**
   * The engine's reading of the citation under a point (the session's `cites` op: each key, its
   * label, where its entry is typeset), or null (none there, no answer): the cards then come
   * from it, the pages' reading shown till it answers and where it has none.
   */
  cites?(k: number, x: number, y: number): Promise<CiteAnswer[] | null>;
  /** A page box's CSS size at `cssWidth` (on whole device pixels). */
  box(d: { w: number; h: number }, cssWidth: number): [number, number];
}

/** A bibliography entry, as the host reads it (the project's .bib): a link's window shows it as a card. */
export interface Cited {
  /** Its BibTeX key, and its label as the document prints it ("2", "Knuth, 1984"). */
  key: string;
  label: string;
  title: string;
  authors: string[];
  /** Where it appeared (journal, book's publisher, conference) and when. */
  venue?: string;
  year?: string;
  doi?: string;
  url?: string;
  /** The entry as written in the .bib: Copy BibTeX's. */
  bibtex?: string;
  /** Where the bibliography prints it: page, and points down it (a click there goes there). */
  at?: [number, number | null];
}

/** A place the view was: its top's page, how far down it (0 to 1), the first words of the line there. */
interface Stop {
  k: number;
  fy: number;
  words: string;
  /** What was clicked there to leave it ("[2, 4]", "Figure 1", "[2] The TeXbook"): none for a place Back or Forward left. */
  via?: string;
  /** That link's box, in points on its page (Back lights it). */
  mark?: { k: number; box: [number, number, number, number] };
}

/** What a link's window shows: its link, the page it is on, a citation's entries. */
interface PeekState {
  link: Link | null;
  from: number;
  list: Cited[];
}

interface Slot {
  el: HTMLElement;
  /** The hash the page has now, and the one drawn (null: not drawn). */
  hash: string;
  drawn: string | null;
  img: PageImage | null;
}

/**
 * Where a page's ink is across it, as fractions of its width (the left
 * of its leftmost mark, the right of its rightmost): read once from a
 * small copy of its bitmap, kept with it; a blank page is all of it.
 */
const spans = new WeakMap<HTMLCanvasElement, [number, number]>();
function inkSpan(c: HTMLCanvasElement): [number, number] {
  const kept = spans.get(c);
  if (kept) return kept;
  const w = 160, h = Math.max(1, Math.round((c.height / c.width) * w));
  const t = document.createElement("canvas");
  t.width = w;
  t.height = h;
  const g = t.getContext("2d", { willReadFrequently: true })!;
  g.drawImage(c, 0, 0, w, h);
  const d = g.getImageData(0, 0, w, h).data;
  let l = w, r = -1;
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (d[i] + d[i + 1] + d[i + 2] < 600) (l = Math.min(l, x)), (r = Math.max(r, x));
    }
  const out: [number, number] = r < l ? [0, 1] : [l / w, (r + 1) / w];
  spans.set(c, out);
  return out;
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
      const tag = (e.target as Element).closest<HTMLElement>(".peer-tag[data-file]");
      if (tag && this.host.peer) return void (e.preventDefault(), this.host.peer(tag.dataset.file!, Number(tag.dataset.pos)));
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
      if (this.newTab(e, to, hit[5] ?? null)) return;
      const slot = (e.target as Element).closest<HTMLElement>(".slot");
      this.follow(to, hit[5] ?? null, slot ? this.linkText(Number(slot.dataset.k), hit) : "", slot ? { k: Number(slot.dataset.k), link: hit } : undefined);
    });
    // (a middle click on a link: a new tab, as a browser's)
    root.addEventListener("auxclick", (e) => {
      const hit = e.button === 1 ? this.linkAt(e) : null;
      if (hit && typeof hit[4] === "number" && this.newTab(e, hit[4], hit[5] ?? null, true)) e.preventDefault();
    });
    // (the middle button's own scroll mode not started on a link the host opens in a new tab)
    root.addEventListener("mousedown", (e) => e.button === 1 && this.host.openAt && this.linkAt(e) && e.preventDefault());
    // (over a link, the hand: a class on its page, which print.ts makes win
    // over the text layer's own text cursor; a link is nearly always text,
    // so a cursor set on the page alone never showed)
    root.addEventListener("mousemove", (e) => {
      const el = (e.target as Element).closest<HTMLElement>(".slot");
      const hit = this.linkAt(e);
      if (el) el.classList.toggle("on-link", !!hit);
      this.pointer = [e.clientX, e.clientY];
      this.hover(hit, el, e);
    });
    root.addEventListener("mouseleave", () => this.hover(null, null));
    const doc = root.ownerDocument;
    doc.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.peekHide();
      else if (host.backWith === "buttons" && e.altKey && e.key === "ArrowLeft" && this.past.length) (e.preventDefault(), this.back());
      else if (host.backWith === "buttons" && e.altKey && e.key === "ArrowRight" && this.future.length) (e.preventDefault(), this.forward());
    });
    // (the window's link gone from under the pointer by other means than a
    // move: the pages scrolled (a wheel, a key, a jump), the tab left, the
    // pointer out of the window: closed, it was left up over the pages)
    scroller.addEventListener("scroll", () => this.peekEl && !this.peekEl.hidden && !this.peekEl.matches(":hover") && this.peekHide(), { passive: true });
    doc.addEventListener("visibilitychange", () => this.peekHide());
    doc.defaultView?.addEventListener("blur", () => this.peekHide());
    doc.documentElement.addEventListener("mouseleave", () => this.peekHide());
    // (anywhere else the pointer goes, over the panel's bars too: not the link, not the window, it closes)
    doc.addEventListener("pointermove", (e) => {
      if (!this.peekEl || this.peekEl.hidden || !this.peekLink) return;
      const path = e.composedPath();
      if (path.includes(this.peekEl) || path.includes(root)) return;
      this.peekAt = null;
      this.peekHide();
    }, { passive: true });
    // (the tab's back and forward: to a links' entry n jumps in, the view
    // moved there, back or forward as many steps as it is from here)
    // (by how far the tab went, not to a count of stops: a trail forgotten (×, Back
    // turned off) or a page reloaded keeps the tab's entries, whose counts then
    // no longer match the trail, and a Back by count went nowhere)
    if (host.backWith === "history") {
      this.hAt = (doc.defaultView?.history.state as { phitexBack?: number } | null)?.phitexBack ?? 0;
      doc.defaultView?.addEventListener("popstate", (e) => {
        const n = (e.state as { phitexBack?: number } | null)?.phitexBack ?? 0;
        let d = n - this.hAt;
        this.hAt = n;
        for (; d < 0 && this.past.length; d++) this.step(-1);
        for (; d > 0 && this.future.length; d--) this.step(1);
      });
    }
    // (the mouse's back and forward buttons where no history has them: 3, 4)
    if (host.backWith === "buttons")
      for (const t of ["mousedown", "mouseup", "auxclick"] as const)
        doc.addEventListener(t, (e) => {
          const go = e.button === 3 ? -1 : e.button === 4 ? 1 : 0;
          if (!go || !(go < 0 ? this.past : this.future).length) return;
          e.preventDefault();
          if (t === "mouseup") go < 0 ? this.back() : this.forward();
        });
  }

  // ---- Link previews: a link hovered, the place it goes to shown in a small window on that page ----

  /**
   * The highlight's scroll margin, a share of the view's height (0 to 0.45): the
   * highlight that follows the editor is kept at least this far from the view's top
   * and bottom, the page scrolled just enough to bring it back to the band's edge
   * (Emacs's scroll-margin); 0, as before: the page moves only once the highlight
   * leaves the view, and then centres it.
   */
  margin = 0.2;

  /** Links previewed on hover (the host's setting). */
  peeks = true;
  /** Back and Forward after a jump (the pill, the history, the mouse's buttons): the host's setting. */
  backs = true;
  /** Where a jump lands lit for a moment: the host's setting. */
  flashes = true;
  /** Ctrl+click or a middle click opens a new tab (where the host has one): the host's setting. */
  newTabs = true;
  private peekEl: HTMLElement | null = null;
  /** The link the window is for (or is about to be), its page and where its box is on it. */
  private peekAt: { link: Link; k: number } | null = null;
  private peekTimer: ReturnType<typeof setTimeout> | undefined;
  private peekHiding: ReturnType<typeof setTimeout> | undefined;

  /**
   * The pointer over `hit` (a link on page slot `el`), or off every link:
   * the window shown once it rests 350 ms on one (80 ms when a window is up
   * already: moving along a list of citations), gone 250 ms after it leaves
   * the link and the window both.
   */
  private hover(hit: Link | null, el: HTMLElement | null, e?: MouseEvent): void {
    if (!this.peeks) return;
    const up = !!this.peekEl && !this.peekEl.hidden;
    if (hit && this.peekAt?.link === hit) return void clearTimeout(this.peekHiding);
    clearTimeout(this.peekTimer);
    if (!hit || !el || !e) {
      this.peekAt = null;
      // (off the link: gone at once, unless the pointer is on its way into the window, between the two)
      if (up) this.onTheWay() ? this.peekSoon(150) : this.peekHide();
      return;
    }
    const at = { link: hit, k: Number(el.dataset.k) };
    this.peekAt = at;
    this.peekTimer = setTimeout(() => this.peekAt === at && this.peekShow(at), up ? 80 : 350);
  }

  /** The window up for `at`: a web address said, a place in the document shown, its page asked for if not drawn yet. */
  private peekShow(at: { link: Link; k: number }): void {
    clearTimeout(this.peekHiding);
    const el = this.peekEl ?? this.peekMake();
    this.peekLink = at.link;
    this.peekFrom = at.k;
    const to = at.link[4];
    el.classList.toggle("url", typeof to === "string");
    // (a web address's window: no citation's cards left from the last one shown)
    if (typeof to === "string") el.classList.remove("cite");
    // (shown before it is drawn: a hidden window has no width to draw at, and keeps no scroll)
    el.hidden = false;
    if (typeof to === "string") {
      el.querySelector(".pu")!.textContent = to;
      const web = /^(https?:|mailto:)/i.test(to), go = el.querySelector<HTMLElement>(".go")!;
      el.querySelector(".where")!.textContent = web ? "Opens in your browser" : "Not followed (not a web address)";
      [go.hidden, go.textContent] = [!web, "Open"];
    } else {
      const [x0, y0, x1, y1] = at.link;
      const mid = { k: at.k, x: (x0 + x1) / 2, y: (y0 + y1) / 2 };
      // (the engine's answer, once it has one for this link; till then, and where it has none, the pages' reading)
      const said = this.citesKnown.get(Viewer.citeKey(mid));
      if (said === undefined && this.host.cites) void this.citesAsk(mid, at);
      const list = said?.length ? said : this.host.cited ? this.host.cited(mid, to, at.link[5] ?? null) : this.citedOnPage(at.k, at.link);
      el.classList.toggle("cite", !!list?.length);
      if (list?.length) this.peekCard(list, at.link);
      else this.peekDraw();
    }
    this.peekPlace(at);
  }

  /** The window, made once: in the scroller, so it scrolls with the link it is for. */
  private peekMake(): HTMLElement {
    const el = document.createElement("div");
    el.className = "peek";
    el.setAttribute("role", "tooltip");
    el.hidden = true;
    el.innerHTML = `<div class="pv"><canvas></canvas><i class="at"></i></div><div class="pc"></div><div class="pu"></div><div class="pf"><span class="where"></span><button type="button" class="go" data-jump>Go there</button><button type="button" class="pin" data-pin title="Keep it open (a window of its own, moved by its bar)" aria-label="Keep it open"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.8 1.6 14.4 6.2l-1.1 1.1-1-.3-2.6 2.6.2 2.6-1.1 1.1-2.6-2.6-3.2 3.2-.7-.7 3.2-3.2-2.6-2.6L4 6.4l2.6.2 2.6-2.6-.3-1z" fill="currentColor"/></svg></button></div>`;
    // (the hover window's timers: a pinned one stays, whatever the pointer does)
    el.addEventListener("mouseenter", () => el === this.peekEl && clearTimeout(this.peekHiding));
    // (out of the window: gone, after a moment to land back on its link)
    el.addEventListener("mouseleave", () => el === this.peekEl && this.peekSoon(100));
    // (a click in it: nothing, but on its buttons: the footer's Go there (Open for an address),
    // a card's Go to it, DOI, Copy BibTeX; the pin. The window is read, scrolled, moved, resized:
    // never a link itself)
    el.addEventListener("click", (e) => {
      if ((e.target as Element).closest("[data-pin]")) return void this.peekPin();
      // (a pinned window's bar: moved by, closed by, never a link)
      if ((e.target as Element).closest(".ph")) return;
      const st = this.stateOf(el);
      // (a card's own buttons: theirs, not the window's)
      const b = (e.target as Element).closest<HTMLElement>("[data-act]");
      if (b) return void this.peekAct(b, e, st);
      if (!(e.target as Element).closest("[data-jump]")) return;
      // (the link read before the window goes: hiding it forgets which it was)
      const { link, from } = st;
      const to = link?.[4];
      if (typeof to === "number" && this.newTab(e, to, link?.[5] ?? null)) return;
      // (the hover window goes with the jump; a pinned one stays where it was put)
      if (el === this.peekEl) this.peekHide();
      if (typeof to === "number") this.follow(to, link?.[5] ?? null, link ? this.linkText(from, link) : "", link ? { k: from, link } : undefined);
      else if (typeof to === "string" && /^(https?:|mailto:)/i.test(to)) this.host.link ? this.host.link(to) : window.open(to, "_blank", "noopener");
    });
    this.scroller.append(el);
    return (this.peekEl = el);
  }

  /** What window `el` shows: the hover window's now, or what a pinned one showed when it was pinned. */
  private stateOf(el: HTMLElement): PeekState {
    return el === this.peekEl ? { link: this.peekLink, from: this.peekFrom, list: this.peekList } : (this.pinned.get(el) ?? { link: null, from: 0, list: [] });
  }
  private pinned = new WeakMap<HTMLElement, PeekState>();

  /**
   * The hover window kept open as a window of its own: over the pages,
   * where it is, staying while they scroll; a bar to move it by, with what
   * it shows and × to close it. Everything in it works as before (scroll,
   * Go to it, DOI, Copy BibTeX); the next link hovered opens a new window.
   */
  private peekPin(): void {
    const el = this.peekEl;
    if (!el || el.hidden) return;
    this.pinned.set(el, this.stateOf(el));
    clearTimeout(this.peekTimer);
    clearTimeout(this.peekHiding);
    // (out of the scroller, into its frame: where it is on screen, kept there, at
    // the size it has, inside the frame, its corner in reach to resize it by)
    const box = (this.scroller.parentElement ?? this.scroller), b = box.getBoundingClientRect(), r = el.getBoundingClientRect();
    box.append(el);
    const h = Math.min(r.height, b.height - 16), t = Math.max(0, Math.min(r.top - b.top, b.height - h - 8));
    Object.assign(el.style, { left: `${r.left - b.left}px`, top: `${t}px`, width: `${r.width}px`, height: `${h}px` });
    el.classList.add("pinned");
    el.setAttribute("role", "dialog");
    const what = el.classList.contains("cite") ? (el.querySelector(".mh span")?.textContent ?? el.querySelector(".ti")?.textContent ?? "") : el.classList.contains("url") ? (el.querySelector(".pu")?.textContent ?? "") : (el.querySelector(".where")?.textContent ?? "");
    const bar = document.createElement("div");
    bar.className = "ph";
    bar.innerHTML = `<span class="t"></span><button type="button" class="x" aria-label="Close" title="Close">×</button>`;
    bar.querySelector(".t")!.textContent = what;
    el.prepend(bar);
    bar.querySelector(".x")!.addEventListener("click", (e) => (e.stopPropagation(), el.remove()));
    // (moved by its bar: kept inside the frame)
    bar.addEventListener("pointerdown", (e) => {
      if ((e.target as Element).closest(".x")) return;
      e.preventDefault();
      bar.setPointerCapture(e.pointerId);
      const [x0, y0, l0, t0] = [e.clientX, e.clientY, parseFloat(el.style.left), parseFloat(el.style.top)];
      const move = (m: PointerEvent) => {
        const w = box.clientWidth - el.offsetWidth, h = box.clientHeight - 30;
        el.style.left = `${Math.max(0, Math.min(w, l0 + m.clientX - x0))}px`;
        el.style.top = `${Math.max(0, Math.min(h, t0 + m.clientY - y0))}px`;
      };
      const up = () => (bar.removeEventListener("pointermove", move), bar.removeEventListener("pointerup", up));
      bar.addEventListener("pointermove", move);
      bar.addEventListener("pointerup", up);
    });
    // (resized by its corner: a page's view drawn again at the new width, sharp, once the drag rests)
    if (!el.classList.contains("cite") && !el.classList.contains("url")) {
      let t: ReturnType<typeof setTimeout> | undefined, w = el.offsetWidth;
      new ResizeObserver(() => {
        if (!el.isConnected || el.offsetWidth === w) return;
        w = el.offsetWidth;
        clearTimeout(t);
        t = setTimeout(() => this.peekDraw(el, this.pinned.get(el)?.link ?? undefined, true), 120);
      }).observe(el);
    }
    // (the next hover a new window: this one is no longer the hover window)
    this.peekEl = null;
    this.peekLink = null;
    this.peekAt = null;
    this.peekList = [];
  }

  /** The link the window shows now. */
  private get peekShown(): Link | null {
    return this.peekEl && !this.peekEl.hidden ? (this.peekLink ?? null) : null;
  }
  private peekLink: Link | null = null;
  /** The page the window's link is on. */
  private peekFrom = 0;

  /**
   * What a link on page `k` refers to, as the reader sees it: the text
   * layer's characters inside its box, widened to the whole reference: a
   * citation's numbers to its brackets ("4" of "[2, 4]" is "[2, 4]"), a
   * bare number to the word before it ("1" is "Figure 1", "page 2").
   * Back's list says it was clicked.
   */
  private linkText(k: number, link: Link): string {
    const s = this.slots[k];
    if (!s) return "";
    const b = s.el.getBoundingClientRect(), sx = b.width / this.size.w, sy = b.height / this.size.h;
    const [l, t, r, bt] = [b.left + link[0] * sx, b.top + link[1] * sy, b.left + link[2] * sx, b.top + link[3] * sy];
    for (const el of s.el.querySelectorAll<SVGTextElement>("text")) {
      const tr = el.getBoundingClientRect();
      if (tr.right < l || tr.left > r || tr.bottom < t || tr.top > bt) continue;
      const m = el.getScreenCTM(), text = el.textContent ?? "";
      if (!m) continue;
      // (the characters whose middles are inside the box: the first and the last)
      let a = -1, z = -1;
      for (let i = 0, n = Math.min(el.getNumberOfChars(), text.length); i < n; i++) {
        const e = el.getExtentOfChar(i);
        const p = new DOMPoint(e.x + e.width / 2, e.y + e.height / 2).matrixTransform(m);
        if (p.x >= l && p.x <= r && p.y >= t && p.y <= bt) (a < 0 && (a = i)), (z = i);
      }
      if (a < 0) continue;
      // (a citation's: out over its numbers, commas, dashes and spaces to the brackets round them)
      const inCite = /[\d,;\s–-]/;
      let x = a, y = z;
      while (x > 0 && inCite.test(text[x - 1])) x--;
      while (y < text.length - 1 && inCite.test(text[y + 1])) y++;
      if (x > 0 && "[(".includes(text[x - 1]) && y < text.length - 1 && "])".includes(text[y + 1])) return text.slice(x - 1, y + 2).replace(/\s+/g, " ");
      const got = text.slice(a, z + 1).trim();
      // (a bare number: with the word before it, "Figure 1", "Section 2.1", "page 3")
      if (/^[\dA-Z.]+$/.test(got)) {
        const before = /([A-Za-z]+)[~\s\u00a0]*$/.exec(text.slice(0, a));
        if (before) return `${before[1]} ${got}`;
      }
      return got.replace(/\s+/g, " ");
    }
    return "";
  }

  /** The window placed under the link's box (over it, where there is no room under), inside the view. */
  private peekPlace(at: { link: Link; k: number }): void {
    const el = this.peekEl!, s = this.slots[at.k];
    if (!s) return;
    this.peekLink = at.link;
    const b = s.el.getBoundingClientRect(), v = this.scroller.getBoundingClientRect();
    const sx = b.width / this.size.w, sy = b.height / this.size.h;
    const [x0, y0, , y1] = at.link;
    const w = el.offsetWidth, h = el.offsetHeight;
    const left = Math.max(v.left + 8, Math.min(b.left + x0 * sx - 24, v.right - w - 8));
    const below = b.top + y1 * sy + 6, above = b.top + y0 * sy - h - 6;
    const top = below + h <= v.bottom - 8 || above < v.top + 8 ? below : above;
    el.style.left = `${left - v.left + this.scroller.scrollLeft}px`;
    el.style.top = `${top - v.top + this.scroller.scrollTop}px`;
    // (the strip from the link to the window, in the view's coordinates: a pointer in it is on its way in)
    const lx0 = b.left + x0 * sx, lx1 = b.left + at.link[2] * sx, ly0 = b.top + y0 * sy, ly1 = b.top + y1 * sy;
    this.peekWay = top === below ? [Math.min(lx0, left), ly0, Math.max(lx1, left + w), top + 4] : [Math.min(lx0, left), top + h - 4, Math.max(lx1, left + w), ly1];
  }

  /** The pointer's last place over the pages (view coordinates), and the strip between the window's link and it. */
  private pointer: [number, number] = [0, 0];
  private peekWay: [number, number, number, number] = [0, 0, 0, 0];

  /** The pointer is between the link and its window, on its way in. */
  private onTheWay(): boolean {
    const [x, y] = this.pointer, [l, t, r, b] = this.peekWay;
    return x >= l && x <= r && y >= t && y <= b;
  }

  /**
   * The target page drawn in the window, the full page's width at the
   * window's, scrolled so the place is near its top, a bar beside it; a
   * page not drawn yet is asked for, and drawn here when it comes (set).
   */
  private peekDraw(el = this.peekEl, link = this.peekLink ?? this.peekAt?.link, keep = false): void {
    if (!el || !link || typeof link[4] !== "number") return;
    // (a pinned window resized: drawn again at its new width, scrolled to the same place)
    const pvKeep = el.querySelector<HTMLElement>(".pv")!, was = keep && pvKeep.scrollHeight ? pvKeep.scrollTop / pvKeep.scrollHeight : -1;
    const k = link[4], top = link[5] ?? 0, s = this.slots[k];
    const pv = el.querySelector<HTMLElement>(".pv")!, c = pv.querySelector("canvas")!;
    el.classList.toggle("wait", !s?.img);
    if (!s?.img) {
      el.querySelector(".where")!.textContent = `Page ${k + 1} · drawing…`;
      if (s) this.ask(k);
      return;
    }
    el.querySelector(".where")!.textContent = `Page ${k + 1}`;
    Object.assign(el.querySelector<HTMLElement>(".go")!, { hidden: false, textContent: "Go there" });
    const img = s.img, W = pv.clientWidth || 420, dpr = window.devicePixelRatio || 1;
    const all = img.canvas instanceof HTMLCanvasElement ? [img.canvas] : [...img.canvas.querySelectorAll("canvas")];
    // (the page's printed width, its margins left out, at the window's: the
    // text at a size to read, not the whole sheet's width shrunk)
    const base = all.find((b) => !b.classList.contains("phx-tile") && b.width);
    const [il, ir] = base ? inkSpan(base) : [0, 1];
    const pad = 10 / img.w, l = Math.max(0, il - pad), r = Math.min(1, ir + pad);
    const PW = W / (r - l), H = Math.round((PW * img.h) / img.w), X = -l * PW;
    c.style.width = `${W}px`;
    c.style.height = `${H}px`;
    c.width = Math.round(W * dpr);
    c.height = Math.round(H * dpr);
    const g = c.getContext("2d")!;
    g.fillStyle = "#fff";
    g.fillRect(0, 0, c.width, c.height);
    g.setTransform(dpr, 0, 0, dpr, X * dpr, 0);
    g.imageSmoothingQuality = "high";
    // (the page's own bitmaps: the whole page, and a zoom's sharper part over it where there is one)
    for (const b of all) {
      if (!b.width) continue;
      const tile = b.classList.contains("phx-tile");
      const f = (v: string) => (parseFloat(v) || 0) / 100;
      const [bl, bt, bw, bh] = tile ? [f(b.style.left), f(b.style.top), f(b.style.width), f(b.style.height)] : [0, 0, 1, 1];
      g.drawImage(b, bl * PW, bt * H, bw * PW, bh * H);
    }
    const y = (top / img.h) * H;
    pv.querySelector<HTMLElement>(".at")!.style.top = `${y}px`;
    pv.scrollTop = was >= 0 ? was * pv.scrollHeight : Math.max(0, y - 28);
  }

  /** The window gone `ms` on, unless the pointer comes back to it or its link (one timer: a later leave restarts it, never a second). */
  private peekSoon(ms: number): void {
    clearTimeout(this.peekHiding);
    this.peekHiding = setTimeout(() => this.peekHide(), ms);
  }

  /**
   * A citation's card: one entry, whole (its label and key, title,
   * authors, where and when, its DOI or address, Copy BibTeX); several (a
   * \cite of many, a range), a list of them, the one under the pointer
   * marked, each a click to its place in the bibliography.
   */
  private peekCard(list: Cited[], link: Link): void {
    const el = this.peekEl!, pc = el.querySelector<HTMLElement>(".pc")!;
    const esc = (t: string) => t.replace(/[&<>"]/g, (ch) => `&#${ch.charCodeAt(0)};`);
    const names = (a: string[]) => (a.length > 4 ? `${a.slice(0, 3).join(", ")} and ${a.length - 3} others` : a.length > 1 ? `${a.slice(0, -1).join(", ")} and ${a.at(-1)}` : (a[0] ?? ""));
    const here = (c: Cited) => list.length > 1 && !!c.at && c.at[0] === link[4] && c.at[1] === (link[5] ?? null);
    // (each entry whole, one or many: its label and key, title, authors, where and when, its DOI or address, Copy BibTeX, Go)
    // (each entry the same, one or many: three lines, the label, title and key; who, when and where; its links)
    const card = (c: Cited, i: number) => {
      const doi = c.doi ? `https://doi.org/${c.doi}` : "";
      return (
        `<section class="ce${here(c) ? " here" : ""}" data-i="${i}">` +
        `<div class="hd">${c.label ? `<span class="lab">[${esc(c.label)}]</span>` : ""}<span class="ti">${esc(c.title)}</span>${c.key ? `<code>${esc(c.key)}</code>` : ""}</div>` +
        // (the year next to the names: a long line cut at its end keeps it)
        `<div class="mt">${[[c.authors.length && esc(names(c.authors)), c.year && esc(c.year)].filter(Boolean).join(", "), c.venue && `<i>${esc(c.venue)}</i>`].filter(Boolean).join(" · ")}</div>` +
        `<div class="lk">` +
        [doi ? `<button type="button" data-act="open" data-u="${esc(doi)}">DOI</button>` : c.url ? `<button type="button" data-act="open" data-u="${esc(c.url)}">${esc(c.url.replace(/^https?:\/\//, "").split("/")[0])}</button>` : "", c.bibtex ? `<button type="button" data-act="bib" data-i="${i}">Copy BibTeX</button>` : "", c.at ? `<button type="button" data-act="go" data-i="${i}">Go to it</button>` : ""].filter(Boolean).join("<i>·</i>") +
        `</div></section>`
      );
    };
    const bibs = list.filter((c) => c.bibtex).length;
    pc.innerHTML =
      (list.length > 1 ? `<div class="mh"><span>${list.length} works cited here</span>${bibs > 1 ? `<button type="button" data-act="bib" data-i="all">Copy all BibTeX (${bibs})</button>` : ""}</div>` : "") +
      `<div class="cs">${list.map(card).join("")}</div>`;
    this.peekList = list;
    // (the one under the pointer in view, in the list's own scroll: scrollIntoView
    // scrolled the pages too, and a scroll of the pages closes the window)
    const cs = pc.querySelector<HTMLElement>(".cs")!, h = pc.querySelector<HTMLElement>(".ce.here");
    cs.scrollTop = 0;
    if (h && h.offsetTop + h.offsetHeight > cs.clientHeight) cs.scrollTop = h.offsetTop - 8;
    const pages = [...new Set(list.map((c) => c.at?.[0]).filter((k): k is number => k !== undefined))];
    el.querySelector(".where")!.textContent = `References · page ${pages.length ? pages.map((k) => k + 1).join(", ") : Number(link[4]) + 1}`;
    // (one work: the footer's Go there; several: each card's own Go to it)
    Object.assign(el.querySelector<HTMLElement>(".go")!, { hidden: list.length > 1, textContent: "Go there" });
  }
  private peekList: Cited[] = [];

  // ---- A citation read from the pages themselves: the bibliography as typeset ----

  /** Page `k`'s text lines (the text layer's), in points from its top left, top to bottom. */
  private pageLines(k: number): { t: string; x0: number; y0: number; x1: number; y1: number; b: number }[] {
    const s = this.slots[k];
    if (!s) return [];
    const b = s.el.getBoundingClientRect(), sx = this.size.w / b.width, sy = this.size.h / b.height;
    return [...s.el.querySelectorAll("text")]
      .map((t) => {
        const r = t.getBoundingClientRect();
        // (b: the line's baseline, the text's own y in the page's points (its SVG's viewBox))
        const y0 = (r.top - b.top) * sy, y1 = (r.bottom - b.top) * sy, base = Number(t.getAttribute("y"));
        return { t: (t.textContent ?? "").replace(/\s+/g, " ").trim(), x0: (r.left - b.left) * sx, y0, x1: (r.right - b.left) * sx, y1, b: Number.isFinite(base) ? base : y1 };
      })
      .filter((l) => l.t)
      .sort((a, c) => a.y0 - c.y0 || a.x0 - c.x0);
  }

  /** An entry's label, where the bibliography prints one at its line's start ("[4]", "[Knu84]"). */
  private static LABEL = /^\[([^\]]{1,12})\]\s*/;

  /**
   * The bibliography entry at page `k`, `top` points down it: its lines
   * from there to the next entry (a line starting with a label, or a gap
   * wider than a line), as one text. Null: no bibliography there (a
   * \ref's target: the page is shown instead).
   */
  private entryAt(k: number, top: number): { label: string; text: string } | null {
    const lines = this.pageLines(k);
    const i = lines.findIndex((l) => l.y1 > top + 1);
    if (i < 0) return null;
    const first = lines[i];
    // (a bibliography: its entries labelled, or a References heading above it on this page)
    const labelled = Viewer.LABEL.exec(first.t);
    const heading = lines.slice(0, i).some((l) => /^(references|bibliography|works cited|literature)$/i.test(l.t));
    if (!labelled && !heading) return null;
    const out = [first.t];
    const lead = first.y1 - first.y0;
    for (let j = i + 1, last = first; j < lines.length && out.length < 8; j++) {
      const l = lines[j];
      if (Viewer.LABEL.test(l.t) || l.y0 - last.y1 > lead * 0.9 || /^\d+$/.test(l.t)) break;
      out.push(l.t);
      last = l;
    }
    // (lines joined, a word broken at a line's end mended)
    const text = out.reduce((a, l) => (/[a-z]-$/.test(a) && /^[a-z]/.test(l) ? a.slice(0, -1) + l : `${a} ${l}`));
    const m = Viewer.LABEL.exec(text);
    return { label: m ? m[1] : "", text: m ? text.slice(m[0].length) : text };
  }

  /** Where the bibliography prints the entry labelled `label`: near page `near` (its other entries' page), its line's start. */
  private findLabel(label: string, near: number): [number, number] | null {
    for (const k of [near, near + 1, near - 1, near + 2]) {
      const l = this.pageLines(k).find((l) => l.t.startsWith(`[${label}]`));
      if (l) return [k, l.y0];
    }
    return null;
  }

  /**
   * An entry's text as a card: authors, title, where and when, the
   * usual "Authors. Title. Venue, year." of a bibliography style, its DOI
   * and address where it prints them. Sentences end at ". " but not at an
   * initial's ("D. E. Knuth").
   */
  private static asCited(label: string, text: string, at: [number, number]): Cited {
    const doi = /\b(10\.\d{4,9}\/[^\s,;]+[^\s,;.])/.exec(text)?.[1];
    const url = /(https?:\/\/[^\s,;]+[^\s,;.])/.exec(text)?.[1];
    const years = [...text.matchAll(/\b(1[5-9]\d\d|20\d\d)[a-z]?\b/g)];
    const year = years.at(-1)?.[0];
    const parts: string[] = [];
    let cur = "";
    for (const w of text.split(" ")) {
      cur = cur ? `${cur} ${w}` : w;
      // (a sentence's end: a full stop after a word longer than an initial)
      if (/[^.\s]{2,}[.?!]$/.test(w) && !/^(et|al|ed|eds|vol|no|pp|Proc|Trans|Jr|Inc)\.$/i.test(w)) (parts.push(cur), (cur = ""));
    }
    if (cur) parts.push(cur);
    const authors = (parts[0] ?? "").replace(/\.$/, "").split(/,\s*(?:and\s+)?|\s+and\s+/).map((a) => a.trim()).filter(Boolean);
    const title = (parts[1] ?? parts[0] ?? text).replace(/\.$/, "");
    const venue = parts.slice(2).join(" ").replace(/\.$/, "").replace(new RegExp(`,?\\s*${year ?? "^$"}\\s*$`), "") || undefined;
    return { key: "", label, title, authors: parts.length > 1 ? authors : [], venue, year, doi, url, at };
  }

  /**
   * What the citation under the pointer cites, read from the pages: the
   * citation's whole text ("[1, 3–5]": its numbers, ranges filled in;
   * else the links beside it in its brackets), each entry as typeset in
   * the bibliography. Null: the link's target is no bibliography entry.
   */
  /** The engine's answers by link (its page and middle), this build's: none (an empty list) where it has no citation; cleared by a new layout. */
  private citesKnown = new Map<string, Cited[]>();
  private citesAsking = new Set<string>();
  private static citeKey(m: { k: number; x: number; y: number }): string {
    return `${m.k},${Math.round(m.x)},${Math.round(m.y)}`;
  }

  /**
   * The engine asked for the citation at `mid` (the link of `at`): its cards made from the
   * answer, kept, and shown if the window is still on that link. An entry's page not drawn yet
   * (its text unknown) is asked for and the answer not kept: the window shown again when the
   * page comes asks again (the engine keeps its answers per build: that costs nothing).
   */
  private async citesAsk(mid: { k: number; x: number; y: number }, at: { k: number; link: Link }): Promise<void> {
    const key = Viewer.citeKey(mid);
    if (this.citesAsking.has(key)) return;
    this.citesAsking.add(key);
    // (each of the citation's own links (hyperref's, one a key: "[1–3]" is three), asked
    // for together, in their order: the link under the pointer one of them)
    const group = this.citeGroup(at.k, at.link);
    const answers = await Promise.all(group.map((l) => this.host.cites!(at.k, (l[0] + l[2]) / 2, (l[1] + l[3]) / 2).catch(() => null)));
    const seen = new Set<string>();
    const said = answers.flatMap((a) => a ?? []).filter((c) => !seen.has(c.key) && (seen.add(c.key), true));
    this.citesAsking.delete(key);
    const list = said?.length ? this.citedFrom(said) : [];
    if (!list) return;
    // (a numeric range, "[1–3]": only its ends are links, the works between them in none.
    // Those read from the bibliography as typeset, as the pages' reading does: by their labels)
    for (const m of this.linkText(at.k, at.link).matchAll(/(\d+)\s*[–-]\s*(\d+)/g)) {
      const [lo, hi] = [Number(m[1]), Number(m[2])];
      if (!list.some((c) => c.label === m[1]) || !list.some((c) => c.label === m[2]) || hi - lo > 200) continue;
      const near = list.find((c) => c.at)?.at?.[0] ?? at.k;
      for (let n = lo + 1; n < hi; n++) {
        if (list.some((c) => c.label === String(n))) continue;
        const pos = this.findLabel(String(n), near), e = pos && this.entryAt(pos[0], pos[1]);
        if (pos && e) list.push(Viewer.asCited(e.label || String(n), e.text, pos));
      }
      list.sort((a, b) => (Number(a.label) || 0) - (Number(b.label) || 0));
    }
    this.citesKnown.set(key, list);
    const el = this.peekEl;
    if (!list.length || !el || el.hidden || this.peekLink !== at.link) return;
    el.classList.add("cite");
    this.peekCard(list, at.link);
    this.peekPlace(at);
  }

  /** A label as the .aux writes it, as it reads: ties as spaces, groups' braces gone, \& and the like as their characters. */
  private static texText(s: string): string {
    return s.replace(/\\([&%$#_{}])/g, "$1").replace(/~/g, "\u00a0").replace(/\\(?:protect|relax)\s*/g, "").replace(/[{}]/g, "").trim();
  }

  /**
   * The citation's links: the hyperref citation links (their destination `cite.KEY`, the
   * frame's 7th element) inside the brackets round `link` on page `k`, as linkText finds
   * them, in order; `link` alone where it has no brackets or no such neighbours.
   */
  private citeGroup(k: number, link: Link): Link[] {
    const dest = (l: Link) => (l as unknown as unknown[])[6];
    const isCite = (l: Link) => typeof dest(l) === "string" && (dest(l) as string).startsWith("cite.");
    const span = this.bracketSpan(k, link);
    const links = this.slots[k]?.img?.links ?? [];
    if (!span || !isCite(link)) return [link];
    const mid = (link[1] + link[3]) / 2;
    const group = links.filter((l) => isCite(l) && l[1] <= mid && mid <= l[3] && (l[0] + l[2]) / 2 >= span[0] && (l[0] + l[2]) / 2 <= span[1]);
    return group.length ? group.sort((a, b) => a[0] - b[0]) : [link];
  }

  /** The brackets round a citation's link, as page points from left to right ([x0, x1]), or null (none). */
  private bracketSpan(k: number, link: Link): [number, number] | null {
    const s = this.slots[k];
    if (!s) return null;
    const b = s.el.getBoundingClientRect(), sx = b.width / this.size.w, sy = b.height / this.size.h;
    const [l, t, r, bt] = [b.left + link[0] * sx, b.top + link[1] * sy, b.left + link[2] * sx, b.top + link[3] * sy];
    for (const el of s.el.querySelectorAll<SVGTextElement>("text")) {
      const tr = el.getBoundingClientRect();
      if (tr.right < l || tr.left > r || tr.bottom < t || tr.top > bt) continue;
      const m = el.getScreenCTM(), text = el.textContent ?? "";
      if (!m) continue;
      const n = Math.min(el.getNumberOfChars(), text.length);
      const px = (i: number) => new DOMPoint(el.getExtentOfChar(i).x, 0).matrixTransform(m).x;
      let a = -1, z = -1;
      for (let i = 0; i < n; i++) {
        const e = el.getExtentOfChar(i), p = new DOMPoint(e.x + e.width / 2, e.y + e.height / 2).matrixTransform(m);
        if (p.x >= l && p.x <= r && p.y >= t && p.y <= bt) (a < 0 && (a = i)), (z = i);
      }
      if (a < 0) continue;
      // (out over what a citation's brackets hold: numbers, names, years, separators; to the brackets)
      let x = a, y = z;
      while (x > 0 && !"[(".includes(text[x - 1]) && !"])".includes(text[x - 1])) x--;
      while (y < n - 1 && !"])".includes(text[y + 1]) && !"[(".includes(text[y + 1])) y++;
      if (x === 0 || y === n - 1 || !"[(".includes(text[x - 1]) || !"])".includes(text[y + 1])) return null;
      const e = el.getExtentOfChar(y + 1);
      const x1 = new DOMPoint(e.x + e.width, 0).matrixTransform(m).x;
      return [(px(x - 1) - b.left) / sx, (x1 - b.left) / sx];
    }
    return null;
  }

  /** The engine's citation as cards: each entry read where it is typeset; null while one of their pages is not drawn (asked for). */
  private citedFrom(said: CiteAnswer[]): Cited[] | null {
    const out: Cited[] = [];
    let waiting = false;
    for (const c of said) {
      const t = c.at ? this.slots[c.at[0]] : undefined;
      if (c.at && t && !t.img) {
        this.ask(c.at[0]);
        waiting = true;
        continue;
      }
      const e = c.at ? this.entryOn(c.at[0], c.at[1]) : null;
      // (no entry read there: the key and its label, still a card)
      // (the label as the page prints it, where the entry has one ([1] in a numbered list):
      // what the reader sees; else the .aux's (natbib's "Knuth and Plass (1981)" for every style))
      const aux = c.label != null ? Viewer.texText(c.label) : null, label = e?.label || aux;
      out.push(e ? { ...Viewer.asCited(label ?? "", e.text, [c.at![0], e.top]), key: c.key, label: label ?? "" } : { key: c.key, label: label ?? c.key, title: c.key, authors: [], at: c.at ? [c.at[0], Math.max(0, c.at[1] - 10)] : undefined });
    }
    return waiting ? null : out;
  }

  /**
   * The bibliography's entry the engine places at `y` on page `k`: its label, its text, its
   * line's top. `y` is where the entry's link goes, its destination's top, which hyperref
   * raises above the entry's first baseline (\Hy@raisedlink, \HyperRaiseLinkDefault): the
   * entry is the first text line whose baseline is at or below it. Read as the baseline and
   * matched to the line around it, the place was the line above's, the previous entry's:
   * every card one entry early, the first none.
   */
  private entryOn(k: number, y: number): { label: string; text: string; top: number } | null {
    const lines = this.pageLines(k);
    let i = -1;
    lines.forEach((l, j) => l.b >= y && (i < 0 || l.b < lines[i].b || (l.b === lines[i].b && l.x0 < lines[i].x0)) && (i = j));
    if (i < 0) return null;
    const first = lines[i], out = [first.t], lead = first.y1 - first.y0;
    for (let j = i + 1, last = first; j < lines.length && out.length < 8; j++) {
      const l = lines[j];
      // (the next entry: a label, a gap, a hanging indent's return to the margin)
      if (Viewer.LABEL.test(l.t) || l.y0 - last.y1 > lead * 0.9 || /^\d+$/.test(l.t) || (Math.abs(l.x0 - first.x0) < 1 && !Viewer.LABEL.test(first.t))) break;
      out.push(l.t);
      last = l;
    }
    const text = out.reduce((a, l) => (/[a-z]-$/.test(a) && /^[a-z]/.test(l) ? a.slice(0, -1) + l : `${a} ${l}`));
    const m = Viewer.LABEL.exec(text);
    return { label: m ? m[1] : "", text: m ? text.slice(m[0].length) : text, top: first.y0 };
  }

  private citedOnPage(k: number, link: Link): Cited[] | null {
    if (typeof link[4] !== "number") return null;
    // (the target page not drawn yet: its text unknown; asked for, the window shown again when it comes (set))
    const t = this.slots[link[4]];
    if (t && !t.img) return void this.ask(link[4]), null;
    const own = this.entryAt(link[4], link[5] ?? 0);
    if (!own) return null;
    const near = link[4];
    const group = this.linkText(k, link);
    const inner = /^[[(](.*)[\])]$/.exec(group)?.[1] ?? "";
    // (a numeric citation: every number, a range's too)
    if (/^[\d,;\s–-]+$/.test(inner)) {
      const labels: string[] = [];
      for (const part of inner.split(/[,;]/)) {
        const r = /^\s*(\d+)\s*[–-]\s*(\d+)\s*$/.exec(part);
        if (r) for (let n = Number(r[1]); n <= Number(r[2]) && labels.length < 200; n++) labels.push(String(n));
        else if (part.trim()) labels.push(part.trim());
      }
      const out: Cited[] = [];
      for (const n of labels) {
        const at = n === own.label ? ([near, link[5] ?? 0] as [number, number]) : this.findLabel(n, near);
        const e = at && this.entryAt(at[0], at[1]);
        if (at && e) out.push(Viewer.asCited(e.label || n, e.text, at));
      }
      return out.length ? out : [Viewer.asCited(own.label, own.text, [near, link[5] ?? 0])];
    }
    // (else: the links on this page inside the citation's brackets, each its entry)
    const s = this.slots[k];
    const img = s?.img;
    const list: Cited[] = [];
    if (img?.links && group.length > 3) {
      const line = img.links.filter((l) => Math.abs((l[1] + l[3]) / 2 - (link[1] + link[3]) / 2) < 3 && Math.abs(l[0] - link[0]) < 200 && typeof l[4] === "number");
      for (const l of line.sort((a, b) => a[0] - b[0])) {
        const e = this.entryAt(l[4] as number, l[5] ?? 0);
        if (e && !list.some((c) => c.at![0] === l[4] && c.at![1] === (l[5] ?? 0))) list.push(Viewer.asCited(e.label, e.text, [l[4] as number, l[5] ?? 0]));
      }
    }
    return list.length ? list : [Viewer.asCited(own.label, own.text, [near, link[5] ?? 0])];
  }

  /** The list's entry `i` followed: its place in the bibliography (Back returns). */
  private peekGo(i: number, e?: MouseEvent, st: PeekState = { link: this.peekLink, from: this.peekFrom, list: this.peekList }): void {
    const c = st.list[i], { link, from } = st;
    if (c?.at && e && this.newTab(e, c.at[0], c.at[1])) return;
    // (from a pinned window: it stays; the hover window goes)
    if (st.list === this.peekList) this.peekHide();
    if (c?.at) this.follow(c.at[0], c.at[1], `[${c.label}] ${c.title}`, link ? { k: from, link } : undefined);
  }

  /** A card's button: its address opened (the host's, as a link's), its BibTeX copied. */
  private peekAct(b: HTMLElement, e?: MouseEvent, st: PeekState = { link: this.peekLink, from: this.peekFrom, list: this.peekList }): void {
    if (b.dataset.act === "go") return this.peekGo(Number(b.dataset.i), e, st);
    if (b.dataset.act === "open") {
      const u = b.dataset.u!;
      return this.host.link ? this.host.link(u) : void window.open(u, "_blank", "noopener");
    }
    const i = b.dataset.i;
    const bib = (i === "all" ? st.list : i ? [st.list[Number(i)]] : st.list).map((c) => c?.bibtex).filter(Boolean).join("\n\n");
    void navigator.clipboard?.writeText(bib).then(() => {
      const was = b.textContent;
      b.textContent = "Copied";
      setTimeout(() => (b.textContent = was), 1200);
    });
  }

  /** The window gone. */
  private peekHide(): void {
    clearTimeout(this.peekTimer);
    clearTimeout(this.peekHiding);
    this.peekAt = null;
    this.peekLink = null;
    if (this.peekEl) this.peekEl.hidden = true;
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
  // ---- Back and forward: a link followed, where the view was is kept, as a browser's history ----

  /** Where the view was before each jump, the latest last (Back's), and where Back left (Forward's, the next last). */
  private past: Stop[] = [];
  private future: Stop[] = [];
  private backEl: HTMLElement | null = null;

  /** Where the view is now: its top's page and how far down it, and the first words of the line there (the list says them). */
  private here(): Stop | null {
    const v = this.scroller.getBoundingClientRect();
    const at = this.pointAt(v.left + v.width / 2, v.top + 12);
    if (!at) return null;
    // (the text layer's line nearest the view's top: what was being read)
    let words = "", far = Infinity;
    const y = v.top + 12;
    for (const t of this.slots[at.k]?.el.querySelectorAll("text") ?? []) {
      const r = t.getBoundingClientRect();
      if (r.bottom < y - 4 || !t.textContent?.trim()) continue;
      const d = Math.abs(r.top - y);
      if (d < far) (far = d), (words = t.textContent.trim());
      if (r.top > y + 120) break;
    }
    return { k: at.k, fy: at.fy, words: words.length > 48 ? `${words.slice(0, 47)}…` : words };
  }

  /** The view at `t`. */
  private toStop(t: Stop): void {
    const s = this.slots[t.k];
    if (s) this.scroller.scrollTo({ top: s.el.offsetTop + t.fy * s.el.offsetHeight - 12, behavior: "auto" });
  }

  /** A link followed: where the view is now kept (Back returns to it; Forward's forgotten, as a browser's), then there. */
  follow(k: number, top: number | null, via = "", from?: { k: number; link: Link }): void {
    const now = this.backs ? this.here() : null;
    if (now) {
      now.via = via;
      if (from) now.mark = { k: from.k, box: [from.link[0], from.link[1], from.link[2], from.link[3]] };
      this.past.push(now);
      this.future = [];
      // (an entry of the tab's history each jump, n: the jumps before it. The
      // entry the page was at is marked first, 0, so a back to it is told)
      if (this.host.backWith === "history") {
        const h = this.scroller.ownerDocument.defaultView!.history;
        if ((h.state as { phitexBack?: number } | null)?.phitexBack === undefined) h.replaceState({ ...(h.state ?? {}), phitexBack: this.hAt }, "");
        h.pushState({ phitexBack: ++this.hAt }, "");
      }
    }
    this.peekHide();
    this.goToPlace(k, top);
    this.flash(k, top);
    this.backShown();
  }

  /**
   * Where a jump landed, lit for a moment: the text layer's lines at
   * `top` on page `k` (an entry, an equation, a caption's first lines),
   * else a band there; a soft accent fading in 1.5 s. A tab in the
   * background (a Ctrl+click's) lights it once it is shown, not unseen.
   */
  private flash(k: number, top: number | null, exact?: [number, number, number, number]): void {
    if (!this.flashes) return;
    const doc = this.scroller.ownerDocument;
    if (doc.visibilityState !== "visible") {
      const go = () => doc.visibilityState === "visible" && (doc.removeEventListener("visibilitychange", go), this.flash(k, top, exact));
      return void doc.addEventListener("visibilitychange", go);
    }
    const s = this.slots[k];
    if (!s) return;
    s.el.querySelector(".phx-flash")?.remove();
    const y0 = top ?? 0, b = s.el.getBoundingClientRect(), sy = this.size.h / b.height, sx = this.size.w / b.width;
    // (the lines that start within 30 points under the target, the first with any text: their box, in points)
    let box: [number, number, number, number] | null = exact ?? null;
    for (const t of exact ? [] : s.el.querySelectorAll("text")) {
      if (!t.textContent?.trim()) continue;
      const r = t.getBoundingClientRect(), ty = (r.top - b.top) * sy, tb = (r.bottom - b.top) * sy;
      if (tb < y0 - 2 || ty > y0 + 30) continue;
      const [l, rr] = [(r.left - b.left) * sx, (r.right - b.left) * sx];
      box = box ? [Math.min(box[0], l), Math.min(box[1], ty), Math.max(box[2], rr), Math.max(box[3], tb)] : [l, ty, rr, tb];
    }
    const [l, t, r, bt] = box ?? [this.size.w * 0.1, y0, this.size.w * 0.9, y0 + 24];
    const f = document.createElement("div");
    f.className = "phx-flash";
    const pad = exact ? 2 : 3;
    Object.assign(f.style, { left: `${((l - pad) / this.size.w) * 100}%`, top: `${((t - pad) / this.size.h) * 100}%`, width: `${((r - l + 2 * pad) / this.size.w) * 100}%`, height: `${((bt - t + 2 * pad) / this.size.h) * 100}%` });
    f.addEventListener("animationend", () => f.remove());
    s.el.append(f);
  }

  /** The tab's history entry the view is at (history hosts): each jump's one past the last. */
  private hAt = 0;

  /** One step back (-1) or forward (1): where the view is goes to the other side. */
  private step(d: -1 | 1): void {
    const [from, to] = d < 0 ? [this.past, this.future] : [this.future, this.past];
    const t = from.pop();
    if (!t) return;
    const now = this.here();
    if (now) to.push(now);
    this.toStop(t);
    // (where it lands, lit: the link clicked there to leave it, else the line at the place)
    if (t.mark) this.flash(t.mark.k, null, t.mark.box);
    else this.flash(t.k, t.fy * this.size.h);
    this.backShown();
  }

  /** `n` steps back (negative) or forward: through the tab's history where there is one (popstate then steps, so the two stay one). */
  private go(n: number): void {
    if (!n) return;
    if (this.host.backWith === "history") return this.scroller.ownerDocument.defaultView!.history.go(n);
    for (let i = 0; i < Math.abs(n); i++) this.step(n < 0 ? -1 : 1);
  }

  /**
   * A click that asks for a new tab (Ctrl, ⌘ on a Mac, or the middle
   * button: `middle`) where the host has one: page `k` opened there, this
   * view left as it is, the window too (another can be opened from it).
   */
  private newTab(e: MouseEvent, k: number, top: number | null, middle = false): boolean {
    if (!this.host.openAt || !this.newTabs || !(middle || e.ctrlKey || e.metaKey)) return false;
    e.preventDefault();
    this.host.openAt(k, top);
    return true;
  }

  /** Where a view opened at a place starts (page, points down it): gone to once that page is drawn and the pages have their size. */
  private startPlace: { k: number; top: number | null } | null = null;

  /** Start at page `k`, `top` points down it (a tab opened by a Ctrl+click): now if that page is drawn, else once it is. */
  startAt(k: number, top: number | null): void {
    this.startPlace = { k, top };
    this.startIfReady();
  }

  private startIfReady(): void {
    const p = this.startPlace;
    if (!p || !this.slots[p.k] || !this.slots.some((s) => s.img)) return;
    this.startPlace = null;
    // (there, and lit once this tab is shown: opened in the background, not before)
    requestAnimationFrame(() => (this.goToPlace(p.k, p.top), requestAnimationFrame(() => this.flash(p.k, p.top))));
  }

  /** Back where the view was before the last link followed. */
  back(): boolean {
    if (!this.past.length) return false;
    this.go(-1);
    return true;
  }

  /** Forward where Back left. */
  forward(): boolean {
    if (!this.future.length) return false;
    this.go(1);
    return true;
  }

  /**
   * The Back pill over the pages' corner while there is history: Back (to
   * the page it says), the earlier stops' list (its count: each a click to
   * go straight there), Forward where Back left one, × to forget them.
   */
  /** Back and Forward turned off: what they kept forgotten, the pill gone. */
  backsOff(): void {
    this.past = [];
    this.future = [];
    this.backShown();
  }

  private backShown(): void {
    if (!this.backEl) {
      const b = (this.backEl = document.createElement("div"));
      b.className = "pback";
      b.innerHTML = `<button type="button" class="go"><span class="ar">←</span><span class="t"></span></button><button type="button" class="more" aria-haspopup="menu"></button><button type="button" class="fw" aria-label="Forward">→</button><button type="button" class="x" aria-label="Forget">×</button><div class="stops" role="menu" hidden></div>`;
      b.querySelector(".go")!.addEventListener("click", () => this.back());
      b.querySelector(".fw")!.addEventListener("click", () => this.forward());
      b.querySelector(".x")!.addEventListener("click", () => ((this.past = []), (this.future = []), this.backShown()));
      const list = b.querySelector<HTMLElement>(".stops")!;
      b.querySelector(".more")!.addEventListener("click", () => {
        list.hidden = !list.hidden;
        if (list.hidden) return;
        // (most recent first: the n-th, n steps back)
        list.innerHTML = this.past
          .map((t, i) => [t, this.past.length - i] as const)
          .reverse()
          .map(([, n]) => `<button type="button" role="menuitem" data-n="${n}"><b></b><span></span></button>`)
          .join("");
        // (each: what was clicked there, then where it was, page and line)
        list.querySelectorAll<HTMLElement>("[data-n]").forEach((e, i) => {
          const t = this.past[this.past.length - 1 - i];
          e.querySelector("b")!.textContent = t.via ? `Clicked ${t.via}` : `Page ${t.k + 1}`;
          e.querySelector("span")!.textContent = [`page ${t.k + 1}`, t.words && `“${t.words}”`].filter(Boolean).join(" · ");
        });
      });
      list.addEventListener("click", (e) => {
        const n = (e.target as Element).closest<HTMLElement>("[data-n]")?.dataset.n;
        if (!n) return;
        list.hidden = true;
        this.go(-Number(n));
      });
      b.ownerDocument.addEventListener("pointerdown", (e) => !e.composedPath().includes(b) && (list.hidden = true));
      (this.scroller.parentElement ?? this.scroller).append(b);
    }
    const b = this.backEl, t = this.past.at(-1);
    b.hidden = !t && !this.future.length;
    b.classList.toggle("noback", !t);
    b.querySelector<HTMLElement>(".stops")!.hidden = true;
    b.querySelector<HTMLElement>(".fw")!.hidden = !this.future.length;
    const more = b.querySelector<HTMLElement>(".more")!;
    more.hidden = this.past.length < 2;
    more.textContent = `+${this.past.length - 1} ▾`;
    more.title = "Every place before, to go straight back to one";
    b.querySelector(".t")!.textContent = t ? `Back to page ${t.k + 1}` : "Back";
    b.querySelector<HTMLElement>(".go")!.title = this.host.backWith ? "Back (the mouse's back button, Alt+←)" : "Back";
    b.querySelector<HTMLElement>(".fw")!.title = this.host.backWith ? "Forward (the mouse's forward button, Alt+→)" : "Forward";
  }

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
  others(marks: { k: number; boxes: [number, number, number, number][]; label: string; hue: number; file?: string; pos?: number }[]): void {
    for (const m of this.root.querySelectorAll(".peer")) m.remove();
    const { w, h } = this.size;
    const at = (m: HTMLElement, x: number, y: number) => Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%` });
    for (const { k, boxes, label, hue, file, pos } of marks) {
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
      // (a click on it: the editor to their cursor, as Overleaf's avatar does)
      if (file !== undefined && pos !== undefined) Object.assign(tag.dataset, { file, pos: String(pos) }), (tag.title = `Go to ${label}'s cursor`);
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

  /**
   * The review's marks (review.ts): each its `boxes` on page `k` as
   * `.rv.CLS` (`data-id` its note's or hunk's), and with `pin` a button at
   * the page's right edge beside its first box (a note's: its thread a click
   * away). The whole set, the last one's replaced; never scrolls, never asks
   * for a page (one drawn later keeps them, as paint does).
   */
  /** A page's size in PDF points (the pages' boxes are in them). */
  pageSize(): { w: number; h: number } {
    return this.size;
  }

  reviewMarks(marks: { k: number; boxes: [number, number, number, number][]; cls: string; id: string; pin?: string; title?: string }[]): void {
    for (const m of this.root.querySelectorAll(".rv")) m.remove();
    const { w, h } = this.size;
    const pinned = new Map<number, number[]>();
    for (const mk of marks) {
      const s = this.slots[mk.k];
      if (!s || !mk.boxes.length) continue;
      for (const [x, y, bw, bh] of mk.boxes) {
        const m = document.createElement("div");
        m.className = `rv ${mk.cls}`;
        m.dataset.id = mk.id;
        Object.assign(m.style, { left: `${(x / w) * 100}%`, top: `${(y / h) * 100}%`, width: `${(Math.max(bw, 1.5) / w) * 100}%`, height: `${(bh / h) * 100}%` });
        s.el.append(m);
      }
      if (mk.pin !== undefined) {
        // (pins on one page kept apart: one under the other where they meet)
        const ys = pinned.get(mk.k) ?? [];
        let y = mk.boxes[0][1];
        while (ys.some((o) => Math.abs(o - y) < 13)) y += 13;
        ys.push(y);
        pinned.set(mk.k, ys);
        const b = document.createElement("button");
        b.type = "button";
        b.className = `rv rv-pin ${mk.cls}`;
        b.dataset.id = mk.id;
        b.textContent = mk.pin;
        if (mk.title) (b.title = mk.title), b.setAttribute("aria-label", mk.title);
        Object.assign(b.style, { top: `${(y / h) * 100}%` });
        s.el.append(b);
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
      const y = marginScroll(top, s.el.offsetTop + (Math.max(...boxes.map(([, by, , bh]) => by + bh)) / h) * s.el.offsetHeight, v.scrollTop, v.clientHeight, this.margin);
      if (y !== null) to.top = y;
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

  /** The pages streamed drawn without some of what they draw with, by hash (a preview's mark). */
  private previews: ReadonlySet<string> = new Set();

  /**
   * The pages there are now, by hash: slots added or removed, changed ones drawn again when seen;
   * those of `previews` marked as previews (streamed, drawn without some of what they draw with).
   */
  layout(hashes: string[], previews?: ReadonlySet<string>): void {
    // (a new build's: the engine's answers were the last one's)
    this.citesKnown.clear();
    this.previews = previews ?? new Set();
    while (this.slots.length > hashes.length) {
      const s = this.slots.pop()!;
      this.io.unobserve(s.el);
      s.el.remove();
    }
    for (let k = 0; k < hashes.length; k++) this.waits(Object.assign(this.slotAt(k), { hash: hashes[k] }));
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
    this.waits(Object.assign(this.slotAt(k), { hash }));
    this.wantVisible();
  }

  /**
   * A page the build has not shipped alone (no hash yet: a streamed page
   * after it came first) says it comes at the build's end, not a blank
   * sheet (a page the engine could not write alone: there are few). A page
   * streamed without some of what it draws with (an object made at the
   * document's end: pgf's transparency, shadings, patterns) says it is a
   * preview, until the build's page replaces it.
   */
  private waits(s: Slot): void {
    s.el.classList.toggle("later", !s.hash && !s.img);
    s.el.classList.toggle("preview", !!s.hash && this.previews.has(s.hash));
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
      this.waits(s);
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
    this.waits(s);
    // (a tab opened at a place: there once the pages have their size)
    if (this.startPlace) queueMicrotask(() => this.startIfReady());
    // (the page a link's window waits for, or shows: shown again with it, a
    // citation's cards now readable from its text, a page view drawn)
    const pl = this.peekShown;
    if (pl && pl[4] === k) queueMicrotask(() => this.peekShown === pl && this.peekShow({ link: pl, k: this.peekFrom }));
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
      // (the boxes snap needs read together, then written: one layout, not one a page)
      const vis = [...this.visible].map((k) => this.slots[k]).filter((s): s is Slot => !!s);
      const boxes = vis.map((s) => (s.img?.canvas?.dataset.scale ? undefined : s.el.getBoundingClientRect()));
      vis.forEach((s, i) => this.snap(s, boxes[i]));
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
    const marks = [...s.el.querySelectorAll(".mark, .peer, .culprit, .rv")];
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
    if (this.wasp) waspOn(c, this.waspAll, this.waspLen);
  }

  // ---- The reading aid (wasp.ts): each line of the text coloured by a gradient, alternating ----

  private wasp = false;
  private waspAll = false;
  private waspLen = 1;

  /** The reading aid on or off, `all` every line or only the body's (the host's setting): the pages drawn so far given it or rid of it, those drawn later as they come. */
  waspShown(on: boolean, all = false, length = 1): void {
    if (on === this.wasp && all === this.waspAll) {
      // (the fade's length alone: the bands' ink stops moved, nothing drawn again)
      if (length !== this.waspLen && on) waspLength(this.root, length);
      this.waspLen = length;
      return;
    }
    [this.wasp, this.waspAll, this.waspLen] = [on, all, length];
    for (const s of this.slots) if (s.img) (on ? waspOn(s.img.canvas, all, length) : waspOff(s.img.canvas));
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
    const pw = Number(wrap.dataset.pw) || c.width, ph = Number(wrap.dataset.ph) || c.height;
    // (drawn at the zoom, `data-scale`: at its own pixels, its box the
    // slot's to a pixel by pageBox's rounding; drawn at another, or a
    // host's bitmap of no scale more than a rounding off its box: stretched.
    // The box read only for a bitmap of no scale: read after the slots'
    // sizes were written, it laid the view out again, each frame of a zoom
    // once a page)
    const r = wrap.dataset.scale ? null : (box ?? s.el.getBoundingClientRect());
    const own = !r ? this.fits(s) : Math.abs(pw - r.width * dpr) <= 1.5 && Math.abs(ph - r.height * dpr) <= 1.5;
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
    // (a page the build has not shipped (no hash: a streamed page after it
    // came first) has nothing to draw: asked, it failed, and was asked again
    // at every zoom's frame, each a turn of the core's mid-build, ahead of
    // the pages it can draw (the TikZ manual zoomed while streaming: 62 of
    // 94 asks failed). Asked once its hash comes (shipped, layout))
    if (!s || !s.hash) return;
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

/**
 * Where to scroll so that a highlight from `top` to `bottom` (pixels down the
 * pages) sits in the band `margin` (a share of `height`) away from the view's
 * edges, the view at `at`; null: it is in the band, no scroll. A highlight taller
 * than the band keeps its top in it. `margin` 0: only once it leaves the view, and
 * then to the middle (the behaviour before the margin).
 */
export function marginScroll(top: number, bottom: number, at: number, height: number, margin: number): number | null {
  if (!(margin > 0)) return top < at + 20 || top > at + height - 40 ? top - height / 2 : null;
  const m = Math.min(0.45, margin) * height;
  if (top < at + m) return top - m;
  if (bottom > at + height - m) return Math.min(bottom - (height - m), top - m);
  return null;
}
