// Blame on the page (blame.ts): the pages' printed lines blamed through
// the glyph origins (bytes of each file, a line of it), a float's picture
// as its image file, a bibliography entry as its .bib entry, a macro's text
// at its use; the colours (age steps, an author's by the hash of the email,
// grey past six authors, "changed since"); and the hosts' parsers: git's
// porcelain blame (VS Code) and Overleaf's history diff.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  AGE, AGE_RAMPS, AUTHOR_COLORS, Model, OVERFLOW, UNCOMMITTED, ageStep, agoWords, authorColors, bibEntries, bibFiles, bibItems, byteIndex, floats,
  historyBlame, imageNames, lineOf, lineStarts, linesOf, lookOf, mapPage, matchLines, parsePorcelain, printedLines, readerWords, sinceTime, wordRuns, type BlameCommit, type BlameData,
} from "../src/blame.ts";
import type { Glyph } from "../src/sync.ts";

const DAY = 86400;
const NOW = 1_800_000_000;
const commit = (id: string, author: string, days: number, subject = `by ${author}`): BlameCommit => ({ id: id.padEnd(40, "0"), short: id.slice(0, 7), author, email: `${author.toLowerCase()}@example.com`, time: NOW - days * DAY, subject });

/** A glyph of `file` at byte `start`, its origin at (x, y). */
const g = (x: number, y: number, file: string | null, start: number, synth = false): Glyph => ({ x, y, file, start, end: start + 1, synth });

test("lines are counted in UTF-8 bytes, as the glyph origins count them", () => {
  // ("é" is two bytes, "𝑌" four: a UTF-16 surrogate pair)
  assert.deepEqual(lineStarts("é\nb\n"), [0, 3, 5]);
  assert.deepEqual(lineStarts("𝑌\nx"), [0, 5]);
  assert.equal(lineOf([0, 3, 5], 0), 0);
  assert.equal(lineOf([0, 3, 5], 2), 0);
  assert.equal(lineOf([0, 3, 5], 3), 1);
  assert.equal(lineOf([0, 3, 5], 99), 2);
  const b = byteIndex("a𝑌b");
  assert.deepEqual([...b], [0, 1, 1, 5, 6]);
});

test("age steps: under a day, a week, a month, six months; older gets no bar", () => {
  assert.equal(ageStep(0.1), 0);
  assert.equal(ageStep(2), 1);
  assert.equal(ageStep(20), 2);
  assert.equal(ageStep(120), 3);
  assert.equal(ageStep(400), 4);
  assert.equal(AGE[4].bar, null);
  assert.deepEqual(AGE.slice(0, 4).map((a) => a.bar![0]), ["#5B2A86", "#7B4FAE", "#A07CC9", "#C9B5E2"]);
  assert.deepEqual(AGE.slice(0, 4).map((a) => a.bar![1]), ["#C7A6F0", "#AE8AD8", "#8E6CB8", "#6B5390"]);
});

test("an author keeps one colour, chosen by the email's hash; past six authors, grey", () => {
  const six = ["a@x", "b@x", "c@x", "d@x", "e@x", "f@x"];
  const one = authorColors(six);
  const two = authorColors([...six].reverse());
  for (const k of six) assert.equal(one.get(k), two.get(k), k);
  assert.deepEqual(new Set(one.values()), new Set(AUTHOR_COLORS));
  const seven = authorColors([...six, "g@x"]);
  assert.equal([...seven.values()].filter((c) => c === OVERFLOW).length, 1);
  assert.ok(!AUTHOR_COLORS.includes("#F0E442"), "no yellow");
});

