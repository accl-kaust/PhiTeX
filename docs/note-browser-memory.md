# The core's memory in the browser (2026-11-04)

The browser builds of PhiTeX (the Overleaf extension, the web demo, VS
Code's built-in engine) run the core as wasm32 on one shared
`WebAssembly.Memory`, whose threads are the host's workers. Its size is
fixed when the core starts and never grows (below). This note records
the limit that size runs into, what documents actually use, why the
memory cannot grow, and what was decided.

## The limit

Chrome's Linux sandbox lowers each renderer's `RLIMIT_DATA` according to
the machine's physical memory (Chromium `sandbox/policy/linux/sandbox_linux.cc`,
`GetProcessDataSizeLimit`): 8 GiB at 16 GB or less, 16 GiB up to 32 GB,
32 GiB above. With `--no-sandbox` nothing is limited, which is why the
release gate's first probe passed. A wasm memory's initial size counts in
full against that limit when it is made, whether it is touched or not.
All of the extension's cores live in one renderer, its offscreen
document, which holds about 1.0 GB before any core. Each Overleaf tab
showing PhiTeX gets a core of its own.

Measured with Chrome for Testing 156.0.8078.47 (the GitHub runner's),
sandboxed, made to see 15983 MB with an `LD_PRELOAD` shim of
`sysconf`/`sysinfo`: the renderers' limit is 8 GiB (`/proc/PID/limits`),
and the offscreen process sits at 5.1 GB with one 4 GiB core. A
reload's core started while the old page's still held its 4 GiB, and was
refused (the release gate's red; fixed in the frontends: one core per
tab and frame, the old one ended before the new one starts). A second tab
does not fit: 1 + 4 + 4 GiB > 8. It says so in its panel.

## What documents use

The VS Code engine (`vscode/dist/smoke.mjs`, the same core), `-j4`.
"Written" counts the wasm pages with any byte set after the first build
and the edits, so a page freed again still counts: it is the peak. The
"high-water" is the highest written address below the allocator's top
chunk, the size the memory must have.

| document | pages | written | high-water |
|---|---|---|---|
| circuitikz scenario (test/scenarios) | 1 | 366 MiB | 521 MiB |
| one page with figures (longdoc) | 1 | 415 MiB | 575 MiB |
| amsldoc | 44 | 684 MiB | 856 MiB |
| a 60-frame Madrid beamer deck | 60 | 747 MiB | 915 MiB |
| Grail (acmart, minted) | 18 | 1550 MiB | 1853 MiB |
| beamer user guide (beameruserguide.tex) | 227 | the cold build completes (652 s at -j4, 227 pages); the first edit after it fails an allocation (496 144 bytes) and the core traps | over 4 GiB |
| pgf manual | ~1300 | not measured: it stopped at a fatal error after 2.8 s in this setup | |

A book-sized document overflows the 4 GiB a wasm32 core can have, at
-j4, by its first edit, though the native engine builds the beamer guide in about
1 GB resident. Why the wasm core needs more than four times that is the
question to answer before any split of the memory between tabs.

## Why the memory does not grow

V8 (checked on 14.6, Node 26) has a bug with shared memories. Thread A
grows the memory by a page and tells thread B with a seq-cst atomic
store. B, which has stayed inside wasm, then does `memory.fill` into the
new page and traps: "memory access out of bounds". B's cached memory
size is stale. The repro is in the frontends tree, `test/v8-grow-race/`:

- B busy in wasm (`FN=spin node race2.mjs`): traps at the 3rd page.
- B parked in `memory.atomic.wait32` inside wasm between pages, as a
  quiet point's thread is (`FN=park`): traps at page 126 of 8000.
- B returning to JS between pages (`node race-js.mjs`): 12 000
  operations, no trap.

So growing is safe only once every thread has left wasm since the grow,
and the core's threads park inside wasm. Growing at quiet points
(option C) would need every thread to park by returning to JS. Filed
upstream (number to add).

The allocator adds a constraint. wasi-libc's dlmalloc takes its first
heap from `__heap_base` to `__heap_end`, which the linker fixes at the
initial memory (`--initial-memory`). Past that it calls `sbrk`, that is
`memory.grow`. A memory made larger than the module's initial size is
therefore not used beyond `__heap_end` unless the allocator grows into
it.

## Options

- A. 4 GiB fixed per tab (before this note). One tab on a 16 GB Linux
  machine, 3 at 32 GB, 7 above.
- B. A size chosen once at the core's start from what the process can
  still give: 4, then 3, then 2 GiB. Never grown under threads. A
  document that outgrows it stops, saying the size it got and what to
  close. At 16 GB the first tab gets 4 GiB and the second 2 GiB.
- C. Start small and grow at quiet points: not safe (above).
- D. One core for every tab, documents keyed by project: 1 + 4 GiB for
  any number of tabs. The core then holds several documents at once.

Decided (the user, 2026-11-04): B now; D designed after v1.0; C no.

For B, the size must reach the allocator, and every growth must happen
on thread 0 before the first thread is spawned. One way: link with
`--initial-memory` at 2 GiB and `--max-memory` at 4 GiB. Before any
thread starts, the host asks the core (thread 0) to take and free the
rest, trying 4, then 3, then 2 GiB. Each try is a `memory.grow` inside
dlmalloc with no other thread in wasm. Open: nothing may grow the memory
after that. Either the memory's maximum is the size chosen, which means
one memory per try, and a refused try's 2 GiB is freed only when it is
collected; or dlmalloc's footprint limit is set to it, if wasi-libc's
build exposes it.
