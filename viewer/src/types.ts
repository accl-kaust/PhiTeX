// The page types the renderer shares with its hosts (the Overleaf
// extension's session, the CLI's viewer page).

/** A page as PhiTeX's first core drew it (v1), in PDF points from the top left. */
export interface Draws {
  w: number;
  h: number;
  /** Font names (Times-Roman, ...), indexed by `t`'s font. */
  f: string[];
  /** Words: x, y (baseline), size, font, text, and the width PhiTeX laid it out with. */
  t: [number, number, number, number, string, number?][];
  /** Rules: x, y (top), width, height. */
  r: [number, number, number, number][];
}

/** A page: its draw list, or a PNG (PDF mode: rendered by pdf.js, `w`/`h` its size in PDF points). */
export type PageImage = { draws: Draws } | { png: Uint8Array; w?: number; h?: number } | { canvas: HTMLElement; w: number; h: number };
