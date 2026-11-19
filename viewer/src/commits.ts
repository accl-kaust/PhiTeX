// The history to choose two versions from, for the diff (Changes' picker,
// changes.ts): lanes as `git log --graph` draws them, coloured by branch
// (main's first-parent chain one colour, each branch merged into it its
// own), a commit's first parent kept in its lane; merges, branch and tag
// pills, the working tree on top. Overleaf's history is one lane of edit
// sessions, its labels the tags.
//
// Choosing, as an airline's dates are: a click sets one end, the rows up to
// the pointer then marked by a bar at their edge (aim), the second click the
// other end, the older becoming From; the same commit again cancels. A drag
// marks its rows filled and sets the range. Shift-click sets To. A
// double-click compares a commit with its first parent and stays in the
// picker. Opened from To, From is the first end already. Self-contained:
// rendered into the element given, the commits fetched through `load`, the
// pair chosen handed to `pick`. No dependency on the viewer.

import { html, svg } from "./html.ts";

/** A commit (Overleaf: an edit session), newest first in topological order (`phitex watch`'s `diff_log` op gives them). */
export interface Commit {
  id: string;
  short: string;
  /** Its parents, the first first. */
  parents: string[];
  author: string;
  /** The author's email, where the host knows it (an author's colour is by it). */
  email?: string;
  /** Seconds since the epoch. */
  time: number;
  subject: string;
  refs: { name: string; kind: "head" | "branch" | "remote" | "tag" }[];
  /** A note beside it ("your last visit"). */
  mark?: string;
}

/** A row of the graph: the commit's lane and colour, and the lines from it to the next row ([from lane, to lane, colour]). */
export interface Row {
  lane: number;
  color: number;
  down: [number, number, number][];
}

/**
 * Each commit's branch colour: 0 for the first-parent chain from the
 * newest commit (main's, the head's), then each other branch its own, in
 * the order their tips come (a tip's first-parent chain takes its colour
 * until it meets a commit coloured already).
 */
export function branches(cs: { id: string; parents: string[] }[]): Map<string, number> {
  const by = new Map(cs.map((c) => [c.id, c]));
  const out = new Map<string, number>();
  let next = 0;
  for (const c of cs) {
    if (out.has(c.id)) continue;
    const k = next++;
    for (let h: string | undefined = c.id; h && by.has(h) && !out.has(h); h = by.get(h)!.parents[0]) out.set(h, k);
  }
  return out;
}

/**
 * The graph's lanes: each commit in the lane that waits for it (else the
 * first free one); its first parent kept in its lane (a lane already
 * waiting for that parent further right bends into it); its other parents
 * in lanes of their own, or joining a lane that waits for them already.
 * Lines are coloured by branch: an edge between two commits of main
 * main's colour, else the branch's. `width`: lanes used at most.
 */
