// Page colours (Settings, Page colours: #37): the pages' background and text
// recoloured by a filter over what is drawn, presets and Custom, the marker's
// colour; Dark PDF wins while it is on; Colour lines follow a dark sheet;
// kept for the next tab; a rebuild touches none of it; the PDF the watch
// writes stays the real one.
//
//   PHITEX_BIN=target/release/phitex node --test viewer/browser/colours.test.mjs
//   (PHITEX_SHOTS=dir: screenshots of each step)

import { test, before, after, afterEach } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Browser, Watch, project, sleep } from "./lib.mjs";

const home = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-home-"));
process.env.PHITEX_CACHE_DIR ??= join(home, "cache");
mkdirSync(join(home, ".config", "phitex"), { recursive: true });
writeFileSync(join(home, ".config", "phitex", "phitex.toml"), 'tips-seen = ["live", "compare", "blame", "changes", "timing", "clean", "dark"]\n');
let browser;
before(async () => (browser = await Browser.start()));
after(async () => (Watch.killAll(), await browser?.stop()));
afterEach(async (t) => browser?.report(t));

const dark = (t, on) => t.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: on ? "dark" : "light" }] });
const text = "\\documentclass{article}\n\\begin{document}\n\\section{Colours}\nThe pages in the reader's own colours: the paper, the ink, and the marker where the editor's cursor is. ".repeat(1) + "A line of text to read, set as the page sets it, long enough to run over a few lines of the page. ".repeat(12) + "\n\\end{document}\n";

