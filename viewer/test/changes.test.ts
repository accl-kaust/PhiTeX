// The Diff's changes as Changes lists them (changes.ts's itemOf): each
// change's kind from its text, the bibliography one of them.
import { test } from "node:test";
import assert from "node:assert/strict";
import { itemOf } from "../src/changes.ts";
import type { DiffChange } from "../src/diff.ts";

const add = (new_text: string, section: string | null = "Results"): DiffChange => ({
  kind: "add",
  old: { file: "main.tex", start: 0, end: 0 },
  new: { file: "main.tex", start: 0, end: new_text.length },
  old_text: "",
  new_text,
  section,
  out: [0, 1],
});

test("a change's kind: a float, a display, a section, the bibliography, text", () => {
  const kinds = [
    "\\begin{figure}[h]\n\\includegraphics{a}\n\\caption{A.}\n\\end{figure}",
    "\\begin{table}[h]\n\\begin{tabular}{lr}\na & 1 \\\\\n\\end{tabular}\n\\end{table}",
    "\\begin{equation}\nx = y\n\\end{equation}",
    "\\section{Results}",
    "\\bibliographystyle{plain}\n\\bibliography{refs}",
    "\\printbibliography",
    "Table~\\ref{t} shows it~\\cite{k}.",
  ].map((t) => itemOf(add(t), 1).kind);
  assert.deepEqual(kinds, ["Figure", "Table", "Equation", "Section", "Bibliography", "Bibliography", "Text"]);
});

test("a change is filed under the section phitex-diff gives it", () => {
  assert.equal(itemOf(add("I can write math:", "Introduction"), 1).sec, "Introduction");
});
