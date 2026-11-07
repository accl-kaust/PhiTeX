// Tetris: an arcade screen. Pieces in the ribbon's colours fall into the well
// and clear full rows; the HUD beside it is the build: its stage (the step),
// a meter of packages fetched, the page being set, the next piece; "Did you
// know" scrolls along the bottom as a ticker. Pieces fall faster once the
// page is set. Page in: every block flattens into a thin line, the rows
// becoming lines of text, and they fade with the HUD onto the page.

import { html } from "../html.ts";
import { layer, loop, ending, RIBBON, PAPER, INK, type Opening } from "./kit.ts";

const COLS = 10;
const SHAPES = [
  [[0, 0], [1, 0], [2, 0], [3, 0]],
  [[0, 0], [1, 0], [2, 0], [0, 1]],
  [[0, 0], [1, 0], [2, 0], [2, 1]],
  [[0, 0], [1, 0], [0, 1], [1, 1]],
  [[1, 0], [2, 0], [0, 1], [1, 1]],
  [[0, 0], [1, 0], [1, 1], [2, 1]],
  [[0, 0], [1, 0], [2, 0], [1, 1]],
];

export function tetris(box: HTMLElement, dark: boolean): Opening {
  const fg = dark ? "#e4e6ea" : INK.light, mute = dark ? "#8d96a5" : "#6b7486";
  const el = layer(
    box,
    "phx-tet",
    `.phx-tet { background: ${dark ? PAPER.dark : PAPER.light}; color: ${fg}; font: 2.6cqw/1.3 "DM Mono", ui-monospace, monospace; text-transform: uppercase; transition: background .3s ease .2s; }
.phx-tet canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.phx-tet .hud { position: absolute; left: 64%; right: 5%; top: 6cqw; display: grid; gap: 3.4cqw; transition: opacity .25s; }
.phx-tet .hud b { display: block; font-weight: 400; font-size: 2.1cqw; color: ${mute}; letter-spacing: .08em; margin-bottom: .6cqw; }
.phx-tet .hud .v { font-size: 2.9cqw; line-height: 1.25; text-transform: none; min-height: 2.5em; }
.phx-tet .meter { display: grid; grid-template-columns: repeat(8, 1fr); gap: .5cqw; }
.phx-tet .meter i { height: 2.4cqw; border-radius: .4cqw; background: ${dark ? "#3d4047" : "#e7e9ee"}; transition: background .15s; }
.phx-tet .meter i.on { background: ${RIBBON[3]}; }
.phx-tet .meter.run i { animation: phx-tet-run 1s steps(1) infinite; }
@keyframes phx-tet-run { 50% { background: ${RIBBON[2]}; } }
.phx-tet .next { height: 9cqw; }
.phx-tet .tick { position: absolute; left: 5%; right: 5%; bottom: 5cqw; overflow: hidden; white-space: nowrap; border-top: 1px solid ${dark ? "#3d4047" : "#d6dae1"}; padding-top: 1.6cqw; text-transform: none; color: ${mute}; transition: opacity .25s; }
.phx-tet .tick span { display: inline-block; padding-left: 100%; animation: phx-tet-tick 14s linear infinite; }
@keyframes phx-tet-tick { to { transform: translateX(-100%); } }
.phx-tet.out { background: transparent; }
.phx-tet.out .hud, .phx-tet.out .tick { opacity: 0; }`,
    html`<canvas></canvas><div class="hud"><div><b>Stage</b><div class="v st">Ready</div></div><div><b>Packages</b><div class="meter">${Array.from({ length: 8 }, () => html`<i></i>`)}</div></div><div><b>Page</b><div class="v pg">–</div></div><div><b>Next</b><div class="next"></div></div></div>
     <div class="tick"><span></span></div>`,
  );
  const $ = (s: string) => el.querySelector<HTMLElement>(s)!;
  const cv = el.querySelector("canvas")!, ctx = cv.getContext("2d")!;
  const colors = [dark ? "#9fb7e0" : INK.light, ...RIBBON];
  const dpr = devicePixelRatio || 1;
  const W = (cv.width = box.clientWidth * dpr), H = (cv.height = box.clientHeight * dpr);
  // (the well: the sheet's left three fifths, above the ticker)
  const cell = (W * 0.56) / COLS, ROWS = Math.floor((H * 0.8) / cell), ox = W * 0.05, oy = W * 0.06;
  let grid: (string | null)[][] = Array.from({ length: ROWS }, () => Array(COLS).fill(null));
  type Piece = { cells: number[][]; x: number; y: number; c: string };
  const make = (): Piece => {
    const cells = SHAPES[Math.floor(Math.random() * SHAPES.length)];
    return { cells, x: 0, y: -2, c: colors[Math.floor(Math.random() * colors.length)] };
  };
  const fits = (p: Piece, x: number, y: number) => p.cells.every(([cx, cy]) => {
    const gx = x + cx, gy = y + cy;
    return gx >= 0 && gx < COLS && gy < ROWS && (gy < 0 || !grid[gy][gx]);
  });
  // (the piece slides to where it lands lowest, as a player would)
  const aim = (p: Piece) => {
    let best = 0, by = -1;
    for (let x = 0; x < COLS; x++) {
      if (!fits(p, x, 0)) continue;
      let y = 0;
      while (fits(p, x, y + 1)) y++;
      if (y > by || (y === by && Math.random() < 0.3)) (by = y), (best = x);
    }
    return best;
  };
  let piece = make(), next = make(), target = aim(piece), acc = 0, clearing: number[] = [], flash = 0, out = -1, speed = 45;
  const block = (x: number, y: number, s: number, c: string, k = 0) => {
    const g = s * 0.08, h = (s - 2 * g) * (1 - 0.8 * k);
    ctx.fillStyle = c;
    ctx.beginPath();
    ctx.roundRect(x + g, y + g + (s - 2 * g - h) / 2, s - 2 * g, h, s * 0.12 * (1 - k));
    ctx.fill();
  };
  const at = (x: number, y: number, c: string, k = 0) => block(ox + x * cell, oy + y * cell, cell, c, k);
  // (the next piece, drawn in the HUD's box)
  const drawNext = () => {
    const r = $(".next").getBoundingClientRect(), b = box.getBoundingClientRect(), s = cell * 0.7;
    next.cells.forEach(([cx, cy]) => block((r.left - b.left) * dpr + cx * s, (r.top - b.top) * dpr + cy * s, s, next.c));
  };
  const t0 = performance.now();
  const stopLoop = loop((t, dt) => {
    ctx.clearRect(0, 0, W, H);
    // (the well's floor and walls, faint)
    ctx.fillStyle = dark ? "#3d4047" : "#e7e9ee";
    if (out < 0) ctx.fillRect(ox - 2 * dpr, oy + ROWS * cell + dpr, COLS * cell + 4 * dpr, 2 * dpr);
    if (out >= 0) {
      const k = Math.min(1, (t - out) / 300), f = Math.max(0, (t - out - 250) / 300);
      ctx.globalAlpha = 1 - f;
      grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, dark ? "#c8cdd6" : "#333", k)));
      ctx.globalAlpha = 1;
      return;
    }
    drawNext();
    if (clearing.length) {
      flash += dt;
      grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, clearing.includes(y) && Math.floor(flash / 70) % 2 ? "#fff" : c)));
      if (flash > 280) {
        for (const y of clearing.sort((a, b) => a - b)) grid.splice(y, 1), grid.unshift(Array(COLS).fill(null));
        (clearing = []), (flash = 0);
      }
      return;
    }
    acc += dt;
    while (acc > speed) {
      acc -= speed;
      if (piece.x !== target && piece.y >= -1) piece.x += Math.sign(target - piece.x);
      else if (fits(piece, piece.x, piece.y + 1)) piece.y++;
      else {
        if (piece.y < 0) grid = grid.map(() => Array(COLS).fill(null));
        else piece.cells.forEach(([cx, cy]) => (grid[piece.y + cy][piece.x + cx] = piece.c));
        clearing = grid.flatMap((row, y) => (row.every(Boolean) ? [y] : []));
        (piece = next), (next = make()), (target = aim(piece));
        break;
      }
    }
    grid.forEach((row, y) => row.forEach((c, x) => c && at(x, y, c)));
    piece.cells.forEach(([cx, cy]) => piece.y + cy >= 0 && at(piece.x + cx, piece.y + cy, piece.c));
  });
  const meter = $(".meter");
  const step = (what: string, pct: number | undefined, detail: string) => {
    $(".st").textContent = what;
    const n = pct === undefined ? (what === "Typesetting" ? 8 : 0) : Math.round((pct / 100) * 8);
    meter.querySelectorAll("i").forEach((m, i) => m.classList.toggle("on", i < n));
    meter.classList.toggle("run", pct === undefined && what !== "Typesetting");
    if (/^Page \d+/.test(detail)) $(".pg").textContent = detail.slice(5);
    // (setting the page: the pieces fall faster)
    speed = what === "Typesetting" ? 28 : 45;
  };
  const hint = (text: string) => {
    const s = $(".tick span");
    s.textContent = `Did you know? ${text}`;
    s.style.animation = "none";
    void s.offsetWidth;
    s.style.animation = "";
  };
  return ending(el, () => ((out = performance.now() - t0), el.classList.add("out"), 560), stopLoop, { step, hint });
}
