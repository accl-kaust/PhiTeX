// Opening a document: the logo's animation on the first page's sheet, while
// the first build runs (assets/logo-concepts.html's "Opening a document").
// Three acts on the woven ΦTeX:
//   start   the Φ draws in alone; its ribbon sweeps right, drawing T, E and
//           X out behind it, the view widening; the byline lands under it;
//   steady  while the document loads, each stripe drains through its far end
//           and flows in again, one after another;
//   end     done(): at once, from wherever the view is, it dives into the
//           white of the Φ's left counter, a hole onto the page beneath, until
//           the page is all that is left (the stripes running on to whole
//           meanwhile); 650 ms in all.
// The art is opening-art.ts's (made once from the logo page: no font is
// needed for the letters). Under prefers-reduced-motion the logo stands still.

import { WORD, WOVEN, MARK, LOCKUP, SEEDS } from "./opening-art.ts";

const NS = "http://www.w3.org/2000/svg";
const f2 = (v: number) => +v.toFixed(2);
const clamp01 = (u: number) => Math.min(1, Math.max(0, u));
const easeIO = (u: number) => ((u = clamp01(u)), u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2);
const easeOutExpo = (u: number) => (u >= 1 ? 1 : 1 - 2 ** (-10 * Math.max(0, u)));
const easeBack = (u: number, k: number) => ((u = clamp01(u) - 1), 1 + (k + 1) * u * u * u + k * u * u);
const ease2 = (u: number) => ((u = clamp01(u)), u < 0.5 ? 2 * u * u : 1 - (-2 * u + 2) ** 2 / 2);

/** A dash offset that hides a stripe whole (dash 1, gap 2: no zero-length dash left at either end). */
const OFF = 1.01;
// (the end is bounded: the dive starts the moment the page is in, whatever the act, and the sheet is gone 650 ms later)
const OPEN = { draw: 700, stagger: 110, sweep: 1000, rest: 300, cycle: 1600, settle: 500, dive: 470, fade: 180, into: [0.3, 0.42], deep: 5 };
/** The Φ's left counter (CMU Serif Bold, font units, y up): the hole the view dives into. */
const PHI_COUNTER = "M228 344Q228 316 233 294Q238 272 252.5 249Q267 226 298 210.5Q329 195 375 190V495Q288 485 258 444Q228 403 228 344Z";
const BY = { h: 18, faint: "#8d96a5", kaust: "#80715e" };
const SANS = `"Noto Sans", system-ui, sans-serif`;

type Box = { x0: number; y0: number; x1: number; y1: number };
let uid = 0;

