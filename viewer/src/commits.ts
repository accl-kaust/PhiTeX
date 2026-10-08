// A commit graph to choose two versions from, for the diff (DESIGN 4.10):
// lanes as `git log --graph` draws them, merges, branch and tag pills, the
// working tree on top. A click chooses From, a shift-click To (the working
// tree: the files as they are), the range between them marked; a
// double-click a commit against its first parent (for a merge, what it
// brought). Self-contained: rendered into the element given, the commits
// fetched through `load`, the pair chosen handed to `pick`. No dependency
// on the viewer.

/** A commit, newest first in topological order (`phitex watch`'s `diff_log` op gives them). */
export interface Commit {
  id: string;
  short: string;
  /** Its parents, the first first. */
  parents: string[];
  author: string;
  /** Seconds since the epoch. */
  time: number;
  subject: string;
  refs: { name: string; kind: "head" | "branch" | "remote" | "tag" }[];
}

/** A row of the graph: the commit's lane, and the lines from it to the next row ([from lane, to lane]). */
export interface Row {
  lane: number;
  down: [number, number][];
}

/**
 * The graph's lanes: each commit in the lane that waits for it (else the
 * first free one), its first parent kept in its lane, its other parents in
 * lanes of their own, or joining a lane that waits for them already.
 * `width`: lanes used at most.
 */
