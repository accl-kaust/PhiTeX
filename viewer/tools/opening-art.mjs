// viewer/src/opening-art.ts from assets/logo-concepts.html: the woven ΦTeX
// and its Φ as SVG (the letters are CMU Serif Bold's outlines there, so the
// viewer needs no font for them), the word's layout, and the byline's two
// logos. The page measures the letters with the font (its kerning search
// takes over a second), so the art is made here once, not on each open.
//
//   node viewer/tools/opening-art.mjs [chromium]   # with playwright-core installed
//
// Ids in the SVG are written as "§n" (the viewer makes them unique per
// sheet), the letters' colour as "#FILL", the stripes' colours as they are.
import { chromium } from "playwright-core";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../..", import.meta.url));
const b = await chromium.launch({ executablePath: process.argv[2] ?? "/usr/bin/chromium", args: ["--no-sandbox", "--disable-crashpad", "--disable-gpu"] });
const p = await b.newPage();
await p.goto(`file://${repo}assets/logo-concepts.html`);
await p.waitForFunction(() => document.fonts.status === "loaded" && document.getElementById("styles")?.childElementCount);
const art = await p.evaluate(() => {
  const ids = (r) => {
    // (every id the SVG defines, renamed §0, §1, … in order, with its references)
    const map = new Map();
    let body = r.body.replace(/id="([^"]+)"/g, (_, id) => (map.has(id) || map.set(id, `§${map.size}`), `id="${map.get(id)}"`));
    body = body.replace(/url\(#([^)]+)\)/g, (m, id) => (map.has(id) ? `url(#${map.get(id)})` : m));
    return { body, x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y1 };
  };
  const strip = (t) => document.getElementById(t).innerHTML.replace(/<metadata>[\s\S]*?<\/metadata>/g, "").replace(/<!--[\s\S]*?-->/g, "").trim();
  return {
    word: word(),
    woven: ids(render(STYLES[0], "#FILL")),
    mark: ids(render(MARK, "#FILL")),
    lockup: strip("accl-lockup"),
    seeds: strip("kaust-seeds"),
  };
});
await b.close();
// (the stripes' colours are the logo page's variables: ACCL's orange, gold, lime and teal, written out)
const COLOURS = { orange: "#f58f2d", gold: "#f0b225", lime: "#d3c72a", teal: "#00a8a4", navy: "#1d3e72" };
const literal = (t) => t.replace(/var\(--(\w+)\)/g, (m, n) => COLOURS[n] ?? m);
for (const k of ["woven", "mark"]) art[k].body = literal(art[k].body);
art.lockup = literal(art.lockup);
art.seeds = literal(art.seeds);
const out =
  `// The opening's art (opening.ts), made by viewer/tools/opening-art.mjs from\n` +
  `// assets/logo-concepts.html: do not edit. Ids are "§n", the letters' colour "#FILL".\n\n` +
  `export const WORD = ${JSON.stringify(art.word)};\n` +
  `export const WOVEN = ${JSON.stringify(art.woven)};\n` +
  `export const MARK = ${JSON.stringify(art.mark)};\n` +
  `export const LOCKUP = ${JSON.stringify(art.lockup)};\n` +
  `export const SEEDS = ${JSON.stringify(art.seeds)};\n`;
writeFileSync(`${repo}viewer/src/opening-art.ts`, out);
console.log(`viewer/src/opening-art.ts: ${out.length} bytes`);
