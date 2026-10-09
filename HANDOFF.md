# HANDOFF: pure SSA (branch pure-ssa, pushed to accl only; origin/forgejo ignored)

TeX layer on the φ core (crates/phi; merged accl/main b40d4bf into
pure-ssa at 662fa3a). Checkpoint 1 approved (DESIGN 3.17). Now: the
ONE-SHOT implementation behind PHITEX_SSA_PURE=1. Report to main only at
the end, or at a design wall.

## Done means (main's conditions)
- e2e and ssa-edits byte-identical with pure on, at W=1 and W=8; the
  course and pgfsub included.
- The structural tests (DESIGN 3.17.8) and the enforced invariant.
- Parallel cold builds and rebuilds at W=1/4/8 against current SSA and
  machine mode, on accl; the speculation acceptance rate.
  - Lang::entries at paragraph and block boundaries, predicted states.
  - Rebuilds in the core's parallel rounds, cancelled by a keystroke.
- Memory against the core as built (~30 B a live node, 1.5 B sealed):
  - course peak RSS ≤ 8 GB in the cold build, ≤ 2 GB retained after
    sealing;
  - pgfsub proportionally;
  - recompute from persisted nodes (steps, definitions, sources,
    effects, creators).
- Large single-command steps accepted, but memoised macro-call
  expansion (core memo store, keyed by meaning and argument versions) is
  REQUIRED. Report the worst interiors on the course.
- The old SSA mode's exactness fixes carried over: the checklist below,
  each ticked with its test.

## Architecture (the bridge), crates/partex-pure (std; deps phi, partex-core)
- TexLang: Lang impl.
  - Op::Main is the unfold of main control; one step is one command
    (engine stop_due → true).
  - Then scans: LineBreak, PageBuild; leaves: interline glue, ship
    segments.
- The engine runs a command against a CACHE of names (its own arrays).
  Every name it read is verified afterwards with cx.read (content
  version); on a mismatch the values are loaded and the command is run
  again. Every definition is reported with cx.define, and groups with
  open_group/close_group, in order. A group's restores are not
  definitions.
- Step state (Phase A): the engine's non-name state as a value
  (snapshot), its version from the state hash without the name tables.
  It is to be shrunk to the true sequential state as the scans and
  chains land.
- Effects: each step's effects are an Effect leaf on its chain; the link
  runs at the end.

## Architecture as built (crates/partex-core/src/pure, CLI pure.rs)
- PARTEX_SSA=1 PHITEX_SSA_PURE=1 → partex-cli/src/pure.rs::run.
- pure/driver.rs Build: engine started to its first command (format
  load) outside the graph; that state is the unfold's init, the engine
  then forked (NoHost) as `base`: the value of a name no step defines.
  Input: the main file's lines (ElemIds kept across edits by common
  prefix/suffix). Op::Main unfold, a step per command.
- pure/lang.rs step: place PState if the engine is not in it (version
  check), place the main file at the core's cursor, resume one command,
  replay the PureTracker log: reads before the first group event checked
  (registered; a mismatch = enforcer hard error);
  then reads checked, definitions at each slot's last live write (end
  value), groups in order; main-file lines read → cx.next(); effects →
  Effect leaf on Chain 0 (OUTPUT); key = hash(place, k).
- phi patch (told main): cx.read honours groups closed in the step.
- FRONTIER (user decision, replaces cache+validate/rollback): the
  engine's arrays are the run's frontier. A run (a step not after the
  one the engine just ran) starts by loading the reaching definition of
  every defined name (stopgap: slots this engine ever defined, through
  cx.read; target: frontier snapshots at sparse positions, CoW, patched
  with defs in (P0, P] — core asked for defined_reaching() and a range
  query). Reads registered after the command; a mismatch is the
  enforcer's hard error. No rollback, no mark/reset.
- Pending core: cx.source + Step::Call (files), cx.mark/reset.

## Enforcer coverage (guardrail 1: nothing outside names and PState)
- Names (checked reads, defined writes): Eqtb, Font, FontTable, Hyph,
  Pdf, Dvi, Out, Read, Random, Mark, Page, Sealed, Alloc scalars (but
  the interner/state ones below).
- PState: InputState (input stack, buffer, files, scanner flags,
  pseudo files), List (nest, cur_list, align), Save (pointers, entries,
  xchain), Cond, Alloc ALIGN_STATE/AFTER_TOKEN.