export function layout(cs: { id: string; parents: string[] }[]): { rows: Row[]; width: number } {
  const col = branches(cs);
  const colOf = (id: string) => col.get(id) ?? 0;
  const edge = (a: string, b: string) => (colOf(a) === 0 && colOf(b) === 0 ? 0 : colOf(a) !== 0 ? colOf(a) : colOf(b));
  const active: (string | null)[] = [];
  const slotCol: (number | undefined)[] = [];
  const rows: Row[] = [];
  // (the lines going down from the last row: [from lane, slot, colour])
  let pending: [number, number, number][] = [];
  let width = 1;
  for (const c of cs) {
    let lane = active.indexOf(c.id);
    if (lane < 0) {
      lane = active.indexOf(null);
      if (lane < 0) lane = active.push(null) - 1;
    }
    // (the lines into this row end at the commit if they waited for it)
    if (rows.length) rows[rows.length - 1].down = pending.map(([x, s, k]) => [x, active[s] === c.id ? lane : s, k]);
    for (let s = 0; s < active.length; s++) if (active[s] === c.id) (active[s] = null), (slotCol[s] = undefined);
    // (the lanes passing by this row, and those its parents go down in)
    const by = active.map((id) => id !== null);
    const from = new Map<number, [number, number]>();
    // (a slot whose waiter moves left into this commit's lane: its line bends there)
    const moved = new Map<number, [number, number]>();
    c.parents.forEach((p, i) => {
      let s = active.indexOf(p);
      if (s > lane && i === 0 && active[lane] === null) {
        const k = slotCol[s] ?? edge(c.id, p);
        active[s] = null;
        slotCol[s] = undefined;
        moved.set(s, [lane, k]);
        s = lane;
        active[s] = p;
        slotCol[s] = edge(c.id, p);
      }
      if (s < 0) {
        s = i === 0 && active[lane] === null ? lane : active.indexOf(null);
        if (s < 0) s = active.push(null) - 1;
        active[s] = p;
        slotCol[s] = edge(c.id, p);
      }
      from.set(s, [lane, edge(c.id, p)]);
    });
    pending = [];
    moved.forEach(([to, k], s) => {
      if (by[s]) pending.push([s, to, k]);
    });
    while (active.length && active[active.length - 1] === null) active.pop();
    active.forEach((id, s) => {
      if (id === null) return;
      if (by[s] && !moved.has(s)) pending.push([s, s, slotCol[s] ?? 0]);
      // (a parent: from the commit, into its lane or one waiting for it already)
      const f = from.get(s);
      if (f !== undefined && !(by[s] && f[0] === s && !moved.has(s))) pending.push([f[0], s, f[1]]);
    });
    width = Math.max(width, lane + 1, active.length);
    rows.push({ lane, color: colOf(c.id), down: [] });
  }
  if (rows.length) rows[rows.length - 1].down = pending;
  return { rows, width };
}

/** How long ago `t` (seconds since the epoch) was. */
export function ago(t: number, now = Date.now() / 1000): string {
  const s = Math.max(0, now - t);
  const units: [number, string][] = [[31536000, "year"], [2592000, "month"], [86400, "day"], [3600, "hour"], [60, "minute"]];
  for (const [n, u] of units)
    if (s >= n) {
      const k = Math.floor(s / n);
      return `${k} ${u}${k === 1 ? "" : "s"} ago`;
    }
  return "just now";
}

/** The merge that brought commit `id` into main's first-parent chain, and the branch's name (its merge's subject, "Merge branch 'x'"; else a ref on the branch). */
export function mergedVia(cs: Commit[], id: string): { merge: Commit; branch: string } | null {
  const by = new Map(cs.map((c) => [c.id, c]));
  const main = new Set<string>();
  for (let h: string | undefined = cs[0]?.id; h && by.has(h); h = by.get(h)!.parents[0]) main.add(h);
  if (main.has(id) || !by.has(id)) return null;
  // (each merge on main, newest first: the first whose other parents reach `id` without main)
  for (const m of cs) {
    if (!main.has(m.id) || m.parents.length < 2) continue;
    const seen = new Set<string>();
    const todo = m.parents.slice(1);
    let found = false;
    while (todo.length && !found) {
      const h = todo.pop()!;
      if (seen.has(h) || main.has(h) || !by.has(h)) continue;
      seen.add(h);
      if (h === id) found = true;
      todo.push(...by.get(h)!.parents);
    }
    if (!found) continue;
    const named = /Merge (?:remote-tracking )?branch '([^']+)'/.exec(m.subject)?.[1] ?? /Merge pull request #\d+ from (\S+)/.exec(m.subject)?.[1];
    const ref = [...seen].flatMap((h) => by.get(h)!.refs).find((r) => r.kind === "branch" || r.kind === "remote")?.name;
    return { merge: m, branch: named ?? ref ?? "a branch" };
  }
  return null;
}