export function layout(cs: { id: string; parents: string[] }[]): { rows: Row[]; width: number } {
  const active: (string | null)[] = [];
  const rows: Row[] = [];
  // (the lines going down from the last row: [from lane, slot])
  let pending: [number, number][] = [];
  let width = 1;
  for (const c of cs) {
    let lane = active.indexOf(c.id);
    if (lane < 0) {
      lane = active.indexOf(null);
      if (lane < 0) lane = active.push(null) - 1;
    }
    // (the lines into this row end at the commit if they waited for it)
    if (rows.length) rows[rows.length - 1].down = pending.map(([x, s]) => [x, active[s] === c.id ? lane : s]);
    for (let s = 0; s < active.length; s++) if (active[s] === c.id) active[s] = null;
    // (the lanes passing by this row, and those its parents go down in)
    const by = active.map((id) => id !== null);
    const from = new Map<number, number>();
    c.parents.forEach((p, i) => {
      let s = active.indexOf(p);
      if (s < 0) {
        s = i === 0 && active[lane] === null ? lane : active.indexOf(null);
        if (s < 0) s = active.push(null) - 1;
        active[s] = p;
      }
      from.set(s, lane);
    });
    while (active.length && active[active.length - 1] === null) active.pop();
    pending = [];
    active.forEach((id, s) => {
      if (id === null) return;
      if (by[s]) pending.push([s, s]);
      // (a parent: from the commit, into its lane or one waiting for it already)
      const f = from.get(s);
      if (f !== undefined && !(by[s] && f === s)) pending.push([f, s]);
    });
    width = Math.max(width, lane + 1, active.length);
    rows.push({ lane, down: [] });
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

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

const ROW = 24;
const LANE = 12;
const COLORS = ["#3e63dd", "#e5484d", "#30a46c", "#f5a524", "#8e4ec6", "#12a594", "#d6409f", "#978365"];

export const COMMITS_CSS = `
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

/** What a commit graph needs: the commits (or why there are none), and what to do with a pair chosen. */
export interface CommitsHost {
  /** Commits `skip`.. (at most `limit`), or an error to show. */
  load(skip: number, limit: number): Promise<Commit[] | string>;
  /** Diff `to` (null: the working tree) against `from` (commit ids). */
  pick(from: string, to: string | null): void;
}

/** A commit graph in `root`. */
export class Commits {
  private el: HTMLElement;
  private host: CommitsHost;
  private commits: Commit[] = [];
  private more = true;
  private loading = false;
  private error = "";
  /** The pair chosen (ids; `to` null: the working tree). */
  private from: string | null = null;
  private to: string | null = null;
  private page = 200;

  constructor(root: HTMLElement, host: CommitsHost) {
    this.host = host;
    this.el = document.createElement("div");
    this.el.className = "phx-commits";
    root.append(this.el);
    this.el.addEventListener("click", (e) => this.click(e as MouseEvent));
    this.el.addEventListener("dblclick", (e) => {
      const c = this.at(e.target as HTMLElement);
      // (a commit against its first parent: a merge's, what it brought)
      if (c && c.parents.length) this.choose(c.parents[0], c.id);
    });
    this.render();
  }

  get element(): HTMLElement {
    return this.el;
  }

  /** Mark the pair shown (as the host has it: a revision, a commit id or its prefix; `to` null: the working tree). */
  mark(from: string | null, to: string | null): void {
    this.from = from;
    this.to = to;
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
      this.commits.push(...got);
      this.more = got.length === this.page;
    }
    this.render();
  }

  /** The history changed (a commit made, a branch moved): read again. */
  reload(): Promise<void> {
    this.commits = [];
    this.more = true;
    this.error = "";
    return this.load();
  }

  private choose(from: string, to: string | null): void {
    this.from = from;
    this.to = to;
    this.render();
    this.host.pick(from, to);
  }

  private at(t: HTMLElement): Commit | undefined {
    const row = t.closest<HTMLElement>(".row[data-i]");
    return row ? this.commits[Number(row.dataset.i)] : undefined;
  }

  /** The row of revision `rev` (a commit id or its prefix, a ref's name, HEAD), else -1. */
  private find(rev: string | null): number {
    if (!rev) return -1;
    if (rev === "HEAD") return this.commits.findIndex((c) => c.refs.some((r) => r.kind === "head"));
    return this.commits.findIndex((c) => c.id.startsWith(rev) || c.refs.some((r) => r.name === rev));
  }

  private click(e: MouseEvent): void {
    const t = e.target as HTMLElement;
    if (t.closest("[data-more]")) return void this.load();
    const row = t.closest<HTMLElement>(".row");
    if (!row || e.detail > 1) return;
    const c = this.at(t);
    if (e.shiftKey) {
      // (To: this commit or the working tree; From stays if it is older, else To's first parent)
      if (!c) return this.choose(this.from ?? "HEAD", null);
      const fi = this.find(this.from), ti = this.commits.indexOf(c);
      const from = fi > ti ? this.commits[fi].id : (c.parents[0] ?? c.id);
      return this.choose(from, c.id);
    }
    // (From: this commit; To stays if it is newer, else the working tree)
    if (!c) return;
    const ti = this.find(this.to), fi = this.commits.indexOf(c);
    this.choose(c.id, this.to && ti >= 0 && ti < fi ? this.commits[ti].id : null);
  }

  private render(): void {
    const scroll = this.el.scrollTop;
    this.el.innerHTML = this.html();
    this.el.scrollTop = scroll;
  }

  private html(): string {
    if (this.error) return `<div class="note err">${esc(this.error)}</div>`;
    if (!this.commits.length) return `<div class="note">${this.loading ? "reading the history…" : "no commits"}</div>`;
    const { rows, width } = layout(this.commits);
    const W = (width + 1) * LANE;
    const x = (l: number) => LANE / 2 + l * LANE + 2;
    const fi = this.find(this.from);
    const ti = this.to ? this.find(this.to) : -1;
    const tree = this.from !== null && !this.to;
    // (the working tree, above all: the range from To down to From marked)
    const top = `<div class="row${tree ? " to" : ""}" title="Shift-click: diff the working tree"><svg width="${W}" height="${ROW}"><circle cx="${x(0)}" cy="${ROW / 2}" r="4" fill="none" stroke="#888" stroke-dasharray="2 2"/><line x1="${x(0)}" y1="${ROW / 2 + 4}" x2="${x(0)}" y2="${ROW * 1.5}" stroke="#888" stroke-dasharray="2 2"/></svg><span class="txt">${tree ? '<span class="end">TO</span>' : ""}<i>the working tree</i></span></div>`;
    const lines = this.commits.map((c, i) => {
      const r = rows[i];
      const col = COLORS[r.lane % COLORS.length];
      // (the lines down to the next row, over its top half)
      const svg = r.down.map(([a, b]) => `<path d="M${x(a)} ${ROW / 2} C${x(a)} ${ROW} ${x(b)} ${ROW} ${x(b)} ${ROW * 1.5}" stroke="${COLORS[b % COLORS.length]}" fill="none" stroke-width="1.6"/>`);
      svg.push(c.parents.length > 1 ? `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4.5" fill="Canvas" stroke="${col}" stroke-width="2"/>` : `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4" fill="${col}"/>`);
      const pills = c.refs.map((f) => `<span class="pill ${f.kind}">${esc(f.name)}</span>`).join("");
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? '<span class="end">FROM</span>' : i === ti ? '<span class="end">TO</span>' : "";
      const title = `${c.subject}\n${c.author}, ${new Date(c.time * 1000).toLocaleString()}\nclick: From · shift-click: To · double-click: against its first parent`;
      return `<div class="row${cls}" data-i="${i}" title="${esc(title)}"><svg width="${W}" height="${ROW}">${svg.join("")}</svg><span class="txt">${end}<span class="sha">${esc(c.short)}</span>${pills}${c.parents.length > 1 ? "<i>merge:</i> " : ""}${esc(c.subject)}<span class="who">${esc(c.author)}, ${ago(c.time)}</span></span></div>`;
    });
    const more = this.more ? `<div class="note"><button type="button" data-more>${this.loading ? "reading…" : "more…"}</button></div>` : "";
    return top + lines.join("") + more;
  }
}