- Interners (not operands; to be content-deterministic, wall 2):
  Hash/HashNext/Name/Search, Str/Pool, Alloc STR_TOP/HASH_USED/HASH_HIGH.
- NOT YET COVERED (reported by `others()` per build): Source, Line,
  Load, Glyphs, PageNode, HyphWord, Class, PdfObj, PdfName, PdfNum,
  Clock, Unknown.
- Debug enforcer (to build): each step re-run on a clean engine from
  base + PState + only the name values it read; writes, effects, next
  state must match; hard error.
## Order of work (DESIGN 3.17.9 table) and status
1. [ ] crate + Lang shell + PureTracker + driver (PHITEX_SSA_PURE=1 in CLI)
2. [ ] eqtb/registers as names (cache + verify)
3. [ ] expansion/macros; memoised macro expansion
4. [ ] catcodes/tokenisation, sources as input Seq / line names
5. [ ] groups as scopes
6. [ ] conditionals, φ
7. [ ] boxes/lists Seq, field values
8. [ ] line_break scan, interline glue node
9. [ ] page builder scan, output routine unfold, inserts, marks
10. [ ] alignments
11. [ ] math
12. [ ] e-TeX extras
13. [ ] files, \write chains, per-entry .aux slots, rerun check, tools
14. [ ] pdfTeX extras, PDF writer chains, segments
15. [ ] parallel entries, folding/sealing, memo, timings, memory

## Carry-over checklist (old SSA exactness fixes; tick with the covering test)
- [ ] virtual PDF object numbers / numbering events (3.12) — e2e pdf cases
- [ ] fonts' /F numbers by the link (FontRef), font numbers as values (DVI) — fontnum, font-rerun
- [ ] font made again in its slot when found before load (found_font, 8a5a454) — includeonly
- [ ] font_id_text positioned (3.2) — fontid
- [ ] names made where first defined (name_defined; citations [?]) — names, names-rerun
- [ ] PDF object lists placed whole with heads (5868b6a) — font-rerun
- [ ] columns as the link's flow (3.8) — every case's log/terminal
- [ ] page tail placed with its length (a9ecc56, dc035b1) — pagetail
- [ ] paragraph-start flag at a step's start (a5e77d0) — course-diff case
- [ ] expansion-scoped flags in the input state (66eb245: \ifincsname ~) — names / thesis-like case
- [ ] remembered skips replay their reads (0f2dd82)
- [ ] e-TeX saved registers above 255 per entry (3.12)
- [ ] soft reads / entry values of the save stack (3.12)
- [ ] dead save stack entries (3.12)
- [ ] streams: loads served from stores, φ of written files, keep_complete, stopped trips publish nothing (3.7) — petals_keys, petals_stop
- [ ] \write18 commands as nodes, doomed runs run none (3.7) — minted, shell
- [ ] BibTeX/makeindex as nodes (3.16) — bibtex, idx cases
- [ ] cascade gone cold runs to the job's end (5dbc810, "auxloss") — includeonly, incl cases
- [ ] fatal trip: no PDF, cut .aux per keep_complete (3.7) — petals
- [ ] virtual object ids by TeX identity, end of job's seed (ac655eb)
- [ ] sealed lines' contents read only by ship/unhbox (seal.rs) — replaced by box fields
- [ ] SyncTeX tags as values (synctex_default_on) — synctex cases

## Notes
- The course on accl: ACCL_COURSE=/home/mohaam0e/code/tmp/course-par.
- The tracer: PARTEX_PURE (purestats.rs); scripts/pure-*.py; job 7536's
  results in target/x/r7536; job 7540 (course and pgfsub edit diffs) in
  partex-phitex-runs/cmd-7540 on accl.
- Scratch: target/x.

## WHERE I AM (paused 2026-10-09 00:35)
- Bridge written and compiling (frontier semantics, no rollback): core
  pure/{tracker,version,state,lang,driver}.rs, CLI pure.rs, switch
  PARTEX_SSA=1 PHITEX_SSA_PURE=1. Never run yet.
- The release build of e346a17 finished OK after the pause (target/release/phitex is current;
  no rebuild needed before the first run).
- NEXT: run target/x/p1/doc.tex (-ini test) with and without pure; diff
  log/terminal; then e2e plain cases (bye, pages, skips) vs plain run;
  then LaTeX (latexdoc). Then frontier snapshots, files via cx.source /
  Step::Call (core pending), interner by content (wall 2), enforcer.
