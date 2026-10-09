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
.phx-problems { position: fixed; inset: 0; z-index: 10; background: rgba(0, 0, 0, 0.45); overflow: auto; padding: 48px 16px; }
.phx-card { max-width: 960px; margin: 0 auto; background: #1b1c1f; color: #e6e6e6; border-radius: 10px; border-top: 6px solid #e5484d;
  box-shadow: 0 12px 48px rgba(0, 0, 0, 0.5); font: 13px/1.5 system-ui, sans-serif; }
.phx-card header { display: flex; align-items: center; gap: 8px; padding: 14px 18px; border-bottom: 1px solid #2c2e33; font-size: 15px; }
.phx-card header b { color: #ff6369; }
.phx-close { margin-left: auto; background: #2c2e33; color: #bbb; border: 0; border-radius: 6px; padding: 3px 10px; font: 12px ui-monospace, monospace; cursor: pointer; }
.phx-card ol { list-style: none; margin: 0; padding: 0; }
.phx-card li { padding: 14px 18px; border-bottom: 1px solid #2c2e33; }
.phx-msg { display: flex; flex-wrap: wrap; align-items: baseline; gap: 8px; font-size: 14px; }
.phx-sev { text-transform: uppercase; font-size: 11px; letter-spacing: 0.06em; font-weight: 700; padding: 1px 6px; border-radius: 4px; }
.phx-error .phx-sev { background: #e5484d; color: #fff; }
.phx-warning .phx-sev { background: #f5a524; color: #1b1c1f; }
.phx-note .phx-sev { background: #3e63dd; color: #fff; }
.phx-code { color: #9a9a9a; font: 12px ui-monospace, monospace; }
.phx-where { margin-top: 4px; font: 12px ui-monospace, monospace; }
.phx-place { color: #70b8ff; text-decoration: none; }
.phx-place:hover { text-decoration: underline; }
.phx-excerpt, .phx-ctx pre { margin: 8px 0 0; padding: 8px 10px; background: #111214; border-radius: 6px; overflow-x: auto;
  font: 12px/1.45 ui-monospace, monospace; white-space: pre; color: #cfcfcf; }
.phx-excerpt mark, .phx-ctx mark { background: none; color: #ff6369; text-decoration: underline wavy #ff6369; text-underline-offset: 3px; }
.phx-ln { color: #666; }
.phx-notes, .phx-help { margin: 8px 0 0; padding-left: 18px; color: #b5b5b5; }
.phx-help li::marker { content: "→ "; color: #70b8ff; }
.phx-ctx { margin-top: 8px; color: #b5b5b5; }
.phx-ctx summary { cursor: pointer; }
.phx-ctx code { color: #e6e6e6; }
.phx-inc { margin-top: 6px; color: #9a9a9a; font-size: 12px; }
.phx-card footer { padding: 10px 18px; color: #8a8a8a; font-size: 12px; }
`;