test("a line's look: age ramps light and dark, uncommitted hatched or hidden, authors' colours; changed since's time", () => {
  const m = new Model();
  m.add({ commits: [commit("a1", "Alice", 0.2), commit("b2", "Bob", 400)], files: {}, now: NOW });
  const colors = authorColors(["alice@example.com", "bob@example.com"]);
  const look = (o: object = {}) => ({ color: "age" as const, ages: "violet" as const, wip: "show" as const, ...o });
  assert.deepEqual(lookOf(m, 0, look(), false, colors), { color: "#5B2A86", tint: 0.22 });
  assert.equal(lookOf(m, 0, look(), true, colors)!.color, "#C7A6F0");
  assert.equal(lookOf(m, 0, look({ ages: "warm" }), false, colors)!.color, AGE_RAMPS.warm.light[0]);
  assert.equal(lookOf(m, 0, look({ ages: "grey" }), true, colors)!.color, AGE_RAMPS.grey.dark[0]);
  assert.equal(lookOf(m, 1, look(), false, colors), null);
  assert.deepEqual(lookOf(m, -1, look(), false, colors), { color: UNCOMMITTED, tint: 0.12, hatch: true });
  assert.equal(lookOf(m, -1, look({ wip: "hide" }), false, colors), null);
  assert.equal(lookOf(m, 1, look({ color: "author" }), false, colors)!.color, colors.get("bob@example.com"));
  assert.equal(sinceTime("off"), null);
  assert.equal(sinceTime(`${NOW - DAY}:v1-submitted`), NOW - DAY);
});

test("finer than lines: runs give each word its writer, each line its newest edit, spacing ignored or counted", () => {
  // (Alice wrote the line; Bob changed one word in it; Carol added a space)
  const text = "The slow  fox\nNext line\n";
  const runs: [number, number, number][] = [[0, 4, 0], [4, 8, 1], [8, 9, 0], [9, 10, 2], [10, 25, 0]];
  const times = [NOW - 9 * DAY, NOW - 2 * DAY, NOW - DAY];
  const newer = (a: number, b: number) => (a < 0 ? a : b < 0 ? b : times[b] > times[a] ? b : a);
  // (a word is its newest writer's, whole: "slow" Bob's; the space Carol's alone)
  assert.deepEqual(wordRuns("ab cd", [[0, 1, 0], [1, 2, 1], [2, 5, 0]], newer), [[0, 2, 1], [2, 5, 0]]);
  assert.deepEqual(linesOf(text, runs, "ignore", newer), [1, 0]);
  assert.deepEqual(linesOf(text, runs, "count", newer), [2, 0]);
  // (the model looks a glyph up by its byte: Word detail, "slow" is Bob's, "The" Alice's)
  const m = new Model();
  m.detail = "word";
  m.add({ commits: [commit("a1", "Alice", 9), commit("b2", "Bob", 2), commit("c3", "Carol", 1)], files: { "w.tex": { text, runs } }, now: NOW });
  assert.equal(m.of(g(0, 0, "w.tex", 5))!.c, 1);
  assert.equal(m.of(g(0, 0, "w.tex", 1))!.c, 0);
  assert.equal(m.of(g(0, 0, "w.tex", 15))!.line, 1);
  // (Line detail: the line's commit, as git blame gives it)
  const l = new Model();
  l.add({ commits: [commit("a1", "Alice", 9), commit("b2", "Bob", 2), commit("c3", "Carol", 1)], files: { "w.tex": { text, runs } }, now: NOW });
  assert.equal(l.of(g(0, 0, "w.tex", 1))!.c, 1);
});

test("printed lines: one baseline (within a point), split a column apart", () => {
  const ls = printedLines([g(10, 100, "m", 0), g(15, 100.2, "m", 1), g(160, 100, "m", 2), g(10, 112, "m", 3)]);
  assert.deepEqual(ls.map((l) => l.map((x) => x.start)), [[0, 1], [2], [3]]);
  // (glyphs either side of a half point: one line, not two interleaved)
  const two = printedLines([g(10, 189.74, "m", 0), g(15, 189.76, "m", 1), g(20, 189.74, "m", 2), g(25, 189.76, "m", 3)]);
  assert.deepEqual(two.map((l) => l.map((x) => x.start)), [[0, 1, 2, 3]]);
});

