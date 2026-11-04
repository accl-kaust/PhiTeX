// guessMain: the project's main file, never a file the main one inputs
// (Overleaf's compile went to the file open when it was a whole document of
// its own: a subfiles chapter, or two documents side by side).
import { test } from "node:test";
import assert from "node:assert/strict";
import { guessMain } from "../src/session.ts";

const doc = (body: string, cls = "article") => `\\documentclass{${cls}}\n\\begin{document}\n${body}\n\\end{document}\n`;

test("a subfiles chapter is not the main file, whatever its name or folder", () => {
  assert.equal(guessMain({ "chapter.tex": doc("Ch.", "[paper]{subfiles}"), "paper.tex": doc("\\subfile{chapter}") }), "paper.tex");
  assert.equal(guessMain({ "main.tex": doc("Ch.", "[thesis]{subfiles}"), "thesis.tex": doc("\\include{main}") }), "thesis.tex");
});

test("the document that inputs the others beats one beside it", () => {
  assert.equal(guessMain({ "cover.tex": doc("Letter."), "intro.tex": "Intro.\n", "paper.tex": doc("\\input{intro}") }), "paper.tex");
  assert.equal(guessMain({ "a.tex": doc("A."), "sec/one.tex": "One.\n", "z.tex": doc("\\input{sec/one}") }), "z.tex");
});

test("a commented-out input counts for nothing; one document is the main one", () => {
  assert.equal(guessMain({ "a.tex": doc("% \\input{b}\nA."), "b.tex": doc("B."), "main.tex": doc("M.") }), "main.tex");
  assert.equal(guessMain({ "only.tex": doc("x") }), "only.tex");
  assert.equal(guessMain({ "x.bib": "" }), null);
});
