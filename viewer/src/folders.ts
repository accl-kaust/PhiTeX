// A folder watch's file manager (`phitex watch DIR`, crates/cli/src/dirwatch.rs):
// what it shows, without the DOM. The watch sends a folder's entries over a
// WebSocket (`folder`); these put them in order, filter them, name their
// state and time, and rank the folder's documents for quick open (Ctrl+P).
// files.ts draws them; the index page (crates/cli/viewer/dir.ts) and the
// viewer's Files panel (crates/cli/viewer/cli.ts) both show it.

/** A document's session, for one opened in this run. */
export interface DocState {
  /** "none" (not built yet), "building", "waiting" (for another document's build), "ok", "errors". */
  build: "none" | "building" | "waiting" | "ok" | "errors";
  errors: number;
  pages: number;
  /** Browser tabs showing it now. */
  tabs: number;
  /** Its session runs (shown, or kept warm); false: stopped. */
  live: boolean;
  message: string | null;
  /** No tab shows it: when it stops by itself (ms since 1970); null: it doesn't. */
  stops_at?: number | null;
}

/** A document whose session runs: the Running strip's row. */
export interface RunningDoc extends DocState {
  path: string;
  id: string;
}

export type Kind = "dir" | "tex" | "pdf" | "image" | "other";

export interface FileEntry {
  name: string;
  /** From the folder watched, `/` between names. */
  path: string;
  kind: Kind;
  /** Modified, ms since 1970. */
  mtime: number | null;
  size: number;
  /** A document's address (`d/ID/`). */
  id?: string;
  doc?: DocState;
}

export interface Folder {
  event: "folder";
  /** The folder watched's own name. */
  root: string;
  /** The folder shown, from the root ("" the root). */
  dir: string;
  entries: FileEntry[];
  /** Every document whose session runs, wherever it is (the Running strip). */
  running?: RunningDoc[];
}

export type SortKey = "name" | "mtime";

/** The breadcrumb: the root, then each folder down to `dir`. */
export function crumbs(root: string, dir: string): { name: string; path: string }[] {
  const out = [{ name: root || "/", path: "" }];
  let at = "";
  for (const part of dir.split("/").filter(Boolean)) {
    at = at ? `${at}/${part}` : part;
    out.push({ name: part, path: at });
  }
  return out;
}

/** The folder above `dir`, or null at the root (never above it). */
export function up(dir: string): string | null {
  if (!dir) return null;
  const k = dir.lastIndexOf("/");
  return k < 0 ? "" : dir.slice(0, k);
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Folders first, then by `by` (names as people sort them: ch2 before ch10); `desc` turns the order, folders still first. */
export function sortEntries(entries: readonly FileEntry[], by: SortKey, desc = false): FileEntry[] {
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

/** The entries whose name holds `q` (any case); all for an empty one. */
export function filterEntries(entries: readonly FileEntry[], q: string): FileEntry[] {
  const n = q.trim().toLowerCase();
  return n ? entries.filter((e) => e.name.toLowerCase().includes(n)) : [...entries];
}

export type Tone = "none" | "busy" | "ok" | "bad" | "idle";

/** How long until `at` (ms since 1970), as the strip says it: "4 min", "20 s". */
export function untilWords(at: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((at - now) / 1000));
  return s >= 60 ? `${Math.ceil(s / 60)} min` : `${s} s`;
}

/** A document's state as the list says it: its words, its tone, and a longer line for its title. */
export function stateOf(d: DocState | undefined, now = Date.now()): { text: string; tone: Tone; title: string } | null {
  if (!d) return null;
  const where = d.tabs > 1 ? `open in ${d.tabs} tabs` : d.tabs === 1 ? "open in a tab" : d.live ? "no tab open: kept ready, it builds when shown again" : "stopped: opening it builds it again";
  const s = (text: string, tone: Tone, more = "") => ({ text, tone, title: [more, where].filter(Boolean).join(" · ") });
  if (!d.live) return s("Stopped", "none", d.message ?? "");
  if (d.tabs === 0 && typeof d.stops_at === "number") return s(`No viewer · stops in ${untilWords(d.stops_at, now)}`, "idle", "No tab shows it: it stops by itself, freeing its memory, unless one opens it first");
  switch (d.build) {
    case "building":
      return s("Building…", "busy");
    case "waiting":
      return s("Waiting…", "busy", "Waiting for another document's build");
    case "ok":
      return s(d.pages === 1 ? "Built · 1 page" : `Built · ${d.pages} pages`, "ok");
    case "errors":
      return d.errors <= 1 && d.message ? s("Failed", "bad", d.message) : s(d.errors === 1 ? "1 error" : `${d.errors} errors`, "bad", d.message ?? "");
    default:
      return s("Not built", "none", d.live ? "Starting" : "");
  }
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const two = (n: number) => String(n).padStart(2, "0");

/** When a file was modified, short and fixed (no "3 min ago" to keep up to date): today's time, Yesterday, this year's day, else the date. */
export function when(mtime: number | null, now = Date.now()): string {
  if (mtime === null) return "";
  const t = new Date(mtime), n = new Date(now);
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(n) - day(t)) / 86400000);
  if (days === 0) return `${two(t.getHours())}:${two(t.getMinutes())}`;
  if (days === 1) return "Yesterday";
  if (t.getFullYear() === n.getFullYear()) return `${MONTHS[t.getMonth()]} ${t.getDate()}`;
  return `${t.getFullYear()}-${two(t.getMonth() + 1)}-${two(t.getDate())}`;
}

