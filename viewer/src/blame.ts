// Blame on the page: who last changed what the pages show, drawn over them
// (an overlay only: no build, no new TeX run, the PDF as it is). A bar in
// the margin of each printed line, coloured by the line's commit (its age,
// its author, or whether it changed since a tag, a date or a commit), an
// optional tint over the words, and a card on hover: the commit's subject,
// hash, author, age and source lines, with Compare with before this commit,
// Show in editor and Copy hash. Text not committed yet is hatched grey.
//
// Where a printed line came from is the pages' glyph origins (sync.ts):
// each glyph's source bytes, a line of its file; a macro's glyphs are its
// call's, so text a macro made is blamed at the macro's use. A float's
// picture has no glyphs: its caption's lines are blamed as text, and the
// space above them as the picture's file (its last change: an image, or
// the lines of a TikZ file it inputs). A bibliography entry is blamed as
// its .bib entry, through the .bbl's \bibitem keys.
//
// The history is the host's (BlameHost): `phitex watch` and VS Code read
// git (`git blame`), Overleaf its project history. The view's state and
// drawing are here, the same in every host.

import { html } from "./html.ts";
import type { Glyph } from "./sync.ts";

/** A commit (Overleaf: a version) a blame names. `time`: seconds since the epoch. */
export interface BlameCommit {
  id: string;
  short: string;
  author: string;
  email: string;
  time: number;
  subject: string;
  /** Its first parent (the version before it), where the host knows it: "What this commit changed" diffs against it. */
  parent?: string;
  /** The version "What this commit changed" diffs to, where it is not `id` (Overleaf: a session's, its author's commit being one of several in it). */
  rev?: string;
}

/**
 * A file's blame: a text file's commit for each line (an index into the
 * commits, -1 not committed yet) with its text as blamed (the glyph
 * origins' bytes are of it); a binary one's (a figure's image) last change.
 */
export interface BlameFile {
  text?: string;
  lines?: number[];
  /**
   * Finer than lines, where the host has it (Overleaf's history, to the
   * character; git, to the word): runs [from, to) of `text` (UTF-16
   * offsets), each with its commit index (-1 not committed), in order,
   * covering the text. Lines are derived from them where `lines` is absent.
   */
  runs?: [number, number, number][];
  last?: number;
  error?: string;
}

/** What a blame is worked out with: the host's part of the look (BlameLook). */
export interface BlameOpts {
  /** Line: each line its newest edit, as git blame does; Word: each word its writer; Char: each character. */
  detail: "line" | "word" | "char";
  /** Edits of spacing only: ignored (the text keeps its writer) or counted. */
  ws: "ignore" | "count";
  /** Text moved: keeps its writer, or is the mover's. */
  moved: "keep" | "mover";
  /** Commits skipped: their lines go to whoever wrote them before (git's --ignore-rev). */
  skip: string[];
  /** Said while it works ("Reading history… 340 of ~1,200 edits"): `done` of `total` where it is known. */
  progress?(text: string, done?: number, total?: number): void;
  /** Aborted when the reads are no longer wanted (the project left). */
  signal?: AbortSignal;
}

/** A tag or a branch, for "changed since". */
export interface BlameRef {
  name: string;
  kind: "tag" | "branch" | string;
  time: number;
  /** The commit it names, where the host says. */
  id?: string;
}

/** The host's answer (the `blame` op's, docs/spec-editor-protocol.md §10). */
export interface BlameData {
  commits: BlameCommit[];
  files: Record<string, BlameFile>;
  refs?: BlameRef[];
  /** The host's clock, seconds since the epoch (ages are from it). */
  now?: number;
  /** The history's head it was worked out at (git's HEAD), where the host has one. */
  head?: string;
}

/** What blame needs of its host. */
export interface BlameHost {
  /** Files `files` (project paths, as the glyph origins name them) blamed, worked out with `opts`, or why not ("no git history here"). */
  blame(files: string[], opts?: BlameOpts): Promise<BlameData | string>;
  /** Commit `c`'s branch, where it was merged into main: "On lina/related-work, merged into main (5d0a7e3)"; null: on main. */
  via?(c: BlameCommit): string | null;
  /** Page `k`'s glyphs (the session's): the pages are where the blame is drawn, never what it is worked out from. */
  glyphs(k: number): Promise<Glyph[]>;
  /**
   * The project's sources to blame: the main file and what it inputs, else
   * every .tex and .bib (sourceFiles). Absent: the host picks them itself,
   * asked with no files (the watch's blame op).
   */
  sources?(): string[] | Promise<string[]>;
  /** A file's text as the editor (or the disk) has it now, where the host knows it: a file blamed again only once it changed. */
  text?(file: string): string | undefined | Promise<string | undefined>;
  /** The history's head now (git's HEAD): moved since the blame, every file blamed again. */
  head?(): Promise<string | null>;
  /** The editor at `file`'s `line` (from 1). */
  goto(file: string, line: number): void;
  /** What `c` changed (null: text not committed, against the last commit): Changes' Diff of it. Absent: not offered. */
  compare?(c: BlameCommit | null): void;
  /** An author's colour where the host gives them one (Overleaf's collaborators); null: by the hash of the email. */
  color?(c: BlameCommit): string | null;
  /** The host's word for a commit ("commit"; Overleaf: "version"), and whether it has hashes to copy. */
  words?: { commit?: string; hashes?: boolean };
}


// ---- colours ----

/** Not committed: hatched grey. */
export const UNCOMMITTED = "#9AA0A6";
/** The age steps, newest strongest; light pages' bar, dark pages', the tint's weight. Older than six months: none. */
export const AGE: { name: string; days: number; bar: [string, string] | null; tint: number }[] = [
  { name: "Under a day", days: 1, bar: ["#5B2A86", "#C7A6F0"], tint: 0.22 },
  { name: "Under a week", days: 7, bar: ["#7B4FAE", "#AE8AD8"], tint: 0.16 },
  { name: "Under a month", days: 31, bar: ["#A07CC9", "#8E6CB8"], tint: 0.1 },
  { name: "Under 6 months", days: 183, bar: ["#C9B5E2", "#6B5390"], tint: 0.05 },
  { name: "Older", days: Infinity, bar: null, tint: 0 },
];
/** Okabe–Ito without its yellow (unreadable on white): one an author, by a hash of the email. */
export const AUTHOR_COLORS = ["#E69F00", "#56B4E9", "#009E73", "#CC79A7", "#0072B2", "#D55E00"];
/** More authors than colours: grey. */
export const OVERFLOW = "#8A8F98";
/** Changed since: one violet, light pages' and dark's. */
export const SINCE: [string, string] = ["#5B2A86", "#C7A6F0"];

/** The age step of a commit `days` old. */
export function ageStep(days: number): number {
  return AGE.findIndex((a) => days < a.days);
}

/** FNV-1a of a string (an email, lower-cased): an author's colour the same on every page and every machine. */
export function fnv(s: string): number {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(s)) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return h;
}

/**
 * Each author's colour (by email, lower-cased; the name where there is
 * none): the colour its hash names, else the next one free; once all six
 * are taken, grey. Authors are placed in order of their keys, so the same
 * people get the same colours however the pages are read.
 */
export function authorColors(keys: Iterable<string>): Map<string, string> {
  const out = new Map<string, string>();
  const taken = new Set<number>();
  for (const k of [...new Set(keys)].sort()) {
    let i = fnv(k) % AUTHOR_COLORS.length;
    let n = 0;
    while (taken.has(i) && n < AUTHOR_COLORS.length) (i = (i + 1) % AUTHOR_COLORS.length), n++;
    if (n === AUTHOR_COLORS.length) out.set(k, OVERFLOW);
    else (taken.add(i), out.set(k, AUTHOR_COLORS[i]));
  }
  return out;
}

/** An author's key: the email lower-cased, else the name. */
export const authorKey = (c: BlameCommit) => (c.email || c.author).trim().toLowerCase();

// ---- the source: lines, floats, bibliography ----

/** The byte offsets (UTF-8) of each UTF-16 index of `text` (one more for its end). */
export function byteIndex(text: string): Uint32Array {
  const out = new Uint32Array(text.length + 1);
  let b = 0;
  for (let i = 0; i < text.length; i++) {
    out[i] = b;
    const c = text.charCodeAt(i);
    if (c < 0x80) b += 1;
    else if (c < 0x800) b += 2;
    else if (c >= 0xd800 && c < 0xdc00 && i + 1 < text.length) (out[++i] = b), (b += 4);
    else b += 3;
  }
  out[text.length] = b;
  return out;
}

/** The byte offsets lines start at (the first, 0): a line ends at its newline, as git counts them. */
export function lineStarts(text: string): number[] {
  const out = [0];
  let b = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    b += c < 0x80 ? 1 : c < 0x800 ? 2 : c >= 0xd800 && c < 0xdc00 ? (i++, 4) : 3;
    if (c === 10) out.push(b);
  }
  return out;
}

/** The line (from 0) byte `at` is on. */
export function lineOf(starts: number[], at: number): number {
  let lo = 0, hi = starts.length;
  while (hi - lo > 1) {
    const m = (lo + hi) >> 1;
    if (starts[m] <= at) lo = m;
    else hi = m;
  }
  return lo;
}

/** A float in a source file: its bytes, its caption's, the files it shows (images, inputs). */
export interface Float {
  file: string;
  from: number;
  to: number;
  caption: [number, number] | null;
  images: string[];
  inputs: string[];
}

/** The end of the brace group opening at `i` (text[i] === "{"): the index after its "}", or -1. */
function group(text: string, i: number): number {
  let d = 0;
  for (let j = i; j < text.length; j++) {
    const c = text[j];
    if (c === "\\") j++;
    else if (c === "%") while (j < text.length && text[j] !== "\n") j++;
    else if (c === "{") d++;
    else if (c === "}" && --d === 0) return j + 1;
  }
  return -1;
}

