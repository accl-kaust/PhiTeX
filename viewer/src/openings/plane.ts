// Paper plane: the build is the folding. A sheet lies on a cutting mat and is
// folded into a dart as the build goes, drawn as an origami diagram is: the
// next fold dashed, the step numbered under it ("②  Fetching packages"), its
// detail and "Did you know" beside. Reading the project: the sheet laid down.
// Fetching packages: the folds, as far as the packages have come (the top
// corners to the middle, the edges in again, folded in half with the wings
// out). Typesetting: the last fold, and it takes off, looping a figure eight
// with a dashed trail. Page in: it swings round to the middle nose up and
// unfolds, its corners opening out to the page's, the PDF on the paper as it
// opens, its halves shaded until flat, the creases fading. A page that comes
// mid-fold unfolds from that fold.

import { html } from "../html.ts";
import { layer, ending, PAPER, INK, type Opening } from "./kit.ts";

type P = [number, number];
// (the sheet's six corners, nose up about its middle: the top's middle, top right, bottom right, the bottom's middle, bottom left, top left)
const FLAT: P[] = [[0, -19.5], [15, -19.5], [15, 19.5], [0, 19.5], [-15, 19.5], [-15, -19.5]];
/** The top corners folded to the middle line. */
const HOUSE: P[] = [[0, -19.5], [15, -4.5], [15, 19.5], [0, 19.5], [-15, 19.5], [-15, -4.5]];
/** The slanted edges folded in to the middle again. */
const NARROW: P[] = [[0, -19.5], [7, -2], [8, 19.5], [0, 19.5], [-8, 19.5], [-7, -2]];
/** Folded in half, the wings folded out: the dart, seen from above. */
const DART: P[] = [[0, -19.2], [3.1, -12], [10, 14.4], [0, 7.2], [-10, 14.4], [-3.1, -12]];
const SHAPES = [FLAT, HOUSE, NARROW, DART];
/** Each fold's line, dashed before it is made (in the shape it is made on). */
const NEXT: P[][] = [[[0, -19.5], [15, -4.5], [0, -19.5], [-15, -4.5]], [[0, -19.5], [7, -2], [0, -19.5], [-7, -2]], [[0, -19.5], [0, 19.5]]];
/** The page's corners, for the unfolding. */
const PAGE: P[] = [[42.5, 0], [85, 0], [85, 110], [42.5, 110], [0, 110], [0, 0]];
const PATH = "M42.5 40 C 70 8, 84 48, 60 62 C 40 74, 22 50, 42.5 40 C 63 30, 50 0, 26 8 C 6 15, 12 40, 42.5 40 Z";
const HOME = { x: 42.5, y: 38, r: 0, s: 1 };
const FLY = 0.42;
const STEPS = ["Reading the project", "Fetching packages", "Typesetting"];

const ease = (u: number) => ((u = Math.min(1, Math.max(0, u))), u < 0.5 ? 4 * u * u * u : 1 - (-2 * u + 2) ** 3 / 2);
const lerp = (a: number, b: number, u: number) => a + (b - a) * u;
const mix = (a: P[], b: P[], u: number): P[] => a.map((p, i) => [lerp(p[0], b[i][0], u), lerp(p[1], b[i][1], u)]);
type Pose = { x: number; y: number; r: number; s: number };
const place = (pts: P[], o: Pose): P[] => pts.map(([x, y]) => [o.x + o.s * (x * Math.cos(o.r) - y * Math.sin(o.r)), o.y + o.s * (x * Math.sin(o.r) + y * Math.cos(o.r))]);
const str = (pts: P[]) => pts.map((p) => p.map((v) => v.toFixed(2)).join(",")).join(" ");
/** The sheet at fold f (0 flat … 3 the dart). */
const folded = (f: number) => {
  const i = Math.min(2, Math.floor(f));
  return mix(SHAPES[i], SHAPES[i + 1], ease(f - i));
};

