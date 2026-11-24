// phitex watch DIR's index page: viewer/src and crates/cli/viewer/dir.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)

// viewer/src/html.ts
var MARK = "phx-slot-";
var parsed = /* @__PURE__ */ new WeakMap();
var parsedSvg = /* @__PURE__ */ new WeakMap();
function html(strings, ...values) {
  return build(strings, values, false);
}
function build(strings, values, inSvg) {
  const cache = inSvg ? parsedSvg : parsed;
  let base = cache.get(strings);
  if (!base) {
    let src = "";
    strings.forEach((s, i) => {
      src += s;
      if (i === values.length) return;
      if (/<(style|textarea|title|script)\b[^>]*>(?![\s\S]*<\/\1>)[\s\S]*$/i.test(src)) throw new Error("html: no value inside <style>, <textarea>, <title> or <script>");
      if (/<[a-zA-Z][^<>]*=\s*"[^"]*$/.test(src)) src += `${MARK}${i}`;
      else if (/<[a-zA-Z][^<>]*$/.test(src)) throw new Error(`html: a value inside a tag must be in a quoted attribute (after ${JSON.stringify(s.slice(-40))})`);
      else src += `<template ${MARK}${i}></template>`;
    });
    const t = new DOMParser().parseFromString(`<template>${inSvg ? `<svg>${src}</svg>` : src}</template>`, "text/html").querySelector("template").content;
    if (inSvg) {
      base = t.ownerDocument.createDocumentFragment();
      base.append(...t.firstElementChild.childNodes);
    } else base = t;
    const drop = (n) => {
      for (const k of [...n.childNodes]) {
        if (k.nodeType === Node.TEXT_NODE && /^\s*$/.test(k.textContent) && k.textContent.includes("\n")) k.remove();
        else drop(k instanceof HTMLTemplateElement ? k.content : k);
      }
    };
    drop(base);
    cache.set(strings, base);
  }
  const frag = base.cloneNode(true);
  const nodes = (v) => v === null || v === void 0 || v === false ? [] : Array.isArray(v) ? v.flatMap(nodes) : [v instanceof Node ? v : String(v)];
  const walk = (root) => {
    for (const el of [...root.querySelectorAll("*")]) {
      if (el.localName === "template" && el.attributes.length === 1 && el.attributes[0].name.startsWith(MARK)) {
        el.replaceWith(...nodes(values[+el.attributes[0].name.slice(MARK.length)]));
        continue;
      }
      for (const a of [...el.attributes]) {
        if (!a.value.includes(MARK)) continue;
        if (a.name.startsWith("on")) throw new Error(`html: no value in an event handler (${a.name})`);
        const whole = new RegExp(`^${MARK}(\\d+)$`).exec(a.value), v = whole && values[+whole[1]];
        if (whole && !/^(aria|data)-/.test(a.name) && (v === true || v === false || v === null || v === void 0)) {
          if (v === true) el.setAttribute(a.name, "");
          else el.removeAttribute(a.name);
          continue;
        }
        el.setAttribute(a.name, a.value.replace(new RegExp(`${MARK}(\\d+)`, "g"), (_, n) => nodes(values[+n]).map((x) => typeof x === "string" ? x : x.textContent).join("")));
      }
      if (el instanceof HTMLTemplateElement) walk(el.content);
    }
  };
  walk(frag);
  return frag;
}

// viewer/src/folders.ts
function crumbs(root, dir) {
  const out = [{ name: root || "/", path: "" }];
  let at = "";
  for (const part of dir.split("/").filter(Boolean)) {
    at = at ? `${at}/${part}` : part;
    out.push({ name: part, path: at });
  }
  return out;
}
function up(dir) {
  if (!dir) return null;
  const k = dir.lastIndexOf("/");
  return k < 0 ? "" : dir.slice(0, k);
}
var collator = new Intl.Collator(void 0, { numeric: true, sensitivity: "base" });
function sortEntries(entries, by, desc = false) {
  const sign = desc ? -1 : 1;
  return [...entries].sort((a, b) => {
    const d = Number(b.kind === "dir") - Number(a.kind === "dir");
    if (d) return d;
    if (by === "mtime") {
      const t = (a.mtime ?? 0) - (b.mtime ?? 0);
      if (t) return sign * t;
    }
    return sign * collator.compare(a.name, b.name) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
  });
}
function filterEntries(entries, q) {
  const n = q.trim().toLowerCase();
  return n ? entries.filter((e) => e.name.toLowerCase().includes(n)) : [...entries];
}
function stateOf(d) {
  if (!d) return null;
  const where = d.tabs > 1 ? `open in ${d.tabs} tabs` : d.tabs === 1 ? "open in a tab" : d.live ? "no tab open: kept ready, it builds when shown again" : "closed: opening it builds it again";
  const s = (text, tone, more = "") => ({ text, tone, title: [more, where].filter(Boolean).join(" \xB7 ") });
  switch (d.build) {
    case "building":
      return s("Building\u2026", "busy");
    case "waiting":
      return s("Waiting\u2026", "busy", "Waiting for another document's build");
    case "ok":
      return s(d.pages === 1 ? "Built \xB7 1 page" : `Built \xB7 ${d.pages} pages`, "ok");
    case "errors":
      return d.errors <= 1 && d.message ? s("Failed", "bad", d.message) : s(d.errors === 1 ? "1 error" : `${d.errors} errors`, "bad", d.message ?? "");
    default:
      return s("Not built", "none", d.live ? "Starting" : "");
  }
}
var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
var two = (n) => String(n).padStart(2, "0");
function when(mtime, now = Date.now()) {
  if (mtime === null) return "";
  const t = new Date(mtime), n = new Date(now);
  const day = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(n) - day(t)) / 864e5);
  if (days === 0) return `${two(t.getHours())}:${two(t.getMinutes())}`;
  if (days === 1) return "Yesterday";
  if (t.getFullYear() === n.getFullYear()) return `${MONTHS[t.getMonth()]} ${t.getDate()}`;
  return `${t.getFullYear()}-${two(t.getMonth() + 1)}-${two(t.getDate())}`;
}
function whenFull(mtime) {
  if (mtime === null) return "";
  const t = new Date(mtime);
  return `${t.getFullYear()}-${two(t.getMonth() + 1)}-${two(t.getDate())} ${two(t.getHours())}:${two(t.getMinutes())}`;
}
function sizeOf(bytes) {
  if (bytes < 1e3) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1e3, k = 0;
  while (v >= 1e3 && k < units.length - 1) v /= 1e3, k++;
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[k]}`;
}
function counts(entries) {
  const dirs = entries.filter((e) => e.kind === "dir").length, files2 = entries.length - dirs;
  const p = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`;
  return [dirs ? p(dirs, "folder") : "", files2 ? p(files2, "file") : ""].filter(Boolean).join(" \xB7 ") || "Empty";
}
function fuzzy(q, path) {
  const query = q.toLowerCase().replace(/\s+/g, "");
  if (!query) return { path, score: 0, hits: [] };
  const p = path.toLowerCase();
  const nameAt = path.lastIndexOf("/") + 1;
  const n = query.length, m = p.length;
  if (n > m) return null;
  const NEG = -1e9;
  const best = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(NEG));
  const take = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(false));
  for (let j = 0; j <= m; j++) best[n][j] = 0;
  const startOfWord = (j) => j === 0 || /[/_\-. ]/.test(p[j - 1]) || path[j] !== p[j] && path[j - 1] === p[j - 1];
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      let skip = best[i][j + 1];
      let here = NEG;
      if (p[j] === query[i] && best[i + 1][j + 1] > NEG) {
        let s = 1;
        if (j >= nameAt) s += 2;
        if (startOfWord(j)) s += 3;
        if (i + 1 < n && j + 1 < m && p[j + 1] === query[i + 1]) s += 2;
        here = s + best[i + 1][j + 1];
      }
      if (here >= skip && here > NEG) {
        best[i][j] = here;
        take[i][j] = true;
      } else best[i][j] = skip;
    }
  }
  if (best[0][0] <= NEG) return null;
  const hits = [];
  for (let i = 0, j = 0; i < n && j < m; j++) {
    if (take[i][j]) {
      hits.push(j);
      i++;
    }
  }
  const score = best[0][0] * 100 - path.length - (m - nameAt);
  return { path, score, hits };
}
function rank(q, paths, limit = 50) {
  if (!q.trim()) return [...paths].sort((a, b) => collator.compare(a, b)).slice(0, limit).map((path) => ({ path, score: 0, hits: [] }));
  const out = [];
  for (const p of paths) {
    const m = fuzzy(q, p);
    if (m) out.push(m);
  }
  out.sort((a, b) => b.score - a.score || collator.compare(a.path, b.path));
  return out.slice(0, limit);
}
function runs(s, hits, from = 0) {
  const set = new Set(hits);
  const out = [];
  for (let k = 0; k < s.length; k++) {
    const on = set.has(k + from);
    const last = out[out.length - 1];
    if (last && last[1] === on) last[0] += s[k];
    else out.push([s[k], on]);
  }
  return out;
}