/** The floats of `file`'s `text`: figure and table environments (starred, wrapped, sidecaps), with their captions, images and inputs. */
export function floats(file: string, text: string): Float[] {
  const out: Float[] = [];
  const bytes = byteIndex(text);
  const begin = /\\begin\s*\{(figure\*?|table\*?|wrapfigure|wraptable|SCfigure|SCtable|subfigure)\}/g;
  for (let m; (m = begin.exec(text)); ) {
    const end = text.indexOf(`\\end{${m[1]}}`, m.index);
    if (end < 0) continue;
    const body = text.slice(m.index, end);
    const images: string[] = [], inputs: string[] = [];
    for (const g of body.matchAll(/\\includegraphics\*?\s*(?:\[[^\]]*\]\s*)?\{([^}]+)\}/g)) images.push(g[1].trim());
    for (const g of body.matchAll(/\\(?:input|include|includesvg|includestandalone)\s*(?:\[[^\]]*\]\s*)?\{([^}]+)\}/g)) inputs.push(g[1].trim());
    let caption: [number, number] | null = null;
    const c = /\\caption\*?\s*(?:\[[^\]]*\]\s*)?\{/.exec(body);
    if (c) {
      const open = m.index + c.index + c[0].length - 1;
      const close = group(text, open);
      if (close > 0) caption = [bytes[m.index + c.index], bytes[close]];
    }
    out.push({ file, from: bytes[m.index], to: bytes[end + `\\end{${m[1]}}`.length], caption, images, inputs });
    begin.lastIndex = end;
  }
  return out;
}

/** The names an image may have on disk, `\graphicspath`'s folders tried first: `fig/a` is `fig/a.pdf`, `.png`, … */
export function imageNames(name: string, paths: string[] = []): string[] {
  const exts = /\.[A-Za-z0-9]{2,5}$/.test(name) ? [""] : [".pdf", ".png", ".jpg", ".jpeg", ".eps"];
  const dirs = [...paths, ""];
  return dirs.flatMap((d) => exts.map((e) => `${d}${name}${e}`));
}

/** An input's names on disk: `fig/a` is `fig/a.tex`, or itself. */
export function inputNames(name: string): string[] {
  return /\.[A-Za-z0-9]{1,5}$/.test(name) ? [name] : [`${name}.tex`, name];
}

/** `\graphicspath{{a/}{b/}}`'s folders in `text`. */
export function graphicsPaths(text: string): string[] {
  const m = /\\graphicspath\s*\{((?:\s*\{[^}]*\})*)\s*\}/.exec(text);
  return m ? [...m[1].matchAll(/\{([^}]*)\}/g)].map((g) => g[1]).filter(Boolean) : [];
}

/** The .bib files `text` names (`\bibliography{a,b}`, `\addbibresource{x.bib}`). */
export function bibFiles(text: string): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/\\bibliography\s*\{([^}]+)\}/g)) for (const n of m[1].split(",")) out.push(/\.bib$/.test(n.trim()) ? n.trim() : `${n.trim()}.bib`);
  for (const m of text.matchAll(/\\addbibresource\s*(?:\[[^\]]*\]\s*)?\{([^}]+)\}/g)) out.push(m[1].trim());
  return out.filter((n) => n && n !== ".bib");
}

/** A .bbl's entries: each `\bibitem`'s byte and key, in order. */
export function bibItems(text: string): [number, string][] {
  const bytes = byteIndex(text);
  return [...text.matchAll(/\\bibitem\s*(?:\[(?:[^\]{}]|\{[^}]*\})*\]\s*)?\{([^}]+)\}/g)].map((m) => [bytes[m.index!], m[1].trim()] as [number, string]);
}

