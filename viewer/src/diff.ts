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

export type DiffReply = { tex: string; changes: DiffChange[] } | { error: string };

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
