# TODO

The work left on DESIGN 4.3 ("partex-PhiTeX: windows"), as of 2026-10-02,
when the eight agents were stopped. Each item says where its unfinished
code is. Read DESIGN 4.3 and LOG.md's 2026-10-02 entries first.

## Where main is

Merged and verified (scripts/ssa-edits, every stage byte-identical to
plain partex, with `--fixpoint` and with `PARTEX_SSA_TRIPS=1`):

- **gate:** `scripts/ssa-edits` (10 cases, 61 stages); `cargo xtask
  ssa-edits`; `xtask check` runs it in both trip modes;
  `bench/ssa-course.sh` (course edits in one SSA process, JSON in
  `bench/results/`).
- **rebuild-cost:**
  - 1ffe4d4: a record's items shrink when its frame closes (the course
    cold build had hit 40 GB of address space at 13 GB resident);
  - 4982b89: a re-run step's index entries are replaced in place;
  - 4b2dda7: `PARTEX_SSA_REBUILD_TRACE` takes a list of rebuilds;
  - 67154b1: a definition only one of two runs made changes nothing if
    it equals the reaching one.
- **records:**
  - 3b410c5: `unsave` is not a call;
  - 813ef42: records only for steps and the typesetting calls
    (`PARTEX_SSA_LEAN=0` restores the old behaviour).
- **aux-loop:**
  - 71f3fba: `\pdffilesize`, `\pdfmdfivesum`, `\pdffiledump`,
    `\pdfobj file` and images read through the store, not the disk;
  - bd07f32: a build is trips until every `.aux` load read what that
    trip stored; BibTeX and makeindex run between trips;
    `PARTEX_SSA_TRIPS=1` keeps one pass.
- **link:** 880be04, the incremental link (`effects::Splice`: an offset
  tree per file; files written from the first changed byte).
- **front:** `phitex-syntax` reads verbatim constructs; `phitex-doc` (the
  static layer) and `partex outline`.
- **view:** `phitex-ir` is `no_std`; `PARTEX_SSA_VIEW=FILE` prints the
  build as a program, one `window(imports; exports)` per step.

Offload: `scripts/accl/accl run edits|course ...|cmd ...|gate` (see
AGENTS.md). Local wall-clock times were taken under heavy load and are
not measurements; the accl numbers below are.

## Baseline numbers

The course on accl (acclnode01, quiet), before the merges above (main
cb30e2e, job 6311):

| | time | steps | commands |
|---|---|---|---|
| cold SSA build | 120.0 s | | 66.06 M (1.68 M records, ~18 GB) |
| space | 51 ms | 2 | 1,903 |
| word | 116 ms (revert 76) | 6 | 4,450 |
| label | 9.7 s | 244 | 3.82 M |
| footnote | 585 s | 851 | 23.6 M |

After link (job 6313, word edit):
- the link's own work went from 6.1–6.8 ms to 3.4 ms on the first
  rebuild and 0.2 ms on later ones;
- writing the files went from 5–9 ms to 0.6–0.8 ms;
- the rebuild itself is still about 64 ms warm.

records item 1 on a 3-chapter copy: instructions, SSA against plain,
went from 3.36× to 2.86×; peak memory from 3.06 GB to 1.57 GB; records
from 224 K to 66 K. Item 2 is not measured yet.