/** A .bib's entries by key: their first and last lines (from 0). */
export function bibEntries(text: string): Map<string, [number, number]> {
  const out = new Map<string, [number, number]>();
  const at = [...text.matchAll(/^[ \t]*@\s*(\w+)\s*[{(]\s*([^,\s]+)\s*,/gm)];
  const lineAt = (i: number) => text.slice(0, i).split("\n").length - 1;
  at.forEach((m, j) => {
    if (/^(comment|string|preamble)$/i.test(m[1])) return;
    const next = j + 1 < at.length ? at[j + 1].index! : text.length;
    const body = text.slice(m.index!, next).replace(/\s+$/, "");
    out.set(m[2], [lineAt(m.index!), lineAt(m.index! + body.length)]);
  });
  return out;
}

/** Whether a UTF-16 unit is part of a word (letters, digits, marks; a TeX command's letters too). */
const WORD = /[\p{L}\p{N}\p{M}_@\\']/u;

/**
 * Runs at word detail: each word (`WORD`'s characters in a row) takes its
 * newest writer, as a whole; spaces and punctuation keep their own.
 * `newer(a, b)`: the newer of two commits.
 */
export function wordRuns(text: string, runs: [number, number, number][], newer: (a: number, b: number) => number): [number, number, number][] {
  const at = new Int32Array(text.length);
  for (const [a, b, c] of runs) at.fill(c, a, b);
  for (let i = 0; i < text.length; ) {
    if (!WORD.test(text[i])) {
      i++;
      continue;
    }
    let j = i, c = at[i];
    while (j < text.length && WORD.test(text[j])) c = newer(c, at[j++]);
    at.fill(c, i, j);
    i = j;
  }
  return compress(at);
}

/** Per-unit commits as runs. */
function compress(at: Int32Array): [number, number, number][] {
  const out: [number, number, number][] = [];
  for (let i = 0; i < at.length; i++) {
    const r = out.at(-1);
    if (r && r[2] === at[i] && r[1] === i) r[1] = i + 1;
    else out.push([i, i + 1, at[i]]);
  }
  return out;
}

/**
 * Each line's commit from runs: its newest writer, as git blame says it;
 * spacing ignored (`ws`), the newest of its other characters (a line of
 * spaces only: of all of them). A line ends at its newline, which is
 * counted with it.
 */
export function linesOf(text: string, runs: [number, number, number][], ws: "ignore" | "count", newer: (a: number, b: number) => number): number[] {
  const at = new Int32Array(text.length);
  for (const [a, b, c] of runs) at.fill(c, a, b);
  const out: number[] = [];
  let start = 0;
  const end = (i: number) => {
    let c: number | undefined, any: number | undefined;
    for (let k = start; k < i; k++) {
      any = any === undefined ? at[k] : newer(any, at[k]);
      if (ws === "ignore" && /\s/.test(text[k])) continue;
      c = c === undefined ? at[k] : newer(c, at[k]);
    }
    out.push(c ?? any ?? -1);
  };
  for (let i = 0; i < text.length; i++)
    if (text[i] === "\n") {
      end(i + 1);
      start = i + 1;
    }
  if (start < text.length) end(text.length);
  return out;
}

/**
 * The project's sources: the main file and everything it inputs (\\input,
 * \\include, \\subfile, \\import, \\bibliography, \\addbibresource), in
 * the order they come, followed from each input; without a main file, every
 * .tex and .bib. `names` the project's files, `text` a file's text.
 */
export function sourceFiles(main: string | null | undefined, names: string[], text: (f: string) => string | undefined): string[] {
  const has = new Set(names);
  if (!main || !has.has(main)) return names.filter((n) => /\.(tex|ltx|bib)$/i.test(n));
  const find = (n: string, ext: string): string | null => {
    const s = n.trim().replace(/^\.\//, "");
    for (const c of [s, `${s}${ext}`]) if (has.has(c)) return c;
    return null;
  };
  const out: string[] = [];
  const todo = [main];
  while (todo.length) {
    const f = todo.shift()!;
    if (out.includes(f)) continue;
    out.push(f);
    const t = (text(f) ?? "").replace(/(^|[^\\])%.*$/gm, "$1");
    for (const m of t.matchAll(/\\(input|include|subfile|subfileinclude|bibliography|addbibresource|import\*?\{([^}]*)\})\s*(?:\[[^\]]*\])?\{([^}]+)\}/g)) {
      const bib = m[1] === "bibliography" || m[1] === "addbibresource";
      for (const n of bib ? m[3].split(",") : [m[3]]) {
        const x = find(m[2] !== undefined ? `${m[2]}${n}` : n, bib ? ".bib" : ".tex");
        if (x && !out.includes(x) && !todo.includes(x)) todo.push(x);
      }
    }
  }
  return out;
}

/** A file made ready to look up: its line starts, and its runs in bytes (where the detail is finer than lines). */
type Ready = BlameFile & { starts?: number[]; br?: { at: Uint32Array; c: Int32Array }; units?: [number, number, number][] };

/** A blame's files made ready to look up: each text's line starts, floats, and the bibliography's keys. */
export class Model {
  commits: BlameCommit[] = [];
  refs: BlameRef[] = [];
  now = Date.now() / 1000;
  files = new Map<string, Ready>();
  floats: Float[] = [];
  private byId = new Map<string, number>();
  private bib = new Map<string, { file: string; lines: [number, number] }>();
  private bbls = new Map<string, [number, string][]>();
  /** The detail lines are looked up at, and spacing's. */
  detail: BlameOpts["detail"] = "line";
  ws: BlameOpts["ws"] = "ignore";

  /** `d` taken in: its commits merged with those already here by id; a file blamed again replaces its own. */
  add(d: BlameData): void {
    const map = d.commits.map((c) => {
      let k = this.byId.get(c.id);
      if (k === undefined) (k = this.commits.push(c) - 1), this.byId.set(c.id, k);
      return k;
    });
    const re = (i: number | undefined) => (i === undefined ? undefined : i < 0 ? -1 : (map[i] ?? -1));
    if (d.now) this.now = d.now;
    if (d.refs) this.refs = d.refs;
    const newer = (a: number, b: number) => this.newer(a, b);
    for (const [name, f] of Object.entries(d.files)) {
      if (this.files.has(name)) {
        this.floats = this.floats.filter((x) => x.file !== name);
        this.bbls.delete(name);
        for (const [k, e] of this.bib) if (e.file === name) this.bib.delete(k);
      }
      const g: Ready = { ...f, lines: f.lines?.map((l) => re(l)!), runs: f.runs?.map(([a, b, c]) => [a, b, re(c)!] as [number, number, number]), last: re(f.last) };
      if (g.text !== undefined && g.runs) {
        g.lines ??= linesOf(g.text, g.runs, this.ws, newer);
        if (this.detail !== "line") {
          const runs = this.detail === "word" ? wordRuns(g.text, g.runs, newer) : g.runs;
          g.units = runs;
          const bytes = byteIndex(g.text);
          g.br = { at: Uint32Array.from(runs, (r) => bytes[r[0]]), c: Int32Array.from(runs, (r) => r[2]) };
        }
      }
      if (f.text !== undefined) g.starts = lineStarts(f.text);
      this.files.set(name, g);
      if (f.text !== undefined && /\.tex$|\.ltx$/i.test(name)) this.floats.push(...floats(name, f.text));
      if (f.text !== undefined && /\.bbl$/i.test(name)) this.bbls.set(name, bibItems(f.text));
      if (f.text !== undefined && /\.bib$/i.test(name)) for (const [key, lines] of bibEntries(f.text)) this.bib.set(key, { file: name, lines });
    }
  }

  /** The files a page's glyphs need besides their own: the floats' images and inputs, the .bib files, as names to try. */
  wanted(): string[] {
    const texts = [...this.files.values()].map((f) => f.text ?? "");
    const paths = texts.flatMap(graphicsPaths);
    const out = new Set<string>();
    for (const f of this.floats) {
      for (const i of f.images) imageNames(i, paths).forEach((n) => out.add(n));
      for (const i of f.inputs) inputNames(i).forEach((n) => out.add(n));
    }
    for (const t of texts) bibFiles(t).forEach((n) => out.add(n));
    for (const n of this.files.keys()) out.delete(n);
    return [...out];
  }

  /**
   * Who wrote the sources, counted in the detail's units: lines (not blank
   * ones), words, or characters (not spacing); each commit's count. A
   * .bbl is the build's, not a source: not counted; nor an image (no text).
   */
  counts(): Map<number, number> {
    const out = new Map<number, number>();
    const add = (c: number, n: number) => n && out.set(c, (out.get(c) ?? 0) + n);
    for (const [name, f] of this.files) {
      if (f.text === undefined || f.error || /\.bbl$/i.test(name)) continue;
      const t = f.text;
      if (this.detail === "line" || !f.units) {
        const ls = f.lines ?? [];
        const starts = f.starts ?? lineStarts(t);
        ls.forEach((c, i) => {
          const a = starts[i] ?? 0, b = starts[i + 1] ?? t.length;
          if (/\S/.test(t.slice(a, b))) add(c, 1);
        });
      } else if (this.detail === "word") {
        for (const [a, b, c] of f.units) add(c, (t.slice(a, b).match(/[\p{L}\p{N}_]+/gu) ?? []).length);
      } else for (const [a, b, c] of f.units) add(c, t.slice(a, b).replace(/\s+/g, "").length);
    }
    return out;
  }

  time(c: number): number {
    return c < 0 ? Infinity : (this.commits[c]?.time ?? -Infinity);
  }

  /** The newer of two commits (-1, not committed, newest of all). */
  newer(a: number | undefined, b: number): number {
    return a === undefined || this.time(b) > this.time(a) ? b : a;
  }

  /** The newest commit of `file`'s lines [a, b] (from 0); undefined: none blamed. */
  newest(file: string, a: number, b: number): number | undefined {
    const ls = this.files.get(file)?.lines;
    if (!ls) return undefined;
    let best: number | undefined;
    for (let i = a; i <= b && i < ls.length; i++) best = this.newer(best, ls[i]);
    return best;
  }

  /** Glyph `g`'s commit, file and line (from 0): its source line's; a .bbl's, its .bib entry's newest. */
  of(g: Glyph): { c: number; file: string; line: number; last: number } | null {
    if (!g.file) return null;
    const bbl = this.bbls.get(g.file);
    if (bbl) {
      let key: string | undefined;
      for (const [at, k] of bbl) if (at <= g.start) key = k;
      const e = key ? this.bib.get(key) : undefined;
      const c = e ? this.newest(e.file, e.lines[0], e.lines[1]) : undefined;
      return e && c !== undefined ? { c, file: e.file, line: e.lines[0], last: e.lines[1] } : null;
    }
    const f = this.files.get(g.file);
    if (!f?.lines || !f.starts) return null;
    const line = lineOf(f.starts, g.start);
    if (f.br) {
      // (finer than lines: the run the glyph's byte is in)
      let lo = 0, hi = f.br.at.length;
      while (hi - lo > 1) {
        const m = (lo + hi) >> 1;
        if (f.br.at[m] <= g.start) lo = m;
        else hi = m;
      }
      return { c: f.br.c[lo] ?? -1, file: g.file, line, last: line };
    }
    return { c: f.lines[line] ?? -1, file: g.file, line, last: line };
  }

  /** The commit a float's picture is blamed on: its images' last changes, its inputs' lines, else the float's own lines but its caption's. */
  floatCommit(fl: Float): { c: number; src: string } | null {
    const texts = [...this.files.values()].map((f) => f.text ?? "");
    const paths = texts.flatMap(graphicsPaths);
    let best: number | undefined, src = "";
    for (const i of fl.images)
      for (const n of imageNames(i, paths)) {
        const f = this.files.get(n);
        if (!f || f.error) continue;
        const c = f.last ?? (f.lines ? this.newest(n, 0, f.lines.length - 1) : undefined);
        if (c !== undefined && (best === undefined || this.newer(best, c) === c)) (best = c), (src = n);
        break;
      }
    for (const i of fl.inputs)
      for (const n of inputNames(i)) {
        const f = this.files.get(n);
        if (!f?.lines) continue;
        const c = this.newest(n, 0, f.lines.length - 1);
        if (c !== undefined && (best === undefined || this.newer(best, c) === c)) (best = c), (src = n);
        break;
      }
    if (best !== undefined) return { c: best, src };
    const f = this.files.get(fl.file);
    if (!f?.lines || !f.starts) return null;
    const a = lineOf(f.starts, fl.from), b = lineOf(f.starts, Math.max(fl.from, fl.to - 1));
    const [ca, cb] = fl.caption ? [lineOf(f.starts, fl.caption[0]), lineOf(f.starts, Math.max(fl.caption[0], fl.caption[1] - 1))] : [-1, -1];
    for (let l = a; l <= b; l++) if (l < ca || l > cb) best = this.newer(best, f.lines[l] ?? -1);
    return best === undefined ? null : { c: best, src: `${fl.file}:${a + 1}–${b + 1}` };
  }
}

// ---- the pages: printed lines ----

/**
 * A printed line (or a float's picture): its box and its bar's place (PDF
 * points from the page's top left), its commit (-1 not committed), its
 * source (file, lines from 0), its words' runs by commit (for the tint),
 * and its paragraph: the lines in a row of a column with one commit.
 */
export interface Seg {
  x0: number;
  x1: number;
  y0: number;
  y1: number;
  /** The bar's left edge. */
  bx: number;
  c: number;
  file: string;
  l0: number;
  l1: number;
  runs: [number, number, number][];
  /** A float's picture: the file it is blamed as. */
  fig?: string;
  para: number;
}

/**
 * A page's glyphs as printed lines: the glyphs within a point of one
 * baseline (a rounded baseline split a line where its glyphs' places
 * straddled the rounding), left to right, split where the next glyph is a
 * column away (18 pt).
 */
export function printedLines(gs: Glyph[]): Glyph[][] {
  const rows: Glyph[][] = [];
  for (const g of [...gs].sort((a, b) => a.y - b.y)) {
    const r = rows.at(-1);
    if (r && g.y - r[0].y < 1) r.push(g);
    else rows.push([g]);
  }
  const out: Glyph[][] = [];
  for (const r of rows) {
    r.sort((a, b) => a.x - b.x);
    let l: Glyph[] = [];
    for (const g of r) {
      if (l.length && g.x - l.at(-1)!.x >= 18) out.push(l), (l = []);
      l.push(g);
    }
    out.push(l);
  }
  return out;
}

/**
 * The left edges of a page's columns: the starts of three or more lines,
 * 40 pt apart or more (a paragraph's indent, a list's, are in their
 * column: its bars in one line).
 */
export function columnStarts(x0s: number[]): number[] {
  const xs = [...x0s].sort((a, b) => a - b);
  const groups: number[][] = [];
  for (const x of xs) {
    const g = groups.at(-1);
    if (g && x - g[0] < 40) g.push(x);
    else groups.push([x]);
  }
  return groups.filter((g) => g.length >= 3).map((g) => g[0]);
}

/** A page's printed lines blamed (`size`: the page in points). */
export function mapPage(gs: Glyph[], m: Model, size: { w: number; h: number }): Seg[] {
  type Raw = Seg & { glyphs: Glyph[]; srcs: { file: string; start: number }[] };
  const raw: Raw[] = [];
  for (const line of printedLines(gs)) {
    // (a number a macro set alone, a page's, a section's or an equation's: no line of text to blame)
    if (line.length <= 4 && line.every((g) => g.synth)) continue;
    let c: number | undefined, file = "", l0 = 0, l1 = 0;
    const runs: [number, number, number][] = [];
    line.forEach((g, i) => {
      const at = m.of(g);
      const next = line[i + 1];
      const w = next && next.x > g.x && next.x - g.x < 15 ? next.x - g.x : 5;
      if (!at) return;
      c = m.newer(c, at.c);
      if (!file) (file = at.file), (l0 = at.line), (l1 = at.last);
      else if (file === at.file) (l0 = Math.min(l0, at.line)), (l1 = Math.max(l1, at.last));
      const r = runs.at(-1);
      if (r && r[2] === at.c && g.x - r[1] < 6) r[1] = g.x + w;
      else runs.push([g.x, g.x + w, at.c]);
    });
    if (c === undefined) continue;
    const x0 = line[0].x, last = line.at(-1)!;
    raw.push({ x0, x1: last.x + 5, y0: line[0].y - 8, y1: line[0].y + 3, bx: 0, c, file, l0, l1, runs, para: 0, glyphs: line, srcs: line.filter((g) => g.file).map((g) => ({ file: g.file!, start: g.start })) });
  }
  const cols = columnStarts(raw.map((s) => s.x0));
  const colOf = (x: number) => cols.filter((c) => c <= x + 1).at(-1) ?? x;
  for (const s of raw) s.bx = colOf(s.x0) - 7;
  // (the floats' pictures: the space over each caption, up to the line above it in its column)
  const textTop = Math.min(...raw.filter((s) => s.y0 > size.h * 0.07).map((s) => s.y0), Infinity);
  const gone = new Set<Raw>();
  const out: Seg[] = [];
  for (const fl of m.floats) {
    const inFloat = (s: Raw) => s.srcs.some((g) => (g.file === fl.file && g.start >= fl.from && g.start < fl.to) || fl.inputs.some((i) => inputNames(i).includes(g.file)));
    const capOf = (s: Raw) => !!fl.caption && s.srcs.some((g) => g.file === fl.file && g.start >= fl.caption![0] && g.start < fl.caption![1]);
    const caps = raw.filter(capOf);
    if (!caps.length) continue;
    const bx = caps[0].bx;
    const capTop = Math.min(...caps.map((s) => s.y0));
    const picture = raw.filter((s) => s.bx === bx && inFloat(s) && !capOf(s) && s.y1 <= capTop + 0.5);
    const above = raw.filter((s) => s.bx === bx && !inFloat(s) && s.y1 <= Math.min(capTop, ...picture.map((p) => p.y0)) + 0.5);
    const top = Math.min(above.length ? Math.max(...above.map((s) => s.y1)) + 3 : textTop, ...picture.map((p) => p.y0));
    const bottom = capTop - 2;
    if (!(bottom - top >= 8)) continue;
    const blamed = m.floatCommit(fl);
    if (!blamed) continue;
    picture.forEach((p) => gone.add(p));
    const x1 = Math.max(...caps.map((s) => s.x1));
    out.push({ x0: bx + 7, x1, y0: top, y1: bottom, bx, c: blamed.c, file: blamed.src, l0: 0, l1: 0, runs: [], fig: blamed.src, para: 0 });
  }
  for (const s of raw) if (!gone.has(s)) out.push({ x0: s.x0, x1: s.x1, y0: s.y0, y1: s.y1, bx: s.bx, c: s.c, file: s.file, l0: s.l0, l1: s.l1, runs: s.runs, para: 0, ...(s.fig ? { fig: s.fig } : {}) });
  // (paragraphs: in each column top to bottom, the lines in a row with one commit)
  out.sort((a, b) => a.bx - b.bx || a.y0 - b.y0);
  let para = 0;
  out.forEach((s, i) => {
    const p = out[i - 1];
    if (!p || p.bx !== s.bx || p.c !== s.c || s.y0 - p.y1 > 9) para++;
    s.para = para;
  });
  return out;
}

// ---- the parsers of hosts' blames ----

/**
 * `git blame --porcelain`'s output (VS Code reads git so): each line's
 * commit, the commits by their first appearance; the all-zero hash (the
 * working tree's, `--contents`) not committed.
 */
export function parsePorcelain(out: string): { commits: BlameCommit[]; lines: number[] } {
  const commits: BlameCommit[] = [];
  const at = new Map<string, number>();
  const lines: number[] = [];
  let cur: BlameCommit | null = null, line = 0;
  for (const l of out.split("\n")) {
    const h = /^([0-9a-f]{40}) \d+ (\d+)/.exec(l);
    if (h) {
      line = Number(h[2]) - 1;
      if (/^0+$/.test(h[1])) cur = null;
      else {
        let k = at.get(h[1]);
        if (k === undefined) (k = commits.push({ id: h[1], short: h[1].slice(0, 7), author: "", email: "", time: 0, subject: "" }) - 1), at.set(h[1], k);
        cur = commits[k];
      }
      continue;
    }
    if (l.startsWith("\t")) {
      lines[line] = cur ? at.get(cur.id)! : -1;
      continue;
    }
    if (!cur) continue;
    const sp = l.indexOf(" ");
    const [k, v] = sp < 0 ? [l, ""] : [l.slice(0, sp), l.slice(sp + 1)];
    if (k === "author") cur.author = v;
    else if (k === "author-mail") cur.email = v.replace(/^<|>$/g, "");
    else if (k === "author-time") cur.time = Number(v);
    else if (k === "summary") cur.subject = v;
  }
  for (let i = 0; i < lines.length; i++) lines[i] ??= -1;
  return { commits, lines };
}

/** An Overleaf history diff's parts (its `/project/:id/diff` answer): unchanged text, or text inserted by an update. */
export type HistoryPart = { u?: string; i?: string; d?: string; meta?: { users?: { id?: string; first_name?: string; last_name?: string; email?: string }[]; start_ts?: number; end_ts?: number } };

/**
 * Overleaf's history as a blame: its diff of a file from its first version
 * to its last marks each inserted run with who made it and when; a line
 * takes the newest insert in it (the text a line has from the first
 * version, the first update's). `commitOf` names an update by its meta
 * (its version, when the host knows it), and `first` the first version's.
 */
export function historyBlame(parts: HistoryPart[], first: BlameCommit, commitOf: (meta: NonNullable<HistoryPart["meta"]>) => BlameCommit): { text: string; commits: BlameCommit[]; lines: number[] } {
  const commits: BlameCommit[] = [first];
  const at = new Map<string, number>([[first.id, 0]]);
  let text = "";
  const lines: number[] = [0];
  for (const p of parts) {
    if (p.d !== undefined) continue;
    const s = p.u ?? p.i ?? "";
    let k = 0;
    if (p.i !== undefined && p.meta) {
      const c = commitOf(p.meta);
      let j = at.get(c.id);
      if (j === undefined) (j = commits.push(c) - 1), at.set(c.id, j);
      k = j;
    }
    for (const ch of s) {
      if (p.i !== undefined) lines[lines.length - 1] = newerOf(commits, lines[lines.length - 1], k);
      text += ch;
      if (ch === "\n") lines.push(0);
    }
  }
  if (text.endsWith("\n")) lines.pop();
  return { text, commits, lines };
}

/**
 * The lines of `b` matched in `a` (a line of `b` → its line in `a`): the
 * common start and end, and between them a longest common subsequence
 * where it is small enough to work out (else none matched there). Overleaf's
 * history is behind the editor by the edits not flushed yet: the editor's
 * lines that the history's last version lacks are not committed.
 */
export function matchLines(a: string[], b: string[]): Map<number, number> {
  const out = new Map<number, number>();
  let s = 0;
  while (s < a.length && s < b.length && a[s] === b[s]) out.set(s, s), s++;
  let ea = a.length, eb = b.length;
  while (ea > s && eb > s && a[ea - 1] === b[eb - 1]) out.set(--eb, --ea);
  const n = ea - s, m = eb - s;
  if (!n || !m || n * m > 4_000_000) return out;
  // (the LCS table from the end: L[i][j] the longest of a[s+i..] and b[s+j..])
  const L = new Uint32Array((n + 1) * (m + 1));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--) L[i * (m + 1) + j] = a[s + i] === b[s + j] ? L[(i + 1) * (m + 1) + j + 1] + 1 : Math.max(L[(i + 1) * (m + 1) + j], L[i * (m + 1) + j + 1]);
  for (let i = 0, j = 0; i < n && j < m; ) {
    if (a[s + i] === b[s + j]) out.set(s + j++, s + i++);
    else if (L[(i + 1) * (m + 1) + j] >= L[i * (m + 1) + j + 1]) i++;
    else j++;
  }
  return out;
}