/** The art with its ids made unique to this sheet, in `fill`. */
const art = (a: { body: string }, fill: string, tag: string) => a.body.replace(/§/g, `phx${tag}-`).replace(/#FILL/g, fill);

const svgOf = (markup: string) => {
  const t = document.createElement("template");
  t.innerHTML = markup;
  return t.content.firstElementChild as SVGSVGElement;
};

/**
 * The byline under ΦTeX ("by ACCL | KAUST"), landing as ACCL's logo does:
 * its stripes shoot in with trails, its A draws and fills, its words slide
 * out, the rule grows, KAUST's seeds pop up and KAUST types on. Laid out
 * under `word`; frame(t), t ms from its start.
 */
function byline(svg: SVGSVGElement, word: Box) {
  const h = BY.h;
  const ctx = document.createElement("canvas").getContext("2d")!;
  const sans = (px: number, wt: number) => ((ctx.font = `${wt} ${px}px ${SANS}`), (s: string) => ctx.measureText(s).width);
  const byW = sans(0.55 * h, 400)("by"), kW = sans(0.72 * h, 700);
  const letters = [..."KAUST"], track = 0.05 * 0.72 * h, adv = letters.map((c) => kW(c) + track);
  const lockup = svgOf(LOCKUP), seeds = svgOf(SEEDS);
  const vb = lockup.viewBox.baseVal, lw = (h * vb.width) / vb.height, sv = seeds.viewBox.baseVal, sh = 1.1 * h, sw = (sh * sv.width) / sv.height;
  const gap = 0.45 * h, sep = 0.9 * h;
  const width = byW + gap + lw + sep * 2 + sw + gap * 0.6 + adv.reduce((a, b) => a + b, 0);
  const x0 = (word.x0 + word.x1) / 2 - width / 2, top = word.y1 + 0.9 * h, mid = top + h / 2;
  const g = document.createElementNS(NS, "g"), wid = `phx-by${uid++}`;
  svg.append(g);
  let x = x0;
  g.insertAdjacentHTML("beforeend", `<text x="${f2(x)}" y="${f2(mid + 0.19 * h)}" fill="${BY.faint}" style="font: 400 ${f2(0.55 * h)}px ${SANS}">by</text>`);
  const byT = g.lastElementChild!;
  x += byW + gap;
  for (const [k, v] of [["x", x], ["y", top], ["width", lw], ["height", h]] as const) lockup.setAttribute(k, String(f2(v)));
  g.append(lockup);
  x += lw + sep;
  g.insertAdjacentHTML("beforeend", `<rect x="${f2(x)}" y="${f2(top)}" width="${f2(0.1 * h)}" height="${f2(h)}" fill="${BY.faint}"/>`);
  const rule = g.lastElementChild!;
  x += sep;
  for (const [k, v] of [["x", x], ["y", mid - sh / 2], ["width", sw], ["height", sh]] as const) seeds.setAttribute(k, String(f2(v)));
  g.append(seeds);
  x += sw + gap * 0.6;
  const ks = letters.map((c, i) => {
    g.insertAdjacentHTML("beforeend", `<text x="${f2(x)}" y="${f2(mid + 0.26 * h)}" fill="${BY.kaust}" style="font: 700 ${f2(0.72 * h)}px ${SANS}">${c}</text>`);
    x += adv[i];
    return g.lastElementChild!;
  });
  // (the lockup's parts: its A, its four stripes, its words)
  const paths = [...lockup.querySelectorAll("path")], A = paths.slice(0, 2), stripes = paths.slice(2, 6), words = paths.slice(6);
  const up = [77.59, -94], n = Math.hypot(up[0], up[1]), D = 150, from = [(-up[0] / n) * D, (-up[1] / n) * D];
  const trails = stripes.map((s) =>
    [0.4, 0.22, 0.1].map((op) => {
      const c = s.cloneNode() as SVGPathElement;
      c.dataset.op = String(op);
      s.before(c);
      return c;
    }),
  );
  const outline = A.map((a) => {
    const c = a.cloneNode() as SVGPathElement;
    for (const [k, v] of [["fill", "none"], ["stroke", "#1D3E72"], ["stroke-width", "1.1"], ["stroke-linejoin", "round"], ["pathLength", "1"], ["stroke-dasharray", "1 2"]]) c.setAttribute(k, v);
    a.after(c);
    return c;
  });
  const wg = document.createElementNS(NS, "g");
  words[0]?.before(wg);
  words.forEach((w) => wg.append(w));
  lockup.insertAdjacentHTML("afterbegin", `<defs><clipPath id="${wid}"><rect x="97.5" y="-50" width="1000" height="200"/></clipPath></defs>`);
  const clipped = document.createElementNS(NS, "g");
  wg.before(clipped);
  clipped.append(wg);
  clipped.setAttribute("clip-path", `url(#${wid})`);
  const at = (e: Element, dx: number, dy: number) => e.setAttribute("transform", `translate(${f2(dx)} ${f2(dy)})`);
  return {
    x0,
    x1: x0 + width,
    y1: top + h,
    frame(t: number) {
      byT.setAttribute("opacity", String(f2(clamp01(t / 300))));
      stripes.forEach((s, i) => {
        const t0 = 50 + i * 70, k = easeBack((t - t0) / 950, 1.15);
        at(s, from[0] * (1 - k), from[1] * (1 - k));
        s.setAttribute("opacity", t < t0 ? "0" : "1");
        trails[i].forEach((c, j) => {
          const tj = t0 + (j + 1) * 40, kj = easeOutExpo((t - tj) / 900);
          at(c, from[0] * (1 - kj), from[1] * (1 - kj));
          c.setAttribute("opacity", t < tj ? "0" : String(f2(Number(c.dataset.op) * (1 - clamp01((t - tj - 350) / 300)))));
        });
      });
      outline.forEach((o) => {
        o.setAttribute("stroke-dashoffset", String(f2(1 - ease2((t - 300) / 750))));
        o.setAttribute("opacity", String(f2(t < 300 ? 0 : 1 - clamp01((t - 1050) / 300))));
      });
      A.forEach((a) => a.setAttribute("opacity", String(f2(clamp01((t - 850) / 350)))));
      words.forEach((w, i) => at(w, -470 * (1 - easeOutExpo((t - 800 - i * 45) / 1100)), 0));
      rule.setAttribute("opacity", t < 900 ? "0" : "1");
      rule.setAttribute("transform", `translate(0 ${f2(mid)}) scale(1 ${f2(ease2((t - 900) / 300))}) translate(0 ${f2(-mid)})`);
      const sk = easeBack((t - 1000) / 550, 2.6), cx = Number(seeds.getAttribute("x")) + sw / 2, by = mid + sh / 2;
      seeds.setAttribute("opacity", String(f2(clamp01((t - 1000) / 200))));
      seeds.setAttribute("transform", `translate(${f2(cx)} ${f2(by + 0.5 * h * (1 - sk))}) scale(${f2(0.2 + 0.8 * sk)}) translate(${f2(-cx)} ${f2(-by)})`);
      ks.forEach((k, i) => k.setAttribute("opacity", t >= 1150 + i * 60 ? "1" : "0"));
    },
  };
}

export interface Opening {
  /** The document is in: dive into the page at once, resolve once only the page is left (the logo gone, 650 ms on). */
  done(): Promise<void>;
  /** Gone at once. */
  stop(): void;
}

/**
 * The opening, playing in `box` (the sheet: its own white, or `bg`, the
 * background the hole cuts through), the letters in `fill`.
 */
export function opening(box: HTMLElement, fill: string, bg: string): Opening {
  const tag = String(uid++);
  const w = WORD, size = w.size, phi = w.letters[0];
  box.insertAdjacentHTML(
    "beforeend",
    `<svg class="phx-open" preserveAspectRatio="none" aria-hidden="true">
    <defs><mask id="phx-hole${tag}" maskUnits="userSpaceOnUse" x="-1e5" y="-1e5" width="2e5" height="2e5"><rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="#fff"/>
      <path class="phx-hole" d="${PHI_COUNTER}" fill="#000" opacity="0" transform="translate(${f2(phi.x)} ${f2(phi.y)}) scale(${size / 1000} ${-size / 1000})"/></mask></defs>
    <rect x="-1e5" y="-1e5" width="2e5" height="2e5" fill="${bg}" mask="url(#phx-hole${tag})"/>${art(WOVEN, fill, tag)}</svg>`,
  );
  const el = box.lastElementChild as SVGSVGElement, hole = el.querySelector(".phx-hole")!;
  const by = byline(el, WOVEN);
  // (the view frames the byline too)
  const word: Box = { x0: Math.min(WOVEN.x0, by.x0), y0: WOVEN.y0, x1: Math.max(WOVEN.x1, by.x1), y1: by.y1 };
  const rest = [...el.querySelectorAll<SVGGElement>(".phx-l")]
    .filter((g) => g.dataset.ch !== "Φ")
    .map((g) => [g, w.letters.find((l) => l.ch === g.dataset.ch)!] as const);
  // (each stripe: its paths, its whole length, and the share of it the Φ alone draws, to the bowl's edge)
  const tmp = document.createElementNS(NS, "svg");
  tmp.style.cssText = "position:absolute;width:0;height:0;visibility:hidden";
  document.body.append(tmp);
  tmp.innerHTML = art(MARK, fill, `${tag}m`);
  const S = [0, 1, 2, 3].map((j) => {
    const ps = [...el.querySelectorAll<SVGPathElement>(`.phx-s${j}`)];
    const full = ps[0].getTotalLength(), part = tmp.querySelector<SVGPathElement>(`.phx-s${j}`)!.getTotalLength();
    ps.forEach((p) => (p.style.strokeDasharray = "1 2"));
    return { ps, full, f: part / full };
  });
  tmp.remove();
  type Cam = { cx: number; cy: number; s: number };
  const W = () => box.clientWidth || 1, H = () => box.clientHeight || 1;
  const fit = (r: Box, fw: number, fh: number): Cam => ({
    cx: (r.x0 + r.x1) / 2,
    cy: (r.y0 + r.y1) / 2,
    s: Math.max((r.x1 - r.x0) / (fw * W()), (r.y1 - r.y0) / (fh * H())),
  });
  const P = { x: phi.x + OPEN.into[0] * size, y: phi.y - OPEN.into[1] * size };
  const deep = () => OPEN.deep / W();
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const T1 = OPEN.draw + 3 * OPEN.stagger, T2 = T1 + 200 + OPEN.sweep + 1.5 * OPEN.stagger, T3 = T2 + OPEN.rest;
  let doneAt: number | null = null, settled: number | null = null, end = false, raf = 0;
  let finish: () => void = () => {};
  const t0 = performance.now();
  // (where stripe j's dash is at time t of the flow: 0 whole, below 0 draining, above 0 filling)
  const flow = (j: number, t: number) => {
    const u = (t - T3 - j * OPEN.stagger) / OPEN.cycle;
    if (u < 0) return 0;
    const v = u % 1;
    return v < 0.5 ? -OFF * easeIO(v * 2) : OFF * (1 - easeIO(v * 2 - 1));
  };
  /** The view at time t before any dive: the Φ, then widening to the word. */
  const cam = (t: number): Cam => {
    const cm = fit(MARK, 0.45, 0.3), cw = fit(word, 0.84, 0.4);
    const k0 = easeIO((t - T1 - 200) / (0.75 * OPEN.sweep));
    return t < T1 + 200 ? cm : { cx: cm.cx + (cw.cx - cm.cx) * k0, cy: cm.cy + (cw.cy - cm.cy) * k0, s: cm.s + (cw.s - cm.s) * k0 };
  };
  const frame = (t: number) => {
    if (still) t = doneAt === null ? T3 : Math.max(t, (settled ??= doneAt) + OPEN.dive);
    let head = phi.r;
    S.forEach((st, j) => {
      let o: number;
      if (t < T1 + 200) o = OFF - (OFF - (1 - st.f)) * easeIO((t - j * OPEN.stagger) / OPEN.draw);
      else if (t < T3) o = (1 - st.f) * (1 - easeIO((t - T1 - 200 - (j * OPEN.stagger) / 2) / OPEN.sweep));
      else if (doneAt === null || t < doneAt) o = flow(j, t);
      else {
        const o0 = flow(j, doneAt), u = easeIO((t - doneAt - (j * OPEN.stagger) / 2) / OPEN.settle);
        o = o0 <= 0 ? (u < 0.5 ? o0 + (-OFF - o0) * u * 2 : OFF * (2 - u * 2)) : o0 * (1 - u);
      }
      st.ps.forEach((p) => (p.style.strokeDashoffset = String(o)));
      if (o >= 0 && o < 1) head = Math.max(head, st.ps[0].getPointAtLength((1 - o) * st.full).x);
    });
    // (T, E and X each fade in whole as the ribbon's head passes into them)
    rest.forEach(([g, l]) => (g.style.opacity = t < T3 ? String(f2(clamp01((head - Math.max(l.l, phi.r)) / (0.3 * (l.r - l.l))))) : "1"));
    // (the camera: the Φ, then the word, then the dive about P, kept in place while the view closes in)
    let c = cam(t);
    if (doneAt !== null && t >= (settled ??= doneAt)) {
      // (from the view as it was when the page came, the Φ alone or the whole word)
      const b = cam(settled), u = Math.min(1, (t - settled) / OPEN.dive), s = b.s * Math.exp(Math.log(deep() / b.s) * u ** 2.2), k = (s - deep()) / (b.s - deep());
      c = { cx: P.x + (b.cx - P.x) * k, cy: P.y + (b.cy - P.y) * k, s };
      hole.setAttribute("opacity", String(f2(Math.min(1, u * 4))));
      el.style.opacity = String(1 - Math.max(0, (t - settled - OPEN.dive) / OPEN.fade));
      if (t >= settled + OPEN.dive + OPEN.fade) end = true;
    }
    by.frame(still ? 1e9 : t - (T2 - 300));
    el.setAttribute("viewBox", [c.cx - (W() * c.s) / 2, c.cy - (H() * c.s) / 2, W() * c.s, H() * c.s].map(f2).join(" "));
  };
  const tick = () => {
    frame(performance.now() - t0);
    if (end) return void (el.remove(), finish());
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return {
    done() {
      doneAt ??= performance.now() - t0;
      return new Promise((ok) => (finish = ok));
    },
    stop() {
      cancelAnimationFrame(raf);
      el.remove();
      finish();
    },
  };
}
