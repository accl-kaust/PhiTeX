# Inventory classification (Phi, 2026-10-16 12:10), rows from inventory.md

## 1. Become speed hints once the engine verifies declared claims/effects (request sent to IncEngine)
R29 reads_on claim, R30 slice_claims_nothing, R31 claim=Some(0) conditions, R32 balanced/no_scope,
R33 cs_no_scope list, R34 effect_none, R58 record claim0/no_scope, R92/R93 record claim0 conditions,
R98 hint claim -> Claim::None/Count, R137 Resume Pending. Today a mistake in any of them = wrong output;
after the engine change = a re-run. Then audit each for cost, delete those that do not pay.

## 2. Special cases to delete (granularity rule, names of LaTeX/expl3 commands or file kinds in code)
- R82/R83 can_leave_scope restrictions + text_opens_group: DELETED in loopback-units 0d1155c9 (B).
- R84 next_opens_group: text test for \begin/\begingroup; re-check after B.
- R64-R67 static \input text matching (`\input name` / `\input{name}`, \def\input guard, texmf path test):
  run-time slicing (R68/R69) already covers every route; delete the static path once the sweep shows
  run-time slices give own nodes (sweep red today: chap1/chap2/.ind have no own nodes).
- R7 WRITTEN_EXTS (aux,toc,...,snm list) and R6 guess_wkeys (r@, b@, @abspage@last): any file the job
  opens for writing is loop-back (R114 \openout already asks); round-1 guesses from the last build, not names.
- R16 loaded_names text scan, R24 loader names own slice, R26, R28 starts_par names, R63 chunk-continue names
  (\makeatletter, \ExplSyntaxOn, \newif): replace by the hint pass's own state (catcode/meaning), not names.
- R52-R55 EVERYPAR_RUN, sets_everypar substring list (\section..\part, \item, \begin...), count: on blank slice:
  replace by observed reads (claims-as-guesses makes a wrong guess cheap).
- R116 \write18 only makeindex: keep as policy (no shell), but the producer is a plain file producer.
- R61 \catcode`\{ before first { -> Initex: check, likely mechanics.

## 3. TeX semantics / engine mechanics: keep
R70-R81 (pull, give-back, read-on, leftover, carried conds), R85-R89, R100-R111, R113-R115,
R117-R130 (pages, cut, resume, overrides), R138 writer fallbacks, R101 loop-back publication.

## 4. Perf hints: keep only where measured
R35-R51, R56, R57, R59-R60, R62, R96-R97, R99, R102, R121, R131-R136. Rule from main-branch lesson:
a hint may only make things faster; a wrong hint must never change output.

## Not inventoried yet (agent was cut short): hints.rs 1700-1900/2420-2720, park internals, pdfout/assemble,
etex/align/math/scan/expand. Next inventory pass after the claims change lands.
