// The page types the renderer shares with its hosts (the Overleaf
// extension's session, the CLI's viewer page).

/**
 * A link on a page, in PDF points from its top left: its box, and where it
 * goes: a web address, or a page (from 0) and the top there (null: none).
 */
export type Link = [number, number, number, number, string | number, (number | null)?];

/** A page: composed from its strips (strips.ts) on a canvas, `w`/`h` its size in PDF points, its links. */
export type PageImage = { canvas: HTMLElement; w: number; h: number; links?: Link[] };
