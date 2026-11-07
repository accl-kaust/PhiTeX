// phitex's live viewer: viewer/src and crates/cli/viewer/cli.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)

// viewer/src/diagnostics.ts
var KINDS = [
  ["error", "Errors"],
  ["overfull", "Overfull"],
  ["underfull", "Underfull"],
  ["reference", "References"],
  ["font", "Fonts"],
  ["other", "Other"]
];
function kindOf(d) {
  if (d.kind && KINDS.some(([k]) => k === d.kind)) return d.kind;
  if (d.severity === "error" || d.severity === "fatal") return "error";
  const m = d.message;
  if (/^Overfull \\[hv]box|^Tight \\[hv]box/.test(m)) return "overfull";
  if (/^Underfull \\[hv]box|^Loose \\[hv]box/.test(m)) return "underfull";
  if (/[Rr]eference|[Cc]itation|database entry|undefined references|\bLabel\b/.test(m)) return "reference";
  if (/[Ff]ont|Missing character/.test(m)) return "font";
  return "other";
}
function lineAt(text, i) {
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
        line: lineAt(text, at)
      });
    }
    for (const at of s.strayBraces.slice(0, 3))
      out.push({ severity: "warning", code: "stray-brace", message: "} with no { to close", file, line: lineAt(text, at) });
    for (const at of s.openDollars.slice(0, 3))
      out.push({ severity: "warning", code: "unclosed-math", message: "$ never closed in this paragraph", file, line: lineAt(text, at) });
    for (const m of text.matchAll(/(^|[^\\])\\[gex]?def\s*\\([A-Za-z@]+)([^{\n%]*)$/gm))
      out.push({ severity: "warning", code: "def-no-body", message: `\\${m[2]} is defined with no { body } on its line`, file, line: lineAt(text, m.index + m[1].length) });
    const ifs = [...text.matchAll(/\\if[a-z]*\b/g)];
    const fis = (text.match(/\\fi\b/g) ?? []).length;
    if (ifs.length > fis && !/\\[gex]?def/.test(text))
      out.push({ severity: "info", code: "unclosed-if", message: `${ifs.length - fis} \\if\u2026 without \\fi`, file, line: lineAt(text, ifs.at(-1).index) });
    for (const m of text.matchAll(/\\input\s*\{?([^\s}\\%]+)/g)) {
      const n = m[1];
      if (!(n in files) && !(`${n}.tex` in files) && !/\.(aux|bbl|ind|toc)$/.test(n) && !n.startsWith("\\"))
        out.push({ severity: "warning", code: "missing-file", message: `\\input ${n}: no such file in the project`, file, line: lineAt(text, m.index) });
    }
    const latex = build?.latex ? [] : [...new Set(text.match(LATEX) ?? [])];
    if (latex.length) {
      const first = text.search(LATEX);
      out.push({
        severity: "error",
        code: "latex",
        message: `LaTeX is not supported yet (dropped): ${latex.slice(0, 5).join(" ")}${latex.length > 5 ? " \u2026" : ""}`,
        file,
        line: lineAt(text, first)
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
      out.push({ severity: "info", code: "non-ascii", message: `${nonAscii} non-ASCII characters: PhiTeX's fonts may not typeset them`, file, line: lineAt(text, firstNon) });
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

// viewer/src/text.ts
var LAYER_LINES = /* @__PURE__ */ new WeakMap();
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
    const lines2 = pages2[k];
    let hay = "";
    const at = [];
    const col = [];
    lines2.forEach((l, i) => {
      if (i) hay += " ", at.push(-1), col.push(-1);
      for (let j = 0; j < l.text.length; j++) hay += fold(l.text[j]), at.push(i), col.push(j);
    });
    for (let from2 = hay.indexOf(needle); from2 >= 0 && out.length < max; from2 = hay.indexOf(needle, from2 + needle.length)) {
      const boxes2 = [];
      let cur = -1, x = 0, r = 0;
      const flush = () => {
        if (cur < 0) return;
        const l = lines2[cur];
        boxes2.push([x, l.y - l.size * 0.78, r - x, l.size * 1.02]);
      };
      for (let u = from2; u < from2 + needle.length; u++) {
        const i = at[u];
        if (i < 0) continue;
        const l = lines2[i], xl = l.xs?.[col[u]] ?? l.x, xr = l.rs?.[col[u]] ?? l.x + l.w;
        if (i !== cur) flush(), cur = i, x = xl, r = xr;
        else x = Math.min(x, xl), r = Math.max(r, xr);
      }
      flush();
      if (boxes2.length) out.push({ k, boxes: boxes2 });
    }
  }
  return out;
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
          const v = scroller.getBoundingClientRect();
          for (const [k, r] of boxes2) if (this.slots[k]) this.snap(this.slots[k], r);
          for (const [k, r] of boxes2) {
            const s = this.slots[k];
            const win = s?.img?.canvas.dataset.win?.split(",").map(Number);
            const now = win && part(r, v);
            if (!now || now[0] >= win[0] - 1e-3 && now[1] >= win[1] - 1e-3 && now[2] <= win[2] + 1e-3 && now[3] <= win[3] + 1e-3) continue;
            const key = `${k}:${s.hash}`;
            if (this.asked.has(key)) continue;
            this.asked.add(key);
            s.drawn = null;
            this.host.need(k);
          }
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
    let drawing = 0;
    root.ownerDocument.addEventListener("selectionchange", () => {
      if (drawing) return;
      drawing = requestAnimationFrame(() => {
        drawing = 0;
        this.drawSelection();
      });
    });
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
      if (el) el.classList.toggle("on-link", !!this.linkAt(e));
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
  /** The text selected on the pages, as blocks a line (`.mark.seltext`), the old ones gone. */
  drawSelection() {
    for (const m of this.root.querySelectorAll(".seltext")) m.remove();
    const rn = this.root.getRootNode();
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
          const from2 = node && r.startContainer === node ? r.startOffset : 0;
          const to = node && r.endContainer === node ? r.endOffset : l.text.length;
          if (to <= from2) continue;
          const x0 = l.xs?.[from2] ?? l.x, x1 = l.rs?.[to - 1] ?? l.x + l.w;
          const m = document.createElement("div");
          m.className = "mark seltext";
          const top = l.y - l.size * 0.8, ht = l.size * 1.05;
          Object.assign(m.style, { left: `${x0 / w * 100}%`, top: `${top / h * 100}%`, width: `${(x1 - x0) / w * 100}%`, height: `${ht / h * 100}%` });
          s.el.append(m);
        }
      }
    }
  }
  /** Esc: the highlights gone (the editor's selection and cursor's word, a double-click's place, find's matches) and the text selected on the pages; whether there were any. */
  unmark() {
    const ms = this.root.querySelectorAll(".mark");
    const sel = this.root.ownerDocument.getSelection();
    const had = ms.length > 0 || !!sel && !sel.isCollapsed && this.root.contains(sel.anchorNode);
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
      tag.style.setProperty("--fs", String(boxes2[0][3] * 0.72 / w));
      at(tag, boxes2[0][0], boxes2[0][1]);
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
  fractions(places) {
    const total = this.scroller.scrollHeight || 1;
    return places.map(({ k, y }) => {
      const s = this.slots[k];
      if (!s) return -1;
      return (s.el.offsetTop + y / this.size.h * s.el.offsetHeight) / total;
    });
  }
  /** Scroll so page `k`'s place `y` (PDF points from its top) is a third down the view. */
  scrollToPlace(k, y) {
    const s = this.slots[k];
    if (!s) return;
    this.scroller.scrollTo({ top: s.el.offsetTop + y / this.size.h * s.el.offsetHeight - this.scroller.clientHeight / 3 });
  }
  /** The culprits drawn last (a page painted later draws its bars again). */
  culpritMarks = [];
  culprits(marks) {
    this.culpritMarks = marks;
    for (const m of this.root.querySelectorAll(".culprit")) m.remove();
    const { w, h } = this.size;
    for (const { k, boxes: boxes2, severity, i, rule, amount } of marks) {
      const s = this.slots[k];
      if (!s) continue;
      if (rule && boxes2.length) {
        const [x, y, bw, bh] = boxes2[0];
        const lines2 = [...s.el.querySelectorAll("text")].map((t) => LAYER_LINES.get(t)).filter((l) => !!l && !!l.text).sort((a, b) => a.y - b.y);
        const right = (l) => l.rs?.[l.rs.length - 1] ?? l.x + l.w;
        let at = lines2.findIndex((l) => l.y >= y && l.y - l.size <= y + bh);
        let line = at >= 0 ? lines2[at] : void 0;
        if (line && amount) {
          const par = [line];
          for (let j = at + 1; j < lines2.length && lines2[j].y - lines2[j - 1].y <= lines2[j].size * 1.6; j++) par.push(lines2[j]);
          const ends = par.map(right).sort((a, b) => a - b);
          const just = ends[ends.length >> 1];
          line = par.reduce((best, l) => Math.abs(right(l) - just - amount) < Math.abs(right(best) - just - amount) ? l : best, line);
        }
        const end = line ? right(line) : x + bw;
        const top = line ? line.y - line.size * 0.75 : y;
        const ht = line ? line.size * 0.95 : bh;
        const m = document.createElement("div");
        m.className = `culprit rule ${rule}`;
        if (i !== void 0) m.dataset.i = String(i);
        Object.assign(m.style, { left: `${(end + 1.5) / w * 100}%`, top: `${top / h * 100}%`, width: `${5 / w * 100}%`, height: `${ht / h * 100}%` });
        s.el.append(m);
        continue;
      }
      for (const [x, y, bw, bh] of boxes2) {
        const m = document.createElement("div");
        m.className = `culprit ${severity}`;
        if (i !== void 0) m.dataset.i = String(i);
        Object.assign(m.style, { left: `${x / w * 100}%`, top: `${y / h * 100}%`, width: `${bw / w * 100}%`, height: `${bh / h * 100}%` });
        s.el.append(m);
      }
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
    const painted = [];
    for (const s of order) {
      if (performance.now() - t0 > 8) break;
      this.toPaint.delete(s);
      this.paint(s);
      painted.push(s);
    }
    const boxes2 = painted.map((s) => s.el.getBoundingClientRect());
    painted.forEach((s, i) => this.snap(s, boxes2[i]));
    if (this.culpritMarks.some((m) => m.rule && painted.some((s) => Number(s.el.dataset.k) === m.k))) this.culprits(this.culpritMarks);
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
  /**
   * The part of page `k` on screen: fractions of its width and height
   * (left, top, right, bottom); null if none of it is. What a zoomed-in
   * page is drawn sharp around (`Strips.page`'s `view`).
   */
  view(k) {
    const s = this.slots[k];
    return s ? part(s.el.getBoundingClientRect(), this.scroller.getBoundingClientRect()) : null;
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
    const marks = [...s.el.querySelectorAll(".mark, .peer, .culprit")];
    this.draw(s);
    for (const m of marks) if (!m.isConnected) s.el.append(m);
  }
  /** The page's bitmap put in its slot (snapped by paintSome, once every page's writes are done). */
  draw(s) {
    if (!s.img) return;
    this.sizeSlot(s);
    const c = s.img.canvas;
    c.style.width = "100%";
    c.style.height = "100%";
    if (s.el.firstChild !== c) s.el.replaceChildren(c);
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
function part(r, v) {
  if (!r.width || !r.height || r.right <= v.left || r.left >= v.right || r.bottom <= v.top || r.top >= v.bottom) return null;
  const fx = (x) => Math.min(1, Math.max(0, (x - r.left) / r.width));
  const fy = (y) => Math.min(1, Math.max(0, (y - r.top) / r.height));
  return [fx(v.left), fy(v.top), fx(v.right), fy(v.bottom)];
}

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
.bar > .status { min-width: calc(4.5ch + 12px); }
.status.err b { color: var(--ph-danger); }
.pageno { display: inline-flex; align-items: center; gap: 4px; color: var(--ph-fg2); font-variant-numeric: tabular-nums; white-space: nowrap; }
.pageno input { width: 3ch; height: 22px; box-sizing: content-box; padding: 0 2px; padding: 0; text-align: center; font: inherit; color: var(--ph-fg); background: transparent;
  border: 1px solid var(--ph-border); border-radius: var(--ph-radius); }
.zoomb { padding: 0 2px 0 8px; }
#zs { display: none; }
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
/* narrower: what gives way, in order: the zoom's long label, find's box, then (560) the buttons the \u22EF menu also has, then the page box beside an open find */
.bar > :not(.status):not(.grow) { flex: none; }
.bar > .grow { flex: 1 1 0; min-width: 0; }
@container (max-width: 820px) { #zl { display: none; } #zs { display: inline; } }
@container (max-width: 760px) {
  /* (find open: Compare and Download step aside, to the \u22EF menu, which has them) */
  .bar:has(.find:not([hidden])) :is(#b-compare, #pdf) { display: none; }
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
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
.win.dark-pdf .slot, .win.docked.pdf-dark .slot { background: ${DARK_PAGE}; }
.win.dark-pdf .slot canvas, .win.docked.pdf-dark .slot canvas { filter: ${DARK_PDF_FILTER}; }
.phx-page { position: relative; width: 100%; height: 100%; background: #fff; }
.phx-page canvas { display: block; width: 100%; height: 100%; }
.phx-text { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-text text { fill: transparent; white-space: pre; cursor: text; font-family: sans-serif; }
/* (a link under the pointer, viewer.ts: the hand, over the text's cursor) */
.slot.on-link, .slot.on-link .phx-text text { cursor: pointer; }
.phx-text ::selection { fill: transparent; color: transparent; -webkit-text-fill-color: transparent; background: transparent; }
.stage.stale .slot { opacity: .45; filter: grayscale(1); }
.stage.offline .slot { opacity: .4; pointer-events: none; }
.slot .mark { position: absolute; pointer-events: none; border-radius: 2px; background: rgb(255 214 0 / 45%); mix-blend-mode: multiply;
  box-shadow: 0 0 0 1px rgb(232 172 0 / 80%); transition: opacity .25s, left .12s ease-out, top .12s ease-out, width .12s ease-out, height .12s ease-out; }
/* (the text selected on the pages, one block a line: viewer.ts drawSelection) */
.slot .mark.seltext { background: rgb(0 100 255 / 25%); mix-blend-mode: normal; border-radius: 0; }
.slot .mark.fade { opacity: 0; transition: opacity 1.2s; }
.slot .mark.sel { transition: none; }
.slot .mark.fnd { background: rgb(255 214 0 / 40%); box-shadow: none; transition: none; }
.slot .mark.fnd.cur { background: rgb(255 170 0 / 55%); box-shadow: 0 0 0 1px rgb(150 100 0 / 90%); }
.slot .culprit { position: absolute; pointer-events: none; }
/* (the marks along the pages' scrollbar: the problems' places and the diff's changes) */
.smarks { position: absolute; right: 0; top: 0; bottom: 0; width: 8px; z-index: 4; pointer-events: none; }
.smarks[hidden] { display: none; }
.smarks i { position: absolute; right: 1px; width: 6px; height: 3px; margin-top: -1px; border-radius: 1px; pointer-events: auto; cursor: pointer; background: var(--ph-warning, #c77700); }
.smarks i.error { background: var(--ph-danger, #d93025); }
.smarks i.change { background: var(--ph-accent, #1d3e72); }
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
/* (a problem's culprit: a wavy line along its glyphs' foot, as an editor underlines one; no fill, the text stays readable) */
.slot .culprit::after { content: ""; position: absolute; left: 0; right: 0; bottom: -2px; height: 3px; background: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23d93025' stroke-width='1'/%3E%3C/svg%3E") repeat-x left bottom / 6px 3px; }
.slot .culprit.warning::after { background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='6' height='3'%3E%3Cpath d='M0 2.5 Q1.5 0 3 1.5 T6 0.5' fill='none' stroke='%23c77700' stroke-width='1'/%3E%3C/svg%3E"); }
.slot .peer { position: absolute; pointer-events: none; border-radius: 2px; background: hsl(var(--hue) 70% 50% / 22%); box-shadow: inset 0 -1.5px 0 hsl(var(--hue) 70% 45%); }
/* (the name as large as the line it marks, so it zooms with the page; readable when far out, never a headline when close) */
.slot .peer-tag { width: auto; height: auto; transform: translateY(-100%); padding: 0 .4em; border-radius: .25em .25em .25em 0; background: hsl(var(--hue) 70% 42%);
  box-shadow: none; color: #fff; font: 600 clamp(7px, calc(var(--fs, .012) * 100cqw), 22px)/1.35 var(--ph-font); white-space: nowrap; z-index: 2; }

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
@keyframes phx-slide { from { left: -30%; } to { left: 100%; } }
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
/* Settings: the panel's own rows and the host's, one dialog in every host, a tab per group */
.settings { inset: 12px auto auto 50%; translate: -50% 0; width: min(560px, calc(100% - 24px)); max-height: calc(100% - 24px); padding: 0; gap: 0; overflow: hidden; }
.settings > header { padding: 12px 14px 0; }
.settings .stabs { display: flex; gap: 2px; padding: 6px 10px 0; border-bottom: 1px solid var(--ph-border); overflow-x: auto; scrollbar-width: none; }
.settings .stabs button { all: unset; cursor: pointer; white-space: nowrap; padding: 6px 8px 7px; font-size: 13px; color: var(--ph-fg2); border-bottom: 2px solid transparent; margin-bottom: -1px; }
.settings .stabs button:hover { color: var(--ph-fg); }
.settings .stabs button[aria-selected="true"] { color: var(--ph-fg); font-weight: 600; border-bottom-color: var(--ph-accent); }
.settings .stabs button:focus-visible { outline: 2px solid var(--ph-link); outline-offset: -2px; }
.settings .sbody { overflow: auto; padding: 4px 0 8px; }
.settings .srow { display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "lab ctl" "help help" "err err"; align-items: center; column-gap: 16px; padding: 10px 16px; }
.settings .srow + .srow { border-top: 1px solid var(--ph-border); }
.settings .srow .lab { grid-area: lab; font-weight: 600; min-width: 0; }
.settings .srow .ctl { grid-area: ctl; justify-self: end; }
.settings .srow .help { grid-area: help; margin-top: 2px; font-size: 12px; line-height: 1.4; color: var(--ph-fg2); }
.settings .srow .help:empty { display: none; }
.settings .srow.off .lab, .settings .srow.off .ctl { opacity: .55; }
.settings .srow .err { grid-area: err; margin-top: 4px; font-size: 12px; color: var(--ph-danger); }
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
  constructor(root, place4) {
    this.place = place4;
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
function typing(e) {
  const el = e.composedPath?.()[0] ?? e.target;
  return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName ?? ""));
}
function bindKeys(on, t) {
  let count = "";
  let g = false;
  let helping = false;
  const times = () => Math.max(1, Number(count) || 1);
  const page = (delta) => t.turn(delta);
  const by = (dx, dy) => t.scroller.scrollBy({ left: dx, top: dy, behavior: "auto" });
  const handler = (ev) => {
    const e = ev;
    if (typing(e)) return;
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
  const esc6 = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = pairs.length ? new RegExp(pairs.map(([p]) => `(?<![A-Za-z0-9_])${esc6(p)}(?![A-Za-z0-9_])`).join("|"), "g") : null;
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
var WOVEN = { "body": '<defs><clipPath id="\xA70" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA71" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA72" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA73" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA74" ><rect x="43.63" y="-870.22" width="35.34" height="1910.45" /></clipPath><clipPath id="\xA75" ><rect x="116" y="-870.22" width="57.31" height="1910.45" /></clipPath><clipPath id="\xA76" ><rect x="214.48" y="-891.22" width="47.67" height="1910.45" /></clipPath><mask id="\xA77" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA74)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA75)"><path d="M39 0V-47H147V-633H39V-680H660L691 -439H644Q639 -480 632.5 -506.5Q626 -533 612 -559Q598 -585 577 -600Q556 -615 522 -624Q488 -633 441 -633H289V-376H344Q411 -376 430.5 -403Q450 -430 450 -493H497V-212H450Q450 -275 430.5 -302Q411 -329 344 -329H289V-47H441Q486 -47 520.5 -53.5Q555 -60 579 -75.5Q603 -91 619.5 -108.5Q636 -126 647 -154.5Q658 -183 664 -209Q670 -235 676 -274H723L677 0Z" transform="translate(116 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA76)"><path d="M34 0V-47H63Q94 -47 124 -50Q147 -52 153.5 -55Q160 -58 167 -67L369 -332L147 -639H49V-686Q84 -683 211 -683Q351 -683 392 -686V-639H309L452 -442L584 -617Q585 -618 588 -622.5Q591 -627 592 -628Q557 -639 500 -639V-686Q536 -683 652 -683Q744 -683 786 -686V-639H757Q723 -639 699 -636Q673 -634 666.5 -631.5Q660 -629 652 -618L484 -397L737 -47H835V0Q800 -3 673 -3Q533 -3 492 0V-47H575L401 -288L236 -69Q235 -68 232 -63.5Q229 -59 228 -58Q263 -47 320 -47V0Q284 -3 168 -3Q76 -3 34 0Z" transform="translate(176.18 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA78" ><rect x="20.7" y="44.88" width="22.93" height="32.48" /></clipPath><clipPath id="\xA79" ><rect x="79.39" y="-891.22" width="28.66" height="1910.45" /></clipPath><clipPath id="\xA710" ><rect x="176.18" y="-891.22" width="38.3" height="1910.45" /></clipPath><mask id="\xA711" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA78)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA79)"><path d="M42 -434 63 -675H736L757 -434H710Q706 -476 703 -498.5Q700 -521 689.5 -550.5Q679 -580 663 -594Q647 -608 618 -618Q589 -628 547 -628H471V-47H624V0Q570 -3 400 -3Q230 -3 176 0V-47H329V-628H252Q210 -628 180.5 -617.5Q151 -607 135 -593Q119 -579 109 -550Q99 -521 95.5 -498.5Q92 -476 89 -434Z" transform="translate(55.51 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA710)"><path d="M34 0V-47H63Q94 -47 124 -50Q147 -52 153.5 -55Q160 -58 167 -67L369 -332L147 -639H49V-686Q84 -683 211 -683Q351 -683 392 -686V-639H309L452 -442L584 -617Q585 -618 588 -622.5Q591 -627 592 -628Q557 -639 500 -639V-686Q536 -683 652 -683Q744 -683 786 -686V-639H757Q723 -639 699 -636Q673 -634 666.5 -631.5Q660 -629 652 -618L484 -397L737 -47H835V0Q800 -3 673 -3Q533 -3 492 0V-47H575L401 -288L236 -69Q235 -68 232 -63.5Q229 -59 228 -58Q263 -47 320 -47V0Q284 -3 168 -3Q76 -3 34 0Z" transform="translate(176.18 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA712" ><rect x="27.96" y="-10.52" width="51.01" height="85.97" /></clipPath><clipPath id="\xA713" ><rect x="116" y="-870.22" width="57.31" height="1910.45" /></clipPath><clipPath id="\xA714" ><rect x="214.48" y="-891.22" width="47.67" height="1910.45" /></clipPath><mask id="\xA715" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA712)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA713)"><path d="M39 0V-47H147V-633H39V-680H660L691 -439H644Q639 -480 632.5 -506.5Q626 -533 612 -559Q598 -585 577 -600Q556 -615 522 -624Q488 -633 441 -633H289V-376H344Q411 -376 430.5 -403Q450 -430 450 -493H497V-212H450Q450 -275 430.5 -302Q411 -329 344 -329H289V-47H441Q486 -47 520.5 -53.5Q555 -60 579 -75.5Q603 -91 619.5 -108.5Q636 -126 647 -154.5Q658 -183 664 -209Q670 -235 676 -274H723L677 0Z" transform="translate(116 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA714)"><path d="M34 0V-47H63Q94 -47 124 -50Q147 -52 153.5 -55Q160 -58 167 -67L369 -332L147 -639H49V-686Q84 -683 211 -683Q351 -683 392 -686V-639H309L452 -442L584 -617Q585 -618 588 -622.5Q591 -627 592 -628Q557 -639 500 -639V-686Q536 -683 652 -683Q744 -683 786 -686V-639H757Q723 -639 699 -636Q673 -634 666.5 -631.5Q660 -629 652 -618L484 -397L737 -47H835V0Q800 -3 673 -3Q533 -3 492 0V-47H575L401 -288L236 -69Q235 -68 232 -63.5Q229 -59 228 -58Q263 -47 320 -47V0Q284 -3 168 -3Q76 -3 34 0Z" transform="translate(176.18 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA716" ><rect x="12.1" y="56.34" width="13.37" height="19.1" /></clipPath><clipPath id="\xA717" ><rect x="79.39" y="-891.22" width="28.66" height="1910.45" /></clipPath><clipPath id="\xA718" ><rect x="176.18" y="-891.22" width="85.97" height="1910.45" /></clipPath><mask id="\xA719" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA716)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA717)"><path d="M42 -434 63 -675H736L757 -434H710Q706 -476 703 -498.5Q700 -521 689.5 -550.5Q679 -580 663 -594Q647 -608 618 -618Q589 -628 547 -628H471V-47H624V0Q570 -3 400 -3Q230 -3 176 0V-47H329V-628H252Q210 -628 180.5 -617.5Q151 -607 135 -593Q119 -579 109 -550Q99 -521 95.5 -498.5Q92 -476 89 -434Z" transform="translate(55.51 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g><g clip-path="url(#\xA718)"><path d="M34 0V-47H63Q94 -47 124 -50Q147 -52 153.5 -55Q160 -58 167 -67L369 -332L147 -639H49V-686Q84 -683 211 -683Q351 -683 392 -686V-639H309L452 -442L584 -617Q585 -618 588 -622.5Q591 -627 592 -628Q557 -639 500 -639V-686Q536 -683 652 -683Q744 -683 786 -686V-639H757Q723 -639 699 -636Q673 -634 666.5 -631.5Q660 -629 652 -618L484 -397L737 -47H835V0Q800 -3 673 -3Q533 -3 492 0V-47H575L401 -288L236 -69Q235 -68 232 -63.5Q229 -59 228 -58Q263 -47 320 -47V0Q284 -3 168 -3Q76 -3 34 0Z" transform="translate(176.18 64) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><mask id="\xA720" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="43.63" y="-870.22" width="35.34" height="1910.45" fill="#000"/></mask><mask id="\xA721" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA720)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA722" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="20.7" y="44.88" width="22.93" height="32.48" fill="#000"/></mask><mask id="\xA723" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA722)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA724" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="27.96" y="-10.52" width="51.01" height="85.97" fill="#000"/></mask><mask id="\xA725" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA724)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA726" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="12.1" y="56.34" width="13.37" height="19.1" fill="#000"/></mask><mask id="\xA727" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA726)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask><mask id="\xA728" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></mask><mask id="\xA729" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="79.39" y="-891.22" width="28.66" height="1910.45" fill="#000"/></mask><mask id="\xA730" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA729)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA731" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></mask><mask id="\xA732" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="79.39" y="-891.22" width="28.66" height="1910.45" fill="#000"/></mask><mask id="\xA733" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA732)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask><mask id="\xA734" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="116" y="-870.22" width="57.31" height="1910.45" fill="#000"/></mask><mask id="\xA735" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA734)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA736" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></mask><mask id="\xA737" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="116" y="-870.22" width="57.31" height="1910.45" fill="#000"/></mask><mask id="\xA738" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA737)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA739" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></mask><mask id="\xA740" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="214.48" y="-891.22" width="47.67" height="1910.45" fill="#000"/></mask><mask id="\xA741" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA740)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA742" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="176.18" y="-891.22" width="38.3" height="1910.45" fill="#000"/></mask><mask id="\xA743" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA742)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA744" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="214.48" y="-891.22" width="47.67" height="1910.45" fill="#000"/></mask><mask id="\xA745" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA744)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA746" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="176.18" y="-891.22" width="85.97" height="1910.45" fill="#000"/></mask><mask id="\xA747" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA746)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask></defs><g mask="url(#\xA77)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L262.18,51.56" fill="none" stroke="#f58f2d" stroke-width="5.28"/></g><g mask="url(#\xA711)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L262.18,45.24" fill="none" stroke="#f0b225" stroke-width="4.07"/></g><g mask="url(#\xA715)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L262.18,40.12" fill="none" stroke="#d3c72a" stroke-width="2.86"/></g><g mask="url(#\xA719)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L262.18,36.22" fill="none" stroke="#00a8a4" stroke-width="1.65"/></g><g class="phx-l" data-ch="\u03A6"><g mask="url(#\xA727)"><g mask="url(#\xA725)"><g mask="url(#\xA723)"><g mask="url(#\xA721)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#FILL"/></g></g></g></g></g><g class="phx-l" data-ch="T"><g mask="url(#\xA733)"><g mask="url(#\xA731)"><g mask="url(#\xA730)"><g mask="url(#\xA728)"><path d="M42 -434 63 -675H736L757 -434H710Q706 -476 703 -498.5Q700 -521 689.5 -550.5Q679 -580 663 -594Q647 -608 618 -618Q589 -628 547 -628H471V-47H624V0Q570 -3 400 -3Q230 -3 176 0V-47H329V-628H252Q210 -628 180.5 -617.5Q151 -607 135 -593Q119 -579 109 -550Q99 -521 95.5 -498.5Q92 -476 89 -434Z" transform="translate(55.51 64) scale(0.09552)" fill="#FILL"/></g></g></g></g></g><g class="phx-l" data-ch="E"><g mask="url(#\xA739)"><g mask="url(#\xA738)"><g mask="url(#\xA736)"><g mask="url(#\xA735)"><path d="M39 0V-47H147V-633H39V-680H660L691 -439H644Q639 -480 632.5 -506.5Q626 -533 612 -559Q598 -585 577 -600Q556 -615 522 -624Q488 -633 441 -633H289V-376H344Q411 -376 430.5 -403Q450 -430 450 -493H497V-212H450Q450 -275 430.5 -302Q411 -329 344 -329H289V-47H441Q486 -47 520.5 -53.5Q555 -60 579 -75.5Q603 -91 619.5 -108.5Q636 -126 647 -154.5Q658 -183 664 -209Q670 -235 676 -274H723L677 0Z" transform="translate(116 85) scale(0.09552)" fill="#FILL"/></g></g></g></g></g><g class="phx-l" data-ch="X"><g mask="url(#\xA747)"><g mask="url(#\xA745)"><g mask="url(#\xA743)"><g mask="url(#\xA741)"><path d="M34 0V-47H63Q94 -47 124 -50Q147 -52 153.5 -55Q160 -58 167 -67L369 -332L147 -639H49V-686Q84 -683 211 -683Q351 -683 392 -686V-639H309L452 -442L584 -617Q585 -618 588 -622.5Q591 -627 592 -628Q557 -639 500 -639V-686Q536 -683 652 -683Q744 -683 786 -686V-639H757Q723 -639 699 -636Q673 -634 666.5 -631.5Q660 -629 652 -618L484 -397L737 -47H835V0Q800 -3 673 -3Q533 -3 492 0V-47H575L401 -288L236 -69Q235 -68 232 -63.5Q229 -59 228 -58Q263 -47 320 -47V0Q284 -3 168 -3Q76 -3 34 0Z" transform="translate(176.18 64) scale(0.09552)" fill="#FILL"/></g></g></g></g></g>', "x0": 0, "y0": -1, "x1": 262.1772889823344, "y1": 89 };
var MARK = { "body": '<defs><clipPath id="\xA70" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA71" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA72" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA73" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA74" ><rect x="43.63" y="-870.22" width="35.34" height="1910.45" /></clipPath><mask id="\xA75" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA74)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA76" ><rect x="20.7" y="44.88" width="22.93" height="32.48" /></clipPath><mask id="\xA77" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA76)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA78" ><rect x="27.96" y="-10.52" width="51.01" height="85.97" /></clipPath><mask id="\xA79" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA78)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA710" ><rect x="12.1" y="56.34" width="13.37" height="19.1" /></clipPath><mask id="\xA711" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA710)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><mask id="\xA712" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="43.63" y="-870.22" width="35.34" height="1910.45" fill="#000"/></mask><mask id="\xA713" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA712)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA714" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="20.7" y="44.88" width="22.93" height="32.48" fill="#000"/></mask><mask id="\xA715" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA714)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA716" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="27.96" y="-10.52" width="51.01" height="85.97" fill="#000"/></mask><mask id="\xA717" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA716)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA718" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="12.1" y="56.34" width="13.37" height="19.1" fill="#000"/></mask><mask id="\xA719" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA718)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask></defs><g mask="url(#\xA75)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#f58f2d" stroke-width="5.28"/></g><g mask="url(#\xA77)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#f0b225" stroke-width="4.07"/></g><g mask="url(#\xA79)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#d3c72a" stroke-width="2.86"/></g><g mask="url(#\xA711)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#00a8a4" stroke-width="1.65"/></g><g class="phx-l" data-ch="\u03A6"><g mask="url(#\xA719)"><g mask="url(#\xA717)"><g mask="url(#\xA715)"><g mask="url(#\xA713)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#FILL"/></g></g></g></g></g>', "x0": 0, "y0": 20, "x1": 71, "y1": 89 };
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
var OPEN = { draw: 700, stagger: 110, sweep: 1e3, rest: 300, cycle: 1600, settle: 500, dive: 470, fade: 180, into: [0.3, 0.42], deep: 5 };
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
  const el = box.lastElementChild, hole2 = el.querySelector(".phx-hole");
  const by = byline(el, WOVEN);
  const word = { x0: Math.min(WOVEN.x0, by.x0), y0: WOVEN.y0, x1: Math.max(WOVEN.x1, by.x1), y1: by.y1 };
  const rest = [...el.querySelectorAll(".phx-l")].filter((g) => g.dataset.ch !== "\u03A6").map((g) => [g, w.letters.find((l) => l.ch === g.dataset.ch)]);
  const tmp = document.createElementNS(NS, "svg");
  tmp.style.cssText = "position:absolute;width:0;height:0;visibility:hidden";
  document.body.append(tmp);
  tmp.innerHTML = art(MARK, fill, `${tag}m`);
  const S = [0, 1, 2, 3].map((j) => {
    const ps = [...el.querySelectorAll(`.phx-s${j}`)];
    const full = ps[0].getTotalLength(), part3 = tmp.querySelector(`.phx-s${j}`).getTotalLength();
    ps.forEach((p) => p.style.strokeDasharray = "1 2");
    return { ps, full, f: part3 / full };
  });
  tmp.remove();
  const W2 = () => box.clientWidth || 1, H2 = () => box.clientHeight || 1;
  const fit = (r, fw, fh) => ({
    cx: (r.x0 + r.x1) / 2,
    cy: (r.y0 + r.y1) / 2,
    s: Math.max((r.x1 - r.x0) / (fw * W2()), (r.y1 - r.y0) / (fh * H2()))
  });
  const P = { x: phi.x + OPEN.into[0] * size, y: phi.y - OPEN.into[1] * size };
  const deep = () => OPEN.deep / W2();
  const still2 = matchMedia("(prefers-reduced-motion: reduce)").matches;
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
  const cam = (t) => {
    const cm = fit(MARK, 0.45, 0.3), cw = fit(word, 0.84, 0.4);
    const k0 = easeIO((t - T1 - 200) / (0.75 * OPEN.sweep));
    return t < T1 + 200 ? cm : { cx: cm.cx + (cw.cx - cm.cx) * k0, cy: cm.cy + (cw.cy - cm.cy) * k0, s: cm.s + (cw.s - cm.s) * k0 };
  };
  const frame = (t) => {
    if (still2) t = doneAt === null ? T3 : Math.max(t, (settled ??= doneAt) + OPEN.dive);
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
    let c = cam(t);
    if (doneAt !== null && t >= (settled ??= doneAt)) {
      const b = cam(settled), u = Math.min(1, (t - settled) / OPEN.dive), s = b.s * Math.exp(Math.log(deep() / b.s) * u ** 2.2), k = (s - deep()) / (b.s - deep());
      c = { cx: P.x + (b.cx - P.x) * k, cy: P.y + (b.cy - P.y) * k, s };
      hole2.setAttribute("opacity", String(f2(Math.min(1, u * 4))));
      el.style.opacity = String(1 - Math.max(0, (t - settled - OPEN.dive) / OPEN.fade));
      if (t >= settled + OPEN.dive + OPEN.fade) end = true;
    }
    by.frame(still2 ? 1e9 : t - (T2 - 300));
    el.setAttribute("viewBox", [c.cx - W2() * c.s / 2, c.cy - H2() * c.s / 2, W2() * c.s, H2() * c.s].map(f2).join(" "));
  };
  const tick = () => {
    frame(performance.now() - t0);
    if (end) return void (el.remove(), finish());
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return {
    done() {
      doneAt ??= performance.now() - t0;
      return new Promise((ok) => finish = ok);
    },
    stop() {
      cancelAnimationFrame(raf);
      el.remove();
      finish();
    }
  };
}
function still() {
  return { done: () => Promise.resolve(), stop() {
  } };
}

// viewer/src/openings/kit.ts
var INK = { light: "#1d3e72", dark: "#e4e6ea" };
var PAPER = { light: "#ffffff", dark: "#2c2e33" };
var esc3 = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
var RIBBON = ["#00a8a4", "#d3c72a", "#f0b225", "#f58f2d"];
var reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
var registered = false;
function register() {
  if (registered) return;
  registered = true;
  try {
    CSS.registerProperty({ name: "--phx-o-r", syntax: "<percentage>", inherits: false, initialValue: "0%" });
  } catch {
  }
}
var SHARED = `
.phx-o { position: absolute; inset: 0; overflow: hidden; }
.phx-o.hole { --phx-o-x: 50%; --phx-o-y: 50%;
  -webkit-mask: radial-gradient(circle at var(--phx-o-x) var(--phx-o-y), transparent var(--phx-o-r), #000 calc(var(--phx-o-r) + 0.5%));
          mask: radial-gradient(circle at var(--phx-o-x) var(--phx-o-y), transparent var(--phx-o-r), #000 calc(var(--phx-o-r) + 0.5%)); }
.phx-o.hole.open { --phx-o-r: 150%; transition: --phx-o-r var(--phx-o-t, 600ms) cubic-bezier(.55, 0, .3, 1); }
.phx-o.wipe { -webkit-mask: linear-gradient(var(--phx-ongle, 115deg), transparent calc(var(--phx-o-r) - 12%), #000 var(--phx-o-r));
                      mask: linear-gradient(var(--phx-ongle, 115deg), transparent calc(var(--phx-o-r) - 12%), #000 var(--phx-o-r)); }
.phx-o.wipe.open { --phx-o-r: 115%; transition: --phx-o-r var(--phx-o-t, 600ms) cubic-bezier(.6, 0, .25, 1); }`;
function layer(box, cls, css, html) {
  register();
  const host2 = document.createElement("div");
  host2.style.cssText = "position: absolute; inset: 0; pointer-events: none;";
  host2.setAttribute("aria-hidden", "true");
  const root = host2.attachShadow({ mode: "open" });
  root.innerHTML = `<style>${SHARED}${css}</style><div class="phx-o ${cls}">${html}</div>`;
  box.append(host2);
  return root.lastElementChild;
}
var gone = (el) => (el.getRootNode().host ?? el).remove();
function hole(el, x = 50, y = 50, ms = 600) {
  el.style.setProperty("--phx-o-x", `${x}%`);
  el.style.setProperty("--phx-o-y", `${y}%`);
  el.style.setProperty("--phx-o-t", `${ms}ms`);
  el.classList.add("hole");
  void el.offsetWidth;
  el.classList.add("open");
}
function wipe(el, angle = 115, ms = 600) {
  el.style.setProperty("--phx-ongle", `${angle}deg`);
  el.style.setProperty("--phx-o-t", `${ms}ms`);
  el.classList.add("wipe");
  void el.offsetWidth;
  el.classList.add("open");
}
function loop(frame) {
  const t0 = performance.now();
  let raf = 0, last = t0, on = true;
  const tick = (now) => {
    if (!on) return;
    frame(now - t0, Math.min(50, now - last));
    last = now;
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return () => (on = false, cancelAnimationFrame(raf));
}
function ending(el, exit, cleanup = () => {
}, says) {
  let over = false, ending2 = null;
  const stop = () => {
    if (over) return;
    over = true;
    cleanup();
    gone(el);
  };
  return {
    ...says,
    done: () => ending2 ??= new Promise((ok) => {
      if (over) return ok();
      const ms = reduced() ? 0 : exit();
      setTimeout(() => (stop(), ok()), ms);
    }),
    stop
  };
}
function crt(cls, screen, glow = "currentColor") {
  const c = `.${cls}`;
  return `${c} { background: linear-gradient(160deg, #3a3b3f, #1c1d20 60%, #151618); }
${c} .scr { position: absolute; inset: 3.4cqw 3.4cqw 5.6cqw; border-radius: 4cqw / 5cqw; background: ${screen}; overflow: hidden;
  box-shadow: 0 0 0 .5cqw #0b0b0c, 0 .3cqw .6cqw .5cqw rgb(255 255 255 / 6%); transform-origin: 50% 50%;
  animation: phx-crt-on .5s cubic-bezier(.2,.8,.3,1) both, phx-crt-flicker 3.2s steps(1) .5s infinite; }
${c} .scr .txt { text-shadow: -.12cqw 0 rgb(255 40 90 / 35%), .12cqw 0 rgb(40 170 255 / 35%), 0 0 .7cqw ${glow}; }
${c} .scr::before { content: ""; position: absolute; inset: 0; z-index: 3; pointer-events: none;
  background: repeating-linear-gradient(rgb(0 0 0 / 0) 0 .25cqw, rgb(0 0 0 / 32%) .25cqw .5cqw); mix-blend-mode: multiply; }
${c} .scr::after { content: ""; position: absolute; inset: 0; z-index: 4; pointer-events: none; border-radius: inherit;
  background: radial-gradient(120% 90% at 30% 15%, rgb(255 255 255 / 9%), transparent 40%), radial-gradient(ellipse at center, transparent 55%, rgb(0 0 0 / 55%) 100%);
  box-shadow: inset 0 0 5cqw rgb(0 0 0 / 70%); }
${c} .band { position: absolute; left: 0; right: 0; height: 18%; z-index: 2; pointer-events: none; background: linear-gradient(transparent, rgb(255 255 255 / 4%), transparent); animation: phx-crt-band 7s linear infinite; }
${c} .led { position: absolute; right: 6cqw; bottom: 2.2cqw; width: 1.2cqw; height: 1.2cqw; border-radius: 50%; background: #5dff8a; box-shadow: 0 0 1cqw #5dff8a; }
${c} .brand { position: absolute; left: 0; right: 0; bottom: 1.7cqw; text-align: center; font: 600 1.6cqw/1 system-ui, sans-serif; letter-spacing: .3em; color: #6b6d72; }
@keyframes phx-crt-on { 0% { transform: scale(1, .004); filter: brightness(4); } 40% { transform: scale(1, .004); filter: brightness(4); } 100% { transform: scale(1, 1); filter: none; } }
@keyframes phx-crt-flicker { 0% { opacity: 1; } 31% { opacity: .96; } 32% { opacity: 1; } 67% { opacity: .98; } 68% { opacity: 1; } }
@keyframes phx-crt-band { from { top: -20%; } to { top: 110%; } }
${c}.off { background: transparent; transition: background .35s ease .1s; }
${c}.off .led, ${c}.off .brand { opacity: 0; transition: opacity .2s; }
${c}.off .scr { box-shadow: none; animation: phx-crt-off .5s cubic-bezier(.7,0,.6,1) forwards; }
${c}.off .scr > * { animation: phx-crt-white .5s forwards; }
@keyframes phx-crt-off { 0% { transform: scale(1, 1); filter: brightness(1); }
  45% { transform: scale(1, .006); filter: brightness(5); border-radius: 0; }
  75% { transform: scale(.02, .006); filter: brightness(6); opacity: 1; }
  100% { transform: scale(0, 0); filter: brightness(6); opacity: 0; } }
@keyframes phx-crt-white { 30%, 100% { background: #fff; color: #fff; } }`;
}
function crtOff(el) {
  el.classList.add("off");
  return 520;
}

// viewer/src/openings/win98.ts
var RESTORED = { left: 9, top: 15, width: 82, height: 66 };
function win98(box, doc = "document.tex") {
  const pdf = esc3(doc.replace(/\.tex$/i, "") + ".pdf");
  const el = layer(
    box,
    "phx-w98",
    `.phx-w98 { background: #008080; font: 3.1cqw/1.3 "MS Sans Serif", Tahoma, "Segoe UI", sans-serif; color: #000; }
.phx-w98 .bev { background: #c0c0c0; border: 2px solid; border-color: #fff #000 #000 #fff; box-shadow: inset -1px -1px #808080, inset 1px 1px #dfdfdf; }
.phx-w98 .w { position: absolute; left: 9%; right: 9%; top: 26%; padding: 3px; }
.phx-w98 .t, .phx-w98 .fly { display: flex; align-items: center; gap: 2px; height: 5cqw; padding: 0 .8cqw; color: #fff; font-weight: bold; background: linear-gradient(90deg, #000080, #1084d0); white-space: nowrap; overflow: hidden; }
.phx-w98 .t span { flex: 1; overflow: hidden; text-overflow: ellipsis; }
.phx-w98 .t i { flex: none; font-style: normal; width: 4.2cqw; height: 3.7cqw; line-height: 3.1cqw; text-align: center; color: #000; background: #c0c0c0; border: 1px solid; border-color: #fff #000 #000 #fff; font-size: 2.8cqw; font-weight: bold; }
.phx-w98 .t i.down { border-color: #000 #fff #fff #000; }
.phx-w98 .b { padding: 3.4cqw 3.4cqw 3cqw; }
.phx-w98 .fl { position: relative; height: 11cqw; margin-bottom: 2.4cqw; }
.phx-w98 .fold { position: absolute; top: 3cqw; width: 9cqw; height: 6.6cqw; background: #e8d36a; border: 1px solid #806000; }
.phx-w98 .fold::before { content: ""; position: absolute; left: -1px; top: -1.8cqw; width: 3.6cqw; height: 1.5cqw; background: #e8d36a; border: 1px solid #806000; border-bottom: none; }
.phx-w98 .f1 { left: 6%; } .phx-w98 .f2 { right: 6%; }
.phx-w98 .pg { position: absolute; top: 1.2cqw; left: 12%; width: 4.2cqw; height: 5.4cqw; background: #fff; border: 1px solid #000; animation: phx-w98-fly 1.6s linear infinite; }
@keyframes phx-w98-fly { 0% { transform: translate(0, 2.4cqw) rotate(-20deg); } 50% { transform: translate(20cqw, -1.8cqw) rotate(0); } 100% { transform: translate(40cqw, 2.4cqw) rotate(20deg); } }
.phx-w98 .msg { margin: 0; font-weight: bold; }
.phx-w98 .det { margin: .6cqw 0 2cqw; min-height: 1.3em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-w98 .bar { height: 4.6cqw; border: 1px solid; border-color: #808080 #fff #fff #808080; padding: 2px; position: relative; overflow: hidden; }
.phx-w98 .bar span { position: absolute; top: 2px; bottom: 2px; left: 2px; width: 0; background: repeating-linear-gradient(90deg, #000080 0 2.2cqw, transparent 2.2cqw 2.8cqw); }
.phx-w98 .bar.march span { width: 30%; animation: phx-w98-march 1.6s steps(10) infinite; }
@keyframes phx-w98-march { from { left: -30%; } to { left: 100%; } }
.phx-w98 .task { position: absolute; left: 0; right: 0; bottom: 0; height: 6.6cqw; background: #c0c0c0; border-top: 2px solid #fff; box-shadow: 0 -1px #dfdfdf; display: flex; align-items: center; gap: 1cqw; padding: 0 .8cqw; transition: transform .3s cubic-bezier(.5,0,.2,1); }
.phx-w98 .start, .phx-w98 .btn { height: 4.9cqw; display: flex; align-items: center; padding: 0 1.4cqw; border: 2px solid; border-color: #fff #000 #000 #fff; white-space: nowrap; overflow: hidden; }
.phx-w98 .start { font-weight: bold; }
.phx-w98 .btn { width: 34%; }
.phx-w98 .btn.on { border-color: #000 #fff #fff #000; background: repeating-conic-gradient(#c0c0c0 0 25%, #fff 0 50%) 0 0 / 2px 2px; font-weight: bold; }
.phx-w98 .clock { margin-left: auto; height: 4.9cqw; display: flex; align-items: center; padding: 0 1.4cqw; border: 1px solid; border-color: #808080 #fff #fff #808080; }
.phx-w98 .tip { position: absolute; right: 2cqw; bottom: 9.4cqw; width: 58%; background: #ffffe1; border: 1px solid #000; border-radius: 1.6cqw; padding: 1.6cqw 2cqw; font-size: 2.6cqw; line-height: 1.35; box-shadow: .4cqw .4cqw 0 rgb(0 0 0 / 25%); visibility: hidden; }
.phx-w98 .tip::after { content: ""; position: absolute; right: 7cqw; bottom: -2.2cqw; border: 1.1cqw solid transparent; border-top: 1.1cqw solid #000; border-right-color: #000; }
.phx-w98 .tip b { display: block; margin-bottom: .6cqw; }
.phx-w98 .tip.on { visibility: visible; animation: phx-w98-tip .2s steps(3); }
@keyframes phx-w98-tip { from { opacity: 0; } }
.phx-w98 .fly { position: absolute; display: none; z-index: 5; }
.phx-w98 .pdfw { position: absolute; display: none; left: ${RESTORED.left}%; top: ${RESTORED.top}%; width: ${RESTORED.width}%; height: ${RESTORED.height}%;
  box-shadow: 0 0 0 1px #808080, 0 0 0 3px #c0c0c0, 0 0 0 4px #dfdfdf, 0 0 0 5px #000, 0 0 0 999px #008080; transition: all .3s cubic-bezier(.5,0,.2,1); }
.phx-w98 .pdfw .chrome { position: absolute; left: -3px; right: -3px; bottom: 100%; background: #c0c0c0; padding: 3px 3px 0; }
.phx-w98 .pdfw .menu { display: flex; gap: 2.6cqw; padding: .5cqw 1.4cqw; font-size: 2.8cqw; border-bottom: 1px solid #808080; }
.phx-w98 .pdfw .menu u { text-decoration: underline; }
.phx-w98.desk { background: transparent; }
.phx-w98.desk .pdfw { display: block; }
.phx-w98.max .pdfw { left: 0; top: 0; width: 100%; height: 100%; }
.phx-w98.max .task { transform: translateY(110%); }`,
    `<div class="w bev"><div class="t"><span>PhiTeX Setup</span><i>_</i><i>\u25A1</i><i>\xD7</i></div><div class="b"><div class="fl"><div class="fold f1"></div><div class="pg"></div><div class="fold f2"></div></div>
       <p class="msg">Starting\u2026</p><p class="det"></p><div class="bar march"><span></span></div></div></div>
     <div class="pdfw"><div class="chrome"><div class="t"><span>${pdf} - PhiTeX</span><i>_</i><i class="mx">\u25A1</i><i>\xD7</i></div><div class="menu"><span><u>F</u>ile</span><span><u>E</u>dit</span><span><u>V</u>iew</span><span><u>W</u>indow</span><span><u>H</u>elp</span></div></div></div>
     <div class="task"><div class="start">\u229E Start</div><div class="btn on">PhiTeX Setup</div><div class="clock"></div></div>
     <div class="tip"><b>Did you know?</b><span></span></div>
     <div class="fly">${pdf} - PhiTeX</div>`
  );
  const $ = (s) => el.querySelector(s);
  const w = $(".w"), fly = $(".fly"), bar2 = $(".bar"), fill = $(".bar span"), btn = $(".btn"), tip = $(".tip");
  $(".clock").textContent = (/* @__PURE__ */ new Date()).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const step = (what, pct, detail) => {
    $(".msg").textContent = `${what}\u2026`;
    $(".det").textContent = detail;
    bar2.classList.toggle("march", pct === void 0);
    fill.style.width = pct === void 0 ? "" : `calc(${Math.round(pct / 5) * 5}% - 4px)`;
  };
  const hint = (text) => {
    tip.querySelector("span").textContent = text;
    tip.classList.remove("on");
    void tip.offsetWidth;
    tip.classList.add("on");
  };
  const timers = [];
  const at = (ms, f) => timers.push(window.setTimeout(f, ms));
  return ending(
    el,
    () => {
      step("Setup is complete", 100, "");
      $(".msg").textContent = "Setup is complete.";
      at(260, () => {
        w.style.display = "none";
        tip.style.display = "none";
        btn.textContent = `${doc.replace(/\.tex$/i, "")}.pdf - PhiTeX`;
        const b = box.getBoundingClientRect(), r = btn.getBoundingClientRect(), H2 = b.height, W2 = b.width;
        const to = { left: RESTORED.left / 100 * W2 - 3, top: RESTORED.top / 100 * H2 - $(".pdfw .chrome").offsetHeight, width: RESTORED.width / 100 * W2 + 6 };
        fly.style.cssText = `display: flex; left: ${r.left - b.left}px; top: ${r.top - b.top}px; width: ${r.width}px;`;
        fly.animate([{}, { left: `${to.left}px`, top: `${to.top}px`, width: `${to.width}px` }], { duration: 230, easing: "ease-out", fill: "forwards" });
      });
      at(500, () => (fly.style.display = "none", el.classList.add("desk")));
      at(760, () => $(".mx").classList.add("down"));
      at(840, () => ($(".mx").classList.remove("down"), el.classList.add("max")));
      return 1180;
    },
    () => timers.forEach(clearTimeout),
    { step, hint }
  );
}

// viewer/src/openings/mac9.ts
var RESTORED2 = { left: 8, top: 17, width: 84, height: 66 };
var DESK = "#6a6fb3";
function mac9(box, doc = "document.tex") {
  const pdf = esc3(doc.replace(/\.tex$/i, "") + ".pdf");
  const el = layer(
    box,
    "phx-m9",
    `.phx-m9 { background: ${DESK}; font: 600 3cqw/1.3 "Charcoal", "Chicago", "Geneva", system-ui, sans-serif; color: #000; }
.phx-m9 .mb { position: absolute; left: 0; right: 0; top: 0; height: 5cqw; background: #e8e8e8; border-bottom: 1px solid #000; display: flex; align-items: center; gap: 3.4cqw; padding: 0 2.4cqw; z-index: 3; transition: transform .3s cubic-bezier(.5,0,.2,1); }
.phx-m9 .mb .ap { width: 3cqw; height: 3.2cqw; border-radius: 50% 50% 45% 45%; background: linear-gradient(#61bb46 0 17%, #fdb827 17% 33%, #f5821f 33% 50%, #e03a3e 50% 67%, #963d97 67% 83%, #009ddc 83%); }
.phx-m9 .mb .ck { margin-left: auto; }
.phx-m9 .ic { position: absolute; right: 6%; top: 9%; width: 16cqw; text-align: center; font: 400 2.4cqw/1.2 "Geneva", system-ui, sans-serif; color: #fff; }
.phx-m9 .ic i { display: block; margin: 0 auto .8cqw; width: 7cqw; height: 9cqw; background: #fff linear-gradient(225deg, ${DESK} 1.8cqw, #000 1.8cqw 2cqw, #e4e4e4 2cqw 2.6cqw, transparent 2.6cqw); border: 1px solid #000; }
.phx-m9 .ic i::after { content: "PDF"; display: block; margin-top: 4.4cqw; font: 700 1.8cqw/1 system-ui, sans-serif; color: #c22; }
.phx-m9 .ic.open i { filter: brightness(.45); }
.phx-m9 .ic.open span { background: #000; }
.phx-m9 .w { position: absolute; left: 7%; right: 7%; top: 32%; background: #dedede; border: 1px solid #000; box-shadow: inset 1px 1px #fff, inset -1px -1px #999, .5cqw .5cqw 0 rgb(0 0 0 / 35%); }
.phx-m9 .t { height: 4.2cqw; margin: .5cqw; display: flex; align-items: center; justify-content: center; background: repeating-linear-gradient(#dedede 0 .4cqw, #9a9a9a .4cqw .55cqw, #fff .55cqw .7cqw); position: relative; }
.phx-m9 .t b { background: #dedede; padding: 0 1.6cqw; font-weight: 600; white-space: nowrap; }
.phx-m9 .t i { position: absolute; width: 2.7cqw; height: 2.7cqw; background: #dedede; border: 1px solid #555; box-shadow: inset 1px 1px #fff, inset -1px -1px #999; }
.phx-m9 .t .cl { left: 1cqw; } .phx-m9 .t .zm { right: 4.6cqw; } .phx-m9 .t .co { right: 1cqw; }
.phx-m9 .t .zm::after { content: ""; position: absolute; left: 0; top: 0; width: 55%; height: 55%; border-right: 1px solid #555; border-bottom: 1px solid #555; }
.phx-m9 .t .co::after { content: ""; position: absolute; left: 0; right: 0; top: 45%; border-top: 1px solid #555; }
.phx-m9 .t i.down { background: #888; }
.phx-m9 .b { padding: 3cqw 4cqw 3.4cqw; display: grid; grid-template-columns: auto 1fr; gap: 1.4cqw 3cqw; align-items: center; }
.phx-m9 .doc { width: 6cqw; height: 7.8cqw; background: #fff linear-gradient(225deg, #dedede 1.4cqw, #000 1.4cqw 1.6cqw, transparent 1.6cqw); border: 1px solid #000; grid-row: span 3; }
.phx-m9 .det { font-weight: 400; font-family: "Geneva", system-ui, sans-serif; font-size: 2.5cqw; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-height: 1.3em; }
.phx-m9 .bar { height: 2.6cqw; border: 1px solid #000; border-radius: 1.4cqw; background: #fff; overflow: hidden; box-shadow: inset 1px 1px 2px rgb(0 0 0 / 40%); }
.phx-m9 .bar span { display: block; height: 100%; width: 0; border-radius: inherit; background: linear-gradient(#bcd3fb, #5d8ee6 50%, #3f6fd0 51%, #8fb2f2); transition: width .3s; }
.phx-m9 .bar.pole span { width: 100%; background-image: repeating-linear-gradient(-45deg, #4f7fd4 0 1.2cqw, #a9c4f2 1.2cqw 2.4cqw); background-size: 3.4cqw 100%; animation: phx-m9-pole .6s linear infinite; }
@keyframes phx-m9-pole { to { background-position: 3.4cqw 0; } }
.phx-m9 .bal { position: absolute; left: 12%; right: 12%; top: 66%; background: #fff; border: 1px solid #000; border-radius: 2.4cqw; padding: 2cqw 2.6cqw; font: 400 2.5cqw/1.4 "Geneva", system-ui, sans-serif; box-shadow: .5cqw .5cqw 0 rgb(0 0 0 / 30%); visibility: hidden; }
.phx-m9 .bal::before { content: ""; position: absolute; left: 22%; top: -1.6cqw; width: 2.8cqw; height: 2.8cqw; background: #fff; border-left: 1px solid #000; border-top: 1px solid #000; transform: skewY(-28deg) rotate(45deg); transform-origin: 0 100%; }
.phx-m9 .bal b { display: block; font-weight: 700; margin-bottom: .4cqw; }
.phx-m9 .bal.on { visibility: visible; }
.phx-m9 .z { position: absolute; border: 1px dashed #000; display: none; z-index: 4; }
.phx-m9 .pdfw { position: absolute; display: none; left: ${RESTORED2.left}%; top: ${RESTORED2.top}%; width: ${RESTORED2.width}%; height: ${RESTORED2.height}%;
  box-shadow: 0 0 0 1px #888, 0 0 0 .5cqw #dedede, 0 0 0 calc(.5cqw + 1px) #000, .9cqw .9cqw 0 calc(.5cqw + 1px) rgb(0 0 0 / 30%), 0 0 0 999px ${DESK}; transition: all .32s cubic-bezier(.5,0,.2,1); }
.phx-m9 .pdfw .t { position: absolute; left: -.5cqw; right: -.5cqw; bottom: 100%; margin: 0; height: 4.8cqw; border: 1px solid #000; border-bottom: none; padding: 0; }
.phx-m9.desk { background: transparent; }
.phx-m9.desk .pdfw { display: block; }
.phx-m9.desk .ic { z-index: 0; }
.phx-m9.max .pdfw { left: 0; top: 0; width: 100%; height: 100%; }
.phx-m9.max .mb { transform: translateY(-110%); }
/* (the window is the page seen through it: the icon behind would show through as it grows) */
.phx-m9.max .ic { opacity: 0; transition: opacity .12s; }`,
    `<div class="mb"><span class="ap"></span><span>File</span><span>Edit</span><span>View</span><span>Special</span><span>Help</span><span class="ck"></span></div>
     <div class="ic"><i></i><span>${pdf}</span></div>
     <div class="w"><div class="t"><i class="cl"></i><b>Typesetting</b><i class="zm"></i><i class="co"></i></div><div class="b"><div class="doc"></div><div class="msg">Starting\u2026</div><div class="det"></div><div class="bar pole"><span></span></div></div></div>
     <div class="bal"><b>Did you know?</b><span></span></div>
     <div class="pdfw"><div class="t"><i class="cl"></i><b>${pdf}</b><i class="zm"></i><i class="co"></i></div></div>
     <div class="z"></div><div class="z"></div><div class="z"></div><div class="z"></div><div class="z"></div>`
  );
  const $ = (s) => el.querySelector(s);
  $(".ck").textContent = (/* @__PURE__ */ new Date()).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const bar2 = $(".bar"), fill = $(".bar span"), bal = $(".bal");
  const step = (what, pct, detail) => {
    $(".msg").textContent = `${what}\u2026`;
    $(".det").textContent = detail;
    bar2.classList.toggle("pole", pct === void 0);
    fill.style.width = pct === void 0 ? "" : `${pct}%`;
  };
  const hint = (text) => {
    bal.querySelector("span").textContent = text;
    const w = $(".w").getBoundingClientRect(), b = box.getBoundingClientRect();
    bal.style.top = `calc(${w.bottom - b.top}px + 2.6cqw)`;
    bal.classList.add("on");
  };
  const timers = [];
  const at = (ms, f) => timers.push(window.setTimeout(f, ms));
  return ending(
    el,
    () => {
      step("Done", 100, "");
      $(".msg").textContent = "Done.";
      at(220, () => ($(".w").style.display = "none", bal.style.display = "none", $(".ic").classList.add("open")));
      at(320, () => {
        const b = box.getBoundingClientRect(), i = $(".ic i").getBoundingClientRect(), W2 = b.width, H2 = b.height;
        const from2 = { left: i.left - b.left, top: i.top - b.top, width: i.width, height: i.height };
        const to = { left: RESTORED2.left / 100 * W2, top: RESTORED2.top / 100 * H2 - $(".pdfw .t").offsetHeight, width: RESTORED2.width / 100 * W2, height: RESTORED2.height / 100 * H2 + $(".pdfw .t").offsetHeight };
        const px = (r) => Object.fromEntries(Object.entries(r).map(([k, v]) => [k, `${v}px`]));
        el.querySelectorAll(".z").forEach((z, k) => {
          Object.assign(z.style, { display: "block", ...px(from2), opacity: "0" });
          z.animate([{ ...px(from2), opacity: 1 }, { ...px(to), opacity: 1 }, { ...px(to), opacity: 0 }], { duration: 260, delay: k * 35, easing: "steps(7, end)", fill: "forwards" });
        });
      });
      at(640, () => el.classList.add("desk"));
      at(860, () => el.querySelectorAll(".pdfw .zm").forEach((z) => z.classList.add("down")));
      at(940, () => (el.querySelectorAll(".pdfw .zm").forEach((z) => z.classList.remove("down")), el.classList.add("max")));
      return 1300;
    },
    () => timers.forEach(clearTimeout),
    { step, hint }
  );
}

// viewer/src/openings/terminal.ts
var SPIN = "|/-\\";
var BAR = 16;
function terminal(box, doc = "document.tex") {
  const el = layer(
    box,
    "phx-term",
    `${crt("phx-term", "radial-gradient(ellipse at 50% 40%, #12201a, #060a08 75%)", "rgb(120 255 170 / 60%)")}
.phx-term .txt { position: absolute; inset: 0; padding: 6cqw 6cqw; font: 2.5cqw/1.5 "DM Mono", ui-monospace, monospace; color: #b8f5c8; display: flex; flex-direction: column; justify-content: flex-end; }
.phx-term .l { white-space: pre-wrap; word-break: break-word; }
.phx-term .p { color: #7dffa0; } .phx-term .ok { color: #7dffa0; } .phx-term .d { color: #79c0ff; } .phx-term .f { color: #ffd27a; } .phx-term .h { color: #6f8f7c; font-style: italic; }
.phx-term .dim { color: #6f8f7c; }
.phx-term .cur { display: inline-block; width: .6em; height: 1.1em; vertical-align: -.2em; background: #7dffa0; animation: phx-term-blink 1s steps(1) infinite; }
@keyframes phx-term-blink { 50% { opacity: 0; } }`,
    `<div class="scr"><div class="txt"></div><div class="band"></div></div><div class="led"></div><div class="brand">PHITEX</div>`
  );
  const txt = el.querySelector(".txt");
  const cursor = document.createElement("span");
  cursor.className = "cur";
  const t0 = performance.now();
  const line = (html, cls = "") => {
    const l = document.createElement("div");
    l.className = `l ${cls}`;
    l.innerHTML = html;
    txt.append(l);
    l.append(cursor);
    while (txt.scrollHeight > txt.clientHeight + 2 && txt.children.length > 1) txt.firstElementChild.remove();
    return l;
  };
  const esc6 = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
  const cmd = `phitex watch ${doc}`, prompt = line(`<span class="p">$ </span>`);
  let typed = 0, timer = 0;
  const key = () => {
    prompt.insertBefore(document.createTextNode(cmd[typed++]), cursor);
    if (typed < cmd.length) timer = window.setTimeout(key, 22 + Math.random() * 30);
    else timer = 0, flush();
  };
  timer = window.setTimeout(key, 420);
  let queue = [];
  const later = (f) => timer || typed < cmd.length ? queue.push(f) : f();
  const flush = () => {
    line(`<span class="d">PhiTeX</span>`);
    queue.forEach((f) => f());
    queue = [];
  };
  let cur = null, spin = 0, spinner = 0;
  const step = (what, pct, detail) => later(() => {
    if (cur?.what !== what) {
      if (cur) cur.el.innerHTML = `<span class="ok">\u2713</span> ${esc6(cur.what.toLowerCase())}`;
      cur = { what, el: line(""), seen: /* @__PURE__ */ new Set() };
    }
    const name = what.toLowerCase();
    if (pct !== void 0) {
      const pkg = detail.split(" \xB7 ")[0];
      if (pkg && !cur.seen.has(pkg)) {
        cur.seen.add(pkg);
        cur.el.before(Object.assign(document.createElement("div"), { className: "l dim", textContent: `  + ${pkg}` }));
      }
      const n = Math.round(pct / 100 * BAR);
      cur.el.innerHTML = `<span class="f">${name}</span> [${"\u2588".repeat(n)}${"\u2591".repeat(BAR - n)}] ${esc6(detail.split(" \xB7 ").slice(1).join("  "))}`;
    } else cur.el.innerHTML = `<span class="f">${SPIN[spin % SPIN.length]}</span> ${name}  <span class="dim">${esc6(detail)}</span>`;
    cur.el.append(cursor);
    clearInterval(spinner);
    if (pct === void 0)
      spinner = window.setInterval(() => {
        const s = cur?.el.querySelector(".f");
        if (s) s.textContent = SPIN[++spin % SPIN.length];
      }, 80);
  });
  const hint = (text) => later(() => cur?.el.before(Object.assign(document.createElement("div"), { className: "l h", textContent: `# ${text}` })));
  const cleanup = () => (clearTimeout(timer), clearInterval(spinner));
  return ending(
    el,
    () => {
      cleanup();
      if (cur) cur.el.innerHTML = `<span class="ok">\u2713</span> ${esc6(cur.what.toLowerCase())}`;
      line(`<span class="ok">\u2713 page 1 in ${((performance.now() - t0) / 1e3).toFixed(1)} s</span>`);
      setTimeout(() => crtOff(el), 180);
      return 700;
    },
    cleanup,
    { step, hint }
  );
}

// viewer/src/openings/dos.ts
var COLS = 26;
function dos(box) {
  const el = layer(
    box,
    "phx-dos",
    `${crt("phx-dos", "#0000a8", "rgb(170 190 255 / 55%)")}
.phx-dos .txt { position: absolute; inset: 0; color: #a8a8a8; font: 2.75cqw/1.6 "DM Mono", ui-monospace, monospace; }
.phx-dos .top { background: #a8a8a8; color: #000; padding: 1.5cqw 4cqw 0; text-shadow: none; }
.phx-dos .main { padding: 4cqw 5cqw; }
.phx-dos .main p { margin: 0 0 2cqw; color: #fff; }
.phx-dos .box { border: .8cqw double #fff; padding: 2cqw 3cqw; margin: 4cqw 0 3cqw; }
.phx-dos .now { color: #55ffff; }
.phx-dos .bar { color: #ffff55; white-space: pre; }
.phx-dos .pct { text-align: center; color: #fff; }
.phx-dos .file { color: #fff; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-dos .tip { border: .4cqw solid #a8a8a8; padding: 1.5cqw 3cqw; margin-top: 3cqw; color: #ffff55; visibility: hidden; }
.phx-dos .tip b { color: #fff; font-weight: normal; }
.phx-dos .bot { position: absolute; left: 0; right: 0; bottom: 0; background: #a8a8a8; color: #000; padding: 0 4cqw 1.2cqw; white-space: pre; text-shadow: none; }
.phx-dos.clear { background: transparent; transition: background .3s; }
.phx-dos.clear .led, .phx-dos.clear .brand { opacity: 0; transition: opacity .2s; }
.phx-dos.clear .scr { box-shadow: none; animation: phx-dos-clear .5s steps(14, end) forwards; }
@keyframes phx-dos-clear { from { clip-path: inset(0 0 0 0); } to { clip-path: inset(100% 0 0 0); } }`,
    `<div class="scr"><div class="txt"><div class="top">PhiTeX Setup</div>
     <div class="main"><p>Please wait while Setup typesets your document.</p>
     <div class="box"><div class="now">Starting\u2026</div><div class="bar"></div><div class="pct">&nbsp;</div><div class="file">&nbsp;</div></div>
     <div class="tip"><b>Did you know?</b> <span></span></div></div>
     <div class="bot">  ENTER=Continue   F1=Help   F3=Exit</div></div><div class="band"></div></div>
     <div class="led"></div><div class="brand">PHITEX</div>`
  );
  const $ = (s) => el.querySelector(s);
  const now = $(".now"), bar2 = $(".bar"), pct = $(".pct"), file = $(".file"), tip = $(".tip");
  let known2, pos = 0;
  const draw = () => {
    if (known2 !== void 0) {
      const n = Math.round(known2 / 100 * COLS);
      bar2.textContent = "\u2588".repeat(n) + "\u2591".repeat(COLS - n);
      pct.textContent = `${Math.round(known2)}%`;
    } else {
      const p = Math.abs(pos++ % ((COLS - 4) * 2) - (COLS - 4));
      bar2.textContent = "\u2591".repeat(p) + "\u2593\u2593\u2593\u2593" + "\u2591".repeat(COLS - 4 - p);
      pct.textContent = " ";
    }
  };
  draw();
  const tick = window.setInterval(() => known2 === void 0 && draw(), 90);
  const step = (what, p, detail) => {
    now.textContent = what;
    known2 = p;
    file.textContent = detail ? p !== void 0 ? `Copying: ${detail.split(" \xB7 ")[0].toUpperCase()}` : detail : " ";
    draw();
  };
  const hint = (text) => {
    tip.style.visibility = "visible";
    tip.querySelector("span").textContent = text;
  };
  return ending(
    el,
    () => {
      clearInterval(tick);
      step("Setup is complete.", 100, "");
      setTimeout(() => el.classList.add("clear"), 220);
      return 740;
    },
    () => clearInterval(tick),
    { step, hint }
  );
}

// viewer/src/openings/typewriter.ts
function typewriter(box, dark, doc = "document.tex") {
  const el = layer(
    box,
    "phx-tw",
    `.phx-tw { background: ${dark ? PAPER.dark : "#fbf8f1"}; color: ${dark ? "#d8d4ca" : "#2a2622"}; transition: background .5s ease; }
.phx-tw .roll { position: absolute; left: 9cqw; right: 8cqw; top: 12cqw; font: 3.1cqw/1.55 "DM Mono", ui-monospace, monospace; white-space: pre-wrap; transition: transform .25s cubic-bezier(.3,.6,.3,1); }
.phx-tw .roll > div { min-height: 1.55em; }
.phx-tw .red { color: ${dark ? "#e07a66" : "#b0352a"}; }
.phx-tw .k { animation: phx-tw-strike .09s ease-out; }
@keyframes phx-tw-strike { from { opacity: .35; } }
.phx-tw .cur { display: inline-block; width: .6em; height: 1.05em; vertical-align: -.18em; background: currentColor; opacity: .5; animation: phx-tw-blink 1s steps(1) infinite; }
@keyframes phx-tw-blink { 50% { opacity: 0; } }
.phx-tw .ding { position: absolute; right: 3cqw; font: 2.2cqw "DM Mono", monospace; opacity: 0; color: #b0352a; }
.phx-tw .ding.on { animation: phx-tw-ding .5s ease-out; }
@keyframes phx-tw-ding { 0% { opacity: 1; transform: translateY(0); } 100% { opacity: 0; transform: translateY(-3cqw); } }
.phx-tw.out { background: transparent; }
.phx-tw.out .roll { filter: blur(.35cqw); opacity: 0; transition: filter .45s ease-in, opacity .5s ease-in; }
.phx-tw.out .cur, .phx-tw.out .ding { display: none; }`,
    `<div class="roll"></div><div class="ding">ding</div>`
  );
  const roll = el.querySelector(".roll"), ding = el.querySelector(".ding");
  const cursor = document.createElement("span");
  cursor.className = "cur";
  const queue = [];
  let line, run, red = false, timer = 0;
  const feed = () => {
    const over = roll.offsetTop + roll.scrollHeight - box.clientHeight * 0.72;
    roll.style.transform = `translateY(${-Math.max(0, over)}px)`;
  };
  const newLine = () => {
    if (line && line.textContent.length > 30) {
      ding.style.top = `${line.getBoundingClientRect().top - box.getBoundingClientRect().top}px`;
      ding.classList.remove("on");
      void ding.offsetWidth;
      ding.classList.add("on");
    }
    line = document.createElement("div");
    roll.append(line);
    newRun();
    feed();
  };
  const newRun = () => {
    run = document.createElement("span");
    if (red) run.className = "red";
    line.append(run);
    line.append(cursor);
  };
  newLine();
  const type = (...parts) => {
    queue.push(...parts);
    if (!timer) timer = window.setTimeout(key, 60);
  };
  const key = () => {
    timer = 0;
    const next = queue.shift();
    if (next === void 0) return;
    if (next === "\n") newLine();
    else if (typeof next === "object") red = next.red, newRun();
    else {
      const k = document.createElement("span");
      k.className = "k";
      k.textContent = next;
      run.append(k);
      if (line.offsetHeight > 0) feed();
    }
    const pace = queue.length > 40 ? 6 : 18 + Math.random() * 30;
    timer = window.setTimeout(key, next === "\n" ? 200 : next === " " ? pace + 20 : pace);
  };
  const chars = (s) => [...s];
  type(...chars(doc), "\n", ...chars("\u2500".repeat(24)), "\n");
  let what = "", pkgs = /* @__PURE__ */ new Set(), page = "";
  const step = (w, pct, detail) => {
    if (w !== what) {
      if (what) type("\n");
      what = w;
      type(...chars(`${w}\u2026`));
      if (pct === void 0 && detail && !/^Page /.test(detail)) type(...chars(` ${detail}`));
    }
    const pkg = pct !== void 0 ? detail.split(" \xB7 ")[0] : "";
    if (pkg && !pkgs.has(pkg)) pkgs.add(pkg), type(...chars(`${pkgs.size > 1 ? "," : ""} ${pkg}`));
    if (/^Page \d+/.test(detail) && detail !== page) page = detail, type(...chars(` p.${detail.slice(5)}`));
  };
  const hint = (text) => type("\n", "\n", { red: true }, ...chars(`Did you know? ${text}`), { red: false }, "\n");
  return ending(el, () => (clearTimeout(timer), el.classList.add("out"), 520), () => clearTimeout(timer), { step, hint });
}

// viewer/src/openings/ink.ts
var AT = 34;
function ink(box, dark) {
  const c = dark ? "#9fb7e0" : INK.light;
  const el = layer(
    box,
    "phx-ink",
    `.phx-ink { background: ${dark ? PAPER.dark : PAPER.light}; color: ${c}; }
.phx-ink .drop { position: absolute; left: 50%; top: -6cqw; width: 3.4cqw; height: 4.6cqw; margin-left: -1.7cqw; background: ${c}; border-radius: 50% 50% 50% 50% / 60% 60% 40% 40%; animation: phx-ink-fall .55s cubic-bezier(.5,0,.9,.6) forwards; }
@keyframes phx-ink-fall { 0% { transform: translateY(0); } 97% { opacity: 1; } 100% { transform: translateY(${AT * 1.294 + 4}cqw); opacity: 0; } }
.phx-ink .blot { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; background: ${c}; transform: scale(var(--s, .08)); transition: transform .5s cubic-bezier(.3,1.6,.5,1); animation: phx-ink-wob 2.6s ease-in-out infinite; }
@keyframes phx-ink-wob { 0%, 100% { border-radius: 52% 48% 50% 50% / 50% 52% 48% 50%; } 33% { border-radius: 58% 42% 55% 45% / 45% 58% 42% 55%; } 66% { border-radius: 44% 56% 47% 53% / 56% 44% 58% 42%; } }
.phx-ink .ring { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; border: .35cqw solid ${c}; border-radius: 50%; animation: phx-ink-ring 1.1s ease-out forwards; }
@keyframes phx-ink-ring { 0% { transform: scale(calc(var(--s, .1) * .9)); opacity: .55; } 100% { transform: scale(calc(var(--s, .1) * 2.2 + .4)); opacity: 0; } }
.phx-ink .cap { position: absolute; left: 10%; right: 10%; top: ${AT + 18}%; text-align: center; font-family: "CMU Serif", Georgia, serif; }
.phx-ink .what { font-size: 4.4cqw; font-style: italic; }
.phx-ink .det { font-size: 2.6cqw; margin-top: 1cqw; opacity: .75; min-height: 1.3em; }
.phx-ink .hint { font-size: 2.5cqw; font-style: italic; margin-top: 5cqw; line-height: 1.4; opacity: .7; }
.phx-ink .bleed { animation: phx-ink-bleed .5s ease-out; }
@keyframes phx-ink-bleed { from { filter: blur(.7cqw); opacity: 0; letter-spacing: .08em; } }
.phx-ink .flood { position: absolute; left: 50%; top: ${AT}%; width: 30cqw; height: 30cqw; margin: -15cqw; background: ${c}; border-radius: 50%; transform: scale(0); }
.phx-ink.out .flood { animation: phx-ink-flood .38s cubic-bezier(.5,0,.7,1) forwards; }
.phx-ink.out .cap { opacity: 0; transition: opacity .2s; }
@keyframes phx-ink-flood { to { transform: scale(10); } }`,
    `<div class="blot"></div><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="flood"></div>`
  );
  const $ = (s) => el.querySelector(s);
  const blot = $(".blot");
  const write = (e, s) => {
    if (e.textContent === s) return;
    e.textContent = s;
    e.classList.remove("bleed");
    void e.offsetWidth;
    e.classList.add("bleed");
  };
  const size = (s) => (el.style.setProperty("--s", String(s)), blot.style.setProperty("--s", String(s)));
  const drop = () => {
    const d = document.createElement("div");
    d.className = "drop";
    el.insertBefore(d, $(".cap"));
    setTimeout(() => {
      d.remove();
      const r = document.createElement("div");
      r.className = "ring";
      el.insertBefore(r, blot);
      setTimeout(() => r.remove(), 1150);
    }, 540);
  };
  let seen = /* @__PURE__ */ new Set(), rain = 0, what = "";
  const step = (w, pct, detail) => {
    write($(".what"), w);
    write($(".det"), pct !== void 0 ? detail.split(" \xB7 ").slice(0, 2).join(" \xB7 ") : detail);
    const pkg = pct !== void 0 ? detail.split(" \xB7 ")[0] : "";
    if (pkg && !seen.has(pkg)) seen.add(pkg), drop();
    if (w !== what) {
      what = w;
      clearInterval(rain);
      if (pct === void 0 && seen.size) rain = window.setInterval(drop, 1100);
    }
    size(pct !== void 0 ? 0.25 + 0.6 * (pct / 100) : seen.size ? 0.9 : 0.12);
  };
  const hint = (text) => write($(".hint"), `Did you know? ${text}`);
  return ending(
    el,
    () => {
      clearInterval(rain);
      el.classList.add("out");
      setTimeout(() => hole(el, 50, AT, 480), 330);
      return 840;
    },
    () => clearInterval(rain),
    { step, hint }
  );
}

// viewer/src/openings/tetris.ts
var COLS2 = 10;
var SHAPES = [
  [[0, 0], [1, 0], [2, 0], [3, 0]],
  [[0, 0], [1, 0], [2, 0], [0, 1]],
  [[0, 0], [1, 0], [2, 0], [2, 1]],
  [[0, 0], [1, 0], [0, 1], [1, 1]],
  [[1, 0], [2, 0], [0, 1], [1, 1]],
  [[0, 0], [1, 0], [1, 1], [2, 1]],
  [[0, 0], [1, 0], [2, 0], [1, 1]]
];
function tetris(box, dark) {
  const fg = dark ? "#e4e6ea" : INK.light, mute = dark ? "#8d96a5" : "#6b7486";
  const el = layer(
    box,
    "phx-tet",
    `.phx-tet { background: ${dark ? PAPER.dark : PAPER.light}; color: ${fg}; font: 2.6cqw/1.3 "DM Mono", ui-monospace, monospace; text-transform: uppercase; transition: background .3s ease .2s; }
.phx-tet canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.phx-tet .hud { position: absolute; left: 64%; right: 5%; top: 6cqw; display: grid; gap: 3.4cqw; transition: opacity .25s; }
.phx-tet .hud b { display: block; font-weight: 400; font-size: 2.1cqw; color: ${mute}; letter-spacing: .08em; margin-bottom: .6cqw; }
.phx-tet .hud .v { font-size: 2.9cqw; line-height: 1.25; text-transform: none; min-height: 2.5em; }
.phx-tet .meter { display: grid; grid-template-columns: repeat(8, 1fr); gap: .5cqw; }
.phx-tet .meter i { height: 2.4cqw; border-radius: .4cqw; background: ${dark ? "#3d4047" : "#e7e9ee"}; transition: background .15s; }
.phx-tet .meter i.on { background: ${RIBBON[3]}; }
.phx-tet .meter.run i { animation: phx-tet-run 1s steps(1) infinite; }
@keyframes phx-tet-run { 50% { background: ${RIBBON[2]}; } }
.phx-tet .next { height: 9cqw; }
.phx-tet .tick { position: absolute; left: 5%; right: 5%; bottom: 5cqw; overflow: hidden; white-space: nowrap; border-top: 1px solid ${dark ? "#3d4047" : "#d6dae1"}; padding-top: 1.6cqw; text-transform: none; color: ${mute}; transition: opacity .25s; }
.phx-tet .tick span { display: inline-block; padding-left: 100%; animation: phx-tet-tick 14s linear infinite; }
@keyframes phx-tet-tick { to { transform: translateX(-100%); } }
.phx-tet.out { background: transparent; }
.phx-tet.out .hud, .phx-tet.out .tick { opacity: 0; }`,
    `<canvas></canvas><div class="hud"><div><b>Stage</b><div class="v st">Ready</div></div><div><b>Packages</b><div class="meter">${"<i></i>".repeat(8)}</div></div><div><b>Page</b><div class="v pg">\u2013</div></div><div><b>Next</b><div class="next"></div></div></div>
     <div class="tick"><span></span></div>`
  );
  const $ = (s) => el.querySelector(s);
  const cv = el.querySelector("canvas"), ctx = cv.getContext("2d");
  const colors = [dark ? "#9fb7e0" : INK.light, ...RIBBON];
  const dpr = devicePixelRatio || 1;
  const W2 = cv.width = box.clientWidth * dpr, H2 = cv.height = box.clientHeight * dpr;
  const cell = W2 * 0.56 / COLS2, ROWS = Math.floor(H2 * 0.8 / cell), ox = W2 * 0.05, oy = W2 * 0.06;
  let grid = Array.from({ length: ROWS }, () => Array(COLS2).fill(null));
  const make = () => {
    const cells = SHAPES[Math.floor(Math.random() * SHAPES.length)];
    return { cells, x: 0, y: -2, c: colors[Math.floor(Math.random() * colors.length)] };
  };
  const fits = (p, x, y) => p.cells.every(([cx, cy]) => {
    const gx = x + cx, gy = y + cy;
    return gx >= 0 && gx < COLS2 && gy < ROWS && (gy < 0 || !grid[gy][gx]);
  });
  const aim = (p) => {
    let best = 0, by = -1;
    for (let x = 0; x < COLS2; x++) {
      if (!fits(p, x, 0)) continue;
      let y = 0;
      while (fits(p, x, y + 1)) y++;
      if (y > by || y === by && Math.random() < 0.3) by = y, best = x;
    }
    return best;
  };
  let piece = make(), next = make(), target = aim(piece), acc = 0, clearing = [], flash = 0, out = -1, speed = 45;
  const block = (x, y, s, c, k = 0) => {
    const g = s * 0.08, h = (s - 2 * g) * (1 - 0.8 * k);
    ctx.fillStyle = c;
    ctx.beginPath();
    ctx.roundRect(x + g, y + g + (s - 2 * g - h) / 2, s - 2 * g, h, s * 0.12 * (1 - k));
    ctx.fill();
  };
  const at = (x, y, c, k = 0) => block(ox + x * cell, oy + y * cell, cell, c, k);
  const drawNext = () => {
    const r = $(".next").getBoundingClientRect(), b = box.getBoundingClientRect(), s = cell * 0.7;
    next.cells.forEach(([cx, cy]) => block((r.left - b.left) * dpr + cx * s, (r.top - b.top) * dpr + cy * s, s, next.c));
  };
  const t0 = performance.now();
  const stopLoop = loop((t, dt) => {
    ctx.clearRect(0, 0, W2, H2);
    ctx.fillStyle = dark ? "#3d4047" : "#e7e9ee";
    if (out < 0) ctx.fillRect(ox - 2 * dpr, oy + ROWS * cell + dpr, COLS2 * cell + 4 * dpr, 2 * dpr);
    if (out >= 0) {
      const k = Math.min(1, (t - out) / 300), f = Math.max(0, (t - out - 250) / 300);
      ctx.globalAlpha = 1 - f;
      grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, dark ? "#c8cdd6" : "#333", k)));
      ctx.globalAlpha = 1;
      return;
    }
    drawNext();
    if (clearing.length) {
      flash += dt;
      grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, clearing.includes(y) && Math.floor(flash / 70) % 2 ? "#fff" : c)));
      if (flash > 280) {
        for (const y of clearing.sort((a, b) => a - b)) grid.splice(y, 1), grid.unshift(Array(COLS2).fill(null));
        clearing = [], flash = 0;
      }
      return;
    }
    acc += dt;
    while (acc > speed) {
      acc -= speed;
      if (piece.x !== target && piece.y >= -1) piece.x += Math.sign(target - piece.x);
      else if (fits(piece, piece.x, piece.y + 1)) piece.y++;
      else {
        if (piece.y < 0) grid = grid.map(() => Array(COLS2).fill(null));
        else piece.cells.forEach(([cx, cy]) => grid[piece.y + cy][piece.x + cx] = piece.c);
        clearing = grid.flatMap((row, y) => row.every(Boolean) ? [y] : []);
        piece = next, next = make(), target = aim(piece);
        break;
      }
    }
    grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, c)));
    piece.cells.forEach(([cx, cy]) => piece.y + cy >= 0 && at(piece.x + cx, piece.y + cy, piece.c));
  });
  const meter = $(".meter");
  const step = (what, pct, detail) => {
    $(".st").textContent = what;
    const n = pct === void 0 ? what === "Typesetting" ? 8 : 0 : Math.round(pct / 100 * 8);
    meter.querySelectorAll("i").forEach((m, i) => m.classList.toggle("on", i < n));
    meter.classList.toggle("run", pct === void 0 && what !== "Typesetting");
    if (/^Page \d+/.test(detail)) $(".pg").textContent = detail.slice(5);
    speed = what === "Typesetting" ? 28 : 45;
  };
  const hint = (text) => {
    const s = $(".tick span");
    s.textContent = `Did you know? ${text}`;
    s.style.animation = "none";
    void s.offsetWidth;
    s.style.animation = "";
  };
  return ending(el, () => (out = performance.now() - t0, el.classList.add("out"), 560), stopLoop, { step, hint });
}

// viewer/src/openings/plane.ts
var FLAT = [[0, -19.5], [15, -19.5], [15, 19.5], [0, 19.5], [-15, 19.5], [-15, -19.5]];
var HOUSE = [[0, -19.5], [15, -4.5], [15, 19.5], [0, 19.5], [-15, 19.5], [-15, -4.5]];
var NARROW = [[0, -19.5], [7, -2], [8, 19.5], [0, 19.5], [-8, 19.5], [-7, -2]];
var DART = [[0, -19.2], [3.1, -12], [10, 14.4], [0, 7.2], [-10, 14.4], [-3.1, -12]];
var SHAPES2 = [FLAT, HOUSE, NARROW, DART];
var NEXT = [[[0, -19.5], [15, -4.5], [0, -19.5], [-15, -4.5]], [[0, -19.5], [7, -2], [0, -19.5], [-7, -2]], [[0, -19.5], [0, 19.5]]];
var PAGE = [[42.5, 0], [85, 0], [85, 110], [42.5, 110], [0, 110], [0, 0]];
var PATH = "M42.5 40 C 70 8, 84 48, 60 62 C 40 74, 22 50, 42.5 40 C 63 30, 50 0, 26 8 C 6 15, 12 40, 42.5 40 Z";
var HOME = { x: 42.5, y: 38, r: 0, s: 1 };
var FLY = 0.42;
var STEPS = ["Reading the project", "Fetching packages", "Typesetting"];
var ease = (u) => (u = Math.min(1, Math.max(0, u)), u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2);
var lerp = (a, b, u) => a + (b - a) * u;
var mix = (a, b, u) => a.map((p, i) => [lerp(p[0], b[i][0], u), lerp(p[1], b[i][1], u)]);
var place2 = (pts, o) => pts.map(([x, y]) => [o.x + o.s * (x * Math.cos(o.r) - y * Math.sin(o.r)), o.y + o.s * (x * Math.sin(o.r) + y * Math.cos(o.r))]);
var str = (pts) => pts.map((p) => p.map((v) => v.toFixed(2)).join(",")).join(" ");
var folded = (f) => {
  const i = Math.min(2, Math.floor(f));
  return mix(SHAPES2[i], SHAPES2[i + 1], ease(f - i));
};
function plane(box, dark) {
  const c = dark ? "#c9d4ea" : INK.light, paper = dark ? "#e9edf5" : "#fff", bg = dark ? PAPER.dark : "#eef3f8";
  const grid = dark ? "rgb(255 255 255 / 5%)" : "rgb(29 62 114 / 7%)", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-pl",
    `.phx-pl { font-family: "Noto Sans", system-ui, sans-serif; color: ${dark ? "#e4e6ea" : "#1b222c"}; }
.phx-pl svg { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-pl .cap { position: absolute; left: 10%; right: 10%; top: 68%; transition: opacity .2s; }
.phx-pl .cap .what { font-size: 3.2cqw; font-weight: 600; display: flex; align-items: center; gap: 1.6cqw; }
.phx-pl .cap .what i { flex: none; width: 4.4cqw; height: 4.4cqw; border-radius: 50%; border: .35cqw solid currentColor; display: grid; place-items: center; font: 700 2.4cqw/1 "Noto Sans", system-ui, sans-serif; font-style: normal; }
.phx-pl .cap .det { font-size: 2.4cqw; color: ${mute}; margin: .8cqw 0 0 6cqw; min-height: 1.4em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-pl .cap .hint { font-size: 2.3cqw; color: ${mute}; margin: 3cqw 0 0 6cqw; line-height: 1.45; }
.phx-pl.out .cap { opacity: 0; }`,
    `<svg viewBox="0 0 85 110" preserveAspectRatio="xMidYMid meet">
      <defs>
        <pattern id="phx-pl-grid" width="5" height="5" patternUnits="userSpaceOnUse"><path d="M5 0H0V5" fill="none" stroke="${grid}" stroke-width=".25"/></pattern>
        <mask id="phx-pl-m" maskUnits="userSpaceOnUse" x="-50" y="-50" width="185" height="210"><rect x="-50" y="-50" width="185" height="210" fill="#fff"/><polygon class="hole" points="" fill="#000"/></mask>
      </defs>
      <g mask="url(#phx-pl-m)"><rect x="-50" y="-50" width="185" height="210" fill="${bg}"/><rect class="mat" x="-50" y="-50" width="185" height="210" fill="url(#phx-pl-grid)"/></g>
      <path class="path" d="${PATH}" fill="none" stroke="none"/>
      <path class="trail" fill="none" stroke="${c}" stroke-width=".35" stroke-dasharray="1.2 1.6" opacity=".4"/>
      <polygon class="shadow" fill="rgb(0 0 0 / 10%)"/>
      <polygon class="sheet" fill="${paper}" stroke="${c}" stroke-width=".45" stroke-linejoin="round"/>
      <polygon class="sh-l" fill="#000" opacity="0"/>
      <path class="crease" fill="none" stroke="${c}" stroke-width=".3"/>
      <path class="next" fill="none" stroke="${c}" stroke-width=".35" stroke-dasharray="1.4 1"/>
    </svg>
    <div class="cap"><div class="what"><i>1</i><span>Starting</span></div><div class="det"></div><div class="hint"></div></div>`
  );
  const $ = (s) => el.querySelector(s);
  const sheet = $(".sheet"), shadow = $(".shadow"), shL = $(".sh-l"), crease = $(".crease"), next = $(".next"), hole2 = $(".hole"), trail = $(".trail");
  const path = $(".path"), LEN = path.getTotalLength();
  let f = 0, want = 0, flying = -1, out = -1, raf = 0, last = performance.now();
  let from2 = null;
  const trailPts = [];
  const poseOnPath = (t) => {
    const d = t / 3600 * LEN % LEN, a = path.getPointAtLength(d), b = path.getPointAtLength((d + 0.5) % LEN);
    return { x: a.x, y: a.y, r: Math.atan2(b.y - a.y, b.x - a.x) + Math.PI / 2, s: FLY };
  };
  const draw = (pts, pose, lift) => {
    const w = place2(pts, pose);
    sheet.setAttribute("points", str(w));
    shadow.setAttribute("points", str(place2(pts, { ...pose, x: pose.x + 1.2 + 3 * lift, y: pose.y + 1.6 + 5 * lift })));
    return w;
  };
  const frame = (now) => {
    const dt = Math.min(50, now - last);
    last = now;
    if (out >= 0 && from2) {
      const t = now - out, SWING = 260, a = ease(t / SWING);
      let dr = HOME.r - from2.pose.r;
      dr = ((dr + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
      const pose2 = { x: lerp(from2.pose.x, 42.5, a), y: lerp(from2.pose.y, 52, a), r: from2.pose.r + dr * a, s: lerp(from2.pose.s, 1, a) };
      const u = Math.max(0, (t - SWING) / 480);
      const flown = place2(from2.shape, pose2);
      const p = flown.map(([fx, fy], i2) => {
        const k = ease((u - (i2 === 1 || i2 === 5 ? 0 : i2 === 0 ? 0.05 : 0.18)) / 0.82);
        return [lerp(fx, PAGE[i2][0], k), lerp(fy, PAGE[i2][1], k)];
      });
      const flat = ease(u);
      hole2.setAttribute("points", str(p));
      sheet.setAttribute("points", str(p));
      sheet.setAttribute("fill", "none");
      sheet.setAttribute("opacity", String(1 - flat));
      shadow.setAttribute("opacity", "0");
      trail.setAttribute("opacity", String(0.4 * (1 - a)));
      shL.setAttribute("points", str([p[0], p[3], p[4], p[5]]));
      shL.setAttribute("opacity", String(0.14 * (1 - flat)));
      crease.setAttribute("d", `M${p[0]} L${p[3]} M${p[0]} L${p[2]} M${p[0]} L${p[4]}`);
      crease.setAttribute("opacity", String(0.6 * (1 - flat)));
      next.setAttribute("d", "");
      if (u < 1) raf = requestAnimationFrame(frame);
      return;
    }
    f += Math.sign(want - f) * Math.min(Math.abs(want - f), dt / 400);
    const shape = folded(f);
    let pose = HOME, lift = 0;
    if (f >= 3 && want >= 3) {
      if (flying < 0) flying = now;
      const k = ease((now - flying) / 700), on = poseOnPath(now - flying);
      let dr = on.r - HOME.r;
      dr = ((dr + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
      pose = { x: lerp(HOME.x, on.x, k), y: lerp(HOME.y, on.y, k), r: HOME.r + dr * k, s: lerp(HOME.s, FLY, k) };
      lift = k;
      trailPts.push([pose.x, pose.y]);
      if (trailPts.length > 90) trailPts.shift();
      trail.setAttribute("d", trailPts.length > 1 ? `M${trailPts.map((q) => q.join(" ")).join(" L")}` : "");
    }
    const w = draw(shape, pose, lift);
    from2 = { pose, shape };
    crease.setAttribute("d", `M${w[0]} L${w[3]}`);
    crease.setAttribute("opacity", ".55");
    shL.setAttribute("points", f > 2 ? str([w[0], w[3], w[4], w[5]]) : "");
    shL.setAttribute("opacity", String(0.12 * Math.max(0, f - 2)));
    const i = Math.floor(f + 1e-6);
    next.setAttribute("d", i < 3 && Math.abs(f - i) < 0.02 ? NEXT[i].reduce((d, q, j) => d + `${j % 2 ? "L" : "M"}${place2([q], pose)[0]} `, "") : "");
    raf = requestAnimationFrame(frame);
  };
  raf = requestAnimationFrame(frame);
  const step = (what, pct, detail) => {
    const k = Math.max(0, STEPS.indexOf(what));
    $(".what i").textContent = String(k + 1);
    $(".what span").textContent = what;
    $(".det").textContent = detail;
    want = k === 0 ? 0 : k === 1 ? 2.6 * ((pct ?? 0) / 100) : 3;
  };
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(
    el,
    () => {
      out = performance.now();
      el.classList.add("out");
      return 800;
    },
    () => cancelAnimationFrame(raf),
    { step, hint }
  );
}

// viewer/src/openings/glyphs.ts
var GLYPHS = [..."\u2211\u222B\u2202\u2207\u03C0\u03C6\u03A6\u03B1\u03B2\u03B3\u03BB\u03BC\u03C3\u03B8\u221A\u221E\u2248\u2260\u2264\u2265\xA7\xB6{}[]\\$&^_#%\uFB01\uFB02"];
function glyphs(box, dark) {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-gl",
    `.phx-gl { background: ${dark ? PAPER.dark : PAPER.light}; transition: background .45s ease .2s; } .phx-gl canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; } .phx-gl.out { background: transparent; }
.phx-gl .cap { position: absolute; left: 8%; right: 8%; top: 34%; text-align: center; font-family: "CMU Serif", Georgia, serif; color: ${ink0}; transition: opacity .25s; }
.phx-gl .cap .what { font-size: 5.2cqw; text-shadow: 0 0 1.2cqw ${dark ? PAPER.dark : "#fff"}, 0 0 .4cqw ${dark ? PAPER.dark : "#fff"}; }
.phx-gl .cap .det { font-size: 2.7cqw; color: ${mute}; margin-top: 1cqw; min-height: 1.3em; }
.phx-gl .cap .hint { font-size: 2.6cqw; font-style: italic; color: ${mute}; margin-top: 5cqw; line-height: 1.4; }
.phx-gl .shelf { position: absolute; left: 8%; right: 8%; bottom: 9%; display: flex; flex-wrap: wrap-reverse; justify-content: center; align-content: flex-start; gap: .8cqw 1.4cqw; padding-bottom: 1.4cqw; border-bottom: .5cqw solid ${dark ? "#4a4e57" : "#c9ced8"}; transition: opacity .25s; }
.phx-gl .shelf span { font: 2.4cqw/1.4 "DM Mono", ui-monospace, monospace; padding: .2cqw 1.2cqw; border-radius: .6cqw; color: #fff; animation: phx-gl-land .7s cubic-bezier(.5,0,.7,.4) both, phx-gl-bounce .35s cubic-bezier(.3,1.8,.5,1) .7s; }
@keyframes phx-gl-land { from { transform: translateY(-110cqw) rotate(var(--r)); } to { transform: translateY(0) rotate(0); } }
@keyframes phx-gl-bounce { 0% { transform: scaleY(.7) translateY(.6cqw); } 100% { transform: none; } }
.phx-gl.out .cap, .phx-gl.out .shelf { opacity: 0; }`,
    `<canvas></canvas><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="shelf"></div>`
  );
  const cv = el.querySelector("canvas"), ctx = cv.getContext("2d"), dpr = devicePixelRatio || 1;
  const W2 = cv.width = box.clientWidth * dpr, H2 = cv.height = box.clientHeight * dpr;
  const ink2 = dark ? "#c9d4ea" : INK.light;
  const make = (y = Math.random() * H2) => {
    const z = [0.45, 0.7, 1][Math.floor(Math.random() * 3)];
    return { x: Math.random() * W2, y, z, v: (0.04 + Math.random() * 0.03) * z * H2, ch: GLYPHS[Math.floor(Math.random() * GLYPHS.length)], rot: Math.random() * 6, spin: (Math.random() - 0.5) * 1.5, c: Math.random() < 0.12 ? RIBBON[Math.floor(Math.random() * 4)] : ink2 };
  };
  const gs = Array.from({ length: 60 }, () => make());
  let out = -1;
  let clear = [], clearAt = -1e9;
  const places = () => {
    const b = box.getBoundingClientRect(), m = 2.4 * (b.width / 100);
    clear = [...el.querySelectorAll(".cap > div, .shelf")].map((e) => e.getBoundingClientRect()).filter((r) => r.height > 0 && r.width > 0).map((r) => {
      return new DOMRect((r.left - b.left - m) * dpr, (r.top - b.top - m) * dpr, (r.width + 2 * m) * dpr, (r.height + 2 * m) * dpr);
    });
  };
  const fade = (x, y) => {
    let a = 1;
    for (const r of clear) {
      const dx = Math.max(r.left - x, 0, x - r.right), dy = Math.max(r.top - y, 0, y - r.bottom), d = Math.hypot(dx, dy);
      a = Math.min(a, d / (14 * dpr));
    }
    return Math.max(0, Math.min(1, a));
  };
  const stopLoop = loop((t, dt) => {
    ctx.clearRect(0, 0, W2, H2);
    if (t - clearAt > 250) places(), clearAt = t;
    const s = dt / 1e3;
    for (const g of gs) {
      let alpha = 0.25 + 0.6 * g.z, rot = g.rot;
      if (out < 0) {
        g.y += g.v * s;
        g.rot += g.spin * s;
        if (g.y > H2 + 30 * dpr) Object.assign(g, make(-30 * dpr));
      } else {
        const k = Math.min(1, (t - out) / 380), e = 1 - (1 - k) ** 3;
        g.x += ((g.tx ?? g.x) - g.x) * e * 0.35;
        g.y += ((g.ty ?? g.y) - g.y) * e * 0.35;
        rot = g.rot * (1 - e);
        alpha *= 1 - Math.max(0, (t - out - 200) / 300);
      }
      ctx.save();
      ctx.globalAlpha = Math.max(0, alpha * (out < 0 ? fade(g.x, g.y) : 1));
      ctx.translate(g.x, g.y);
      ctx.rotate(rot);
      const px = (out < 0 ? 40 * g.z : 40 * g.z * (1 - 0.55 * Math.min(1, (t - out) / 380))) * (W2 / 520);
      ctx.font = `${px}px "CMU Serif", Georgia, serif`;
      ctx.fillStyle = g.c;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(g.ch, 0, 0);
      ctx.restore();
    }
  });
  const t0 = performance.now();
  const $ = (s) => el.querySelector(s);
  const seen = /* @__PURE__ */ new Set();
  const step = (what, pct, detail) => {
    $(".what").textContent = what;
    $(".det").textContent = pct !== void 0 ? detail.split(" \xB7 ").slice(1).join(" \xB7 ") : detail;
    const pkg = pct !== void 0 ? detail.split(" \xB7 ")[0] : "";
    if (pkg && !seen.has(pkg)) {
      seen.add(pkg);
      const w = document.createElement("span");
      w.textContent = pkg;
      w.style.background = [INK.light, ...RIBBON][seen.size % 5];
      w.style.setProperty("--r", `${(Math.random() - 0.5) * 40}deg`);
      $(".shelf").append(w);
    }
  };
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(el, () => {
    out = performance.now() - t0;
    const lh = W2 * 0.0162 * 1.25 * 1.6;
    gs.forEach((g) => (g.tx = W2 * (0.115 + 0.77 * Math.random()), g.ty = H2 * 0.2 + Math.round(Math.random() * H2 * 0.65 / lh) * lh));
    el.classList.add("out");
    return 620;
  }, stopLoop, { step, hint });
}

// viewer/src/openings/kaust.ts
var STEPS2 = ["Reading the project", "Fetching packages", "Typesetting"];
function kaust(box, dark) {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ka",
    `.phx-ka { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; color: ${ink0}; }
.phx-ka .row { position: absolute; left: 50%; top: 36%; transform: translate(-50%, -50%); display: flex; align-items: center; gap: 4cqw; transition: opacity .2s; }
.phx-ka .lk { height: 9cqw; width: auto; animation: phx-ka-in .9s cubic-bezier(.2,.8,.2,1) .35s both; }
.phx-ka .rule { width: .5cqw; height: 9cqw; background: #8d96a5; animation: phx-ka-rule .3s ease-out .9s both; }
.phx-ka .sd { height: 10cqw; width: auto; animation: phx-ka-pop .55s cubic-bezier(.3,1.8,.5,1) 1.05s both; }
@keyframes phx-ka-in { from { opacity: 0; transform: translateX(-6cqw); } }
@keyframes phx-ka-rule { from { transform: scaleY(0); } }
@keyframes phx-ka-pop { from { opacity: 0; transform: translateY(3cqw) scale(.2); } }
.phx-ka .st { position: absolute; left: -40%; width: 180%; height: 2.2cqw; transform: rotate(-50.5deg); animation: phx-ka-shoot 1s cubic-bezier(.2,.9,.3,1.05) both, phx-ka-breathe 2.4s ease-in-out 1.2s infinite; opacity: .14; }
.phx-ka .st:nth-of-type(1) { top: 26%; background: ${RIBBON[0]}; animation-delay: 0s, 1.2s; }
.phx-ka .st:nth-of-type(2) { top: 30%; background: ${RIBBON[1]}; animation-delay: .07s, 1.4s; }
.phx-ka .st:nth-of-type(3) { top: 34%; background: ${RIBBON[2]}; animation-delay: .14s, 1.6s; }
.phx-ka .st:nth-of-type(4) { top: 38%; background: ${RIBBON[3]}; animation-delay: .21s, 1.8s; }
@keyframes phx-ka-shoot { from { transform: rotate(-50.5deg) translateX(-120%); } }
@keyframes phx-ka-breathe { 50% { opacity: .26; } }
.phx-ka .prog { position: absolute; left: 50%; top: 52%; transform: translateX(-50%); display: flex; gap: 1.6cqw; transition: opacity .2s; }
.phx-ka .prog i { position: relative; width: 11cqw; height: 1.8cqw; transform: skewX(-38deg); background: ${dark ? "#3d4047" : "#e7e9ee"}; overflow: hidden; }
.phx-ka .prog i b { position: absolute; inset: 0 auto 0 0; width: 0; background: var(--c); transition: width .35s cubic-bezier(.3,.7,.3,1); }
.phx-ka .prog i.run b { width: 100%; background: linear-gradient(90deg, var(--c) 40%, color-mix(in srgb, var(--c), #fff 55%) 50%, var(--c) 60%) 0 0 / 300% 100%; animation: phx-ka-run 1.2s linear infinite; }
@keyframes phx-ka-run { from { background-position: 100% 0; } to { background-position: -50% 0; } }
.phx-ka .cap { position: absolute; left: 10%; right: 10%; top: 58%; text-align: center; transition: opacity .2s; }
.phx-ka .cap .what { font-size: 3.2cqw; font-weight: 600; }
.phx-ka .cap .det { font-size: 2.4cqw; color: ${mute}; margin-top: .6cqw; min-height: 1.4em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-ka .cap .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 4cqw; line-height: 1.45; }
.phx-ka.out .row, .phx-ka.out .prog, .phx-ka.out .cap { opacity: 0; }
.phx-ka.sweep .st { animation: none; opacity: 1; height: 16cqw; transition: transform .62s cubic-bezier(.55,0,.35,1); }
.phx-ka.sweep .st:nth-of-type(2) { transition-delay: .05s; } .phx-ka.sweep .st:nth-of-type(3) { transition-delay: .1s; } .phx-ka.sweep .st:nth-of-type(4) { transition-delay: .15s; }
.phx-ka.sweep.go .st { transform: rotate(-50.5deg) translateY(150cqw); }`,
    `<div class="st"></div><div class="st"></div><div class="st"></div><div class="st"></div>
     <div class="row">${LOCKUP.replace("<svg ", `<svg class="lk" aria-hidden="true" `).replace(/#1D3E72/gi, dark ? "#e4e6ea" : "#1D3E72")}<div class="rule"></div>${SEEDS.replace("<svg ", `<svg class="sd" aria-hidden="true" `)}</div>
     <div class="prog">${RIBBON.map((c) => `<i style="--c:${c}"><b></b></i>`).join("")}</div>
     <div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div>`
  );
  const $ = (s) => el.querySelector(s);
  const bars = [...el.querySelectorAll(".prog i")];
  const fill = (i, w) => {
    bars[i].classList.toggle("run", w === "run");
    bars[i].querySelector("b").style.width = w === "run" ? "" : `${w}%`;
  };
  const step = (what, pct, detail) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
    const k = Math.max(0, STEPS2.indexOf(what));
    for (let i = 0; i < k; i++) fill(i, 100);
    fill(k, pct !== void 0 ? pct : "run");
  };
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(
    el,
    () => {
      bars.forEach((_, i) => fill(i, 100));
      setTimeout(() => {
        el.classList.add("out", "sweep");
        el.querySelectorAll(".st").forEach((s) => (s.style.transition = "none", s.style.transform = "rotate(-50.5deg) translateY(-70cqw)"));
        void el.offsetWidth;
        el.querySelectorAll(".st").forEach((s) => (s.style.transition = "", s.style.transform = ""));
        el.classList.add("go");
        setTimeout(() => wipe(el, 129.5, 560), 180);
      }, 200);
      return 1e3;
    },
    void 0,
    { step, hint }
  );
}

// viewer/src/openings/badapple-data.ts
var FRAMES = 900;
var W = 128;
var H = 96;
var FPS = 15;
var DATA = "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";

// viewer/src/openings/badapple.ts
var frames = null;
var read = () => frames ??= (async () => {
  const z = Uint8Array.from(atob(DATA), (c) => c.charCodeAt(0));
  const b = new Uint8Array(await new Response(new Blob([z]).stream().pipeThrough(new DecompressionStream("deflate-raw"))).arrayBuffer());
  let i = 0;
  const varint = () => {
    let n = 0, s = 0, x;
    do
      x = b[i++], n |= (x & 127) << s, s += 7;
    while (x & 128);
    return n;
  };
  const zz = () => {
    const n = varint();
    return n & 1 ? -(n + 1 >> 1) : n >> 1;
  };
  const out = [];
  for (let f = 0; f < FRAMES; f++) {
    const p = new Path2D();
    for (let c = varint(); c > 0; c--) {
      const n = varint();
      let x = varint(), y = varint();
      p.moveTo(x, y);
      for (let s = 0; s < n; s++) {
        const x1 = x += zz(), y1 = y += zz(), x2 = x += zz(), y2 = y += zz(), x3 = x += zz(), y3 = y += zz();
        p.bezierCurveTo(x1, y1, x2, y2, x3, y3);
      }
      p.closePath();
    }
    out.push(p);
  }
  return out;
})();
function badapple(box, dark) {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ba",
    `.phx-ba { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; transition: background .3s ease .25s; }
.phx-ba .scr { position: absolute; left: 8cqw; right: 8cqw; top: 16cqw; aspect-ratio: 4 / 3; background: #fff; box-shadow: 0 0 0 .3cqw #000, 0 1.4cqw 3cqw rgb(0 0 0 / 22%); overflow: hidden; transition: all .38s cubic-bezier(.6,0,.3,1); }
.phx-ba canvas { width: 100%; height: 100%; display: block; }
.phx-ba .sub { position: absolute; left: 8%; right: 8%; top: 86cqw; text-align: center; color: ${ink0}; transition: opacity .2s; }
.phx-ba .sub .what { font-size: 3.6cqw; letter-spacing: .06em; }
.phx-ba .sub .det { font-size: 2.4cqw; color: ${mute}; margin-top: .8cqw; min-height: 1.4em; }
.phx-ba .sub .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 3.6cqw; line-height: 1.45; font-style: italic; }
.phx-ba .cr { position: absolute; left: 0; right: 0; bottom: 3cqw; text-align: center; font-size: 1.7cqw; color: ${mute}; opacity: .8; transition: opacity .2s; }
.phx-ba.out { background: transparent; }
.phx-ba.out .sub, .phx-ba.out .cr { opacity: 0; }
.phx-ba.out .scr { left: 0; right: 0; top: 0; aspect-ratio: auto; height: 100%; box-shadow: none; background: transparent; }`,
    `<div class="scr"><canvas></canvas></div><div class="sub"><div class="what"></div><div class="det"></div><div class="hint"></div></div>
     <div class="cr">Bad Apple!! \xB7 Alstroemeria Records feat. nomico \xB7 PV: Anira \xB7 original: ZUN</div>`
  );
  const $ = (s) => el.querySelector(s);
  const cv = el.querySelector("canvas"), ctx = cv.getContext("2d"), dpr = devicePixelRatio || 1;
  let paths = null;
  void read().then((p) => paths = p);
  const t0 = performance.now();
  let out = -1;
  const MELT = 560;
  const stop = loop(() => {
    const r = cv.getBoundingClientRect();
    if (cv.width !== Math.round(r.width * dpr)) cv.width = Math.round(r.width * dpr), cv.height = Math.round(r.height * dpr);
    if (!paths) return;
    const f = Math.floor((performance.now() - t0) / 1e3 * FPS) % FRAMES;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, cv.width, cv.height);
    ctx.setTransform(cv.width / W, 0, 0, cv.height / H, 0, 0);
    if (out < 0) {
      ctx.fillStyle = "#fff";
      ctx.fill(paths[f], "evenodd");
      return;
    }
    const k = Math.max(0, (performance.now() - out - 160) / MELT), w = Math.min(1, k) ** 1.6 * 70;
    ctx.globalCompositeOperation = "destination-out";
    ctx.fill(paths[f], "evenodd");
    if (w > 0) {
      ctx.lineWidth = w;
      ctx.lineJoin = "round";
      ctx.stroke(paths[f]);
      ctx.strokeRect(0, 0, W, H);
    }
  });
  const step = (what, _pct, detail) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
  };
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(
    el,
    () => {
      out = performance.now();
      el.classList.add("out");
      return 160 + MELT + 60;
    },
    stop,
    { step, hint }
  );
}

// viewer/src/openings/index.ts
var LOOKS = [
  { id: "phitex", name: "PhiTeX", make: (s, dark) => opening(s, dark ? "#e4e6ea" : "#1d3e72", dark ? "#2c2e33" : "#ffffff") },
  { id: "terminal", name: "Terminal", make: (s, _d, doc) => terminal(s, doc) },
  { id: "win98", name: "Windows 98", make: (s, _d, doc) => win98(s, doc) },
  { id: "mac9", name: "Mac OS 9", make: (s, _d, doc) => mac9(s, doc) },
  { id: "dos", name: "MS-DOS", make: (s) => dos(s) },
  { id: "typewriter", name: "Typewriter", make: (s, dark, doc) => typewriter(s, dark, doc) },
  { id: "plane", name: "Paper plane", make: (s, dark) => plane(s, dark) },
  { id: "ink", name: "Ink drop", make: (s, dark) => ink(s, dark) },
  { id: "tetris", name: "Tetris", make: (s, dark) => tetris(s, dark) },
  { id: "glyphs", name: "Glyph rain", make: (s, dark) => glyphs(s, dark) },
  { id: "kaust", name: "ACCL | KAUST", make: (s, dark) => kaust(s, dark) },
  { id: "badapple", name: "Bad Apple!!", make: (s, dark) => badapple(s, dark) }
];
function lookOf(v) {
  if (v === "retro") return "win98";
  return v === "random" || v === "off" || LOOKS.some((l) => l.id === v) ? v : "random";
}
function play(look2, first, last, sheet, dark, doc) {
  if (look2 === "off") return { id: "off", anim: still() };
  let l = LOOKS.find((x) => x.id === look2);
  if (!l) {
    const pool = LOOKS.filter((x) => x.id !== last);
    l = first ? LOOKS[0] : pool[Math.floor(Math.random() * pool.length)];
  }
  return { id: l.id, anim: l.make(sheet, dark, doc) };
}

// viewer/src/hints.ts
var HINTS = [
  { text: "Double-click a word on the page to go to its source." },
  { text: "Builds run by themselves as you type: Rebuild is for a change made outside the editor." },
  { text: "Press ` to see everything phitex watch printed in the terminal.", hosts: ["watch"] },
  { text: "Press ? over the pages for the keyboard shortcuts.", hosts: ["watch"] },
  { text: "Compare shows what changed since an earlier version, marked as latexdiff marks it.", hosts: ["watch", "vscode"] },
  { text: "Put \\label after \\caption: \\ref then gives the figure's number, not the section's." },
  { text: "A ~ is a space the line never breaks at: Figure~\\ref{fig:plot}." },
  { text: "You can pick your favourite loading animation, or turn them off, in Settings \u203A Appearance." },
  { text: "Donald Knuth began TeX in 1977; its version number gets one more digit of \u03C0 at each release (3.141592653)." }
];
function hintsFor(host2) {
  return HINTS.filter((h) => !h.hosts || h.hosts.includes(host2)).map((h) => h.text);
}

// viewer/src/panel.ts
var STALE_GRACE_MS = 1e3;
var MAKING = "Making the format, once per PhiTeX update";
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
var esc4 = (t) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
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
  copy: "M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z",
  close: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
  log: "M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
  toc: "M3 9h14V7H3v2zm0 4h14v-2H3v2zm0 4h14v-2H3v2zm16 0h2v-2h-2v2zm0-10v2h2V7h-2zm0 6h2v-2h-2v2z",
  dark: "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z",
  keyboard: "M20 5H4c-1.1 0-1.99.9-1.99 2L2 17c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm-9 3h2v2h-2V8zm0 3h2v2h-2v-2zM8 8h2v2H8V8zm0 3h2v2H8v-2zm-1 2H5v-2h2v2zm0-3H5V8h2v2zm9 7H8v-2h8v2zm0-4h-2v-2h2v2zm0-3h-2V8h2v2zm3 3h-2v-2h2v2zm0-3h-2V8h2v2z",
  bolt: "M11 21h-1l1-7H7.5c-.58 0-.57-.32-.38-.66.19-.34.05-.08.07-.12C8.48 10.94 10.42 7.54 13 3h1l-1 7h3.5c.49 0 .56.33.47.51l-.07.15C12.96 17.55 11 21 11 21z",
  tips: "M9 21c0 .55.45 1 1 1h4c.55 0 1-.45 1-1v-1H9v1zm3-19C8.14 2 5 5.14 5 9c0 2.38 1.19 4.47 3 5.74V17c0 .55.45 1 1 1h6c.55 0 1-.45 1-1v-2.26c1.81-1.27 3-3.36 3-5.74 0-3.86-3.14-7-7-7z"
};
var copyBtn = (i) => `<button type="button" class="copyai" data-i="${i}" title="Copy for an AI assistant: this problem with its source around it, the preamble, the other problems and the log, to paste into the assistant you use (nothing is sent anywhere)" aria-label="Copy for an AI assistant">${icon("copy", 16)}</button>`;
var icon = (name, size = 18) => `<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`;
var ZOOMS = [["fit", "Page width"], ["page", "Whole page"], ["0.5", "50%"], ["0.75", "75%"], ["1", "100%"], ["1.25", "125%"], ["1.5", "150%"], ["2", "200%"], ["3", "300%"]];
var mb = (b) => b ? ` \xB7 ${b < 1048576 ? `${Math.max(1, Math.round(b / 1024))} KB` : `${(b / 1048576).toFixed(1)} MB`}` : "";
var dur = (ms) => ms < 10 ? `${ms.toFixed(1)} ms` : ms < 1e3 ? `${Math.round(ms)} ms` : `${(ms / 1e3).toFixed(ms < 1e4 ? 1 : 0)} s`;
function framePart(json) {
  if (!json) return "";
  const f = JSON.parse(json);
  const split = [["finding what changed", f.probe], ["drawing", f.draw], ["waiting for the engine", Math.max(0, f.request - f.watch)], ["decoding", f.decode]];
  return `<div class="row"><span class="k">Drawing page ${f.page}${f.inView ? " (in view)" : ""}</span><span class="v">${esc4(dur(f.request + f.decode))}</span><div class="tbar">${split.map(([, ms], i) => `<i class="c${i}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`).join("")}</div><div class="legend">${split.map(([n, ms], i) => `<span><i class="c${i}"></i>${n} ${esc4(dur(ms))}</span>`).join("")}</div><span class="note">${f.drawn} slice${f.drawn === 1 ? "" : "s"} of the page drawn, ${f.kept} kept from before</span></div>`;
}
function frameText(json) {
  const f = JSON.parse(json);
  return `page ${f.page}${f.inView ? " (in view)" : ""} in ${dur(f.request + f.decode)}: finding what changed ${dur(f.probe)}, drawing ${dur(f.draw)}, waiting for the engine ${dur(Math.max(0, f.request - f.watch))}, decoding ${dur(f.decode)}; ${f.drawn} slices drawn, ${f.kept} kept`;
}
var PART_NAMES = {
  "first paint": "Typesetting this page",
  "fixed point": "Typesetting every page",
  "keystroke\u2192page": "Keystroke to page",
  rebuilt: "Parts redone",
  batching: "Edits batched"
};
var PART_NOTES = {
  "first paint": "in the engine",
  "fixed point": "the whole document, references and contents included",
  rebuilt: "",
  batching: ""
};
var SPLIT_NAMES = {
  queued: "waiting",
  "round trip": "engine",
  paint: "drawing",
  "build and paint": "building and drawing"
};
function part2(key, raw) {
  const label = PART_NAMES[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
  const paren = /\s*\(([^)]*)\)\s*$/.exec(raw);
  let rest = paren ? raw.slice(0, paren.index) : raw;
  const note = paren?.[1];
  const num = /^([\d.]+)\s*(ms|s)\b\s*/.exec(rest);
  if (!num) return { key, label, value: rest.trim(), note };
  const ms = Number(num[1]) * (num[2] === "s" ? 1e3 : 1);
  rest = rest.slice(num[0].length).trim();
  let split;
  if (rest.startsWith("=")) {
    split = [...rest.slice(1).matchAll(/([a-z][a-z ]*?)\s+([\d.]+)/gi)].map((m) => [SPLIT_NAMES[m[1].trim()] ?? m[1].trim(), Number(m[2])]);
    rest = "";
  }
  const extra = rest.replace(/^·\s*/, "");
  return { key, label, value: dur(ms), ms, split, note: [key in PART_NOTES ? PART_NOTES[key] : note, extra].filter(Boolean).join(" \xB7 ") || void 0 };
}
var lines = (...w) => w.map((x) => `<i style="width:${x}%"></i>`).join("");
var TIPS = [
  {
    id: "live",
    at: "#st",
    title: "Typeset as you type",
    text: "No compile button: change the source and only what you touched is typeset again. The page follows in milliseconds.",
    // (a heading typed in the editor, the page's heading following it letter by letter, the time it took)
    demo: `<div class="demo live" aria-hidden="true"><div class="ed"><span class="k">\\section{</span><span class="ty">Results</span><span class="car"></span><span class="k">}</span><i style="width:80%"></i><i style="width:60%"></i></div><div class="pg"><b>1&nbsp;&nbsp;<span class="ty">Results</span></b>${lines(95, 90, 70)}</div><span class="ms">${icon("bolt", 11)}12 ms</span></div>`
  },
  {
    id: "compare",
    at: "#b-compare",
    title: "Compare versions",
    fresh: true,
    text: "See what changed since an earlier version, marked as latexdiff marks it: additions in blue, deletions struck out.",
    // (a sentence changes, then the change marked, then the navigator; Overleaf's compare tip had it first)
    demo: `<div class="demo cmp" aria-hidden="true">The results are <span class="del">good</span><span class="add">&nbsp;excellent</span>.<span class="ver"><i class="v0">Submitted v1</i><i class="v1">Now vs Submitted v1</i></span><span class="tag">\u2039 1 / 1 \u203A</span></div>`
  },
  {
    id: "timing",
    at: "#st",
    title: "How long it took",
    text: "Click the status for the last build's times and how long a keystroke takes to reach the page.",
    demo: `<div class="demo tim" aria-hidden="true"><div class="mbar"><span class="mst">Built \xB7 42 ms</span></div><div class="mpop"><b>Keystroke to page</b><p><span>edit</span><i style="--w:30%"></i><em>11 ms</em></p><p><span>typeset</span><i style="--w:55%"></i><em>25 ms</em></p><p><span>paint</span><i style="--w:18%"></i><em>6 ms</em></p></div></div>`
  },
  {
    id: "rebuild",
    at: "#b-rec",
    title: "Clean rebuild",
    text: "Builds run by themselves as you type. Start over from a clean slate here, or clear what PhiTeX keeps.",
    demo: `<div class="demo reb" aria-hidden="true"><div class="mbar"><span class="msplit"><span class="go">${icon("refresh", 12)}Rebuild</span><span class="dd">${icon("expand_more", 12)}</span></span></div><div class="mmenu"><span>Rebuild</span><span class="pick">Clean rebuild</span></div><div class="pg">${lines(60, 95, 90, 70, 92, 50)}</div></div>`
  },
  {
    id: "more",
    at: "#b-more",
    title: "Dark PDF and more",
    text: "The pages in dark mode, the log, the contents and the keys are in this menu.",
    demo: `<div class="demo drk" aria-hidden="true"><div class="mbar"><span class="dots">${icon("more_horiz", 14)}</span></div><div class="mmenu"><span class="pick">${icon("dark", 11)}Dark PDF<b>\u2713</b></span><span>View log</span></div><div class="pg">${lines(55, 95, 88, 92, 60, 85)}</div></div>`
  },
  {
    id: "contents",
    at: "#b-side",
    title: "Contents",
    fresh: true,
    text: "The document's sections beside the pages: click one to go there. This button (or t) shows or hides them.",
    demo: `<div class="demo toc" aria-hidden="true"><div class="col"><span>1 Introduction</span><span>2 Method</span><span>3 Results</span><i class="at"></i></div><div class="pg"><div class="roll">${lines(45, 95, 90, 70)}<s></s>${lines(40, 92, 85, 60)}<s></s>${lines(42, 90, 94, 75)}</div></div></div>`
  }
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
    <button type="button" class="ib" id="b-side" title="Contents (t)" aria-label="Contents" aria-pressed="false"><span class="icon">toc</span></button>
    <span class="split" id="split"><button type="button" class="go" id="go" title="Build again with the files as they are (builds run by themselves as you type)"><span class="icon">refresh</span><span class="wide">Rebuild</span></button><button type="button" class="more" id="b-rec" aria-haspopup="menu" aria-expanded="false" title="More ways to build" aria-label="More ways to build"><span class="icon">expand_more</span></button></span>
    <button type="button" class="status" id="st" aria-haspopup="dialog" aria-expanded="false" title="The last build's times"><b id="sttext">Starting\u2026</b><span id="stmore"></span></button>
    <span class="grow"></span>
    <span class="find" id="find" hidden><input id="findq" type="search" placeholder="Find" spellcheck="false" aria-label="Find in the pages"><span id="findn"></span></span>
    <button type="button" class="ib wide" id="b-find" title="Find in the pages (Ctrl+F)" aria-label="Find"><span class="icon">search</span></button>
    <button type="button" class="ib wide" id="b-compare" title="Compare with an earlier version (latexdiff)" aria-label="Compare"><span class="icon">compare</span></button>
    <button type="button" class="ib wide" id="pdf" title="Download the PDF" aria-label="Download the PDF"><span class="icon">download</span></button>
    <span class="pageno"><input id="pgin" value="\u2013" aria-label="Page" inputmode="numeric" autocomplete="off"><span>/&nbsp;<span id="pgn">0</span></span></span>
    <button type="button" class="ib zoomb wide" id="b-zoom" aria-haspopup="menu" aria-expanded="false" title="Zoom"><span id="zl">Page width</span><span id="zs">Fit</span><span class="icon">expand_more</span></button>
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
      ${item2("x-settings", "tips", "Settings\u2026")}
      ${item2("x-tips", "tips", "What's new")}
      ${item2("x-keys", "keyboard", "Keyboard shortcuts", "?")}
      ${item2("x-about", "info", "About PhiTeX")}
    </div>
    <div class="tip" id="tip" role="dialog" aria-live="polite" hidden></div>
  </div>
  <div class="diffbar" id="diffbar" role="toolbar" aria-label="Diff"></div>
  <div class="body">
    <nav class="side" id="side" aria-label="Contents"></nav>
    <div class="stage" id="stage"><div class="strip gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">\u2715</button></div><div class="strip approx" id="approx"></div><div class="strip banner" id="banner"></div><div class="empty" id="empty" hidden></div><div class="viewer" id="viewer"></div></div><div class="smarks" id="smarks" aria-hidden="true"></div><div class="ctip" id="ctip" role="tooltip" hidden></div><div id="speedchip" hidden><b id="speedms"></b><span id="stripsline"></span></div>
    <div class="opening" id="opening" hidden><div class="sheet"></div><div class="steps" aria-live="polite"><div class="what"></div><div class="track unknown"><i></i></div><div class="detail"></div><div class="hint" id="hint" hidden></div></div></div>
    <div class="menu diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="menu logs" id="logs" role="dialog" aria-label="Problems" hidden><header><span id="logshead">Problems</span><span class="grow"></span><button type="button" class="ib" id="logsx" title="Close (Esc)" aria-label="Close"><span class="icon">close</span></button></header><div class="diags" id="diags" role="list"></div></div>
    <div class="dialog settings" id="settings" role="dialog" aria-label="Settings" hidden></div>
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
    offer("#x-contents, #b-side", !!opts.outline);
    offer("#x-keys", !!opts.keys);
    offer("#b-find, #x-find", !!ev.onFind);
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
    this.$("#r-clean").onclick = () => (this.menu(null), this.clean());
    this.$("#r-compare").onclick = this.$("#b-compare").onclick = this.$("#x-compare").onclick = () => (this.menu(null), ev.onCompare?.());
    this.$("#r-cache").onclick = () => (this.menu(null), ev.onClearCache?.());
    this.$("#pdf").onclick = this.$("#x-pdf").onclick = () => (this.menu(null), ev.onPdf());
    this.$("#x-log").onclick = () => (this.menu(null), void this.showLog());
    this.$("#x-contents").onclick = this.$("#b-side").onclick = () => (this.menu(null), this.side());
    this.$("#x-find").onclick = this.$("#b-find").onclick = () => (this.menu(null), this.find(true));
    this.$("#x-tips").onclick = () => (this.menu(null), ev.onNews ? ev.onNews() : this.tips("fresh"));
    this.$("#x-settings").onclick = () => (this.menu(null), void this.settings(true));
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
    this.$("#stage").addEventListener("pointermove", (e) => this.culpritAt(e), { passive: true });
    this.$("#stage").addEventListener("pointerleave", () => this.$("#ctip").hidden = true);
    const license = this.$("#license");
    if (license) license.href = opts.fileUrl?.(w.license) ?? w.license;
    this.$("#settings").onclick = (e) => void this.settingClicked(e);
    this.$("#settings").onchange = (e) => void this.settingClicked(e);
    this.$("#min").onclick = () => this.collapse(!this.prefs.collapsed);
    this.$("#diags").onclick = (e) => {
      const copy = e.target.closest(".copyai");
      if (copy) return void this.copyForAi(this.listed[Number(copy.dataset.i)]);
      const at = e.target.closest(".phx-place[data-file]");
      if (at) {
        e.preventDefault();
        return ev.onGoto(at.dataset.file, Number(at.dataset.line) || 1);
      }
      const d = e.target.closest(".diag[data-line]");
      if (d) return ev.onGoto(d.dataset.file, Number(d.dataset.line));
      const card = e.target.closest(".phx-card[data-key]");
      const place4 = card && this.culpritPlaces.get(card.dataset.key);
      if (place4) this.viewer.mark(place4.k, [place4.box]);
      else if (card) this.msg("Not on the pages (no text of it was typeset)", false, true);
    };
    this.$("#stage").addEventListener("pointerdown", () => {
      this.menu(null);
      this.diagnostics(false);
      this.settings(false);
      this.$("#about").hidden = true;
    });
    this.root.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const open = this.root.querySelector(".dialog:not([hidden])");
      if (open) open.hidden = true;
      else if (this.menus.some(([, m]) => !this.$(m).hidden)) this.menu(null);
      else if (!this.escapeProblems()) this.unmark();
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
    this.darkPdf("off");
    window.addEventListener("resize", () => this.place());
    const stage = this.$("#stage");
    new ResizeObserver(() => (this.stageSize = { w: stage.clientWidth, h: stage.clientHeight }, this.drawScrollMarks())).observe(stage);
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
    this.again();
    (this.ev.onRebuild ?? this.ev.onReload).call(this.ev);
  }
  /**
   * A build asked for from the start (Rebuild, Clean rebuild): the opening
   * plays again over the pages, its steps on the sheet, until the new
   * build's first page (no "last complete render" meanwhile).
   */
  again() {
    if (this.ending) this.stopSheet();
    this.reopen = true;
    this.typesetting = false;
    this.loadPct = 0;
    this.loadDone = -1;
    this.paints = 0;
    this.walk = null;
    this.opened = false;
    this.sheetUp();
    this.win.classList.toggle("reopening", !!this.anim);
  }
  /**
   * Another document opens in this panel (a host's project switched): the
   * opening plays until the new first page, as a cold build's does. Nothing
   * of the last document stays: its pages, its status and its timing are
   * not this one's (never "last complete render" of another document).
   */
  opening() {
    this.last = null;
    this.lastStatus = null;
    this.speed = void 0;
    this.built = void 0;
    this.at = 0;
    clearTimeout(this.staleTimer);
    this.staleSince = 0;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout([]);
    this.again();
    this.renderStatus();
    this.nav();
  }
  /** A rebuild's opening is up (firstPending, whatever pages are here). */
  reopen = false;
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
      if (this.pageTheme === void 0) this.win.classList.toggle("light", document.body.dataset.theme === "light");
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
  /** A build of the core's (`on`) or its end: past 400 ms the status says so. */
  preparing(on) {
    if (!this.words.preparing) return;
    this.prepSince = on ? this.prepSince || performance.now() : 0;
    this.activity();
  }
  /** A streamed open's pages so far (before the first: the sheet's step; after: the status's page). */
  streamed;
  /** The engine makes the format (not cached: once per PhiTeX update), before the document. */
  making = false;
  streaming(s) {
    this.streamed = s ? s.pages : void 0;
    this.making = s?.phase === "format";
    if (s && this.firstPending()) this.typesetting = true, this.step(this.words.steps[2], void 0, this.making ? MAKING : `Page ${s.pages + 1}`);
    this.activity();
  }
  busy(on) {
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
      const what = this.making ? `${MAKING}\u2026` : this.streamed ? `Typesetting page ${this.streamed + 1}\u2026` : "Typesetting\u2026";
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
    else if (this.firstPending() && this.stepNow) t = this.stepNow;
    else if (this.pkgNow) t = "Fetching packages\u2026", m = `${this.pkgNow.done} of ${this.pkgNow.total}`;
    else if (running) {
      const s = (now - (this.buildSince || this.prepSince)) / 1e3;
      t = this.making ? `${MAKING}\u2026` : this.streamed ? `Typesetting page ${this.streamed + 1}\u2026` : "Typesetting\u2026";
      m = s >= 1 ? `${s < 10 ? s.toFixed(1) : Math.round(s)} s` : "";
    } else if (this.warm) t = "Preparing instant typing\u2026", m = `${Math.round((Date.now() - this.warm.since) / 1e3)} s`;
    else if (!this.last && this.viewer.pages === 0 && !this.$("#empty").hidden) t = errs ? `Stopped with ${errs} error${errs === 1 ? "" : "s"}` : "No page yet", err = !!errs;
    else if (this.reopen || !this.last && this.viewer.pages === 0) t = this.stepNow || "Opening\u2026";
    else if (errs) t = `Built with ${errs} error${errs === 1 ? "" : "s"}`, m = this.latest() ? dur(this.latest().ms) : "", err = true;
    else t = this.latest()?.label ?? "Built", m = this.latest() ? dur(this.latest().ms) : "";
    text.textContent = t;
    more.textContent = m ? `\xB7 ${m}` : "";
    const was = this.statusNow;
    if (was.text !== t || was.more !== m || was.err !== err || was.busy !== running) {
      this.statusNow = { text: t, more: m, err, busy: running };
      queueMicrotask(() => this.emit());
    }
    st.classList.toggle("err", err);
    this.$("#prog").classList.toggle("on", running || !this.last && this.viewer.pages === 0 && !this.offline && this.$("#empty").hidden === true);
    this.$("#split").classList.toggle("busy", running);
  }
  /** The status as last said (ViewState.status: a host's toolbar shows it). */
  statusNow = { text: "", more: "", err: false, busy: false };
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
  /** The problems' culprits underlined on the pages: the latest status's, a later one replacing an earlier still being looked up. */
  culpritsSeq = 0;
  /** Whether they are (the ⋯ menu's Underline problems, remembered on this machine); the last status's problems, and those drawn (by data-i). */
  culpritsOn = true;
  culpritDiags = [];
  culpritsDrawn = [];
  /** What the underlines drawn (or being looked up) are of: the problems and the pages' hashes. A build's every status repeats them. */
  culpritsOf = "";
  /** The pages' hashes as last laid out. */
  pageHashes = [];
  async underline(diags) {
    if (!this.ev.onBoxes) return;
    this.culpritDiags = diags;
    if (!this.culpritsOn) return void (++this.culpritsSeq, this.culpritsOf = "", this.viewer.culprits([]), this.$("#ctip").hidden = true);
    const want = diags.filter((d) => !!d.problem?.file && !!d.problem.range && d.problem.severity !== "note").filter((d) => !this.hiddenKinds.has(kindOf(d))).map((d) => {
      const k = kindOf(d);
      const amount = Number(/\(([\d.]+)pt too (?:wide|high)\)/.exec(d.message)?.[1]) || void 0;
      return { file: d.problem.file, range: d.problem.range, severity: d.problem.severity === "warning" ? "warning" : "error", message: d.message, rule: k === "overfull" || k === "underfull" ? k : void 0, amount };
    }).sort((a, b) => a.severity === b.severity ? 0 : a.severity === "error" ? -1 : 1).slice(0, 20);
    const of = JSON.stringify([want, this.pageHashes, [...this.hiddenKinds]]);
    if (of === this.culpritsOf) return;
    this.culpritsOf = of;
    const seq = ++this.culpritsSeq;
    const found = await Promise.all(want.map((w, i) => this.ev.onBoxes(w.file, w.range[0], w.range[1]).then((hits) => hits.map((h) => ({ ...h, severity: w.severity, i, rule: w.rule, amount: w.amount })), () => [])));
    if (seq !== this.culpritsSeq) return;
    this.culpritsDrawn = want;
    this.viewer.culprits(found.flat());
    this.culpritPlaces = new Map(want.map((w, i) => [`${w.file}\0${w.range[0]}`, found[i][0] ? { k: found[i][0].k, box: found[i][0].boxes[0] } : null]));
    this.problemMarks = want.flatMap((w, i) => found[i][0]?.boxes[0] ? [{ k: found[i][0].k, y: found[i][0].boxes[0][1], cls: w.severity, title: w.message }] : []);
    this.drawScrollMarks();
  }
  /** Each problem's first place on the pages, by its file and byte (a card's "on the page"). */
  culpritPlaces = /* @__PURE__ */ new Map();
  /** The scrollbar's marks: the problems' places, and the diff's changes (diffMarks). */
  problemMarks = [];
  changeMarks = [];
  /** The diff's changes' places (page from 0, height in PDF points), marked along the scrollbar; none: gone. */
  diffMarks(changes) {
    this.changeMarks = changes.map((c) => ({ ...c, cls: "change" }));
    this.drawScrollMarks();
  }
  /** The marks along the pages' scrollbar, each where its place is in the whole scroll; a click goes there. */
  drawScrollMarks() {
    const el = this.$("#smarks");
    const marks = [...this.problemMarks, ...this.changeMarks];
    el.replaceChildren();
    el.hidden = !marks.length;
    if (!marks.length) return;
    const fr = this.viewer.fractions(marks);
    marks.forEach((m, i) => {
      if (fr[i] < 0) return;
      const t = document.createElement("i");
      t.className = m.cls;
      t.title = m.title;
      t.style.top = `${(fr[i] * 100).toFixed(3)}%`;
      t.onclick = () => this.viewer.scrollToPlace(m.k, m.y);
      el.append(t);
    });
  }
  /** The underlines on or off. */
  culpritsShown(on) {
    this.culpritsOn = on;
    void this.underline(this.culpritDiags);
  }
  /** The pages' origins came after the problems did: their underlines looked up again (the memo held an answer with no pages). */
  origins() {
    this.culpritsOf = "";
    void this.underline(this.culpritDiags);
  }
  /** The pointer over an underline: its problem's message, under it. */
  culpritAt(e) {
    const tip = this.$("#ctip");
    if (!this.culpritsDrawn.length || e.buttons) return void (tip.hidden = true);
    const hit = [...this.root.querySelectorAll(".slot .culprit")].find((m) => {
      const r2 = m.getBoundingClientRect();
      return e.clientX >= r2.left - 2 && e.clientX <= r2.right + 2 && e.clientY >= r2.top - 2 && e.clientY <= r2.bottom + 5;
    });
    const p = hit && this.culpritsDrawn[Number(hit.dataset.i)];
    if (!hit || !p) return void (tip.hidden = true);
    if (tip.hidden || tip.dataset.i !== hit.dataset.i) {
      tip.dataset.i = hit.dataset.i;
      tip.className = `ctip ${p.severity}`;
      tip.innerHTML = `${icon(p.severity, 14)}<span></span>`;
      tip.querySelector("span").textContent = p.message;
      tip.hidden = false;
    }
    const body = this.$(".body").getBoundingClientRect(), r = hit.getBoundingClientRect();
    const left = Math.max(8, Math.min(body.width - tip.offsetWidth - 8, e.clientX - body.left - 12));
    const below = r.bottom - body.top + 8;
    tip.style.left = `${left}px`;
    tip.style.top = `${below + tip.offsetHeight > body.height - 8 ? r.top - body.top - tip.offsetHeight - 6 : below}px`;
  }
  status(s) {
    this.lastStatus = s;
    queueMicrotask(() => this.stopped());
    const diags = s.diagnostics ?? [];
    void this.underline(diags);
    const worst = diags[0]?.severity;
    const partial = !!s.pending || s.pages === 0;
    this.chip = {
      level: worst === "error" ? "error" : partial || worst === "warning" ? "warning" : "ok",
      text: `${s.pages} page${s.pages === 1 ? "" : "s"}${s.pending ? " \xB7 partial" : ""}`,
      title: (s.pending ? `${s.pending} parts not read (out of fuel or unsupported). ` : "") + (diags.length ? `${diags.length} diagnostic${diags.length === 1 ? "" : "s"}` : "No problems found") + (s.file ? ` \xB7 editing ${s.file}` : "")
    };
    const list = this.$("#diags");
    list.innerHTML = "";
    const byKind = /* @__PURE__ */ new Map();
    for (const d of diags) byKind.set(kindOf(d), (byKind.get(kindOf(d)) ?? 0) + 1);
    if (byKind.size > 1 || this.hiddenKinds.size) {
      const f = document.createElement("div");
      f.className = "kinds";
      f.setAttribute("role", "group");
      f.setAttribute("aria-label", "Show");
      for (const [k, name] of KINDS) {
        const n = byKind.get(k);
        if (!n) continue;
        const b = document.createElement("button");
        b.type = "button";
        b.dataset.kind = k;
        b.setAttribute("aria-pressed", String(!this.hiddenKinds.has(k)));
        b.textContent = `${name} ${n}`;
        b.onclick = (e) => (e.stopPropagation(), this.toggleKind(k));
        f.append(b);
      }
      list.append(f);
    }
    const shown = diags.filter((d) => !this.hiddenKinds.has(kindOf(d)));
    this.listed = shown.slice(0, this.listedMax);
    for (const [i, d] of this.listed.entries()) {
      const copy = d.severity === "error" || d.severity === "warning" ? copyBtn(i) : "";
      if (d.problem) {
        const card = document.createElement("ol");
        card.className = "phx-card phx-inline";
        card.setAttribute("role", "listitem");
        card.innerHTML = problemHtml(d.problem);
        card.firstElementChild.insertAdjacentHTML("afterbegin", copy);
        if (d.problem.file && d.problem.range) {
          card.dataset.key = `${d.problem.file}\0${d.problem.range[0]}`;
          card.title = "Show it on the page (its file:line opens it in the editor)";
        }
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
      row.innerHTML = `${copy}<span class="where"></span><div class="text"></div>`;
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
    if (shown.length > this.listed.length) {
      const left = shown.length - this.listed.length;
      const more = document.createElement("div");
      more.className = "diag info more";
      const next = document.createElement("button");
      next.type = "button";
      next.textContent = left > 50 ? `Show 50 more (${left} left)` : `Show the other ${left}`;
      next.onclick = (e) => (e.stopPropagation(), this.listMore(50));
      more.append(next);
      if (left > 50) {
        const all = document.createElement("button");
        all.type = "button";
        all.textContent = "Show all";
        all.onclick = (e) => (e.stopPropagation(), this.listMore(Infinity));
        more.append(all);
      }
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
  /** The problems listed (by the rows' data-i). */
  listed = [];
  /** How many problems the list shows (50, then more as asked: "Show 50 more", "Show all"). */
  listedMax = 50;
  /** `n` more problems listed (Infinity: all), the list drawn again where it is. */
  listMore(n) {
    this.listedMax += n;
    if (this.lastStatus) this.status(this.lastStatus);
  }
  /**
   * Copy for an AI assistant: one problem with what an assistant needs to
   * explain it: its place, message, TeX's notes, help and suggestions, the
   * macros it was in, the files that included its file, its source lines
   * around it, the main file's preamble (the packages and definitions), the
   * other problems listed, and the log where it was met (and its end, for
   * an error). On the clipboard, nothing sent.
   */
  async copyForAi(d) {
    if (!d) return;
    const p = d.problem;
    const file = p?.file ?? d.file, line = p?.line ?? d.line;
    const main = this.lastStatus?.main ?? this.mainFiles.main;
    const source2 = async (f) => (await this.ev.onSource(f).catch(() => null))?.split("\n") ?? null;
    const numbered = (lines3, from2, to, at) => lines3.slice(from2 - 1, to).map((t, i) => `${String(from2 + i).padStart(4)}${from2 + i === at ? ">" : " "} ${t}`);
    const parts = [
      `I'm writing a LaTeX document (typeset by PhiTeX, a pdfLaTeX/XeLaTeX-compatible engine) and the build reports this ${d.severity}. What is wrong, and how do I fix it in my source?`,
      "",
      `## The ${d.severity}`,
      `${d.message}${file ? ` (${file}${line ? `, line ${line}${p?.col ? `, column ${p.col}` : ""}` : ""})` : ""}${p?.code ? ` [${p.code}]` : ""}`
    ];
    if (p?.notes.length) parts.push(...p.notes.map((n) => `- ${n}`));
    if (p?.context.length) parts.push(`TeX was inside: ${p.context.map((m) => `\\${m.name.replace(/^\\/, "")} (${m.before}\u2038${m.after})`).join(" \u2190 ")}`);
    if (p?.included.length) parts.push(`Its file was included from: ${p.included.map((i) => `${i.file}:${i.line}`).join(" \u2190 ")}`);
    if (p?.help.length || p?.suggestions.length) parts.push(`PhiTeX's hint: ${[...p?.help ?? [], ...p?.suggestions ?? []].join(" ")}`);
    const lines2 = file ? await source2(file) : null;
    if (lines2 && line) parts.push("", `## ${file}, lines around ${line}`, "```latex", ...numbered(lines2, Math.max(1, line - 15), Math.min(lines2.length, line + 15), line), "```");
    else if (p?.excerpt) parts.push("", "## The source line", "```latex", p.excerpt.text, "```");
    const mainLines = main ? main === file ? lines2 : await source2(main) : null;
    if (main && mainLines) {
      const end = mainLines.findIndex((l) => /\\begin\s*\{document\}/.test(l));
      const upto = Math.min(end >= 0 ? end + 1 : 60, 120);
      if (!(main === file && line && line <= upto + 15)) parts.push("", `## The preamble of ${main}`, "```latex", ...numbered(mainLines, 1, upto), "```");
    }
    const others = this.listed.filter((x) => x !== d && (x.severity === "error" || x.severity === "warning")).slice(0, 10);
    if (others.length) parts.push("", "## Other problems in the same build", ...others.map((x) => `- ${x.severity}: ${x.message}${x.file ? ` (${x.file}${x.line ? `:${x.line}` : ""})` : ""}`));
    const log2 = (await this.ev.onLog?.().catch(() => null))?.split("\n") ?? null;
    if (log2) {
      const key = d.message.replace(/^.*?:\s*/, "").slice(0, 40);
      const at = key ? log2.findIndex((l) => l.includes(key)) : -1;
      if (at >= 0) parts.push("", "## The log where it was met", "```", ...log2.slice(Math.max(0, at - 10), at + 15), "```");
      if (d.severity === "error" && (at < 0 || at + 15 < log2.length - 40)) parts.push("", "## The end of the log", "```", ...log2.slice(-40), "```");
    }
    const text = parts.join("\n");
    try {
      await navigator.clipboard.writeText(text);
      this.msg("Copied: paste it into your AI assistant", false, true);
    } catch {
      this.msg("Couldn't copy to the clipboard", true, true);
    }
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
    const now = performance.now(), d = this.downloads;
    if (p.loading.length && d.since === void 0) d.since = now;
    else if (!p.loading.length && d.since !== void 0) d.ms += now - d.since, d.since = void 0;
    d.n = done, d.bytes = p.bytes ?? 0;
    const total = done + p.loading.length;
    if (done !== this.loadDone) this.loadDone = done;
    const pct = this.loadPct = Math.max(this.loadPct, total ? Math.round(100 * done / total) : 100);
    if (this.firstPending()) {
      if (!p.loading.length && !p.building) {
        this.pkgBusy = false;
        if (this.stopped()) return;
        if (this.typesetting) this.step(this.words.steps[2], void 0, downloaded(done, p.bytes));
        else if (done) this.step(this.words.steps[1], pct, `${done} of ${total}${mb(p.bytes)}`);
        return;
      }
      this.pkgBusy = true;
      if (p.loading.length) {
        const name = p.loading[0];
        const what = `${name} \xB7 ${done} of ${total}${mb(p.bytes)}${p.unavailable.length ? ` \xB7 ${p.unavailable.length} not found` : ""}`;
        if (this.typesetting) this.step(this.words.steps[2], void 0, `fetching ${what}`);
        else this.step(this.words.steps[1], pct, what);
      } else this.typesetting = true, this.step(this.words.steps[2], void 0, downloaded(done, p.bytes));
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
    this.step(this.words.steps[k] ?? "", void 0, k === 2 && this.streamed !== void 0 && !this.making ? `Page ${this.streamed + 1} \xB7 ${text}` : text);
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
    return this.reopen || !this.last && this.viewer.pages === 0 && this.$("#empty").hidden === true;
  }
  /** The sheet's step: what, how far (undefined: not known), and the detail; the sheet put up first if it isn't. */
  /** This opening's typesetting has begun (the build running, or pages shipped): a later fetch said under it. */
  typesetting = false;
  /** The "Did you know" under the opening: one hint a few seconds, from 2 s into a first build, gone with the sheet. */
  hintTimer;
  hintAt = Math.floor(Math.random() * 1e3);
  hints(on) {
    const el = this.$("#hint");
    if (!on) return void (clearTimeout(this.hintTimer), this.hintTimer = void 0, el.hidden = true);
    if (this.hintTimer) return;
    const list = hintsFor(this.kind);
    const show = () => {
      const text = list[this.hintAt++ % list.length];
      if (this.anim?.hint) this.anim.hint(text);
      else el.textContent = `Did you know? ${text}`, el.hidden = false;
      this.hintTimer = setTimeout(show, 7e3);
    };
    this.hintTimer = setTimeout(show, 2e3);
  }
  step(what, pct, detail = "") {
    if (!this.firstPending()) return;
    this.walkTo(what);
    this.sheetUp();
    this.hints(true);
    const el = this.$("#opening");
    this.stepNow = what;
    el.querySelector(".what").textContent = what;
    el.querySelector(".detail").textContent = detail;
    const track = el.querySelector(".track");
    track.classList.toggle("unknown", pct === void 0);
    track.querySelector("i").style.width = pct === void 0 ? "" : `${pct}%`;
    this.anim?.step?.(what, pct, detail);
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
    const sheet = el.querySelector(".sheet");
    const first = !this.seenIds.has("opening");
    const { id, anim } = play(this.look, first, this.lastLook, sheet, dark, (this.mainFiles.main ?? this.doc ?? "document.tex").split("/").pop());
    this.anim = anim;
    this.lastLook = id;
    if (id !== "off") this.seen(["opening"]);
    el.classList.toggle("owns", !!anim.step);
    if (anim.step) anim.step(this.stepNow || this.words.steps[0], void 0, "");
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
    this.win.classList.remove("reopening");
    const el = this.$("#opening");
    const follow2 = () => {
      if (!this.ending) return;
      const slot = this.$("#viewer").querySelector(".slot");
      if (slot) {
        const r = slot.getBoundingClientRect(), b = this.$(".body").getBoundingClientRect();
        Object.assign(el.style, { left: `${r.left - b.left}px`, top: `${r.top - b.top}px`, width: `${r.width}px`, height: `${r.height}px` });
      }
      requestAnimationFrame(follow2);
    };
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return this.stopSheet();
    requestAnimationFrame(() => {
      follow2();
      el.classList.add("in");
    });
    const a = this.anim;
    const stop = () => this.anim === a && this.stopSheet();
    void a.done().then(stop);
    setTimeout(stop, 1600);
  }
  /** The sheet gone (played out, or a card in its place). */
  stopSheet() {
    this.hints(false);
    const was = !!this.anim;
    this.anim?.stop();
    this.anim = null;
    this.ending = false;
    this.reopen = false;
    this.win.classList.remove("reopening");
    this.$("#opening").hidden = true;
    this.win.classList.remove("sheet-up");
    if (was) {
      this.opened = true;
      this.stepNow = "";
      this.renderStatus();
      setTimeout(() => this.tips("unseen"), 600);
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
    el.className = `toast ${kind}${t.err ? " err" : ""}`;
    el.hidden = false;
    el.innerHTML = `<div class="t">${t.icon ? icon(t.icon, 16) : ""}<span></span></div><div class="d"></div>` + (t.pct === null ? "" : `<div class="track"><i style="width:${t.pct}%"></i></div>`);
    el.querySelector(".t span").textContent = t.title;
    el.querySelector(".d").textContent = t.detail ?? "";
    if (t.ms) this.toastTimer = setTimeout(() => this.toast(kind, null), t.ms);
    el.title = t.err && !t.ms ? "Click to dismiss" : "";
    el.onclick = t.err && !t.ms ? () => this.toast(kind, null) : null;
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
    if (this.reopen) return void (clearTimeout(this.staleTimer), this.staleSince = 0, stage.classList.remove("stale"));
    const gone2 = (s.pages === 0 || this.at >= s.pages) && !!this.last;
    clearTimeout(this.staleTimer);
    if (!gone2) {
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
      settings: !this.$("#settings").hidden,
      // (the badge counts what needs a look: info is listed, not counted)
      diagCount: this.lastStatus?.diagnostics?.filter((d) => d.severity !== "info").length ?? 0,
      diagWorst: this.lastStatus?.diagnostics?.[0]?.severity ?? "",
      percent: Math.round(this.scale() * 100),
      status: { ...this.statusNow },
      timing: { summary: this.lat?.summary ?? "", parts: this.timingParts(), message: this.msgText }
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
  /** The part of page `k` on screen (the viewer's `view`): what a zoomed-in page is drawn sharp around. */
  pageView(k) {
    return this.viewer?.view(k) ?? null;
  }
  pixelsPerPoint() {
    return 96 / 72 * this.scale() * (window.devicePixelRatio || 1);
  }
  /** The stage's client size, kept by its ResizeObserver (null until it is first laid out). */
  stageSize = null;
  scale() {
    const z = this.zoomNow ?? this.prefs.zoom;
    if (z !== "fit" && z !== "page") return Number(z);
    const st = this.stageSize ??= { w: this.$("#stage").clientWidth, h: this.$("#stage").clientHeight };
    const width = Math.max(st.w - 40, 200) / ((this.last?.w ?? 595) * (96 / 72));
    if (z === "fit") return width;
    return Math.min(width, Math.max(st.h - 24, 100) / ((this.last?.h ?? 842) * (96 / 72)));
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
    this.sideOn(on);
    if (on) this.ev.onContents?.();
    try {
      localStorage.setItem("phitex.side", on ? "1" : "0");
    } catch {
    }
    if (this.prefs.zoom === "fit") this.redraw();
  }
  /** The column's class and the bar's button pressed with it. */
  sideOn(on) {
    this.win.classList.toggle("side-on", on);
    this.$("#b-side").setAttribute("aria-pressed", String(on));
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
  /** The tips shown by themselves (PanelOptions.tips). */
  autoTips = true;
  extras(opts) {
    this.autoTips = opts.tips !== false;
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
      this.sideOn(on);
      this.noOutline(true);
    }
    if (!opts.keys) return;
    const help = this.$("#keyhelp");
    const extra = [["e", "the build's errors and warnings"], ...opts.outline ? [["t", "the contents beside the pages"]] : []];
    help.innerHTML = `<header>Keyboard shortcuts<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><table>${[...KEYS, ...extra].map(([k, w]) => `<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`).join("")}</table>`;
    addEventListener(
      "keydown",
      (e) => {
        if (e.ctrlKey || e.altKey || e.metaKey || typing(e)) return;
        if (e.key === "Escape" && (this.escapeProblems() || this.unmark())) {
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
  /** The kinds of problem not shown (the filters): this browser's, and the host's (problemsHidden). */
  hiddenKinds = (() => {
    try {
      return new Set(JSON.parse(localStorage.getItem("phitex.kinds.hidden") ?? "[]"));
    } catch {
      return /* @__PURE__ */ new Set();
    }
  })();
  /** The kinds hidden as the host keeps them (the watch: the user's phitex.toml, on every port and document). */
  problemsHidden(kinds) {
    this.hiddenKinds = new Set(kinds.filter((k) => KINDS.some(([x]) => x === k)));
    if (this.lastStatus) this.status(this.lastStatus);
  }
  /** A kind's filter clicked: shown or hidden, everywhere, and kept. */
  toggleKind(k) {
    if (!this.hiddenKinds.delete(k)) this.hiddenKinds.add(k);
    try {
      localStorage.setItem("phitex.kinds.hidden", JSON.stringify([...this.hiddenKinds]));
    } catch {
    }
    this.ev.onProblemsHidden?.([...this.hiddenKinds]);
    if (this.lastStatus) this.status(this.lastStatus);
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
  /** The pages' highlights cleared (Esc), find's included: whether there were any. */
  unmark() {
    const had = this.viewer.unmark();
    if (had) this.hits = [];
    return had;
  }
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
    this.again();
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
    this.$("#zs").textContent = z === "fit" ? "Fit" : z === "page" ? "Page" : `${Math.round(Number(z) * 100)}%`;
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
        const typing2 = e.target?.closest?.("input, textarea, select, [contenteditable=true]") && e.composedPath()[0] !== q;
        if (e.ctrlKey) {
          const k = e.key;
          if (k === "f" && this.ev.onFind) this.find(true);
          else if (k === "=" || k === "+") this.zoomStep(1);
          else if (k === "-") this.zoomStep(-1);
          else if (k === "0") this.zoomTo("fit");
          else return;
        } else if (typing2 || e.composedPath()[0] === q) return;
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
    this.ev.onDebug?.(this.debugOn);
    this.msg(this.debugOn ? "Debug check on" : "Debug check off", false, true);
    this.emit();
  }
  /**
   * The timing popover under a host's own control (Overleaf's toolbar
   * status, where the panel's bar is hidden): moved out of the bar into the
   * window, at the control's left, just under it. Again: closed.
   */
  timingAt(anchor) {
    const el = this.$("#m-time");
    if (!el.hidden) return this.menu(null);
    if (el.parentElement !== this.win) this.win.append(el);
    const a = anchor.getBoundingClientRect(), w = this.win.getBoundingClientRect();
    el.style.left = `${Math.max(6, Math.min(a.left - w.left, w.width - el.offsetWidth - 6, w.width - 346))}px`;
    el.style.top = `${Math.max(4, a.bottom - w.top + 4)}px`;
    this.menu("#m-time");
  }
  // ---- Settings: the panel's own rows and the host's, one dialog in every host ----
  /** The rows drawn (by data-id): what a click or a change sets. */
  settingRows = /* @__PURE__ */ new Map();
  /** The Settings dialog shown (drawn anew: the host's values as they are now) or hidden. */
  async settings(on = this.$("#settings").hidden) {
    const el = this.$("#settings");
    if (!on) {
      if (!el.hidden) el.hidden = true, this.emit();
      return;
    }
    const groups = this.ownSettings();
    for (const g of await this.ev.settings?.() ?? []) {
      const same = groups.find((x) => x.title === g.title);
      if (same) same.rows.push(...g.rows);
      else groups.push({ title: g.title, rows: [...g.rows] });
    }
    this.settingRows.clear();
    const shown = groups.filter((g) => g.rows.length);
    const at = Math.max(0, shown.findIndex((g) => g.title === this.settingsTab));
    el.innerHTML = `<header>Settings<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><nav class="stabs" role="tablist">${shown.map((g, i) => `<button type="button" role="tab" data-tab="${i}" data-group="${esc4(g.title)}" aria-selected="${i === at}">${esc4(g.title)}</button>`).join("")}</nav><div class="sbody"></div>`;
    const body = el.querySelector(".sbody");
    shown.forEach((g, i) => {
      const sec = document.createElement("section");
      sec.setAttribute("role", "tabpanel");
      sec.hidden = i !== at;
      for (const r of g.rows) sec.append(this.settingRow(r));
      body.append(sec);
    });
    el.querySelector(".stabs").onclick = (e) => {
      const t = e.target.closest("[data-tab]");
      if (!t) return;
      const k = Number(t.dataset.tab);
      this.settingsTab = shown[k].title;
      el.querySelectorAll("[data-tab]").forEach((b) => b.setAttribute("aria-selected", String(b === t)));
      body.querySelectorAll("section").forEach((s, i) => s.hidden = i !== k);
    };
    el.hidden = false;
    this.emit();
  }
  /** The Settings tab last shown (its group's title). */
  settingsTab = "";
  /** One row: its label and help, its control, a line for the host's error. */
  settingRow(r) {
    this.settingRows.set(r.id, r);
    const row = document.createElement("div");
    row.className = `srow ${r.kind}${r.disabled ? " off" : ""}`;
    row.dataset.id = r.id;
    row.innerHTML = `<b class="lab"></b><div class="ctl"></div><small class="help"></small><div class="err" hidden></div>`;
    row.querySelector("b").textContent = r.label;
    row.querySelector("small").textContent = r.disabled ?? r.help ?? "";
    const ctl = row.querySelector(".ctl");
    const dis = r.disabled || !r.set ? " disabled" : "";
    if (r.kind === "toggle") ctl.innerHTML = `<button type="button" role="switch" class="switch" aria-checked="${r.value === true}" aria-label="${esc4(r.label)}"${dis}><i></i></button>`;
    else if (r.kind === "choice")
      ctl.innerHTML = `<div class="seg" role="group" aria-label="${esc4(r.label)}">${(r.options ?? []).map(([v, l]) => `<button type="button" data-v="${esc4(v)}" aria-pressed="${r.value === v}"${dis}>${esc4(l)}</button>`).join("")}</div>`;
    else if (r.kind === "select") {
      const s = document.createElement("select");
      s.setAttribute("aria-label", r.label);
      s.disabled = !!dis;
      for (const [v, l] of r.options ?? []) s.append(new Option(l, v, v === r.value, v === r.value));
      ctl.append(s);
    } else if (r.kind === "action") ctl.innerHTML = `<button type="button" class="btn"${dis}>${esc4(r.button ?? r.label)}</button>`;
    else ctl.innerHTML = `<span class="val"></span>`, ctl.querySelector(".val").textContent = String(r.value ?? "");
    return row;
  }
  /** A row's control used: its new value to the row's set, its error (if any) under it. */
  async settingClicked(e) {
    const t = e.target;
    const row = t.closest(".srow");
    const r = row && this.settingRows.get(row.dataset.id);
    if (!row || !r?.set || r.disabled) return;
    let v;
    if (r.kind === "toggle" && e.type === "click" && t.closest(".switch")) v = r.value !== true;
    else if (r.kind === "choice" && e.type === "click") v = t.closest("[data-v]")?.dataset.v;
    else if (r.kind === "select" && e.type === "change") v = t.value;
    else if (r.kind === "action" && e.type === "click" && t.closest(".btn")) v = void 0;
    else return;
    if (r.kind !== "action" && (v === void 0 || v === r.value)) return;
    const err = row.querySelector(".err");
    const why = await r.set(v);
    err.hidden = !why;
    err.textContent = why || "";
    if (why) return;
    r.value = v;
    if (r.kind === "toggle") row.querySelector(".switch").setAttribute("aria-checked", String(v === true));
    if (r.kind === "choice") row.querySelectorAll("[data-v]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.v === v)));
  }
  /** The panel's own rows: the main file, the pages' look, the debug check, the tips. */
  ownSettings() {
    const ev = this.ev;
    const pref = (k, apply2) => async (v) => {
      const why = await ev.onPref?.(k, v);
      if (!why) apply2(v);
      return why;
    };
    const keep = ev.onPref ? void 0 : "This host doesn't keep it";
    const preview = [];
    if (this.mainFiles.names.length)
      preview.push({ id: "main", label: "Main file", help: "The file PhiTeX typesets", kind: "select", options: this.mainFiles.names.map((n) => [n, n]), value: this.mainFiles.main ?? "", set: (v) => void ev.onMain(String(v)) });
    const look2 = [];
    look2.push(
      { id: "dark-pdf", label: "Dark PDF", help: "Darker pages; it inverts figures and photos too", kind: "choice", options: [["off", "Off"], ["on", "On"], ["theme", "Match theme"]], value: this.dark, disabled: keep, set: pref("dark-pdf", (d) => this.darkPdf(d)) },
      { id: "opening", label: "Loading animation", help: "Played on the first page's sheet while it is typeset. Random: a different one each time", kind: "select", options: [["random", "Random"], ...LOOKS.map((l) => [l.id, l.name]), ["off", "Off"]], value: this.look, disabled: keep, set: pref("opening", (l) => this.look = lookOf(l)) }
    );
    if (ev.onBoxes) look2.push({ id: "underline", label: "Underline problems", help: "A wavy line under what each error and warning is about", kind: "toggle", value: this.culpritsOn, disabled: keep, set: pref("underline", (on) => this.culpritsShown(on)) });
    preview.push({ id: "problems", label: "Problems shown", help: "Which kinds: the filters at the top of the problems list", kind: "info", value: this.hiddenKinds.size ? `${this.hiddenKinds.size} kind${this.hiddenKinds.size === 1 ? "" : "s"} hidden` : "All" });
    const build = [];
    if (ev.onDebug) build.push({ id: "debug", label: "Debug check", help: "Compare with a fresh build every 5 s (this session only)", kind: "toggle", value: this.debugOn, set: () => this.toggleDebug() });
    build.push({ id: "reload", label: "Reload the files", help: "Read the project's files again", kind: "action", button: "Reload", set: () => (this.settings(false), void ev.onReload()) });
    const tips = [{ id: "tips", label: "Feature tips", help: "Shown again, from the start", kind: "action", button: "Show again", set: () => (this.tipsReset(), this.settings(false)) }];
    if (ev.onTour) tips.push({ id: "tour", label: "The tour", kind: "action", button: "Take the tour", set: () => (this.settings(false), ev.onTour()) });
    if (ev.onNews) tips.push({ id: "news", label: "What's new", kind: "action", button: "Show", set: () => (this.settings(false), ev.onNews()) });
    return [
      { title: "Preview", rows: preview },
      { title: "Appearance", rows: look2 },
      { title: "Build", rows: build },
      { title: "Packages", rows: [] },
      { title: "Updates and tips", rows: tips }
    ];
  }
  /** The panel's own choices as the host keeps them (ViewPrefs): applied, not told back. */
  viewPrefs(p) {
    if (p["dark-pdf"]) this.darkPdf(p["dark-pdf"]);
    if (p.opening) this.look = lookOf(p.opening);
    if (p.underline !== void 0 && p.underline !== this.culpritsOn) this.culpritsShown(p.underline);
  }
  /** Repaints so far: the first is the first build's. */
  paints = 0;
  /** Where the pages' strips say the last frame's numbers (drawn, kept, watch, probe, draw, fetch, compose): under the ⚡ chip's. */
  stripsChip() {
    return this.$("#stripsline");
  }
  /** A repaint reached the screen, `ms` after its keystroke: the status says how fast. */
  painted(ms) {
    const first = this.paints++ === 0;
    if (first) this.walkEnd();
    if (first && this.walk?.ms !== void 0) ms = this.walk.ms;
    this.speed = { ms, at: performance.now(), first };
    this.$("#speedms").textContent = `\u26A1 ${this.speed.first ? "first page " : ""}${dur(ms)}`;
    this.renderStatus();
    if (!this.$("#m-time").hidden) this.timing();
    this.emit();
  }
  // ---- Dark PDF: the user's choice, Off by default (a static filter: it inverts figures too) ----
  dark = "off";
  darkQuery = matchMedia("(prefers-color-scheme: dark)");
  /** Dark pages off, on, or as the host's theme is (the host keeps the choice: viewPrefs). */
  darkPdf(d) {
    this.dark = d;
    this.darkQuery.onchange = () => this.applyDark();
    this.applyDark();
  }
  /** The loading animation (the host keeps the choice: viewPrefs). */
  look = "random";
  /** The last one played (random plays another). */
  lastLook = null;
  /** The host page's own theme (theme()): undefined, never said. */
  pageTheme = void 0;
  /**
   * The host page's theme: "dark" or "light" as its own toggle says, null
   * to follow the system (prefers-color-scheme). For hosts whose page has a
   * toggle of its own (the web frontend); Overleaf's comes from dock(), VS
   * Code's from its body.
   */
  theme(t) {
    this.pageTheme = t;
    this.win.classList.toggle("dark", t === "dark");
    this.win.classList.toggle("light", t === "light");
    this.applyDark();
  }
  /** Whether the host's theme is dark: Overleaf's (dock() says), VS Code's (its body's class), the page's (theme()), the system's. */
  hostDark() {
    if (this.kind === "overleaf") return !this.win.classList.contains("light");
    if (this.kind === "vscode") return !/\bvscode-light\b/.test(document.body.className);
    return this.pageTheme ? this.pageTheme === "dark" : this.darkQuery.matches;
  }
  applyDark() {
    this.win.classList.toggle("dark-pdf", this.dark === "on" || this.dark === "theme" && this.hostDark());
  }
  // ---- Feature tips, each once (own-bar hosts; Overleaf's toolbar has the extension's) ----
  tipAt = -1;
  tipList = [];
  /**
   * The tips now: those not seen yet, where the host keeps them from showing
   * by themselves (PanelOptions.tips false: after its own); `all`, the tour
   * itself, every tip from the first (a host's own "Take the tour").
   */
  tour(all = false) {
    this.tips(all ? "all" : "unseen", true);
  }
  /**
   * The tips, from the first: those not seen yet (a first visit), the new
   * features' (What's new: `fresh`), or every one (the tour); `asked`: the
   * host's tour().
   */
  /** The tips seen: this browser's, and the host's (tipsSeen). */
  seenIds = (() => {
    try {
      return new Set(JSON.parse(localStorage.getItem("phitex.tips.seen") ?? "[]"));
    } catch {
      return /* @__PURE__ */ new Set();
    }
  })();
  /** The tips seen as the host keeps them (the watch: the user's phitex.toml, the same on every port): not offered by themselves again. */
  tipsSeen(ids) {
    for (const id of ids) this.seenIds.add(id);
    if (!this.$("#tip").hidden && this.tipList.every((t) => this.seenIds.has(t.id))) this.tip(-1);
  }
  /** Every tip unseen again (Settings' Show again): offered from the start at the next page. */
  tipsReset() {
    this.seenIds.clear();
    try {
      localStorage.removeItem("phitex.tips.seen");
    } catch {
    }
    this.tips("unseen", true);
  }
  tips(which, asked = false) {
    if (!this.ownBar || this.prefs.collapsed || which === "unseen" && !asked && !this.autoTips) return;
    const seen = [...this.seenIds];
    const offered = TIPS.filter((t) => !(t.id === "compare" && !this.ev.onCompare) && !(t.id === "contents" && !this.contents) && !(t.id === "live" && this.kind === "overleaf"));
    this.tipList = which === "all" ? offered : which === "fresh" ? offered.filter((t) => t.fresh) : offered.filter((t) => !seen.includes(t.id));
    this.tip(0);
  }
  /** Tips seen: not offered by themselves again (What's new and the tour still show them). */
  seen(ids) {
    const before = this.seenIds.size;
    for (const id of ids) this.seenIds.add(id);
    if (this.seenIds.size === before) return;
    try {
      localStorage.setItem("phitex.tips.seen", JSON.stringify([...this.seenIds]));
    } catch {
    }
    this.ev.onTipsSeen?.([...this.seenIds]);
  }
  tip(i) {
    const el = this.$("#tip");
    this.root.querySelectorAll(".ringed").forEach((e) => e.classList.remove("ringed"));
    this.tipAt = i;
    const t = this.tipList[i];
    if (!t) return void (el.hidden = true);
    this.seen([t.id]);
    let target = this.$(t.at);
    if (!target || !target.offsetParent) target = this.$("#b-more");
    target.classList.add("ringed");
    el.innerHTML = `<h4>${t.fresh ? '<span class="new">New</span>' : ""}${esc4(t.title)}</h4>${t.demo}<p>${esc4(t.text)}</p><div class="acts"><span class="n">${i + 1} of ${this.tipList.length}</span><button type="button" data-t="skip">Skip</button><button type="button" class="primary" data-t="next">${i + 1 < this.tipList.length ? "Next" : "Got it"}</button></div>`;
    el.hidden = false;
    const bar2 = this.$(".bar").getBoundingClientRect(), r = target.getBoundingClientRect(), w = el.offsetWidth;
    const cx = r.left + r.width / 2 - bar2.left, left = Math.max(8, Math.min(bar2.width - w - 8, cx - w / 2));
    Object.assign(el.style, { left: `${left}px`, top: `${r.bottom - bar2.top + 9}px` });
    el.style.setProperty("--ax", `${cx - left}px`);
    el.onclick = (e) => {
      e.stopPropagation();
      const a = e.target.closest("[data-t]")?.dataset.t;
      if (a === "skip") this.seen(this.tipList.map((t2) => t2.id));
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
    const esc6 = (t) => t.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
    const n = d.busy ? `<span class="n busy">${esc6(d.busy)}</span>` : `<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    el.innerHTML = `<span class="what" title="${esc6(d.title)} against the current version">${esc6(d.title)} vs current <small>${esc6(d.when)}</small></span><button data-a="prev" title="Previous change (Shift+N)" ${d.busy || !d.changes ? "disabled" : ""}>\u2039</button>${n}<button data-a="next" title="Next change (N)" ${d.busy || !d.changes ? "disabled" : ""}>\u203A</button><span class="sep"></span><span class="seg" role="group" aria-label="Pages shown (D)"><button data-a="${d.showing === "diff" ? "settings" : "toggle"}" class="on-diff" aria-pressed="${d.showing === "diff"}" title="${d.showing === "diff" ? "The diff is shown: its style (colors, markup, change bars)" : "Show the diff (D)"}">Diff${d.showing === "diff" ? ` <span class="chev" aria-label="Diff style">\u25BE</span>` : ""}</button><button data-a="${d.showing === "current" ? "" : "toggle"}" class="on-current" aria-pressed="${d.showing === "current"}" title="${d.showing === "current" ? "The current version is shown" : "Show the current version (D)"}">Current</button></span><button data-a="pdf" title="Download the diff PDF" ${d.busy ? "disabled" : ""}>\u2B07 PDF</button><button data-a="tex" title="Download diff.tex" ${d.busy ? "disabled" : ""}>\u2B07 .tex</button><button data-a="settings" title="Diff style: colors, markup, change bars" aria-label="Diff style">\u2699</button><button data-a="close" title="Stop comparing" aria-label="Stop comparing">\u2715</button>`;
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
      el.className = "menu diffset";
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
      ["invisible", "Hide deletions"]
    ];
    const WITH_BARS = { underline: "culinechbar", cfont: "cfontchbar", invisible: "changebar", ctraditional: "cchangebar" };
    const BASE2 = Object.fromEntries(Object.entries(WITH_BARS).map(([a, b]) => [b, a]));
    const base = BASE2[look2.markup] ?? look2.markup;
    const bars = look2.markup in BASE2;
    const SUB = [["safe", "Plain"], ["color", "Colored"], ["marker", "Margin marks"]];
    const PAIRS = [
      ["#0000ff", "#ff0000", "Blue / red"],
      ["#15803d", "#b91c1c", "Green / red"],
      ["#0072b2", "#d55e00", "Colorblind-safe"],
      ["#7e22ce", "#6b7280", "Purple / gray"]
    ];
    const seg = (key, opts) => `<div class="opts">${opts.map(([v, t]) => `<button data-k="${key}" data-v="${v}" aria-pressed="${(key === "markup" ? base : look2[key]) === v}">${t}</button>`).join("")}</div>`;
    const canBars = base in WITH_BARS;
    const deco = (add) => {
      const c = add ? look2.add_color : look2.del_color;
      if (look2.markup === "invisible" && !add) return "display:none";
      const strike = !add && /^(underline|fontstrike|culinechbar)$/.test(look2.markup);
      const line = add && /^(underline|culinechbar)$/.test(look2.markup) ? `text-decoration: underline wavy ${c}; text-underline-offset: 3px;` : "";
      return `color:${c};${strike ? `text-decoration: line-through ${c};` : ""}${line}${look2.markup === "bold" && add ? "font-weight:700;" : ""}`;
    };
    el.innerHTML = `<h4>Style</h4>${seg("markup", MARKUP)}<div class="row bars"><span>Change bars in the margin</span><button data-bars="1" role="switch" aria-checked="${bars}" aria-pressed="${bars}" ${canBars ? "" : 'disabled title="Not with this style"'}>${bars ? "On" : "Off"}</button></div><h4>Colors</h4><div class="opts">${PAIRS.map(([a, d, t]) => `<button class="pair" data-pair="${a},${d}" aria-pressed="${look2.add_color === a && look2.del_color === d}"><i style="background:${a}"></i><i style="background:${d}"></i>${t}</button>`).join("")}</div><div class="row"><span>Added</span><input type="color" data-c="add_color" value="${look2.add_color}"><span>Deleted</span><input type="color" data-c="del_color" value="${look2.del_color}"></div><h4>Mode</h4>${seg("subtype", SUB)}<div class="sample"${bars ? ` style="box-shadow: inset 3px 0 0 ${look2.add_color}; padding-left: 12px"` : ""}>The results are <span style="${deco(false)}">good</span> <span style="${deco(true)}">excellent</span>.</div><div class="foot"><small>As latexdiff's --type, --subtype</small><button data-reset="1">Reset</button></div>`;
    el.className = "menu diffset on";
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
      if (b.dataset.bars) return canBars && set({ markup: bars ? base : WITH_BARS[base] });
      if (b.dataset.k === "markup") return set({ markup: bars && b.dataset.v in WITH_BARS ? WITH_BARS[b.dataset.v] : b.dataset.v });
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
    if (!this.$("#m-time").hidden) this.timing();
  }
  /**
   * This opening's steps as the sheet said them (the project read, the
   * packages, the format, typesetting), each with the time it was up, from
   * the first step to the first page on screen (`ms`, then closed): the
   * first build's time as the writer waited for it, not the engine's part.
   */
  walk = null;
  /** The opening's step is now `what` (a step of the sheet's). */
  walkTo(what) {
    const now = performance.now();
    const w = this.walk ??= { since: now, parts: [], at: now };
    if (w.ms !== void 0) return;
    const step = what === this.words.steps[1] ? what : what === this.words.steps[2] ? this.making ? "Making the format" : what : this.words.steps[0];
    if (step === w.now) return;
    this.walkAdd(now);
    w.now = step, w.at = now;
  }
  /** The step up until `now` counted (a step seen again adds to its first time). */
  walkAdd(now) {
    const w = this.walk;
    if (!w.now) return;
    const p = w.parts.find(([k]) => k === w.now);
    if (p) p[1] += now - w.at;
    else w.parts.push([w.now, now - w.at]);
  }
  /** The first page is on screen: the opening's time closed. */
  walkEnd() {
    const w = this.walk;
    if (!w || w.ms !== void 0) return;
    const now = performance.now();
    this.walkAdd(now);
    w.now = void 0;
    w.ms = now - w.since;
  }
  /** The timing popover's first build: each step's time, as a bar, from opening to the first page. */
  walkPart() {
    const w = this.walk;
    if (!w || w.ms === void 0 || !this.speed?.first) return "";
    const parts = w.parts.filter(([, ms]) => ms >= 1);
    return (
      // (its total is the head's, "First page · 124 s": said once)
      `<div class="row"><span class="k">Where the time went</span><span class="v"></span><div class="tbar">${parts.map(([, ms], i) => `<i class="c${i % 4}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`).join("")}</div><div class="legend">${parts.map(([n, ms], i) => `<span><i class="c${i % 4}"></i>${esc4(n)} ${esc4(dur(ms))}</span>`).join("")}</div></div>`
    );
  }
  /** The last build's parts as its host measured them (the watch's stats), in order, and work done alongside them (not in the build's time): [name, ms]. */
  phases = null;
  /** The last build's parts (`parts` add up to its time; `beside`: done at the same time, said under the bar). */
  buildParts(parts, beside = []) {
    this.phases = { parts, beside };
    if (!this.$("#m-time").hidden) this.timing();
  }
  /** The timing popover's "Where the last build's time went": its parts as a bar, like the opening's. */
  phasesPart() {
    const p = this.phases;
    if (!p) return "";
    const parts = p.parts.filter(([, ms]) => ms >= 1);
    if (!parts.length) return "";
    const beside = p.beside.filter(([, ms]) => ms >= 1).map(([n, ms]) => `${n} ${dur(ms)}`).join(", ");
    return `<div class="row"><span class="k">Where the last build's time went</span><span class="v"></span><div class="tbar">${parts.map(([, ms], i) => `<i class="c${i % 4}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`).join("")}</div><div class="legend">${parts.map(([n, ms], i) => `<span><i class="c${i % 4}"></i>${esc4(n)} ${esc4(dur(ms))}</span>`).join("")}</div>` + (beside ? `<span class="note">${esc4(`Alongside: ${beside}`)}</span>` : "") + `</div>`;
  }
  /** The packages fetched since the project opened: how many, their bytes, and the time any was on its way (`since`: one is now). */
  downloads = { n: 0, bytes: 0, ms: 0 };
  /** The timing popover's Downloads row: the packages' time, how many and their size; none fetched, none. */
  downloadsPart() {
    const d = this.downloads;
    if (!d.n && d.since === void 0) return "";
    const ms = d.ms + (d.since === void 0 ? 0 : performance.now() - d.since);
    const size = d.bytes ? ` \xB7 ${d.bytes < 1e6 ? `${Math.max(1, Math.round(d.bytes / 1e3))} kB` : `${(d.bytes / 1e6).toFixed(1)} MB`}` : "";
    return `<div class="row"><span class="k">Downloads</span><span class="v">${esc4(dur(ms))}</span><span class="note">${d.n} package${d.n === 1 ? "" : "s"}${size}${d.since === void 0 ? "" : ", more on the way"}</span></div>`;
  }
  /** The first build's time and the edits' since (the watch's), for the popover's bars. */
  times = { edits: [] };
  /** A build's time: the first (cold) one's, or an edit's: drawn as bars beside each other in the timing popover. */
  buildTime(ms, first) {
    if (first) this.times = { first: ms, edits: [] };
    else this.times.edits.push(ms);
    this.built = { ms, at: performance.now(), first };
    this.renderStatus();
    if (!this.$("#m-time").hidden) this.timing();
    this.emit();
  }
  /** The last build the host said (buildTime), painted or not; `joined`: the one before this tab joined (joinedLate). */
  built;
  /**
   * What the status and the timing popover's head say: the newest of the
   * last paint (its time from the keystroke) and the last build (one that
   * changed no page paints nothing, and was not said before: grail's 7 s).
   */
  latest() {
    const s = this.speed, b = this.built;
    if (b && (!s || b.at > s.at)) return { label: b.first ? "First build" : b.joined ? "Last build" : "Built, no page changed", ms: b.ms, first: b.first };
    if (s) return { label: s.first ? "First page" : "Built", ms: s.ms, first: !!s.first };
    return void 0;
  }
  /**
   * This tab joined `builds` builds in: `first` the first build's time for
   * the bars; the first build's paint was not this tab's, so its next is not
   * called one; `last` the last build's time, for the status and the popover's
   * head ("Last build", or "First build" if it was the only one).
   */
  joinedLate(first, builds, last) {
    if (first !== void 0 && this.times.first === void 0) this.times.first = first;
    if (builds >= 1) this.paints = Math.max(this.paints, 1);
    if (last && !this.built) {
      this.built = { ms: last.ms, at: performance.now(), first: last.cold, joined: true };
      this.renderStatus();
      if (!this.$("#m-time").hidden) this.timing();
    }
  }
  /** The bars: the first build, the last edit, the edits' median, each as long as its time (the longest the full width; a short one at least a sliver). */
  timeBars() {
    const t = this.times;
    if (t.first === void 0 || !t.edits.length) return "";
    const sorted = [...t.edits].sort((a, b) => a - b);
    const rows = [
      ["First build", t.first, "first"],
      ["This edit", t.edits[t.edits.length - 1], "edit"],
      ...t.edits.length > 2 ? [["Edits, median", sorted[sorted.length >> 1], "median"]] : []
    ];
    const max = Math.max(...rows.map((r) => r[1]), 1e-3);
    const x = t.first / Math.max(t.edits[t.edits.length - 1], 1e-3);
    return `<div class="tbars" role="img" aria-label="First build ${esc4(dur(t.first))}, this edit ${esc4(dur(t.edits[t.edits.length - 1]))}">` + rows.map(([k, ms, c]) => `<div class="tb"><span class="k">${k}</span><span class="track"><i class="${c}" style="width:${Math.max(0.6, 100 * ms / max).toFixed(2)}%"></i></span><span class="v">${esc4(dur(ms))}</span></div>`).join("") + (x >= 1.5 ? `<div class="tb x">${x < 10 ? x.toFixed(1) : Math.round(x)}\xD7 faster than the first build</div>` : "") + `</div>`;
  }
  /** The last build's parts as the session lists them: [what, how long]. */
  timingParts() {
    return (this.lat?.details ?? "").split("\n").filter(Boolean).map((l) => l.trim().split(/\s{2,}/)).map(([k, ...v]) => [k ?? "", v.join(" ")]);
  }
  /** The timing popover: the last build's line, its parts as the session lists them, the last message. */
  timing() {
    const el = this.$("#m-time");
    const parts = this.timingParts().map(([k, v]) => part2(k, v));
    const l = this.latest();
    const e2e = parts.find((p) => p.key === "keystroke\u2192page")?.ms ?? l?.ms;
    const head = !l ? e2e !== void 0 ? ["Keystroke to page", e2e] : null : l.first || l.label !== "Built" ? [l.label, l.ms] : ["Keystroke to page", e2e ?? l.ms];
    const rows = parts.map((p) => {
      const bar2 = p.split?.length ? `<div class="tbar">${p.split.map(([n, ms], i) => `<i class="c${i}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`).join("")}</div><div class="legend">${p.split.map(([n, ms], i) => `<span><i class="c${i}"></i>${esc4(n)} ${esc4(dur(ms))}</span>`).join("")}</div>` : "";
      return `<div class="row"><span class="k">${esc4(p.label)}</span><span class="v">${esc4(p.value)}</span>${bar2}${p.note ? `<span class="note">${esc4(p.note)}</span>` : ""}</div>`;
    }).join("");
    const strips2 = framePart(this.$("#stripsline").dataset.frame);
    el.innerHTML = (head ? `<div class="head"><span>${esc4(String(head[0]))}</span><b>${esc4(dur(head[1]))}</b></div>` : `<div class="head"><span>No build yet</span></div>`) + this.walkPart() + this.phasesPart() + this.timeBars() + this.downloadsPart() + rows + strips2 + `<div class="foot"></div><div class="foot msg${this.msgErr ? " err" : ""}"></div><button type="button" class="reportbtn copydbg">Copy debug info</button>`;
    el.querySelector(".copydbg").onclick = (e) => {
      e.stopPropagation();
      const b = e.currentTarget;
      void this.debugInfo().then((t) => navigator.clipboard.writeText(t)).then(() => b.textContent = "Copied \u2713", () => b.textContent = "Couldn't copy");
    };
    el.querySelector(".foot").textContent = parts.length || this.walk?.ms !== void 0 ? "" : this.lat?.summary ?? "";
    el.querySelector(".msg").textContent = this.msgText;
  }
  /**
   * The timing popover's numbers as plain text, for a bug report or a chat:
   * the host's lines (version, engine, files), the last build's head and
   * parts as the session or the watch said them, the first page's steps,
   * the downloads, the last page drawn, and this machine.
   */
  async debugInfo() {
    const out = [...await this.ev.onDebugInfo?.().catch(() => []) ?? []];
    const l = this.latest();
    if (l) out.push(`${l.label}: ${dur(l.ms)}`);
    if (this.lat?.summary) out.push(this.lat.summary);
    for (const line of (this.lat?.details ?? "").split("\n").filter(Boolean)) out.push(`  ${line.trim()}`);
    const w = this.walk;
    if (w?.ms !== void 0) out.push(`opening to first page ${dur(w.ms)}: ${w.parts.map(([n, ms]) => `${n.toLowerCase()} ${dur(ms)}`).join(", ")}`);
    const d = this.downloads;
    if (d.n) out.push(`downloads: ${d.n} packages, ${(d.bytes / 1e6).toFixed(1)} MB, ${dur(d.ms)}`);
    const t = this.times;
    if (t.first !== void 0) out.push(`first build ${dur(t.first)}${t.edits.length ? `; edits ${t.edits.length}, last ${dur(t.edits[t.edits.length - 1])}, median ${dur([...t.edits].sort((a, b) => a - b)[t.edits.length >> 1])}` : ""}`);
    const f = this.$("#stripsline").dataset.frame;
    if (f) out.push(`last page drawn: ${frameText(f)}`);
    const nav = navigator;
    out.push(`machine: ${nav.hardwareConcurrency ?? "?"} cores, ${nav.userAgentData?.platform || nav.platform || "?"}; ${nav.userAgent}`);
    return out.join("\n") + "\n";
  }
  /** The last message (connected, files read, a debug check's result), shown in the timing popover. */
  msgText = "";
  msgErr = false;
  /**
   * The status's message (in its popover); `shown`: the answer to something
   * the user did (Clear cache, a toggle), so a note in the corner says it
   * too, for a few seconds, or until clicked away if it is an error.
   */
  msg(t, err = false, shown = false) {
    this.msgText = t;
    this.msgErr = err;
    if (!this.$("#m-time").hidden) this.timing();
    if (shown) this.toast("msg", { title: t, pct: null, icon: err ? "warning" : "info", err, ms: err ? void 0 : 4e3 });
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
    this.msg(`${names.length} file${names.length === 1 ? "" : "s"} read ${took} \xB7 they stay in this browser`);
  }
  /** The main files to pick from (Settings' Main file), and the one built. */
  mains(names, main) {
    this.mainFiles = { names, main };
    if (!this.$("#settings").hidden) void this.settings(true);
  }
  mainFiles = { names: [], main: null };
  /** The document's main file as the host names it, where it has no Main file to pick (the watch): the openings that show it (a prompt's, a title bar's). */
  docName(name) {
    this.doc = name;
  }
  doc = null;
  /** What's new (the host's news, `text` as plain lines), in a dialog over the pages, with the new features' tips a click away. */
  news(version, text) {
    const box = this.$("#logview"), pre = this.$("#logtext");
    box.querySelector("header").firstChild.textContent = `What's new in ${version}`;
    pre.textContent = text;
    let b = box.querySelector("#newstips");
    if (!b) {
      b = document.createElement("button");
      b.id = "newstips";
      b.className = "btn";
      b.textContent = "Show the new features";
      b.onclick = () => (box.hidden = true, this.tips("fresh"));
      box.append(b);
    }
    b.hidden = false;
    box.hidden = false;
    box.querySelector("[data-close]")?.focus();
  }
  /** The last build's log (the host's), in a dialog over the pages. */
  async showLog() {
    const box = this.$("#logview"), pre = this.$("#logtext");
    pre.textContent = "Reading the log\u2026";
    box.querySelector("header").firstChild.textContent = "The last build's log";
    const nb = box.querySelector("#newstips");
    if (nb) nb.hidden = true;
    box.hidden = false;
    box.querySelector("[data-close]")?.focus();
    const text = await this.ev.onLog?.().catch((e) => `Couldn't read the log: ${e instanceof Error ? e.message : String(e)}`);
    pre.textContent = text ?? "No log yet: the first build hasn't finished.";
  }
  redraw() {
    this.viewer?.redraw();
    if (this.viewer) requestAnimationFrame(() => this.drawScrollMarks());
  }
  /** The page box, for the page in view. */
  nav() {
    const n = this.viewer.pages;
    const k = this.at;
    const pg = this.$("#pgin");
    if (this.root.activeElement !== pg) pg.value = n ? String(k + 1) : "\u2013";
    this.$("#pgn").textContent = String(n);
    pg.style.width = `${String(n || 1).length + 1.6}ch`;
    queueMicrotask(() => this.emit());
  }
  /** The pages there are, by hash (the viewer draws the changed ones it shows). */
  layout(hashes) {
    this.pageHashes = hashes;
    this.$("#empty").hidden = true;
    this.$("#stage").classList.remove("stale");
    this.viewer.layout(hashes);
    if (this.at >= hashes.length) this.at = Math.max(0, hashes.length - 1);
    if (hashes.length) this.sheetDown();
    this.nav();
    if (this.culpritDiags.length) void this.underline(this.culpritDiags);
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
    this.walkEnd();
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
var downloaded = (n, bytes) => n ? `${n} package${n === 1 ? "" : "s"} downloaded${mb(bytes)}` : "";
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
function glyphs2(json, path) {
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
var widthsOf = /* @__PURE__ */ new WeakMap();
function widths(all) {
  let next = widthsOf.get(all);
  if (next) return next;
  next = /* @__PURE__ */ new Map();
  for (let i = 0; i < all.length; i++) {
    const n = all[i + 1];
    next.set(all[i], n && Math.abs(n.y - all[i].y) < 0.5 && n.x > all[i].x && n.x - all[i].x < 15 ? n.x - all[i].x : 5);
  }
  widthsOf.set(all, next);
  return next;
}
function boxes(gs, all) {
  const next = widths(all);
  const lines2 = /* @__PURE__ */ new Map();
  for (const g of gs) {
    const k = Math.round(g.y * 2) / 2;
    const r = lines2.get(k), e = g.x + (next.get(g) ?? 5);
    lines2.set(k, r ? [Math.min(r[0], g.x), Math.max(r[1], e)] : [g.x, e]);
  }
  return [...lines2].map(([y, [a, b]]) => [a - 1, y - 8, b - a + 2, 11]);
}
function lineAt2(hits, at) {
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
var NEAR_CULPRIT = 64;
function place3(t, pos) {
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
  const strip = (t) => t.replace(/(^|[^\\])%.*$/gm, "$1");
  const inputs = /* @__PURE__ */ new Map();
  for (const n of names) {
    const dir = n.includes("/") ? n.slice(0, n.lastIndexOf("/") + 1) : "";
    const got = [];
    for (const m of strip(files[n]).matchAll(/\\(?:input|include|subfile|subfileinclude|includestandalone)\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}/g)) {
      const f = m[1].trim().replace(/^\.\//, "");
      const tex = /\.tex$/i.test(f) ? f : `${f}.tex`;
      got.push(tex, dir + tex);
    }
    inputs.set(n, got);
  }
  const inputted = /* @__PURE__ */ new Set();
  for (const [n, got] of inputs) for (const f of got) if (f !== n) inputted.add(f);
  const score = (n) => {
    const t = strip(files[n]);
    let s = 0;
    if (inputted.has(n)) s -= 16;
    if (inputs.get(n).some((f) => f !== n && f in files)) s += 8;
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
  /**
   * A pack the host fetches for the core (a core that fetches its own: the
   * panel had said "0 package files" all through a cold build's downloads):
   * loading while it comes, then done with its bytes counted, or failed.
   */
  onPack(e) {
    this.tr("package: host fetches for the core", { id: e.id, state: e.state, bytes: e.bytes });
    this.pkg.source = this.source.label || "Shelf";
    const p = this.pkg;
    p.loading = p.loading.filter((n) => n !== e.id);
    if (e.state === "fetching") {
      if (!p.done?.includes(e.id)) p.loading = [...p.loading, e.id];
    } else if (e.state === "done") {
      if (!p.done?.includes(e.id)) p.done = [...p.done ?? [], e.id], p.bytes = (p.bytes ?? 0) + (e.bytes ?? 0);
    } else p.failed = [...(p.failed ?? []).filter((f) => f.name !== e.id), { name: e.id, error: "could not be fetched" }];
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
      const ev = e.event ?? "";
      if (this.ended.length && (ev === "settled" || ev === "failed" || ev === "trapped")) for (const f of this.ended.splice(0)) f(e);
      if (ev === "settled") {
        const b = e;
        if (typeof b.ms === "number" && !b.settling) this.sink.buildTime?.(b.ms, !!b.cold);
      }
      if (e.event === "follow") return this.followBytes(e.file, e.at, e.word, e.line);
      if (e.event === "select") return void this.selectBytes(e.file, e.from, e.to);
      if (this.o.remote && e.event === "origins") return void (this.originsSeen++, this.glyphsAsked.clear(), this.glyphCache.clear(), this.culprits.clear(), this.sink.origins?.());
      if (this.o.remote && "event" in e && WATCH2.has(e.event)) return this.onWatch(e);
      if (e.event === "building" || e.event === "preparing") return this.sink.preparing?.(e.on);
      if (e.event === "switched") {
        this.tr("core: the SSA worker took over", {});
        this.glyphCache.clear();
        if (this.opened) this.chain = this.chain.then(() => this.layout()).then(() => this.statusSoon());
        return;
      }
      if (e.event === "settled") {
        if (this.opened && !this.busy) this.chain = this.chain.then(() => this.layout()).then(() => this.queuedLatency(e)).then(() => this.statusSoon());
        return;
      }
      if (e.event === "pack") return this.onPack(e);
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
        if (r.op === "edit") {
          const len = new TextEncoder().encode(r.text).length;
          this.mapCulprits(r.file, r.start, r.end, len);
          this.mapGlyphs(r.file, r.start, r.end, len);
        }
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
        if (lost && this.opened && /needs more than the [\d.]+ GiB of memory/.test(res.error ?? "")) {
          this.opened = false;
          this.sink.error(res.error);
          this.sink.connection?.(false, res.error);
          return res;
        }
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
    const rt = this.now() - t, said2 = (ms) => ms < 1e3 ? `${ms.toFixed(0)} ms` : `${(ms / 1e3).toFixed(1)} s`;
    this.sink.latency(`opened: the engine answered in ${said2(rt)}${r.json.build_ms >= 1 ? `, its build ${said2(r.json.build_ms)}` : ""}`);
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
  /**
   * boxesOf's answers, by page (its glyph list: a page changed has a new
   * one) and range (`file\0start\0end`): the range's boxes there and the
   * page's glyph nearest it. Mapped through edits as the glyphs are, so a
   * build's problems are looked up only on the pages it changed.
   */
  culprits = /* @__PURE__ */ new Map();
  /** The culprits' answers moved with an edit (before the glyphs are): one the edit touches, between its range and its nearest glyph, dropped. */
  mapCulprits(file, start, end, len) {
    const live = new Set(this.glyphCache.values());
    for (const [all, memo] of this.culprits) {
      if (!live.has(all)) {
        this.culprits.delete(all);
        continue;
      }
      const moved = [];
      for (const [id, m] of memo) {
        const [f, a, b] = id.split("\0");
        if (f !== file) continue;
        const lo = Math.min(+a, m.g?.start ?? +a), hi = Math.max(+b, m.g?.end ?? +b);
        if (start > hi) continue;
        memo.delete(id);
        if (end < lo) moved.push([`${f}\0${+a + len - (end - start)}\0${+b + len - (end - start)}`, m]);
      }
      for (const [id, m] of moved) memo.set(id, m);
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
      const mine = this.glyphsFetch(k, key).finally(() => this.glyphsAsked.get(key) === mine && this.glyphsAsked.delete(key));
      this.glyphsAsked.set(key, p = mine);
    }
    return p;
  }
  glyphsAsked = /* @__PURE__ */ new Map();
  /** The watch's `origins` events seen: a page's answer asked before the latest is of the origins before it. */
  originsSeen = 0;
  async glyphsFetch(k, key) {
    const seen = this.originsSeen;
    const r = await this.core.request({ op: "origins", page: k });
    if (seen !== this.originsSeen) return this.glyphsOf(k);
    if (!r.ok || !r.json) return [];
    const path = (n) => {
      n = n.replace(/^\.\//, "");
      if (this.o.remote) return n;
      return n in this.files ? n : n + ".tex" in this.files ? n + ".tex" : null;
    };
    const gs = glyphs2(r.json, path);
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
    const { at, word, line } = place3(this.files[file], pos);
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
  /**
   * The collaborators' marks, from the glyphs already here. A collaborator
   * not found whose last mark is on a page made again, its glyphs not here
   * yet (a build: the page's new hash), keeps that mark until they come
   * ([`peerPages`] asks for them): dropped and drawn again a round trip
   * later, it blinked at every build.
   */
  drawPeers() {
    if (!this.sink.others || !this.opened) return;
    const out = [];
    const drawn = /* @__PURE__ */ new Map();
    for (const p of this.peers) {
      const t = this.files[p.file];
      if (t === void 0) continue;
      const id = `${p.label}\0${p.hue}`;
      const { at, word, line } = place3(t, p.pos);
      let e;
      for (let k = 0; k < this.hashes.length; k++) {
        const all = this.cached(k);
        const hit = all ? this.wordOn(all, p.file, at, word, line) : [];
        if (!all || !hit.length) continue;
        e = { k, boxes: boxes(hit, all), label: p.label, hue: p.hue };
        break;
      }
      const was = this.peersDrawn.get(id);
      if (!e && was && was.k < this.hashes.length && !this.cached(was.k)) e = was;
      if (!e) continue;
      out.push(e);
      drawn.set(id, e);
    }
    this.peersDrawn = drawn;
    this.sink.others(out);
  }
  /** The collaborators' marks last drawn, by label and hue. */
  peersDrawn = /* @__PURE__ */ new Map();
  /** The glyphs of the pages around the one in view, and of those a collaborator's kept mark is on, asked for, then the collaborators drawn again. */
  async peerPages() {
    if (!this.peers.length || !this.sink.others) return;
    const near = [this.page, this.page + 1, this.page - 1, ...[...this.peersDrawn.values()].map((e) => e.k)];
    const ks = [...new Set(near)].filter((k) => k >= 0 && k < this.hashes.length && !this.cached(k));
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
    return word.length ? word : lineAt2(onLine, at);
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
    file = file.replace(/^\.\//, "");
    const n = Math.max(this.hashes.length, this.pages, 1);
    const order = [this.page, ...Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.page)];
    const out = [];
    let near = null;
    for (const k of order) {
      if (k >= this.hashes.length) continue;
      const all = this.cached(k) ?? await this.glyphsOf(k);
      const memo = this.culprits.get(all) ?? this.culprits.set(all, /* @__PURE__ */ new Map()).get(all);
      const id = `${file}\0${start}\0${end}`;
      let m = memo.get(id);
      if (!m) {
        const hi = Math.max(end, start + 1), hit = [];
        m = { boxes: null, g: null, d: Infinity };
        for (const g of all) {
          if (g.file !== file || g.synth) continue;
          if (g.start < hi && g.end > start) hit.push(g);
          const d = g.start >= end ? g.start - end : start - g.end;
          if (d >= 0 && d < m.d) m.g = g, m.d = d;
        }
        if (hit.length) m.boxes = boxes(hit, all);
        memo.set(id, m);
      }
      if (m.boxes) out.push({ k, boxes: m.boxes });
      if (m.g && (!near || m.d < near.d)) near = { k, g: m.g, d: m.d, all };
    }
    if (!out.length && near && near.d <= NEAR_CULPRIT) {
      const mine = near.all.filter((x) => x.file === file && !x.synth).sort((x, y) => x.start - y.start);
      let i = mine.indexOf(near.g), j = i;
      while (i > 0 && mine[i - 1].end >= mine[i].start) i--;
      while (j + 1 < mine.length && mine[j].end >= mine[j + 1].start) j++;
      out.push({ k: near.k, boxes: boxes(mine.slice(i, j + 1), near.all) });
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
      const hit = lineAt2(from(all, file, a, b), byteOffset(t, pos));
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
  /** Waiting for a queued edit's build to end (`send`): its `settled`, else `failed` or `trapped`. */
  ended = [];
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
        void this.peerPages();
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
        if (!e.settling) {
          this.watchBusy(false);
          this.sink.preparing?.(false);
          if (this.pkg.building) this.pkg.building = false, this.tellPackages();
        }
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
      const hit = lineAt2(from(all, file, lo, hi), at);
      if (hit.length) return this.sink.mark?.(k, boxes(hit, all), true);
    }
  }
  /** The last keystroke a core queued (its build said by `settled`): when it was typed, sent, answered. */
  queued = null;
  /** A queued keystroke's build in and drawn: its time from the keystroke, and the build's own, said. */
  queuedLatency(e) {
    const q = this.queued;
    if (!q || e.ms === void 0) return;
    this.queued = null;
    const e2e = this.now() - q.t0;
    this.sink.painted?.(e2e);
    const ran = e.ran ?? 0, kept = e.kept ?? 0, passes = e.passes ?? 1;
    this.sink.latency(
      `${e2e.toFixed(0)} ms keystroke\u2192page \xB7 build ${e.ms.toFixed(1)} ms \xB7 ${this.keystrokes} edits \u2192 ${this.builds} builds`,
      [
        `build            ${e.ms.toFixed(2)} ms (in the core: the edit's steps, every page, the .aux/.bbl loops)`,
        `keystroke\u2192page   ${e2e.toFixed(1)} ms = queued ${(q.tSend - q.t0).toFixed(1)} + round trip ${(q.tBack - q.tSend).toFixed(1)} + build and paint ${(this.now() - q.tBack).toFixed(1)}`,
        `rebuilt          ${ran} of ${ran + kept} steps, ${passes} pass${passes === 1 ? "" : "es"}`,
        `batching         ${this.keystrokes} edits \u2192 ${this.builds} builds (one in flight; the rest merged)`
      ].join("\n")
    );
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
      let done;
      const ended = last ? new Promise((res) => (done = res, this.ended.push(res))) : null;
      const unheard = () => this.ended = this.ended.filter((f) => f !== done);
      const r = await this.core.request({ op: "edit", file, ...e, page: last ? this.page : -1, dpi: this.dpi() });
      if (!r.ok) {
        unheard();
        this.sink.error((typeof r.json?.error === "string" ? r.json.error : void 0) ?? r.error ?? "edit failed");
        if (r.error?.includes("trapped")) await this.reopen();
        return false;
      }
      if (!last) continue;
      if (!r.json?.queued) unheard();
      const tBack = this.now();
      const j = r.json;
      if (j.queued) {
        this.queued = { t0, tSend, tBack };
        this.setPages(j.pages);
        const end = await ended;
        if (end.event === "settled") await this.layout(), this.queuedLatency(end);
        continue;
      }
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

// viewer/src/strips.ts
var BUDGET = 16e6;
var BASE = 4e6;
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
  /** Each page's size in points, as its last frame gave it. */
  sizes = /* @__PURE__ */ new Map();
  /**
   * Page `k` at `scale` pixels a point: its element (canvas and text),
   * size and links. `view`: the part of it on screen (the viewer's
   * `view(k)`); zoomed past `BUDGET` pixels a page, the page drawn small
   * and stretched and the part on screen (and as much again around it)
   * drawn at `scale` over it, its element's `data-win` that part.
   */
  async page(k, scale, view) {
    const size = this.sizes.get(k) ?? this.sizes.values().next().value;
    if (view && !size) await this.frame(k, Math.min(scale, 1));
    const sz = this.sizes.get(k) ?? size;
    const px = sz ? sz.w * sz.h * scale * scale : 0;
    if (!view || !sz || px <= BUDGET) {
      const f = await this.frame(k, sz && px > BUDGET ? Math.sqrt(BUDGET / (sz.w * sz.h)) : scale);
      if (!f) return null;
      const el2 = document.createElement("div");
      el2.className = "phx-page";
      el2.append(this.canvas(f), textLayer(f.text.map(composeLine), f.w, f.h));
      return { el: el2, w: f.w, h: f.h, links: f.links };
    }
    const grow = (a, b2, most2) => {
      const m = (a + b2) / 2, half = Math.min((b2 - a) * 2, most2) / 2;
      return [Math.max(0, m - half), Math.min(1, m + half)];
    };
    const most = Math.sqrt(BUDGET / px);
    const [l, r] = grow(view[0], view[2], most);
    const [t, b] = grow(view[1], view[3], most);
    const win = [l, t, r, b];
    const [base, part3] = await Promise.all([this.frame(k, Math.min(scale, Math.sqrt(BASE / (sz.w * sz.h)))), this.frame(k, scale, win)]);
    if (!base || !part3?.win) return null;
    const el = document.createElement("div");
    el.className = "phx-page";
    el.dataset.win = win.join(",");
    const tile = this.canvas(part3);
    const [x, y, w, h] = part3.win;
    tile.className = "phx-tile";
    Object.assign(tile.style, {
      position: "absolute",
      left: `${x / part3.pw * 100}%`,
      top: `${y / part3.ph * 100}%`,
      width: `${w / part3.pw * 100}%`,
      height: `${h / part3.ph * 100}%`
    });
    el.append(this.canvas(base), tile, textLayer(base.text.map(composeLine), base.w, base.h));
    return { el, w: base.w, h: base.h, links: base.links };
  }
  /** Page `k`'s frame at `scale` (only `win` of it, if given), its strips fetched and kept. */
  async frame(k, scale, win) {
    const t0 = performance.now();
    let f = null;
    let t1 = t0;
    if (this.src.frameWith) {
      const have = (this.shown.get(k) ?? []).filter((key) => this.kept.has(key));
      const buf = await this.src.frameWith(k, scale, have, win);
      if (!buf || buf.byteLength < 4) return null;
      const n = new DataView(buf).getUint32(0, true);
      f = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, n)));
      t1 = performance.now();
      await this.take(f.sent ?? [], buf, 4 + n);
    } else {
      f = await this.src.frame(k, scale, win);
      if (!f) return null;
      t1 = performance.now();
    }
    if (!f) return null;
    this.sizes.set(k, { w: f.w, h: f.h });
    if (!win) this.shown.set(k, f.bands.map((b) => b[0]));
    const want = [...new Set(f.bands.map((b) => b[0]).filter((key) => !this.kept.has(key)))];
    if (want.length) await this.fetch(want);
    const t2 = performance.now();
    if (this.chip)
      this.chip.dataset.frame = JSON.stringify({ page: k + 1, inView: !!win, drawn: f.drawn, kept: f.kept, probe: f.us.probe / 1e3, draw: f.us.draw / 1e3, watch: f.us.all / 1e3, request: t1 - t0, decode: t2 - t1 });
    if (this.chip)
      this.chip.textContent = `p${k + 1}${win ? " (in view)" : ""}: ${f.drawn} drawn, ${f.kept} kept \xB7 watch ${(f.us.all / 1e3).toFixed(1)} ms (probe ${(f.us.probe / 1e3).toFixed(1)}, draw ${(f.us.draw / 1e3).toFixed(1)}) \xB7 fetch ${(t1 - t0).toFixed(1)}+${(t2 - t1).toFixed(1)} ms`;
    return f;
  }
  /** `f` composed: a canvas of its window (the page, but for a window's frame), white under its strips. */
  canvas(f) {
    const [x, y, w, h] = f.win ?? [0, 0, f.pw, f.ph];
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    const g = c.getContext("2d");
    g.fillStyle = "#fff";
    g.fillRect(0, 0, w, h);
    for (const [key, top, , left] of f.bands) {
      const b = this.kept.get(key);
      if (b) g.drawImage(b, (left ?? x) - x, top - y);
    }
    return c;
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
        const px = pagewise(new Uint8ClampedArray(buf, at, w * h * 4));
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
function pagewise(px) {
  const clone = globalThis.cloneInto;
  return clone ? clone(px, window) : px;
}
var SVG = "http://www.w3.org/2000/svg";
function textLayer(lines2, w, h) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "phx-text");
  svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  svg.setAttribute("preserveAspectRatio", "none");
  for (const l of lines2) {
    const t = document.createElementNS(SVG, "text");
    t.setAttribute("x", l.xs ? l.xs.map((x) => x.toFixed(2)).join(" ") : String(l.x));
    t.setAttribute("y", String(l.y));
    t.setAttribute("font-size", String(l.size));
    t.textContent = l.text;
    LAYER_LINES.set(t, l);
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
var esc5 = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
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
var KINDS2 = /* @__PURE__ */ new Set(["head", "branch", "remote", "tag"]);
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
    if (this.error) return `<div class="note err">${esc5(this.error)}</div>`;
    if (!this.commits.length) return `<div class="note">${this.loading ? "reading the history\u2026" : "no commits"}</div>`;
    const { rows, width } = layout(this.commits);
    const W2 = (width + 1) * LANE;
    const x = (l) => LANE / 2 + l * LANE + 2;
    const fi = this.find(this.from);
    const ti = this.to ? this.find(this.to) : -1;
    const tree = this.from !== null && !this.to;
    const top = `<div class="row${tree ? " to" : ""}" title="Shift-click: diff the working tree"><svg width="${W2}" height="${ROW}"><circle cx="${x(0)}" cy="${ROW / 2}" r="4" fill="none" stroke="#888" stroke-dasharray="2 2"/><line x1="${x(0)}" y1="${ROW / 2 + 4}" x2="${x(0)}" y2="${ROW * 1.5}" stroke="#888" stroke-dasharray="2 2"/></svg><span class="txt">${tree ? '<span class="end">TO</span>' : ""}<i>the working tree</i></span></div>`;
    const lines2 = this.commits.map((c, i) => {
      const r = rows[i];
      const col = COLORS[r.lane % COLORS.length];
      const svg = r.down.map(([a, b]) => `<path d="M${x(a)} ${ROW / 2} C${x(a)} ${ROW} ${x(b)} ${ROW} ${x(b)} ${ROW * 1.5}" stroke="${COLORS[b % COLORS.length]}" fill="none" stroke-width="1.6"/>`);
      svg.push(c.parents.length > 1 ? `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4.5" fill="Canvas" stroke="${col}" stroke-width="2"/>` : `<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4" fill="${col}"/>`);
      const pills = c.refs.map((f) => `<span class="pill ${KINDS2.has(f.kind) ? f.kind : "branch"}">${esc5(f.name)}</span>`).join("");
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? '<span class="end">FROM</span>' : i === ti ? '<span class="end">TO</span>' : "";
      const title = `${c.subject}
${c.author}, ${new Date(c.time * 1e3).toLocaleString()}
click: From \xB7 shift-click: To \xB7 double-click: against its first parent`;
      return `<div class="row${cls}" data-i="${i}" title="${esc5(title)}"><svg width="${W2}" height="${ROW}">${svg.join("")}</svg><span class="txt">${end}<span class="sha">${esc5(c.short)}</span>${pills}${c.parents.length > 1 ? "<i>merge:</i> " : ""}${esc5(c.subject)}<span class="who">${esc5(c.author)}, ${ago(c.time)}</span></span></div>`;
    });
    const more = this.more ? `<div class="note"><button type="button" data-more>${this.loading ? "reading\u2026" : "more\u2026"}</button></div>` : "";
    return top + lines2.join("") + more;
  }
};

// crates/cli/viewer/cli.ts
var Socket = class {
  ws;
  /** The wait before the next try (doubled to 3 s while the watch is away: a watch started again on this document serves this same address, and the tab joins it within that). */
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
    this.wait = Math.min(this.wait * 2, 3e3);
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
var dur2 = (ms) => ms < 1e3 ? `${Math.round(ms)} ms` : `${(ms / 1e3).toFixed(ms < 1e4 ? 1 : 0)} s`;
function timing(s) {
  const ms = s.ms ?? 0;
  const parts = [
    [s.cold ? "first build" : "build", `${dur2(ms)}${s.cold ? " (the format, the packages, the whole document)" : ""}`],
    ["save to build", typeof s.since_ms === "number" ? dur2(s.since_ms) : null],
    ["LaTeX runs", s.passes === void 0 ? null : String(s.passes)],
    ["steps", s.ran === void 0 ? null : `${s.ran} run, ${s.kept ?? 0} kept from the last build`],
    ["packages fetched", s.fetched ? String(s.fetched) : null],
    ["threads", s.threads === void 0 ? null : String(s.threads)]
  ];
  return [
    s.cold ? `First build in ${dur2(ms)}` : `Built in ${dur2(ms)}`,
    parts.filter((p) => p[1] !== null).map(([k, v]) => `${k.padEnd(17)}  ${v}`).join("\n")
  ];
}
var lastStats;
function statsIn(s) {
  lastStats = s;
  const ph = s.phases_ms ?? {};
  const names = [["typesetting", "Typesetting"], ["output", "Writing the PDF"], ["writing files", "Writing files"]];
  const known2 = /* @__PURE__ */ new Set([...names.map(([k]) => k), "viewer"]);
  const parts = [...names.filter(([k]) => typeof ph[k] === "number").map(([k, n]) => [n, ph[k]]), ...Object.entries(ph).filter(([k]) => !known2.has(k))];
  panel.buildParts(parts, typeof ph.viewer === "number" ? [["pages for the viewer", ph.viewer]] : []);
}
function statsLines(s) {
  const n = s.nodes ?? {};
  const size = (b) => b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`;
  const pc = s.pages_changed;
  const changed = pc === void 0 ? "?" : pc.length && pc.length <= 8 ? `${pc.length} (${pc.join(", ")})` : String(pc.length);
  return [
    `PhiTeX ${s.version ?? "?"} \xB7 ${s.engine ?? "?"} \xB7 files from ${s.files === "shelf" ? `Shelf ${s.files_release ?? "?"}` : s.files === "texlive" ? "TeX Live" : "?"}`,
    `build ${s.build ?? "?"} (${s.why ?? "?"}) ${s.ok === false ? "failed" : "ok"} in ${dur2(s.wall_ms ?? 0)}: ${Object.entries(s.phases_ms ?? {}).map(([k, ms]) => `${k} ${dur2(ms)}`).join(" \xB7 ")}`,
    `nodes ${n.ran ?? "?"} ran, ${n.kept ?? "?"} kept of ${n.live ?? "?"} \xB7 ${s.runs ?? "?"} runs, ${s.rounds ?? "?"} rounds \xB7 pages ${changed} of ${s.pages ?? "?"} changed`,
    `${s.cores_busy == null ? `threads ${s.threads ?? "?"}` : `parallel ${s.cores_busy.toFixed(1)} of ${s.threads ?? "?"} threads busy`} \xB7 busy ${dur2(s.busy_ms ?? 0)}, engine ${dur2(s.engine_ms ?? 0)}, wasted ${dur2(s.wasted_ms ?? 0)}`,
    `memory ${s.resident_kb === void 0 ? "?" : size(s.resident_kb * 1024)} resident, values ${s.values_bytes === void 0 ? "?" : size(s.values_bytes)}`,
    `hints ${Object.entries(s.hints ?? {}).map(([k, v]) => `${k} ${v}`).join(", ") || "\u2014"}`
  ];
}
var Pages = class {
  sock;
  /** A change on its way to the screen: ms from its save to `settled`, and when that came. */
  change = null;
  /** Every page's text, for finding (asked again after each build). */
  texts = null;
  constructor(sock2) {
    this.sock = sock2;
    sock2.onEvent((e) => {
      if (e.event === "stats") return void statsIn(e);
      if (e.event !== "settled") return;
      this.texts = null;
      const s = e;
      const since = typeof s.since_ms === "number" ? s.since_ms : s.cold && typeof s.ms === "number" ? s.ms : null;
      this.change = since === null ? null : { since, at: performance.now() };
      if (typeof s.ms === "number") panel.latency(...timing(s));
    });
  }
  async request(op) {
    const o = op;
    if (o.op === "png") {
      const p = await strips.page(o.page ?? 0, panel.pixelsPerPoint(), panel.pageView(o.page ?? 0)).catch(() => null);
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
      (r) => r.ok ? (r.json?.pages ?? []).map((lines2) => lines2.map(composeLine)) : []
    );
    return search(await this.texts, q);
  }
  onEvent(cb) {
    this.sock.onEvent((e) => {
      if (e.event === "follow" && follow !== "cursor") return;
      if (e.event === "select" && follow === "dblclick") return;
      cb(e);
    });
  }
};
var stripSource = {
  async frame(k, scale, win) {
    const part3 = win ? `/${win.map((x) => x.toFixed(5)).join(",")}` : "";
    const r = await fetch(new URL(`frame/${k}/${Math.round(scale * 1e3)}${part3}`, location.href), { cache: "no-store" });
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
async function clearCache() {
  const mb2 = (b) => `${(b / 1e6).toFixed(b < 1e7 ? 1 : 0)} MB`;
  const now = await sock.request({ op: "cache" });
  if (!now.ok) return void panel.msg(`Couldn't clear the cache: ${now.error ?? "not available"}`, true, true);
  const bytes = Number(now.json?.bytes ?? 0);
  if (!confirm(`Clear PhiTeX's cache (${mb2(bytes)} in ${now.json?.dir ?? "its cache folder"})? Packages and formats are fetched again when a build needs them.`)) return;
  const r = await sock.request({ op: "cache", clear: true });
  panel.msg(r.ok ? `Cache cleared: ${mb2(Number(r.json?.freed ?? 0))} freed` : `Couldn't clear the cache: ${r.error ?? "not cleared"}`, !r.ok, true);
}
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
    // (Clean rebuild: the watch drops its build and makes it again from nothing, its format kept)
    onClean: () => void sock.request({ op: "cold" }),
    // (the watch builds again from the files on disk, as `r` in its terminal)
    onRebuild: () => void sock.request({ op: "rebuild" }),
    onCompare: () => openPick(),
    onLog: () => session.log(),
    onBoxes: (file, start, end) => session.boxesOf(file, start, end),
    onClearCache: () => void clearCache(),
    onGoto: (file, line) => source(file, line),
    onSyncSource: (k, x, y) => void session.toSource(k, x, y),
    onContents: () => void session.outline(),
    onFind: (q) => pages.find(q),
    // (Settings: the watch's rows, from and into the user's phitex.toml)
    settings: () => watchSettings(),
    // (the panel's own choices: the user's phitex.toml too, never this browser's)
    onPref: async (key, value) => {
      const r = await sock.request({ op: "prefs", set: { [key]: value } });
      return r.ok ? void 0 : `Not saved: ${r.error ?? "the watch did not answer"}`;
    },
    // (a project file's text, for Copy for an AI assistant)
    onSource: async (file) => {
      const r = await sock.request({ op: "info", what: "source", file });
      return r.ok ? String(r.json?.text ?? "") : null;
    },
    // (What's new: this version's CHANGELOG entry, from the binary)
    onNews: () => void sock.request({ op: "info", what: "news" }).then((r) => {
      if (r.ok) panel.news(String(r.json?.version ?? ""), String(r.json?.text ?? ""));
      else panel.msg("No news for this version", false, true);
    }),
    // (the timing popover's "Copy debug info": the last build's stats, the CLI's `s` stats; before any, the version, the news op's)
    onDebugInfo: async () => {
      if (lastStats) return ["phitex watch", ...statsLines(lastStats)];
      const r = await sock.request({ op: "info", what: "news" });
      return [`PhiTeX ${r.ok ? String(r.json?.version ?? "?") : "?"} \xB7 phitex watch`];
    },
    // (the tips seen: the user's phitex.toml through the watch, the same on every port and browser)
    onTipsSeen: (ids) => void sock.request({ op: "prefs", set: { "tips-seen": ids } }),
    // (the problems' filters: kept the same way)
    onProblemsHidden: (kinds) => void sock.request({ op: "prefs", set: { "problems-hidden": kinds } })
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
  const placed = (diffState.changes ?? []).map((c, i) => typeof c.page === "number" ? { k: c.page, y: c.y ?? 0, title: `Change ${i + 1}: ${words(c.new_text || "") || words(c.old_text || "")}` } : null).filter((c) => !!c);
  panel.diffMarks(diffState.shown === false ? [] : placed);
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
var gotoChange = async (k) => {
  const c = (diffState.changes ?? [])[k - 1];
  if (!c) return;
  const q = words(c.new_text || "") || words(c.old_text || "");
  const hits = q ? await pages.find(q) : [];
  if (typeof c.page === "number") {
    const y = c.y ?? 0;
    const near = hits.filter((h) => h.k === c.page).sort((a, b) => Math.abs(a.boxes[0][1] + a.boxes[0][3] - y) - Math.abs(b.boxes[0][1] + b.boxes[0][3] - y))[0];
    return panel.reveal([near ?? { k: c.page, boxes: [[18, Math.max(0, y - 10), 14, 12]] }]);
  }
  if (!hits.length) return void panel.msg(`Change ${k} isn't on the pages`, false, true);
  panel.reveal(hits.slice(0, 1));
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
  stop: () => (panel.diffMarks([]), void ask({ op: "diff_stop" }))
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
#phx-pick { position: fixed; z-index: 20; top: 44px; right: 12px; width: min(560px, calc(100vw - 24px)); max-height: 70vh; display: flex; flex-direction: column; overflow: hidden;
  background: var(--bg); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; box-shadow: 0 6px 20px rgb(0 0 0 / 25%); }
#phx-pick[hidden] { display: none; }
/* (the title and Close on one line, the hint under them; the commits scroll in their own box below, never under the header) */
#phx-pick header { flex: none; display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 2px 10px; padding: 9px 12px; border-bottom: 1px solid var(--line); background: var(--bg); }
#phx-pick header b { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
#phx-pick header small { grid-column: 1 / -1; color: var(--fg2); font-size: 12px; }
#phx-pick .phx-commits { flex: 1; min-height: 0; }
#phx-pick header button { grid-row: 1; grid-column: 2; border: 1px solid var(--line); background: none; color: var(--fg); border-radius: 9999px; padding: 1px 10px; cursor: pointer; font: inherit; font-size: 12px; }
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
  void sock.request({ op: "info", what: "built" }).then((r) => {
    if (!r.ok || !(Number(r.json?.builds) > 0)) return;
    const st = r.json?.stats, se = r.json?.settled;
    const last = se && typeof se.ms === "number" ? { ms: se.ms, cold: !!se.cold } : void 0;
    panel.joinedLate(typeof r.json?.first_ms === "number" ? r.json.first_ms : void 0, Number(r.json.builds), last);
    if (st && typeof st === "object" && !lastStats) statsIn(st);
    if (se && typeof se.ms === "number") panel.latency(...timing(se));
  });
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
var favicon = (() => {
  const link = Object.assign(document.createElement("link"), { rel: "icon" });
  document.head.append(link);
  let was = "";
  return (colour) => {
    if (colour === was) return;
    was = colour;
    link.href = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="${colour}"/></svg>`)}`;
  };
})();
var showTitle = () => {
  const file = (status.text ?? "").split(" \xB7 ")[0] || "phitex watch";
  const connected = !/not connected/.test(status.text ?? "");
  const errs = /(\d+) errors?/.exec(status.text ?? "")?.[1];
  const [what, colour] = !connected ? ["Not connected", "#8a93a3"] : busy ? ["Building\u2026", "#f58f2d"] : status.ok === false ? [errs ? `${errs} error${errs === "1" ? "" : "s"}` : "Failed", "#d1453b"] : status.ok === true ? ["Built", "#2e9d63"] : ["Loading\u2026", "#8a93a3"];
  document.title = connected && file !== "phitex watch" ? `${file} \xB7 ${what}` : `${what} \xB7 phitex watch`;
  if (/\.(tex|ltx)$/.test(file)) panel.docName(file);
  favicon(colour);
};
var showBar = () => {
  showTitle();
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
      const [phase, time] = [parts[1], parts[2]];
      if (phase.startsWith("making")) panel.loadStep(1, `Making the format, once per PhiTeX update \xB7 ${time}`);
      else if (phase === "format") panel.loadStep(1, `Loading the format \xB7 ${time}`);
      else panel.loadStep(2, `Preparing the live preview \xB7 ${time}`);
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
  if (live && /^(✓|ok:|✗|failed:)/.test(s.text ?? "")) unseen = { errors: 0, warnings: 0 }, showBar();
  else if (live && con.hidden && /^(error|warning): /.test(line.textContent ?? "")) {
    if (line.className === "error") unseen.errors++;
    else unseen.warnings++;
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
var follow = "cursor";
var setPref = async (k, v) => {
  const r = await sock.request({ op: "prefs", set: { [k]: v } });
  return r.ok ? void 0 : r.error ?? "not saved";
};
async function watchSettings() {
  const [r, info, cache] = await Promise.all([sock.request({ op: "prefs" }), sock.request({ op: "info", what: "settings" }), sock.request({ op: "cache" })]);
  if (!r.ok) return [{ title: "Preview", rows: [{ id: "prefs", label: "Your settings", kind: "info", value: "", disabled: `Couldn't read them: ${r.error ?? "not available"}` }] }];
  const p = r.json ?? {}, at = info.ok ? info.json ?? {} : {};
  const file = String(p.file ?? "phitex.toml");
  const project = "In the project's phitex.toml (or the command line): the page never changes a project's files";
  const mb2 = (b) => b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`;
  const toggle2 = (id, label, help) => ({ id, label, help, kind: "toggle", value: !!p[id], set: (v) => setPref(id, v) });
  const cores = Number(p.cores ?? 1);
  return [
    {
      title: "Preview",
      rows: [
        { id: "w-main", label: "Main file", help: project, kind: "info", value: String(at.main ?? "") },
        { id: "w-engine", label: "Engine", help: "Set `engine` in the project's phitex.toml (or a `% !TEX program` line)", kind: "info", value: String(at.engine ?? "") },
        {
          id: "follow",
          label: "Highlight follows",
          help: "Where your editor is, on the page: as the cursor moves, what you select, or a double-click alone",
          kind: "choice",
          options: [["cursor", "Cursor"], ["selection", "Selection"], ["dblclick", "Double-click"]],
          value: String(p.follow ?? "cursor"),
          set: async (v) => await setPref("follow", v) ?? void (follow = String(v))
        }
      ]
    },
    {
      title: "Build",
      rows: [
        toggle2("watch-write", "Write the PDF into the folder", "After each build, the PDF with its .synctex.gz, as phitex build does. From the next phitex watch on."),
        toggle2("synctex", "SyncTeX file with a written PDF", "JOB.synctex.gz beside the PDF, for a PDF viewer's jumps to the source."),
        {
          id: "threads",
          label: "Threads",
          help: at.jobs ? `This watch runs with -j ${at.jobs}, over this. After a restart of phitex watch.` : "After a restart of phitex watch. The output is the same at any count.",
          kind: "select",
          options: [["0", `One per core (${cores})`], ...Array.from({ length: cores }, (_, i) => [String(i + 1), String(i + 1)])],
          value: String(p.threads ?? 0),
          set: (v) => setPref("threads", Number(v))
        },
        { id: "w-files", label: "TeX's files", help: "From TeX Live or from Shelf: `files` in your phitex.toml or the project's, or --shelf", kind: "info", value: at.files === "shelf" ? "Shelf" : "TeX Live" }
      ]
    },
    {
      title: "Packages",
      rows: [
        { id: "cache", label: "The cache", help: cache.ok ? `${mb2(Number(cache.json?.bytes ?? 0))} in ${cache.json?.dir ?? "its folder"}: packages and formats, fetched again when a build needs them` : "Its size: not available", kind: "action", button: "Clear", set: () => void clearCache() },
        {
          id: "cache-limit",
          label: "Keep at most",
          help: "Past it, the least recently used go first",
          kind: "choice",
          options: [["500MB", "500 MB"], ["1GB", "1 GB"], ["4GB", "4 GB"], ["16GB", "16 GB"]],
          value: { 5e8: "500MB", 1e9: "1GB", 4e9: "4GB", 16e9: "16GB" }[Number(p["cache-limit"] ?? 4e9)] ?? "",
          set: (v) => setPref("cache-limit", v)
        }
      ]
    },
    { title: "Updates and tips", rows: [toggle2("update-check", "Say when a newer PhiTeX is out", "Asked once a day, in the terminal.")] },
    { title: "Where", rows: [{ id: "w-file", label: "Your settings", help: "Every document and port: `phitex config` lists them all", kind: "info", value: file }] }
  ];
}
var prefs = async () => {
  const r = await sock.request({ op: "prefs" });
  if (!r.ok) return;
  panel.tipsSeen((r.json?.["tips-seen"] ?? []).filter((x) => typeof x === "string"));
  panel.problemsHidden((r.json?.["problems-hidden"] ?? []).filter((x) => typeof x === "string"));
  panel.viewPrefs({ "dark-pdf": r.json?.["dark-pdf"], opening: r.json?.opening, underline: r.json?.underline !== false });
  follow = String(r.json?.follow ?? "cursor");
};
var opened = sock.onOpen;
sock.onOpen = () => (opened(), void backlog(), void prefs());
showBar();