/** The time in full, for a title. */
export function whenFull(mtime: number | null): string {
  if (mtime === null) return "";
  const t = new Date(mtime);
  return `${t.getFullYear()}-${two(t.getMonth() + 1)}-${two(t.getDate())} ${two(t.getHours())}:${two(t.getMinutes())}`;
}

/** A file's size, short: 812 B, 4.2 KB, 13 MB. */
export function sizeOf(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1000, k = 0;
  while (v >= 1000 && k < units.length - 1) (v /= 1000), k++;
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[k]}`;
}

/** What a folder holds, for the foot: "3 folders · 5 files". */
export function counts(entries: readonly FileEntry[]): string {
  const dirs = entries.filter((e) => e.kind === "dir").length, files = entries.length - dirs;
  const p = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  return [dirs ? p(dirs, "folder") : "", files ? p(files, "file") : ""].filter(Boolean).join(" · ") || "Empty";
}

/** A match of quick open's query in a path: its score (higher better) and the characters matched. */
export interface Match {
  path: string;
  score: number;
  /** Indexes into `path` of the characters matched. */
  hits: number[];
}

/**
 * Query `q` matched in `path` as its letters in order (any case), each
 * where it scores best: in the file's name over its folders', at a word's
 * start, after the last; spaces in the query separate parts matched in
 * turn. Null if it does not match.
 */
export function fuzzy(q: string, path: string): Match | null {
  const query = q.toLowerCase().replace(/\s+/g, "");
  if (!query) return { path, score: 0, hits: [] };
  const p = path.toLowerCase();
  const nameAt = path.lastIndexOf("/") + 1;
  // (best score of query[i..] matched in p[j..]: a small table, paths are short)
  const n = query.length, m = p.length;
  if (n > m) return null;
  const NEG = -1e9;
  const best: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(NEG));
  const take: boolean[][] = Array.from({ length: n + 1 }, () => new Array<boolean>(m + 1).fill(false));
  for (let j = 0; j <= m; j++) best[n][j] = 0;
  const startOfWord = (j: number) => j === 0 || /[/_\-. ]/.test(p[j - 1]) || (path[j] !== p[j] && path[j - 1] === p[j - 1]);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      let skip = best[i][j + 1];
      let here = NEG;
      if (p[j] === query[i] && best[i + 1][j + 1] > NEG) {
        let s = 1;
        if (j >= nameAt) s += 2;
        if (startOfWord(j)) s += 3;
        // (the next letter right after this one: a run)
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
  const hits: number[] = [];
  // (the way the table went: a letter taken where taking it scored best)
  for (let i = 0, j = 0; i < n && j < m; j++) {
    if (take[i][j]) {
      hits.push(j);
      i++;
    }
  }
  // (shorter paths and names first among equals)
  const score = best[0][0] * 100 - path.length - (m - nameAt);
  return { path, score, hits };
}

/** The paths matching `q`, best first, at most `limit`; all of them in order for an empty query. */
export function rank(q: string, paths: readonly string[], limit = 50): Match[] {
  if (!q.trim()) return [...paths].sort((a, b) => collator.compare(a, b)).slice(0, limit).map((path) => ({ path, score: 0, hits: [] }));
  const out: Match[] = [];
  for (const p of paths) {
    const m = fuzzy(q, p);
    if (m) out.push(m);
  }
  out.sort((a, b) => b.score - a.score || collator.compare(a.path, b.path));
  return out.slice(0, limit);
}

/** The text of `s` split at the characters at `hits`: [text, matched] runs, for drawing the matched ones bold. */
export function runs(s: string, hits: readonly number[], from = 0): [string, boolean][] {
  const set = new Set(hits);
  const out: [string, boolean][] = [];
  for (let k = 0; k < s.length; k++) {
    const on = set.has(k + from);
    const last = out[out.length - 1];
    if (last && last[1] === on) last[0] += s[k];
    else out.push([s[k], on]);
  }
  return out;
}
