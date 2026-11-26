// Diagnostics: what the preview can tell about the source and the build,
// as records (the shape a JSON mode, `--json`, would print). Source checks
// are heuristics on plain TeX's syntax: PhiTeX itself keeps going through
// incomplete input (an open group runs to the end), so these say *why* the
// preview looks as it does, at the line to look at. Pure: no DOM.

import type { Problem } from "./problems.ts";

export type Severity = "error" | "warning" | "info";

export interface Diagnostic {
  severity: Severity;
  /** Stable, for filtering and a JSON mode: "unclosed-brace", "latex", ... */
  code: string;
  /** The filters' kind, as the build named it (phitex-live's problems::kind_of, the editor's filter too); none: `kindOf` reads it off the message. */
  kind?: Kind;
  message: string;
  file?: string;
  /** 1-based. */
  line?: number;
  /** More, shown under the message as is (TeX's own context lines). */
  detail?: string;
  /** The whole problem, when the build told it (`phitex watch`'s diagnostics): shown as its card. */
  problem?: Problem;
}

/** A problem's kind, for the problems' filters (panel.ts): errors, then the warnings by what they are about. */
export type Kind = "error" | "overfull" | "underfull" | "reference" | "font" | "other";

/** The kinds, in the filters' order, with their names. */
export const KINDS: [Kind, string][] = [
  ["error", "Errors"],
  ["overfull", "Overfull"],
  ["underfull", "Underfull"],
  ["reference", "References"],
  ["font", "Fonts"],
  ["other", "Other"],
];

/** What `d` is about: its `kind` when the build gave it (phitex-live's problems::kind_of: one rule for the viewer and the editor), else read here the same way: an error; an overfull or underfull box (TeX's "Overfull \hbox", "Underfull \vbox", "Tight", "Loose"); an undefined reference or citation (LaTeX's, BibTeX's); a font (a missing character, a substitution); else other. */
export function kindOf(d: { severity: string; message: string; kind?: string }): Kind {
  if (d.kind && KINDS.some(([k]) => k === d.kind)) return d.kind as Kind;
  if (d.severity === "error" || d.severity === "fatal") return "error";
  const m = d.message;
  if (/^Overfull \\[hv]box|^Tight \\[hv]box/.test(m)) return "overfull";
  if (/^Underfull \\[hv]box|^Loose \\[hv]box/.test(m)) return "underfull";
  if (/[Rr]eference|[Cc]itation|database entry|undefined references|\bLabel\b/.test(m)) return "reference";
  if (/[Ff]ont|Missing character/.test(m)) return "font";
  return "other";
}

/** The build's first TeX error, as the core reads it off the terminal. */
export interface TexError {
  message: string;
  /** The file being read when it happened. */
  file: string | null;
  line: number | null;
  /** The error as TeX printed it, through the `l.N` line. */
  context: string;
  /** A file the error says was not found. */
  missing: string | null;
}

/** What the core's status reports (session.ts's scan). */
export interface BuildFacts {
  pages: number;
  pending?: number;
  undefinedNames?: string[];
  /** Packages the build read that neither the project nor the package source has. */
  unavailable?: string[];
  /** Where packages come from ("TeX Live 2025"); undefined: nowhere yet. */
  packageSource?: string;
  /** The core runs LaTeX (phitex's pdfTeX): no "LaTeX is dropped" or non-ASCII warnings. */
  latex?: boolean;
  /** The last build's first error. */
  texError?: TexError;
  /** Package downloads that failed, with why. */
  packageErrors?: { name: string; error: string }[];
}

function lineAt(text: string, i: number): number {
  let n = 1;
  for (let k = 0; k < i; k++) if (text.charCodeAt(k) === 10) n++;
  return n;
}

