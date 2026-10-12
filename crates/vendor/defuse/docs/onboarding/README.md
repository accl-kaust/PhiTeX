# defuse onboarding deck

A 50-slide reveal.js deck for new developers on the engine, in the style of
the CS394A week-7 deck (same vendored reveal.js, fonts and `deck.css`; a
trimmed `deck.js`). All diagrams are inline SVG in `index.html`.

- `index.html`: the deck. Open it in a browser (no server or network needed).
  Keys: arrows to move, `T` contents, `S` speaker notes, `F` fullscreen.
  `index.html?static` shows every fragment and final animation frame.
- `assets/`: reveal.js 4 (+ notes, highlight plugins), fonts, `deck.css`,
  `deck.js`.
- `defuse-onboarding.pdf`: rendered copy (one 1920x1080 image per slide).

## Rebuild the PDF

    docs/onboarding/render.sh

Needs `chromium` and ImageMagick (`magick`); both run inside bubblewrap
(`~/.local/bin/bw`). reveal's own `?print-pdf` mode was unreliable in headless
chromium here (page size depends on timing), so the script screenshots each
slide instead.

## Sources of the numbers

Measured numbers come from commit messages (cited by hash on each slide) or
from runs made while writing the deck:

    scripts/run.sh ./target/release/examples/typing 10000 --edits 2 --no-check
    scripts/run.sh ./target/release/examples/typing 10000 --scaling --no-check --edits 0
    scripts/run.sh ./target/release/examples/typing 100000 --scaling --edits 0 --no-check

Line references marked `≈` were taken at commit 191d61f and drift as the code
changes; function names are the stable reference.

The worked-example slides (marked "Example") use numbers from a scratch
driver outside the repo (`~/code/tmp/defuse-demo`, including
`examples/common` by path) that loads small toy programs, applies edits,
prints each Report and checks the result against the interpreter.
