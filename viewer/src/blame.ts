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
}

/**
 * A file's blame: a text file's commit for each line (an index into the
 * commits, -1 not committed yet) with its text as blamed (the glyph
 * origins' bytes are of it); a binary one's (a figure's image) last change.
 */
export interface BlameFile {
  text?: string;
  lines?: number[];
  last?: number;
  error?: string;
}

/** A tag or a branch, for "changed since". */
export interface BlameRef {
  name: string;
  kind: "tag" | "branch" | string;
  time: number;
}

/** The host's answer (the `blame` op's, docs/spec-editor-protocol.md §10). */
export interface BlameData {
  commits: BlameCommit[];
  files: Record<string, BlameFile>;
  refs?: BlameRef[];
  /** The host's clock, seconds since the epoch (ages are from it). */
  now?: number;
}

/** What blame needs of its host. */
export interface BlameHost {
  /** Files `files` (project paths, as the glyph origins name them) blamed, or why not ("no git history here"). */
  blame(files: string[]): Promise<BlameData | string>;
  /** Page `k`'s glyphs (the session's). */
  glyphs(k: number): Promise<Glyph[]>;
  /** The editor at `file`'s `line` (from 1). */
  goto(file: string, line: number): void;
  /** Compare with the version before `c` (null: text not committed, against the last commit): the host's Compare. Absent: not offered. */
  compare?(c: BlameCommit | null): void;
  /** An author's colour where the host gives them one (Overleaf's collaborators); null: by the hash of the email. */
  color?(c: BlameCommit): string | null;
  /** The host's word for a commit ("commit"; Overleaf: "version"), and whether it has hashes to copy. */
  words?: { commit?: string; hashes?: boolean };
}

/** The panel's part blame draws in. */
export interface BlamePanel {
  root: ShadowRoot;
  stage: HTMLElement;
  /** The row under the bar (the mode, the tint, the legend). */
  bar: HTMLElement;
  /** The pages' size in points (one size for all). */
  pageSize(): { w: number; h: number };
  /** Whether the pages are drawn dark (the dark PDF). */
  dark(): boolean;
  msg(t: string, err?: boolean): void;
  /** The toggle's state shown (the bar's button). */
  pressed(on: boolean): void;
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

/** A blame's files made ready to look up: each text's line starts, floats, and the bibliography's keys. */
export class Model {
  commits: BlameCommit[] = [];
  refs: BlameRef[] = [];
  now = Date.now() / 1000;
  files = new Map<string, BlameFile & { starts?: number[] }>();
  floats: Float[] = [];
  private byId = new Map<string, number>();
  private bib = new Map<string, { file: string; lines: [number, number] }>();
  private bbls = new Map<string, [number, string][]>();

