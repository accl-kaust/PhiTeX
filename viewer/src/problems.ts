// The build's errors and warnings over the pages, as the dev servers of
// web frameworks show theirs: a panel that opens by itself when a build
// ends with errors, each error with its place (a click opens the source),
// the line as TeX read it with the culprit marked, the macros it was in,
// and TeX's help. Host-neutral: the host gives the items (snippet::json's,
// the `diagnostics` event's) and what opening a place does.

/** One diagnostic, as `snippet::json` writes it. */
export interface Problem {
  severity: "fatal" | "error" | "warning" | "note";
  code: string;
  message: string;
  notes: string[];
  help: string[];
  suggestions: string[];
  file: string | null;
  line: number | null;
  col: number | null;
  /** The line as far as TeX read it and after, the culprit at [start, start+len) (characters). */
  excerpt: { text: string; start: number; len: number } | null;
  /** The macros it happened in, innermost first. */
  context: { name: string; before: string; after: string }[];
  /** The files it was included from. */
  included: { file: string; line: number }[];
  box: { lines: [number, number]; amount: number; excerpt: string } | null;
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/** Errors (fatal ones too) and warnings, counted. */
export function counts(items: Problem[]): { errors: number; warnings: number } {
  let errors = 0, warnings = 0;
  for (const p of items) {
    if (p.severity === "error" || p.severity === "fatal") errors++;
    else if (p.severity === "warning") warnings++;
  }
  return { errors, warnings };
}

export class Problems {
  private el: HTMLElement;
  private items: Problem[] = [];
  /** The errors last closed by hand: not opened again until they change. */
  private dismissed = "";
  private open: (file: string, line: number, col: number) => void;

  constructor(root: HTMLElement, open: (file: string, line: number, col: number) => void) {
    this.open = open;
    this.el = document.createElement("div");
    this.el.className = "phx-problems";
    this.el.hidden = true;
    this.el.setAttribute("role", "dialog");
    this.el.setAttribute("aria-label", "Build problems");
    root.append(this.el);
    this.el.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      const at = t.closest<HTMLElement>("[data-file]");
      if (at) {
        e.preventDefault();
        this.open(at.dataset.file!, Number(at.dataset.line) || 1, Number(at.dataset.col) || 1);
        return;
      }
      if (t.closest(".phx-close") || t === this.el) this.close();
    });
  }

  get shown(): boolean {
    return !this.el.hidden;
  }

  /** A build's diagnostics: the panel opens by itself if it has errors not closed before. */
  set(items: Problem[]): void {
    this.items = items;
    const key = JSON.stringify(items.filter((p) => p.severity === "error" || p.severity === "fatal").map((p) => [p.message, p.file, p.line]));
    if (!key || key === "[]") {
      this.dismissed = "";
      if (!this.items.length) this.el.hidden = true;
      else if (this.shown) this.render();
      return;
    }
    if (key !== this.dismissed) this.show();
    else if (this.shown) this.render();
  }

  show(): void {
    this.render();
    this.el.hidden = false;
  }

  /** Closed by hand: these errors stay closed until they change. */
  close(): void {
    const errs = this.items.filter((p) => p.severity === "error" || p.severity === "fatal");
    this.dismissed = JSON.stringify(errs.map((p) => [p.message, p.file, p.line]));
    this.el.hidden = true;
  }

  toggle(): void {
    if (this.shown) this.close();
    else this.show();
  }

  private render(): void {
    const { errors, warnings } = counts(this.items);
    const head = `<header><b>${errors ? `${errors} error${errors === 1 ? "" : "s"}` : "No errors"}</b>${warnings ? ` · ${warnings} warning${warnings === 1 ? "" : "s"}` : ""}<button type="button" class="phx-close" aria-label="Close">Esc</button></header>`;
    // (errors first, then warnings, then notes: each in the order TeX met them)
    const rank = { fatal: 0, error: 0, warning: 1, note: 2 } as const;
    const sorted = [...this.items].sort((a, b) => rank[a.severity] - rank[b.severity]);
    this.el.innerHTML = `<div class="phx-card">${head}<ol>${sorted.map((p) => item(p)).join("")}</ol><footer>Click a place to open it in the editor · Esc or a click outside closes this · it opens again when the errors change</footer></div>`;
  }
}

