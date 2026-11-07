// The project's latexmk rc files as the core's open takes them: the root's
// .latexmkrc (else latexmkrc) and what it loads by a ./ or ../ path, followed
// once each; a bare name, or one out of the project, not given (the core says
// why). The core's notes as the panel's info diagnostics.
import { test } from "node:test";
import assert from "node:assert/strict";
import { latexmkFiles, latexmkNotes } from "../src/latexmkrc.ts";

test("no rc file at the root: nothing given", () => {
  assert.equal(latexmkFiles({ "main.tex": "", "lab/.latexmkrc": "$pdf_mode = 1;\n" }), undefined);
});

test("the root's .latexmkrc over latexmkrc, and what it does or requires", () => {
  const files = {
    ".latexmkrc": 'do "./conf/paths.rc";\n# do "./conf/off.rc";\nrequire("./conf/../conf/more.rc");\ndo "bare.rc";\ndo "../outside.rc";\n',
    latexmkrc: "$pdf_mode = 5;\n",
    "conf/paths.rc": "ensure_path('TEXINPUTS', './common/tex//');\ndo './.latexmkrc';\n",
    "conf/more.rc": "$bibtex_use = 2;\n",
    "conf/off.rc": "",
    "bare.rc": "",
    "main.tex": "",
  };
  assert.deepEqual(Object.keys(latexmkFiles(files)!), [".latexmkrc", "conf/paths.rc", "conf/more.rc"]);
  assert.deepEqual(Object.keys(latexmkFiles({ latexmkrc: "", "main.tex": "" })!), ["latexmkrc"]);
});

test("the core's notes: at their file and line, or the project's", () => {
  assert.deepEqual(latexmkNotes([".latexmkrc:3: $pdf_mode not applied (phitex chooses the engine itself)", "TEXINPUTS: /etc/tex: not searched: outside the project"]), [
    { severity: "info", code: "latexmkrc", file: ".latexmkrc", line: 3, message: "latexmkrc: $pdf_mode not applied (phitex chooses the engine itself)" },
    { severity: "info", code: "latexmkrc", message: "latexmkrc: TEXINPUTS: /etc/tex: not searched: outside the project" },
  ]);
});
