// "Did you know" while a first build runs (panel.ts, under the opening's
// step): PhiTeX's features and a little LaTeX, each checked true. A hint
// may name the hosts it holds in (a key only the watch's page has). The
// user's list will grow; keep each one short, true and checked.

export interface Hint {
  text: string;
  /** The hosts it is true in (none: every one). */
  hosts?: ("overleaf" | "vscode" | "watch")[];
}

export const HINTS: Hint[] = [
  { text: "Double-click a word on the page to go to its source." },
  { text: "Builds run by themselves as you type: Rebuild is for a change made outside the editor." },
  { text: "Press ` to see everything phitex watch printed in the terminal.", hosts: ["watch"] },
  { text: "Press ? over the pages for the keyboard shortcuts.", hosts: ["watch"] },
  { text: "Compare shows what changed since an earlier version, marked as latexdiff marks it.", hosts: ["watch", "vscode"] },
  { text: "Put \\label after \\caption: \\ref then gives the figure's number, not the section's." },
  { text: "A ~ is a space the line never breaks at: Figure~\\ref{fig:plot}." },
  { text: "Donald Knuth began TeX in 1977; its version number gets one more digit of π at each release (3.141592653)." },
];

/** The hints true in `host`. */
export function hintsFor(host: "overleaf" | "vscode" | "watch"): string[] {
  return HINTS.filter((h) => !h.hosts || h.hosts.includes(host)).map((h) => h.text);
}
