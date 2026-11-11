// "Did you know" while a first build runs (panel.ts, under the opening's
// step): PhiTeX's features and a little LaTeX, each checked true. A hint
// may name the hosts it holds in (a key only the watch's page has). The
// user's list will grow; keep each one short, true and checked. A key
// is written {KEY} and drawn as a key cap ([`keyed`]): after "Did you
// know?" a bare ? (or `) read as punctuation.

export interface Hint {
  text: string;
  /** The hosts it is true in (none: every one). */
  hosts?: ("overleaf" | "vscode" | "watch")[];
}

export const HINTS: Hint[] = [
  { text: "Double-click a word on the page to go to its source." },
  { text: "Builds run by themselves as you type: Rebuild is for a change made outside the editor." },
  { text: "Press {`} to see everything phitex watch printed in the terminal.", hosts: ["watch"] },
  { text: "Press {?} over the pages for the keyboard shortcuts.", hosts: ["watch"] },
  { text: "Compare shows what changed since an earlier version, marked as latexdiff marks it.", hosts: ["watch", "vscode"] },
  { text: "Put \\label after \\caption: \\ref then gives the figure's number, not the section's." },
  { text: "A ~ is a space the line never breaks at: Figure~\\ref{fig:plot}." },
  { text: "You can pick your favourite loading animation, or turn them off, in Settings › Appearance." },
  { text: "Donald Knuth began TeX in 1977; its version number gets one more digit of π at each release (3.141592653)." },
];

/** The hints true in `host`. */
export function hintsFor(host: "overleaf" | "vscode" | "watch"): string[] {
  return HINTS.filter((h) => !h.hosts || h.hosts.includes(host)).map((h) => h.text);
}

/** `text` with its keys ({KEY}) as plain text, in brackets: where a
 * hint is typed or drawn letter by letter. */
export function plain(text: string): string {
  return text.replace(/\{([^{}]+)\}/g, "[$1]");
}

/** `el` holding `text` (after `before`), each key ({KEY}) a key cap: a
 * `<kbd>` drawn as a small raised button in the text's own font size. */
export function keyed(el: Element, text: string, before = ""): void {
  if (!document.getElementById("phx-keycap")) {
    const st = document.createElement("style");
    st.id = "phx-keycap";
    st.textContent = `kbd.phx-key { display: inline-block; min-width: 1.1em; padding: 0 .35em; margin: 0 .1em;
      font: inherit; font-style: normal; font-weight: 600; line-height: 1.35; text-align: center;
      border: 1px solid currentColor; border-bottom-width: 2px; border-radius: .3em; opacity: .9; }`;
    document.head.append(st);
  }
  const parts = text.split(/\{([^{}]+)\}/);
  el.replaceChildren(
    before,
    ...parts.map((t, k) => (k % 2 ? Object.assign(document.createElement("kbd"), { className: "phx-key", textContent: t }) : t)),
  );
}
