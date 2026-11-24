// A folder watch's file manager (`phitex watch DIR`): the folder's files and
// folders, one folder at a time, as a file manager shows them; a document
// opened from it in a tab of its own, or in place. Drawn full size on the
// index page (crates/cli/viewer/dir.ts) and as the viewer's Files column
// (crates/cli/viewer/cli.ts); quick open (Ctrl+P) over both. What it shows
// comes from the watch over a WebSocket (FolderLink); folders.ts orders and
// words it. Every rule reads the --ph-* tokens (print.ts), so it looks as
// the viewer does, light or dark.

import { html } from "./html.ts";
import { counts, crumbs, filterEntries, rank, runs, sizeOf, sortEntries, stateOf, up, when, whenFull, type FileEntry, type Folder, type RunningDoc, type SortKey } from "./folders.ts";

/** Material icons (Apache-2.0), inline. */
const ICONS: Record<string, string> = {
  folder: "M10 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2h-8l-2-2z",
  tex: "M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
  other: "M6 2c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6H6zm7 7V3.5L18.5 9H13z",
  pdf: "M20 2H8c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-8.5 7.5c0 .83-.67 1.5-1.5 1.5H9v2H7.5V7H10c.83 0 1.5.67 1.5 1.5v1zm5 2c0 .83-.67 1.5-1.5 1.5h-2.5V7H15c.83 0 1.5.67 1.5 1.5v3zm4-3H19v1h1.5V11H19v2h-1.5V7h3v1.5zM9 9.5h1v-1H9v1zM4 6H2v14c0 1.1.9 2 2 2h14v-2H4V6zm10 5.5h1v-3h-1v3z",
  image: "M21 19V5c0-1.1-.9-2-2-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2zM8.5 13.5l2.5 3.01L14.5 12l4.5 6H5l3.5-4.5z",
  open_in_new: "M19 19H5V5h7V3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2v-7h-2v7zM14 3v2h3.59l-9.83 9.83 1.41 1.41L19 6.41V10h2V3h-7z",
  up: "M4 12l1.41 1.41L11 7.83V20h2V7.83l5.58 5.59L20 12l-8-8-8 8z",
  search: "M15.5 14h-.79l-.28-.27C15.41 12.59 16 11.11 16 9.5 16 5.91 13.09 3 9.5 3S3 5.91 3 9.5 5.91 16 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z",
  close: "M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  chevron: "M10 6 8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z",
  asc: "M7 14l5-5 5 5z",
  desc: "M7 10l5 5 5-5z",
  folder_open: "M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z",
};

/** Icon `name` (ICONS), `size` px, in the text's colour. */
export function fileIcon(name: string, size = 18): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const s = document.createElementNS(ns, "svg");
  s.setAttribute("viewBox", "0 0 24 24");
  s.setAttribute("width", String(size));
  s.setAttribute("height", String(size));
  s.setAttribute("aria-hidden", "true");
  s.setAttribute("focusable", "false");
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", ICONS[name] ?? ICONS.other);
  s.append(p);
  return s;
}

/** A document of quick open's list. */
export interface Found {
  path: string;
  id: string;
}

/**
 * The watch's file manager over its WebSocket (`ws` on the index's
 * address): `hello` first (the folder's name, and the document of the
 * viewer asking), then each folder asked for (`list`), sent again as it
 * changes; quick open's documents (`find`). Joined again when it drops,
 * the folder shown asked for again.
 */
export class FolderLink {
  private ws?: WebSocket;
  private wait = 400;
  private dir: string | null = null;
  private finds: ((f: Found[]) => void)[] = [];
  /** Whether it is joined now; whether it ever was. */
  up = false;
  everUp = false;
  onHello: (h: { root: string; where?: string; doc: { path: string; id: string; dir: string } | null }) => void = () => {};
  onFolder: (f: Folder) => void = () => {};
  onError: (message: string, dir: string) => void = () => {};
  onState: (up: boolean) => void = () => {};

  private url: URL;

  constructor(url: URL) {
    this.url = url;
    this.open();
  }

  private open(): void {
    const ws = (this.ws = new WebSocket(this.url));
    ws.onopen = () => {
      this.up = this.everUp = true;
      this.wait = 400;
      this.onState(true);
      if (this.dir !== null) this.send({ op: "list", dir: this.dir });
      if (this.finds.length) this.send({ op: "find" });
    };
    ws.onmessage = (m) => {
      let e: any;
      try {
        e = JSON.parse(String(m.data));
      } catch {
        return;
      }
      if (e.event === "hello") this.onHello(e);
      else if (e.event === "folder") this.onFolder(e as Folder);
      else if (e.event === "error") this.onError(String(e.message ?? "refused"), String(e.dir ?? ""));
      else if (e.event === "found") for (const f of this.finds.splice(0)) f(e.files as Found[]);
    };
    ws.onclose = () => {
      const was = this.up;
      this.up = false;
      if (was || !this.everUp) this.onState(false);
      // (the watch restarted, or not yet there: joined again, more slowly while it stays away)
      setTimeout(() => this.open(), this.wait);
      this.wait = Math.min(this.wait * 2, 3000);
    };
  }

  private send(op: object): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(op));
  }

  /** Folder `dir` (from the root), sent as it is and again as it changes. */
  list(dir: string): void {
    this.dir = dir;
    this.send({ op: "list", dir });
  }

  /** Document `path`'s session stopped (its memory freed); opened again, it starts again. */
  stop(path: string): void {
    this.send({ op: "stop", path });
  }

  /** Every running document's session stopped. */
  stopAll(): void {
    this.send({ op: "stop-all" });
  }

  /** Every document of the folder (quick open). */
  find(): Promise<Found[]> {
    return new Promise((ok) => {
      this.finds.push(ok);
      if (this.finds.length === 1) this.send({ op: "find" });
    });
  }
}

/** Where the file manager is shown, and what opening does there. */
export interface FilesHost {
  /** A document's address, from this page (its path in it: a tab left open finds it in the next watch). */
  docHref(id: string, path: string): string;
  /** A raw file's address (a PDF, a picture), from this page. */
  rawHref(path: string): string;
  /** A folder's address, from this page (the index at that folder). */
  dirHref(path: string): string;
  /** The document this tab shows (a viewer), marked. */
  current?: string;
  /** The Files column (a viewer's): narrow, no column heads. */
  compact?: boolean;
  /** Quick open asked for (its button). */
  quickOpen(): void;
  /** Where the sort is remembered (localStorage), per place. */
  prefsKey: string;
}

const isMac = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform ?? "");
const MOD = isMac ? "⌘" : "Ctrl";