test("floats: the environment's bytes, its caption's (brace-matched), its images and inputs", () => {
  const t = "Text é.\n\\begin{figure}[t]\n\\includegraphics[width=\\linewidth]{fig/plot}\n\\input{fig/tikz.tex}\n\\caption{A {nested} caption.}\n\\end{figure}\nAfter.\n";
  const [f] = floats("main.tex", t);
  const b = (s: string) => new TextEncoder().encode(t.slice(0, t.indexOf(s))).length;
  assert.equal(f.from, b("\\begin{figure}"));
  assert.equal(f.to, b("\nAfter."));
  assert.deepEqual(f.caption, [b("\\caption"), b("\n\\end{figure}")]);
  assert.deepEqual(f.images, ["fig/plot"]);
  assert.deepEqual(f.inputs, ["fig/tikz.tex"]);
  assert.deepEqual(imageNames("fig/plot").slice(0, 2), ["fig/plot.pdf", "fig/plot.png"]);
  assert.deepEqual(imageNames("a.png", ["img/"]), ["img/a.png", "a.png"]);
});

test("the bibliography: \\bibitem keys in the .bbl, entries' lines in the .bib", () => {
  assert.deepEqual(bibFiles("\\bibliography{refs,more.bib}\n\\addbibresource{x.bib}"), ["refs.bib", "more.bib", "x.bib"]);
  const bbl = "\\begin{thebibliography}{1}\n\\bibitem[Knuth(1984)]{knuth}\nD. Knuth.\n\\bibitem{lamport}\nL. Lamport.\n";
  assert.deepEqual(bibItems(bbl).map(([, k]) => k), ["knuth", "lamport"]);
  const bib = "@book{knuth,\n  title={TeX},\n}\n\n@comment{x,}\n@manual{lamport,\n  title={LaTeX}\n}\n";
  const e = bibEntries(bib);
  assert.deepEqual(e.get("knuth"), [0, 2]);
  assert.deepEqual(e.get("lamport"), [5, 7]);
});

/** A two-column page's model: main.tex's lines by commit, a figure, a bibliography. */
function model(): { m: Model; main: string } {
  const main = [
    "\\documentclass{article}", // 0
    "\\begin{document}", // 1
    "Carol's first draft of a paragraph.", // 2
    "Alice's rewrite.", // 3
    "\\begin{figure}", // 4
    "\\includegraphics{fig}", // 5
    "\\caption{Dana's caption.}", // 6
    "\\end{figure}", // 7
    "Not committed yet.", // 8
    "\\bibliography{refs}", // 9
    "\\end{document}", // 10
  ].join("\n") + "\n";
  const commits = [commit("c0", "Carol", 120), commit("a1", "Alice", 0.1), commit("d3", "Dana", 9), commit("b4", "Bob", 6), commit("e5", "Eve", 2)];
  const d: BlameData = {
    commits,
    files: {
      "main.tex": { text: main, lines: [0, 0, 0, 1, 0, 0, 2, 0, -1, 0, 0] },
      "fig.pdf": { last: 3 },
      "main.bbl": { text: "\\begin{thebibliography}{9}\n\\bibitem{knuth}\nD. Knuth. The TeXbook.\n" },
      "refs.bib": { text: "@book{knuth,\n  title={The TeXbook},\n}\n", lines: [0, 4, 0] },
    },
    now: NOW,
  };
  const m = new Model();
  m.add(d);
  return { m, main };
}