After all the merges (main cb6cb6e, job 6317, acclnode01; outputs equal
to the cold build's):

| | before (6311) | after (6317) |
|---|---|---|
| cold SSA build | 120.0 s, ~18 GB | 91.1 s, 2.28 GB peak |
| records | 1.68 M | 123 K |
| word, median of 3 warm | 116 ms | 49 ms (rebuild 48.5, link 0.2) |
| label | 9.7 s, 244 steps | 6.9 s, 250 steps, 3.84 M commands |

## Left, in priority order

1. **windows** (DESIGN 4.3 item 1). This is the biggest win: the label
   and footnote cases.
   - The branch `np/windows` is at 40b33be (work in progress). Uncommitted
     work in `~/code/tmp/np-windows`: `ssa/rebuild.rs`, the CLI's window
     switch and the sanitizer.
   - Its harness ran all 11 cases identical, including a new `windows`
     case, with the default window of 4,096 commands and with
     `PARTEX_SSA_WINDOW=0`.
   - Left:
     - merge it with main; it conflicts in `ssa/rebuild.rs` with
       aux-loop, link and view;
     - find what a dropped run leaks: a mispredicted start for a new
       step made a dropped run lose `.aux` lines;
     - run `PARTEX_SSA_RERUN_CHECK=1` on every case;
     - clippy;
     - course numbers on accl for word, label and footnote against the
       table above;
     - the LOG entry.
2. **The label chain** (rebuild-cost; work in progress, uncommitted, in
   `~/code/tmp/np-rebuild-cost`: `ssa.rs`, `ssa/rebuild.rs`,
   `partex-ssa/src/{fold,open}.rs`).
   - The cause is `\@savsf` (`\count146`, eqtb 29537). `\label`'s
     `\@bsphack` changes it. Every later output routine saves it (a
     local assignment) and restores it at its group's end, so each
     output step defines the incoming value again and wakes the next one:
     244 steps.
   - The fix is "step-level soft reads": a step whose local assignment
     saved the incoming value and restored it unobserved neither reads
     nor defines that slot.
   - `PDF_FONTS` (pdf:30) and the glyphs chain behind it.
3. **Rebuild cost** (rebuild-cost). The warm word edit, profiled
   (2026-10-02, locally, 80 edit/revert rebuilds of the course in one
   process, about 72 ms each under the profiler):

   | share | what |
   |---:|---|
   | 22% | the job's end (`close_files_and_terminate`), run again every rebuild: deflate 14%, the name tree 3% |
   | 18% | the tracker's reads and writes while TeX runs (`read_content`, `stamp`, `row_read`, `table_at`) |
   | 19% | TeX itself (4,450 commands) |
   | 9% | `Fold::close` at each step's end |
   | 8% | deflating the shipped page's content stream |
   | 4.5% | `NativeHost::unchanged`: a `stat` of every input file |
   | 4% | `Edit::diff` |
   | 15% | the rest (`Version::of` 2.6%, `run_step` 2.4%, `Fold::latest` 1.1%) |

   - **Done: the job's end no longer runs for a word edit** (an early
     cutoff on each font's union of glyphs; LOG 2026-10-02). The word
     edit went from 49.3 ms to 37.9 ms on accl (job 6318).
   - The step holding `\input{ch15}` re-runs 564 commands because the
     file it loads changed, and changes nothing.
   - How the profile was taken: a frame-pointer build
     (`RUSTFLAGS="-C force-frame-pointers=yes"`, `CARGO_TARGET_DIR=target/fp`)
     and `perf record --call-graph fp -p` attached from a rebuild line
     after the cold build (as `bench/ssa-course.sh` attaches `perf stat`);
     DWARF unwinding stops after a frame or two on this binary.
   - The footnote rebuild costs 24.8 µs a command, against 1.8 µs cold:
     the rebuild path grows worse than linearly with what it re-runs.
     Profile it.
4. **Recording overhead** (records; the targets are 1.5× plain and 4 GB):
   - measure item 2;
   - the tracker's read path (`table_at` three times, the stamps: 13%);
   - a fast path for `Fold::close`'s sorted insert (a cold build only
     appends: 5–7%);
   - eqtb versions made lazily rather than hashed at every write;
   - token-list versions (`TokenList::remake`: 4%).
5. **zlib in Rust** (link; not started).
   - `crates/partex-zlib`, `no_std`, byte-identical to the system zlib
     at pdfTeX's parameters, differential tests, `PARTEX_ZLIB=system`
     to switch back.
   - Then incremental deflate: reuse the output before the first changed
     byte.
   - Why: the first rebuild after an edit spends 3.3 ms deflating the
     cross-reference stream and one object stream.
6. **aux-loop leftovers.**
   - The tools test: BibTeX matched at every stage; makeindex matched
     except DVI font numbering, which needs investigating.
   - The course label run: `bench/aux-label.sh`, on the branch
     `np/aux-loop` (c632234), not merged.
   - LOG entry.
7. **front leftovers** (uncommitted in `~/code/tmp/np-front`):
   - `\texorpdfstring` in the table of contents (789/790 entries match);
   - a speed pass (open plus views: 155 M down to 117 M instructions);
   - an edit-update benchmark;
   - DESIGN and LOG.
8. **view leftovers.**
   - Fonts and hyphenation are not placed on a rebuild, so a re-run
     paragraph imports from an earlier window than a cold build would.
     The output is right; the graph is not.
   - The per-step trace needs a re-run with the full recorder once
     records are lean.
   - The view costs about 7.5 s per course build; its measurement,
     `bench/view-course.sh`, is on the branch `np/view` (6456c0f), not
     merged.
9. **Infrastructure.**
   - `perf` is not in the accl image: add it to `scripts/accl/partex.def`
     and rebuild the image (`build-image.sbatch`).
   - `scripts/sandbox` matches `"$repo"*` as a prefix, so a directory
     named like the repo plus a suffix is not mounted. Whether to fix
     it is the user's decision.
   - `scripts/sandbox` passes `PARTEX_*` variables one line at a time:
     pass multi-line values inside it (`scripts/sandbox env K="..."`).
   - The first rebuild in `tokens` and `cutoff_pdf` costs 25–30 ms
     extra; the cause is unknown.
10. **Machine mode removal** (DESIGN 4.3 item 8), once SSA passes
    everything.