/** Whether a click or key asks for a new tab: Ctrl or ⌘ (or Shift) held, the middle button. */
const wantsTab = (e: MouseEvent | KeyboardEvent) => e.ctrlKey || e.metaKey || e.shiftKey || ("button" in e && e.button === 1);

/** Text, `hits` (indexes into it from `from`) bold. */
function marked(s: string, hits: readonly number[], from = 0): DocumentFragment {
  const f = document.createDocumentFragment();
  for (const [t, on] of runs(s, hits, from)) {
    if (on) {
      const b = document.createElement("b");
      b.textContent = t;
      f.append(b);
    } else f.append(t);
  }
  return f;
}

/** A document's state, as a small badge (null: not opened this run). */
function badge(e: FileEntry): HTMLElement | null {
  const s = stateOf(e.doc);
  if (!s) return null;
  const b = document.createElement("span");
  b.className = `fm-badge tone-${s.tone}${e.doc && e.doc.tabs > 0 ? " shown" : ""}${e.doc && !e.doc.live ? " ended" : ""}`;
  b.title = s.title;
  b.append(document.createElement("i"), s.text);
  return b;
}

/**
 * The file manager: a folder's breadcrumb, filter, sort and list; folders
 * entered with a double click or Enter (one click in the Files column),
 * Backspace or Alt+↑ back up, never above the folder watched. A document
 * opens on a click (a new tab with Ctrl, ⌘, Shift or the middle button);
 * a PDF or a picture in a tab as it is; other files are shown, muted.
 */
export class FileList {
  readonly el: HTMLElement;
  private list: HTMLElement;
  private filter: HTMLInputElement;
  private crumbsEl: HTMLElement;
  private note: HTMLElement;
  private empty: HTMLElement;
  private foot: HTMLElement;
  private upBtn: HTMLButtonElement;
  private head: HTMLElement;
  private sortBtn: HTMLButtonElement;
  private folder: Folder | null = null;
  private shown: FileEntry[] = [];
  private sel: string | null = null;
  private by: SortKey = "name";
  private desc = false;
  private root = "";
  /** The folder to select once its parent is shown (going up). */
  private from: string | null = null;
  private lost = false;
  /** The folder asked for, until it comes. */
  dir = "";
  /** The folder shown changed (the page keeps it in its address). */
  onDir: (dir: string) => void = () => {};

  private host: FilesHost;
  private link: FolderLink;