test("a page blamed: each printed line its source line's commit, a macro's text at its use", () => {
  const { m, main } = model();
  const at = (s: string) => new TextEncoder().encode(main.slice(0, main.indexOf(s))).length;
  const page = [
    // (left column: Carol's line, then Alice's; the second's last glyph a macro's, at its use)
    g(54, 100, "main.tex", at("Carol's")), g(60, 100, "main.tex", at("Carol's") + 1),
    g(54, 112, "main.tex", at("Alice's")), g(60, 112, "main.tex", at("Alice's") + 1), g(66, 112, "main.tex", at("Alice's") + 3, true),
    g(54, 124, "main.tex", at("Not committed")),
    // (the right column: the figure's caption under its picture, then the bibliography)
    g(318, 100, "main.tex", at("Carol's") + 2), g(318, 112, "main.tex", at("Carol's") + 3), g(318, 124, "main.tex", at("Carol's") + 4),
    g(318, 200, "main.tex", at("Dana's")), g(324, 200, "main.tex", at("Dana's") + 1),
    g(318, 300, "main.bbl", new TextEncoder().encode("\\begin{thebibliography}{9}\n\\bibitem{knuth}\nD").length),
  ];
  const segs = mapPage(page, m, { w: 612, h: 792 });
  const find = (y: number, x: number) => segs.find((s) => !s.fig && s.y0 < y && y < s.y1 && s.x0 <= x && x <= s.x1)!;
  assert.equal(find(100, 54).c, 0);
  assert.equal(find(112, 54).c, 1);
  assert.equal(find(112, 54).l0, 3);
  assert.equal(find(124, 54).c, -1, "not committed");
  // (the bars at each column's left edge, 7 pt out)
  assert.equal(find(100, 54).bx, 47);
  assert.equal(find(100, 318).bx, 311);
  // (the caption: its own line's commit; the space over it: the image's last change)
  assert.equal(find(200, 318).c, 2);
  const fig = segs.find((s) => s.fig)!;
  assert.equal(fig.fig, "fig.pdf");
  assert.equal(fig.c, 3);
  assert.ok(fig.y0 >= 127 - 0.5 && fig.y1 <= 190 && fig.y1 > fig.y0, `${fig.y0}..${fig.y1}`);
  // (the bibliography entry: its .bib entry's newest line)
  const bib = find(300, 318);
  assert.equal(bib.file, "refs.bib");
  assert.equal(bib.c, 4);
  // (a paragraph: a column's lines in a row with one commit)
  assert.equal(find(100, 318).para, find(124, 318).para);
  assert.notEqual(find(100, 54).para, find(112, 54).para);
});

test("the tint's runs: a printed line's words by their own commits", () => {
  const { m, main } = model();
  const at = (s: string) => new TextEncoder().encode(main.slice(0, main.indexOf(s))).length;
  const [s] = mapPage([g(54, 100, "main.tex", at("Carol's")), g(59, 100, "main.tex", at("Carol's") + 1), g(64, 100, "main.tex", at("Alice's")), g(69, 100, "main.tex", at("Alice's") + 1)], m, { w: 612, h: 792 });
  assert.equal(s.c, 1, "the line's bar: its newest commit");
  assert.deepEqual(s.runs.map((r) => r[2]), [0, 1]);
});

test("git blame --porcelain: each line's commit, the working tree's not committed", () => {
  const a = "a".repeat(40), z = "0".repeat(40);
  const out = [
    `${a} 1 1 2`, "author Alice", "author-mail <alice@example.com>", "author-time 1700000000", "author-tz +0000", "summary First draft", "filename main.tex", "\tline one",
    `${a} 2 2`, "\tline two",
    `${z} 3 3 1`, "author Not Committed Yet", "author-mail <not.committed.yet>", "author-time 1800000000", "summary Version of main.tex from main.tex", "filename main.tex", "\tline three",
  ].join("\n");
  const r = parsePorcelain(out);
  assert.deepEqual(r.lines, [0, 0, -1]);
  assert.equal(r.commits.length, 1);
  assert.deepEqual(r.commits[0], { id: a, short: "aaaaaaa", author: "Alice", email: "alice@example.com", time: 1700000000, subject: "First draft" });
});

