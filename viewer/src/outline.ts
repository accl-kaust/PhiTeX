// The document's contents beside the pages, from its PDF outline (its
// bookmarks: what hyperref makes of \chapter, \section and the rest): a
// title clicked goes to its place, the section being read is marked as
// the pages scroll, and a part folds. Host-neutral: the host gives the
// items (phitex-draw's `outline()`, the `outline` op's) and the viewer.

/** An outline item: its title, its page (from 0) and top in points (null: none), its children. */
export interface Entry {
  t: string;
  p: number | null;
  y: number | null;
  k: Entry[];
}

export interface Place {
  /** Scroll to page `k`, `top` points from its top. */
  goToPlace(k: number, top: number | null): void;
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export class Outline {
  private el: HTMLElement;
  /** The items in document order, flat, with their rows. */
  private flat: { e: Entry; row: HTMLElement }[] = [];
  private marked: HTMLElement | null = null;
  /** Folded parts, by their titles' path: kept when a build changes the outline. */
  private folded = new Set<string>();

  constructor(
    root: HTMLElement,
    private place: Place,
  ) {
    this.el = document.createElement("nav");
    this.el.className = "phx-outline";
    this.el.setAttribute("aria-label", "Contents");
    root.append(this.el);
    this.el.addEventListener("click", (ev) => {
      const t = ev.target as HTMLElement;
      const li = t.closest<HTMLElement>("li");
      if (!li) return;
      ev.preventDefault();
      if (t.closest(".phx-fold")) {
        const key = li.dataset.path!;
        if (li.classList.toggle("folded")) this.folded.add(key);
        else this.folded.delete(key);
        return;
      }
      const i = Number(li.dataset.i);
      const e = this.flat[i]?.e;
      if (e && e.p != null) this.place.goToPlace(e.p, e.y);
    });
  }

  get element(): HTMLElement {
    return this.el;
  }

  /** A build's outline. */
  set(items: Entry[]): void {
    this.flat = [];
    this.marked = null;
    if (!items.length) {
      this.el.innerHTML = `<p class="phx-none">No contents: the PDF has no outline (hyperref makes one from the sections).</p>`;
      return;
    }
    const list = (es: Entry[], path: string, depth: number): string =>
      `<ul>${es
        .map((e) => {
          const i = this.flat.length;
          this.flat.push({ e, row: null as unknown as HTMLElement });
          const p = `${path}/${e.t}`;
          const kids = e.k.length ? list(e.k, p, depth + 1) : "";
          const cls = [e.k.length ? "parent" : "", this.folded.has(p) ? "folded" : ""].filter(Boolean).join(" ");
          return `<li data-i="${i}" data-path="${esc(p)}" class="${cls}"><div class="phx-row" style="--d:${depth}">${e.k.length ? `<span class="phx-fold" aria-hidden="true"></span>` : `<span class="phx-dot"></span>`}<a href="#" class="phx-t" title="${esc(e.t)}">${esc(e.t)}</a>${e.p != null ? `<span class="phx-p">${e.p + 1}</span>` : ""}</div>${kids}</li>`;
        })
        .join("")}</ul>`;
    this.el.innerHTML = list(items, "", 0);
    for (const li of this.el.querySelectorAll<HTMLElement>("li")) {
      const f = this.flat[Number(li.dataset.i)];
      if (f) f.row = li.querySelector(":scope > .phx-row")!;
    }
  }

  /** The page in view: the last item that starts on it or before is the one being read. */
  at(page: number): void {
    let best: HTMLElement | null = null;
    for (const { e, row } of this.flat) {
      if (e.p == null) continue;
      if (e.p > page) break;
      best = row;
    }
    if (best === this.marked) return;
    this.marked?.classList.remove("here");
    best?.classList.add("here");
    this.marked = best;
    // (kept in sight, unless a fold hides it)
    if (best && best.offsetParent) {
      const r = best.getBoundingClientRect(), v = this.el.getBoundingClientRect();
      if (r.top < v.top || r.bottom > v.bottom) best.scrollIntoView({ block: "nearest" });
    }
  }
}

/** The sidebar's CSS, in the host's tokens where it has them (--ink, --dim, --bar). */
export const OUTLINE_CSS = `
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
.phx-fold::before { content: "▾"; }
li.folded > .phx-row .phx-fold::before { content: "▸"; }
.phx-dot { cursor: default; }
.phx-none { color: var(--dim, #777); padding: 8px 16px; margin: 0; }
`;