  /** `d` taken in: its commits merged with those already here by id. */
  add(d: BlameData): void {
    const map = d.commits.map((c) => {
      let k = this.byId.get(c.id);
      if (k === undefined) (k = this.commits.push(c) - 1), this.byId.set(c.id, k);
      return k;
    });
    const re = (i: number | undefined) => (i === undefined ? undefined : i < 0 ? -1 : (map[i] ?? -1));
    if (d.now) this.now = d.now;
    if (d.refs) this.refs = d.refs;
    for (const [name, f] of Object.entries(d.files)) {
      const g: BlameFile & { starts?: number[] } = { ...f, lines: f.lines?.map((l) => re(l)!), last: re(f.last) };
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

/** A page's glyphs as printed lines: one baseline (to half a point), split where the next glyph is a column away (18 pt). */
export function printedLines(gs: Glyph[]): Glyph[][] {
  const base = (g: Glyph) => Math.round(g.y * 2);
  const order = [...gs].sort((a, b) => base(a) - base(b) || a.x - b.x);
  const out: Glyph[][] = [];
  for (const g of order) {
    const l = out.at(-1);
    if (l && base(l[0]) === base(g) && g.x - l.at(-1)!.x < 18) l.push(g);
    else out.push([g]);
  }
  return out;
}

/** The left edges of a page's columns: the starts of three or more lines, 12 pt apart or more. */
export function columnStarts(x0s: number[]): number[] {
  const xs = [...x0s].sort((a, b) => a - b);
  const groups: number[][] = [];
  for (const x of xs) {
    const g = groups.at(-1);
    if (g && x - g[0] < 12) g.push(x);
    else groups.push([x]);
  }
  return groups.filter((g) => g.length >= 3).map((g) => g[0]);
}

/** A page's printed lines blamed (`size`: the page in points). */
export function mapPage(gs: Glyph[], m: Model, size: { w: number; h: number }): Seg[] {
  type Raw = Seg & { glyphs: Glyph[]; srcs: { file: string; start: number }[] };
  const raw: Raw[] = [];
  for (const line of printedLines(gs)) {
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

export type BlameMode = "age" | "author" | "since";
export type BlameTint = "off" | "hover" | "all";

/** A line's look: its bar's colour, the tint's weight, hatched (not committed); null: no bar (older than six months, not changed since). */
export function lookOf(m: Model, c: number, mode: BlameMode, dark: boolean, since: number, colors: Map<string, string>): { color: string; tint: number; hatch?: boolean } | null {
  if (c < 0) return { color: UNCOMMITTED, tint: 0.12, hatch: true };
  const x = m.commits[c];
  if (!x) return null;
  if (mode === "age") {
    const a = AGE[ageStep((m.now - x.time) / 86400)];
    return a?.bar ? { color: a.bar[dark ? 1 : 0], tint: a.tint } : null;
  }
  if (mode === "author") return { color: colors.get(authorKey(x)) ?? OVERFLOW, tint: 0.16 };
  return x.time > since ? { color: SINCE[dark ? 1 : 0], tint: 0.18 } : null;
}

export const BLAME_CSS = `
.blamebar { display: none; flex: none; align-items: center; gap: 6px 14px; padding: 4px 8px 4px 12px; flex-wrap: wrap; border-bottom: 1px solid var(--ph-border); background: var(--ph-bar); color: var(--ph-fg); font-size: 12.5px; }
.blamebar.on { display: flex; }
.blamebar .grp { display: inline-flex; align-items: center; gap: 6px; }
.blamebar .lbl { font-size: 10.5px; letter-spacing: .06em; text-transform: uppercase; color: var(--ph-fg2); }
.blamebar .seg { display: inline-flex; border: 1px solid var(--ph-border); border-radius: var(--ph-radius); overflow: hidden; }
.blamebar .seg button { all: unset; cursor: pointer; padding: 2px 9px; color: var(--ph-fg); }
.blamebar .seg button + button { border-left: 1px solid var(--ph-border); }
.blamebar .seg button[aria-pressed=true] { background: var(--ph-accent); color: var(--ph-accent-fg, #fff); }
.blamebar .seg button:focus-visible, .blamebar select:focus-visible, .blamebar .x:focus-visible { outline: 2px solid var(--ph-accent); outline-offset: 1px; }
.blamebar select { font: inherit; padding: 1px 4px; border-radius: var(--ph-radius); border: 1px solid var(--ph-border); background: var(--ph-bar); color: var(--ph-fg); max-width: 260px; }
.blamebar .legend { display: inline-flex; flex-wrap: wrap; gap: 4px 12px; align-items: center; }
.blamebar .lg { display: inline-flex; align-items: center; gap: 5px; white-space: nowrap; }
.blamebar .lg small { color: var(--ph-fg2); font-variant-numeric: tabular-nums; font-size: inherit; }
.blamebar .sw { width: 18px; height: 11px; border-radius: 2px; position: relative; overflow: hidden; border: 1px solid var(--ph-border); }
.blamebar .sw i { position: absolute; left: 0; top: 0; bottom: 0; width: 3px; }
.blamebar .say { color: var(--ph-fg2); }
.blamebar .grow { flex: 1; }
.blamebar .x { all: unset; cursor: pointer; padding: 0 6px; color: var(--ph-fg2); border-radius: var(--ph-radius); }
.blamebar .x:hover { background: var(--ph-hover); color: var(--ph-fg); }
.slot .bl { position: absolute; pointer-events: none; }
.slot .bl.bar { border-radius: 1px; width: 3px; }
.slot .bl.bar.on { width: 4px; }
.slot .bl.tint { border-radius: 1px; }
.slot .bl.hatch { background-image: repeating-linear-gradient(45deg, rgb(154 160 166 / 85%) 0 1.5px, rgb(154 160 166 / 18%) 1.5px 4px) !important; }
.blcard { position: fixed; z-index: 30; width: min(320px, 86vw); background: var(--ph-menu, var(--ph-bar)); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: 8px; box-shadow: 0 6px 24px rgb(20 24 33 / 22%); padding: 9px 12px 9px 16px; font-size: 13px; line-height: 1.35; }
.blcard[hidden] { display: none; }
.blcard .strip { position: absolute; left: 0; top: 0; bottom: 0; width: 5px; border-radius: 8px 0 0 8px; }
.blcard .sub { font-weight: 600; }
.blcard .meta { color: var(--ph-fg2); margin-top: 3px; }
.blcard code, .blcard .src { font-family: ui-monospace, "JetBrains Mono", Menlo, monospace; font-size: 11.5px; }
.blcard .src { color: var(--ph-fg2); margin-top: 6px; overflow-wrap: anywhere; }
.blcard .acts { display: flex; flex-wrap: wrap; gap: 4px 12px; margin-top: 8px; }
.blcard .acts button { all: unset; cursor: pointer; color: var(--ph-link, var(--ph-accent)); font-size: 12.5px; }
.blcard .acts button:hover, .blcard .acts button:focus-visible { text-decoration: underline; }
@media (prefers-reduced-motion: no-preference) { .slot .bl.tint { transition: opacity .12s; } }
`;

const HATCH_SWATCH = "repeating-linear-gradient(45deg,#9AA0A6 0 1.5px,transparent 1.5px 4px)";

/** Blame over the pages: its bar's state, the pages' marks, the card. */
export class Blame {
  private p: BlamePanel;
  private host: BlameHost;
  on = false;
  mode: BlameMode = "age";
  tint: BlameTint = "hover";
  /** Changed since: seconds since the epoch, and the choice's value in the list. */
  private since: { t: number; v: string } | null = null;
  private m: Model | null = null;
  private error = "";
  private busy = false;
  private segs = new Map<number, Seg[]>();
  private colors = new Map<string, string>();
  private hover: { k: number; s: Seg } | null = null;
  private pinned: { k: number; s: Seg } | null = null;
  private card: HTMLElement;
  private turn = 0;
  private again: ReturnType<typeof setTimeout> | undefined;

  constructor(p: BlamePanel, host: BlameHost) {
    this.p = p;
    this.host = host;
    this.card = document.createElement("div");
    this.card.className = "blcard";
    this.card.hidden = true;
    this.card.setAttribute("role", "dialog");
    this.card.setAttribute("aria-label", "Commit");
    p.root.append(this.card);
    p.stage.addEventListener("pointermove", (e) => this.move(e));
    p.stage.addEventListener("pointerleave", () => !this.pinned && this.over(null));
    p.stage.addEventListener("click", (e) => this.click(e));
    this.card.addEventListener("click", (e) => this.act(e));
    p.root.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Escape" && (this.pinned || this.hover)) (this.pinned = null), this.over(null), e.stopPropagation();
    }, { capture: true });
    p.bar.addEventListener("click", (e) => this.barClick(e));
    p.bar.addEventListener("change", (e) => {
      const t = e.target as HTMLSelectElement;
      if (t.id !== "bl-since") return;
      this.since = { t: Number(t.value.split(":")[0]), v: t.value };
      this.draw();
    });
  }

  private toggled: ((on: boolean) => void)[] = [];

  /** `cb(on)` whenever blame is shown or hidden (a host's own toggle: Overleaf's toolbar). */
  onToggle(cb: (on: boolean) => void): void {
    this.toggled.push(cb);
  }

  /** Blame shown or not (the bar's toggle). */
  toggle(on = !this.on): void {
    this.on = on;
    this.p.pressed(on);
    for (const f of this.toggled) f(on);
    if (!on) {
      this.pinned = this.hover = null;
      this.card.hidden = true;
      this.clear();
      this.p.bar.className = "blamebar";
      this.p.bar.replaceChildren();
      return;
    }
    this.p.bar.className = "blamebar on";
    void this.load();
  }

  /** The pages changed (a build, new origins): blamed again, a moment after the last change. */
  refresh(): void {
    if (!this.on) return;
    clearTimeout(this.again);
    this.again = setTimeout(() => void this.load(), 300);
  }

  /** The pages drawn again (a new look: dark pages). */
  redraw(): void {
    if (this.on) this.draw();
  }

  private pages(): number {
    return this.p.stage.querySelectorAll(".slot").length;
  }

  private async load(): Promise<void> {
    const turn = ++this.turn;
    this.busy = true;
    this.bar();
    try {
      const n = this.pages();
      const glyphs: Glyph[][] = [];
      for (let k = 0; k < n; k++) glyphs.push(await this.host.glyphs(k));
      if (turn !== this.turn) return;
      const files = new Set<string>();
      for (const gs of glyphs) for (const g of gs) if (g.file) files.add(g.file);
      const m = new Model();
      const d = await this.host.blame([...files]);
      if (turn !== this.turn) return;
      if (typeof d === "string") throw new Error(d);
      m.add(d);
      const more = m.wanted();
      if (more.length) {
        const d2 = await this.host.blame(more);
        if (turn !== this.turn) return;
        if (typeof d2 !== "string") m.add(d2);
      }
      this.m = m;
      this.error = "";
      this.colors = authorColors(m.commits.map(authorKey));
      if (this.host.color) for (const c of m.commits) {
        const h = this.host.color(c);
        if (h) this.colors.set(authorKey(c), h);
      }
      const size = this.p.pageSize();
      this.segs.clear();
      glyphs.forEach((gs, k) => this.segs.set(k, mapPage(gs, m, size)));
      if (!this.since) this.since = this.sinceChoices()[0] ?? null;
    } catch (e) {
      if (turn !== this.turn) return;
      this.m = null;
      this.error = (e as Error).message || String(e);
    }
    this.busy = false;
    this.draw();
  }

  /** "Changed since" choices: the tags (newest first), then a day, a week, a month and three months ago, then the commits shown. */
  sinceChoices(): { t: number; v: string; label: string }[] {
    const m = this.m;
    if (!m) return [];
    const out: { t: number; v: string; label: string }[] = [];
    for (const r of [...m.refs].filter((r) => r.kind === "tag").sort((a, b) => b.time - a.time)) out.push({ t: r.time, v: `${r.time}:tag:${r.name}`, label: `${r.name} · ${agoWords(r.time, m.now)}` });
    for (const [d, w] of [[1, "yesterday"], [7, "a week ago"], [30, "a month ago"], [91, "three months ago"]] as [number, string][]) out.push({ t: m.now - d * 86400, v: `${m.now - d * 86400}:days:${d}`, label: w });
    const word = this.host.words?.commit ?? "commit";
    for (const c of [...m.commits].sort((a, b) => b.time - a.time).slice(0, 12)) out.push({ t: c.time, v: `${c.time}:c:${c.id}`, label: `${word} ${c.short} · ${c.subject.slice(0, 40)}` });
    return out;
  }

  private look(c: number) {
    return this.m ? lookOf(this.m, c, this.mode, this.p.dark(), this.since?.t ?? Infinity, this.colors) : null;
  }

  private clear(): void {
    for (const e of this.p.stage.querySelectorAll(".slot .bl")) e.remove();
  }

  /** The bars and tints on every page, and the bar's legend. */
  private draw(): void {
    this.clear();
    this.bar();
    if (!this.on || !this.m) return;
    const { w, h } = this.p.pageSize();
    const dark = this.p.dark();
    const focus = this.pinned ?? this.hover;
    const pct = (v: number, of: number) => `${(v / of) * 100}%`;
    for (const [k, segs] of this.segs) {
      const slot = this.p.stage.querySelector<HTMLElement>(`.slot[data-k="${k}"]`);
      if (!slot) continue;
      const frag = document.createDocumentFragment();
      for (const s of segs) {
        const lk = this.look(s.c);
        if (!lk) continue;
        const lit = !!focus && focus.k === k && focus.s.para === s.para;
        const tinted = this.tint === "all" || (this.tint === "hover" && lit);
        if (tinted) {
          const runs = s.fig || !s.runs.length ? [[s.x0, s.x1, s.c] as [number, number, number]] : s.runs;
          for (const [a, b, c] of runs) {
            const l = this.look(c);
            if (!l) continue;
            const t = document.createElement("div");
            t.className = `bl tint${l.hatch ? " hatch" : ""}`;
            Object.assign(t.style, { left: pct(a - 1, w), top: pct(s.y0, h), width: pct(b - a + 2, w), height: pct(s.y1 - s.y0, h), background: l.color, opacity: String(l.hatch ? 1 : dark ? l.tint * 1.3 : l.tint) });
            frag.append(t);
          }
        }
        const b = document.createElement("div");
        b.className = `bl bar${lk.hatch ? " hatch" : ""}${lit ? " on" : ""}`;
        Object.assign(b.style, { left: pct(s.bx, w), top: pct(s.y0 + 0.5, h), height: pct(s.y1 - s.y0 - 1, h), background: lk.color });
        frag.append(b);
      }
      slot.append(frag);
    }
  }

  /** The row under the bar: Colour by, Since (its choice), Word tint, the legend. */
  private bar(): void {
    if (!this.on) return;
    const seg = (id: string, cur: string, opts: [string, string][]) =>
      html`<span class="seg" role="group" id="${id}">${opts.map(([v, t]) => html`<button type="button" data-v="${v}" aria-pressed="${String(cur === v)}">${t}</button>`)}</span>`;
    const choices = this.sinceChoices();
    const since = this.mode === "since" ? html`<span class="grp"><span class="lbl">Since</span><select id="bl-since" aria-label="Changed since">${choices.map((c) => html`<option value="${c.v}" selected="${c.v === this.since?.v}">${c.label}</option>`)}</select></span>` : "";
    const say = this.busy ? html`<span class="say">Reading the history…</span>` : this.error ? html`<span class="say">${this.error}</span>` : this.legend();
    this.p.bar.replaceChildren(
      html`<span class="grp"><span class="lbl">Colour by</span>${seg("bl-mode", this.mode, [["age", "Age"], ["author", "Author"], ["since", "Since…"]])}</span>${since}<span class="grp"><span class="lbl">Word tint</span>${seg("bl-tint", this.tint, [["off", "Off"], ["hover", "Paragraph under pointer"], ["all", "All"]])}</span>${say}<span class="grow"></span><button type="button" class="x" data-a="close" title="Hide blame" aria-label="Hide blame">✕</button>`,
    );
  }

  /** The legend: each colour's meaning and how many printed lines have it. */
  private legend(): DocumentFragment {
    const m = this.m;
    if (!m) return html``;
    const all = [...this.segs.values()].flat();
    const dark = this.p.dark();
    const row = (color: string, name: string, n: number, hatch = false) =>
      html`<span class="lg"><span class="sw" style="${hatch ? `background:${HATCH_SWATCH}` : `background:${color}${dark ? "33" : "26"}`}"><i style="${`background:${hatch ? UNCOMMITTED : color}`}"></i></span><span>${name}</span><small>${n}</small></span>`;
    const unc = all.filter((s) => s.c < 0).length;
    const lines = (f: (c: BlameCommit) => boolean) => all.filter((s) => s.c >= 0 && f(m.commits[s.c])).length;
    let rows: DocumentFragment[] = [];
    if (this.mode === "age") {
      rows = [row("", "Not committed", unc, true), ...AGE.map((a, i) => row(a.bar ? a.bar[dark ? 1 : 0] : "transparent", a.name, lines((c) => ageStep((m.now - c.time) / 86400) === i)))];
    } else if (this.mode === "author") {
      const who = new Map<string, { name: string; n: number }>();
      for (const s of all) {
        if (s.c < 0) continue;
        const c = m.commits[s.c];
        const k = authorKey(c);
        const e = who.get(k) ?? { name: c.author || c.email, n: 0 };
        e.n++;
        who.set(k, e);
      }
      rows = [...[...who].sort((a, b) => b[1].n - a[1].n).map(([k, e]) => row(this.colors.get(k) ?? OVERFLOW, e.name, e.n)), row("", "Not committed", unc, true)];
    } else {
      const t = this.since?.t ?? Infinity;
      rows = [row(SINCE[dark ? 1 : 0], "Changed since then", lines((c) => c.time > t)), row("", "Not committed", unc, true)];
    }
    return html`<span class="legend" aria-label="Legend">${rows}</span>`;
  }

  private barClick(e: Event): void {
    const t = e.target as HTMLElement;
    const b = t.closest<HTMLElement>("button");
    if (!b) return;
    if (b.dataset.a === "close") return this.toggle(false);
    const g = b.closest<HTMLElement>(".seg")?.id;
    if (g === "bl-mode") this.mode = b.dataset.v as BlameMode;
    else if (g === "bl-tint") this.tint = b.dataset.v as BlameTint;
    else return;
    this.draw();
  }

  /** The printed line (or picture) under a pointer, by its page's slot. */
  private at(e: MouseEvent): { k: number; s: Seg } | null {
    const slot = (e.target as HTMLElement).closest?.<HTMLElement>(".slot");
    if (!slot) return null;
    const k = Number(slot.dataset.k);
    const r = slot.getBoundingClientRect();
    const { w, h } = this.p.pageSize();
    const x = ((e.clientX - r.left) / r.width) * w, y = ((e.clientY - r.top) / r.height) * h;
    const s = (this.segs.get(k) ?? []).find((s) => x >= Math.min(s.bx - 4, s.x0) && x <= s.x1 && y >= s.y0 && y <= s.y1);
    return s ? { k, s } : null;
  }

  private move(e: PointerEvent): void {
    if (!this.on || this.pinned) return;
    const a = this.at(e);
    if (a?.s === this.hover?.s) return;
    this.over(a, e);
  }

  private over(a: { k: number; s: Seg } | null, e?: MouseEvent): void {
    const was = this.hover;
    this.hover = a;
    if (!a) this.card.hidden = true;
    else if (e) this.show(a, e);
    if (was?.s.para !== a?.s.para || was?.k !== a?.k) this.draw();
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
    this.show(a, e);
    this.draw();
  }

  /** The card for line `a`, near the pointer. */
  private show(a: { k: number; s: Seg }, e: MouseEvent): void {
    const m = this.m;
    if (!m) return;
    const c = a.s.c >= 0 ? m.commits[a.s.c] : null;
    const lk = this.look(a.s.c) ?? { color: UNCOMMITTED, tint: 0 };
    const src = a.s.fig ? `${a.s.fig}` : `${a.s.file}:${a.s.l0 + 1}${a.s.l1 > a.s.l0 ? `–${a.s.l1 + 1}` : ""}`;
    const hashes = this.host.words?.hashes !== false;
    const word = this.host.words?.commit ?? "commit";
    this.card.replaceChildren(
      html`<span class="strip" style="${`background:${lk.hatch ? HATCH_SWATCH : lk.color}`}"></span><div class="sub">${c ? c.subject || "(no subject)" : "Not committed yet"}</div><div class="meta">${c ? html`${hashes ? html`<code>${c.short}</code> · ` : ""}${c.author || c.email} · ${agoWords(c.time, m.now)}` : "Your changes since the last " + word}</div><div class="src">${src}</div><div class="acts">${this.host.compare ? html`<button type="button" data-a="cmp">${c ? `Compare with before this ${word}` : `Compare with the last ${word}`}</button>` : ""}<button type="button" data-a="src">Show in editor</button>${c && hashes ? html`<button type="button" data-a="copy">Copy hash</button>` : ""}</div>`,
    );
    this.card.hidden = false;
    const W = this.card.offsetWidth || 320, H = this.card.offsetHeight || 120;
    const vw = innerWidth, vh = innerHeight;
    this.card.style.left = `${Math.max(6, Math.min(vw - W - 6, e.clientX + 14))}px`;
    this.card.style.top = `${e.clientY + H + 20 > vh ? Math.max(6, e.clientY - H - 12) : e.clientY + 16}px`;
  }

  private act(e: Event): void {
    e.stopPropagation();
    const a = (e.target as HTMLElement).closest<HTMLElement>("button")?.dataset.a;
    const cur = this.pinned ?? this.hover;
    const m = this.m;
    if (!a || !cur || !m) return;
    const c = cur.s.c >= 0 ? m.commits[cur.s.c] : null;
    if (a === "cmp") return this.host.compare?.(c);
    if (a === "src") return this.host.goto(cur.s.fig && !cur.s.fig.includes(":") ? cur.s.fig : cur.s.file.replace(/:.*$/, ""), cur.s.fig ? 1 : cur.s.l0 + 1);
    if (a === "copy" && c)
      void navigator.clipboard?.writeText(c.id).then(
        () => this.p.msg(`Copied ${c.short}`),
        () => this.p.msg(`Hash: ${c.id}`),
      );
  }
}