function place(file: string, line: number | null, col: number | null, text?: string): string {
  return `<a href="#" class="phx-place" data-file="${esc(file)}" data-line="${Number(line) || 1}" data-col="${Number(col) || 1}">${esc(text ?? `${file}${line ? `:${line}` : ""}${col ? `:${col}` : ""}`)}</a>`;
}

/** One problem's card (the CLI's panel, and the Overleaf-style panel's diagnostics drawer). */
export function problemHtml(p: Problem): string {
  return item(p);
}

/** A problem as diagnostics.ts's Diagnostic, the whole problem kept for the drawer to show. */
export function asDiagnostic(p: Problem): { severity: "error" | "warning" | "info"; code: string; message: string; file?: string; line?: number; problem: Problem } {
  // (from the wire: what a sender left out, empty, so one short item never stops the view)
  p = Object.assign({ notes: [], help: [], suggestions: [], file: null, line: null, col: null, excerpt: null, context: [], included: [], box: null }, p);
  const severity = p.severity === "fatal" || p.severity === "error" ? "error" : p.severity === "warning" ? "warning" : "info";
  return { severity, code: p.code, message: p.message, file: p.file ?? undefined, line: p.line ?? undefined, problem: p };
}

function item(p: Problem): string {
  // (a class name: only the four known)
  const sev = p.severity === "fatal" ? "error" : p.severity === "warning" || p.severity === "note" ? p.severity : "error";
  const where = p.file ? place(p.file, p.line, p.col) : "";
  let ex = "";
  if (p.excerpt) {
    const chars = [...p.excerpt.text];
    const a = chars.slice(0, p.excerpt.start).join("");
    const b = chars.slice(p.excerpt.start, p.excerpt.start + p.excerpt.len).join("");
    const c = chars.slice(p.excerpt.start + p.excerpt.len).join("");
    ex = `<pre class="phx-excerpt">${p.line ? `<span class="phx-ln">${Number(p.line)} │ </span>` : ""}${esc(a)}<mark>${esc(b)}</mark>${esc(c)}</pre>`;
  }
  const ctx = p.context.length
    ? `<details class="phx-ctx"><summary>in ${p.context.map((m) => `<code>${esc(m.name)}</code>`).join(" ← ")}</summary>${p.context
        .map((m) => `<pre><b>${esc(m.name)}</b> ${esc(m.before)}<mark>‸</mark>${esc(m.after)}</pre>`)
        .join("")}</details>`
    : "";
  const inc = p.included.length ? `<div class="phx-inc">included from ${p.included.map((i) => place(i.file, i.line, 1)).join(", ")}</div>` : "";
  const box = p.box ? `<pre class="phx-excerpt">${esc(p.box.excerpt)}</pre>` : "";
  const list = (cls: string, xs: string[]) => (xs.length ? `<ul class="${cls}">${xs.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>` : "");
  return `<li class="phx-${sev}"><div class="phx-msg"><span class="phx-sev">${esc(String(p.severity))}</span>${p.code ? `<code class="phx-code">${esc(p.code)}</code>` : ""}<span>${esc(p.message)}</span></div>${where ? `<div class="phx-where">${where}</div>` : ""}${ex}${box}${list("phx-notes", p.notes)}${ctx}${inc}${list("phx-help", [...p.help, ...p.suggestions])}</li>`;
}

/** The panel's CSS, in the host's tokens where it has them (fallbacks otherwise). */
export const PROBLEMS_CSS = `
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
.phx-help li::marker { content: "→ "; color: #d71920; }
.phx-ctx { margin-top: 8px; color: #4a4a48; }
.phx-ctx summary { cursor: pointer; }
.phx-ctx code { color: #141414; }
.phx-inc { margin-top: 6px; color: #8a8a86; font-size: 12px; }
.phx-card footer { padding: 10px 20px; color: #8a8a86; font-size: 11px; }
`;