  constructor(host: FilesHost, link: FolderLink, start = "") {
    this.host = host;
    this.link = link;
    const compact = !!host.compact;
    this.loadPrefs();
    this.el = document.createElement("div");
    this.el.className = `fm${compact ? " compact" : ""}`;
    this.el.append(html`<div class="fm-bar">
  <button type="button" class="fm-ib fm-up" title="Up a folder (Backspace)" aria-label="Up a folder">${fileIcon("up", 18)}</button>
  <nav class="fm-crumbs" aria-label="Folder"></nav>
  <span class="fm-grow"></span>
  <label class="fm-filter">${fileIcon("search", 16)}<input type="search" placeholder="Filter" aria-label="Filter this folder" spellcheck="false" autocomplete="off"></label>
  <button type="button" class="fm-ib fm-sort" title="Sort" aria-label="Sort"></button>
</div>
<div class="fm-note" role="status" hidden></div>
<section class="fm-run" aria-label="Running" hidden><header><b>Running</b><span class="fm-run-n"></span><span class="fm-grow"></span><button type="button" class="fm-btn fm-stopall" title="Stop every document: their sessions end, their memory is freed">Stop all</button></header><div class="fm-run-list"></div></section>
<div class="fm-head" role="presentation">
  <span class="fm-c-icon"></span>
  <button type="button" class="fm-h" data-sort="name">Name<span class="fm-dir"></span></button>
  <span class="fm-h fm-c-state">Status</span>
  <button type="button" class="fm-h fm-c-time" data-sort="mtime">Modified<span class="fm-dir"></span></button>
  <span class="fm-h fm-c-size">Size</span>
</div>
<div class="fm-list" aria-label="Files"></div>
<div class="fm-empty" hidden></div>
<div class="fm-foot"><span class="fm-count"></span><span class="fm-grow"></span><button type="button" class="fm-qo" title="Find a document by its name (${MOD}+P)">${fileIcon("search", 15)}<span>Find a document</span><kbd>${MOD}</kbd><kbd>P</kbd></button></div>`);
    const $ = <T extends HTMLElement>(s: string) => this.el.querySelector(s) as T;
    this.list = $(".fm-list");
    this.filter = $(".fm-filter input");
    this.crumbsEl = $(".fm-crumbs");
    this.note = $(".fm-note");
    this.empty = $(".fm-empty");
    this.foot = $(".fm-count");
    this.upBtn = $(".fm-up");
    this.head = $(".fm-head");
    this.sortBtn = $(".fm-sort");
    this.upBtn.onclick = () => this.goUp();
    $<HTMLButtonElement>(".fm-stopall").onclick = () => this.link.stopAll();
    $<HTMLButtonElement>(".fm-qo").onclick = () => host.quickOpen();
    this.sortBtn.onclick = () => this.sortBy(this.by === "name" ? "mtime" : "name", this.by === "name");
    this.head.addEventListener("click", (e) => {
      const b = (e.target as HTMLElement).closest<HTMLElement>("[data-sort]");
      if (b) this.sortBy(b.dataset.sort as SortKey, b.dataset.sort === this.by ? !this.desc : b.dataset.sort === "mtime");
    });
    this.crumbsEl.addEventListener("click", (e) => {
      const b = (e.target as HTMLElement).closest<HTMLElement>("button[data-path]");
      if (b) this.go(b.dataset.path!);
    });
    this.filter.addEventListener("input", () => this.render());
    this.filter.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (this.filter.value) (this.filter.value = ""), this.render();
        else this.focus();
        e.preventDefault();
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        this.focus();
      } else if (e.key === "Enter") {
        e.preventDefault();
        const r = this.rowEl(this.sel ?? this.shown[0]?.path ?? null);
        if (r && wantsTab(e) && r instanceof HTMLAnchorElement) window.open(r.href, "_blank", "noopener");
        else r?.click();
      }
    });
    this.list.addEventListener("focusin", (e) => {
      const r = (e.target as HTMLElement).closest<HTMLElement>(".fm-row");
      if (r) this.select(r.dataset.path!, false);
    });
    this.list.addEventListener("keydown", (e) => this.key(e));
    this.list.addEventListener("click", (e) => this.click(e));
    link.onFolder = (f) => this.show(f);
    link.onError = (m, d) => this.refused(m, d);
    link.onState = (on) => this.connection(on);
    this.go(start);
    this.renderSortButton();
  }

  private loadPrefs(): void {
    try {
      const p = JSON.parse(localStorage.getItem(this.host.prefsKey) ?? "{}");
      if (p.by === "name" || p.by === "mtime") this.by = p.by;
      this.desc = !!p.desc;
    } catch {
      /* (no storage: the defaults) */
    }
  }

  private sortBy(by: SortKey, desc: boolean): void {
    this.by = by;
    this.desc = desc;
    try {
      localStorage.setItem(this.host.prefsKey, JSON.stringify({ by, desc }));
    } catch {
      /* (not kept) */
    }
    this.renderSortButton();
    this.render();
  }

  private renderSortButton(): void {
    this.sortBtn.textContent = this.by === "name" ? "A–Z" : "Newest";
    this.sortBtn.title = this.by === "name" ? "Sorted by name: click to sort by modified" : "Sorted by modified, newest first: click to sort by name";
    for (const h of this.head.querySelectorAll<HTMLElement>("[data-sort]")) {
      const on = h.dataset.sort === this.by;
      h.classList.toggle("on", on);
      h.setAttribute("aria-sort", on ? (this.desc ? "descending" : "ascending") : "none");
      h.querySelector(".fm-dir")!.replaceChildren(on ? fileIcon(this.desc ? "desc" : "asc", 16) : "");
    }
  }

  /** Into folder `dir` (the watch refuses one outside the folder watched). */
  go(dir: string): void {
    if (dir !== this.dir) this.filter.value = "";
    this.dir = dir;
    this.link.list(dir);
  }

  goUp(): void {
    const u = up(this.folder?.dir ?? this.dir);
    if (u === null) return;
    this.from = this.folder?.dir ?? this.dir;
    this.go(u);
  }

  /** The list focused (the page's keys go to it). */
  focus(): void {
    const p = this.sel ?? this.shown[0]?.path ?? null;
    if (p) this.select(p);
    this.rowEl(p)?.focus({ preventScroll: true });
  }

  private rowEl(path: string | null): HTMLElement | null {
    if (path === null) return null;
    return [...this.list.querySelectorAll<HTMLElement>(".fm-row")].find((r) => r.dataset.path === path) ?? null;
  }

  /** Whether a row of the list has the keyboard. */
  private hasFocus(): boolean {
    const a = (this.el.getRootNode() as Document | ShadowRoot).activeElement;
    return !!a && this.list.contains(a);
  }

  focusFilter(): void {
    this.filter.focus();
    this.filter.select();
  }

  private refused(message: string, dir: string): void {
    this.say(`${message}.`, "bad");
    // (a folder refused or gone: the root, or the folder shown, stays)
    if (dir === this.dir) this.go(this.folder && this.folder.dir !== dir ? this.folder.dir : "");
  }

  private connection(on: boolean): void {
    this.lost = !on;
    if (on) this.say(null);
    else this.say(this.link.everUp ? "phitex watch has stopped. Start it again in this folder and this page joins it by itself." : "Joining phitex watch…", this.link.everUp ? "bad" : "info");
    this.el.classList.toggle("lost", !on && this.link.everUp);
  }

  /** A line above the list (null: none). */
  private say(text: string | null, tone: "info" | "bad" = "info"): void {
    this.note.hidden = !text;
    this.note.className = `fm-note ${tone}`;
    this.note.textContent = text ?? "";
  }

  /** The running documents as last told, and the strip's clock (its "stops in" counted down while one has it). */
  private running: RunningDoc[] = [];
  private runTick?: ReturnType<typeof setInterval>;

  /** The Running strip (the index's): a row a running document, its state, its tabs, Stop; Stop all. Hidden with none. */
  private drawRunning(): void {
    const box = this.el.querySelector<HTMLElement>(".fm-run")!;
    const run = this.host.compact ? [] : this.running;
    box.hidden = !run.length;
    const counting = run.some((d) => d.tabs === 0 && typeof d.stops_at === "number");
    if (counting && !this.runTick) this.runTick = setInterval(() => this.drawRunning(), 15000);
    if (!counting && this.runTick) (clearInterval(this.runTick), (this.runTick = undefined));
    if (!run.length) return;
    box.querySelector(".fm-run-n")!.textContent = String(run.length);
    box.querySelector(".fm-run-list")!.replaceChildren(
      ...run.map((d) => {
        const st = stateOf(d)!;
        const row = document.createElement("div");
        row.className = "fm-run-row";
        row.dataset.path = d.path;
        const a = document.createElement("a");
        a.className = "fm-run-name";
        a.href = this.host.docHref(d.id, d.path);
        a.title = `Open ${d.path} (${MOD}+click: in a new tab)`;
        a.append(fileIcon("tex", 16), Object.assign(document.createElement("span"), { textContent: d.path }));
        const state = document.createElement("span");
        state.className = `fm-badge tone-${st.tone}`;
        state.title = st.title;
        state.append(document.createElement("i"), st.text);
        const tabs = document.createElement("span");
        tabs.className = "fm-run-tabs";
        tabs.textContent = d.tabs === 0 ? "no tab" : d.tabs === 1 ? "1 tab" : `${d.tabs} tabs`;
        const stop = document.createElement("button");
        stop.type = "button";
        stop.className = "fm-btn fm-stop";
        stop.textContent = "Stop";
        stop.title = `Stop ${d.path}: its session ends and its memory is freed; open it again to start it`;
        stop.onclick = () => ((stop.disabled = true), this.link.stop(d.path));
        row.append(a, state, tabs, stop);
        return row;
      }),
    );
  }

  private show(f: Folder): void {
    this.running = f.running ?? [];
    this.drawRunning();
    const moved = !this.folder || this.folder.dir !== f.dir;
    this.folder = f;
    this.root = f.root;
    this.dir = f.dir;
    if (!this.lost) this.say(null);
    if (moved) {
      this.sel = this.from && f.entries.some((e) => e.path === this.from) ? this.from : null;
      this.from = null;
      this.list.scrollTop = 0;
      this.onDir(f.dir);
    }
    this.render();
    if (moved && this.sel) this.reveal();
  }

  private selected(): FileEntry | undefined {
    return this.shown.find((e) => e.path === this.sel);
  }

  /** The folder drawn: crumbs, rows, the empty states, the foot. */
  render(): void {
    const f = this.folder;
    if (!f) {
      this.empty.hidden = false;
      this.empty.replaceChildren(html`<div class="fm-e-title">Loading…</div>`);
      return;
    }
    const c = crumbs(this.root, f.dir);
    this.crumbsEl.replaceChildren(
      ...c.flatMap((x, i) => {
        const last = i === c.length - 1;
        const b = document.createElement(last ? "span" : "button");
        if (!last) ((b as HTMLButtonElement).type = "button"), (b.dataset.path = x.path), (b.title = `Go to ${x.path || x.name}`);
        else b.setAttribute("aria-current", "location");
        b.className = `fm-crumb${i === 0 ? " root" : ""}`;
        if (i === 0) b.append(fileIcon(c.length === 1 ? "folder_open" : "folder", 16));
        b.append(Object.assign(document.createElement("span"), { className: "fm-ct", textContent: x.name }));
        return i ? [Object.assign(document.createElement("span"), { className: "fm-sep", ariaHidden: "true" }), b] : [b];
      }),
    );
    for (const s of this.crumbsEl.querySelectorAll(".fm-sep")) s.replaceChildren(fileIcon("chevron", 14));
    this.upBtn.disabled = !f.dir;
    const all = sortEntries(f.entries, this.by, this.desc);
    const q = this.filter.value;
    this.shown = filterEntries(all, q);
    if (this.sel && !this.shown.some((e) => e.path === this.sel)) this.sel = null;
    const focused = this.hasFocus();
    const rows = this.shown.map((e) => this.row(e, q));
    this.list.replaceChildren(...rows);
    // (one row in the tab order: the selected one, else the first)
    (this.rowEl(this.sel) ?? rows[0])?.setAttribute("tabindex", "0");
    if (focused) this.rowEl(this.sel)?.focus({ preventScroll: true });
    // (the states said in words, not the list alone)
    const tex = f.entries.filter((e) => e.kind === "tex").length;
    const dirs = f.entries.filter((e) => e.kind === "dir").length;
    if (!f.entries.length) this.emptyState("This folder is empty", f.dir ? "Nothing in it yet. Files you add here show up at once." : "Add a .tex file to it and it shows up here at once.");
    else if (!this.shown.length) this.emptyState(`Nothing here matches “${q.trim()}”`, "", true);
    else this.empty.hidden = true;
    // (a folder with files but no document: said above them, not in their place)
    const noTex = f.entries.length > 0 && !tex && !dirs && !q;
    this.el.classList.toggle("no-tex", noTex);
    let tip = this.list.querySelector<HTMLElement>(".fm-tip");
    if (noTex) {
      tip ??= this.list.insertBefore(document.createElement("div"), this.list.firstChild);
      tip.className = "fm-tip";
      tip.setAttribute("role", "presentation");
      tip.replaceChildren(html`<b>No .tex files here.</b> Only a .tex file opens; the other files are shown as they are.`);
    }
    this.foot.textContent = q ? `${this.shown.length} of ${f.entries.length} shown` : counts(f.entries);
  }

  private emptyState(title: string, text: string, clear = false): void {
    this.empty.hidden = false;
    this.empty.replaceChildren(html`<div class="fm-e-icon">${fileIcon(clear ? "search" : "folder_open", 40)}</div><div class="fm-e-title">${title}</div>${text ? html`<div class="fm-e-text">${text}</div>` : ""}${clear ? html`<button type="button" class="fm-btn">Clear the filter</button>` : ""}`);
    const b = this.empty.querySelector<HTMLButtonElement>(".fm-btn");
    if (b) b.onclick = () => ((this.filter.value = ""), this.render(), this.filter.focus());
  }

  /** An entry's row: a link where it opens (a folder, a document, a PDF, a picture), so the browser's own ways of opening a link hold. */
  private row(e: FileEntry, q: string): HTMLElement {
    const href = this.hrefOf(e);
    const r = document.createElement(href ? "a" : "div");
    r.className = `fm-row k-${e.kind}`;
    r.dataset.path = e.path;
    r.tabIndex = -1;
    if (r instanceof HTMLAnchorElement && href) r.href = href;
    r.setAttribute("aria-selected", String(e.path === this.sel));
    if (e.path === this.host.current) r.setAttribute("aria-current", "page");
    const name = document.createElement("span");
    name.className = "fm-name";
    const at = q ? e.name.toLowerCase().indexOf(q.trim().toLowerCase()) : -1;
    name.append(at >= 0 ? marked(e.name, Array.from({ length: q.trim().length }, (_, k) => at + k)) : e.name);
    const kindWord = { dir: "Folder", tex: "Document", pdf: "PDF", image: "Picture", other: "File" }[e.kind];
    r.title = e.kind === "other" ? `${e.name}: only .tex files open here` : e.path === this.host.current ? `${e.name}: shown in this tab` : `${e.name}: ${e.kind === "dir" ? "open the folder" : "open it"} (${MOD}+click: in a new tab)`;
    const sub = document.createElement("span");
    sub.className = "fm-sub";
    sub.textContent = [e.kind === "dir" ? kindWord : "", when(e.mtime), e.kind === "dir" ? "" : sizeOf(e.size)].filter(Boolean).join(" · ");
    const nameBox = html`<span class="fm-c-name">${name}${sub}</span>`;
    const st = document.createElement("span");
    st.className = "fm-c-state";
    const b = badge(e);
    if (b) st.append(b);
    const time = document.createElement("span");
    time.className = "fm-c-time";
    time.textContent = when(e.mtime);
    time.title = whenFull(e.mtime);
    const size = document.createElement("span");
    size.className = "fm-c-size";
    size.textContent = e.kind === "dir" ? "" : sizeOf(e.size);
    const ic = document.createElement("span");
    ic.className = "fm-c-icon";
    ic.append(fileIcon(e.kind === "dir" ? "folder" : e.kind, 20));
    r.append(ic, nameBox, st, time, size);
    if (!href) r.setAttribute("aria-disabled", "true");
    return r;
  }

  private hrefOf(e: FileEntry): string | null {
    if (e.kind === "dir") return this.host.dirHref(e.path);
    if (e.kind === "tex" && e.id) return this.host.docHref(e.id, e.path);
    if (e.kind === "pdf" || e.kind === "image") return this.host.rawHref(e.path);
    return null;
  }

  private rowOf(e: Event): FileEntry | undefined {
    const r = (e.target as HTMLElement).closest<HTMLElement>(".fm-row");
    return r ? this.shown.find((x) => x.path === r.dataset.path) : undefined;
  }

  /** A click on a row: a link's, the browser's own with Ctrl, ⌘, Shift or the middle button (a new tab); a plain one on a folder goes into it here, on the document shown does nothing. */
  private click(e: MouseEvent): void {
    const row = this.rowOf(e);
    if (!row) return;
    this.select(row.path, false);
    if (wantsTab(e) || e.button !== 0 || e.altKey) return;
    if (row.kind === "dir") {
      e.preventDefault();
      this.go(row.path);
    } else if (row.kind === "other" || row.path === this.host.current) e.preventDefault();
  }

    private select(path: string | null, reveal = true): void {
    this.sel = path;
    for (const r of this.list.querySelectorAll<HTMLElement>(".fm-row")) {
      const on = r.dataset.path === path;
      r.setAttribute("aria-selected", String(on));
      r.tabIndex = on ? 0 : -1;
    }
    if (reveal) this.reveal();
  }

  private reveal(): void {
    this.list.querySelector<HTMLElement>('.fm-row[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }

  /** The selection moved `by` rows (from none: the first). */
  private move(by: number): void {
    if (!this.shown.length) return;
    const k = this.shown.findIndex((e) => e.path === this.sel);
    const next = k < 0 ? (by < 0 ? this.shown.length - 1 : 0) : Math.max(0, Math.min(this.shown.length - 1, k + by));
    this.select(this.shown[next].path);
    this.rowEl(this.shown[next].path)?.focus({ preventScroll: true });
  }

  private page(): number {
    const row = this.list.querySelector<HTMLElement>(".fm-row");
    return Math.max(1, Math.floor(this.list.clientHeight / (row?.offsetHeight || 36)) - 1);
  }

  private key(e: KeyboardEvent): void {
    let done = true;
    if (e.key === "ArrowDown" && !e.altKey) this.move(1);
    else if (e.key === "ArrowUp" && e.altKey) this.goUp();
    else if (e.key === "ArrowUp") this.move(-1);
    else if (e.key === "Home") this.move(-Infinity);
    else if (e.key === "End") this.move(Infinity);
    else if (e.key === "PageDown") this.move(this.page());
    else if (e.key === "PageUp") this.move(-this.page());
    else if (e.key === "Backspace" || (e.key === "ArrowLeft" && this.host.compact)) this.goUp();
    else if (e.key === "ArrowRight" && this.host.compact && this.sel && this.selected()?.kind === "dir") this.go(this.sel);
    else if (e.key === "Escape" && this.filter.value) {
      this.filter.value = "";
      this.render();
    }
    else if (e.key === "/") this.focusFilter();
    else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== " ") {
      // (typing filters: the letter goes into the filter)
      this.filter.focus();
      this.filter.value += e.key;
      this.render();
    } else done = false;
    if (done) {
      e.preventDefault();
      e.stopPropagation();
    }
  }
}

