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

**Documents over about 300 pages do not build in the browser today.**
A dense manual (the beamer user guide) needs about 8 MiB of the core's
memory a page at 4 threads on top of about 1.1 GiB, so it fits to
about 350 pages; plain text needs about 1.5 MiB a page and fits to
about 1,800. The core has 4 GiB (below) and no more.

Measured on dev 3b9fe3bfa (2026-11-04), the VS Code engine
(`vscode/test/smoke.ts`, the same core) with a core built with
`--features memstats` (frontends `core/src/mem.rs`: the allocations
counted, the bytes live, their peak, and the highest end of any block,
which on the fixed memory is the size the memory must have), a cold
build and six edits. "Written" counts the wasm pages with any byte set
at the end, so a page freed again still counts.

| document | pages | -j | cold build | live peak | high-water | written |
|---|---|---|---|---|---|---|
| amsldoc | 44 | 4 | 202 s (its format made in the run) | 1092 MiB | 1280 MiB | 1142 MiB |
| beamer user guide | 241 | 1 | 115 s | 2250 MiB | 2415 MiB | 2243 MiB |
| beamer user guide | 241 | 4 | 428 s | 2794 MiB | 3114 MiB | 2758 MiB |
| 200 sections of plain text with equations | 462 | 4 | 101 s | 1596 MiB | 1875 MiB | 1753 MiB |

The same documents natively (`phitex watch --json`, jemalloc, the
process's resident set, accl 24826 and 24832):

| document | -j | cold build | resident after it | peak over the edits | `phitex build`'s peak |
|---|---|---|---|---|---|
| amsldoc | 1 / 4 | 3 / 4 s | 704 / 962 MiB | 739 / 1005 MiB | |
| beamer user guide | 1 | 31-44 s | 2313-2436 MiB | 2466-2601 MiB | 2350 MiB |
| beamer user guide | 4 | 115-119 s | 3602-3618 MiB | 3737-3763 MiB | 4141 MiB |

So the wasm core needs no more than the native engine: less (its
pointers are 4 bytes). The figure of "about 1 GB native" this note had
was not this document's: the native engine holds 2.3 GB building the
beamer user guide at -j1 and 3.6-4.1 GB at -j4. What the footprint is:

- Not the allocator. dlmalloc's high-water is 1.07 to 1.17 times the
  live peak in every run above: about 10% lost to fragmentation. That it
  never returns memory does not matter on a memory that never shrinks
  or grows. It has no per-thread arenas (one heap, one lock), and the
  native build's jemalloc, which has them, needs more, not less.
- Not the files. The texmf files and packs the host gives the core
  were 23 MiB for the beamer user guide.
- The engine's state kept per page. Live memory grows with the pages
  typeset: 5.4 MiB a page at -j1 (1.1 GiB at the start of the text, 2.25
  GiB at 241 pages) and 8.3 at -j4, for the beamer user guide; 1.4 MiB
  a page for plain text. Natively the engine's tables after the beamer
  user guide's cold build at -j1 are 8737 nodes and 1 555 095 values
  for 2.3 GB resident.
- -j4 does more and keeps it. At -j4 the beamer user guide's cold build
  makes 5.4 times the allocations of -j1 in wasm (782 M against 146 M)
  and takes 3.6 times as long (428 s against 115 s); natively it runs
  226 141 runs against 16 440 (its second pass 191 900 against 3 955), its
  tables 1 931 413 values (24% more) and 3.6 GB resident (56% more),
  and 118 s against 31 s. Nothing is collected after the cold build
  ("0 us collecting, 0 nodes freed").
- The edit's second copy: the first edit adds about 300 MiB over the
  cold build's live memory, at -j1 and at -j4 alike.

An engine design item, not the wasm build's: what a page keeps for its
edits (the values and checkpoints behind 5 MiB a page), and the cold
build at -j4 making 14 times the runs of -j1 (its second pass 49
times). ux3's
trap at the beamer user guide's first edit (on 5a0b41465) does not
recur on 3b9fe3bfa: 3.1 GiB of the 4 at -j4.

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