function newerOf(cs: BlameCommit[], a: number, b: number): number {
  return cs[b].time > cs[a].time ? b : a;
}

// ---- the view ----

/** How long ago `t` was (seconds since the epoch), in words: "3 hours ago", "9 days ago". */
export function agoWords(t: number, now: number): string {
  const s = Math.max(0, now - t);
  const units: [number, string][] = [[31536000, "year"], [2592000, "month"], [86400, "day"], [3600, "hour"], [60, "minute"]];
  for (const [n, u] of units)
    if (s >= n) {
      const k = Math.floor(s / n);
      return `${k} ${u}${k === 1 ? "" : "s"} ago`;
    }
  return "just now";
}

/** Blame's look, the popover's settings (kept as the viewer's other settings are). */
export interface BlameLook extends Omit<BlameOpts, "progress" | "signal"> {
  /** What the bars' colours say: who wrote the line, or how old it is. */
  color: "author" | "age";
  /** Text not committed: hatched, or not marked. */
  wip: "show" | "hide";
  /** Lines older than this dimmed: "off", or "SECONDS:label" (a tag's time, a date). */
  since: string;
  /** The words tinted: off, the paragraph under the pointer, everywhere. */
  tint: "off" | "hover" | "all";
  bars: "left" | "right";
  /** The bars' width, px. */
  width: number;
  people: "okabe" | "overleaf";
  ages: "violet" | "warm" | "grey";
}

