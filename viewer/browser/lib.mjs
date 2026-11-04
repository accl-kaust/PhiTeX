// A real browser against a real `phitex watch`, for what a user clicks: one
// headless Chromium over the DevTools protocol, no packages (node's own
// WebSocket and fetch, as scripts/readme-capture.mjs). Clicks are the browser's
// (Input.dispatchMouseEvent at the element's centre), so a control under an
// overlay, hidden, or off screen fails as it does for a person.
//
//   PHITEX_BIN=path/to/phitex node --test viewer/browser/*.test.mjs
//
// PHITEX_VIEWER_JS=path serves that bundle instead of the binary's own (only
// to iterate on the viewer without a rebuild; the gate never sets it).
// CHROMIUM names the browser (default: chromium, else google-chrome).
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function project(files) {
  const dir = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-browser-"));
  for (const [f, t] of Object.entries(files)) {
    mkdirSync(join(dir, f, ".."), { recursive: true });
    writeFileSync(join(dir, f), t);
  }
  return dir;
}

export function git(dir, ...args) {
  const r = spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@t", ...args], { cwd: dir, encoding: "utf8" });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`);
  return r.stdout;
}

/** A `phitex watch` in `dir`; `home` (config and cache) kept apart from the user's. */
export class Watch {
  constructor(dir, { file = "main.tex", args = [], home, pipe = true, env = {} } = {}) {
    this.dir = dir;
    this.out = "";
    this.home = home;
    const bin = process.env.PHITEX_BIN ?? "phitex";
    this.proc = spawn(bin, ["watch", ...args, ...(file ? [file] : [])], {
      cwd: dir,
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, ...(home ? { HOME: home, XDG_CONFIG_HOME: join(home, ".config") } : {}), BROWSER: "true", ...env },
    });
    for (const s of [this.proc.stdout, this.proc.stderr]) s.on("data", (d) => (this.out += d));
    this.exited = new Promise((ok) => this.proc.on("exit", (c) => ok(c)));
    Watch.all.add(this);
    this.exited.then(() => Watch.all.delete(this));
  }
  /** Every watch still running (a failed test's): killed by `killAll`. */
  static all = new Set();
  static killAll() {
    for (const w of Watch.all) w.proc.kill("SIGKILL");
  }
  async url(ms = 240000) {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      const m = /http:\/\/127\.0\.0\.1:\d+\/[0-9a-f]+\//.exec(this.out);
      if (m) return m[0];
      await sleep(50);
    }
    throw new Error(`no viewer address in: ${this.out}`);
  }
  key(k) {
    this.proc.stdin.write(`${k}\n`);
  }
  async stop() {
    this.proc.stdin.end();
    const c = await Promise.race([this.exited, sleep(15000).then(() => "late")]);
    if (c === "late") this.proc.kill("SIGKILL");
    return c;
  }
  write(f, t) {
    writeFileSync(join(this.dir, f), t);
  }
  read(f) {
    return readFileSync(join(this.dir, f), "utf8");
  }
}

function chromiumName() {
  if (process.env.CHROMIUM) return process.env.CHROMIUM;
  return spawnSync("sh", ["-c", "command -v chromium"]).status === 0 ? "chromium" : "google-chrome";
}

