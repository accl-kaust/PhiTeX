// Captures an animation page (assets/banner.html, cli.html, everywhere.html) frame by frame, by its own clock:
// window.render(t) for each t, then a screenshot. One headless Chromium, driven over the DevTools protocol.
// Usage: node scripts/readme-capture.mjs PAGE.html THEME FPS SCALE OUTDIR
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [page, theme, fps, scale, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const port = 9300 + Math.floor(Math.random() * 600);
const chrome = spawn("chromium", ["--headless=new", "--disable-gpu", "--hide-scrollbars", `--remote-debugging-port=${port}`,
  `--user-data-dir=${mkdtempSync(join(tmpdir(), "readme-capture-"))}`, "about:blank"], { stdio: "ignore" });

const sleep = ms => new Promise(r => setTimeout(r, ms));
let ws;
for (let i = 0; i < 100 && !ws; i++) {
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    ws = list.find(t => t.type === "page")?.webSocketDebuggerUrl;
  } catch { await sleep(100); }
}
const sock = new WebSocket(ws);
await new Promise(r => sock.addEventListener("open", r, { once: true }));
let id = 0;
const waiting = new Map();
sock.addEventListener("message", e => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
});
const send = (method, params = {}) => new Promise(r => { const i = ++id; waiting.set(i, r); sock.send(JSON.stringify({ id: i, method, params })); });
const evaluate = async expr => (await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value;

await send("Page.enable");
const url = `file://${resolve(page)}?render&theme=${theme}`;
await send("Page.navigate", { url });
await sleep(300);
for (let i = 0; i < 50 && !(await evaluate("!!window.bannerReady")); i++) await sleep(100);
await evaluate("window.bannerReady");
// the stage's size and the loop's length, from the page
const { w, h, d } = await evaluate(`(() => { const s = document.getElementById('stage');
  return { w: s.offsetWidth, h: s.offsetHeight, d: document.getElementById('scrub').max * 1 }; })()`);
await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: Number(scale), mobile: false });
const n = Math.round(d * Number(fps));
for (let i = 0; i < n; i++) {
  await evaluate(`window.render(${i / Number(fps)})`);
  const shot = await send("Page.captureScreenshot", { format: "png", clip: { x: 0, y: 0, width: w, height: h, scale: 1 } });
  writeFileSync(join(out, `f${String(i).padStart(4, "0")}.png`), Buffer.from(shot.result.data, "base64"));
}
console.log(JSON.stringify({ page, theme, frames: n, w: Math.round(w * scale), h: Math.round(h * scale) }));
sock.close();
chrome.kill();