const ROW = 40;
const LANE = 16;
/** Branch colours: main's, then the branches'. */
export const BRANCH_COLORS = ["var(--phc-g0, #2f6fd6)", "var(--phc-g1, #c2410c)", "var(--phc-g2, #0f8a6a)", "var(--phc-g3, #9333ea)", "var(--phc-g4, #b5700f)", "var(--phc-g5, #be185d)"];
const bc = (k: number) => BRANCH_COLORS[k % BRANCH_COLORS.length];

export const COMMITS_CSS = `
.phx-commits { overflow: auto; user-select: none; -webkit-user-select: none; padding: 2px 0 8px; }
.phx-commits .row { display: flex; align-items: center; height: ${ROW}px; cursor: pointer; position: relative; transition: background .18s; }
.phx-commits .row:hover { background: var(--phc-chip); }
.phx-commits .row.inrange, .phx-commits .row.from, .phx-commits .row.to, .phx-commits .row.drag { background: var(--phc-sel); }
.phx-commits .row::before { content: ""; position: absolute; left: 0; top: 0; bottom: 0; width: 3px; background: var(--phc-aim); transform: scaleY(0); transform-origin: top; transition: transform .18s; }
.phx-commits .row.aim::before { transform: none; }
.phx-commits .row.new { animation: phc-in .2s ease-out both; }
@keyframes phc-in { from { opacity: 0; transform: translateY(-4px); } }
.phx-commits svg { flex: none; overflow: visible; }
.phx-commits .t { min-width: 0; flex: 1; display: grid; padding-right: 10px; }
.phx-commits .s { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.phx-commits .s i { color: var(--phc-muted); font-style: normal; }
.phx-commits .m { display: flex; gap: 6px; align-items: center; font-size: 11.5px; color: var(--phc-muted); white-space: nowrap; overflow: hidden; }
.phx-commits code { font: 500 11px ui-monospace, "DM Mono", monospace; }
.phx-commits .endtag { font-weight: 700; font-size: 9.5px; letter-spacing: .06em; color: #fff; background: var(--phc-focus); border-radius: 4px; padding: 1px 5px; margin-right: 6px; vertical-align: 1px; }
.phx-commits .ref { font: 600 10.5px ui-monospace, monospace; border-radius: 5px; padding: 0 5px; border: 1px solid var(--phc-line); }
.phx-commits .ref.tag { color: #b5700f; border-color: currentColor; }
.phx-commits .ref.head { background: var(--phc-fg); color: var(--phc-bg); border-color: var(--phc-fg); }
.phx-commits .ref.branch, .phx-commits .ref.remote { color: var(--phc-bcol); border-color: currentColor; }
.phx-commits .ref.mark { color: var(--phc-muted); }
.phx-commits .av { width: 16px; height: 16px; border-radius: 50%; display: inline-grid; place-items: center; font: 700 8px system-ui, sans-serif; color: #fff; flex: none; }
.phx-commits .note { padding: 10px 14px; color: var(--phc-muted); }
.phx-commits .note.err { color: var(--phc-danger, #c23a2f); }
.phx-commits .more { display: block; margin: 6px auto; border: 1px solid var(--phc-line); background: none; color: inherit; border-radius: 999px; padding: 2px 12px; cursor: pointer; font: inherit; font-size: 12px; }
.phx-commits .skel { cursor: default; }
.phx-commits .skel:hover { background: none; }
.phx-commits .skel .bone { display: block; height: 9px; border-radius: 4px; background: var(--phc-chip2); margin: 3px 0; }
.phx-commits .skel circle { fill: var(--phc-chip2); }
.phx-commits .skel .bone, .phx-commits .skel svg { animation: phc-shimmer 1.2s ease-in-out infinite; }
@keyframes phc-shimmer { 50% { opacity: .45; } }
@media (prefers-reduced-motion: reduce) { .phx-commits .skel .bone, .phx-commits .skel svg, .phx-commits .row.new { animation: none; } .phx-commits .row, .phx-commits .row::before { transition: none; } }
`;