/** A headless browser with one tab per `tab()`. */
export class Browser {
  static async start() {
    const b = new Browser();
    b.data = mkdtempSync(join(process.env.PHITEX_BROWSER_TMP ?? tmpdir(), "phitex-chrome-"));
    b.proc = spawn(chromiumName(), ["--headless=new", "--no-sandbox", "--disable-gpu", "--no-first-run", "--window-size=1200,900", "--remote-debugging-port=0", `--user-data-dir=${b.data}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
    let err = "";
    b.proc.stderr.on("data", (d) => (err += d));
    for (let i = 0; i < 300 && !b.port; i++) {
      const m = /DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)\//.exec(err);
      if (m) b.port = m[1];
      else await sleep(50);
    }
    if (!b.port) throw new Error(`no browser: ${err}`);
    return b;
  }
  async tabs() {
    return (await (await fetch(`http://127.0.0.1:${this.port}/json/list`)).json()).filter((t) => t.type === "page");
  }
  async tab(url = "about:blank") {
    const t = await (await fetch(`http://127.0.0.1:${this.port}/json/new?about:blank`, { method: "PUT" })).json();
    return Tab.attach(t, url);
  }
  async stop() {
    this.proc.kill("SIGKILL");
    await sleep(100);
    rmSync(this.data, { recursive: true, force: true, maxRetries: 3 });
  }
}

export class Tab {
  static async attach(t, url) {
    const tab = new Tab();
    tab.id = t.id;
    tab.errors = [];
    tab.sock = new WebSocket(t.webSocketDebuggerUrl);
    tab.waiting = new Map();
    tab.next = 1;
    tab.dialogs = [];
    tab.dialogAnswer = true;
    await new Promise((r) => tab.sock.addEventListener("open", r, { once: true }));
    tab.sock.addEventListener("message", (e) => tab.onMessage(JSON.parse(e.data)));
    await tab.send("Page.enable");
    await tab.send("Runtime.enable");
    const js = process.env.PHITEX_VIEWER_JS;
    if (js) await tab.send("Fetch.enable", { patterns: [{ urlPattern: "*viewer.js" }] });
    if (url !== "about:blank") await tab.go(url);
    return tab;
  }
  onMessage(m) {
    if (m.id && this.waiting.has(m.id)) {
      this.waiting.get(m.id)(m);
      this.waiting.delete(m.id);
      return;
    }
    if (m.method === "Runtime.exceptionThrown") this.errors.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
    if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") this.errors.push(m.params.args.map((a) => a.value ?? a.description).join(" "));
    if (m.method === "Page.javascriptDialogOpening") {
      this.dialogs.push(m.params);
      void this.send("Page.handleJavaScriptDialog", { accept: this.dialogAnswer });
    }
    if (m.method === "Fetch.requestPaused") {
      const body = readFileSync(process.env.PHITEX_VIEWER_JS).toString("base64");
      void this.send("Fetch.fulfillRequest", { requestId: m.params.requestId, responseCode: 200, responseHeaders: [{ name: "Content-Type", value: "text/javascript; charset=utf-8" }], body });
    }
  }
  send(method, params = {}) {
    return new Promise((r, no) => {
      const id = this.next++;
      this.waiting.set(id, (m) => (m.error ? no(new Error(`${method}: ${m.error.message}`)) : r(m.result)));
      this.sock.send(JSON.stringify({ id, method, params }));
    });
  }
  async go(url) {
    await this.send("Page.navigate", { url });
  }
  async js(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(`${expr}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  }
  /** An expression over `$`, an element by selector, in the panel's shadow root or the page. */
  q(body) {
    return this.js(`(() => { const R = document.querySelector("phitex-preview")?.shadowRoot; const $ = (s) => R?.querySelector(s) ?? document.querySelector(s); const $$ = (s) => [...(R?.querySelectorAll(s) ?? []), ...document.querySelectorAll(s)]; return (${body}); })()`);
  }
  async until(body, what = body, ms = 60000) {
    const end = Date.now() + ms;
    for (;;) {
      // (the page still loading, its panel not there yet: asked again)
      let err;
      const v = await this.q(body).catch((e) => void (err = e));
      if (err && Date.now() > end) throw err;
      if (v) return v;
      if (Date.now() > end) {
        const s = await this.state().catch(() => "?");
        throw new Error(`waited ${ms} ms for ${what}; the page says: ${JSON.stringify(s)}`);
      }
      await sleep(50);
    }
  }
  /** What the bar says, for failures and logs. */
  state() {
    return this.q(`({ st: $("#st")?.innerText, title: document.title, bar: $("#phx-bar")?.innerText, pages: $("#pgn")?.textContent, probs: $("#b-probs")?.hidden === false ? $("#b-probs").innerText : null, diff: $("#diffbar")?.innerText })`);
  }
  /** The element's centre, in view; fails if another element is on top of it (a person could not click it). */
  async point(sel) {
    const r = await this.q(`(() => { const e = $(${JSON.stringify(sel)}); if (!e) return { no: "none" }; e.scrollIntoView({ block: "nearest", inline: "nearest" }); const b = e.getBoundingClientRect(); if (!b.width || !b.height) return { no: "not shown" }; const x = b.left + b.width / 2, y = b.top + b.height / 2; let t = document.elementFromPoint(x, y); while (t?.shadowRoot) { const i = t.shadowRoot.elementFromPoint(x, y); if (!i || i === t) break; t = i; } return { x, y, on: e === t || e.contains(t) || t?.contains(e) && false, top: t ? (t.id || t.className || t.tagName) : "" }; })()`);
    if (r.no) throw new Error(`click ${sel}: ${r.no}`);
    if (!r.on) throw new Error(`click ${sel}: covered by ${r.top}`);
    return r;
  }
  async click(sel) {
    const { x, y } = await this.point(sel);
    await this.mouse(x, y);
  }
  async mouse(x, y, clickCount = 1) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount });
  }
  /** A press at (x0, y0), moved in steps with the button down, let go at (x1, y1): a drag that selects. */
  async drag(x0, y0, x1, y1, steps = 12) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x0, y: y0 });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x: x0, y: y0, button: "left", clickCount: 1 });
    for (let i = 1; i <= steps; i++) {
      await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x0 + ((x1 - x0) * i) / steps, y: y0 + ((y1 - y0) * i) / steps, button: "left", buttons: 1 });
      await sleep(16);
    }
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x1, y: y1, button: "left", clickCount: 1 });
  }
  async key(key, code = key, vk = 0) {
    const base = { key, code, windowsVirtualKeyCode: vk };
    await this.send("Input.dispatchKeyEvent", { type: "keyDown", ...base, ...(key.length === 1 ? { text: key } : {}) });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  }
  async shot(file) {
    const r = await this.send("Page.captureScreenshot", { format: "png" });
    writeFileSync(file, Buffer.from(r.data, "base64"));
  }
  close() {
    this.sock.close();
  }
}