// viewer/src/files.ts
var ICONS = {
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
  folder_open: "M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z"
};
function fileIcon(name, size = 18) {
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
var FolderLink = class {
  ws;
  wait = 400;
  dir = null;
  finds = [];
  /** Whether it is joined now; whether it ever was. */
  up = false;
  everUp = false;
  onHello = () => {
  };
  onFolder = () => {
  };
  onError = () => {
  };
  onState = () => {
  };
  url;
  constructor(url) {
    this.url = url;
    this.open();
  }
  open() {
    const ws = this.ws = new WebSocket(this.url);
    ws.onopen = () => {
      this.up = this.everUp = true;
      this.wait = 400;
      this.onState(true);
      if (this.dir !== null) this.send({ op: "list", dir: this.dir });
      if (this.finds.length) this.send({ op: "find" });
    };
    ws.onmessage = (m) => {
      let e;
      try {
        e = JSON.parse(String(m.data));
      } catch {
        return;
      }
      if (e.event === "hello") this.onHello(e);
      else if (e.event === "folder") this.onFolder(e);
      else if (e.event === "error") this.onError(String(e.message ?? "refused"), String(e.dir ?? ""));
      else if (e.event === "found") for (const f of this.finds.splice(0)) f(e.files);
    };
    ws.onclose = () => {
      const was = this.up;
      this.up = false;
      if (was || !this.everUp) this.onState(false);
      setTimeout(() => this.open(), this.wait);
      this.wait = Math.min(this.wait * 2, 3e3);
    };
  }
  send(op) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(op));
  }
  /** Folder `dir` (from the root), sent as it is and again as it changes. */
  list(dir) {
    this.dir = dir;
    this.send({ op: "list", dir });
  }
  /** Every document of the folder (quick open). */
  find() {
    return new Promise((ok) => {
      this.finds.push(ok);
      if (this.finds.length === 1) this.send({ op: "find" });
    });
  }
};
var isMac = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform ?? "");
var MOD = isMac ? "\u2318" : "Ctrl";
var wantsTab = (e) => e.ctrlKey || e.metaKey || e.shiftKey || "button" in e && e.button === 1;
function marked(s, hits, from = 0) {
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
function badge(e) {
  const s = stateOf(e.doc);
  if (!s) return null;
  const b = document.createElement("span");
  b.className = `fm-badge tone-${s.tone}${e.doc && e.doc.tabs > 0 ? " shown" : ""}${e.doc && !e.doc.live ? " ended" : ""}`;
  b.title = s.title;
  b.append(document.createElement("i"), s.text);
  return b;
}
var FileList = class {
  el;
  list;
  filter;
  crumbsEl;
  note;
  empty;
  foot;
  upBtn;
  head;
  sortBtn;
  folder = null;
  shown = [];
  sel = null;
  by = "name";
  desc = false;
  root = "";
  /** The folder to select once its parent is shown (going up). */
  from = null;
  lost = false;
  /** The folder asked for, until it comes. */
  dir = "";
  /** The folder shown changed (the page keeps it in its address). */
  onDir = () => {
  };
  host;
  link;
  constructor(host2, link2, start = "") {
    this.host = host2;
    this.link = link2;
    const compact = !!host2.compact;
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
    const $ = (s) => this.el.querySelector(s);
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
    $(".fm-qo").onclick = () => host2.quickOpen();
    this.sortBtn.onclick = () => this.sortBy(this.by === "name" ? "mtime" : "name", this.by === "name");
    this.head.addEventListener("click", (e) => {
      const b = e.target.closest("[data-sort]");
      if (b) this.sortBy(b.dataset.sort, b.dataset.sort === this.by ? !this.desc : b.dataset.sort === "mtime");
    });
    this.crumbsEl.addEventListener("click", (e) => {
      const b = e.target.closest("button[data-path]");
      if (b) this.go(b.dataset.path);
    });
    this.filter.addEventListener("input", () => this.render());
    this.filter.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (this.filter.value) this.filter.value = "", this.render();
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
      const r = e.target.closest(".fm-row");
      if (r) this.select(r.dataset.path, false);
    });
    this.list.addEventListener("keydown", (e) => this.key(e));
    this.list.addEventListener("click", (e) => this.click(e));
    link2.onFolder = (f) => this.show(f);
    link2.onError = (m, d) => this.refused(m, d);
    link2.onState = (on) => this.connection(on);
    this.go(start);
    this.renderSortButton();
  }
  loadPrefs() {
    try {
      const p = JSON.parse(localStorage.getItem(this.host.prefsKey) ?? "{}");
      if (p.by === "name" || p.by === "mtime") this.by = p.by;
      this.desc = !!p.desc;
    } catch {
    }
  }
  sortBy(by, desc) {
    this.by = by;
    this.desc = desc;
    try {
      localStorage.setItem(this.host.prefsKey, JSON.stringify({ by, desc }));
    } catch {
    }
    this.renderSortButton();
    this.render();
  }
  renderSortButton() {
    this.sortBtn.textContent = this.by === "name" ? "A\u2013Z" : "Newest";
    this.sortBtn.title = this.by === "name" ? "Sorted by name: click to sort by modified" : "Sorted by modified, newest first: click to sort by name";
    for (const h of this.head.querySelectorAll("[data-sort]")) {
      const on = h.dataset.sort === this.by;
      h.classList.toggle("on", on);
      h.setAttribute("aria-sort", on ? this.desc ? "descending" : "ascending" : "none");
      h.querySelector(".fm-dir").replaceChildren(on ? fileIcon(this.desc ? "desc" : "asc", 16) : "");
    }
  }
  /** Into folder `dir` (the watch refuses one outside the folder watched). */
  go(dir) {
    if (dir !== this.dir) this.filter.value = "";
    this.dir = dir;
    this.link.list(dir);
  }
  goUp() {
    const u = up(this.folder?.dir ?? this.dir);
    if (u === null) return;
    this.from = this.folder?.dir ?? this.dir;
    this.go(u);
  }
  /** The list focused (the page's keys go to it). */
  focus() {
    const p = this.sel ?? this.shown[0]?.path ?? null;
    if (p) this.select(p);
    this.rowEl(p)?.focus({ preventScroll: true });
  }
  rowEl(path) {
    if (path === null) return null;
    return [...this.list.querySelectorAll(".fm-row")].find((r) => r.dataset.path === path) ?? null;
  }
  /** Whether a row of the list has the keyboard. */
  hasFocus() {
    const a = this.el.getRootNode().activeElement;
    return !!a && this.list.contains(a);
  }
  focusFilter() {
    this.filter.focus();
    this.filter.select();
  }
  refused(message, dir) {
    this.say(`${message}.`, "bad");
    if (dir === this.dir) this.go(this.folder && this.folder.dir !== dir ? this.folder.dir : "");
  }
  connection(on) {
    this.lost = !on;
    if (on) this.say(null);
    else this.say(this.link.everUp ? "phitex watch has stopped. Start it again in this folder and this page joins it by itself." : "Joining phitex watch\u2026", this.link.everUp ? "bad" : "info");
    this.el.classList.toggle("lost", !on && this.link.everUp);
  }
  /** A line above the list (null: none). */
  say(text, tone = "info") {
    this.note.hidden = !text;
    this.note.className = `fm-note ${tone}`;
    this.note.textContent = text ?? "";
  }
  show(f) {
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
  selected() {
    return this.shown.find((e) => e.path === this.sel);
  }
  /** The folder drawn: crumbs, rows, the empty states, the foot. */
  render() {
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
        if (!last) b.type = "button", b.dataset.path = x.path, b.title = `Go to ${x.path || x.name}`;
        else b.setAttribute("aria-current", "location");
        b.className = `fm-crumb${i === 0 ? " root" : ""}`;
        if (i === 0) b.append(fileIcon(c.length === 1 ? "folder_open" : "folder", 16));
        b.append(Object.assign(document.createElement("span"), { className: "fm-ct", textContent: x.name }));
        return i ? [Object.assign(document.createElement("span"), { className: "fm-sep", ariaHidden: "true" }), b] : [b];
      })
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
    (this.rowEl(this.sel) ?? rows[0])?.setAttribute("tabindex", "0");
    if (focused) this.rowEl(this.sel)?.focus({ preventScroll: true });
    const tex = f.entries.filter((e) => e.kind === "tex").length;
    const dirs = f.entries.filter((e) => e.kind === "dir").length;
    if (!f.entries.length) this.emptyState("This folder is empty", f.dir ? "Nothing in it yet. Files you add here show up at once." : "Add a .tex file to it and it shows up here at once.");
    else if (!this.shown.length) this.emptyState(`Nothing here matches \u201C${q.trim()}\u201D`, "", true);
    else this.empty.hidden = true;
    const noTex = f.entries.length > 0 && !tex && !dirs && !q;
    this.el.classList.toggle("no-tex", noTex);
    let tip = this.list.querySelector(".fm-tip");
    if (noTex) {
      tip ??= this.list.insertBefore(document.createElement("div"), this.list.firstChild);
      tip.className = "fm-tip";
      tip.setAttribute("role", "presentation");
      tip.replaceChildren(html`<b>No .tex files here.</b> Only a .tex file opens; the other files are shown as they are.`);
    }
    this.foot.textContent = q ? `${this.shown.length} of ${f.entries.length} shown` : counts(f.entries);
  }
  emptyState(title, text, clear = false) {
    this.empty.hidden = false;
    this.empty.replaceChildren(html`<div class="fm-e-icon">${fileIcon(clear ? "search" : "folder_open", 40)}</div><div class="fm-e-title">${title}</div>${text ? html`<div class="fm-e-text">${text}</div>` : ""}${clear ? html`<button type="button" class="fm-btn">Clear the filter</button>` : ""}`);
    const b = this.empty.querySelector(".fm-btn");
    if (b) b.onclick = () => (this.filter.value = "", this.render(), this.filter.focus());
  }
  /** An entry's row: a link where it opens (a folder, a document, a PDF, a picture), so the browser's own ways of opening a link hold. */
  row(e, q) {
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
    sub.textContent = [e.kind === "dir" ? kindWord : "", when(e.mtime), e.kind === "dir" ? "" : sizeOf(e.size)].filter(Boolean).join(" \xB7 ");
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
  hrefOf(e) {
    if (e.kind === "dir") return this.host.dirHref(e.path);
    if (e.kind === "tex" && e.id) return this.host.docHref(e.id);
    if (e.kind === "pdf" || e.kind === "image") return this.host.rawHref(e.path);
    return null;
  }
  rowOf(e) {
    const r = e.target.closest(".fm-row");
    return r ? this.shown.find((x) => x.path === r.dataset.path) : void 0;
  }
  /** A click on a row: a link's, the browser's own with Ctrl, ⌘, Shift or the middle button (a new tab); a plain one on a folder goes into it here, on the document shown does nothing. */
  click(e) {
    const row = this.rowOf(e);
    if (!row) return;
    this.select(row.path, false);
    if (wantsTab(e) || e.button !== 0 || e.altKey) return;
    if (row.kind === "dir") {
      e.preventDefault();
      this.go(row.path);
    } else if (row.kind === "other" || row.path === this.host.current) e.preventDefault();
  }
  select(path, reveal = true) {
    this.sel = path;
    for (const r of this.list.querySelectorAll(".fm-row")) {
      const on = r.dataset.path === path;
      r.setAttribute("aria-selected", String(on));
      r.tabIndex = on ? 0 : -1;
    }
    if (reveal) this.reveal();
  }
  reveal() {
    this.list.querySelector('.fm-row[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }
  /** The selection moved `by` rows (from none: the first). */
  move(by) {
    if (!this.shown.length) return;
    const k = this.shown.findIndex((e) => e.path === this.sel);
    const next = k < 0 ? by < 0 ? this.shown.length - 1 : 0 : Math.max(0, Math.min(this.shown.length - 1, k + by));
    this.select(this.shown[next].path);
    this.rowEl(this.shown[next].path)?.focus({ preventScroll: true });
  }
  page() {
    const row = this.list.querySelector(".fm-row");
    return Math.max(1, Math.floor(this.list.clientHeight / (row?.offsetHeight || 36)) - 1);
  }
  key(e) {
    let done = true;
    if (e.key === "ArrowDown" && !e.altKey) this.move(1);
    else if (e.key === "ArrowUp" && e.altKey) this.goUp();
    else if (e.key === "ArrowUp") this.move(-1);
    else if (e.key === "Home") this.move(-Infinity);
    else if (e.key === "End") this.move(Infinity);
    else if (e.key === "PageDown") this.move(this.page());
    else if (e.key === "PageUp") this.move(-this.page());
    else if (e.key === "Backspace" || e.key === "ArrowLeft" && this.host.compact) this.goUp();
    else if (e.key === "ArrowRight" && this.host.compact && this.sel && this.selected()?.kind === "dir") this.go(this.sel);
    else if (e.key === "Escape" && this.filter.value) {
      this.filter.value = "";
      this.render();
    } else if (e.key === "/") this.focusFilter();
    else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && e.key !== " ") {
      this.filter.focus();
      this.filter.value += e.key;
      this.render();
    } else done = false;
    if (done) {
      e.preventDefault();
      e.stopPropagation();
    }
  }
};
var QuickOpen = class {
  back;
  input;
  listEl;
  files = null;
  shown = [];
  at = 0;
  hitsOf = /* @__PURE__ */ new Map();
  /** Ctrl+P again while open (the viewer prints its PDF). */
  onAgain;
  host;
  link;
  constructor(mount, host2, link2, hint = "") {
    this.host = host2;
    this.link = link2;
    this.back = document.createElement("div");
    this.back.className = "qo-back";
    this.back.hidden = true;
    this.back.append(html`<div class="qo" role="dialog" aria-modal="true" aria-label="Open a document by its name">
  <label class="qo-in">${fileIcon("search", 18)}<input type="text" placeholder="Open a .tex file by name" aria-label="A document's name" spellcheck="false" autocomplete="off" role="combobox" aria-expanded="true" aria-controls="qo-list"></label>
  <div class="qo-list" id="qo-list" role="listbox" aria-label="Documents"></div>
  <div class="qo-foot"><span><kbd>↑</kbd><kbd>↓</kbd> choose</span><span><kbd>Enter</kbd> open</span><span><kbd>${MOD}</kbd><kbd>Enter</kbd> new tab</span><span><kbd>Esc</kbd> close</span>${hint ? html`<span class="qo-hint">${hint}</span>` : ""}</div>
</div>`);
    this.input = this.back.querySelector("input");
    this.listEl = this.back.querySelector(".qo-list");
    this.back.addEventListener("mousedown", (e) => {
      if (e.target === this.back) e.preventDefault(), this.close();
    });
    this.input.addEventListener("input", () => (this.at = 0, this.render()));
    this.input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "ArrowDown") e.preventDefault(), this.step(1);
      else if (e.key === "ArrowUp") e.preventDefault(), this.step(-1);
      else if (e.key === "PageDown") e.preventDefault(), this.step(8);
      else if (e.key === "PageUp") e.preventDefault(), this.step(-8);
      else if (e.key === "Escape") e.preventDefault(), this.close();
      else if (e.key === "Enter") {
        e.preventDefault();
        const f = this.shown[this.at];
        if (f) this.open(f, e.ctrlKey || e.metaKey || e.shiftKey);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        if (this.onAgain) this.close(), this.onAgain();
      }
    });
    this.listEl.addEventListener("click", (e) => {
      const r = e.target.closest(".qo-row");
      if (!r) return;
      e.preventDefault();
      const f = this.shown[Number(r.dataset.k)];
      if (f) this.open(f, wantsTab(e));
    });
    this.listEl.addEventListener("auxclick", (e) => {
      const r = e.target.closest(".qo-row");
      if (e.button !== 1 || !r) return;
      e.preventDefault();
      const f = this.shown[Number(r.dataset.k)];
      if (f) this.open(f, true);
    });
    this.listEl.addEventListener("mousemove", (e) => {
      const r = e.target.closest(".qo-row");
      if (r && Number(r.dataset.k) !== this.at) this.at = Number(r.dataset.k), this.mark();
    });
    mount.append(this.back);
  }
  get isOpen() {
    return !this.back.hidden;
  }
  open_() {
    this.back.hidden = false;
    this.input.value = "";
    this.at = 0;
    this.render();
    this.input.focus();
    void this.link.find().then((f) => {
      this.files = f;
      this.render();
    });
  }
  close() {
    this.back.hidden = true;
  }
  toggle() {
    if (this.isOpen) this.close();
    else this.open_();
  }
  open(f, newTab) {
    this.close();
    const href = this.host.docHref(f.id);
    if (newTab) window.open(href, "_blank", "noopener");
    else if (f.path !== this.host.current) location.assign(href);
  }
  step(by) {
    if (!this.shown.length) return;
    this.at = Math.max(0, Math.min(this.shown.length - 1, this.at + by));
    this.mark();
  }
  mark() {
    this.listEl.querySelectorAll(".qo-row").forEach((r, i) => r.setAttribute("aria-selected", String(i === this.at)));
    this.listEl.querySelector('.qo-row[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    this.input.setAttribute("aria-activedescendant", `qo-r-${this.at}`);
  }
  render() {
    if (!this.files) {
      this.listEl.replaceChildren(html`<div class="qo-none">Looking for documents…</div>`);
      return;
    }
    const q = this.input.value;
    const ms = rank(q, this.files.map((f) => f.path), 60);
    const byPath = new Map(this.files.map((f) => [f.path, f]));
    this.shown = ms.map((m) => byPath.get(m.path));
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
        r.href = this.host.docHref(f.id);
        r.tabIndex = -1;
        r.setAttribute("role", "option");
        r.setAttribute("aria-selected", String(k === this.at));
        if (f.path === this.host.current) r.setAttribute("aria-current", "page");
        r.append(fileIcon("tex", 18), html`<span class="qo-name">${marked(f.path.slice(cut), hits, cut)}</span><span class="qo-dir">${cut ? marked(f.path.slice(0, cut - 1), hits, 0) : ""}</span>${f.path === this.host.current ? html`<span class="qo-here">this tab</span>` : ""}`);
        return r;
      })
    );
    this.input.setAttribute("aria-activedescendant", `qo-r-${this.at}`);
  }
};
var FILES_CSS = `
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
.fm-head, .fm-row { display: grid; grid-template-columns: 28px minmax(0, 1fr) minmax(130px, 170px) 92px 72px 0px; align-items: center; column-gap: 12px; padding: 0 14px; }
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

// viewer/src/print.ts
var OVERLEAF = `
  --ph-bar: var(--toolbar-alt-bg-color, #2f3a4c); --ph-fg: var(--content-primary-dark, #f4f5f6); --ph-fg2: var(--neutral-40, #afb5c0);
  --ph-border: var(--neutral-70, #495365); --ph-hover: var(--toolbar-btn-hover-bg-color, #495365);
  --ph-stage: var(--pane, var(--pdf-bg, #2f3a4c)); --ph-menu: var(--bg-dark-primary, #1b222c);
  --ph-accent: #2c5aa0; --ph-accent-hover: #3567b3; --ph-accent-fg: #fff;
  --ph-danger: var(--content-danger-dark, #e36d66); --ph-warning: var(--content-warning-dark, #de8014); --ph-info: var(--content-info-dark, #97b6e5);
  --ph-link: var(--link-ui-dark, #97b6e5); --ph-progress: #f58f2d;
  --ph-radius: var(--border-radius-base, 4px); --ph-pill: var(--border-radius-full, 9999px);
  --ph-font: var(--font-sans, "Noto Sans", system-ui, sans-serif); --ph-size: 13px; --ph-bar-h: 32px;
`;
var OVERLEAF_LIGHT = `
  --ph-bar: var(--toolbar-alt-bg-color, #fff); --ph-fg: var(--content-primary, #1b222c); --ph-fg2: var(--content-secondary, #495365);
  --ph-border: var(--border-divider, #e7e9ee); --ph-hover: var(--toolbar-btn-hover-bg-color, #f4f5f6);
  --ph-stage: var(--pane, var(--pdf-bg, #f4f5f6)); --ph-menu: var(--bg-light-primary, #fff);
  --ph-danger: var(--content-danger, #b83a33); --ph-warning: var(--content-warning, #8f5514); --ph-info: var(--content-info, #366cbf);
  --ph-link: var(--link-ui, #366cbf); --ph-accent: #1d3e72; --ph-accent-hover: #16325d;
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
.ib:hover, .ib[aria-expanded="true"], .ib[aria-pressed="true"] { background: var(--ph-hover); color: var(--ph-fg); }
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
/* (squeezed: its detail goes first, then its words end in an ellipsis; the leading word kept) */
.status b { flex: 0 1 auto; min-width: 4.5ch; font-weight: 400; color: var(--ph-fg); overflow: hidden; text-overflow: ellipsis; }
#stmore { flex: 0 1000 auto; min-width: 0; overflow: hidden; }
/* (a fixed slot: its words change as builds come, the buttons after it never move; a narrow bar squeezes it, as wide as the bar, not its words) */
.bar > .status { flex: 0 1 auto; width: calc(26ch + 12px); min-width: calc(4.5ch + 12px); }
.status.err b { color: var(--ph-danger); }
.pageno { display: inline-flex; align-items: center; gap: 4px; color: var(--ph-fg2); font-variant-numeric: tabular-nums; white-space: nowrap; }
/* (the count three digits wide at least, as the page box: panel.ts's nav) */
.pageno #pgn { display: inline-block; min-width: 3ch; }
.pageno input { width: 3ch; height: 22px; box-sizing: content-box; padding: 0 2px; padding: 0; text-align: center; font: inherit; color: var(--ph-fg); background: transparent;
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); }
.zoomb { padding: 0 2px 0 8px; }
#zs { display: none; }
.probs { font-weight: 600; font-variant-numeric: tabular-nums; }
.probs.error { color: var(--ph-danger); } .probs.warning { color: var(--ph-warning); }
.progress { position: absolute; left: 0; right: 0; bottom: -1px; height: 2px; overflow: hidden; opacity: 0; transition: opacity .2s; pointer-events: none; }
.progress.on { opacity: 1; }
/* (the carriage runs only while the line shows, and by transform: a hidden one moving by left laid the page out every frame, scrolling included) */
.progress::after { content: ""; position: absolute; top: 0; bottom: 0; left: 0; width: 30%; background: var(--ph-progress); transform: translateX(-100%); }
.progress.on::after { animation: phx-carriage 1.2s ease-in-out infinite; }
@keyframes phx-carriage { from { transform: translateX(-100%); } to { transform: translateX(333.4%); } }
.find { display: inline-flex; align-items: center; gap: 6px; }
.find input { width: 170px; height: 24px; padding: 0 8px; font: inherit; color: var(--ph-fg); background: var(--ph-menu);
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); outline: none; }
.find input:focus { border-color: var(--ph-link); }
#findn { color: var(--ph-fg2); min-width: 48px; font-variant-numeric: tabular-nums; }
#findn.none { color: var(--ph-danger); }
/* narrower: what gives way, in order: the zoom's long label, find's box, then (560) the buttons the \u22EF menu also has, then the page box beside an open find */
.bar > :not(.status):not(.grow) { flex: none; }
.chg-btn { gap: 5px; padding: 0 7px; }
.chg-btn .chg-glyph { font: 600 15px ui-monospace, "DM Mono", monospace; line-height: 1; }
.chg-btn.on { background: var(--ph-hover); color: var(--ph-fg); box-shadow: inset 0 0 0 1px var(--ph-border); }
.bar > .grow { flex: 1 1 0; min-width: 0; }
@container (max-width: 820px) { #zl { display: none; } #zs { display: inline; } }
@container (max-width: 760px) {
  /* (find open: Compare and Download step aside, to the \u22EF menu, which has them) */
  .bar:has(.find:not([hidden])) :is(#b-changes, #pdf) { display: none; }
  .bar:has(.find:not([hidden])) #x-compare.narrow:not([hidden]) { display: flex !important; }
}
@container (max-width: 700px) { .find input { width: 110px; } #findn { min-width: 0; } }
@container (max-width: 560px) {
  .wide { display: none !important; }
}
@container (max-width: 460px) {
  .bar:has(.find:not([hidden])) .pageno { display: none; }
  .find input { width: 90px; }
  .bar { gap: 2px; }
}
@container (max-width: 360px) { .find input { width: 72px; } .bar { padding: 0 4px; } }
@container (min-width: 561px) { .narrow { display: none !important; } }

/* menus and popovers: plain lists in the host's menu colour; above a
   feature tip (30): a menu is what the user just opened, and the welcome
   tip's card covered the record menu's Clean rebuild */
.menu { position: absolute; z-index: 31; min-width: 230px; max-width: calc(100% - 12px); padding: 4px 0; text-align: left;
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
.timing { width: 340px; padding: 12px 14px; font-variant-numeric: tabular-nums; }
.timing .head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-bottom: 8px; }
.timing .head span { color: var(--ph-fg2); }
/* (Did you know: under the opening's step, quiet) */
.opening .steps .hint { margin: 10px auto 0; color: var(--ph-fg2); font-size: 12px; line-height: 1.4; max-width: 420px; }
/* (the first build beside the edits: bars as long as their times) */
.timing .tbars { padding: 4px 12px 8px; display: grid; gap: 4px; }
.timing .tb { display: grid; grid-template-columns: 88px 1fr auto; align-items: center; gap: 8px; font-variant-numeric: tabular-nums; }
.timing .tb .k { color: var(--ph-fg2); }
.timing .tb .track { height: 8px; border-radius: 4px; background: var(--ph-hover, rgb(0 0 0 / 6%)); overflow: hidden; }
.timing .tb .track i { display: block; height: 100%; border-radius: 4px; background: var(--ph-fg2); }
.timing .tb .track i.edit, .timing .tb .track i.median { background: var(--ph-progress, #f58f2d); }
.timing .tb.x { display: block; color: var(--ph-fg2); font-size: 11px; }
.timing .head b { font-size: 20px; font-weight: 600; }
.timing .row { display: grid; grid-template-columns: 1fr auto; gap: 3px 12px; padding: 7px 0; border-top: 1px solid var(--ph-border); }
.timing .row .k { color: var(--ph-fg2); }
.timing .row .v { font-weight: 600; text-align: right; }
.timing .row .note { grid-column: 1 / -1; font-size: 11px; line-height: 1.35; color: var(--ph-fg2); }
.timing .row .tbar { grid-column: 1 / -1; display: flex; gap: 2px; height: 6px; margin-top: 2px; }
.timing .row .tbar i { min-width: 3px; border-radius: 2px; }
.timing .row .legend { grid-column: 1 / -1; display: flex; flex-wrap: wrap; gap: 2px 10px; font-size: 11px; color: var(--ph-fg2); }
.timing .row .legend span { display: inline-flex; align-items: center; gap: 4px; }
.timing .row .legend i { width: 8px; height: 8px; border-radius: 2px; }
.timing .c0 { background: var(--ph-progress); } .timing .c1 { background: var(--ph-accent); } .timing .c2 { background: var(--ph-fg2); } .timing .c3 { background: var(--ph-border); }
.timing .foot { margin-top: 8px; color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.timing .foot.err { color: var(--ph-danger); }
.timing .copydbg { margin: 8px 0 0; font-size: 12px; }

/* problems: Overleaf's log entries, in a popover under the bar */
/* (the problems' filters: a kind a pill, on or off) */
.logs .kinds { display: flex; flex-wrap: wrap; gap: 4px; padding: 6px 12px 8px; border-bottom: 1px solid var(--ph-border); }
.logs .kinds button { all: unset; cursor: pointer; padding: 2px 9px; border: 1px solid var(--ph-border); border-radius: var(--ph-pill); font-size: 12px; font-variant-numeric: tabular-nums; }
.logs .kinds button[aria-pressed=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.logs .kinds button:hover { background: var(--ph-hover); }
.logs .kinds button[aria-pressed=true]:hover { background: var(--ph-accent); }
.logs { right: 6px; top: 4px; width: min(460px, calc(100% - 12px)); max-height: calc(100% - 8px); overflow: auto; padding: 0; }
.logs > header { all: unset; display: flex; align-items: center; gap: 8px; padding: 8px 8px 8px 14px; position: sticky; top: 0; z-index: 1;
  background: var(--ph-menu); border-bottom: 1px solid var(--ph-border); font-weight: 600; }
.diag { display: block; padding: 9px 14px 10px; border-left: 3px solid var(--sev, var(--ph-fg2)); border-bottom: 1px solid var(--ph-border); }
.diag[data-line] { cursor: pointer; } .diag[data-line]:hover { background: var(--ph-hover); }
.diag.more { display: flex; gap: 8px; }
.diag.more button { font: inherit; color: var(--ph-accent, inherit); background: none; border: 1px solid var(--ph-border); border-radius: 5px; padding: 2px 8px; cursor: pointer; }
.diag.error { --sev: var(--ph-danger); } .diag.warning { --sev: var(--ph-warning); } .diag.info { --sev: var(--ph-info); }
.diag .where { float: right; margin-left: 10px; color: var(--ph-link); font-size: 12px; }
.diag .text { font-weight: 600; overflow-wrap: anywhere; }
.diag .detail, .folded .detail { margin: 6px 0 0; padding: 0; font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; color: var(--ph-fg2);
  white-space: pre-wrap; overflow-wrap: anywhere; max-height: 12em; overflow: auto; font-weight: 400; }
.folded { margin-top: 6px; font-size: 12px; font-weight: 400; }
.folded summary { cursor: pointer; color: var(--ph-fg2); }
.diags .phx-inline { margin: 0; padding: 0; list-style: none; }
/* a problem's Copy for an AI assistant: its row's top right corner, clear when the row is pointed at or focused */
.diag, .diags .phx-inline > li { position: relative; padding-right: 40px; }
.copyai { all: unset; position: absolute; top: 6px; right: 8px; display: inline-flex; padding: 4px; border-radius: var(--ph-radius, 4px); color: var(--ph-fg2); opacity: .45; cursor: pointer; }
.diag:hover .copyai, .phx-inline > li:hover .copyai, .copyai:focus-visible { opacity: 1; }
.copyai:hover { background: var(--ph-hover); color: var(--ph-fg); }
.copyai:focus-visible { outline: 2px solid var(--ph-link); }
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
/* (Diff | Current: which pages are shown, the one shown filled; the diff's style on its half) */
.diffbar .seg { display: inline-flex; border: 1px solid var(--ph-accent); border-radius: var(--ph-pill); overflow: hidden; }
.diffbar .seg button { border-radius: 0; padding: 2px 10px; color: var(--ph-accent); font-weight: 600; }
.diffbar .seg button[aria-pressed=true] { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.diffbar .seg button[aria-pressed=true]:hover:not(:disabled) { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.diffbar .seg .chev { font-weight: 400; margin-left: 2px; }
.diffset .row.bars button { padding: 1px 10px; }
.diffset .row.bars button[aria-checked=true] { background: var(--ph-accent); border-color: var(--ph-accent); color: var(--ph-accent-fg); }
.diffset .row.bars button:disabled { opacity: .45; cursor: default; }
.diffbar button.cur { background: var(--ph-hover); color: var(--ph-fg); }
.diffbar .sep { width: 1px; align-self: stretch; margin: 2px; background: var(--ph-border); }
.diffset { display: none; left: 50%; transform: translateX(-50%); top: 4px; width: 320px; max-height: calc(100% - 12px); overflow: auto; padding: 12px; font-size: 12px; }
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
/* (a rebuild's opening up: the last build's pages hidden under it until the dive) */
.win.reopening .viewer { visibility: hidden; }
/* (the first page's sheet up: the stage holds still under it, its width kept, so nothing scrolls behind the sheet) */
.win.sheet-up .stage, .win.docked.sheet-up .stage { overflow: hidden; scrollbar-gutter: stable; }
.slot { margin: 12px auto; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); container-type: inline-size; }
/* (a page the build has not shipped alone yet: it comes at the build's end, said, not a blank sheet) */
.slot.later::before { content: "Drawn when the build ends"; position: absolute; inset: 0; display: grid; place-items: center; color: #8a8f98; font: 500 clamp(9px, 2.6cqw, 18px)/1.3 var(--ph-font); }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
/* (a page streamed while the build runs and drawn without some of what it draws with: a quiet mark, gone when the build's page replaces it) */
.slot.preview::after { content: "Preview: final colours and shading when the build ends"; position: absolute; left: 50%; bottom: 6px; transform: translateX(-50%); max-width: calc(100% - 12px); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding: 1px 8px; border-radius: 999px; background: rgb(20 24 31 / 55%); color: #fff; font: 500 clamp(8px, 1.7cqw, 12px)/1.5 var(--ph-font); pointer-events: none; }
.win.dark-pdf .slot, .win.docked.pdf-dark .slot { background: ${DARK_PAGE}; }
.win.dark-pdf .slot canvas, .win.docked.pdf-dark .slot canvas { filter: ${DARK_PDF_FILTER}; }
/* (Page colours: the pages' white to the page's colour, black to the text's, by a colour matrix (Panel.pageApply); Dark PDF wins) */
.win > svg.phx-theme { position: absolute; width: 0; height: 0; overflow: hidden; pointer-events: none; }
.win.paged .slot canvas, .win.paged .peek .pv canvas { filter: url(#phx-page-colours); }
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
/* the reading aid (wasp.ts): bands over the lines, blended: lighten on a white page (black ink takes the colour, the paper stays
   white), darken on Dark PDF's (its light ink takes a light colour, the grey stays); a stop "wi" leaves the ink as it is; the
   colours the panel's variables (Panel.waspApply: the preset or the user's, at the strength chosen), Classic's at full without them */
.phx-wasp { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; mix-blend-mode: lighten; }
.phx-wasp .wi { stop-color: #000; }
.phx-wasp .w0 { stop-color: var(--ph-wasp0, #1747d6); }
.phx-wasp .w1 { stop-color: var(--ph-wasp1, #d0202a); }
.win.dark-pdf .phx-wasp, .win.docked.pdf-dark .phx-wasp, .win.paged-dark .phx-wasp { mix-blend-mode: darken; }
.win.dark-pdf .phx-wasp .wi, .win.docked.pdf-dark .phx-wasp .wi, .win.paged-dark .phx-wasp .wi { stop-color: #fff; }
.win.dark-pdf .phx-wasp .w0, .win.docked.pdf-dark .phx-wasp .w0, .win.paged-dark .phx-wasp .w0 { stop-color: var(--ph-wasp0-dark, #6e98ff); }
.win.dark-pdf .phx-wasp .w1, .win.docked.pdf-dark .phx-wasp .w1, .win.paged-dark .phx-wasp .w1 { stop-color: var(--ph-wasp1-dark, #ff7b72); }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
/* (out of the layout while a zoom gesture runs, Panel.zoomLay: its glyphs laid out again at every size, most of a frame's layout) */
.zooming .phx-text, .phx-text[data-hold] { display: none; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
/* (a link under the pointer, viewer.ts: the hand, over the text's cursor) */
.slot.on-link, .slot.on-link .phx-text text { cursor: pointer; }
/* a link hovered: where it goes, in a small window (viewer.ts peekShow) */
.peek { position: absolute; z-index: 4; width: min(440px, calc(100% - 16px)); text-align: left; background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 24px rgb(0 0 0 / 22%); overflow: hidden; cursor: default; font: var(--ph-size)/1.4 var(--ph-font); }
.peek .pv { position: relative; height: 250px; overflow: auto; overscroll-behavior: contain; background: #fff; touch-action: pan-x pan-y; }
.peek .pv canvas { display: block; position: absolute; left: 0; top: 0; }
.peek .pv .pz { pointer-events: none; }
.peek.zoomed .pv { cursor: grab; }
.peek.zoomed .pv.grabbing { cursor: grabbing; }
.peek .pf .zm { display: inline-flex; align-items: center; gap: 1px; margin-left: auto; }
.peek .pf .zm button { all: unset; cursor: pointer; display: inline-grid; place-items: center; min-width: 20px; height: 20px; border-radius: var(--ph-radius); color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.peek .pf .zm button svg { width: 14px; height: 14px; }
.peek .pf .zm .zl { min-width: 38px; font-size: 11.5px; }
.peek .pf .zm button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); color: var(--ph-fg); }
.peek .pf .zm button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.peek.cite .pf .zm, .peek.url .pf .zm, .peek.wait .pf .zm { display: none; }
.peek .pf .zm + .go { margin-left: 4px; }
.peek .pv .at { position: absolute; left: 0; width: 3px; height: 18px; background: var(--ph-accent); border-radius: 0 2px 2px 0; }
.peek.wait .pv canvas, .peek.wait .pv .at { visibility: hidden; }
.peek .pu { display: none; padding: 8px 10px 0; word-break: break-all; color: var(--ph-link, var(--ph-accent)); }
.peek.url .pv { display: none; }
.peek.url .pu { display: block; }
.peek .pf { display: flex; justify-content: space-between; gap: 12px; padding: 5px 10px; font-size: 12px; color: var(--ph-fg2); border-top: 1px solid var(--ph-border); }
.peek.url .pf { border-top: 0; }
.peek .pf .pin { all: unset; cursor: pointer; display: inline-flex; align-items: center; margin-left: 8px; color: var(--ph-fg2); }
.peek .pf .pin svg { width: 14px; height: 14px; }
.peek .pf .pin:hover { color: var(--ph-fg); }
.peek .pf .pin:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 2px; border-radius: 2px; }
.peek .pf { align-items: center; }
.peek .pf .go { all: unset; margin-left: auto; cursor: pointer; padding: 1px 10px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); color: var(--ph-fg); }
.peek .pf .go:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.peek .pf .go:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 2px; }
.peek .pf .go[hidden] { display: none; }
/* a window kept open (viewer.ts peekPin): over the pages, where it was put, a bar to move it by */
.peek.pinned { z-index: 5; box-shadow: 0 10px 32px rgb(0 0 0 / 28%); resize: both; display: flex; flex-direction: column; min-width: 240px; min-height: 120px; max-width: calc(100% - 8px); }
/* (resized: what it shows fills it, scrolling inside) */
.peek.pinned .pv { flex: 1 1 auto; height: auto; min-height: 60px; }
.peek.pinned.cite .pc { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; }
.peek.pinned .pc .cs { flex: 1 1 auto; max-height: none; min-height: 0; }
/* (the corner kept for resizing: the footer's button clear of it) */
.peek.pinned .pf { flex: none; padding-right: 28px; }
.peek.pinned .pf .pin { display: none; }
.peek .ph { display: flex; align-items: center; gap: 8px; padding: 4px 6px 4px 10px; cursor: move; user-select: none; touch-action: none; font-size: 12px; font-weight: 600; color: var(--ph-fg); background: var(--ph-hover, rgb(0 0 0 / 5%)); border-bottom: 1px solid var(--ph-border); }
.peek .ph .t { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.peek .ph .x { all: unset; cursor: pointer; width: 20px; height: 20px; display: grid; place-items: center; border-radius: var(--ph-radius); color: var(--ph-fg2); font-size: 15px; }
.peek .ph .x:hover { background: var(--ph-border); color: var(--ph-fg); }
.peek .pc { display: none; }
.peek.cite .pv { display: none; }
.peek.cite .pc { display: block; }
.peek .pc .mh { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 6px 12px; font-size: 12px; color: var(--ph-fg2); border-bottom: 1px solid var(--ph-border); }
.peek .pc .mh button { all: unset; cursor: pointer; padding: 2px 10px; border-radius: var(--ph-pill); border: 1px solid var(--ph-border); color: var(--ph-fg); }
.peek .pc .mh button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.peek .pc .cs { position: relative; max-height: 300px; overflow-y: auto; overscroll-behavior: contain; }
/* an entry, the same one or many: the label, title and key; who, when and where; its links */
.peek .pc .ce { padding: 7px 12px 7px 9px; border-left: 3px solid transparent; }
.peek .pc .ce + .ce { border-top: 1px solid var(--ph-border); }
.peek .pc .ce.here { border-left-color: var(--ph-accent); background: color-mix(in srgb, var(--ph-accent) 7%, transparent); }
.peek .pc .hd { display: flex; align-items: baseline; gap: 6px; min-width: 0; }
.peek .pc .hd .lab { flex: none; font-size: 12px; font-weight: 600; color: var(--ph-accent); }
/* (the title and the line under it wrap, whole: cut at one line, the card hid the
   title, the one thing it is for; the list scrolls past 300px) */
.peek .pc .hd .ti { flex: 1; min-width: 0; font-size: 13px; font-weight: 600; line-height: 1.3; overflow-wrap: anywhere; }
.peek .pc .hd code { flex: none; font: 10.5px/1.3 ui-monospace, "DM Mono", monospace; padding: 0 4px; border-radius: 3px; background: var(--ph-hover, rgb(0 0 0 / 6%)); color: var(--ph-fg2); }
.peek .pc .mt { margin-top: 1px; font-size: 12px; line-height: 1.35; color: var(--ph-fg2); overflow-wrap: anywhere; }
.peek .pc .lk { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0 5px; margin-top: 2px; font-size: 11.5px; }
.peek .pc .lk > i { font-style: normal; color: var(--ph-fg2); }
.peek .pc .lk button { all: unset; cursor: pointer; color: var(--ph-link, var(--ph-accent)); }
.peek .pc .lk button:hover { text-decoration: underline; }
.peek .pc .lk button.u { overflow-wrap: anywhere; text-decoration: underline; text-decoration-color: color-mix(in srgb, currentColor 40%, transparent); }
.peek .pc .lk button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; border-radius: 2px; }
/* where a jump landed, lit for a moment (viewer.ts flash): only its opacity animates */
.slot .phx-flash { position: absolute; z-index: 2; pointer-events: none; border-radius: 3px; background: color-mix(in srgb, var(--ph-accent) 22%, transparent); box-shadow: 0 0 0 1.5px color-mix(in srgb, var(--ph-accent) 55%, transparent); animation: phx-flash 1.5s ease-out forwards; }
@keyframes phx-flash { 0%, 35% { opacity: 1; } 100% { opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .slot .phx-flash { animation-duration: 2.5s; } }
/* Back, after a link followed: over the pages' bottom left corner */
.pback { position: absolute; z-index: 4; left: 12px; bottom: 12px; display: flex; align-items: stretch; border-radius: var(--ph-pill); background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); box-shadow: 0 3px 12px rgb(0 0 0 / 18%); font: 12.5px/1 var(--ph-font); }
.pback[hidden] { display: none; }
.pback button { all: unset; cursor: pointer; padding: 7px 10px; }
.pback .go { display: flex; gap: 6px; align-items: center; padding-left: 12px; font-weight: 600; }
.pback .go .ar { font-size: 14px; }
.pback .x { color: var(--ph-fg2); border-left: 1px solid var(--ph-border); padding: 7px 9px; }
.pback { overflow: visible; }
.pback .more, .pback .fw { border-left: 1px solid var(--ph-border); color: var(--ph-fg2); font-weight: 600; }
.pback .more[hidden], .pback .fw[hidden] { display: none; }
.pback.noback .go { display: none; }
.pback.noback .fw { border-left: 0; padding-left: 12px; }
.pback .go { border-radius: var(--ph-pill) 0 0 var(--ph-pill); }
.pback .x { border-radius: 0 var(--ph-pill) var(--ph-pill) 0; }
.pback .stops { position: absolute; left: 0; bottom: calc(100% + 6px); width: max-content; min-width: 300px; max-width: min(460px, 80vw); max-height: 260px; overflow-y: auto; padding: 4px 0; background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 6px 20px rgb(0 0 0 / 20%); }
.pback .stops[hidden] { display: none; }
.pback .stops button { display: grid; grid-template-columns: 1fr; gap: 1px; line-height: 1.35; width: 100%; box-sizing: border-box; padding: 6px 12px; text-align: left; font-size: 12.5px; }
.pback .stops button b { font-weight: 600; white-space: nowrap; }
.pback .stops button b small { margin-left: 5px; font-weight: 400; color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.pback .stops button span { color: var(--ph-fg2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.pback button:hover { background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.pback button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: -2px; }
.win.dark-pdf .peek .pv canvas, .win.docked.pdf-dark .peek .pv canvas { filter: ${DARK_PDF_FILTER}; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: transparent; }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.stage.offline .slot { opacity: .4; pointer-events: none; }
.slot .mark { position: absolute; pointer-events: none; border-radius: 2px; background: var(--ph-mark-fill, rgb(255 214 0 / 45%)); mix-blend-mode: multiply;
  box-shadow: 0 0 0 1px var(--ph-mark-line, rgb(232 172 0 / 80%)); transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; }
/* (the text selected on the pages, one block a line: viewer.ts drawSelection; a selection's blue alone, never the marker's outline) */
.slot .mark.seltext { background: rgb(0 100 255 / 25%); mix-blend-mode: normal; border-radius: 0; box-shadow: none; }
/* (on a dark sheet the marker lights the text: multiplied, it went black on black) */
.win.paged-dark .slot .mark:not(.seltext):not(.fnd), .win.dark-pdf .slot .mark:not(.seltext):not(.fnd), .win.docked.pdf-dark .slot .mark:not(.seltext):not(.fnd) { mix-blend-mode: screen; }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); box-shadow: none; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); box-shadow: 0 0 0 1px rgb(150 100 0 / 90%); }
.slot .culprit { position: absolute; pointer-events: none; }
/* (the marks along the pages' scrollbar: the problems' places and the diff's changes) */
.smarks { position: absolute; right: 0; top: 0; bottom: 0; width: 8px; z-index: 4; pointer-events: none; }
.smarks[hidden] { display: none; }
.smarks i { position: absolute; right: 1px; width: 6px; height: 3px; margin-top: -1px; border-radius: 1px; pointer-events: auto; cursor: pointer; background: var(--ph-warning, #c77700); transition: transform .1s; }
/* (a target larger than the tick: it is 3 px tall) */
.smarks i::before { content: ""; position: absolute; inset: -4px -3px; }
.smarks i:hover { transform: scale(1.5, 2); }
.smarks i.error { background: var(--ph-danger, #d93025); }
.smarks i.change { background: var(--ph-accent, #1d3e72); }
.smarks i.rv-highlight { background: #c9a800; } .smarks i.rv-note { background: #2f6fd1; } .smarks i.rv-comment { background: #e08a1e; }
.smarks i.rv-change { background: #1a7f37; } .smarks i.rv-dim { opacity: .4; }
.phx-card[data-key] { cursor: pointer; }
/* (an overfull box's bar at its line's end, where pdflatex's draft rule goes, drawn by the viewer only, in the warning's
   amber, as its tooltip and the underfull's outline: draft black read as a broken block, and vanished on a dark page; an underfull's hollow) */
.slot .culprit.rule::after { display: none; }
.slot .culprit.rule.overfull { background: rgb(199 119 0 / 85%); border-radius: 1px; }
.slot .culprit.rule.underfull { box-shadow: inset 0 0 0 1.5px #c77700; }
/* (a culprit hovered: its problem's message, a small tooltip under the line) */
.ctip { position: absolute; z-index: 8; max-width: min(420px, calc(100% - 16px)); display: flex; gap: 6px; align-items: flex-start; padding: 5px 9px;
  background: var(--ph-menu); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: var(--ph-radius);
  box-shadow: 0 3px 12px rgb(0 0 0 / 25%); font-size: 12px; line-height: 1.35; pointer-events: none; overflow-wrap: anywhere; }
.ctip .icon { margin-top: 1px; }
.ctip.error .icon { color: var(--ph-danger); } .ctip.warning .icon { color: var(--ph-warning); }
.ctip .txt { min-width: 0; text-align: left; }
.ctip code, .ctip .ex { font: 11.5px/1.4 ui-monospace, "DejaVu Sans Mono", monospace; }
.ctip .ex { margin: 3px 0 0; white-space: pre; overflow: hidden; text-overflow: ellipsis; opacity: .85; }
.ctip .ex mark { background: none; color: inherit; font-weight: 600; text-decoration: underline wavy var(--ph-danger); text-underline-offset: 3px; }
.ctip.warning .ex mark { text-decoration-color: var(--ph-warning); }
.ctip .at { margin-top: 2px; font-size: 11px; color: var(--ph-fg2); }
.stage.onculprit, .stage.onculprit * { cursor: pointer !important; }
/* (a problem's culprit: a wavy line along its glyphs' foot, as an editor underlines one; no fill, the text stays readable) */
.slot .culprit::after { content: ""; position: absolute; left: 0; right: 0; bottom: -2px; height: 3px; background: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23d93025' stroke-width='1'/%3E%3C/svg%3E") repeat-x left bottom / 6px 3px; }
.slot .culprit.warning::after { background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23c77700' stroke-width='1'/%3E%3C/svg%3E"); }
.slot .peer { position: absolute; pointer-events: none; border-radius: 2px; background: hsl(var(--hue) 70% 50% / 22%); box-shadow: inset 0 -1.5px 0 hsl(var(--hue) 70% 45%); }
/* (the name as large as the line it marks, so it zooms with the page; readable when far out, never a headline when close) */
.slot .peer-tag { width: auto; height: auto; transform: translateY(-100%); padding: 0 .4em; border-radius: .25em .25em .25em 0; background: hsl(var(--hue) 70% 42%);
  box-shadow: none; color: #fff; font: 600 clamp(7px, calc(var(--fs, .012) * 100cqw), 22px)/1.35 var(--ph-font); white-space: nowrap; z-index: 2; }
.slot .peer-tag[data-file] { pointer-events: auto; cursor: pointer; }
.slot .peer-tag[data-file]:hover { background: hsl(var(--hue) 70% 34%); }

/* strips over the pages: plain, in the host's colours */
/* (over the pages' scroll, not in it: in it, a strip went with the pages when they scrolled sideways) */
.stagebox { position: relative; flex: 1; min-height: 0; min-width: 0; display: flex; flex-direction: column; }
.strips { position: absolute; top: 0; left: 0; right: 0; z-index: 2; pointer-events: none; }
.strip { pointer-events: auto; display: none; padding: 6px 12px; text-align: left; font-size: 12px;
  background: var(--ph-menu); color: var(--ph-fg); border-bottom: 1px solid var(--ph-border); border-left: 3px solid var(--sev, var(--ph-warning)); }
.stage.stale ~ .strips .banner, .approx.on { display: block; }
/* the main file asked (a first build on a guess): over the opening's sheet too, until answered */
.strips:has(.mainask.on) { z-index: 4; }
.mainask { --sev: var(--ph-info); }
.mainask.on { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; }
.mainask .q { font-weight: 600; }
.mainask .why { color: var(--ph-fg2); flex-basis: 100%; }
.mainsel { font: inherit; color: var(--ph-fg); background: var(--ph-bg, transparent); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 1px 4px; max-width: 100%; }
.mainask .keep { all: unset; cursor: pointer; padding: 2px 10px; border-radius: var(--ph-pill); background: var(--ph-accent); color: var(--ph-accent-fg); font-weight: 600; }
.mainask .keep:focus-visible { outline: 2px solid var(--ph-fg); outline-offset: 2px; }
.card .mainrow { align-items: center; color: var(--ph-fg2); }
.gone { --sev: var(--ph-danger); font-weight: 600; }
.stage.offline ~ .strips .gone { display: flex; align-items: baseline; gap: 8px; }
.gone .x { margin-left: auto; flex: none; cursor: pointer; border: 0; background: none; color: inherit; font: inherit; padding: 0 2px; }
.offpill { display: none; position: absolute; top: 8px; right: 14px; z-index: 3; padding: 1px 8px; border-radius: var(--ph-pill); font-size: 12px;
  background: var(--ph-menu); color: var(--ph-danger); border: 1px solid var(--ph-border); pointer-events: none; }
.win.offline-quiet .offpill { display: block; }

/* before any page: the first page's sheet, the opening playing on it, the step under it */
.opening { position: absolute; z-index: 3; pointer-events: none; left: 0; top: 0; width: 0; height: 0;
  transition: left .3s ease, top .3s ease, width .3s ease, height .3s ease; }
.opening .sheet { position: absolute; inset: 0; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 18%); container-type: inline-size; }
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
@keyframes phx-slide { from { transform: translateX(-100%); } to { transform: translateX(333.4%); } }
.win.dark-pdf .steps, .win.docked.pdf-dark .steps { color: #a3a9b4; }
.win.dark-pdf .steps .what, .win.docked.pdf-dark .steps .what { color: #e4e6ea; }
.win.dark-pdf .steps .track, .win.docked.pdf-dark .steps .track { background: #3d4047; }
.opening.in .steps { opacity: 0; }
/* (an opening that says the steps itself, the animation's own part) */
.opening.owns .steps { display: none; }
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
.toast.msg .t { align-items: flex-start; } .toast.msg .t span { white-space: normal; overflow-wrap: anywhere; }
.toast.msg .t .icon { margin-top: 2px; }
.toast .d { color: var(--ph-fg2); font-size: 12px; overflow-wrap: anywhere; }
.toast.err { border-left: 3px solid var(--ph-danger); }
.toast .track { position: relative; height: 3px; margin-top: 2px; border-radius: 2px; overflow: hidden; background: var(--ph-border); }
.toast .track i { position: absolute; inset: 0 auto 0 0; background: var(--ph-progress); transition: width .25s; }
.toast .track.unknown i { width: 30% !important; animation: phx-slide 1.2s ease-in-out infinite; }

/* feature tips: a popover with an arrow at the control it is about; under the dialogs (a tip left showing never covers Settings) */
.tip { position: absolute; z-index: 24; width: 270px; max-width: calc(100% - 16px); padding: 12px 14px 10px; text-align: left;
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

/* a tip's demo: a little stage looping what the feature does (4 s; the compare's 6 s), in the host's colours; a page is paper */
.tip .demo { position: relative; height: 84px; margin: 8px 0 10px; overflow: hidden; border-radius: var(--ph-radius);
  background: var(--ph-stage); border: 1px solid var(--ph-border); font: 10px/1.3 var(--ph-font); color: var(--ph-fg); }
.tip .demo .mbar { position: absolute; inset: 0 0 auto; height: 20px; display: flex; align-items: center; gap: 4px; padding: 0 6px;
  background: var(--ph-bar); border-bottom: 1px solid var(--ph-border); }
.tip .demo .pg { position: absolute; background: #fff; box-shadow: 0 1px 3px rgb(0 0 0 / 25%); padding: 6px 7px; overflow: hidden; }
.tip .demo .pg i { display: block; height: 3px; margin: 0 0 4px; border-radius: 1px; background: #c3c8d0; }
.tip .demo .mmenu { position: absolute; display: flex; flex-direction: column; padding: 3px; opacity: 0; z-index: 1;
  background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 3px 10px rgb(0 0 0 / 25%); }
.tip .demo .mmenu span { display: flex; align-items: center; gap: 4px; padding: 2px 6px; border-radius: 2px; white-space: nowrap; }
.tip .demo .mmenu b { margin-left: auto; padding-left: 6px; }

/* compare: "good" struck out, "excellent" added, then the navigator */
.tip .demo.cmp { height: auto; padding: 10px 12px 30px; background: #fff; color: #1b222c; font: 13px/1.5 "Noto Serif", Georgia, serif; }
.tip .cmp .del { animation: phx-del 6s ease-in-out infinite; }
.tip .cmp .add { display: inline-block; max-width: 0; overflow: hidden; vertical-align: bottom; white-space: nowrap; color: #1d4ed8;
  text-decoration: underline wavy #1d4ed8; text-underline-offset: 3px; animation: phx-add 6s ease-in-out infinite; }
.tip .cmp .tag { position: absolute; right: 8px; bottom: 7px; padding: 3px 8px; border-radius: 9px; font: 600 11px/1 var(--ph-font);
  color: #fff; background: rgb(27 34 44 / 88%); opacity: 0; animation: phx-tag 6s ease-in-out infinite; }
.tip .cmp .ver { position: absolute; left: 12px; right: 76px; bottom: 9px; height: 12px; font: 11px/12px var(--ph-font); color: #6b7280; }
.tip .cmp .ver i { position: absolute; left: 0; font-style: normal; white-space: nowrap; }
.tip .cmp .v0 { animation: phx-v0 6s steps(1) infinite; }
.tip .cmp .v1 { opacity: 0; animation: phx-v1 6s steps(1) infinite; }
@keyframes phx-del { 0%, 30% { color: #1b222c; text-decoration: none; } 40%, 92% { color: #b91c1c; text-decoration: line-through 2px #b91c1c; } 100% { color: #1b222c; } }
@keyframes phx-add { 0%, 45% { max-width: 0; } 60%, 92% { max-width: 6em; } 100% { max-width: 0; } }
@keyframes phx-tag { 0%, 62% { opacity: 0; transform: translateY(6px); } 70%, 92% { opacity: 1; transform: none; } 100% { opacity: 0; } }
@keyframes phx-v0 { 0% { opacity: 1; } 31% { opacity: 0; } }
@keyframes phx-v1 { 0% { opacity: 0; } 31% { opacity: 1; } }

/* live: a heading typed in the editor, the page's heading following it, letter by letter, and how long that took */
.tip .live .ed { position: absolute; left: 6px; top: 8px; bottom: 8px; width: 50%; padding: 6px 7px; background: var(--ph-menu); border: 1px solid var(--ph-border);
  border-radius: var(--ph-radius); font: 10px/14px ui-monospace, "DejaVu Sans Mono", monospace; white-space: nowrap; overflow: hidden; }
.tip .live .ed .k { color: var(--ph-link); }
.tip .live .ed i { display: block; height: 3px; margin: 7px 0 0; border-radius: 1px; background: var(--ph-border); }
.tip .live .ty { display: inline-block; vertical-align: bottom; overflow: hidden; white-space: nowrap; max-width: 0; animation: phx-type 5s steps(7, end) infinite; }
.tip .live .ed .ty { --to: 7ch; }
.tip .live .car { display: inline-block; width: 1px; height: 11px; vertical-align: -1px; background: var(--ph-fg); animation: phx-caret 1s steps(1) infinite; }
.tip .live .pg { right: 8px; top: 8px; width: 38%; bottom: -8px; }
.tip .live .pg b { display: block; margin: 2px 0 6px; font: 700 10px/1.2 "Noto Serif", Georgia, serif; color: #1b222c; white-space: nowrap; }
.tip .live .pg .ty { --to: 4.4em; animation-delay: .12s; }
.tip .live .ms { position: absolute; right: 10px; bottom: 6px; display: flex; align-items: center; gap: 2px; padding: 1px 6px; border-radius: 9px;
  font: 600 10px/14px var(--ph-font); color: #fff; background: rgb(27 34 44 / 85%); opacity: 0; animation: phx-ms 5s infinite; }
@keyframes phx-type { 0%, 10% { max-width: 0; } 50%, 88% { max-width: var(--to); } 100% { max-width: 0; } }
@keyframes phx-caret { 50% { opacity: 0; } }
@keyframes phx-ms { 0%, 14% { opacity: 0; } 20%, 86% { opacity: 1; } 94%, 100% { opacity: 0; } }

/* timing: the status clicked, its times drop down, each bar filling */
.tip .tim .mst { padding: 1px 6px; border-radius: var(--ph-radius); font-weight: 600; animation: phx-press 4s infinite; }
.tip .tim .mpop { position: absolute; left: 6px; top: 24px; width: 75%; padding: 3px 7px 4px; line-height: 12px; opacity: 0; animation: phx-drop 4s ease-out infinite;
  background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); box-shadow: 0 3px 10px rgb(0 0 0 / 25%); }
.tip .tim .mpop b { display: block; margin-bottom: 2px; font-weight: 600; }
.tip .tim .mpop p { display: grid; grid-template-columns: 40px 1fr 34px; align-items: center; gap: 5px; margin: 0; color: var(--ph-fg2); }
.tip .tim .mpop i { height: 4px; width: 0; border-radius: 2px; background: var(--ph-progress); animation: phx-fill 4s ease-out infinite; }
.tip .tim .mpop em { text-align: right; font-style: normal; font-variant-numeric: tabular-nums; white-space: nowrap; }
@keyframes phx-press { 0%, 12% { background: none; } 18%, 85% { background: var(--ph-hover); } 100% { background: none; } }
@keyframes phx-drop { 0%, 18% { opacity: 0; transform: translateY(-4px); } 28%, 88% { opacity: 1; transform: none; } 100% { opacity: 0; } }
@keyframes phx-fill { 0%, 28% { width: 0; } 50%, 88% { width: var(--w); } 100% { width: var(--w); } }

/* rebuild: the menu opens, Clean rebuild chosen, the page typeset afresh */
.tip .reb .msplit { display: flex; border-radius: var(--ph-pill); overflow: hidden; color: var(--ph-accent-fg); background: var(--ph-accent); font-weight: 600; }
.tip .reb .msplit > span { display: flex; align-items: center; gap: 2px; padding: 1px 5px; }
.tip .reb .dd { border-left: 1px solid rgb(255 255 255 / 30%); animation: phx-press-on 4s infinite; }
.tip .reb .go .icon { animation: phx-turn 4s linear infinite; }
.tip .reb .mmenu { left: 6px; top: 24px; animation: phx-menu 4s infinite; }
.tip .reb .mmenu .pick { animation: phx-pick 4s infinite; }
.tip .reb .pg { right: 14px; top: 26px; width: 34%; height: 70px; }
.tip .reb .pg i { animation: phx-retype 4s infinite; transform-origin: left; }
.tip .reb .pg i:nth-child(2) { animation-delay: .08s; } .tip .reb .pg i:nth-child(3) { animation-delay: .16s; }
.tip .reb .pg i:nth-child(4) { animation-delay: .24s; } .tip .reb .pg i:nth-child(5) { animation-delay: .32s; } .tip .reb .pg i:nth-child(6) { animation-delay: .4s; }
@keyframes phx-press-on { 0%, 10% { background: none; } 15%, 45% { background: var(--ph-accent-hover); } 50% { background: none; } }
@keyframes phx-menu { 0%, 14% { opacity: 0; transform: translateY(-3px); } 20%, 46% { opacity: 1; transform: none; } 52%, 100% { opacity: 0; } }
@keyframes phx-pick { 0%, 30% { background: none; } 36%, 52% { background: var(--ph-hover); } }
@keyframes phx-turn { 0%, 50% { transform: none; } 75%, 100% { transform: rotate(360deg); } }
@keyframes phx-retype { 0%, 52% { transform: none; } 56% { transform: scaleX(0); } 64%, 100% { transform: none; } }

/* dark PDF: the \u22EF menu, Dark PDF ticked, the page goes soft dark (a grey page, off-white ink; never black) */
.tip .drk .mbar { justify-content: flex-end; }
.tip .drk .dots { display: flex; padding: 1px 3px; border-radius: var(--ph-radius); animation: phx-press-on2 4s infinite; }
.tip .drk .mmenu { right: 6px; top: 24px; animation: phx-menu 4s infinite; }
.tip .drk .mmenu .pick { animation: phx-pick 4s infinite; }
.tip .drk .mmenu b { opacity: 0; animation: phx-tick 4s steps(1) infinite; }
.tip .drk .pg { left: 14px; top: 26px; width: 38%; height: 70px; animation: phx-dark 4s infinite; }
.tip .drk .pg i { animation: phx-ink 4s infinite; }
@keyframes phx-press-on2 { 0%, 10% { background: none; } 15%, 45% { background: var(--ph-hover); } 50% { background: none; } }
@keyframes phx-tick { 0%, 38% { opacity: 0; } 40%, 100% { opacity: 1; } }
@keyframes phx-dark { 0%, 46% { background: #fff; } 56%, 92% { background: ${DARK_PAGE}; } 100% { background: #fff; } }
@keyframes phx-ink { 0%, 46% { background: #c3c8d0; } 56%, 92% { background: #a9aeb6; } 100% { background: #c3c8d0; } }

/* contents: the column beside the page, the section at following the page as it scrolls */
.tip .toc .col { position: absolute; inset: 0 auto 0 0; width: 44%; padding: 6px 0; background: var(--ph-bar); border-right: 1px solid var(--ph-border);
  animation: phx-col 4s ease-out infinite; }
.tip .toc .col span { position: relative; z-index: 1; display: block; height: 18px; padding: 0 8px; line-height: 18px; white-space: nowrap; }
.tip .toc .col .at { position: absolute; left: 4px; right: 4px; top: 6px; height: 18px; border-radius: 2px; background: var(--ph-hover);
  box-shadow: inset 2px 0 var(--ph-accent); animation: phx-at 4s infinite; }
.tip .toc .pg { left: 50%; right: 10%; top: 8px; bottom: -40px; padding-top: 0; }
.tip .toc .roll { padding-top: 6px; animation: phx-roll 4s infinite; }
.tip .toc .roll s { display: block; height: 6px; }
@keyframes phx-col { 0% { transform: translateX(-100%); } 12%, 100% { transform: none; } }
@keyframes phx-at { 0%, 30% { transform: none; } 38%, 62% { transform: translateY(18px); } 70%, 100% { transform: translateY(36px); } }
@keyframes phx-roll { 0%, 30% { transform: none; } 38%, 62% { transform: translateY(-28px); } 70%, 100% { transform: translateY(-56px); } }

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
/* Settings: the panel's own rows and the host's, one dialog in every host: a tab per section (one line at any width), sub-headings inside, a search over them all */
.settings { inset: 12px auto auto 50%; translate: -50% 0; width: min(560px, calc(100% - 24px)); max-height: calc(100% - 24px); padding: 0; gap: 0; overflow: hidden; container-type: inline-size; }
.settings > header { padding: 10px 10px 0 16px; gap: 10px; }
.settings .sfind { flex: 1; display: flex; align-items: center; gap: 6px; min-width: 0; max-width: 220px; margin-left: auto; padding: 0 8px; height: 28px; box-sizing: border-box;
  border: 1px solid var(--ph-border); border-radius: var(--ph-pill); background: var(--ph-menu); color: var(--ph-fg2); font-weight: 400; cursor: text; }
.settings .sfind:focus-within { border-color: var(--ph-link); color: var(--ph-fg); }
.settings .sfind input { all: unset; flex: 1; min-width: 0; font-size: 12px; color: var(--ph-fg); }
.settings .sfind input::placeholder { color: var(--ph-fg2); }
.settings .sfind input::-webkit-search-cancel-button { cursor: pointer; }
/* (the sections share the line: five short names, each as wide as it needs and the room left shared, never wrapped, never scrolled) */
.settings .stabs { display: flex; padding: 6px 10px 0; border-bottom: 1px solid var(--ph-border); }
.settings .stabs button { all: unset; flex: 1 1 auto; cursor: pointer; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-align: center; padding: 6px 4px 8px; font-size: 13px; color: var(--ph-fg2); border-bottom: 2px solid transparent; margin-bottom: -1px; }
.settings .stabs button:hover { color: var(--ph-fg); }
.settings .stabs button[aria-selected="true"] { color: var(--ph-fg); font-weight: 600; border-bottom-color: var(--ph-accent); }
.settings .stabs button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: -2px; }
.settings.searching .stabs button { color: var(--ph-fg2); }
.settings .sbody { position: relative; overflow: auto; padding: 0 0 10px; }
.settings .snone { padding: 18px 16px 22px; text-align: center; font-size: 12px; }
/* a sub-heading: small capitals over its rows; while searching, its section's name before it */
.settings .ssec { display: none; }
.settings.searching .ssec { display: inline; }
.settings .sgroup + .sgroup { margin-top: 6px; }
.settings .ssub { margin: 0; padding: 14px 16px 4px; font-size: 11px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--ph-fg2); }
.settings.searching .sbody > section + section { border-top: 1px solid var(--ph-border); }
.settings .srow { display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "lab ctl" "help help" "err err"; align-items: center; column-gap: 16px; padding: 10px 16px; }
.settings .sgroup > .srow:not([hidden]) ~ .srow:not([hidden]) { border-top: 1px solid var(--ph-border); }
.settings .srow[hidden] { display: none; }
.settings .srow .lab { grid-area: lab; font-weight: 600; min-width: 0; }
.settings .srow .ctl { grid-area: ctl; justify-self: end; }
.settings .srow .help { grid-area: help; margin-top: 2px; font-size: 12px; line-height: 1.4; color: var(--ph-fg2); }
.settings .srow .help:empty { display: none; }
.settings .srow.off .lab, .settings .srow.off .ctl { opacity: .55; }
.settings .srow .err { grid-area: err; margin-top: 4px; font-size: 12px; color: var(--ph-danger); }
.settings .srow.picks { grid-template-areas: "lab ctl" "help help" "picks picks" "err err"; }
.settings .tiles { grid-area: picks; display: grid; grid-template-columns: repeat(auto-fill, minmax(92px, 1fr)); gap: 12px 10px; margin-top: 10px; }
.settings .pick { all: unset; cursor: pointer; display: grid; gap: 6px; min-width: 0; }
.settings .pick:disabled { cursor: default; }
.settings .pick .lk { position: relative; display: block; aspect-ratio: 210 / 297; overflow: hidden; border-radius: 2px; background: #fff; box-shadow: 0 0 0 1px var(--ph-border); opacity: .5; transition: opacity .15s, box-shadow .15s; }
.settings .pick .lk.dark { background: ${DARK_PAGE}; }
.settings .pick:hover .lk { opacity: .8; }
.settings .pick[aria-checked="true"] .lk { opacity: 1; box-shadow: 0 0 0 2px var(--ph-accent); }
.settings .pick .lk .pg, .settings .pick .lk .sheet { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.settings .pick .lk .sheet { background: #fff; container-type: inline-size; }
.settings .pick .lk.dark .sheet { background: ${DARK_PAGE}; }
.settings .pick .lk .sheet.in { background: none; }
.settings .pick .lk .phx-open { position: absolute; inset: 0; width: 100%; height: 100%; }
.settings .pick .nm { display: flex; align-items: center; gap: 6px; font-size: 12px; line-height: 1.3; color: var(--ph-fg2); min-width: 0; }
.settings .pick[aria-checked="true"] .nm { color: var(--ph-fg); }
.settings .pick .box { flex: none; width: 12px; height: 12px; border-radius: 3px; box-shadow: inset 0 0 0 1.5px var(--ph-fg2); }
.settings .pick[aria-checked="true"] .box { box-shadow: none; background: var(--ph-accent) url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 12'%3E%3Cpath d='M2.5 6.2 5 8.6 9.6 3.6' fill='none' stroke='%23fff' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E") center / 100% no-repeat; }
.settings .pick:focus-visible .lk { outline: 2px solid var(--ph-link); outline-offset: 2px; }
/* Colour lines' tab: the presets as swatches, the strength's slider, Custom's two pickers, the sample */
.settings .srow.swatches { grid-template-columns: minmax(0, 1fr); grid-template-areas: "lab" "help" "picks" "err"; }
.settings .srow.swatches > .ctl, .settings .srow.custom > .ctl { display: none; }
.settings .swatches .tiles { grid-template-columns: repeat(auto-fill, minmax(76px, 1fr)); gap: 8px; }
.settings .sw { all: unset; cursor: pointer; display: grid; gap: 5px; justify-items: center; font-size: 12px; color: var(--ph-fg2); }
.settings .sw .art { display: grid; gap: 3px; width: 100%; box-sizing: border-box; padding: 7px 8px; border-radius: var(--ph-radius); background: #fff; box-shadow: 0 0 0 1px var(--ph-border); }
.settings .sw .art.dark { background: ${DARK_PAGE}; }
.settings .sw .art i { display: block; height: 4px; border-radius: 2px; }
.settings .sw .art i:last-child { width: 60%; }
.settings .sw[aria-pressed="true"] { color: var(--ph-fg); font-weight: 600; }
.settings .sw[aria-pressed="true"] .art { box-shadow: 0 0 0 2px var(--ph-accent); }
.settings .sw:focus-visible .art { outline: 2px solid var(--ph-link); outline-offset: 2px; }
.settings .sw:disabled { cursor: default; opacity: .55; }
/* a tile's own row under it (Custom's two pickers), while it is picked */
.settings .swc { display: grid; align-content: start; gap: 8px; min-width: 0; }
.settings .srow.sub { display: grid; justify-items: center; gap: 0; padding: 0; border: 0; }
.settings .srow.sub[hidden] { display: none; }
.settings .srow.sub .cols { gap: 6px; }
.settings .srow.sub label { display: grid; justify-items: center; gap: 3px; font-size: 11px; color: var(--ph-fg2); cursor: pointer; }
.settings .srow.sub .err { margin: 0; font-size: 11px; color: var(--ph-danger); text-align: center; }
/* a colour's reset beside its picker, while it is not as it came */
.settings .cols { align-items: center; }
.settings .rst { all: unset; cursor: pointer; padding: 2px 6px; border-radius: var(--ph-radius); font-size: 12px; color: var(--ph-link); }
.settings .rst:hover { background: var(--ph-hover); }
.settings .rst:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
.settings .rst[hidden] { display: none; }
.settings input[type="range"] { width: 180px; accent-color: var(--ph-accent); }
.settings .cols { display: inline-flex; gap: 8px; }
.settings .cols input { width: 34px; height: 24px; padding: 0 2px; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); background: var(--ph-menu); cursor: pointer; }
.settings .srow.custom { grid-template-columns: minmax(0, 1fr); grid-template-areas: "lab" "help" "picks"; }
.settings .wsample { grid-area: picks; margin-top: 8px; padding: 12px 16px; border-radius: var(--ph-radius); box-shadow: 0 0 0 1px var(--ph-border); background: #fff; color: #000; isolation: isolate; overflow: hidden; text-align: left; font: 13px/1.45 Georgia, "Times New Roman", serif; }
.settings .wsample.dark { background: #282828; color: #e1e1e1; }
.settings .wsample span { position: relative; display: block; width: fit-content; white-space: nowrap; }
.settings .wsample span.h { font-weight: 700; font-size: 15px; margin-bottom: 4px; }
.settings .wsample span i { position: absolute; inset: 0 -2px; mix-blend-mode: lighten; pointer-events: none; }
.settings .wsample.dark span i { mix-blend-mode: darken; }
.settings .srow .val { color: var(--ph-fg2); font-variant-numeric: tabular-nums; }
.settings select { font: inherit; font-size: 12px; color: var(--ph-fg); background: var(--ph-menu); border: 1px solid var(--ph-border); border-radius: var(--ph-radius); padding: 3px 6px; max-width: 220px; }
.settings .btn { padding: 3px 12px; font-size: 12px; white-space: nowrap; max-width: 220px; overflow: hidden; text-overflow: ellipsis; }
.settings .seg { display: inline-flex; gap: 2px; padding: 2px; border-radius: var(--ph-pill); background: var(--ph-hover, rgb(0 0 0 / 6%)); }
.settings .seg button { all: unset; cursor: pointer; padding: 3px 11px; border-radius: var(--ph-pill); font-size: 12px; line-height: 1.4; white-space: nowrap; color: var(--ph-fg); }
.settings .seg button:hover:not([aria-pressed="true"]):not(:disabled) { background: var(--ph-border); }
.settings .seg button[aria-pressed="true"] { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.settings .seg button:disabled { cursor: default; }
.settings .switch { all: unset; cursor: pointer; position: relative; width: 30px; height: 18px; border-radius: 9px; background: var(--ph-border); transition: background .12s; }
.settings .switch i { position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: #fff; box-shadow: 0 1px 2px rgb(0 0 0 / 25%); transition: left .12s; }
.settings .switch[aria-checked="true"] { background: var(--ph-accent); }
.settings .switch[aria-checked="true"] i { left: 14px; }
.settings .switch:disabled { cursor: default; }
.settings .switch:focus-visible, .settings .seg button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: 1px; }
/* a narrow dialog (a narrow pane): less padding, the tabs smaller */
@container (max-width: 380px) {
  .settings > header { padding-left: 12px; }
  .settings .stabs { padding: 4px 2px 0; }
  .settings .stabs button { font-size: 12px; padding: 6px 1px 8px; }
  .settings .srow { padding-left: 12px; padding-right: 12px; }
  .settings .ssub { padding-left: 12px; }
  /* (a row's buttons under its label, not squeezing it to a word a line) */
  .settings .srow.choice, .settings .srow.select, .settings .srow.range { grid-template-columns: minmax(0, 1fr); grid-template-areas: "lab" "help" "ctl" "err"; }
  .settings .srow.choice .ctl, .settings .srow.select .ctl, .settings .srow.range .ctl { justify-self: start; margin-top: 6px; }
  .settings .srow.range .ctl, .settings .srow.range input[type="range"] { justify-self: stretch; width: 100%; }
  .settings .seg { flex-wrap: wrap; border-radius: 12px; }
  .settings .seg button { padding: 3px 8px; }
}

/* docked: the panel fills the host's pane under its toolbar */
.win.docked { position: relative; left: auto !important; top: auto !important; width: 100% !important; height: 100% !important; max-width: none; max-height: none;
  min-width: 0; min-height: 0; resize: none; border: 0; border-radius: 0; box-shadow: none; }
.win.collapsed { resize: none; min-height: 0; min-width: 0; border-radius: var(--ph-pill); }
.win.collapsed > :not(header) { display: none; }
.win.collapsed header { border-bottom: 0; }
@media (prefers-reduced-motion: reduce) {
  .progress::after, .split.busy .go .icon, .steps .track.unknown i, .toast .track.unknown i, .diffbar .busy::before { animation: none; }
  .opening, .toast { transition: none; }
  /* (the demos still, at what they show: the change marked, the times, Dark PDF on, the contents) */
  .tip .demo, .tip .demo * { animation: none !important; }
  .tip .live .ty { max-width: none; } .tip .live .ms { opacity: 1; }
  .tip .cmp .del { color: #b91c1c; text-decoration: line-through 2px #b91c1c; } .tip .cmp .add { max-width: 6em; }
  .tip .cmp .tag, .tip .cmp .v1, .tip .tim .mpop { opacity: 1; } .tip .cmp .v0 { opacity: 0; } .tip .tim .mpop i { width: var(--w); }
  .tip .reb .mmenu, .tip .drk .mmenu, .tip .drk .mmenu b { opacity: 1; } .tip .reb .pick, .tip .drk .pick { background: var(--ph-hover); }
  .tip .drk .pg { background: ${DARK_PAGE}; } .tip .drk .pg i { background: #a9aeb6; }
}
`;

// viewer/src/glide.ts
var OMEGA = 1 / 50;
var ZOOM_RATE = Math.LN2 / 1e3;

// viewer/src/keys.ts
function typing(e) {
  const el = e.composedPath?.()[0] ?? e.target;
  return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName ?? ""));
}

// crates/cli/viewer/dir.ts
var style = document.createElement("style");
style.textContent = `
:root { ${WATCH} --fm-ok: #1f8a4c; --fm-folder: #d08a12; color-scheme: light; }
@media (prefers-color-scheme: dark) { :root { ${WATCH_DARK} --fm-ok: #4cc27f; --fm-folder: #e3a840; --fm-pdf: #f0786d; --fm-image: #5cc48c; color-scheme: dark; } }
html, body { margin: 0; height: 100%; background: var(--ph-bar); color: var(--ph-fg); }
* { box-sizing: border-box; }
.top { display: flex; align-items: center; gap: 10px; height: 44px; padding: 0 16px; border-bottom: 1px solid var(--ph-border); background: var(--ph-stage); font: 13px/1 var(--ph-font); }
.top .brand { font-weight: 700; letter-spacing: .2px; }
.top .brand i { font-style: normal; color: var(--ph-progress); }
.top .what { color: var(--ph-fg2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.app { display: flex; flex-direction: column; height: 100%; }
.app > .fm { flex: 1; min-height: 0; }
.fm .fm-bar { padding-left: 12px; padding-right: 16px; }
@media (min-width: 1000px) { .app > .fm { width: 100%; max-width: 1180px; margin: 0 auto; border-left: 1px solid var(--ph-border); border-right: 1px solid var(--ph-border); } .app { background: var(--ph-stage); } }
${FILES_CSS}`;
document.head.append(style);
var dirOfHash = () => {
  try {
    return decodeURIComponent(/^#dir=(.*)$/.exec(location.hash)?.[1] ?? "");
  } catch {
    return "";
  }
};
var app = document.createElement("div");
app.className = "app";
var top = document.createElement("header");
top.className = "top";
var brand = document.createElement("span");
brand.className = "brand";
brand.append("Phi", Object.assign(document.createElement("i"), { textContent: "TeX" }));
var what = document.createElement("span");
what.className = "what";
what.textContent = "";
top.append(brand, what);
app.append(top);
document.body.append(app);
var wsUrl = new URL("ws", location.href);
wsUrl.protocol = wsUrl.protocol === "https:" ? "wss:" : "ws:";
var link = new FolderLink(wsUrl);
var esc = (p) => p.split("/").map(encodeURIComponent).join("/");
var quick;
var host = {
  docHref: (id) => new URL(`d/${id}/`, location.href).href,
  rawHref: (p) => new URL(`raw/${esc(p)}`, location.href).href,
  dirHref: (p) => p ? `#dir=${encodeURIComponent(p)}` : location.pathname + location.search,
  quickOpen: () => quick.toggle(),
  prefsKey: "phitex.files.index"
};
var files = new FileList(host, link, dirOfHash());
app.append(files.el);
quick = new QuickOpen(document.body, host, link);
link.onHello = (h) => {
  document.title = `${h.root} \xB7 phitex watch`;
  what.textContent = h.where ?? h.root;
  what.title = `phitex watch: ${h.where ?? h.root}`;
};
files.onDir = (d) => {
  const want = d ? `#dir=${encodeURIComponent(d)}` : "";
  if (location.hash !== want && (location.hash || want)) history.pushState(null, "", want || location.pathname + location.search);
};
addEventListener("popstate", () => {
  const d = dirOfHash();
  if (d !== files.dir) files.go(d);
});
addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "p") {
    e.preventDefault();
    quick.toggle();
    return;
  }
  if (quick.isOpen || typing(e)) return;
  if (e.key === "/" || (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
    e.preventDefault();
    files.focusFilter();
  } else if ((e.key === "Backspace" || e.altKey && e.key === "ArrowUp") && !e.ctrlKey && !e.metaKey) {
    e.preventDefault();
    files.goUp();
  } else if (/^(ArrowDown|ArrowUp|Home|End|PageDown|PageUp|Enter)$/.test(e.key) && document.activeElement === document.body) {
    e.preventDefault();
    files.focus();
  }
});
files.focus();
