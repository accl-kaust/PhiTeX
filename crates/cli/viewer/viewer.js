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
function diagnose(files, main, build2) {
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
    const latex = build2?.latex ? [] : [...new Set(text.match(LATEX) ?? [])];
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
    if (nonAscii && !build2?.latex)
      out.push({ severity: "info", code: "non-ascii", message: `${nonAscii} non-ASCII characters: PhiTeX's fonts may not typeset them`, file, line: lineAt(text, firstNon) });
  }
  if (main && files[main] !== void 0 && !/\\(bye|end|enddocument)\b/.test(files[main]))
    out.push({ severity: "info", code: "no-end", message: "no \\bye: the last page ships at the end of the file anyway", file: main });
  for (const f of build2?.packageErrors ?? [])
    out.push({ severity: "error", code: "package-download", message: `couldn't download ${f.name}: ${f.error}` });
  if (build2?.texError) {
    const e = build2.texError;
    const stuck = e.missing && build2.unavailable?.includes(e.missing);
    const failed = e.missing && build2.packageErrors?.some((f) => f.name === e.missing);
    out.push({
      severity: "error",
      code: e.missing ? "tex-missing-file" : "tex-error",
      message: e.missing ? `${e.missing} not found${stuck ? ` in the project or ${build2.packageSource ?? "the package source"}` : failed ? " (its download failed, above)" : " (still downloading?)"}: the build stopped here` : e.message,
      file: e.file ?? void 0,
      line: e.line ?? void 0,
      detail: e.context
    });
  }
  if (build2) {
    if (build2.pending)
      out.push({ severity: "warning", code: "pending", message: `${build2.pending} part${build2.pending === 1 ? "" : "s"} not read: out of fuel (a loop?) or unsupported` });
    if (build2.undefinedNames?.length) out.push({ severity: "warning", code: "undefined", message: `undefined: ${build2.undefinedNames.join(" ")}` });
    const optional = [...new Set(build2.unavailable?.filter((n) => n !== build2.texError?.missing).map((n) => n.replace(/(\.[a-z]+)\.tex$/, "$1")))];
    if (optional.length) {
      const names = optional.slice(0, 6).join(" ") + (optional.length > 6 ? " \u2026" : "");
      out.push({
        severity: "info",
        code: "package-unavailable",
        message: build2.packageSource ? `looked for and not found in ${build2.packageSource} (usually optional): ${names}` : `packages are not downloaded yet: ${names}`
      });
    }
    if (build2.pages === 0) out.push({ severity: "warning", code: "no-pages", message: "no page shipped yet" });
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

// viewer/src/html.ts
var MARK = "phx-slot-";
var parsed = /* @__PURE__ */ new WeakMap();
var parsedSvg = /* @__PURE__ */ new WeakMap();
function html(strings, ...values) {
  return build(strings, values, false);
}
function svg(strings, ...values) {
  return build(strings, values, true);
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
function rich(text) {
  const f = document.createDocumentFragment();
  const open = [f];
  for (const part3 of text.split(/(<\/?(?:b|code|i|kbd)>)/)) {
    const tag = /^<(\/?)(b|code|i|kbd)>$/.exec(part3);
    if (!tag) part3 && open[open.length - 1].appendChild(document.createTextNode(part3));
    else if (!tag[1]) open.push(open[open.length - 1].appendChild(document.createElement(tag[2])));
    else if (open.length > 1) open.pop();
  }
  return f;
}
function svgArt(markup) {
  if (/<script|\son[a-z]+\s*=/i.test(markup)) throw new Error("svgArt: art has no script or event handler");
  const doc = new DOMParser().parseFromString(`<svg xmlns="http://www.w3.org/2000/svg">${markup}</svg>`, "image/svg+xml");
  if (doc.querySelector("parsererror")) throw new Error("svgArt: not well-formed SVG");
  const f = document.createDocumentFragment();
  f.append(...[...doc.documentElement.childNodes].map((n) => document.importNode(n, true)));
  return f;
}

// viewer/src/problems.ts
function place(file, line, col, text) {
  return html`<a href="#" class="phx-place" data-file="${file}" data-line="${Number(line) || 1}" data-col="${Number(col) || 1}">${text ?? `${file}${line ? `:${line}` : ""}${col ? `:${col}` : ""}`}</a>`;
}
function problemNode(p) {
  return item(p);
}
function asDiagnostic(p) {
  p = Object.assign({ notes: [], help: [], suggestions: [], file: null, line: null, col: null, excerpt: null, context: [], included: [], box: null }, p);
  const severity = p.severity === "fatal" || p.severity === "error" ? "error" : p.severity === "warning" ? "warning" : "info";
  return { severity, code: p.code, message: p.message, file: p.file ?? void 0, line: p.line ?? void 0, problem: p };
}
function item(p) {
  const sev = p.severity === "fatal" ? "error" : p.severity === "warning" || p.severity === "note" ? p.severity : "error";
  let ex = "";
  if (p.excerpt) {
    const chars = [...p.excerpt.text];
    const a = chars.slice(0, p.excerpt.start).join("");
    const b = chars.slice(p.excerpt.start, p.excerpt.start + p.excerpt.len).join("");
    const c = chars.slice(p.excerpt.start + p.excerpt.len).join("");
    ex = html`<pre class="phx-excerpt">${p.line ? html`<span class="phx-ln">${Number(p.line)} │ </span>` : ""}${a}<mark>${b}</mark>${c}</pre>`;
  }
  const ctx = p.context.length ? html`<details class="phx-ctx"><summary>in ${p.context.map((m, i) => [i ? " \u2190 " : "", html`<code>${m.name}</code>`])}</summary>${p.context.map((m) => html`<pre><b>${m.name}</b> ${m.before}<mark>‸</mark>${m.after}</pre>`)}</details>` : "";
  const inc = p.included.length ? html`<div class="phx-inc">included from ${p.included.map((i, k) => [k ? ", " : "", place(i.file, i.line, 1)])}</div>` : "";
  const box = p.box ? html`<pre class="phx-excerpt">${p.box.excerpt}</pre>` : "";
  const list = (cls, xs) => xs.length ? html`<ul class="${cls}">${xs.map((x) => html`<li>${x}</li>`)}</ul>` : "";
  return html`<li class="phx-${sev}"><div class="phx-msg"><span class="phx-sev">${String(p.severity)}</span>${p.code ? html`<code class="phx-code">${p.code}</code>` : ""}<span>${p.message}</span></div>${p.file ? html`<div class="phx-where">${place(p.file, p.line, p.col)}</div>` : ""}${ex}${box}${list("phx-notes", p.notes)}${ctx}${inc}${list("phx-help", [...p.help.length ? [p.help.join(" ").replace(/\s+/g, " ").trim()] : [], ...p.suggestions])}</li>`;
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
    const list = (es, path, depth) => html`<ul>${es.map((e) => {
      const i = this.flat.length;
      this.flat.push({ e, row: null });
      const p = `${path}/${e.t}`;
      const kids = e.k.length ? list(e.k, p, depth + 1) : "";
      const cls = [e.k.length ? "parent" : "", this.folded.has(p) ? "folded" : ""].filter(Boolean).join(" ");
      return html`<li data-i="${i}" data-path="${p}" class="${cls}"><div class="phx-row" style="--d:${depth}">${e.k.length ? html`<span class="phx-fold" aria-hidden="true"></span>` : html`<span class="phx-dot"></span>`}<a href="#" class="phx-t" title="${e.t}">${e.t}</a>${e.p != null ? html`<span class="phx-p">${Number(e.p) + 1}</span>` : ""}</div>${kids}</li>`;
    })}</ul>`;
    this.el.replaceChildren(list(items, "", 0));
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
  const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = pairs.length ? new RegExp(pairs.map(([p]) => `(?<![A-Za-z0-9_])${esc(p)}(?![A-Za-z0-9_])`).join("|"), "g") : null;
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
var MARK2 = { "body": '<defs><clipPath id="\xA70" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA71" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA72" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA73" ><rect x="-500" y="-1000" width="2000" height="1089"/></clipPath><clipPath id="\xA74" ><rect x="43.63" y="-870.22" width="35.34" height="1910.45" /></clipPath><mask id="\xA75" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA74)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA76" ><rect x="20.7" y="44.88" width="22.93" height="32.48" /></clipPath><mask id="\xA77" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA76)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA78" ><rect x="27.96" y="-10.52" width="51.01" height="85.97" /></clipPath><mask id="\xA79" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA78)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><clipPath id="\xA710" ><rect x="12.1" y="56.34" width="13.37" height="19.1" /></clipPath><mask id="\xA711" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g clip-path="url(#\xA710)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#000" stroke="#000" stroke-width="34.55" stroke-linejoin="miter" stroke-miterlimit="4"/></g></mask><mask id="\xA712" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="43.63" y="-870.22" width="35.34" height="1910.45" fill="#000"/></mask><mask id="\xA713" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA712)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#000" stroke-width="8.58"/></g></mask><mask id="\xA714" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="20.7" y="44.88" width="22.93" height="32.48" fill="#000"/></mask><mask id="\xA715" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA714)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#000" stroke-width="7.37"/></g></mask><mask id="\xA716" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="27.96" y="-10.52" width="51.01" height="85.97" fill="#000"/></mask><mask id="\xA717" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA716)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#000" stroke-width="6.16"/></g></mask><mask id="\xA718" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><rect x="12.1" y="56.34" width="13.37" height="19.1" fill="#000"/></mask><mask id="\xA719" maskUnits="userSpaceOnUse" x="-500" y="-500" width="2000" height="1500"><rect x="-500" y="-500" width="2000" height="1500" fill="#fff"/><g mask="url(#\xA718)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#000" stroke-width="4.95"/></g></mask></defs><g mask="url(#\xA75)"><path class="phx-s0" pathLength="1" clip-path="url(#\xA70)" d="M21.2,89 L46.41,54.3 A6.64,6.64 0 0 1 51.78,51.56 L71,51.56" fill="none" stroke="#f58f2d" stroke-width="5.28"/></g><g mask="url(#\xA77)"><path class="phx-s1" pathLength="1" clip-path="url(#\xA71)" d="M13.38,89 L41.29,50.58 A12.97,12.97 0 0 1 51.78,45.24 L71,45.24" fill="none" stroke="#f0b225" stroke-width="4.07"/></g><g mask="url(#\xA79)"><path class="phx-s2" pathLength="1" clip-path="url(#\xA72)" d="M7.06,89 L37.16,47.58 A18.08,18.08 0 0 1 51.78,40.12 L71,40.12" fill="none" stroke="#d3c72a" stroke-width="2.86"/></g><g mask="url(#\xA711)"><path class="phx-s3" pathLength="1" clip-path="url(#\xA73)" d="M2.23,89 L34,45.28 A21.98,21.98 0 0 1 51.78,36.22 L71,36.22" fill="none" stroke="#00a8a4" stroke-width="1.65"/></g><g class="phx-l" data-ch="\u03A6"><g mask="url(#\xA719)"><g mask="url(#\xA717)"><g mask="url(#\xA715)"><g mask="url(#\xA713)"><path d="M80 -344Q80 -424 162 -473.5Q244 -523 375 -533V-638H240V-686Q345 -683 451 -683Q518 -683 642 -686V-638H506V-534Q634 -528 723 -476.5Q812 -425 812 -343Q812 -264 725 -211Q638 -158 506 -152V-48H642V0Q537 -3 432 -3Q364 -3 240 0V-48H375V-153Q231 -164 155.5 -218Q80 -272 80 -344ZM228 -344Q228 -316 233 -294Q238 -272 252.5 -249Q267 -226 298 -210.5Q329 -195 375 -190V-495Q288 -485 258 -444Q228 -403 228 -344ZM506 -189Q584 -198 624 -231.5Q664 -265 664 -341Q664 -411 630.5 -448.5Q597 -486 507 -497L506 -453Z" transform="translate(-7 85) scale(0.09552)" fill="#FILL"/></g></g></g></g></g>', "x0": 0, "y0": 20, "x1": 71, "y1": 89 };
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
var svgOf = (markup) => svgArt(markup).firstElementChild;
function byline(svg2, word) {
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
  svg2.append(g);
  let x = x0;
  g.append(svg`<text x="${f2(x)}" y="${f2(mid + 0.19 * h)}" fill="${BY.faint}" style="font: 400 ${f2(0.55 * h)}px ${SANS}">by</text>`);
  const byT = g.lastElementChild;
  x += byW + gap;
  for (const [k, v] of [["x", x], ["y", top], ["width", lw], ["height", h]]) lockup.setAttribute(k, String(f2(v)));
  g.append(lockup);
  x += lw + sep;
  g.append(svg`<rect x="${f2(x)}" y="${f2(top)}" width="${f2(0.1 * h)}" height="${f2(h)}" fill="${BY.faint}"/>`);
  const rule = g.lastElementChild;
  x += sep;
  for (const [k, v] of [["x", x], ["y", mid - sh / 2], ["width", sw], ["height", sh]]) seeds.setAttribute(k, String(f2(v)));
  g.append(seeds);
  x += sw + gap * 0.6;
  const ks = letters.map((c, i) => {
    g.append(svg`<text x="${f2(x)}" y="${f2(mid + 0.26 * h)}" fill="${BY.kaust}" style="font: 700 ${f2(0.72 * h)}px ${SANS}">${c}</text>`);
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
  lockup.prepend(svg`<defs><clipPath id="${wid}"><rect x="97.5" y="-50" width="1000" height="200"/></clipPath></defs>`);
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
  box.append(
    html`<svg class="phx-open" preserveAspectRatio="none" aria-hidden="true">
    <defs><mask id="phx-hole${tag}" maskUnits="userSpaceOnUse" x="-1e5" y="-1e5" width="2e5" height="2e5"><rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="#fff"/>
      <path class="phx-hole" d="${PHI_COUNTER}" fill="#000" opacity="0" transform="translate(${f2(phi.x)} ${f2(phi.y)}) scale(${size / 1e3} ${-size / 1e3})"/></mask></defs>
    <rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="${bg}" mask="url(#phx-hole${tag})"/>${svgArt(art(WOVEN, fill, tag))}</svg>`
  );
  const el = box.lastElementChild, hole2 = el.querySelector(".phx-hole");
  const by = byline(el, WOVEN);
  const word = { x0: Math.min(WOVEN.x0, by.x0), y0: WOVEN.y0, x1: Math.max(WOVEN.x1, by.x1), y1: by.y1 };
  const rest = [...el.querySelectorAll(".phx-l")].filter((g) => g.dataset.ch !== "\u03A6").map((g) => [g, w.letters.find((l) => l.ch === g.dataset.ch)]);
  const tmp = document.createElementNS(NS, "svg");
  tmp.style.cssText = "position:absolute;width:0;height:0;visibility:hidden";
  document.body.append(tmp);
  tmp.replaceChildren(svgArt(art(MARK2, fill, `${tag}m`)));
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
    const cm = fit(MARK2, 0.45, 0.3), cw = fit(word, 0.84, 0.4);
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
function layer(box, cls, css, markup) {
  register();
  const host2 = document.createElement("div");
  host2.style.cssText = "position: absolute; inset: 0; pointer-events: none;";
  host2.setAttribute("aria-hidden", "true");
  const root = host2.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = SHARED + css;
  root.replaceChildren(style, html`<div class="phx-o ${cls}">${markup}</div>`);
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
    done: (page) => ending2 ??= new Promise((ok) => {
      if (over) return ok();
      const ms = reduced() ? 0 : exit(page);
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
  const pdf = doc.replace(/\.tex$/i, "") + ".pdf";
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
    html`<div class="w bev"><div class="t"><span>PhiTeX Setup</span><i>_</i><i>□</i><i>×</i></div><div class="b"><div class="fl"><div class="fold f1"></div><div class="pg"></div><div class="fold f2"></div></div>
       <p class="msg">Starting…</p><p class="det"></p><div class="bar march"><span></span></div></div></div>
     <div class="pdfw"><div class="chrome"><div class="t"><span>${pdf} - PhiTeX</span><i>_</i><i class="mx">□</i><i>×</i></div><div class="menu"><span><u>F</u>ile</span><span><u>E</u>dit</span><span><u>V</u>iew</span><span><u>W</u>indow</span><span><u>H</u>elp</span></div></div></div>
     <div class="task"><div class="start">⊞ Start</div><div class="btn on">PhiTeX Setup</div><div class="clock"></div></div>
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
  const pdf = doc.replace(/\.tex$/i, "") + ".pdf";
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
    html`<div class="mb"><span class="ap"></span><span>File</span><span>Edit</span><span>View</span><span>Special</span><span>Help</span><span class="ck"></span></div>
     <div class="ic"><i></i><span>${pdf}</span></div>
     <div class="w"><div class="t"><i class="cl"></i><b>Typesetting</b><i class="zm"></i><i class="co"></i></div><div class="b"><div class="doc"></div><div class="msg">Starting…</div><div class="det"></div><div class="bar pole"><span></span></div></div></div>
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
    html`<div class="scr"><div class="txt"></div><div class="band"></div></div><div class="led"></div><div class="brand">PHITEX</div>`
  );
  const txt = el.querySelector(".txt");
  const cursor = document.createElement("span");
  cursor.className = "cur";
  const t0 = performance.now();
  const line = (content, cls = "") => {
    const l = document.createElement("div");
    l.className = `l ${cls}`;
    l.replaceChildren(html`${content}`);
    txt.append(l);
    l.append(cursor);
    while (txt.scrollHeight > txt.clientHeight + 2 && txt.children.length > 1) txt.firstElementChild.remove();
    return l;
  };
  const cmd = `phitex watch ${doc}`, prompt = line(html`<span class="p">$ </span>`);
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
    line(html`<span class="d">PhiTeX</span>`);
    queue.forEach((f) => f());
    queue = [];
  };
  let cur = null, spin = 0, spinner = 0;
  const step = (what, pct, detail) => later(() => {
    if (cur?.what !== what) {
      if (cur) cur.el.replaceChildren(html`<span class="ok">✓</span> ${cur.what.toLowerCase()}`);
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
      cur.el.replaceChildren(html`<span class="f">${name}</span> [${"\u2588".repeat(n)}${"\u2591".repeat(BAR - n)}] ${detail.split(" \xB7 ").slice(1).join("  ")}`);
    } else cur.el.replaceChildren(html`<span class="f">${SPIN[spin % SPIN.length]}</span> ${name}  <span class="dim">${detail}</span>`);
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
      if (cur) cur.el.replaceChildren(html`<span class="ok">✓</span> ${cur.what.toLowerCase()}`);
      line(html`<span class="ok">✓ page 1 in ${((performance.now() - t0) / 1e3).toFixed(1)} s</span>`);
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
    html`<div class="scr"><div class="txt"><div class="top">PhiTeX Setup</div>
     <div class="main"><p>Please wait while Setup typesets your document.</p>
     <div class="box"><div class="now">Starting…</div><div class="bar"></div><div class="pct">&nbsp;</div><div class="file">&nbsp;</div></div>
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
    html`<div class="roll"></div><div class="ding">ding</div>`
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
    html`<div class="blot"></div><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="flood"></div>`
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
    $(".det").textContent = pct !== void 0 ? detail.split(" \xB7 ").slice(0, 2).join(" \xB7 ") : detail;
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
    html`<canvas></canvas><div class="hud"><div><b>Stage</b><div class="v st">Ready</div></div><div><b>Packages</b><div class="meter">${Array.from({ length: 8 }, () => html`<i></i>`)}</div></div><div><b>Page</b><div class="v pg">–</div></div><div><b>Next</b><div class="next"></div></div></div>
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
    html`<svg viewBox="0 0 85 110" preserveAspectRatio="xMidYMid meet">
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
.phx-gl .shelf { position: absolute; left: 8%; right: 8%; bottom: 9%; height: 3.8cqw; border-bottom: .5cqw solid ${dark ? "#4a4e57" : "#c9ced8"}; padding-bottom: 1.4cqw; box-sizing: content-box; transition: opacity .25s; }
.phx-gl .shelf span { position: absolute; bottom: 1.4cqw; white-space: nowrap; font: 2.4cqw/1.4 "DM Mono", ui-monospace, monospace; padding: .2cqw 1.2cqw; border-radius: .6cqw; color: #fff; transition: left .32s cubic-bezier(.3,.7,.3,1) .2s; }
.phx-gl .shelf span.in { animation: phx-gl-land .52s cubic-bezier(.5,0,.8,.45) both, phx-gl-bounce .32s cubic-bezier(.3,1.8,.5,1) .52s; }
.phx-gl .shelf span.gone { animation: phx-gl-drop 1s cubic-bezier(.5,0,1,.6) .2s forwards; }
.phx-gl .shelf .more { left: 0; color: ${mute}; padding-left: 0; }
@keyframes phx-gl-land { from { transform: translateY(-110cqw) rotate(var(--r)); } to { transform: none; } }
@keyframes phx-gl-bounce { 0% { transform: scaleY(.7) translateY(.6cqw); } 100% { transform: none; } }
@keyframes phx-gl-drop { 0% { transform: none; } 14% { transform: translate(-.6cqw, -.8cqw) rotate(-10deg); } 85% { opacity: 1; } to { transform: translate(-4cqw, 24cqw) rotate(-40deg); opacity: 0; } }
.phx-gl.out .cap, .phx-gl.out .shelf { opacity: 0; }`,
    html`<canvas></canvas><div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div><div class="shelf"></div>`
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
    if (pkg && !seen.has(pkg)) seen.add(pkg), pending = pkg;
  };
  let pending = null, shown = 0;
  const row = [];
  const shelf = $(".shelf");
  const more = document.createElement("span");
  more.className = "more";
  shelf.append(more);
  const place4 = () => {
    const W3 = shelf.clientWidth, gap = box.clientWidth * 0.014, from2 = (more.textContent ? more.offsetWidth : 0) + gap;
    let x = W3;
    for (let i = row.length - 1; i >= 0; i--) {
      x -= row[i].offsetWidth;
      if (x < from2) {
        for (const e of row.splice(0, i + 1)) e.classList.add("gone"), setTimeout(() => e.remove(), 1300);
        break;
      }
      row[i].style.left = `${x}px`;
      x -= gap;
    }
  };
  const land = () => {
    if (!pending) return;
    const pkg = pending;
    pending = null;
    shown++;
    const w = document.createElement("span");
    w.textContent = pkg;
    w.style.background = [INK.light, ...RIBBON][shown % 5];
    w.style.setProperty("--r", `${(Math.random() - 0.5) * 40}deg`);
    w.style.transition = "none";
    shelf.append(w);
    w.style.left = `${shelf.clientWidth - w.offsetWidth}px`;
    w.classList.add("in");
    row.push(w);
    place4();
    more.textContent = seen.size > row.length ? `+${seen.size - row.length}` : "";
    requestAnimationFrame(() => w.style.transition = "");
  };
  const lander = window.setInterval(land, 400);
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(el, () => {
    out = performance.now() - t0;
    const lh = W2 * 0.0162 * 1.25 * 1.6;
    gs.forEach((g) => (g.tx = W2 * (0.115 + 0.77 * Math.random()), g.ty = H2 * 0.2 + Math.round(Math.random() * H2 * 0.65 / lh) * lh));
    el.classList.add("out");
    return 620;
  }, () => (stopLoop(), clearInterval(lander)), { step, hint });
}

// viewer/src/openings/kaust.ts
var art2 = (markup, cls) => {
  const s = svgArt(markup).firstElementChild;
  s.setAttribute("class", cls);
  s.setAttribute("aria-hidden", "true");
  return s;
};
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
    html`<div class="st"></div><div class="st"></div><div class="st"></div><div class="st"></div>
     <div class="row">${art2(LOCKUP.replace(/#1D3E72/gi, dark ? "#e4e6ea" : "#1D3E72"), "lk")}<div class="rule"></div>${art2(SEEDS, "sd")}</div>
     <div class="prog">${RIBBON.map((c) => html`<i style="--c:${c}"><b></b></i>`)}</div>
     <div class="cap"><div class="what"></div><div class="det"></div><div class="hint"></div></div>`
  );
  const $ = (s) => el.querySelector(s);
  const bars2 = [...el.querySelectorAll(".prog i")];
  const fill = (i, w) => {
    bars2[i].classList.toggle("run", w === "run");
    bars2[i].querySelector("b").style.width = w === "run" ? "" : `${w}%`;
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
      bars2.forEach((_, i) => fill(i, 100));
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
var FRAMES = 780;
var W = 256;
var H = 192;
var FPS = 30;
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
var MORPH = 1150;
var SETTLE = 220;
var ease3 = (k) => k < 0.5 ? 4 * k * k * k : 1 - (-2 * k + 2) ** 3 / 2;
var LW = 300;
var SIGMAS = [0, 1.2, 2.5, 4.5, 7.5, 12];
function blur(a, lw, lh, sigma) {
  const r = Math.max(1, Math.round((Math.sqrt(4 * sigma * sigma + 1) - 1) / 2)), d = 2 * r + 1;
  let src = a.slice(), dst = new Float32Array(a.length);
  for (let pass = 0; pass < 6; pass++) {
    const across = pass % 2 === 0, n = across ? lw : lh, m = across ? lh : lw, step = across ? 1 : lw, line = across ? lw : 1;
    for (let j = 0; j < m; j++) {
      const o = j * line, last = o + (n - 1) * step;
      let sum = src[o] * (r + 1);
      for (let i = 1; i <= r; i++) sum += src[o + Math.min(i, n - 1) * step];
      for (let i = 0, at = o; i < n; i++, at += step) {
        dst[at] = sum / d;
        const add = i + r + 1 < n ? o + (i + r + 1) * step : last, drop = i - r > 0 ? o + (i - r) * step : o;
        sum += src[add] - src[drop];
      }
    }
    [src, dst] = [dst, src];
  }
  return src;
}
function levels(a, lw, lh, norm) {
  const made = [];
  return (l) => {
    if (made[l]) return made[l];
    const s = SIGMAS[l], b = s ? blur(a, lw, lh, s) : a.slice();
    if (norm) {
      const hist = new Uint32Array(256);
      let n = 0;
      for (let i = 0; i < b.length; i++) if (b[i] > 0.03) hist[Math.min(255, b[i] * 255 | 0)]++, n++;
      let top = 255;
      for (let v = 0, c = 0; v < 256; v++) if ((c += hist[v]) >= n * 0.9) {
        top = v;
        break;
      }
      const k = (s ? 0.8 : 1.15) / Math.max(top / 255, 1e-3);
      for (let i = 0; i < b.length; i++) b[i] = Math.min(1, b[i] * k);
    }
    return made[l] = b;
  };
}
function field(lw, lh, ink2, draw) {
  const c = new OffscreenCanvas(lw, lh).getContext("2d", { willReadFrequently: true });
  draw(c);
  const px = c.getImageData(0, 0, lw, lh).data, out = new Float32Array(lw * lh);
  for (let i = 0; i < out.length; i++) {
    const l = (0.299 * px[4 * i] + 0.587 * px[4 * i + 1] + 0.114 * px[4 * i + 2]) / 255;
    out[i] = ink2 === "dark" ? 1 - l : l;
  }
  return out;
}
var rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
function badapple(box, dark) {
  const ink0 = dark ? "#e4e6ea" : "#1b222c", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-ba",
    `.phx-ba { background: ${dark ? PAPER.dark : PAPER.light}; font-family: "Noto Sans", system-ui, sans-serif; }
.phx-ba canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.phx-ba .scr { position: absolute; left: 8cqw; right: 8cqw; top: 16cqw; aspect-ratio: 4 / 3; box-shadow: 0 0 0 .3cqw #000, 0 1.4cqw 3cqw rgb(0 0 0 / 22%); transition: opacity .25s; }
.phx-ba .sub { position: absolute; left: 8%; right: 8%; top: 86cqw; text-align: center; color: ${ink0}; transition: opacity .25s; }
.phx-ba .sub .what { font-size: 3.6cqw; letter-spacing: .06em; }
.phx-ba .sub .det { font-size: 2.4cqw; color: ${mute}; margin-top: .8cqw; min-height: 1.4em; }
.phx-ba .sub .hint { font-size: 2.3cqw; color: ${mute}; margin-top: 3.6cqw; line-height: 1.45; font-style: italic; }
.phx-ba .cr { position: absolute; left: 0; right: 0; bottom: 3cqw; text-align: center; font-size: 1.7cqw; color: ${mute}; opacity: .8; transition: opacity .25s; }
.phx-ba.out .sub, .phx-ba.out .cr, .phx-ba.out .scr { opacity: 0; }`,
    html`<div class="scr"></div><canvas></canvas><div class="sub"><div class="what"></div><div class="det"></div><div class="hint"></div></div>
     <div class="cr">Bad Apple!! · Alstroemeria Records feat. nomico · PV: Anira · original: ZUN</div>`
  );
  const $ = (s) => el.querySelector(s);
  const cv = el.querySelector("canvas"), ctx = cv.getContext("2d"), dpr = devicePixelRatio || 1;
  let paths = null;
  void read().then((p) => paths = p);
  const t0 = performance.now();
  const screen = (width) => ({ x: 0.08 * width, y: 0.16 * width, w: 0.84 * width, h: 0.84 * width * H / W });
  let drawn = -1;
  let morph = null;
  const begin = (page) => {
    if (!paths) return;
    const f = drawn < 0 ? 0 : drawn, lw = LW, lh = Math.round(LW * cv.height / Math.max(1, cv.width)), s = screen(lw);
    const night = !!page && getComputedStyle(page).filter.includes("invert");
    const paper = night ? [40, 40, 40] : [255, 255, 255], ink2 = night ? [225, 225, 225] : [17, 17, 17];
    const a = field(lw, lh, night ? "light" : "dark", (c) => {
      c.fillStyle = night ? "#000" : "#fff";
      c.fillRect(0, 0, lw, lh);
      c.fillStyle = "#000";
      c.fillRect(s.x, s.y, s.w, s.h);
      c.setTransform(s.w / W, 0, 0, s.h / H, s.x, s.y);
      c.fillStyle = "#fff";
      c.fill(paths[f], "evenodd");
    });
    const b = field(lw, lh, "dark", (c) => {
      c.fillStyle = "#fff";
      c.fillRect(0, 0, lw, lh);
      if (page) c.imageSmoothingQuality = "high", c.drawImage(page, 0, 0, lw, lh);
    });
    const m = field(lw, lh, "light", (c) => {
      c.fillStyle = "#fff";
      c.fillRect(0, 0, lw, lh);
      c.fillStyle = "#000";
      c.fillRect(s.x, s.y, s.w, s.h);
    });
    const lo = Object.assign(document.createElement("canvas"), { width: lw, height: lh });
    cv.replaceWith(lo);
    const sheet = rgb(dark ? PAPER.dark : PAPER.light);
    morph = { at: performance.now(), lw, lh, a: levels(a, lw, lh, false), b: levels(b, lw, lh, true), m: levels(m, lw, lh, false), lo: lo.getContext("2d"), img: new ImageData(lw, lh), paper, ink: ink2, sheet };
  };
  const stop = loop(() => {
    const now = performance.now();
    const bw = Math.round(el.clientWidth * dpr), bh = Math.round(el.clientHeight * dpr);
    if (!morph && (cv.width !== bw || cv.height !== bh)) cv.width = bw, cv.height = bh, drawn = -1;
    if (!paths) return;
    if (!morph) {
      const f = Math.floor((now - t0) / 1e3 * FPS) % FRAMES;
      if (f === drawn) return;
      drawn = f;
      const s = screen(cv.width);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, cv.width, cv.height);
      ctx.fillStyle = "#000";
      ctx.fillRect(s.x, s.y, s.w, s.h);
      ctx.setTransform(s.w / W, 0, 0, s.h / H, s.x, s.y);
      ctx.fillStyle = "#fff";
      ctx.fill(paths[f], "evenodd");
      return;
    }
    const { lw, lh, a, b, m, lo, img, paper, ink: ink2, sheet } = morph, t = Math.min(1, (now - morph.at) / MORPH), e = ease3(Math.min(1, Math.max(0, (t - 0.08) / 0.62)));
    const L = Math.sin(Math.PI * t) ** 0.8 * (SIGMAS.length - 1), l0 = Math.floor(L), l1 = Math.min(SIGMAS.length - 1, l0 + 1), u = L - l0;
    const A0 = a(l0), A1 = a(l1), B0 = b(l0), B1 = b(l1), M0 = m(l0), M1 = m(l1), px = img.data;
    const soft = ease3(Math.min(1, Math.max(0, (t - 0.6) / 0.4))), dusk = 1 - ease3(Math.min(1, t / 0.5));
    for (let i = 0; i < lw * lh; i++) {
      const v = (1 - e) * (A0[i] + (A1[i] - A0[i]) * u) + e * (B0[i] + (B1[i] - B0[i]) * u);
      const c = Math.min(1, Math.max(0, (v - 0.42) / 0.16)), k = c * c * (3 - 2 * c), q = k + (Math.min(1, v) - k) * soft;
      const w = dusk * (M0[i] + (M1[i] - M0[i]) * u) * (1 - q);
      for (let j = 0; j < 3; j++) px[4 * i + j] = paper[j] + (ink2[j] - paper[j]) * q + (sheet[j] - paper[j]) * w;
      px[4 * i + 3] = 255;
    }
    lo.putImageData(img, 0, 0);
    el.style.opacity = String(1 - Math.min(1, Math.max(0, (now - morph.at - MORPH) / SETTLE)));
  });
  const step = (what, _pct, detail) => {
    $(".what").textContent = what;
    $(".det").textContent = detail;
  };
  const hint = (text) => $(".hint").textContent = `Did you know? ${text}`;
  return ending(
    el,
    (page) => {
      el.classList.add("out");
      begin(page);
      return MORPH + SETTLE + 40;
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
  // (made when a panel reads it, not when this module loads: nothing of the DOM before a page)
  get about() {
    return html`<b>⚡ Instant is not part of Overleaf.</b> It is added by the <b>unofficial PhiTeX</b> browser extension, not made,
      endorsed or supported by Overleaf: an experimental incremental TeX engine that runs entirely in your browser. Nothing is
      sent anywhere; Overleaf's own PDF is on the <b>PDF</b> tab.
      <div class="about-foot">LaTeX, with TeX Live's packages fetched as needed. Free software (AGPL-3.0-only,
      <a id="license" target="_blank" rel="noopener">full license</a>), provided as is, without any warranty.
      To turn it off: <code>chrome://extensions</code>.</div>`;
  },
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
var copyBtn = (i) => html`<button type="button" class="copyai" data-i="${i}" title="Copy for an AI assistant: this problem with its source around it, the preamble, the other problems and the log, to paste into the assistant you use (nothing is sent anywhere)" aria-label="Copy for an AI assistant">${icon("copy", 16)}</button>`;
var icon = (name, size = 18) => html`<svg class="icon" viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true"><path fill="currentColor" d="${PATHS[name]}"/></svg>`.firstElementChild;
var ZOOMS = [["fit", "Page width"], ["page", "Whole page"], ["0.5", "50%"], ["0.75", "75%"], ["1", "100%"], ["1.25", "125%"], ["1.5", "150%"], ["2", "200%"], ["3", "300%"]];
var mb = (b) => b ? ` \xB7 ${b < 1048576 ? `${Math.max(1, Math.round(b / 1024))} KB` : `${(b / 1048576).toFixed(1)} MB`}` : "";
var dur = (ms) => ms < 10 ? `${ms.toFixed(1)} ms` : ms < 1e3 ? `${Math.round(ms)} ms` : `${(ms / 1e3).toFixed(ms < 1e4 ? 1 : 0)} s`;
var bars = (parts, cycle = Infinity) => html`<div class="tbar">${parts.map(([, ms], i) => html`<i class="c${i % cycle}" style="flex-grow:${Math.max(ms, 0.05)}"></i>`)}</div><div class="legend">${parts.map(([n, ms], i) => html`<span><i class="c${i % cycle}"></i>${n} ${dur(ms)}</span>`)}</div>`;
function framePart(json) {
  if (!json) return "";
  const f = JSON.parse(json);
  const split = [["finding what changed", f.probe], ["drawing", f.draw], ["waiting for the engine", Math.max(0, f.request - f.watch)], ["decoding", f.decode]];
  return html`<div class="row"><span class="k">Drawing page ${f.page}${f.inView ? " (in view)" : ""}</span><span class="v">${dur(f.request + f.decode)}</span>${bars(split)}<span class="note">${f.drawn} slice${f.drawn === 1 ? "" : "s"} of the page drawn, ${f.kept} kept from before</span></div>`;
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
var lines = (...w) => w.map((x) => html`<i style="width:${x}%"></i>`);
var TIPS = [
  {
    id: "live",
    at: "#st",
    title: "Typeset as you type",
    text: "No compile button: change the source and only what you touched is typeset again. The page follows in milliseconds.",
    // (a heading typed in the editor, the page's heading following it letter by letter, the time it took)
    demo: () => html`<div class="demo live" aria-hidden="true"><div class="ed"><span class="k">\\section{</span><span class="ty">Results</span><span class="car"></span><span class="k">}</span><i style="width:80%"></i><i style="width:60%"></i></div><div class="pg"><b>1&nbsp;&nbsp;<span class="ty">Results</span></b>${lines(95, 90, 70)}</div><span class="ms">${icon("bolt", 11)}12 ms</span></div>`
  },
  {
    id: "compare",
    at: "#b-compare",
    title: "Compare versions",
    fresh: true,
    text: "See what changed since an earlier version, marked as latexdiff marks it: additions in blue, deletions struck out.",
    // (a sentence changes, then the change marked, then the navigator; Overleaf's compare tip had it first)
    demo: () => html`<div class="demo cmp" aria-hidden="true">The results are <span class="del">good</span><span class="add">&nbsp;excellent</span>.<span class="ver"><i class="v0">Submitted v1</i><i class="v1">Now vs Submitted v1</i></span><span class="tag">‹ 1 / 1 ›</span></div>`
  },
  {
    id: "timing",
    at: "#st",
    title: "How long it took",
    text: "Click the status for the last build's times and how long a keystroke takes to reach the page.",
    demo: () => html`<div class="demo tim" aria-hidden="true"><div class="mbar"><span class="mst">Built · 42 ms</span></div><div class="mpop"><b>Keystroke to page</b><p><span>edit</span><i style="--w:30%"></i><em>11 ms</em></p><p><span>typeset</span><i style="--w:55%"></i><em>25 ms</em></p><p><span>paint</span><i style="--w:18%"></i><em>6 ms</em></p></div></div>`
  },
  {
    id: "rebuild",
    at: "#b-rec",
    title: "Clean rebuild",
    text: "Builds run by themselves as you type. Start over from a clean slate here, or clear what PhiTeX keeps.",
    demo: () => html`<div class="demo reb" aria-hidden="true"><div class="mbar"><span class="msplit"><span class="go">${icon("refresh", 12)}Rebuild</span><span class="dd">${icon("expand_more", 12)}</span></span></div><div class="mmenu"><span>Rebuild</span><span class="pick">Clean rebuild</span></div><div class="pg">${lines(60, 95, 90, 70, 92, 50)}</div></div>`
  },
  {
    id: "more",
    at: "#b-more",
    title: "Dark PDF and more",
    text: "The pages in dark mode, the log, the contents and the keys are in this menu.",
    demo: () => html`<div class="demo drk" aria-hidden="true"><div class="mbar"><span class="dots">${icon("more_horiz", 14)}</span></div><div class="mmenu"><span class="pick">${icon("dark", 11)}Dark PDF<b>✓</b></span><span>View log</span></div><div class="pg">${lines(55, 95, 88, 92, 60, 85)}</div></div>`
  },
  {
    id: "contents",
    at: "#b-side",
    title: "Contents",
    fresh: true,
    text: "The document's sections beside the pages: click one to go there. This button (or t) shows or hides them.",
    demo: () => html`<div class="demo toc" aria-hidden="true"><div class="col"><span>1 Introduction</span><span>2 Method</span><span>3 Results</span><i class="at"></i></div><div class="pg"><div class="roll">${lines(45, 95, 90, 70)}<s></s>${lines(40, 92, 85, 60)}<s></s>${lines(42, 90, 94, 75)}</div></div></div>`
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
  /** A page drawn in this page's life: the engine is up, a rebuild's opening starts past loading it. */
  drewOnce = false;
  /** The step an opening says first: the one now, else loading the engine (or, once it ran here, the packages). */
  firstStep() {
    return this.stepNow || this.words.steps[this.drewOnce ? 1 : 0];
  }
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
    const item2 = (id, name, label, sub = "", cls = "") => html`<button type="button" role="menuitem" id="${id}" class="${cls || null}">${name ? html`<span class="icon">${name}</span>` : html`<span class="check"></span>`}<span>${label}</span>${sub ? html`<span class="sub">${sub}</span>` : ""}</button>`;
    const style = document.createElement("style");
    style.textContent = VIEWER_CSS + PROBLEMS_CSS + OUTLINE_CSS + PRINT_CSS;
    const skeleton = html`<div class="win ${this.kind}" role="dialog" aria-label="PhiTeX preview">
  <header><span>PhiTeX</span><span class="grow"></span><button class="ib" id="min" title="Collapse (Alt+Shift+P)" aria-label="Collapse"><span class="icon">close_fullscreen</span></button></header>
  <div class="bar" role="toolbar" aria-label="PhiTeX">
    <button type="button" class="ib" id="b-side" title="Contents (t)" aria-label="Contents" aria-pressed="false"><span class="icon">toc</span></button>
    <span class="split" id="split"><button type="button" class="go" id="go" title="Build again with the files as they are (builds run by themselves as you type)"><span class="icon">refresh</span><span class="wide">Rebuild</span></button><button type="button" class="more" id="b-rec" aria-haspopup="menu" aria-expanded="false" title="More ways to build" aria-label="More ways to build"><span class="icon">expand_more</span></button></span>
    <button type="button" class="status" id="st" aria-haspopup="dialog" aria-expanded="false" title="The last build's times"><b id="sttext">Starting…</b><span id="stmore"></span></button>
    <span class="grow"></span>
    <span class="find" id="find" hidden><input id="findq" type="search" placeholder="Find" spellcheck="false" aria-label="Find in the pages"><span id="findn"></span></span>
    <button type="button" class="ib wide" id="b-find" title="Find in the pages (Ctrl+F)" aria-label="Find"><span class="icon">search</span></button>
    <button type="button" class="ib wide" id="b-compare" title="Compare with an earlier version (latexdiff)" aria-label="Compare"><span class="icon">compare</span></button>
    <button type="button" class="ib wide" id="pdf" title="Download the PDF" aria-label="Download the PDF"><span class="icon">download</span></button>
    <span class="pageno"><input id="pgin" value="–" aria-label="Page" inputmode="numeric" autocomplete="off"><span>/&nbsp;<span id="pgn">0</span></span></span>
    <button type="button" class="ib zoomb wide" id="b-zoom" aria-haspopup="menu" aria-expanded="false" title="Zoom"><span id="zl">Page width</span><span id="zs">Fit</span><span class="icon">expand_more</span></button>
    <button type="button" class="ib probs" id="b-probs" aria-haspopup="dialog" aria-expanded="false" title="Problems" hidden><span id="psev"></span><span id="pcount"></span></button>
    <button type="button" class="ib" id="b-more" aria-haspopup="menu" aria-expanded="false" title="More" aria-label="More"><span class="icon">more_horiz</span></button>
    <span class="progress" id="prog"></span>
    <div class="menu" id="m-rec" role="menu" hidden style="left:6px;top:calc(100% + 4px)">
      ${item2("r-rebuild", "", "Rebuild")}
      <button type="button" role="menuitem" id="r-clean"><span class="check"></span><span>Clean rebuild<span class="desc">Start over: the project read again, built from the start</span></span></button>
      <hr class="if-compare"><button type="button" role="menuitem" id="r-compare" class="if-compare"><span class="check"></span><span>Compare with an earlier version…<span class="desc">latexdiff against a saved version</span></span></button>
      <hr class="if-cache"><button type="button" role="menuitem" id="r-cache" class="if-cache"><span class="check"></span><span>Clear cache…<span class="desc">The packages and formats PhiTeX keeps</span></span></button>
    </div>
    <div class="menu timing" id="m-time" role="dialog" aria-label="Timing" hidden style="left:6px;top:calc(100% + 4px)"></div>
    <div class="menu" id="m-zoom" role="menu" hidden style="right:60px;top:calc(100% + 4px);min-width:170px">
      ${ZOOMS.map(([z, t], i) => [i === 2 ? html`<hr>` : "", html`<button type="button" role="menuitemradio" data-zoom="${z}"><span class="check"></span>${t}</button>`])}
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
    <div class="stage" id="stage"><div class="strip gone" id="gone" role="alert"><span class="why" id="gonewhy"></span><button type="button" class="x" id="gonex" title="Dismiss: keep viewing these pages (Esc)" aria-label="Dismiss">✕</button></div><div class="strip approx" id="approx"></div><div class="strip banner" id="banner"></div><div class="empty" id="empty" hidden></div><div class="viewer" id="viewer"></div></div><div class="smarks" id="smarks" aria-hidden="true"></div><div class="ctip" id="ctip" role="tooltip" hidden></div><div id="speedchip" hidden><b id="speedms"></b><span id="stripsline"></span></div>
    <div class="opening" id="opening" hidden><div class="sheet"></div><div class="steps" aria-live="polite"><div class="what"></div><div class="track unknown"><i></i></div><div class="detail"></div><div class="hint" id="hint" hidden></div></div></div>
    <div class="menu diffset" id="diffset" role="dialog" aria-label="Diff style"></div>
    <div class="menu logs" id="logs" role="dialog" aria-label="Problems" hidden><header><span id="logshead">Problems</span><span class="grow"></span><button type="button" class="ib" id="logsx" title="Close (Esc)" aria-label="Close"><span class="icon">close</span></button></header><div class="diags" id="diags" role="list"></div></div>
    <div class="dialog settings" id="settings" role="dialog" aria-label="Settings" hidden></div>
    <div class="toast" id="toast" role="status" aria-live="polite" hidden></div>
    <div class="dialog keys" id="keyhelp" role="dialog" aria-label="Keys" hidden></div>
    <div class="dialog small about" id="about" role="dialog" aria-label="About the PhiTeX preview" hidden><header>About PhiTeX<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><div>${typeof w.about === "string" ? rich(w.about) : w.about.cloneNode(true)}</div></div>
    <div class="dialog" id="logview" role="dialog" aria-label="The build's log" hidden><header>The last build's log<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close"><span class="icon">close</span></button></header><pre id="logtext"></pre></div>
    <div class="dialog" id="report" role="dialog" aria-label="Debug report" hidden>
      <header>Debug report</header>
      <p>Anonymized: no document text, nothing you typed, no file names (file1.tex, …), no project id. Read it before you send it.</p>
      <textarea id="reporttext" readonly spellcheck="false"></textarea>
      <div class="btns"><button type="button" class="btn" id="reportcopy">Copy</button><button type="button" class="btn" id="reportmail">Email</button><button type="button" class="btn" id="reportclose">Close</button></div>
    </div>
    <div class="offpill" id="offpill">Disconnected</div>
    <button type="button" class="byline" id="byline" title="About this preview">${w.byline}</button>
  </div>
</div>`;
    for (const ic of skeleton.querySelectorAll("span.icon"))
      if (ic.className === "icon" && /^\w+$/.test(ic.textContent ?? "")) ic.replaceWith(icon(ic.textContent, ic.textContent === "expand_more" || ic.textContent === "refresh" ? 16 : 18));
    this.root.replaceChildren(style, skeleton);
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
    if (on && !this.last) this.step(this.firstStep(), void 0, "");
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
    b.replaceChildren(icon(on ? "open_in_full" : "close_fullscreen"));
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
      tip.replaceChildren(icon(p.severity, 14), document.createElement("span"));
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
      const copy = d.severity === "error" || d.severity === "warning" ? copyBtn(i) : null;
      if (d.problem) {
        const card = document.createElement("ol");
        card.className = "phx-card phx-inline";
        card.setAttribute("role", "listitem");
        card.replaceChildren(problemNode(d.problem));
        if (copy) card.firstElementChild.prepend(copy);
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
      row.replaceChildren(html`${copy}<span class="where"></span><div class="text"></div>`);
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
    this.$("#psev").replaceChildren(icon(errors ? "error" : "warning", 16));
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
    if (anim.step) anim.step(this.firstStep(), void 0, "");
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
    void a.done(this.$("#viewer").querySelector(".slot canvas") ?? void 0).then(stop);
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
    el.replaceChildren(html`<div class="t">${t.icon ? icon(t.icon, 16) : ""}<span></span></div><div class="d"></div>${t.pct === null ? "" : html`<div class="track"><i style="width:${t.pct}%"></i></div>`}`);
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
    help.replaceChildren(html`<header>Keyboard shortcuts<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><table>${[...KEYS, ...extra].map(([k, w]) => html`<tr><td><kbd>${k}</kbd></td><td>${w}</td></tr>`)}</table>`);
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
    el.replaceChildren(html`<header>Settings<span class="grow"></span><button type="button" class="ib" data-close aria-label="Close">${icon("close")}</button></header><nav class="stabs" role="tablist">${shown.map((g, i) => html`<button type="button" role="tab" data-tab="${i}" data-group="${g.title}" aria-selected="${i === at}">${g.title}</button>`)}</nav><div class="sbody"></div>`);
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
    const off = !!(r.disabled || !r.set);
    if (r.kind === "toggle") ctl.replaceChildren(html`<button type="button" role="switch" class="switch" aria-checked="${r.value === true}" aria-label="${r.label}" disabled="${off}"><i></i></button>`);
    else if (r.kind === "choice")
      ctl.replaceChildren(html`<div class="seg" role="group" aria-label="${r.label}">${(r.options ?? []).map(([v, l]) => html`<button type="button" data-v="${v}" aria-pressed="${r.value === v}" disabled="${off}">${l}</button>`)}</div>`);
    else if (r.kind === "select") {
      const s = document.createElement("select");
      s.setAttribute("aria-label", r.label);
      s.disabled = off;
      for (const [v, l] of r.options ?? []) s.append(new Option(l, v, v === r.value, v === r.value));
      ctl.append(s);
    } else if (r.kind === "action") ctl.replaceChildren(html`<button type="button" class="btn" disabled="${off}">${r.button ?? r.label}</button>`);
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
    const build2 = [];
    if (ev.onDebug) build2.push({ id: "debug", label: "Debug check", help: "Compare with a fresh build every 5 s (this session only)", kind: "toggle", value: this.debugOn, set: () => this.toggleDebug() });
    build2.push({ id: "reload", label: "Reload the files", help: "Read the project's files again", kind: "action", button: "Reload", set: () => (this.settings(false), void ev.onReload()) });
    const tips = [{ id: "tips", label: "Feature tips", help: "Shown again, from the start", kind: "action", button: "Show again", set: () => (this.tipsReset(), this.settings(false)) }];
    if (ev.onTour) tips.push({ id: "tour", label: "The tour", kind: "action", button: "Take the tour", set: () => (this.settings(false), ev.onTour()) });
    if (ev.onNews) tips.push({ id: "news", label: "What's new", kind: "action", button: "Show", set: () => (this.settings(false), ev.onNews()) });
    return [
      { title: "Preview", rows: preview },
      { title: "Appearance", rows: look2 },
      { title: "Build", rows: build2 },
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
    el.replaceChildren(html`<h4>${t.fresh ? html`<span class="new">New</span>` : ""}${t.title}</h4>${t.demo()}<p>${t.text}</p><div class="acts"><span class="n">${i + 1} of ${this.tipList.length}</span><button type="button" data-t="skip">Skip</button><button type="button" class="primary" data-t="next">${i + 1 < this.tipList.length ? "Next" : "Got it"}</button></div>`);
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
    const n = d.busy ? html`<span class="n busy">${d.busy}</span>` : html`<span class="n">${d.changes ? `${d.at > 0 ? `${d.at} / ` : ""}${d.changes} change${d.changes === 1 ? "" : "s"}` : "no changes"}</span>`;
    const stuck = !!d.busy || !d.changes;
    el.replaceChildren(html`<span class="what" title="${d.title} against the current version">${d.title} vs current <small>${d.when}</small></span><button data-a="prev" title="Previous change (Shift+N)" disabled="${stuck}">‹</button>${n}<button data-a="next" title="Next change (N)" disabled="${stuck}">›</button><span class="sep"></span><span class="seg" role="group" aria-label="Pages shown (D)"><button data-a="${d.showing === "diff" ? "settings" : "toggle"}" class="on-diff" aria-pressed="${d.showing === "diff"}" title="${d.showing === "diff" ? "The diff is shown: its style (colors, markup, change bars)" : "Show the diff (D)"}">Diff${d.showing === "diff" ? html` <span class="chev" aria-label="Diff style">▾</span>` : ""}</button><button data-a="${d.showing === "current" ? "" : "toggle"}" class="on-current" aria-pressed="${d.showing === "current"}" title="${d.showing === "current" ? "The current version is shown" : "Show the current version (D)"}">Current</button></span><button data-a="pdf" title="Download the diff PDF" disabled="${!!d.busy}">⬇ PDF</button><button data-a="tex" title="Download diff.tex" disabled="${!!d.busy}">⬇ .tex</button><button data-a="settings" title="Diff style: colors, markup, change bars" aria-label="Diff style">⚙</button><button data-a="close" title="Stop comparing" aria-label="Stop comparing">✕</button>`);
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
    const bars2 = look2.markup in BASE2;
    const SUB = [["safe", "Plain"], ["color", "Colored"], ["marker", "Margin marks"]];
    const PAIRS = [
      ["#0000ff", "#ff0000", "Blue / red"],
      ["#15803d", "#b91c1c", "Green / red"],
      ["#0072b2", "#d55e00", "Colorblind-safe"],
      ["#7e22ce", "#6b7280", "Purple / gray"]
    ];
    const seg = (key, opts) => html`<div class="opts">${opts.map(([v, t]) => html`<button data-k="${key}" data-v="${v}" aria-pressed="${(key === "markup" ? base : look2[key]) === v}">${t}</button>`)}</div>`;
    const canBars = base in WITH_BARS;
    const deco = (add) => {
      const c = add ? look2.add_color : look2.del_color;
      if (look2.markup === "invisible" && !add) return "display:none";
      const strike = !add && /^(underline|fontstrike|culinechbar)$/.test(look2.markup);
      const line = add && /^(underline|culinechbar)$/.test(look2.markup) ? `text-decoration: underline wavy ${c}; text-underline-offset: 3px;` : "";
      return `color:${c};${strike ? `text-decoration: line-through ${c};` : ""}${line}${look2.markup === "bold" && add ? "font-weight:700;" : ""}`;
    };
    el.replaceChildren(html`<h4>Style</h4>${seg("markup", MARKUP)}<div class="row bars"><span>Change bars in the margin</span><button data-bars="1" role="switch" aria-checked="${bars2}" aria-pressed="${bars2}" disabled="${!canBars}" title="${canBars ? null : "Not with this style"}">${bars2 ? "On" : "Off"}</button></div><h4>Colors</h4><div class="opts">${PAIRS.map(([a, d, t]) => html`<button class="pair" data-pair="${a},${d}" aria-pressed="${look2.add_color === a && look2.del_color === d}"><i style="background:${a}"></i><i style="background:${d}"></i>${t}</button>`)}</div><div class="row"><span>Added</span><input type="color" data-c="add_color" value="${look2.add_color}"><span>Deleted</span><input type="color" data-c="del_color" value="${look2.del_color}"></div><h4>Mode</h4>${seg("subtype", SUB)}<div class="sample" style="${bars2 ? `box-shadow: inset 3px 0 0 ${look2.add_color}; padding-left: 12px` : null}">The results are <span style="${deco(false)}">good</span> <span style="${deco(true)}">excellent</span>.</div><div class="foot"><small>As latexdiff's --type, --subtype</small><button data-reset="1">Reset</button></div>`);
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
      if (b.dataset.bars) return canBars && set({ markup: bars2 ? base : WITH_BARS[base] });
      if (b.dataset.k === "markup") return set({ markup: bars2 && b.dataset.v in WITH_BARS ? WITH_BARS[b.dataset.v] : b.dataset.v });
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
    return html`<div class="row"><span class="k">Where the time went</span><span class="v"></span>${bars(parts, 4)}</div>`;
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
    return html`<div class="row"><span class="k">Where the last build's time went</span><span class="v"></span>${bars(parts, 4)}${beside ? html`<span class="note">Alongside: ${beside}</span>` : ""}</div>`;
  }
  /** The packages fetched since the project opened: how many, their bytes, and the time any was on its way (`since`: one is now). */
  downloads = { n: 0, bytes: 0, ms: 0 };
  /** The timing popover's Downloads row: the packages' time, how many and their size; none fetched, none. */
  downloadsPart() {
    const d = this.downloads;
    if (!d.n && d.since === void 0) return "";
    const ms = d.ms + (d.since === void 0 ? 0 : performance.now() - d.since);
    const size = d.bytes ? ` \xB7 ${d.bytes < 1e6 ? `${Math.max(1, Math.round(d.bytes / 1e3))} kB` : `${(d.bytes / 1e6).toFixed(1)} MB`}` : "";
    return html`<div class="row"><span class="k">Downloads</span><span class="v">${dur(ms)}</span><span class="note">${d.n} package${d.n === 1 ? "" : "s"}${size}${d.since === void 0 ? "" : ", more on the way"}</span></div>`;
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
    return html`<div class="tbars" role="img" aria-label="First build ${dur(t.first)}, this edit ${dur(t.edits[t.edits.length - 1])}">${rows.map(([k, ms, c]) => html`<div class="tb"><span class="k">${k}</span><span class="track"><i class="${c}" style="width:${Math.max(0.6, 100 * ms / max).toFixed(2)}%"></i></span><span class="v">${dur(ms)}</span></div>`)}${x >= 1.5 ? html`<div class="tb x">${x < 10 ? x.toFixed(1) : Math.round(x)}× faster than the first build</div>` : ""}</div>`;
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
    const rows = parts.map((p) => html`<div class="row"><span class="k">${p.label}</span><span class="v">${p.value}</span>${p.split?.length ? bars(p.split) : ""}${p.note ? html`<span class="note">${p.note}</span>` : ""}</div>`);
    const strips2 = framePart(this.$("#stripsline").dataset.frame);
    el.replaceChildren(html`${head ? html`<div class="head"><span>${String(head[0])}</span><b>${dur(head[1])}</b></div>` : html`<div class="head"><span>No build yet</span></div>`}${this.walkPart()}${this.phasesPart()}${this.timeBars()}${this.downloadsPart()}${rows}${strips2}<div class="foot"></div><div class="foot msg${this.msgErr ? " err" : ""}"></div><button type="button" class="reportbtn copydbg">Copy debug info</button>`);
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
    this.drewOnce = true;
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
  /** The text after all of them. */
  text;
  constructor(text) {
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
  /** The queued edits as the core takes them (UTF-16), in order, each against the text before it. */
  take() {
    const out = this.edits.map((e) => ({ start: e.from, end: e.to, text: e.text }));
    this.edits = [];
    this.befores = [];
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

// viewer/src/latexmkrc.ts
function latexmkFiles(files) {
  const get = typeof files === "function" ? files : (p) => Object.hasOwn(files, p) ? files[p] : void 0;
  const out = {};
  const read2 = (path) => {
    const text = Object.hasOwn(out, path) ? void 0 : get(path);
    if (text === void 0) return;
    out[path] = text;
    for (const m of text.matchAll(/^[^#\n]*?\b(?:do|require)\s*\(?\s*(["'])(\.\.?\/[^"'\n]+)\1/gm)) {
      const p = normal(m[2]);
      if (p) read2(p);
    }
  };
  read2(get(".latexmkrc") !== void 0 ? ".latexmkrc" : "latexmkrc");
  return Object.keys(out).length ? out : void 0;
}
function normal(path) {
  const parts = [];
  for (const p of path.split("/")) {
    if (p === "" || p === ".") continue;
    if (p !== "..") parts.push(p);
    else if (!parts.pop()) return void 0;
  }
  return parts.join("/") || void 0;
}
function latexmkNotes(notes) {
  return notes.map((n) => {
    const m = /^(?:\/compile\/|\.\/)?([^\s:][^:]*):(\d+): ([\s\S]*)$/.exec(n);
    return m ? { severity: "info", code: "latexmkrc", file: m[1], line: +m[2], message: `latexmkrc: ${m[3]}` } : { severity: "info", code: "latexmkrc", message: `latexmkrc: ${n}` };
  });
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
      if (e.event === "latexmkrc") return void (this.rcNotes = latexmkNotes(e.notes), this.statusSoon());
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
    const rc = latexmkFiles(this.files);
    this.rcFiles = new Set(Object.keys(rc ?? {}));
    this.rcNotes = [];
    const latexmk = rc && { files: rc, exact: false };
    const r = await this.core.request({ op: "open", main: this.main, files: { ...this.pkgFiles, ...shims, ...this.files }, fuel: this.o.fuel, engine, workers: this.o.workers, latexmk, projectFiles: rc && Object.keys(this.files) });
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
  /** The open's rc files (latexmkrc.ts): an edit to one reopens. */
  rcFiles = /* @__PURE__ */ new Set();
  /** What the open's rc files set that was not applied (the core's `latexmkrc` event). */
  rcNotes = [];
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
    this.sink.status({ pages: this.pages, pending, main: this.main, file: this.open, diagnostics: this.rcNotes.length ? [...this.diags, ...this.rcNotes] : this.diags });
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
      this.rcChanged(file);
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
    this.rcChanged(file);
    if (!this.scheduled) {
      this.scheduled = true;
      const f = () => this.flush();
      if (this.o.schedule) this.o.schedule(f);
      else setTimeout(f, 0);
    }
  }
  rcTimer;
  /** `file` changed: one of the open's rc files, or a new root one, reopens (once typing pauses): the core reads them at an open only. */
  rcChanged(file) {
    if (!this.opened || !(this.rcFiles.has(file) || file === "latexmkrc" || file === ".latexmkrc")) return;
    clearTimeout(this.rcTimer);
    this.rcTimer = setTimeout(() => this.chain = this.chain.then(() => this.reopen()), 800);
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
  const svg2 = document.createElementNS(SVG, "svg");
  svg2.setAttribute("class", "phx-text");
  svg2.setAttribute("viewBox", `0 0 ${w} ${h}`);
  svg2.setAttribute("preserveAspectRatio", "none");
  for (const l of lines2) {
    const t = document.createElementNS(SVG, "text");
    t.setAttribute("x", l.xs ? l.xs.map((x) => x.toFixed(2)).join(" ") : String(l.x));
    t.setAttribute("y", String(l.y));
    t.setAttribute("font-size", String(l.size));
    t.textContent = l.text;
    LAYER_LINES.set(t, l);
    svg2.append(t);
  }
  return svg2;
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
    this.el.replaceChildren(this.html());
    this.el.scrollTop = scroll;
  }
  html() {
    if (this.error) return html`<div class="note err">${this.error}</div>`;
    if (!this.commits.length) return html`<div class="note">${this.loading ? "reading the history\u2026" : "no commits"}</div>`;
    const { rows, width } = layout(this.commits);
    const W2 = (width + 1) * LANE;
    const x = (l) => LANE / 2 + l * LANE + 2;
    const fi = this.find(this.from);
    const ti = this.to ? this.find(this.to) : -1;
    const tree = this.from !== null && !this.to;
    const top = html`<div class="row${tree ? " to" : ""}" title="Shift-click: diff the working tree"><svg width="${W2}" height="${ROW}"><circle cx="${x(0)}" cy="${ROW / 2}" r="4" fill="none" stroke="#888" stroke-dasharray="2 2"/><line x1="${x(0)}" y1="${ROW / 2 + 4}" x2="${x(0)}" y2="${ROW * 1.5}" stroke="#888" stroke-dasharray="2 2"/></svg><span class="txt">${tree ? html`<span class="end">TO</span>` : ""}<i>the working tree</i></span></div>`;
    const lines2 = this.commits.map((c, i) => {
      const r = rows[i];
      const col = COLORS[r.lane % COLORS.length];
      const marks = r.down.map(([a, b]) => svg`<path d="M${x(a)} ${ROW / 2} C${x(a)} ${ROW} ${x(b)} ${ROW} ${x(b)} ${ROW * 1.5}" stroke="${COLORS[b % COLORS.length]}" fill="none" stroke-width="1.6"/>`);
      marks.push(c.parents.length > 1 ? svg`<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4.5" fill="Canvas" stroke="${col}" stroke-width="2"/>` : svg`<circle cx="${x(r.lane)}" cy="${ROW / 2}" r="4" fill="${col}"/>`);
      const pills = c.refs.map((f) => html`<span class="pill ${KINDS2.has(f.kind) ? f.kind : "branch"}">${f.name}</span>`);
      const lo = this.to ? ti : -1;
      const cls = i === fi ? " from" : i === ti ? " to" : fi >= 0 && i > lo && i < fi ? " inrange" : "";
      const end = i === fi ? html`<span class="end">FROM</span>` : i === ti ? html`<span class="end">TO</span>` : "";
      const title = `${c.subject}
${c.author}, ${new Date(c.time * 1e3).toLocaleString()}
click: From \xB7 shift-click: To \xB7 double-click: against its first parent`;
      return html`<div class="row${cls}" data-i="${i}" title="${title}"><svg width="${W2}" height="${ROW}">${marks}</svg><span class="txt">${end}<span class="sha">${c.short}</span>${pills}${c.parents.length > 1 ? html`<i>merge:</i> ` : ""}${c.subject}<span class="who">${c.author}, ${ago(c.time)}</span></span></div>`;
    });
    const more = this.more ? html`<div class="note"><button type="button" data-more>${this.loading ? "reading\u2026" : "more\u2026"}</button></div>` : "";
    return html`${top}${lines2}${more}`;
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
      about: html`<b>phitex watch</b>: the document built from the files on disk, again at each save, its pages here as
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
