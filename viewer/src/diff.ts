// PhiTeX's latexdiff (phitex-diff, built as dist/diff.wasm: diff-wasm/):
// two versions' files in, the marked-up .tex (latexdiff's markup, flattened)
// and its changes out. Run in the core's host (corehost.ts: the offscreen
// document, an extension page where wasm is allowed; VS Code's extension
// host), for the compare (compare.ts). Offsets are UTF-8 bytes.

export interface DiffLoc {
  file: string;
  start: number;
  end: number;
}

export interface DiffChange {
  kind: "add" | "del" | "change";
  old: DiffLoc;
  new: DiffLoc;
  old_text: string;
  new_text: string;
  section: string | null;
  /** Its range in `tex`. */
  out: [number, number];
}

/** latexdiff's --type: how a change is marked. */
export type DiffMarkup = "underline" | "ctraditional" | "traditional" | "cfont" | "fontstrike" | "bold" | "changebar" | "culinechbar" | "invisible";
/** latexdiff's --subtype: what else marks it. */
export type DiffSubtype = "safe" | "color" | "marker";

/** How the diff looks: latexdiff's type and subtype, and the colors (an xcolor name or #RRGGBB). */
export interface DiffStyle {
  markup?: DiffMarkup;
  subtype?: DiffSubtype;
  add_color?: string;
  del_color?: string;
}

export interface DiffReq extends DiffStyle {
  old: Record<string, string>;
  new: Record<string, string>;
  main: string;
}

/**
 * The diff: the marked-up text, its changes, and the map back to the
 * user's files (phitex-diff's DiffOut): `map`'s runs `[out, len, file,
 * src]` are `len` bytes of `tex` at `out` that are those at `src` in
 * `files[file]`, in order.
 */
export interface DiffOut {
  tex: string;
  changes: DiffChange[];
  files?: string[];
  map?: [number, number, number, number][];
}

export type DiffReply = DiffOut | { error: string };

/**
 * Where byte `out` of the marked-up text is in the user's files, as
 * phitex-diff's DiffOut::source: in a run copied as it is, its very place;
 * in a change's markup, the change's place in the new version; in
 * latexdiff's own markup, the end of the run before it.
 */
export function diffSource(d: DiffOut, out: number): { file: string; at: number } | null {
  const map = d.map ?? [], files = d.files ?? [];
  let lo = 0, hi = map.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (map[mid][0] <= out) lo = mid + 1;
    else hi = mid;
  }
  const r = map[lo - 1];
  if (r && out < r[0] + r[1] && files[r[2]] !== undefined) return { file: files[r[2]], at: r[3] + (out - r[0]) };
  const c = d.changes.find((c) => out >= c.out[0] && out < c.out[1]);
  if (c) return { file: c.new.file, at: c.new.start };
  return r && files[r[2]] !== undefined ? { file: files[r[2]], at: r[3] + r[1] } : null;
}

/**
 * Where byte `at` of the user's `file` is in the marked-up text (the
 * editor's cursor followed on the diff's pages): in a run copied as it is,
 * its place; else in a change there, the change's markup; else the end of
 * the run before it in that file.
 */
export function diffPlace(d: DiffOut, file: string, at: number): number | null {
  const map = d.map ?? [], files = d.files ?? [];
  let before: number | null = null, best = -1;
  for (const [out, len, f, src] of map) {
    if (files[f] !== file) continue;
    if (at >= src && at < src + len) return out + (at - src);
    if (src + len <= at && src > best) [best, before] = [src, out + len];
  }
  const c = d.changes.find((c) => c.new.file === file && at >= c.new.start && at <= c.new.end);
  return c ? c.out[0] : before;
}

interface Exports {
  memory: WebAssembly.Memory;
  ph_alloc(n: number): number;
  ph_diff(p: number, n: number): number;
  ph_out(): number;
}

/** The diff, from diff.wasm's bytes: one call at a time (its reply buffer is reused). */
export async function loadDiff(wasm: BufferSource): Promise<(r: DiffReq) => DiffReply> {
  const { instance } = await WebAssembly.instantiate(wasm, {});
  const x = instance.exports as unknown as Exports;
  return (r) => {
    const req = new TextEncoder().encode(JSON.stringify(r));
    const p = x.ph_alloc(req.length);
    new Uint8Array(x.memory.buffer, p >>> 0, req.length).set(req);
    const n = x.ph_diff(p, req.length);
    return JSON.parse(new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.ph_out() >>> 0, n))) as DiffReply;
  };
}
