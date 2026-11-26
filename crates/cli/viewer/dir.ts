// The index page of a folder's watch, `phitex watch DIR`
// (crates/cli/src/dirwatch.rs): a file manager of the folder (files.ts), the
// folder shown kept in the address (#dir=…, so Back and a reload stay), a
// document opened in a tab of its own; Ctrl+P finds one by name.
//
// Bundled by scripts/viewer-bundle.sh into dir.js.

import { FILES_CSS, FileList, FolderLink, QuickOpen, type FilesHost } from "../../../viewer/src/files.ts";
import { WATCH, WATCH_DARK } from "../../../viewer/src/print.ts";
import { typing } from "../../../viewer/src/keys.ts";

const style = document.createElement("style");
style.textContent = `
:root { ${WATCH} --fm-ok: #1f8a4c; --fm-folder: #d08a12; color-scheme: light; }
@media (prefers-color-scheme: dark) { :root { ${WATCH_DARK} --fm-ok: #4cc27f; --fm-folder: #e3a840; --fm-idle: #8fb3ec; --fm-pdf: #f0786d; --fm-image: #5cc48c; color-scheme: dark; } }
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

const dirOfHash = (): string => {
  try {
    return decodeURIComponent(/^#dir=(.*)$/.exec(location.hash)?.[1] ?? "");
  } catch {
    return "";
  }
};

const app = document.createElement("div");
app.className = "app";
const top = document.createElement("header");
top.className = "top";
const brand = document.createElement("span");
brand.className = "brand";
brand.append("Phi", Object.assign(document.createElement("i"), { textContent: "TeX" }));
const what = document.createElement("span");
what.className = "what";
what.textContent = "";
top.append(brand, what);
app.append(top);
document.body.append(app);

const wsUrl = new URL("ws", location.href);
wsUrl.protocol = wsUrl.protocol === "https:" ? "wss:" : "ws:";
const link = new FolderLink(wsUrl);
const esc = (p: string) => p.split("/").map(encodeURIComponent).join("/");
let quick: QuickOpen;
const host: FilesHost = {
  docHref: (id, path) => new URL(`d/${id}/?path=${encodeURIComponent(path)}`, location.href).href,
  rawHref: (p) => new URL(`raw/${esc(p)}`, location.href).href,
  dirHref: (p) => (p ? `#dir=${encodeURIComponent(p)}` : location.pathname + location.search),
  quickOpen: () => quick.toggle(),
  prefsKey: "phitex.files.index",
};
const files = new FileList(host, link, dirOfHash());
app.append(files.el);
quick = new QuickOpen(document.body, host, link);
link.onHello = (h) => {
  document.title = `${h.root} · phitex watch`;
  what.textContent = h.where ?? h.root;
  what.title = `phitex watch: ${h.where ?? h.root}`;
};
// (the folder in the address: Back goes to the folder before)
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
  if (e.key === "/" || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f")) {
    e.preventDefault();
    files.focusFilter();
  } else if ((e.key === "Backspace" || (e.altKey && e.key === "ArrowUp")) && !e.ctrlKey && !e.metaKey) {
    e.preventDefault();
    files.goUp();
  } else if (e.key === "b" && !e.ctrlKey && !e.metaKey && !e.altKey) {
    // (b, the Files column beside a document's pages: here the files are the whole page, so it goes to them)
    e.preventDefault();
    files.focus();
  } else if (/^(ArrowDown|ArrowUp|Home|End|PageDown|PageUp|Enter)$/.test(e.key) && document.activeElement === document.body) {
    e.preventDefault();
    files.focus();
  }
});
files.focus();