/** Positions of `{`/`}` and `$` outside comments and escapes, and the unmatched ones. */
function scan(text: string): { openBraces: number[]; strayBraces: number[]; openDollars: number[] } {
  const open: number[] = [];
  const stray: number[] = [];
  const dollars: number[] = [];
  let paraDollars: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") {
      i++; // (\{ \} \$ \% \\: escaped)
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
      if (text[i + 1] === "$") i++; // (display math: as one)
      paraDollars.push(i);
    } else if (c === "\n" && /^[ \t]*\n/.test(text.slice(i + 1, i + 40))) {
      // (a blank line ends a paragraph, and TeX's math with it)
      if (paraDollars.length % 2) dollars.push(paraDollars.at(-1)!);
      paraDollars = [];
    }
  }
  if (paraDollars.length % 2) dollars.push(paraDollars.at(-1)!);
  return { openBraces: open, strayBraces: stray, openDollars: dollars };
}

const LATEX = /\\(documentclass|usepackage|begin\{[A-Za-z*]+\}|end\{[A-Za-z*]+\}|section|subsection|maketitle|cite|ref|label|emph|textbf|textit)\b/g;

/** Diagnostics of the project's `files` (main `main`) and of its build. */
export function diagnose(files: Record<string, string>, main: string | null, build?: BuildFacts): Diagnostic[] {
  const out: Diagnostic[] = [];
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
        line: lineAt(text, at),
      });
    }
    for (const at of s.strayBraces.slice(0, 3))
      out.push({ severity: "warning", code: "stray-brace", message: "} with no { to close", file, line: lineAt(text, at) });
    for (const at of s.openDollars.slice(0, 3))
      out.push({ severity: "warning", code: "unclosed-math", message: "$ never closed in this paragraph", file, line: lineAt(text, at) });
    // (\def\x with no { on its line: the body is whatever comes next)
    for (const m of text.matchAll(/(^|[^\\])\\[gex]?def\s*\\([A-Za-z@]+)([^{\n%]*)$/gm))
      out.push({ severity: "warning", code: "def-no-body", message: `\\${m[2]} is defined with no { body } on its line`, file, line: lineAt(text, m.index! + m[1].length) });
    const ifs = [...text.matchAll(/\\if[a-z]*\b/g)];
    const fis = (text.match(/\\fi\b/g) ?? []).length;
    // (only where no macro could be closing them: files without \def)
    if (ifs.length > fis && !/\\[gex]?def/.test(text))
      out.push({ severity: "info", code: "unclosed-if", message: `${ifs.length - fis} \\if… without \\fi`, file, line: lineAt(text, ifs.at(-1)!.index!) });
    for (const m of text.matchAll(/\\input\s*\{?([^\s}\\%]+)/g)) {
      const n = m[1];
      if (!(n in files) && !(`${n}.tex` in files) && !/\.(aux|bbl|ind|toc)$/.test(n) && !n.startsWith("\\"))
        out.push({ severity: "warning", code: "missing-file", message: `\\input ${n}: no such file in the project`, file, line: lineAt(text, m.index!) });
    }
    const latex = build?.latex ? [] : [...new Set(text.match(LATEX) ?? [])];
    if (latex.length) {
      const first = text.search(LATEX);
      out.push({
        severity: "error",
        code: "latex",
        message: `LaTeX is not supported yet (dropped): ${latex.slice(0, 5).join(" ")}${latex.length > 5 ? " …" : ""}`,
        file,
        line: lineAt(text, first),
      });
    }
    let nonAscii = 0;
    let firstNon = -1;
    for (let i = 0; i < text.length; i++)
      if (text.charCodeAt(i) > 0x7f) {
        nonAscii++;
        if (firstNon < 0) firstNon = i;
      }
    if (nonAscii && !build?.latex)
      out.push({ severity: "info", code: "non-ascii", message: `${nonAscii} non-ASCII characters: PhiTeX's fonts may not typeset them`, file, line: lineAt(text, firstNon) });
  }
  if (main && files[main] !== undefined && !/\\(bye|end|enddocument)\b/.test(files[main]))
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
      message: e.missing
        ? `${e.missing} not found${stuck ? ` in the project or ${build.packageSource ?? "the package source"}` : failed ? " (its download failed, above)" : " (still downloading?)"}: the build stopped here`
        : e.message,
      file: e.file ?? undefined,
      line: e.line ?? undefined,
      detail: e.context,
    });
  }
  if (build) {
    if (build.pending)
      out.push({ severity: "warning", code: "pending", message: `${build.pending} part${build.pending === 1 ? "" : "s"} not read: out of fuel (a loop?) or unsupported` });
    if (build.undefinedNames?.length) out.push({ severity: "warning", code: "undefined", message: `undefined: ${build.undefinedNames.join(" ")}` });
    // (most are optional files LaTeX only looks for: .cfg, hooks; the one a
    // build stopped at is the error above)
    // (`x.tex` is TeX's first try at `x`: one name, as the source says it)
    const optional = [...new Set(build.unavailable?.filter((n) => n !== build.texError?.missing).map((n) => n.replace(/(\.[a-z]+)\.tex$/, "$1")))];
    if (optional.length) {
      const names = optional.slice(0, 6).join(" ") + (optional.length > 6 ? " …" : "");
      out.push({
        severity: "info",
        code: "package-unavailable",
        message: build.packageSource
          ? `looked for and not found in ${build.packageSource} (usually optional): ${names}`
          : `packages are not downloaded yet: ${names}`,
      });
    }
    if (build.pages === 0) out.push({ severity: "warning", code: "no-pages", message: "no page shipped yet" });
  }
  const rank = { error: 0, warning: 1, info: 2 };
  return out.sort((a, b) => rank[a.severity] - rank[b.severity]);
}

