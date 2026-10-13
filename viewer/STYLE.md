# The viewer's style

One look in every host (`phitex watch`, Overleaf, VS Code): a proof
sheet's, not an app's. PhiTeX is a typesetting tool; its viewer takes its
look from print: paper, ink, the proofreader's red, rules and crop marks.
The sheet is `src/print.ts`; it comes last and is the only place colours
are chosen.

## Tokens

| Token | Light | Dark | For |
|---|---|---|---|
| `--paper` | `#f1f1ef` | `#121212` | the window, bars |
| `--paper-2` | `#e6e6e3` | `#1c1c1c` | the stage under the pages, hover |
| `--sheet` | `#ffffff` | `#ffffff` | a page, the loading proof |
| `--ink` | `#141414` | `#e8e8e4` | text, the ok mark |
| `--ink-2` | `#4a4a48` | `#a9a9a4` | secondary text |
| `--ink-3` | `#8a8a86` | `#6e6e6a` | captions, crop marks |
| `--rule` | `#c9c9c5` | `#3a3a38` | hairlines |
| `--mark` | `#d71920` | `#ff4a3d` | the proofreader's marks: errors, the step running, focus |
| `--caution` | `#8a5a00` | `#d9a441` | warnings |

Neutral greys, never warm cream or blue-grey. `--mark` is the only accent,
and it marks; it never decorates.

## Type

- Chrome: IBM Plex Sans (then Helvetica), 11–12 px. Labels in small caps
  or spaced capitals at 10–11 px.
- Numbers, times, paths, the console: IBM Plex Mono, tabular figures, so
  nothing jitters as it counts.
- One serif: Latin Modern (then CMU, Georgia) for a document's title or
  the loading proof's heading.

## Shapes and marks

- Square corners (0, at most 2 px); hairline rules (1 px) instead of
  shadows; overlays are flat sheets with a 1 px ink rule.
- Pages sit on `--paper-2` with crop marks at their corners, no shadow.
- Errors: a red rule, a straight red underline under the culprit, the
  severity in spaced capitals. No pills, no icons in circles.
- Progress: a 1 px rule; the carriage runs while the length is unknown.
- Status: a 6 px square, ink when ok, red on an error, blinking (steps,
  not fading) while busy.

## Motion

Only where it means something: the progress rule, a number counting. Linear, 150 ms to 1.6 s; no springs, bounces, glows,
scale or blur. `prefers-reduced-motion` stops all of it.

## Words

Terse and factual, as the terminal says it: "Undefined control
sequence, line 22", "Building · 4 s". No "Oops", no exclamation marks.

## Banned

Gradients (other than the 1 px lines that draw crop marks), `box-shadow`,
`backdrop-filter`, `border-radius` over 2 px, pills, emoji (no ⚡, ✨),
violet or indigo, glows, glassmorphism, cards inside cards, a centred
icon over a heading over a muted sentence.