/** The ref kinds a pill may be (its class). */
const KINDS = new Set(["head", "branch", "remote", "tag"]);

/** Two letters for a name's avatar. */
export const initials = (n: string) => (n.trim().split(/\s+/).map((w) => w[0] ?? "").join("").slice(0, 2) || "?").toUpperCase();

/** What a commit graph needs: the commits (or why there are none), and what to do with a pair chosen. */
export interface CommitsHost {
  /** Commits `skip`.. (at most `limit`), or an error to show. */
  load(skip: number, limit: number): Promise<Commit[] | string>;
  /**
   * A pair chosen (ids; `to` null: the working tree). `done`: a range
   * completed (two clicks, a drag, a shift-click): the picker closes;
   * false: a double-click's one commit, the picker staying.
   */
  pick(from: string, to: string | null, done: boolean): void;
  /** The pair marked so far, a first end clicked among them (a click's From, To the working tree till the second). */
  marked?(from: string | null, to: string | null): void;
  /** An author's colour (their avatar). */
  color?(c: Commit): string;
  /** The working tree's row; null: none. */
  tree?: { title: string; detail: string } | null;
  /** The host's word for a commit and whether it has hashes ("version", none: Overleaf). */
  words?: { commit?: string; hashes?: boolean };
}

/** A commit graph in `root`. */
export class Commits {
  private el: HTMLElement;
  private host: CommitsHost;
  commits: Commit[] = [];
  private more = true;
  private loading = false;
  private error = "";
  /** The pair chosen (ids; `to` null: the working tree). */
  private from: string | null = null;
  private to: string | null = null;
  /** A range's first end, clicked (a row; -1 the working tree), the second not yet: the rows to the pointer marked as it moves. */
  private anchor: number | null = null;
  private page = 200;
  /** Rows drawn before (the new ones slide in). */
  private seen = new Set<string>();
  private listeners: (() => void)[] = [];

  constructor(root: HTMLElement, host: CommitsHost) {
    this.host = host;
    this.el = document.createElement("div");
    this.el.className = "phx-commits";
    root.append(this.el);
    this.el.addEventListener("click", (e) => this.click(e as MouseEvent));
    // (a drag from one row to another: the range between them, the older end From, the newer To)
    this.el.addEventListener("pointerdown", (e) => this.dragFrom(e));
    // (a first end clicked: the rows from it to the pointer marked, as an airline's dates are)
    this.el.addEventListener("pointermove", (e) => {
      if (this.anchor === null || this.drag) return;
      const over = this.rowAt(e.clientX, e.clientY);
      if (over !== null) this.paintDrag({ from: this.anchor, over, moved: true }, "aim");
    });
    this.el.addEventListener("pointerleave", () => this.anchor !== null && !this.drag && this.paintDrag(null, "aim"));
    this.el.addEventListener("dblclick", (e) => {
      const c = this.at(e.target as HTMLElement);
      // (a commit against its first parent (a merge's: what it brought); the picker stays)
      if (!c || !c.parents.length) return;
      this.anchor = null;
      this.paintDrag();
      this.choose(c.parents[0], c.id, false);
    });
    this.render();
  }

  get element(): HTMLElement {
    return this.el;
  }

  /** Called whenever the commits change (a page read). */
  onChange(f: () => void): void {
    this.listeners.push(f);
  }

  /** Mark the pair shown (as the host has it: a commit id or its prefix, a ref's name, HEAD; `to` null: the working tree). */
  mark(from: string | null, to: string | null): void {
    this.from = from;
    this.to = to;
    this.render();
  }

  /**
   * The picker opened from From or To: from To, From is the first end
   * already (the next click is To); from From, nothing is (the next click
   * one end, then the other).
   */
  open(which: "from" | "to"): void {
    const fi = this.find(this.from);
    this.anchor = which === "to" && fi >= 0 ? fi : null;
    this.paintDrag(null, "aim");
    this.render();
  }