test("Page colours: presets and Custom recolour the pages by a filter, Dark PDF wins, the marker's colour, kept, a rebuild touches none of it", async () => {
  const dir = project({ "main.tex": text });
  const w = new Watch(dir, { home });
  const t = await browser.tab(await w.url());
  await t.until(`document.title.endsWith("· Built")`, "the build", 240000);
  const shot = async (name) => process.env.PHITEX_SHOTS && (await sleep(300), await t.shot(join(process.env.PHITEX_SHOTS, `colours-${name}.png`)));
  // (the PDF a download gets, as typeset: hashed now, before any colour)
  const pdfHash = () => t.js("fetch('doc.pdf', { cache: 'no-store' }).then((r) => r.arrayBuffer()).then((b) => crypto.subtle.digest('SHA-256', b)).then((d) => [...new Uint8Array(d)].map((x) => x.toString(16).padStart(2, '0')).join(''))");
  const pdf0 = await pdfHash();
  await t.click("#b-more");
  await t.click("#x-settings");
  await t.until(`!!$("#settings [data-section='Colours']")`, "Settings: Page colours", 5000);
  await t.click("#settings [data-section='Colours']");
  assert.deepEqual(await t.q(`$$("#settings [data-id='page-colours'] .sw").map((s) => s.dataset.v)`), ["paper", "sepia", "dark", "contrast", "custom"], "the presets, and Custom");
  assert.equal(await t.q(`$(".win").classList.contains("paged")`), false, "Paper: the pages as printed, no filter");
  await shot("settings-light");
  // (Sepia: the canvases' filter, white to the page's colour and black to the text's)
  await t.click("#settings [data-id='page-colours'] .sw[data-v='sepia']");
  const sepia = await t.q(`({ paged: $(".win").classList.contains("paged"), dark: $(".win").classList.contains("paged-dark"), filter: getComputedStyle($(".slot canvas")).filter, m: $("svg.phx-theme feColorMatrix")?.getAttribute("values") })`);
  assert.ok(sepia.paged && !sepia.dark && /url\(.*#phx-page-colours/.test(sepia.filter), `Sepia's filter on the pages: ${JSON.stringify(sepia)}`);
  // (black (0,0,0) to the text's #5b4636: each row's constant; white to the page's #f4ecd8: each row's sum)
  const rows = sepia.m.split(/\s+/).map(Number);
  const at = (r) => [rows[r * 5 + 4], rows[r * 5] + rows[r * 5 + 1] + rows[r * 5 + 2] + rows[r * 5 + 4]].map((v) => Math.round(v * 255));
  assert.deepEqual([at(0), at(1), at(2)], [[0x5b, 0xf4], [0x46, 0xec], [0x36, 0xd8]], "black to the text's colour, white to the page's");
  await t.key("Escape", "Escape", 27);
  await shot("sepia-light");
  await dark(t, true);
  await shot("sepia-dark-theme");
  await dark(t, false);
  // (Dark PDF on: its own filter, Page colours' off; off again: Sepia back)
  await sleep(300);
  await t.click("#b-more");
  await t.click("#x-settings");
  await t.until(`!$("#settings").hidden && !!$("#settings [data-section='Colours']")`, "Settings", 5000);
  await t.click("#settings [data-section='Colours']");
  await t.click("#settings [data-id='dark-pdf'] [data-v='on']");
  // (kept by the watch first, in the user's phitex.toml: then on)
  await t.until(`$(".win").classList.contains("dark-pdf") && !$(".win").classList.contains("paged") && /invert/.test(getComputedStyle($(".slot canvas")).filter)`, "Dark PDF wins: its filter, Page colours' off", 5000);
  await t.click("#settings [data-id='dark-pdf'] [data-v='off']");
  await t.until(`!$(".win").classList.contains("dark-pdf") && $(".win").classList.contains("paged")`, "Dark PDF off: Sepia again", 5000);
  // (Dark: a dark sheet, Colour lines' blend as on Dark PDF's)
  await t.click("#settings [data-section='Colours']");
  await t.click("#settings [data-id='page-colours'] .sw[data-v='dark']");
  await t.click("#settings [data-section='Colours']");
  await t.click("#settings [data-id='wasp'] .switch");
  await t.until(`!!$(".phx-wasp")`, "Colour lines drawn", 10000);
  assert.deepEqual(await t.q(`[$(".win").classList.contains("paged-dark"), getComputedStyle($(".phx-wasp")).mixBlendMode]`), [true, "darken"], "Dark: a dark sheet, Colour lines darken as on Dark PDF");
  await t.click("#settings [data-id='wasp'] .switch");
  // (Custom: its two pickers; the marker's colour)
  await t.click("#settings [data-section='Colours']");
  await t.q(`(() => { const [a, b] = $$("#settings [data-id='page-custom'] input"); a.value = "#102030"; b.value = "#e0f0a0"; b.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  await t.q(`(() => { const i = $("#settings [data-id='mark-colour'] input"); i.value = "#00c0ff"; i.dispatchEvent(new Event("input", { bubbles: true })); return 1; })()`);
  const custom = await t.q(`({ pressed: $("#settings [data-id='page-colours'] .sw[data-v='custom']").getAttribute("aria-pressed"), dark: $(".win").classList.contains("paged-dark"), mark: $(".win").style.getPropertyValue("--ph-mark-fill") })`);
  assert.deepEqual(custom, { pressed: "true", dark: true, mark: "color-mix(in srgb, #00c0ff 45%, transparent)" }, "Custom chosen by its pickers; the marker's colour");
  await shot("settings-custom-light");
  await dark(t, true);
  await shot("settings-custom-dark");
  await dark(t, false);
  await t.key("Escape", "Escape", 27);
  // (the PDF a download gets: the same bytes, every colour on the screen only)
  assert.equal(await pdfHash(), pdf0, "the PDF as typeset, not recoloured");
  // (a rebuild: nothing of it touched, no filter written again)
  await t.q(`(() => { globalThis.__themeMuts = 0; const n = () => globalThis.__themeMuts++; new MutationObserver(n).observe($("svg.phx-theme"), { subtree: true, attributes: true, childList: true }); new MutationObserver(n).observe($(".win"), { attributes: true, attributeFilter: ["class", "style"] }); return 1; })()`);
  const n = (w.out.match(/^(ok|failed|no output|✓|✗|⚠):?\s+\d+/gm) ?? []).length;
  writeFileSync(join(dir, "main.tex"), text.replace("A line of text", "A changed line of text"));
  for (const end = Date.now() + 120000; (w.out.match(/^(ok|failed|no output|✓|✗|⚠):?\s+\d+/gm) ?? []).length <= n; await sleep(50)) if (Date.now() > end) throw new Error(`no rebuild: ${w.out}`);
  await sleep(2500);
  assert.equal(await t.q(`globalThis.__themeMuts`), 0, "a rebuild writes nothing of the colours");
  // (narrow: Settings' Page colours whole at 700 px)
  await t.send("Emulation.setDeviceMetricsOverride", { width: 700, height: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(300);
  await t.click("#b-more");
  await t.click("#x-settings");
  await t.until(`!$("#settings").hidden && !!$("#settings [data-section='Colours']")`, "Settings", 5000);
  await t.click("#settings [data-section='Colours']");
  assert.ok(await t.q(`$$("#settings [data-id='page-colours'] .sw").every((s) => s.getBoundingClientRect().right <= $("#settings").getBoundingClientRect().right + 1 && s.offsetWidth > 0)`), "700 px: every preset in view");
  await shot("settings-narrow-light");
  await t.key("Escape", "Escape", 27);
  await t.send("Emulation.clearDeviceMetricsOverride", {});
  // (kept: a new tab on the watch opens in Custom, the marker's colour too)
  const t2 = await browser.tab(await w.url());
  await t2.until(`+$("#pgn")?.textContent > 0`, "the first page", 120000);
  assert.deepEqual(await t2.q(`[$(".win").classList.contains("paged"), $(".win").classList.contains("paged-dark"), $(".win").style.getPropertyValue("--ph-mark-fill")]`), [true, true, "color-mix(in srgb, #00c0ff 45%, transparent)"], "kept for the next tab");
  // (Paper again: the filter gone)
  await sleep(300);
  await t2.click("#b-more");
  await t2.click("#x-settings");
  await t2.until(`!$("#settings").hidden && !!$("#settings [data-section='Colours']")`, "Settings", 5000);
  await t2.click("#settings [data-section='Colours']");
  await t2.click("#settings [data-id='page-colours'] .sw[data-v='paper']");
  assert.deepEqual(await t2.q(`[$(".win").classList.contains("paged"), getComputedStyle($(".slot canvas")).filter]`), [false, "none"], "Paper: the pages as printed");
  assert.deepEqual(t.errors, []);
  assert.deepEqual(t2.errors, []);
  await w.stop();
});
