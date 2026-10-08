// A debug report a user can send: what the extension did and what went
// wrong, anonymized. It holds no document text, nothing typed, no project id
// or URL, and no file names: each project file is `file1.tex`, `file2.bib`, …
// It holds versions, timings, build kinds, error and panic messages,
// diagnostics without their source lines, and TeX Live package names (public).
// The user sees the whole text before copying or emailing it.

/** Where reports are emailed; "" hides the Email button. */
export const REPORT_TO = "flinner@nand.sh";

export interface ReportInput {
  version: string;
  engine: string;
  userAgent: string;
  /** The project's file paths: replaced by file1.tex, … everywhere. */
  paths: string[];
  /** The session's trace: { t, k, d }. */
  trace: { t: number; k: string; d?: unknown }[];
  diagnostics: { severity: string; code?: string; message: string; file?: string; line?: number }[];
  status?: Record<string, unknown>;
  packages?: { done?: string[]; failed?: { name: string; error?: string }[]; unavailable?: string[] };
  errors: string[];
}

/** Replaces every project path (and its name without extension) by an alias. */
export function anonymizer(paths: string[]): (s: string) => string {
  const pairs: [string, string][] = [];
  paths.forEach((p, i) => {
    const ext = p.match(/\.[A-Za-z0-9]+$/)?.[0] ?? "";
    const alias = `file${i + 1}${ext}`;
    pairs.push([p, alias]);
    const base = p.replace(/^.*\//, "");
    if (base !== p) pairs.push([base, alias]);
    const stem = base.replace(/\.[A-Za-z0-9]+$/, "");
    // (a job name, "Thesis" for Thesis.tex; only names long enough not to hit ordinary words)
    if (stem.length >= 4 && stem !== base) pairs.push([stem, alias.replace(/\.[A-Za-z0-9]+$/, "")]);
  });
  pairs.sort((a, b) => b[0].length - a[0].length);
  const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = pairs.length ? new RegExp(pairs.map(([p]) => `(?<![A-Za-z0-9_])${esc(p)}(?![A-Za-z0-9_])`).join("|"), "g") : null;
  const map = new Map(pairs);
  return (s) =>
    (re ? s.replace(re, (m) => map.get(m) ?? m) : s)
      // (a label's, a citation's or a file's name, as LaTeX quotes them)
      .replace(/`[^'\n]{1,80}'/g, "`…'")
      // (paths on the machine that built the engine, in a panic's location)
      .replace(/\/home\/[^/\s]+\//g, "~/")
      .replace(/\/Users\/[^/\s]+\//g, "~/");
}

/** Keys whose values are document content: dropped. */
const CONTENT = new Set(["text", "files", "binaries", "expect", "detail", "glyphs", "g"]);

function clean(v: unknown, anon: (s: string) => string, depth = 0): unknown {
  if (typeof v === "string") return anon(v.length > 400 ? v.slice(0, 400) + "…" : v);
  if (typeof v !== "object" || v === null || depth > 4) return v;
  if (Array.isArray(v)) return v.slice(0, 40).map((x) => clean(x, anon, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(v)) {
    if (CONTENT.has(k)) out[k] = typeof x === "string" ? `(${x.length} chars, not included)` : "(not included)";
    else out[k] = clean(x, anon, depth + 1);
  }
  return out;
}

/** The report, as plain text. */
export function report(r: ReportInput): string {
  const anon = anonymizer(r.paths);
  const line = (e: { t: number; k: string; d?: unknown }) => `${String(e.t).padStart(8)} ms  ${e.k.padEnd(16)} ${e.d === undefined ? "" : JSON.stringify(clean(e.d, anon))}`;
  const exts = new Map<string, number>();
  for (const p of r.paths) {
    const x = p.match(/\.[A-Za-z0-9]+$/)?.[0] ?? "(none)";
    exts.set(x, (exts.get(x) ?? 0) + 1);
  }
  return [
    "PhiTeX Instant debug report (anonymized: no document text, no file names, no project id)",
    `extension ${r.version} · engine ${r.engine}`,
    `browser ${r.userAgent}`,
    `project: ${r.paths.length} files (${[...exts].map(([x, n]) => `${n} ${x}`).join(", ")})`,
    "",
    "errors:",
    ...(r.errors.length ? r.errors.map((e) => "  " + anon(e)) : ["  (none)"]),
    "",
    "status:",
    "  " + JSON.stringify(clean(r.status ?? {}, anon)),
    "",
    "diagnostics:",
    ...(r.diagnostics.length
      ? r.diagnostics.slice(0, 40).map((d) => `  ${d.severity} ${d.code ?? ""}: ${anon(d.message)}${d.file ? ` (${anon(d.file)}${d.line ? `:${d.line}` : ""})` : ""}`)
      : ["  (none)"]),
    "",
    "packages (TeX Live):",
    `  fetched: ${(r.packages?.done ?? []).join(", ") || "(none)"}`,
    `  failed: ${(r.packages?.failed ?? []).map((f) => `${f.name}${f.error ? ` (${anon(f.error)})` : ""}`).join(", ") || "(none)"}`,
    `  not in TeX Live: ${(r.packages?.unavailable ?? []).length}`,
    "",
    "trace (last 300 events):",
    ...r.trace.slice(-300).map(line),
  ].join("\n");
}