export const BLAME_LOOK: BlameLook = { color: "author", detail: "word", ws: "ignore", moved: "keep", wip: "show", skip: [], since: "off", tint: "hover", bars: "left", width: 4, people: "okabe", ages: "violet" };

/** The age ramps, newest first, light pages' and dark pages' (Under a day, a week, a month, six months; older: none). */
export const AGE_RAMPS: Record<BlameLook["ages"], { name: string; light: string[]; dark: string[] }> = {
  violet: { name: "Violet", light: ["#5B2A86", "#7B4FAE", "#A07CC9", "#C9B5E2"], dark: ["#C7A6F0", "#AE8AD8", "#8E6CB8", "#6B5390"] },
  warm: { name: "Warm", light: ["#b91c1c", "#ea580c", "#f59e0b", "#fde68a"], dark: ["#fca5a5", "#fdba74", "#fcd34d", "#a16207"] },
  grey: { name: "Grey", light: ["#222222", "#555555", "#888888", "#bbbbbb"], dark: ["#eeeeee", "#bbbbbb", "#888888", "#555555"] },
};
/** Overleaf's collaborator colours, as a fixed palette (by the hash of the email) where the host has none of its own. */
export const OVERLEAF_COLORS = ["#3e9cdb", "#e26a6a", "#43b581", "#a776d9", "#e0a030", "#2bb3b1"];

/** A line's look: its bar's colour, the tint's weight, hatched (not committed); null: no bar (older than six months by age, not committed and hidden). */
export function lookOf(m: Model, c: number, look: Pick<BlameLook, "color" | "ages" | "wip">, dark: boolean, colors: Map<string, string>): { color: string; tint: number; hatch?: boolean } | null {
  if (c < 0) return look.wip === "hide" ? null : { color: UNCOMMITTED, tint: 0.12, hatch: true };
  const x = m.commits[c];
  if (!x) return null;
  if (look.color === "age") {
    const i = ageStep((m.now - x.time) / 86400);
    const ramp = AGE_RAMPS[look.ages] ?? AGE_RAMPS.violet;
    const col = (dark ? ramp.dark : ramp.light)[i];
    return col ? { color: col, tint: AGE[i].tint } : null;
  }
  return { color: colors.get(authorKey(x)) ?? OVERFLOW, tint: 0.16 };
}

/** The time "changed since" names (seconds), or null: off. */
export const sinceTime = (since: string): number | null => (since && since !== "off" ? Number(since.split(":")[0]) : null);

export const BLAME_CSS = `
.slot .bl { position: absolute; pointer-events: none; }
.slot .bl-bar { border-radius: 1px; }
.slot .bl-tint { border-radius: 1px; }
.slot .bl-hatch { background-image: repeating-linear-gradient(45deg, rgb(154 160 166 / 85%) 0 1.5px, rgb(154 160 166 / 18%) 1.5px 4px) !important; }
.slot .bl-wait { background: repeating-linear-gradient(0deg, rgb(154 160 166 / 45%) 0 2px, transparent 2px 4px); opacity: .7; }
.slot .bl-wash { background: rgb(255 255 255 / 62%); }
.win.dark-pdf .slot .bl-wash, .slot.dark .bl-wash { background: rgb(20 22 26 / 58%); }
.slot .bl-dim { opacity: .28; }
@media (prefers-reduced-motion: no-preference) {
  .slot .bl-in { animation: bl-in .22s ease-out both; }
  .slot .bl-wait { animation: bl-wait 1.4s ease-in-out infinite; }
  .slot .bl-tint { transition: opacity .12s; }
}
@keyframes bl-in { from { opacity: 0; } }
@keyframes bl-wait { 50% { opacity: .3; } }
.blcard { position: fixed; z-index: 30; width: min(300px, 86vw); background: var(--ph-menu, var(--ph-bar)); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: 10px; box-shadow: 0 10px 30px rgb(15 25 40 / 18%); padding: 10px 12px; font-size: 13px; line-height: 1.45; }
.blcard[hidden] { display: none; }
.blcard .h { display: flex; gap: 8px; align-items: center; font-weight: 600; }
.blcard .av { width: 18px; height: 18px; border-radius: 50%; display: inline-grid; place-items: center; font: 700 8px system-ui, sans-serif; color: #fff; flex: none; }
.blcard .meta { color: var(--ph-fg2); font-size: 12px; }
.blcard .sub { margin-top: 2px; }
.blcard code, .blcard .src { font-family: ui-monospace, "JetBrains Mono", Menlo, monospace; font-size: 11.5px; }
.blcard .src { color: var(--ph-fg2); margin-top: 4px; overflow-wrap: anywhere; }
.blcard .via { color: var(--ph-fg2); font-size: 12px; margin-top: 4px; }
.blcard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
.blcard .acts button { font: inherit; font-size: 12px; background: var(--ph-hover); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: 6px; padding: 3px 8px; cursor: pointer; }
.blcard .acts button:hover, .blcard .acts button:focus-visible { border-color: var(--ph-accent); }
`;

/** The panel's part blame draws in. */
export interface BlamePanel {
  root: ShadowRoot;
  stage: HTMLElement;
  /** The pages' size in points (one size for all). */
  pageSize(): { w: number; h: number };
  /** Whether the pages are drawn dark (the dark PDF). */
  dark(): boolean;
  msg(t: string, err?: boolean): void;
  /** A box the card keeps clear of (Changes' popover, open beside the pages), or null. */
  avoid?(): DOMRect | null;
}

/** What the popover shows of a blame: its progress, who wrote the paper as it is, and the commits that still own lines. */
export interface BlameSummary {
  /** What it is doing ("Working out who wrote each word… main.tex"), "" when done. */
  busy: string;
  /** `done` of `total`, where the total is known. */
  progress?: [number, number];
  error: string;
  /** The sources' units blamed so far (lines, words or characters: `unit`), and files still to come. */
  lines: number;
  unit: "lines" | "words" | "characters";
  waiting: number;
  people: { key: string; name: string; color: string; n: number }[];
  commits: { c: number; commit: BlameCommit | null; n: number; color: string; via: string | null }[];
  /** The tags and the dates "changed since" offers. */
  since: { v: string; label: string }[];
}

/** A printed line's key on its page (the same line drawn again is not new). */
const segKey = (k: number, s: Seg) => `${k}:${Math.round(s.x0)}:${Math.round(s.y0)}`;

/** Blame over the pages: its state, the pages' marks, the card. Its controls are the Changes popover's (changes.ts). */
export class Blame {
  private p: BlamePanel;
  private host: BlameHost;
  on = false;
  look: BlameLook = { ...BLAME_LOOK };
  /** One person's lines picked out (an author key), or one commit's (an index; -1 not committed). */
  focusP: string | null = null;
  focusC: number | null = null;
  private m: Model | null = null;
  private error = "";
  private busy = "";
  private progress: [number, number] | undefined;
  private segs = new Map<number, Seg[]>();
  /** Printed lines whose files are still being read: a quiet placeholder. */
  private waits = new Map<number, { x: number; y0: number; y1: number }[]>();
  private colors = new Map<string, string>();
  private hover: { k: number; s: Seg } | null = null;
  private pinned: { k: number; s: Seg } | null = null;
  private card: HTMLElement;
  private turn = 0;
  private again: ReturnType<typeof setTimeout> | undefined;
  /** The history's head the blame was worked out at (the host's, where it says). */
  private head: string | undefined;
  /** Each line's marks first drawn when (ms): drawn again within the fade, the fade goes on where it was. */
  private drawn = new Map<string, number>();
  private abort: AbortController | undefined;
  private listeners: (() => void)[] = [];
  /** The last look the blame was worked out with (its host's part): another one reads again. */
  private worked = "";