/**
 * The build's errors as a reader wants them (the drawer, the strip, the card over the pages, a
 * host's editor marks): the cause first, in plain words, its consequences folded under it.
 * - An undefined control sequence names the command, its culprit the command itself:
 *   "Undefined command \foobar".
 * - A brace left open says so, where: "Missing } after \section{Results", never the internal
 *   macro TeX was reading ("File ended while scanning use of \@xdblarg").
 * - TeX's interactive help ("type `I' and the correct spelling…") is for its terminal, not
 *   here: dropped, a one-line hint of ours where we have one.
 * - TeX stopping because of an error ("*** (job aborted, no legal \end found)", "Emergency
 *   stop") is that error's consequence: a note on it, not an error of its own listed first.
 * - LaTeX's internal names (\@sect, \@xdblarg) are said as what they are.
 */
export function plainDiagnostics<D extends Diagnostic>(ds: readonly D[]): D[] {
  const out: D[] = [];
  const stops: string[] = [];
  for (const d0 of ds) {
    if (d0.severity === "error" && STOP.test(d0.message)) {
      stops.push(stopWords(d0.message));
      continue;
    }
    out.push(d0.severity === "error" ? plainError(d0) : d0);
  }
  const cause = out.find((d) => d.severity === "error");
  if (stops.length && cause) {
    const note = `Then TeX stopped: ${stops[0]}.`;
    if (cause.problem) cause.problem = { ...cause.problem, notes: [...cause.problem.notes, note] };
    else cause.detail = cause.detail ? `${cause.detail}\n${note}` : note;
  } else if (stops.length) {
    // (no cause told: the stop said in plain words, still an error)
    const first = ds.find((d) => d.severity === "error" && STOP.test(d.message))!;
    out.unshift(plainError({ ...first, message: `TeX stopped: ${stops[0]}` }));
  }
  return out;
}