/**
 * Quick open: every document of the folder by its name, the best matches
 * first; Enter opens (as a click does where it was asked), Ctrl or ⌘ with
 * it a new tab.
 */
export class QuickOpen {
  private back: HTMLElement;
  private input: HTMLInputElement;
  private listEl: HTMLElement;
  private files: Found[] | null = null;
  private shown: Found[] = [];
  private at = 0;
  private hitsOf = new Map<string, number[]>();
  /** Ctrl+P again while open (the viewer prints its PDF). */
  onAgain?: () => void;

  private host: FilesHost;
  private link: FolderLink;

  constructor(mount: Element | ShadowRoot, host: FilesHost, link: FolderLink, hint = "") {
    this.host = host;
    this.link = link;
    this.back = document.createElement("div");
    this.back.className = "qo-back";
    this.back.hidden = true;
    this.back.append(html`<div class="qo" role="dialog" aria-modal="true" aria-label="Open a document by its name">
  <label class="qo-in">${fileIcon("search", 18)}<input type="text" placeholder="Open a .tex file by name" aria-label="A document's name" spellcheck="false" autocomplete="off" role="combobox" aria-expanded="true" aria-controls="qo-list"></label>
  <div class="qo-list" id="qo-list" role="listbox" aria-label="Documents"></div>
  <div class="qo-foot"><span><kbd>↑</kbd><kbd>↓</kbd> choose</span><span><kbd>Enter</kbd> open</span><span><kbd>${MOD}</kbd><kbd>Enter</kbd> new tab</span><span><kbd>Esc</kbd> close</span>${hint ? html`<span class="qo-hint">${hint}</span>` : ""}</div>
</div>`);
    this.input = this.back.querySelector("input")!;
    this.listEl = this.back.querySelector(".qo-list")!;
    this.back.addEventListener("mousedown", (e) => {
      if (e.target === this.back) (e.preventDefault(), this.close());
    });
    this.input.addEventListener("input", () => ((this.at = 0), this.render()));
    this.input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "ArrowDown") (e.preventDefault(), this.step(1));
      else if (e.key === "ArrowUp") (e.preventDefault(), this.step(-1));
      else if (e.key === "PageDown") (e.preventDefault(), this.step(8));
      else if (e.key === "PageUp") (e.preventDefault(), this.step(-8));
      else if (e.key === "Escape") (e.preventDefault(), this.close());
      else if (e.key === "Enter") {
        e.preventDefault();
        const f = this.shown[this.at];
        if (f) this.open(f, e.ctrlKey || e.metaKey || e.shiftKey);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        if (this.onAgain) (this.close(), this.onAgain());
      }
    });
    this.listEl.addEventListener("click", (e) => {
      const r = (e.target as HTMLElement).closest<HTMLElement>(".qo-row");
      if (!r) return;
      e.preventDefault();
      const f = this.shown[Number(r.dataset.k)];
      if (f) this.open(f, wantsTab(e));
    });
    this.listEl.addEventListener("auxclick", (e) => {
      const r = (e.target as HTMLElement).closest<HTMLElement>(".qo-row");
      if (e.button !== 1 || !r) return;
      e.preventDefault();
      const f = this.shown[Number(r.dataset.k)];
      if (f) this.open(f, true);
    });
    this.listEl.addEventListener("mousemove", (e) => {
      const r = (e.target as HTMLElement).closest<HTMLElement>(".qo-row");
      if (r && Number(r.dataset.k) !== this.at) ((this.at = Number(r.dataset.k)), this.mark());
    });
    mount.append(this.back);
  }

  get isOpen(): boolean {
    return !this.back.hidden;
  }

  open_(): void {
    this.back.hidden = false;
    this.input.value = "";
    this.at = 0;
    this.render();
    this.input.focus();
    // (the folder's documents asked for each time: files come and go)
    void this.link.find().then((f) => {
      this.files = f;
      this.render();
    });
  }

  close(): void {
    this.back.hidden = true;
  }

  toggle(): void {
    if (this.isOpen) this.close();
    else this.open_();
  }

  private open(f: Found, newTab: boolean): void {
    this.close();
    const href = this.host.docHref(f.id, f.path);
    if (newTab) window.open(href, "_blank", "noopener");
    else if (f.path !== this.host.current) location.assign(href);
  }

  private step(by: number): void {
    if (!this.shown.length) return;
    this.at = Math.max(0, Math.min(this.shown.length - 1, this.at + by));
    this.mark();
  }

  private mark(): void {
    this.listEl.querySelectorAll<HTMLElement>(".qo-row").forEach((r, i) => r.setAttribute("aria-selected", String(i === this.at)));
    this.listEl.querySelector<HTMLElement>('.qo-row[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    this.input.setAttribute("aria-activedescendant", `qo-r-${this.at}`);
  }

  private render(): void {
    if (!this.files) {
      this.listEl.replaceChildren(html`<div class="qo-none">Looking for documents…</div>`);
      return;
    }
    const q = this.input.value;
    const ms = rank(q, this.files.map((f) => f.path), 60);
    const byPath = new Map(this.files.map((f) => [f.path, f]));
    this.shown = ms.map((m) => byPath.get(m.path)!);
    this.hitsOf = new Map(ms.map((m) => [m.path, m.hits]));
    if (!this.files.length) {
      this.listEl.replaceChildren(html`<div class="qo-none"><b>No .tex files in this folder</b>Add one and it shows up here.</div>`);
      return;
    }
    if (!this.shown.length) {
      this.listEl.replaceChildren(html`<div class="qo-none"><b>No document matches “${q.trim()}”</b>Try fewer letters, or part of its folder's name.</div>`);
      return;
    }
    this.at = Math.min(this.at, this.shown.length - 1);
    this.listEl.replaceChildren(
      ...this.shown.map((f, k) => {
        const hits = this.hitsOf.get(f.path) ?? [];
        const cut = f.path.lastIndexOf("/") + 1;
        const r = document.createElement("a");
        r.className = "qo-row";
        r.id = `qo-r-${k}`;
        r.dataset.k = String(k);
        r.href = this.host.docHref(f.id, f.path);
        r.tabIndex = -1;
        r.setAttribute("role", "option");
        r.setAttribute("aria-selected", String(k === this.at));
        if (f.path === this.host.current) r.setAttribute("aria-current", "page");
        r.append(fileIcon("tex", 18), html`<span class="qo-name">${marked(f.path.slice(cut), hits, cut)}</span><span class="qo-dir">${cut ? marked(f.path.slice(0, cut - 1), hits, 0) : ""}</span>${f.path === this.host.current ? html`<span class="qo-here">this tab</span>` : ""}`);
        return r;
      }),
    );
    this.input.setAttribute("aria-activedescendant", `qo-r-${this.at}`);
  }
}