export function plane(box: HTMLElement, dark: boolean): Opening {
  const c = dark ? "#c9d4ea" : INK.light, paper = dark ? "#e9edf5" : "#fff", bg = dark ? PAPER.dark : "#eef3f8";
  const grid = dark ? "rgb(255 255 255 / 5%)" : "rgb(29 62 114 / 7%)", mute = dark ? "#a3a9b4" : "#5d6778";
  const el = layer(
    box,
    "phx-pl",
    `.phx-pl { font-family: "Noto Sans", system-ui, sans-serif; color: ${dark ? "#e4e6ea" : "#1b222c"}; }
.phx-pl svg { position: absolute; inset: 0; width: 100%; height: 100%; }
.phx-pl .cap { position: absolute; left: 10%; right: 10%; top: 68%; transition: opacity .2s; }
.phx-pl .cap .what { font-size: 3.2cqw; font-weight: 600; display: flex; align-items: center; gap: 1.6cqw; }
.phx-pl .cap .what i { flex: none; width: 4.4cqw; height: 4.4cqw; border-radius: 50%; border: .35cqw solid currentColor; display: grid; place-items: center; font: 700 2.4cqw/1 "Noto Sans", system-ui, sans-serif; font-style: normal; }
.phx-pl .cap .det { font-size: 2.4cqw; color: ${mute}; margin: .8cqw 0 0 6cqw; min-height: 1.4em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.phx-pl .cap .hint { font-size: 2.3cqw; color: ${mute}; margin: 3cqw 0 0 6cqw; line-height: 1.45; }
.phx-pl.out .cap { opacity: 0; }`,
    html`<svg viewBox="0 0 85 110" preserveAspectRatio="xMidYMid meet">
      <defs>
        <pattern id="phx-pl-grid" width="5" height="5" patternUnits="userSpaceOnUse"><path d="M5 0H0V5" fill="none" stroke="${grid}" stroke-width=".25"/></pattern>
        <mask id="phx-pl-m" maskUnits="userSpaceOnUse" x="-50" y="-50" width="185" height="210"><rect x="-50" y="-50" width="185" height="210" fill="#fff"/><polygon class="hole" points="" fill="#000"/></mask>
      </defs>
      <g mask="url(#phx-pl-m)"><rect x="-50" y="-50" width="185" height="210" fill="${bg}"/><rect class="mat" x="-50" y="-50" width="185" height="210" fill="url(#phx-pl-grid)"/></g>
      <path class="path" d="${PATH}" fill="none" stroke="none"/>
      <path class="trail" fill="none" stroke="${c}" stroke-width=".35" stroke-dasharray="1.2 1.6" opacity=".4"/>
      <polygon class="shadow" fill="rgb(0 0 0 / 10%)"/>
      <polygon class="sheet" fill="${paper}" stroke="${c}" stroke-width=".45" stroke-linejoin="round"/>
      <polygon class="sh-l" fill="#000" opacity="0"/>
      <path class="crease" fill="none" stroke="${c}" stroke-width=".3"/>
      <path class="next" fill="none" stroke="${c}" stroke-width=".35" stroke-dasharray="1.4 1"/>
    </svg>
    <div class="cap"><div class="what"><i>1</i><span>Starting</span></div><div class="det"></div><div class="hint"></div></div>`,
  );
  const $ = <T extends Element>(s: string) => el.querySelector(s) as T;
  const sheet = $(".sheet"), shadow = $(".shadow"), shL = $(".sh-l"), crease = $(".crease"), next = $(".next"), hole = $(".hole"), trail = $(".trail");
  const path = $<SVGPathElement>(".path"), LEN = path.getTotalLength();
  // (the fold it is at, and the one it goes to; when it took off)
  let f = 0, want = 0, flying = -1, out = -1, raf = 0, last = performance.now();
  let from: { pose: Pose; shape: P[] } | null = null;
  const trailPts: P[] = [];
  const poseOnPath = (t: number): Pose => {
    const d = ((t / 3600) * LEN) % LEN, a = path.getPointAtLength(d), b = path.getPointAtLength((d + 0.5) % LEN);
    return { x: a.x, y: a.y, r: Math.atan2(b.y - a.y, b.x - a.x) + Math.PI / 2, s: FLY };
  };
  const draw = (pts: P[], pose: Pose, lift: number) => {
    const w = place(pts, pose);
    sheet.setAttribute("points", str(w));
    shadow.setAttribute("points", str(place(pts, { ...pose, x: pose.x + 1.2 + 3 * lift, y: pose.y + 1.6 + 5 * lift })));
    return w;
  };
  const frame = (now: number) => {
    const dt = Math.min(50, now - last);
    last = now;
    if (out >= 0 && from) {
      // (page in: to the middle nose up, then open out to the page)
      const t = now - out, SWING = 260, a = ease(t / SWING);
      let dr = HOME.r - from.pose.r;
      dr = (((dr + Math.PI) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
      const pose = { x: lerp(from.pose.x, 42.5, a), y: lerp(from.pose.y, 52, a), r: from.pose.r + dr * a, s: lerp(from.pose.s, 1, a) };
      const u = Math.max(0, (t - SWING) / 480);
      const flown = place(from.shape, pose);
      const p = flown.map(([fx, fy], i): P => {
        const k = ease((u - (i === 1 || i === 5 ? 0 : i === 0 ? 0.05 : 0.18)) / 0.82);
        return [lerp(fx, PAGE[i][0], k), lerp(fy, PAGE[i][1], k)];
      });
      const flat = ease(u);
      hole.setAttribute("points", str(p));
      sheet.setAttribute("points", str(p));
      sheet.setAttribute("fill", "none");
      sheet.setAttribute("opacity", String(1 - flat));
      shadow.setAttribute("opacity", "0");
      trail.setAttribute("opacity", String(0.4 * (1 - a)));
      shL.setAttribute("points", str([p[0], p[3], p[4], p[5]]));
      shL.setAttribute("opacity", String(0.14 * (1 - flat)));
      crease.setAttribute("d", `M${p[0]} L${p[3]} M${p[0]} L${p[2]} M${p[0]} L${p[4]}`);
      crease.setAttribute("opacity", String(0.6 * (1 - flat)));
      next.setAttribute("d", "");
      if (u < 1) raf = requestAnimationFrame(frame);
      return;
    }
    // (folding toward the fold the build has reached, a fold at most each 400 ms)
    f += Math.sign(want - f) * Math.min(Math.abs(want - f), dt / 400);
    const shape = folded(f);
    let pose: Pose = HOME, lift = 0;
    if (f >= 3 && want >= 3) {
      if (flying < 0) flying = now;
      // (take off: from the mat onto the eight, smaller, over 700 ms)
      const k = ease((now - flying) / 700), on = poseOnPath(now - flying);
      let dr = on.r - HOME.r;
      dr = (((dr + Math.PI) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
      pose = { x: lerp(HOME.x, on.x, k), y: lerp(HOME.y, on.y, k), r: HOME.r + dr * k, s: lerp(HOME.s, FLY, k) };
      lift = k;
      trailPts.push([pose.x, pose.y]);
      if (trailPts.length > 90) trailPts.shift();
      trail.setAttribute("d", trailPts.length > 1 ? `M${trailPts.map((q) => q.join(" ")).join(" L")}` : "");
    }
    const w = draw(shape, pose, lift);
    (from = { pose, shape });
    // (the creases made so far, and the next fold dashed)
    // (the middle crease; each fold's own lies along the new edge, so it is the outline)
    crease.setAttribute("d", `M${w[0]} L${w[3]}`);
    crease.setAttribute("opacity", ".55");
    shL.setAttribute("points", f > 2 ? str([w[0], w[3], w[4], w[5]]) : "");
    shL.setAttribute("opacity", String(0.12 * Math.max(0, f - 2)));
    const i = Math.floor(f + 1e-6);
    next.setAttribute("d", i < 3 && Math.abs(f - i) < 0.02 ? NEXT[i].reduce((d, q, j) => d + `${j % 2 ? "L" : "M"}${place([q], pose)[0]} `, "") : "");
    raf = requestAnimationFrame(frame);
  };
  raf = requestAnimationFrame(frame);
  const step = (what: string, pct: number | undefined, detail: string) => {
    const k = Math.max(0, STEPS.indexOf(what));
    $(".what i").textContent = String(k + 1);
    $(".what span").textContent = what;
    $(".det").textContent = detail;
    // (the folds: none while the project is read, then as far as the packages have come, the last one for typesetting)
    want = k === 0 ? 0 : k === 1 ? 2.6 * ((pct ?? 0) / 100) : 3;
  };
  const hint = (text: string) => ($(".hint").textContent = `Did you know? ${text}`);
  return ending(
    el,
    () => {
      out = performance.now();
      el.classList.add("out");
      return 800;
    },
    () => cancelAnimationFrame(raf),
    { step, hint },
  );
}
