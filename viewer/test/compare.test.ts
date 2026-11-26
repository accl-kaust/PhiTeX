// The Diff's problems at the user's own lines (compare.ts's diffDiagnostics):
// a place of the marked-up text taken through the diff's map.
import { test } from "node:test";
import assert from "node:assert/strict";
import { diffDiagnostics } from "../src/compare.ts";
import type { DiffOut } from "../src/diff.ts";
import type { Diagnostic } from "../src/diagnostics.ts";

// (the user's main.tex, and the marked-up text: a head of markup, then the
// user's lines, the second with an added word)
const user = "\\documentclass{article}\n\\begin{document}\nOne \\foo here.\nTwo more.\n\\end{document}\n";
const head = "\\documentclass{article}\n%DIF PREAMBLE\n\\def\\DIFadd#1{#1}\n%DIF END\n\\begin{document}\n";
const tex = head + "One \\foo here.\nTwo \\DIFadd{more}.\n\\end{document}\n";
const b = (s: string, t = tex) => new TextEncoder().encode(t.slice(0, t.indexOf(s))).length;
const d: DiffOut = {
  tex,
  files: ["main.tex"],
  map: [
    [0, b("%DIF PREAMBLE"), 0, 0],
    [b("\\begin{document}"), b("Two ") - b("\\begin{document}"), 0, b("\\begin{document}", user)],
  ],
  changes: [],
};
const texts = (f: string) => (f === "main.tex" ? user : undefined);
const at = (line: number, col: number, range?: [number, number]): Diagnostic => ({
  severity: "error",
  code: "tex",
  message: "Undefined control sequence",
  file: "main.tex",
  line,
  problem: { severity: "error", code: "tex", message: "Undefined control sequence", notes: [], help: [], suggestions: [], file: "main.tex", line, col, range, excerpt: { text: "", start: col - 1, len: 4 }, context: [], included: [], box: null },
});

test("a problem of the marked-up text is at the user's line, its excerpt that line", () => {
  const line = tex.slice(0, tex.indexOf("One")).split("\n").length;
  const [x] = diffDiagnostics([at(line, 5, [b("\\foo"), b("\\foo") + 4])], d, "main.tex", texts);
  assert.equal(x.file, "main.tex");
  assert.equal(x.line, 3);
  assert.deepEqual(x.problem?.excerpt, { text: "One \\foo here.", start: 4, len: 4 });
  // (by its line and column where the build gave no bytes)
  const [y] = diffDiagnostics([at(line, 5)], d, "main.tex", texts);
  assert.equal(y.line, 3);
  assert.equal(y.problem?.col, 5);
  // (an empty range, [0, 0], is no place: its line and column are)
  const [z] = diffDiagnostics([at(line, 5, [0, 0])], d, "main.tex", texts);
  assert.equal(z.line, 3);
});

test("a problem in the markup's own definitions has no line of the user's; another file's is as it was", () => {
  const map = { ...d, map: d.map!.slice(1) };
  const [x] = diffDiagnostics([at(3, 1)], map, "main.tex", texts);
  assert.equal(x.line, undefined);
  assert.equal(x.problem?.excerpt, null);
  const other = { ...at(2, 1), file: "refs.bib" };
  assert.deepEqual(diffDiagnostics([other], d, "main.tex", texts), [other]);
});