/** The file manager's and quick open's look: the --ph-* tokens only. */
export const FILES_CSS = `
.fm { --fm-row: 34px; display: flex; flex-direction: column; min-height: 0; height: 100%; container-type: inline-size; color: var(--ph-fg); font: var(--ph-size, 13px)/1.4 var(--ph-font, system-ui, sans-serif); background: var(--ph-bar); }
.fm svg { fill: currentColor; flex: none; }
.fm [hidden], .qo [hidden] { display: none !important; }
.fm button, .qo button { font: inherit; color: inherit; }
.fm kbd, .qo kbd { display: inline-block; min-width: 1.6em; padding: 0 4px; margin: 0 1px; border: 1px solid var(--ph-border); border-bottom-width: 2px; border-radius: 4px; font: 600 11px/1.5 var(--ph-font, system-ui, sans-serif); text-align: center; color: var(--ph-fg2); background: var(--ph-stage); }
.fm-grow { flex: 1; }
.fm-bar { display: flex; align-items: center; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--ph-border); min-height: 48px; flex-wrap: wrap; }
.fm-ib { all: unset; box-sizing: border-box; display: inline-flex; align-items: center; justify-content: center; gap: 4px; min-width: 30px; height: 30px; padding: 0 6px; border-radius: var(--ph-radius, 6px); color: var(--ph-fg2); cursor: pointer; font: 600 12px/1 var(--ph-font, system-ui, sans-serif); }
.fm-ib:hover:not(:disabled) { background: var(--ph-hover); color: var(--ph-fg); }
.fm-ib:focus-visible { outline: 2px solid var(--ph-accent); outline-offset: 1px; }
.fm-ib:disabled { opacity: .35; cursor: default; }
.fm-crumbs { display: flex; align-items: center; flex-wrap: wrap; gap: 0; min-width: 0; font-size: 14px; }
.fm-crumb { all: unset; box-sizing: border-box; display: inline-flex; align-items: center; gap: 6px; padding: 4px 6px; border-radius: var(--ph-radius, 6px); max-width: 26ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
button.fm-crumb { color: var(--ph-fg2); cursor: pointer; }
button.fm-crumb:hover { background: var(--ph-hover); color: var(--ph-fg); }
button.fm-crumb:focus-visible { outline: 2px solid var(--ph-accent); }
span.fm-crumb { font-weight: 600; color: var(--ph-fg); }
.fm-ct { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.fm-crumb.root svg { color: var(--fm-folder, #d69a2d); }
.fm-sep { display: inline-flex; color: var(--ph-fg2); opacity: .6; }
.fm-filter { display: flex; align-items: center; gap: 6px; height: 30px; padding: 0 8px; border: 1px solid var(--ph-border); border-radius: var(--ph-radius, 6px); background: var(--ph-stage); color: var(--ph-fg2); width: 220px; max-width: 100%; }
.fm-filter:focus-within { border-color: var(--ph-accent); box-shadow: 0 0 0 2px color-mix(in srgb, var(--ph-accent) 22%, transparent); }
.fm-filter input { all: unset; flex: 1; min-width: 0; width: 100%; color: var(--ph-fg); font: inherit; }
.fm-filter input::placeholder { color: var(--ph-fg2); opacity: .8; }
.fm-sort { display: none; }
.fm-note { padding: 8px 14px; font-size: 13px; border-bottom: 1px solid var(--ph-border); }
.fm-note.info { background: color-mix(in srgb, var(--ph-info, #1d3e72) 10%, var(--ph-bar)); }
.fm-note.bad { background: color-mix(in srgb, var(--ph-danger) 14%, var(--ph-bar)); color: var(--ph-fg); }
.fm-head, .fm-row { display: grid; grid-template-columns: 28px minmax(0, 1fr) minmax(130px, 220px) 92px 72px 0px; align-items: center; column-gap: 12px; padding: 0 14px; }
.fm-head { height: 30px; border-bottom: 1px solid var(--ph-border); color: var(--ph-fg2); font-size: 12px; font-weight: 600; }
.fm-h { all: unset; display: inline-flex; align-items: center; gap: 2px; cursor: default; white-space: nowrap; }
button.fm-h { cursor: pointer; border-radius: 4px; padding: 2px 4px; margin: 0 -4px; }
button.fm-h:hover { color: var(--ph-fg); background: var(--ph-hover); }
button.fm-h:focus-visible { outline: 2px solid var(--ph-accent); }
.fm-h.on { color: var(--ph-fg); }
.fm-h .fm-dir { display: inline-flex; width: 16px; }
.fm-c-time, .fm-c-size { text-align: right; justify-content: flex-end; }
.fm-head .fm-c-time { justify-self: end; }
.fm-head .fm-c-size { justify-self: end; }
.fm-list { flex: 1; min-height: 0; overflow: auto; outline: none; padding: 4px 0 12px; overscroll-behavior: contain; }
.fm-row { position: relative; height: var(--fm-row); cursor: default; user-select: none; border-left: 3px solid transparent; padding-left: 11px; color: inherit; text-decoration: none; outline: none; }
a.fm-row { cursor: pointer; }
.fm-row:hover { background: var(--ph-hover); }
.fm-row[aria-selected=true] { background: color-mix(in srgb, var(--ph-accent) 12%, transparent); }
.fm-row:focus-visible { background: color-mix(in srgb, var(--ph-accent) 20%, transparent); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ph-accent) 55%, transparent); }
.fm-row[aria-current=page] { border-left-color: var(--ph-accent); }
.fm-row[aria-current=page] .fm-name { font-weight: 650; }
.fm-c-icon { display: inline-flex; align-items: center; justify-content: center; color: var(--ph-fg2); }
.k-dir .fm-c-icon { color: var(--fm-folder, #d69a2d); }
.k-tex .fm-c-icon { color: var(--ph-link, var(--ph-accent)); }
.k-pdf .fm-c-icon { color: var(--fm-pdf, #c8443a); }
.k-image .fm-c-icon { color: var(--fm-image, #2f8a5b); }
.k-other .fm-c-icon { opacity: .55; }
.fm-c-name { display: flex; flex-direction: column; min-width: 0; }
.fm-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fm-name b { font-weight: 700; background: color-mix(in srgb, var(--ph-progress, #f58f2d) 28%, transparent); border-radius: 2px; }
.k-tex .fm-name { font-weight: 550; }
.k-other .fm-name, .k-other .fm-c-time, .k-other .fm-c-size { color: var(--ph-fg2); opacity: .75; }
.fm-sub { display: none; color: var(--ph-fg2); font-size: 11.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fm-c-time, .fm-c-size { color: var(--ph-fg2); font-variant-numeric: tabular-nums; white-space: nowrap; }
.fm-c-state { display: flex; min-width: 0; }
.fm-badge { display: inline-flex; align-items: center; gap: 6px; max-width: 100%; padding: 2px 9px 2px 7px; border-radius: 999px; font-size: 12px; font-weight: 600; line-height: 18px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; background: color-mix(in srgb, currentColor 11%, transparent); }
.fm-badge i { flex: none; width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
.fm-badge.tone-ok { color: var(--fm-ok, #1f8a4c); }
.fm-badge.tone-bad { color: var(--ph-danger); }
.fm-badge.tone-busy { color: var(--ph-warning); }
.fm-badge.tone-busy i { animation: fm-pulse 1s ease-in-out infinite; }
.fm-badge.tone-none { color: var(--ph-fg2); }
.fm-badge.tone-idle { color: var(--fm-idle, #5b7bb0); }
.fm-run { margin: 10px 14px 4px; border: 1px solid var(--ph-border); border-radius: 8px; background: var(--ph-stage); overflow: hidden; }
.fm-run header { display: flex; align-items: center; gap: 8px; padding: 6px 8px 6px 12px; border-bottom: 1px solid var(--ph-border); font-size: 12px; color: var(--ph-fg2); text-transform: uppercase; letter-spacing: .4px; }
.fm-run header b { font-weight: 650; }
.fm-run-n { min-width: 18px; padding: 0 6px; border-radius: 999px; background: var(--ph-hover); color: var(--ph-fg); font-weight: 650; text-align: center; letter-spacing: 0; }
.fm-run .fm-btn { margin: 0; background: var(--ph-bar); padding: 3px 12px; font-size: 12px; text-transform: none; letter-spacing: 0; }
.fm-run .fm-btn:disabled { opacity: .5; cursor: default; }
.fm-run-row { display: grid; grid-template-columns: minmax(0, 1fr) auto 64px auto; align-items: center; column-gap: 12px; padding: 5px 8px 5px 12px; border-top: 1px solid var(--ph-border); }
.fm-run-row:first-child { border-top: 0; }
.fm-run-name { display: flex; align-items: center; gap: 8px; min-width: 0; color: var(--ph-fg); text-decoration: none; font-weight: 550; }
.fm-run-name span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fm-run-name svg { color: var(--ph-link, var(--ph-accent)); }
.fm-run-name:hover span { text-decoration: underline; }
.fm-run-name:focus-visible { outline: 2px solid var(--ph-accent); border-radius: 4px; }
.fm-run-tabs { color: var(--ph-fg2); font-size: 12px; white-space: nowrap; text-align: right; }
.fm-badge.ended { opacity: .6; }
.fm-badge.shown i { box-shadow: 0 0 0 2px color-mix(in srgb, currentColor 30%, transparent); }
@keyframes fm-pulse { 50% { opacity: .25; } }
@media (prefers-reduced-motion: reduce) { .fm-badge.tone-busy i { animation: none; } }
.fm-tip { padding: 10px 14px 10px 17px; margin: 0 0 4px; color: var(--ph-fg2); font-size: 12.5px; border-bottom: 1px dashed var(--ph-border); }
.fm-tip b { color: var(--ph-fg); font-weight: 600; }
.fm-empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; padding: 32px 20px 48px; text-align: center; color: var(--ph-fg2); }
.fm-list:has(+ .fm-empty:not([hidden])) { flex: none; padding: 0; }
.fm-e-icon { color: var(--ph-border); margin-bottom: 4px; }
.fm-e-icon svg { fill: color-mix(in srgb, var(--ph-fg2) 45%, transparent); }
.fm-e-title { color: var(--ph-fg); font-size: 15px; font-weight: 600; }
.fm-e-text { max-width: 32em; font-size: 13px; }
.fm-btn { all: unset; margin-top: 10px; padding: 6px 14px; border-radius: var(--ph-radius, 6px); border: 1px solid var(--ph-border); background: var(--ph-stage); cursor: pointer; font-weight: 600; color: var(--ph-fg); }
.fm-btn:hover { border-color: var(--ph-accent); }
.fm-btn:focus-visible { outline: 2px solid var(--ph-accent); }
.fm-foot { display: flex; align-items: center; gap: 8px; min-height: 34px; padding: 4px 8px 4px 14px; border-top: 1px solid var(--ph-border); color: var(--ph-fg2); font-size: 12px; }
.fm-qo { all: unset; display: inline-flex; align-items: center; gap: 4px; padding: 3px 8px; border-radius: var(--ph-radius, 6px); cursor: pointer; color: var(--ph-fg2); font-size: 12px; }
.fm-qo span { margin: 0 6px 0 2px; }
.fm-qo:hover { background: var(--ph-hover); color: var(--ph-fg); }
.fm-qo:focus-visible { outline: 2px solid var(--ph-accent); }
.fm.lost .fm-list, .fm.lost .fm-head { opacity: .55; }
/* (narrow: name and state, the time and size under the name) */
@container (max-width: 620px) {
  .fm-head { display: none; }
  .fm-row { grid-template-columns: 26px minmax(0, 1fr) auto; height: auto; min-height: 44px; padding-top: 5px; padding-bottom: 5px; }
  .fm-row .fm-c-time, .fm-row .fm-c-size { display: none; }
  .fm-sub { display: block; }
  .fm-c-state { justify-content: flex-end; }
  .fm-filter { width: auto; flex: 1 1 140px; }
  .fm-sort { display: inline-flex; }
  .fm-bar { gap: 4px; }
  .fm-qo kbd { display: none; }
  .fm-run-row { grid-template-columns: minmax(0, 1fr) auto; row-gap: 4px; }
  .fm-run-row .fm-badge { grid-column: 1; justify-self: start; }
  .fm-run-tabs { display: none; }
  .fm-run-row .fm-stop { grid-row: 1 / span 2; grid-column: 2; }
}
/* (the viewer's Files column) */
.fm.compact { --fm-row: 30px; }
.fm.compact .fm-bar { padding: 6px 6px 8px 6px; gap: 4px 2px; min-height: 0; }
.fm.compact .fm-up { order: 0; min-width: 28px; height: 28px; }
.fm.compact .fm-crumbs { order: 1; flex: 1 1 calc(100% - 40px); flex-wrap: nowrap; overflow: hidden; font-size: 12.5px; }
.fm.compact .fm-crumb { flex: 0 1 auto; min-width: 3ch; padding: 3px 4px; max-width: none; }
.fm.compact span.fm-crumb { flex: 0 0 auto; max-width: 100%; }
.fm.compact .fm-sep { flex: none; }
.fm.compact .fm-filter { order: 2; height: 28px; flex: 1 1 0; width: auto; min-width: 0; }
.fm.compact .fm-sort { order: 3; height: 28px; }
.fm.compact .fm-grow { display: none; }
.fm.compact .fm-head { display: none; }
.fm.compact .fm-row { grid-template-columns: 22px minmax(0, 1fr) auto; column-gap: 8px; height: auto; min-height: var(--fm-row); padding: 3px 8px 3px 7px; }
.fm.compact .fm-row .fm-c-time, .fm.compact .fm-row .fm-c-size { display: none; }
.fm.compact .fm-sub { display: none; }
.fm.compact .fm-badge { font-size: 0; gap: 0; padding: 4px; background: none; }
.fm.compact .fm-badge i { width: 8px; height: 8px; }
.fm.compact .fm-sort { display: inline-flex; }
.fm.compact .fm-foot { padding-left: 8px; }
.fm.compact .fm-tip { padding: 8px 10px; }
.fm.compact .fm-empty { padding: 24px 12px; }

.qo-back { position: fixed; inset: 0; z-index: 50; display: flex; justify-content: center; align-items: flex-start; padding: min(12vh, 96px) 16px 16px; background: rgb(10 14 20 / 32%); }
.qo-back[hidden] { display: none; }
.qo { width: min(620px, 100%); max-height: min(70vh, 560px); display: flex; flex-direction: column; overflow: hidden; border-radius: 10px; background: var(--ph-menu, var(--ph-bar)); color: var(--ph-fg); border: 1px solid var(--ph-border); box-shadow: 0 18px 48px rgb(0 0 0 / 30%); font: var(--ph-size, 13px)/1.4 var(--ph-font, system-ui, sans-serif); }
.qo svg { fill: currentColor; flex: none; }
.qo-in { display: flex; align-items: center; gap: 10px; padding: 12px 14px; border-bottom: 1px solid var(--ph-border); color: var(--ph-fg2); }
.qo-in input { all: unset; flex: 1; min-width: 0; color: var(--ph-fg); font-size: 15px; }
.qo-in input::placeholder { color: var(--ph-fg2); }
.qo-list { overflow: auto; padding: 6px; }
.qo-row { display: flex; align-items: baseline; gap: 10px; padding: 7px 10px; border-radius: 6px; color: inherit; text-decoration: none; cursor: pointer; }
.qo-row svg { align-self: center; color: var(--ph-link, var(--ph-accent)); }
.qo-row[aria-selected=true] { background: color-mix(in srgb, var(--ph-accent) 16%, transparent); }
.qo-name { font-weight: 600; white-space: nowrap; }
.qo-dir { color: var(--ph-fg2); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 1; }
.qo-name b, .qo-dir b { color: var(--ph-accent); font-weight: 750; }
.qo-here { flex: none; font-size: 11px; color: var(--ph-fg2); border: 1px solid var(--ph-border); border-radius: 999px; padding: 0 7px; }
.qo-none { display: flex; flex-direction: column; gap: 2px; padding: 22px 12px; text-align: center; color: var(--ph-fg2); }
.qo-none b { color: var(--ph-fg); font-weight: 600; }
.qo-foot { display: flex; flex-wrap: wrap; gap: 4px 14px; padding: 8px 14px; border-top: 1px solid var(--ph-border); color: var(--ph-fg2); font-size: 12px; }
.qo-hint { margin-left: auto; }
@media (max-width: 520px) { .qo-foot span:nth-child(n+3) { display: none; } .qo-back { padding-top: 12px; } }
`;