  constructor(p: BlamePanel, host: BlameHost) {
    this.p = p;
    this.host = host;
    this.card = document.createElement("div");
    this.card.className = "blcard";
    this.card.hidden = true;
    this.card.setAttribute("role", "dialog");
    this.card.setAttribute("aria-label", "Who wrote this");
    // (in the panel's window, so the host's tokens (--ph-*) colour it)
    (p.stage.closest(".win") ?? p.root).append(this.card);
    p.stage.addEventListener("pointermove", (e) => this.move(e));
    // (the pointer on its way to the card's actions keeps it; leaving both, it goes)
    const gone = (e: PointerEvent, to: HTMLElement) => !this.pinned && !(e.relatedTarget instanceof Node && to.contains(e.relatedTarget)) && this.over(null);
    p.stage.addEventListener("pointerleave", (e) => gone(e, this.card));
    this.card.addEventListener("pointerleave", (e) => gone(e, p.stage));
    p.stage.addEventListener("click", (e) => this.click(e));
    this.card.addEventListener("click", (e) => this.act(e));
    p.root.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Escape" && (this.pinned || this.hover) && !this.card.hidden) (this.pinned = null), this.over(null), e.stopPropagation();
    }, { capture: true });
  }

  /** `f()` whenever what the popover shows changes (progress, a file read, a focus). */
  onState(f: () => void): void {
    this.listeners.push(f);
  }

  /** The host's word for a commit, and whether it has hashes. */
  get words(): { commit: string; hashes: boolean } {
    return { commit: this.host.words?.commit ?? "commit", hashes: this.host.words?.hashes !== false };
  }

  /** Each page's glyphs the segments were mapped from (the session's own array: the same one, the page unchanged). */
  private mapped = new Map<number, Glyph[]>();
  /** Each page's lookup for the pointer: its segments by their top, each column's right edge. */
  private index = new Map<number, { rows: Seg[]; right: Map<number, number> }>();
  private slots: MutationObserver | null = null;
  /** Files still being worked out (a quiet placeholder on their lines), in the first blame only. */
  private pending = new Set<string>();
  /** The summary as last emitted: emitted again only when it changes. */
  private said = "";
  private sum: BlameSummary | null = null;
  /** Each file's text the blame was worked out on: blamed again only once it changed. */
  private texts = new Map<string, string>();

  /**
   * Blame shown or not. Shown, what is known is drawn at once and the
   * sources are read again quietly (only what changed is blamed again); the
   * first time, the sources are blamed from the history: no page needed.
   */
  toggle(on = !this.on): void {
    if (on === this.on) return;
    this.on = on;
    this.slots?.disconnect();
    if (!on) {
      this.pinned = this.hover = null;
      this.card.hidden = true;
      clearTimeout(this.dwell);
      this.clear();
      this.drawn.clear();
      this.mapped.clear();
      return;
    }
    // (pages come and go (a build streaming, a page drawn again): each mapped as it comes, no blame again)
    this.slots = new MutationObserver(() => this.pagesSoon());
    this.slots.observe(this.p.stage, { childList: true, subtree: true });
    if (this.m && this.worked === this.workKey()) {
      void this.mapPages();
      this.checkTexts(true);
    } else void this.work();
  }

  /** The look changed: drawn again, or worked out again where the host's part changed. */
  setLook(l: Partial<BlameLook>): void {
    this.look = { ...this.look, ...l };
    this.colorsOf();
    this.sum = null;
    if (this.on && this.workKey() !== this.worked) void this.work();
    else this.drawAll();
    this.emit();
  }

  /** One person's lines, or one commit's, picked out (again: none). */
  focus(kind: "person" | "commit", key: string | number | null): void {
    if (kind === "person") (this.focusP = this.focusP === key ? null : (key as string | null)), (this.focusC = null);
    else (this.focusC = this.focusC === key ? null : (key as number | null)), (this.focusP = null);
    this.drawAll();
    this.emit(true);
    if (kind === "commit" && this.focusC !== null) this.reveal(this.focusC);
  }

  /** The first printed line of commit `c` brought into view. */
  private reveal(c: number): void {
    for (const [k, segs] of this.segs) {
      const s = segs.find((s) => s.c === c);
      if (!s) continue;
      const slot = this.slot(k);
      if (!slot) return;
      const { h } = this.p.pageSize();
      const r = slot.getBoundingClientRect(), box = this.p.stage.getBoundingClientRect();
      const y = r.top + (s.y0 / h) * r.height;
      if (y < box.top + 40 || y > box.bottom - 40) this.p.stage.scrollBy({ top: y - (box.top + box.height / 3), behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
      return;
    }
  }

  /**
   * A build done, new origins: the pages mapped again (no blame), and the
   * files whose text changed blamed again, each alone, once typing pauses.
   */
  refresh(): void {
    if (!this.on) return;
    this.pagesSoon();
    this.checkTexts(false);
  }

  /**
   * The files whose text is not the one blamed, or every file once the
   * history moved (the host's head): blamed again, quietly, each alone. A
   * host that can say neither: every file asked again (its answer applied
   * only where it differs). `now`: at once (Blame shown again), else once
   * typing pauses.
   */
  private checkTexts(now: boolean): void {
    if (!this.m || !this.on) return;
    clearTimeout(this.again);
    this.again = setTimeout(() => void this.recheck(), now ? 0 : 700);
  }

  private async recheck(): Promise<void> {
    if (!this.on || !this.m) return;
    const turn = this.turn, files = [...this.texts.keys()];
    let changed = files;
    if (this.host.text) {
      const host = this.host;
      const texts = await Promise.all(files.map((f) => Promise.resolve(host.text!(f)).catch(() => undefined)));
      changed = files.filter((f, i) => texts[i] !== undefined && texts[i] !== this.texts.get(f));
      // (the history moved (a commit): every file, its lines' writers may have)
      if (this.head !== undefined && host.head) {
        const h = await host.head().catch(() => null);
        if (h !== null && h !== this.head) changed = files;
      }
    }
    if (turn !== this.turn || !this.on || !this.m) return;
    if (changed.length) void this.work(changed, true);
  }

  /** The pages drawn again (a new look: dark pages). */
  redraw(): void {
    if (this.on) this.drawAll();
  }

  /** The reads stopped (the project left). */
  stop(): void {
    this.abort?.abort();
    this.turn++;
  }

  private slot(k: number): HTMLElement | null {
    return this.p.stage.querySelector<HTMLElement>(`.slot[data-k="${k}"]`);
  }

  private workKey(): string {
    const l = this.look;
    return JSON.stringify([l.detail, l.ws, l.moved, [...l.skip].sort()]);
  }

  private colorsOf(): void {
    const m = this.m;
    if (!m) return;
    const keys = m.commits.map(authorKey);
    if (this.look.people === "overleaf") {
      this.colors = new Map();
      for (const c of m.commits) {
        const k = authorKey(c);
        if (this.colors.has(k)) continue;
        this.colors.set(k, this.host.color?.(c) ?? OVERLEAF_COLORS[fnv(k) % OVERLEAF_COLORS.length]);
      }
    } else this.colors = authorColors(keys);
  }

  /** An author's colour. */
  colorOf(c: BlameCommit): string {
    return this.colors.get(authorKey(c)) ?? (this.look.people === "overleaf" ? (this.host.color?.(c) ?? OVERLEAF_COLORS[fnv(authorKey(c)) % OVERLEAF_COLORS.length]) : authorColors([authorKey(c)]).get(authorKey(c))!);
  }

  /** The files to blame: the host's sources; none known (a host without them): those the pages show. */
  private async sourceList(): Promise<string[] | null> {
    if (!this.host.sources) return null;
    const s = await this.host.sources();
    if (s.length) return s;
    const out: string[] = [];
    for (let k = 0, n = this.p.stage.querySelectorAll(".slot").length; k < n; k++) for (const g of await this.host.glyphs(k)) if (g.file && !out.includes(g.file)) out.push(g.file);
    return out;
  }

  /**
   * Blamed from the sources and the history, no page needed: each file as
   * it comes, the people and the commits counted from the sources. `only`:
   * just those files, into the blame known (an edit; shown again: `quiet`,
   * no progress said, the lists changed only if their numbers do). A newer
   * work or a stop drops this one's results, never the blame drawn already.
   */
  private async work(only?: string[], quiet = false): Promise<void> {
    const turn = ++this.turn;
    this.abort?.abort();
    const abort = (this.abort = new AbortController());
    const say = (text: string, progress?: [number, number]) => {
      if (quiet || turn !== this.turn) return;
      this.busy = text;
      this.progress = progress;
      this.emit(true);
    };
    const opts: BlameOpts = {
      detail: this.look.detail,
      ws: this.look.ws,
      moved: this.look.moved,
      skip: this.look.skip,
      signal: abort.signal,
      progress: (text, done, total) => say(text, done !== undefined && total ? [done, total] : undefined),
    };
    const key = this.workKey();
    const fresh = !only || !this.m || this.worked !== key;
    const unit = this.look.detail === "line" ? "line" : this.look.detail === "word" ? "word" : "character";
    say("Reading the history…");
    try {
      const m = fresh ? new Model() : this.m!;
      if (fresh) {
        m.detail = this.look.detail;
        m.ws = this.look.ws;
        // (the tags known already, till the new answer brings its own: "Changed since" keeps its choices)
        m.refs = this.m?.refs ?? [];
      }
      const take = (d: BlameData) => {
        m.add(d);
        for (const [f, x] of Object.entries(d.files)) if (x.text !== undefined) this.texts.set(f, x.text);
        if (d.head !== undefined) this.head = d.head;
        if (fresh || this.m !== m) (this.m = m), (this.worked = key), this.colorsOf();
        this.sum = null;
      };
      let files = only ?? (await this.sourceList());
      if (turn !== this.turn) return;
      if (files === null) {
        // (the host picks the sources: asked with none)
        say(`Working out who wrote each ${unit}…`);
        const d = await this.host.blame([], opts);
        if (turn !== this.turn) return;
        if (typeof d === "string") throw new Error(d);
        take(d);
        files = Object.keys(d.files);
      } else {
        if (fresh) (this.pending = new Set(files)), void this.mapPages();
        if (fresh && this.m !== m && this.m) this.texts.clear();
        let i = 0;
        for (const file of files) {
          say(`Working out who wrote each ${unit}… ${file}`, files.length > 1 ? [i, files.length] : undefined);
          const d = await this.host.blame([file], opts);
          if (turn !== this.turn) return;
          if (typeof d === "string") throw new Error(d);
          take(d);
          this.pending.delete(file);
          i++;
          // (each file's lines on the pages as it lands; the lists as they grow)
          void this.mapPages();
          this.emit();
        }
      }
      const more = m.wanted();
      if (more.length) {
        say("Reading the figures' and the bibliography's history…");
        const d2 = await this.host.blame(more, opts);
        if (turn !== this.turn) return;
        if (typeof d2 !== "string") take(d2);
      }
      if (!this.m) (this.m = m), (this.worked = key);
      this.error = "";
    } catch (e) {
      if (turn !== this.turn) return;
      if (!quiet) this.error = abort.signal.aborted ? "" : (e as Error).message || String(e);
    }
    this.busy = "";
    this.progress = undefined;
    this.pending.clear();
    await this.mapPages(true);
    this.emit(true);
  }

  /** Read again (Try again). */
  retry(): void {
    this.error = "";
    void this.work();
  }

  private pagesT: ReturnType<typeof setTimeout> | undefined;
  /** The pages changed: mapped again a moment after the last change (one pass for a stream of them). */
  private pagesSoon(): void {
    if (!this.on || (!this.m && !this.pending.size)) return;
    clearTimeout(this.pagesT);
    this.pagesT = setTimeout(() => void this.mapPages(), 80);
  }

  /**
   * The blame known mapped onto each page shown: a page whose glyphs are
   * the ones mapped already is left as it is; a new or changed one gets its
   * bars (no blame again). `all`: every page (the blame changed).
   */
  private async mapPages(all = false): Promise<void> {
    // (no blame yet, its files being worked out: their lines' quiet placeholders only)
    const m = this.m;
    if (!this.on || (!m && !this.pending.size)) return;
    const size = this.p.pageSize();
    const ks = [...this.p.stage.querySelectorAll<HTMLElement>(".slot")].map((s) => Number(s.dataset.k));
    const live = new Set(ks);
    for (const k of [...this.segs.keys()]) if (!live.has(k)) (this.segs.delete(k), this.index.delete(k), this.mapped.delete(k));
    for (const k of ks) {
      const gs = await this.host.glyphs(k).catch(() => [] as Glyph[]);
      if (!this.on || this.m !== m) return;
      if (!all && m && this.mapped.get(k) === gs && this.slot(k)?.querySelector(".bl")) continue;
      if (m) this.mapped.set(k, gs);
      const segs = m ? mapPage(gs, m, size) : [];
      this.segs.set(k, segs);
      this.waits.set(k, this.pending.size ? printedLines(gs).filter((l) => l.some((g) => g.file && this.pending.has(g.file)) && !l.some((g) => g.file && !this.pending.has(g.file))).map((l) => ({ x: l[0].x, y0: l[0].y - 8, y1: l[0].y + 3 })) : []);
      this.indexPage(k, segs);
      this.drawPage(k);
    }
  }

  /** Page `k`'s lookup: its segments by top, and each column's right edge (a line hit anywhere across its column). */
  private indexPage(k: number, segs: Seg[]): void {
    const right = new Map<number, number>();
    for (const s of segs) right.set(s.bx, Math.max(right.get(s.bx) ?? 0, s.x1));
    this.index.set(k, { rows: [...segs].sort((a, b) => a.y0 - b.y0), right });
  }

  /** "Changed since" choices: off, the tags (newest first), then a day, a week, a month and three months ago. */
  sinceChoices(): { v: string; label: string }[] {
    const m = this.m;
    const now = m?.now ?? Date.now() / 1000;
    const out: { v: string; label: string }[] = [];
    for (const r of [...(m?.refs ?? [])].filter((r) => r.kind === "tag").sort((a, b) => b.time - a.time).slice(0, 6)) out.push({ v: `${r.time}:${r.name}`, label: r.name });
    for (const [d, w] of [[1, "Yesterday"], [7, "A week ago"], [30, "A month ago"]] as [number, string][]) out.push({ v: `${Math.round(now - d * 86400)}:${w}`, label: w });
    return out;
  }

  /** The commits blamed so far (skip offers them). */
  commitsKnown(): BlameCommit[] {
    return this.m ? [...this.m.commits].sort((a, b) => b.time - a.time) : [];
  }

  /** `f()` called when what the popover shows changed (`force`: progress or a focus, said even if the counts are the same). */
  private emit(force = false): void {
    const s = this.summary();
    const sig = JSON.stringify([s.busy, s.error, s.lines, s.unit, s.waiting, s.people.map((p) => [p.key, p.n, p.color]), s.commits.map((c) => [c.c, c.n, c.color]), this.focusP, this.focusC, this.look.color]);
    if (!force && sig === this.said) return;
    this.said = sig;
    for (const f of this.listeners) f();
  }

  /** What the popover shows: who wrote the sources as they are, counted in the detail's units; the pages are not needed. */
  summary(): BlameSummary {
    const m = this.m;
    const unit: BlameSummary["unit"] = this.look.detail === "line" ? "lines" : this.look.detail === "word" ? "words" : "characters";
    const base = { busy: this.busy, progress: this.progress, error: this.error, waiting: this.pending.size, unit, since: this.sinceChoices() };
    if (!m) return { ...base, lines: 0, people: [], commits: [] };
    if (!this.sum || this.sum.unit !== unit) {
      const counts = m.counts();
      const people = new Map<string, { key: string; name: string; color: string; n: number }>();
      const commits = new Map<number, { c: number; commit: BlameCommit | null; n: number; color: string; via: string | null }>();
      const dark = this.p.dark();
      let total = 0;
      for (const [k, n] of counts) {
        if (k < 0 && this.look.wip !== "show") continue;
        total += n;
        const c = k >= 0 ? m.commits[k] : null;
        const key = c ? authorKey(c) : "";
        const p = people.get(key) ?? { key, name: c ? c.author || c.email : this.words.commit === "version" ? "Not saved yet" : "Not committed", color: c ? this.colorOf(c) : UNCOMMITTED, n: 0 };
        p.n += n;
        people.set(key, p);
        commits.set(k, { c: k, commit: c, n, color: lookOf(m, k, this.look, dark, this.colors)?.color ?? "transparent", via: c ? (this.host.via?.(c) ?? null) : null });
      }
      this.sum = {
        ...base,
        lines: total,
        people: [...people.values()].sort((a, b) => b.n - a.n),
        commits: [...commits.values()].sort((a, b) => ((a.commit ? a.commit.time : Infinity) > (b.commit ? b.commit.time : Infinity) ? -1 : 1)),
      };
    }
    return { ...this.sum, ...base };
  }

  private shade(c: number): { color: string; tint: number; hatch?: boolean } | null {
    return this.m ? lookOf(this.m, c, this.look, this.p.dark(), this.colors) : null;
  }

  /** Whether a line is set back: another person's or commit's picked out, or older than "changed since". */
  private dimmed(c: number): boolean {
    const m = this.m;
    if (!m) return false;
    const x = c >= 0 ? m.commits[c] : null;
    if (this.focusP !== null && (x ? authorKey(x) : "") !== this.focusP) return true;
    if (this.focusC !== null && c !== this.focusC) return true;
    const since = sinceTime(this.look.since);
    return since !== null && !!x && x.time <= since;
  }

  private clear(): void {
    for (const e of this.p.stage.querySelectorAll(".slot .bl")) e.remove();
  }

  /** Every page drawn again (a look, a focus). */
  private drawAll(): void {
    if (!this.on || !this.m) return this.clear();
    for (const k of this.segs.keys()) this.drawPage(k);
  }

  /** Page `k`'s bars, washes and tints (the hover's tint apart: drawn alone as the pointer moves). */
  private drawPage(k: number): void {
    const slot = this.slot(k);
    if (!slot) return;
    for (const e of slot.querySelectorAll(".bl")) e.remove();
    const segs = this.segs.get(k);
    if (!this.on || (!this.m && !this.waits.get(k)?.length) || !segs) return;
    const { w, h } = this.p.pageSize();
    const dark = this.p.dark();
    const pct = (v: number, of: number) => `${(v / of) * 100}%`;
    const bw = Math.max(1, this.look.width);
    const now = performance.now();
    const frag = document.createDocumentFragment();
    const right = this.index.get(k)?.right ?? new Map<number, number>();
    for (const s of segs) {
      const lk = this.shade(s.c);
      const key = segKey(k, s);
      const t0 = this.drawn.get(key) ?? now;
      this.drawn.set(key, t0);
      const fade = now - t0 < 220 ? " bl-in" : "";
      const delay = fade ? `${-(now - t0)}ms` : "";
      const dim = this.dimmed(s.c);
      if (dim) {
        const wsh = document.createElement("div");
        wsh.className = `bl bl-wash${fade}`;
        Object.assign(wsh.style, { left: pct(s.x0 - 1, w), top: pct(s.y0, h), width: pct(s.x1 - s.x0 + 2, w), height: pct(s.y1 - s.y0, h), animationDelay: delay });
        frag.append(wsh);
      }
      if (!lk) continue;
      if (!dim && this.look.tint === "all") frag.append(...this.tints(s, w, h, dark));
      const b = document.createElement("div");
      b.className = `bl bl-bar${lk.hatch ? " bl-hatch" : ""}${dim ? " bl-dim" : ""}${fade}`;
      const x = this.look.bars === "right" ? (right.get(s.bx) ?? s.x1) + 5 : s.bx + 3 - bw;
      Object.assign(b.style, { left: pct(x, w), width: `${bw}px`, top: pct(s.y0 + 0.5, h), height: pct(s.y1 - s.y0 - 1, h), background: lk.color, animationDelay: delay });
      frag.append(b);
    }
    for (const l of this.waits.get(k) ?? []) {
      const b = document.createElement("div");
      b.className = "bl bl-bar bl-wait";
      Object.assign(b.style, { left: pct(this.look.bars === "right" ? l.x + 4 : l.x - 7 + 3 - bw, w), width: `${bw}px`, top: pct(l.y0 + 0.5, h), height: pct(l.y1 - l.y0 - 1, h) });
      frag.append(b);
    }
    slot.append(frag);
    if ((this.pinned ?? this.hover)?.k === k) this.drawHover();
  }

  /** A line's tint over its words, by writer. */
  private tints(s: Seg, w: number, h: number, dark: boolean, cls = "bl-tint"): HTMLElement[] {
    const pct = (v: number, of: number) => `${(v / of) * 100}%`;
    const out: HTMLElement[] = [];
    const runs = s.fig || !s.runs.length ? [[s.x0, s.x1, s.c] as [number, number, number]] : s.runs;
    for (const [a, b, c] of runs) {
      const l = this.shade(c);
      if (!l) continue;
      const t = document.createElement("div");
      t.className = `bl ${cls}${l.hatch ? " bl-hatch" : ""}`;
      Object.assign(t.style, { left: pct(a - 1, w), top: pct(s.y0, h), width: pct(b - a + 2, w), height: pct(s.y1 - s.y0, h), background: l.color, opacity: String(l.hatch ? 1 : dark ? l.tint * 1.3 : l.tint) });
      out.push(t);
    }
    return out;
  }

  /** The paragraph under the pointer tinted ("under the pointer"), alone: the rest of the page left as it is. */
  private drawHover(): void {
    for (const e of this.p.stage.querySelectorAll(".slot .bl-htint")) e.remove();
    const f = this.pinned ?? this.hover;
    if (!f || !this.on || this.look.tint !== "hover") return;
    const slot = this.slot(f.k);
    if (!slot) return;
    const { w, h } = this.p.pageSize();
    const dark = this.p.dark();
    const frag = document.createDocumentFragment();
    for (const s of this.segs.get(f.k) ?? []) if (s.para === f.s.para && !this.dimmed(s.c)) frag.append(...this.tints(s, w, h, dark, "bl-tint bl-htint"));
    slot.append(frag);
  }

  /** Each page's box, read once after a scroll or a resize (not on every pointer move). */
  private rects = new Map<number, DOMRect>();
  private rectsFresh = false;
  private watching = false;

  private rectOf(slot: HTMLElement, k: number): DOMRect {
    if (!this.watching) {
      this.watching = true;
      const stale = () => ((this.rectsFresh = false), this.rects.clear());
      this.p.stage.addEventListener("scroll", stale, { passive: true, capture: true });
      addEventListener("resize", stale, { passive: true });
      new MutationObserver(stale).observe(this.p.stage, { attributes: true, attributeFilter: ["style", "class"], subtree: false });
    }
    let r = this.rectsFresh ? this.rects.get(k) : undefined;
    if (!r) {
      r = slot.getBoundingClientRect();
      this.rects.set(k, r);
      this.rectsFresh = true;
    }
    return r;
  }

  /** The printed line (or picture) under a point, by its page's slot: a binary search by its top, then its column. */
  private atPoint(target: EventTarget | null, cx: number, cy: number): { k: number; s: Seg } | null {
    const slot = (target as HTMLElement | null)?.closest?.<HTMLElement>(".slot");
    if (!slot) return null;
    const k = Number(slot.dataset.k);
    const ix = this.index.get(k);
    if (!ix) return null;
    const r = this.rectOf(slot, k);
    const { w, h } = this.p.pageSize();
    const x = ((cx - r.left) / r.width) * w, y = ((cy - r.top) / r.height) * h;
    const rows = ix.rows;
    // (the last segment starting above the point, then back over those it may still be inside)
    let lo = 0, hi = rows.length;
    while (lo < hi) {
      const m = (lo + hi) >> 1;
      if (rows[m].y0 <= y) lo = m + 1;
      else hi = m;
    }
    for (let i = lo - 1; i >= 0 && i >= lo - 40; i--) {
      const s = rows[i];
      if (y <= s.y1 && x >= Math.min(s.bx - 4, s.x0) && x <= (ix.right.get(s.bx) ?? s.x1) + 8) return { k, s };
    }
    return null;
  }

  private at(e: MouseEvent): { k: number; s: Seg } | null {
    return this.atPoint(e.target, e.clientX, e.clientY);
  }

  /** The pointer's last place, hit-tested once a frame at most. */
  private moved: { t: EventTarget | null; x: number; y: number } | null = null;
  private frame = 0;
  private dwell: ReturnType<typeof setTimeout> | undefined;

  private move(e: PointerEvent): void {
    if (!this.on || this.pinned) return;
    this.moved = { t: e.target, x: e.clientX, y: e.clientY };
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      const p = this.moved;
      if (!p || !this.on || this.pinned) return;
      const a = this.atPoint(p.t, p.x, p.y);
      if (a?.s === this.hover?.s) return;
      this.over(a, p.x, p.y);
    });
  }

  /** The line under the pointer changed: its paragraph's tint (only when that changes), the card after a short dwell. */
  private over(a: { k: number; s: Seg } | null, x?: number, y?: number): void {
    const was = this.hover;
    this.hover = a;
    clearTimeout(this.dwell);
    if (!a) this.card.hidden = true;
    else if (x !== undefined && y !== undefined) {
      // (a card already up follows at once; a first one waits a moment: the pointer just passing shows none)
      if (!this.card.hidden) this.show(a, x, y);
      else this.dwell = setTimeout(() => this.hover === a && this.show(a, x, y), 180);
    }
    if (was?.s.para !== a?.s.para || was?.k !== a?.k) this.drawHover();
  }

  private click(e: MouseEvent): void {
    if (!this.on) return;
    if ((e.target as HTMLElement).closest(".blcard")) return;
    const a = this.at(e);
    if (!a) {
      if (this.pinned) (this.pinned = null), this.over(null);
      return;
    }
    this.pinned = a;
    this.hover = a;
    clearTimeout(this.dwell);
    this.show(a, e.clientX, e.clientY);
    this.drawHover();
  }

  /** What the card shows now (its line's commit and source): drawn again only when that changes. */
  private cardFor = "";

  /** The card for line `a`, near the pointer: its words drawn only when its line's commit or source changes. */
  private show(a: { k: number; s: Seg }, px: number, py: number): void {
    const m = this.m;
    if (!m) return;
    const c = a.s.c >= 0 ? m.commits[a.s.c] : null;
    const src = a.s.fig ? `${a.s.fig}` : `${a.s.file}:${a.s.l0 + 1}${a.s.l1 > a.s.l0 ? `–${a.s.l1 + 1}` : ""}`;
    const id = `${a.s.c}|${src}`;
    if (id !== this.cardFor || this.card.hidden) {
      this.cardFor = id;
      const { commit: word, hashes } = this.words;
      const color = c ? this.colorOf(c) : UNCOMMITTED;
      const via = c ? (this.host.via?.(c) ?? null) : null;
      this.card.replaceChildren(
        html`<div class="h"><span class="av" style="${`background:${color}`}">${c ? (c.author || c.email).trim().split(/\s+/).map((w) => w[0] ?? "").join("").slice(0, 2).toUpperCase() : "Y"}</span>${c ? c.author || c.email : "You"}</div><div class="meta">${c ? html`${agoWords(c.time, m.now)} · <code>${c.short}</code>` : `now · not ${word === "version" ? "saved to the history" : "committed"}`}</div><div class="sub">${c ? c.subject || "(no subject)" : word === "version" ? "Typed since the last version" : "Typed since the last commit"}</div>${via ? html`<div class="via">${via}</div>` : ""}<div class="src">${src}</div><div class="acts">${this.host.compare && c ? html`<button type="button" data-a="cmp" title="${`Diff of this ${word} against the one before it`}">${`What this ${word} changed`}</button>` : ""}<button type="button" data-a="src" title="The editor at this line">Show in editor</button>${c && hashes ? html`<button type="button" data-a="copy" title="Copy the full hash">Copy hash</button>` : ""}</div>`,
      );
      this.card.hidden = false;
      this.cardSize = [this.card.offsetWidth || 300, this.card.offsetHeight || 120];
      this.stageBox = this.p.stage.getBoundingClientRect();
      this.avoidBox = this.p.avoid?.() ?? null;
    }
    // (near the pointer, inside the pages' box; placed, then moved by where it landed: a fixed box is
    // placed in its containing block, which a host's transform or containment makes its pane)
    const [W, H] = this.cardSize;
    const box = this.stageBox ?? this.p.stage.getBoundingClientRect();
    let x = Math.max(box.left + 6, Math.min(box.right - W - 6, px + 14));
    let y = py + H + 20 > box.bottom ? Math.max(box.top + 6, py - H - 12) : py + 16;
    // (never over Changes' popover: left of the pointer, else under the popover, else under the pointer)
    const v = this.avoidBox, over = (l: number, t: number) => !!v && l < v.right && l + W > v.left && t < v.bottom && t + H > v.top;
    if (v && over(x, y)) {
      const tries: [number, number][] = [[px - 14 - W, y], [x, v.bottom + 8], [x, py + 16]];
      const fit = tries.find(([l, t]) => l >= box.left + 6 && t + H <= box.bottom && !over(l, t));
      if (fit) [x, y] = fit;
    }
    this.card.style.left = `${x + this.cardShift[0]}px`;
    this.card.style.top = `${y + this.cardShift[1]}px`;
    if (!this.cardPlaced) {
      this.cardPlaced = true;
      const at = this.card.getBoundingClientRect();
      this.cardShift = [x - at.left + this.cardShift[0], y - at.top + this.cardShift[1]];
      this.card.style.left = `${x + this.cardShift[0]}px`;
      this.card.style.top = `${y + this.cardShift[1]}px`;
    }
  }
  private cardSize: [number, number] = [300, 120];
  private avoidBox: DOMRect | null = null;
  private stageBox: DOMRect | null = null;
  /** Where a fixed box lands against where it was put (its containing block's offset), measured once. */
  private cardShift: [number, number] = [0, 0];
  private cardPlaced = false;

  private act(e: Event): void {
    e.stopPropagation();
    const a = (e.target as HTMLElement).closest<HTMLElement>("button")?.dataset.a;
    const cur = this.pinned ?? this.hover;
    const m = this.m;
    if (!a || !cur || !m) return;
    const c = cur.s.c >= 0 ? m.commits[cur.s.c] : null;
    if (a === "cmp") {
      this.pinned = null;
      this.over(null);
      return this.host.compare?.(c);
    }
    if (a === "src") return this.host.goto(cur.s.fig && !cur.s.fig.includes(":") ? cur.s.fig : cur.s.file.replace(/:.*$/, ""), cur.s.fig ? 1 : cur.s.l0 + 1);
    if (a === "copy" && c)
      void navigator.clipboard?.writeText(c.id).then(
        () => this.p.msg(`Copied ${c.short}`),
        () => this.p.msg(`Hash: ${c.id}`),
      );
  }
}