/** TeX giving up, after an error said before it. */
const STOP = /^\*\*\* \(job aborted|^Emergency stop|^==> Fatal error|Fatal error occurred|^\*\*\* \(cannot \\read/;

function stopWords(m: string): string {
  if (/no legal \\end found/.test(m)) return "the file ended with no \\end{document}";
  if (/^Emergency stop/.test(m)) return "it could not go on";
  return "it could not go on";
}

/** LaTeX's internal macros (\@sect, \@xdblarg) said as what they are. */
const internal = (s: string) => s.replace(/\\@[A-Za-z@]*/g, "a LaTeX internal command");

/** TeX's own help for its terminal ("type `I'…", "I'll forget about whatever was undefined"). */
const TEX_HELP = /type [`'‘]?I[`'’]?|I'll |I'm |I've |Otherwise just continue|Try typing|Type\s+<return>|\\def'ed|misspelled/i;

function plainError<D extends Diagnostic>(d: D): D {
  const p = d.problem;
  const ex = p?.excerpt ?? null;
  let message = d.message, hint: string | null = null, mark: [number, number] | null = null;
  if (/^Undefined control sequence/.test(message)) {
    const cmd = ex ? commandAt(ex.text, ex.start, ex.len) : null;
    if (cmd) (message = `Undefined command ${cmd.name}`), (mark = [cmd.at, cmd.name.length]);
    else message = "Undefined command";
    hint = "Check its spelling, or load the package that defines it.";
  } else if (/^File ended while scanning use of|^Runaway argument|^Paragraph ended before .* was complete/.test(message)) {
    const open = ex ? openBrace(ex.text) : null;
    message = open ? `Missing } after ${open.text}` : "Missing }: a brace opened and never closed";
    if (open) mark = [open.at, [...open.text].length];
    hint = "Close the brace where the argument ends.";
  } else message = internal(message);
  const r = { ...d, message } as D;
  if (d.detail && TEX_HELP.test(d.detail)) r.detail = hint ?? undefined;
  else if (hint && !d.detail) r.detail = undefined;
  if (p) {
    r.problem = {
      ...p,
      message,
      help: hint ? [hint] : p.help.filter((h) => !TEX_HELP.test(h)),
      // (TeX's "not defined" and the like: said by the message now)
      notes: hint ? [] : p.notes.map(internal),
      excerpt: ex && mark ? { text: ex.text, start: mark[0], len: mark[1] } : ex,
    };
  }
  return r;
}

/**
 * The undefined command in `text`: the culprit TeX marked ([start, start + len)) when it is one,
 * else the last command before its end (TeX's top line ends at it), else the first after it.
 */
function commandAt(text: string, start: number, len: number): { name: string; at: number } | null {
  const chars = [...text];
  const mark = chars.slice(start, start + len).join("");
  const own = /^\\(?:[A-Za-z@]+|.)/.exec(mark);
  if (own) return { name: own[0], at: start };
  const upto = chars.slice(0, Math.min(chars.length, Math.max(0, start + len))).join("");
  const all = [...upto.matchAll(/\\(?:[A-Za-z@]+|[^A-Za-z@\s])/g)];
  const last = all[all.length - 1];
  if (last && last.index! + last[0].length >= upto.trimEnd().length - 1) return { name: last[0], at: [...upto.slice(0, last.index)].length };
  const after = /\\[A-Za-z@]+/.exec(chars.slice(start + len).join(""));
  if (after) return { name: after[0], at: start + len + [...chars.slice(start + len).join("").slice(0, after.index)].length };
  return last ? { name: last[0], at: [...upto.slice(0, last.index)].length } : null;
}

/** The last brace in `text` never closed, and the command it belongs to: "\section{Results". */
function openBrace(text: string): { text: string; at: number } | null {
  const chars = [...text];
  const stack: number[] = [];
  for (let i = 0; i < chars.length; i++) {
    if (chars[i] === "\\") i++;
    else if (chars[i] === "{") stack.push(i);
    else if (chars[i] === "}") stack.pop();
  }
  const b = stack.length ? stack[stack.length - 1] : -1;
  if (b < 0) return null;
  // (the command before it, its options too: \section[short]{Results)
  const before = chars.slice(0, b).join("");
  const m = /\\[A-Za-z@]+\*?(?:\[[^\]]*\])?\s*$/.exec(before);
  const at = m ? [...before.slice(0, m.index)].length : b;
  const tail = chars.slice(at).join("").trimEnd();
  return { text: tail.length > 48 ? `${[...tail].slice(0, 47).join("")}…` : tail, at };
}