  /** Whether a first end is clicked, the second not yet. */
  get aiming(): boolean {
    return this.anchor !== null;
  }

  /** The first end let go (Esc). */
  cancel(): void {
    this.anchor = null;
    this.paintDrag(null, "aim");
    this.render();
  }

  /** The commits: the first page now, more as asked. */
  async load(): Promise<void> {
    if (this.loading || !this.more) return;
    this.loading = true;
    this.render();
    const got = await this.host.load(this.commits.length, this.page);
    this.loading = false;
    if (typeof got === "string") {
      this.error = got;
      this.more = false;
    } else {
      const have = new Set(this.commits.map((c) => c.id));
      this.commits.push(...got.filter((c) => !have.has(c.id)));
      // (a host may answer with less than a page as it reads (Overleaf's sessions, a page of its history at a
      // time): more till it answers none)
      this.more = got.length > 0;
    }
    this.render();
    for (const f of this.listeners) f();
  }

  /** The history changed (a commit made, a branch moved): read again. */
  reload(): Promise<void> {
    this.commits = [];
    this.more = true;
    this.error = "";
    return this.load();
  }

  /** Whether more commits can be read. */
  get hasMore(): boolean {
    return this.more;
  }

  /** Whether a page is being read. */
  get busy(): boolean {
    return this.loading;
  }

  private choose(from: string | null, to: string | null, done: boolean): void {
    this.from = from;
    this.to = to;
    this.render();
    this.host.marked?.(from, to);
    if (from) this.host.pick(from, to, done);
  }

  /** The range between two rows (-1 the working tree), whichever order: the older end From, the newer To. */
  private range(a: number, b: number): void {
    const [newer, older] = a < b ? [a, b] : [b, a];
    if (older < 0) return;
    this.choose(this.commits[older].id, newer < 0 ? null : this.commits[newer].id, true);
  }

  private at(t: HTMLElement): Commit | undefined {
    const row = t.closest<HTMLElement>(".row[data-i]");
    return row ? this.commits[Number(row.dataset.i)] : undefined;
  }

  /** The row of revision `rev` (a commit id or its prefix, a ref's name, HEAD), else -1. */
  find(rev: string | null): number {
    if (!rev) return -1;
    if (rev === "HEAD") return this.commits.findIndex((c) => c.refs.some((r) => r.kind === "head"));
    return this.commits.findIndex((c) => c.id === rev || c.id.startsWith(rev) || c.refs.some((r) => r.name === rev));
  }

  /** A drag across the rows: where it began (a row's index; -1 the working tree), and whether it left that row. */
  private drag: { from: number; over: number; moved: boolean } | null = null;
  /** A drag just chose: the click that ends it is not a click. */
  private dragged = false;

  /** The row under a point: a commit's index, -1 the working tree's, null none. */
  private rowAt(x: number, y: number): number | null {
    const root = this.el.getRootNode() as Document | ShadowRoot;
    const row = (root.elementFromPoint?.(x, y) ?? document.elementFromPoint(x, y))?.closest<HTMLElement>(".row:not(.skel)");
    if (!row || !this.el.contains(row)) return null;
    return row.dataset.i === undefined ? -1 : Number(row.dataset.i);
  }

  private dragFrom(e: PointerEvent): void {
    if (e.button !== 0 || e.shiftKey) return;
    const at = this.rowAt(e.clientX, e.clientY);
    if (at === null) return;
    // (no text selection begun)
    e.preventDefault();
    this.drag = { from: at, over: at, moved: false };
    const move = (m: PointerEvent) => {
      const over = this.rowAt(m.clientX, m.clientY);
      if (!this.drag || over === null || over === this.drag.over) return;
      this.drag.over = over;
      // (held once it leaves its row, not before: a captured pointer's click goes to the list, not the row clicked)
      if (!this.drag.moved && over !== this.drag.from) this.el.setPointerCapture?.(m.pointerId);
      this.drag.moved ||= over !== this.drag.from;
      this.paintDrag();
    };
    const up = () => {
      this.el.removeEventListener("pointermove", move);
      this.el.removeEventListener("pointerup", up);
      const d = this.drag;
      this.drag = null;
      this.paintDrag();
      if (!d?.moved) return;
      // (the click a drag ends with, if any, is not a click: dropped, now or never)
      this.dragged = true;
      setTimeout(() => (this.dragged = false), 0);
      this.anchor = null;
      this.range(d.from, d.over);
    };
    this.el.addEventListener("pointermove", move);
    this.el.addEventListener("pointerup", up);
  }