test("Overleaf's history diff as a blame: a line takes its newest insert", () => {
  const first = commit("v0", "Carol", 100, "The first version");
  const by = (who: string, days: number) => ({ users: [{ first_name: who, email: `${who.toLowerCase()}@example.com` }], start_ts: (NOW - days * DAY) * 1000, end_ts: (NOW - days * DAY) * 1000 });
  const r = historyBlame(
    [{ u: "Old line.\nOld " }, { i: "new words", meta: by("Alice", 1) }, { u: " here.\n" }, { d: "gone\n", meta: by("Bob", 2) }, { i: "Bob's line.\n", meta: by("Bob", 2) }, { u: "Last.\n" }],
    first,
    (meta) => ({ ...commit(meta.users![0].first_name!.toLowerCase(), meta.users![0].first_name!, (NOW - meta.end_ts! / 1000) / DAY) }),
  );
  assert.equal(r.text, "Old line.\nOld new words here.\nBob's line.\nLast.\n");
  assert.deepEqual(r.lines.map((k) => r.commits[k].author), ["Carol", "Alice", "Bob", "Carol"]);
});

test("ages in words", () => {
  assert.equal(agoWords(NOW - 3 * 3600, NOW), "3 hours ago");
  assert.equal(agoWords(NOW - 9 * DAY, NOW), "9 days ago");
  assert.equal(agoWords(NOW - 20, NOW), "just now");
});

test("the editor's lines matched in the history's: those it lacks are not committed", () => {
  const m = matchLines(["a", "b", "c", "d"], ["a", "x", "b", "d", "e"]);
  assert.deepEqual([...m].sort((p, q) => p[0] - q[0]), [[0, 0], [2, 1], [3, 3]]);
  assert.deepEqual([...matchLines([], ["a"])], []);
});

test("a reader's words: runs of letters; no numbers, commands, comments, options, key=value, names", () => {
  const words = (t: string) => {
    const w = readerWords(t);
    const out: string[] = [];
    for (let i = 0; i < w.length; i += 2) out.push(t.slice(w[i], w[i + 1]));
    return out;
  };
  // (prose, its section title, a citation's key and a label's name not)
  assert.deepEqual(words("\\section{Our results} We don't wait~\\cite{knuth84}.\\label{sec:res} % a comment here\n"), ["Our", "results", "We", "don't", "wait"]);
  // (a table's column spec is no word)
  assert.deepEqual(words("\\begin{tabular}{lrr} Name & 1 \\\\ \\end{tabular}\\begin{tabularx}{\\linewidth}{lX} Width"), ["Name", "Width"]);
  // (a generated table of numbers: nothing; a 3D or x1: not words)
  assert.deepEqual(words("0.12 & 3.4e-5 & 17 \\\\\n1 & 2 & 3D x1 \\\\"), []);
  // (TikZ: options, coordinates, key=value, a style's name not; a node's text is)
  assert.deepEqual(words("\\begin{tikzpicture}[scale=0.8, every node/.style={draw}]\n\\draw[thick, ->] (0,0) -- (1,2) node[above] {Peak load};\n\\end{tikzpicture}"), ["Peak", "load"]);
  // (a figure: its file and width not, its caption is)
  assert.deepEqual(words("\\includegraphics[width=0.3\\linewidth]{figs/plot.pdf}\\caption{Edit time}"), ["Edit", "time"]);
  // (a .bib entry is all key=value)
  assert.deepEqual(words("@article{knuth84, author = {Donald Knuth}, title = {Literate Programming}, year = 1984}"), ["article"]);
});

test("Blame counts words as a reader reads them, a word its first letter's writer", () => {
  const m = new Model();
  m.detail = "word";
  const text = "Alpha beta 12 \\cite{key} gamma\n";
  m.add({ commits: [commit("a1", "Ann", 3), commit("b2", "Bob", 1)], files: { "main.tex": { text, runs: [[0, 11, 0], [11, text.length, 1]] } } } as BlameData);
  assert.deepEqual([...m.counts()].sort(), [[0, 2], [1, 1]]);
});