  /**
   * The rows a drag spans, filled as it goes (`drag`), or those a second
   * click would choose, from the first end to the pointer (`aim`: a bar at
   * their edge only, growing as the pointer moves).
   */
  private paintDrag(d = this.drag, kind: "drag" | "aim" = "drag"): void {
    const [a, b] = d ? [Math.min(d.from, d.over), Math.max(d.from, d.over)] : [0, -2];
    for (const r of this.el.querySelectorAll<HTMLElement>(".row:not(.skel)")) {
      const i = r.dataset.i === undefined ? -1 : Number(r.dataset.i);
      const on = !!d?.moved && i >= a && i <= b;
      r.classList.toggle("drag", kind === "drag" && on);
      r.classList.toggle("aim", kind === "aim" && on);
    }
  }

  private click(e: MouseEvent): void {
    if (this.dragged) return void (this.dragged = false);
    const t = e.target as HTMLElement;
    if (t.closest("[data-more]")) return void this.load();
    const row = t.closest<HTMLElement>(".row:not(.skel)");
    if (!row || e.detail > 1) return;
    const c = this.at(t), i = c ? this.commits.indexOf(c) : -1;
    if (e.shiftKey) {
      // (To: this commit or the working tree; From stays if it is older, else To's first parent)
      this.anchor = null;
      if (!c) return this.choose(this.from ?? this.commits[0]?.id ?? null, null, true);
      const fi = this.find(this.from);
      return this.choose(fi > i ? this.commits[fi].id : (c.parents[0] ?? c.id), c.id, true);
    }
    // (the first click one end (From, to the working tree till the second); the same again cancels)
    if (this.anchor === null || this.anchor === i) {
      this.anchor = this.anchor === i ? null : i;
      if (this.anchor !== null && c) {
        this.from = c.id;
        this.to = null;
        this.host.marked?.(this.from, null);
      }
      this.render();
      this.paintDrag(null, "aim");
      return;
    }
    const a = this.anchor;
    this.anchor = null;
    this.paintDrag(null, "aim");
    this.range(a, i);
  }

  private seenMore: IntersectionObserver | null = null;

  render(): void {
    const scroll = this.el.scrollTop;
    this.el.replaceChildren(this.html());
    this.el.scrollTop = scroll;
    // (the older ones read as the list's end comes into view: the list fills as it is scrolled, never a page to ask for)
    this.seenMore?.disconnect();
    const more = this.el.querySelector("[data-more]");
    if (more && typeof IntersectionObserver === "function") {
      this.seenMore = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && void this.load());
      this.seenMore.observe(more);
    }
  }

  /** Rows shaped like the real ones while the first page is read: a lane's dot, an avatar, two lines. */
  static skeleton(n = 6): DocumentFragment {
    return html`${Array.from({ length: n }, (_, i) => html`<div class="row skel" aria-hidden="true"><svg width="${2 * LANE}" height="${ROW}"><circle cx="${LANE / 2 + 4}" cy="${ROW / 2}" r="5"/></svg><span class="t"><span class="bone" style="${`width:${62 - ((i * 17) % 30)}%`}"></span><span class="bone" style="${`width:${38 - ((i * 11) % 14)}%`}"></span></span></div>`)}`;
  }

  private html(): DocumentFragment {
    if (this.error) return html`<div class="note err">${this.error}</div>`;
    if (!this.commits.length) return this.loading || this.more ? Commits.skeleton() : html`<div class="note">${this.host.words?.commit === "version" ? "No history yet: the project's versions show here once Overleaf has saved them" : "No commits in this repository yet"}</div>`;
    const { rows, width } = layout(this.commits);
    const W = (width + 1) * LANE;
    const x = (l: number) => LANE / 2 + l * LANE + 4;
    const fi = this.find(this.from);
    const ti = this.to ? this.find(this.to) : -1;
    const tree = this.from !== null && !this.to;
    const color = (c: Commit) => this.host.color?.(c) ?? "#8a96a3";
    const hashes = this.host.words?.hashes !== false;
    const t = this.host.tree === undefined ? { title: "Working tree", detail: "The files as they are, not committed" } : this.host.tree;
    // (the working tree, above all: the range from To down to From marked)
    const top = t
      ? html`<div class="${`row${tree ? " to" : ""}`}" title="${t.detail}"><svg width="${W}" height="${ROW}"><circle cx="${x(0)}" cy="${ROW / 2}" r="5" fill="none" stroke="currentColor" stroke-opacity=".5" stroke-dasharray="2 2"/><line x1="${x(0)}" y1="${ROW / 2 + 5}" x2="${x(0)}" y2="${ROW * 1.5}" stroke="currentColor" stroke-opacity=".5" stroke-dasharray="2 3" stroke-width="1.6"/></svg><span class="t"><span class="s">${tree ? html`<span class="endtag">TO</span>` : ""}<i>${t.title}</i></span><span class="m">${t.detail}</span></span></div>`
      : "";
    const fresh = this.seen.size > 0;
    const lines = this.commits.map((c, i) => {
      const r = rows[i];
      const col = bc(r.color);
      const marks = r.down.map(([a, b, k]) => svg`<path d="${`M${x(a)} ${ROW / 2} C${x(a)} ${ROW} ${x(b)} ${ROW} ${x(b)} ${ROW * 1.5}`}" stroke="${bc(k)}" fill="none" stroke-width="2"/>`);
      marks.push(c.parents.length > 1 ? svg`<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="5.5" fill="var(--phc-bg, Canvas)" stroke="${col}" stroke-width="2.4"/>` : svg`<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="5" fill="${col}"/>`);
      const pills = c.refs.map((f) => html`<span class="${`ref ${KINDS.has(f.kind) ? f.kind : "branch"}`}" style="${`--phc-bcol:${col}`}">${f.name}</span>`);
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? html`<span class="endtag">FROM</span>` : i === ti ? html`<span class="endtag">TO</span>` : "";
      const isNew = fresh && !this.seen.has(c.id);
      const title = `${c.subject}\n${c.author}, ${new Date(c.time * 1000).toLocaleString()}\nclick: one end · shift-click: To · double-click: against its first parent`;
      return html`<div class="${`row${cls}${isNew ? " new" : ""}`}" data-i="${i}" data-id="${c.id}" title="${title}"><svg width="${W}" height="${ROW}">${marks}</svg><span class="t"><span class="s">${end}${c.parents.length > 1 ? html`<i>merge:</i> ` : ""}${c.subject}</span><span class="m">${hashes ? html`<code>${c.short}</code>` : ""}${pills}<span class="av" style="${`background:${color(c)}`}">${initials(c.author)}</span>${c.author} · ${ago(c.time)}${c.mark ? html` · <span class="ref mark">${c.mark}</span>` : ""}</span></span></div>`;
    });
    for (const c of this.commits) this.seen.add(c.id);
    const more = this.loading ? Commits.skeleton(2) : this.more ? html`<button type="button" class="more" data-more>${`Older ${this.host.words?.commit === "version" ? "versions" : "commits"}…`}</button>` : "";
    return html`${top}${lines}${more}`;
  }
}
