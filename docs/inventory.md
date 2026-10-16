# PhiTeX client rules inventory (pages-skel @ 7e65f0b7, crates/engines/tex/src)
PARTIAL (time-boxed, cut short by coordinator). "why" = introducing commit (git log -S, oldest); full subjects in subjects.txt beside this file.
Correctness: yes = removing/wrong changes output or errors; perf = engine refutes/re-runs; unk = not established.
Engine facts used: static Claim::None/Count is trusted (a dynamic cx.claim is applied only when static claim is Pending: defuse core/commit.rs:582); static defs() mismatch -> EvalError::UndeclaredDefinition (eval.rs:250); may_define/may_define_field/predicted_defs/static_contribs/reads are refutable (perf).

| id | file:line (fn) | what it decides | cat | why (commit) | correct? | env |
|---|---|---|---|---|---|---|
| R1 | api.rs:2217 (doc_nodes) | document node texts = per-chunk slices; \input slices replaced by file pieces | A | 80d55b74/880be045 | yes | |
| R2 | api.rs:2271 (node_args) | load slice gets pkg record/optimistic hint instead of text hint; doc slice gets hint_arg_after | A,B,C | 5fe34cbc | yes (claim/no_scope in record) | |
| R3 | api.rs:2185 (is_doc) | job text sliced into regions vs one node | A | 9f678e12 | yes | |
| R4 | api.rs:2181 (FORMAT_HINT) | format text = one unbounded node | A,C | 2870c433 | perf | |
| R5 | api.rs:2199 (register_doc) | doc analysis registered by name only when one chunk; else fragments answer Any | B | 80d55b74 | perf | |
| R6 | api.rs:2110 (guess_wkeys) | round-1 guessed .aux keys: cs:r@<label>, cs:b@<bibitem>, cs:@abspage@last, only if \documentclass | B,D | cf0050fd | perf (loop-back fixpoint) | |
| R7 | api.rs:2178 (WRITTEN_EXTS) | aux,toc,lof,lot,out,idx,nav,snm declared as loop-back files before eval | B,D | db064e1c | yes (which value a read sees: loop-back vs disk) | |
| R8 | api.rs:2170 (defines_only) | .aux read back as per-field wdef: values, not text | B,D | db064e1c | yes | |
| R9 | api.rs:1645 (written_file) | file the job writes: file:<n> absent, wfile:/wkeys: loop-back declared | B | 09494215 | yes | |
| R10 | api.rs:1843 (settle) | a supplied file later found written switches to read-back | B | 09494215 | yes | |
| R11 | api.rs:1884 (settle) | new wdef:<file,field> loop-backs declared after each round | B | db064e1c | yes | |
| R12 | api.rs:1908 (settle) | cutreq contributions -> env cut:<node>=k, another round | A | 116b8efc | yes | |
| R13 | api.rs:1955/1961 (settle) | needs pdf:exact / pdf:fold -> rebuild in exact / folded writer mode | B | a18f18fb / 406f882e | yes | |
| R14 | api.rs:1701 (environment) | env: wfiledecl:, wdefdecl:, wdefset, pdf:exact/fold, cut:<pid>, loopfiles | B | 92716ec7 | yes | |
| R15 | api.rs:2157 (wdef_id) | read-back definitions keyed per (file, field) | B | 8c90e1ae | yes | |
| R16 | api.rs:2372/2429 (loaded_names, harvest_inputs) | format Defs harvested by text scan of \input/\RequirePackage/\usepackage/\LoadClass/\documentclass | C,D | cf8c06e0 | perf (hints only) | |
| R17 | api.rs:3039 (Build::with) | never-seen loads get optimistic hint under speculation | B,C | 5fe34cbc | perf | PHITEX_PKG_OPTIMISTIC=0 off |
| R18 | api.rs:3041 | pkg record values predicted | B | 5c6a8862 | perf | PHITEX_PKG_PREDICT |
| R19 | api.rs:1519 (provide) | env provider answers file: reads in place (not written files) | B | a7f2e38d | perf | PHITEX_NO_PROVIDER |
| R20 | api.rs:147 (scheduler) | speculation mode override | B | 8a8a1cb6 | perf (should be) | PHITEX_SPEC |
| R21 | hints.rs:981 (slices_of) | doc slices: paragraph regions to next depth-0, non-conditional par end; loaders own slice; cs+[..]{..} args one slice; braces never split | A | 0049e2d3 | yes (slice end decides claim/no_scope; read-on recovers open spans only) | PHITEX_SLICES_TRACE (trace) |
| R22 | hints.rs:1010 | "command in value position"/"value is a macro": text to next command claimed | A | 1db7dcc0 | unk | |
| R23 | hints.rs:1144 | \headline={..}: = and value in same slice | A | 3c96e841 | unk | |
| R24 | hints.rs:1116,1169 | loaders documentclass/usepackage/RequirePackage/LoadClass/input/include: own slice; package loads take trailing blanks (\@ifnextchar look-ahead) | A,D | 332d9d85 | yes | |
| R25 | hints.rs:1204 | vskip/hrule/unvbox/unvcopy/halign do not end the paragraph region | A | 7ec90218 | unk | |
| R26 | hints.rs:1222 | a loader ends the paragraph before it | A | 0049e2d3 | unk | |
| R27 | hints.rs:1312 | no cut at tokens after an unread catcode change (Pending tokenization) | A | 0049e2d3 | yes | |
| R28 | hints.rs:1618 (starts_par) | names starting a paragraph (incl. tex_indent:D, tex_noindent:D) | A,D | 24322f19 | unk | |
| R29 | hints.rs:1418/2820 (SCANS_ON, reads_on) | slice ending in font/vskip/hskip/... reads on into next slice: hint unioned, claim left Pending | A,C | 85582eef/8fa4f0d6 | yes (claim) | |
| R30 | hints.rs:1646 (slice_claims_nothing) | claim(0) only if slice ends in own par end, all items end inside by Known extent, no \end/\dump/\bye | A,C | f8ee8ee7 | yes (static Claim::None trusted) | PHITEX_NO_CLAIMS -> Pending |
| R31 | hints.rs:2983 | claim=Some(0) requires Document && no_scope && !reads_on | A,C | f8ee8ee7 | yes | |
| R32 | hints.rs:1358/2874 (balanced) | no_scope=true for balanced slice (braces, begingroup/endgroup, bgroup/egroup incl. callee closures) | C | 8d09fc13 | yes (Effect::None trusted; blocks leave_scope) | |
| R33 | hints.rs:2150/2176 (no_scope, cs_no_scope) | per-command no_scope class list (def, let, catcode, kern, ... params except everypar/output) | C | ea2172be | yes | |
| R34 | hints.rs:3657 (effect_none) | Exec::effect None vs Pending from hint | C | ea2172be | yes | PHITEX_HINTS_OFF |
| R35 | hints.rs:185 (class_state) | per-primitive state heads a command may set (tex.web table, expl3 tex_*:D aliases) | C,D | e164b803 (merge) | perf | |
| R36 | hints.rs:374/376/387 (PAR_END, NORMAL_PARAGRAPH, MATH_SHIFT) | par end declares pg:; normal_paragraph resets pending to next par-ending slice; math shift heads | C | 4ffe5e83/66d41451 | perf | |
| R37 | hints.rs:547 (Hint::may_define) | every bounded hint also declares ps, pages:shipped and record prefixes | C | d17b572b | perf | |
| R38 | hints.rs:630 (may_define_field) | unbounded node's per-field guess | C | 35053760 | perf | PHITEX_NO_GUESS |
| R39 | hints.rs:805 (LOOKTHROUGH) | expandafter/noexpand/input/scantokens/... (+expl3 exp_after:wN, exp_not:N/n, tex_input:D) -> Any | C,D | 0049e2d3 | perf | |
| R40 | hints.rs:2544 | a \pdf on the command line adds pdf:, box: heads | C | cd437b52 | perf | |
| R41 | hints.rs:1992/2075 (hint_run) | operand of unbounded command -> Any; active char not made by text -> Any | C | 0049e2d3 | perf | |
| R42 | hints.rs:3453/1933 (GROUP_LOCAL, selects_outside) | font/fam dropped unless selected outside every brace group | C | 813a0559 | perf | |
| R43 | hints.rs:2273 (calls_state_split) | calls inside a body's group: group-local heads dropped | C | f66a900d | perf | |
| R44 | hints.rs:2344 (defined_state) | state of a text-defined name by shape | C | 51aa69e8 | perf | |
| R45 | hints.rs:3276 (compute_hint) | hint from extra arg; file frame -> line_span_end; s-frame by registry name; \read/pseudo-file -> Any | B,C | 0049e2d3 | perf | |
| R46 | hints.rs:1477 (line_span_end) | file-rest node hint spans line + lines while group/conditional open | A,C | 3e9c68ac | perf | |
| R47 | hints.rs:3346 | char-only expansion tokens bounded (pg:,box:,lvlfix,align,badness) | C | 75555b81 | perf | |
| R48 | hints.rs:3370 | other expansion tokens demoted to Any unless env | C | 51aa69e8 | perf | PHITEX_HINTS_FRAGMENTS |
| R49 | hints.rs:3439 (fragment_context) | fragments add deadcycles, box:, normal_paragraph, pg:, aftertoken | C | e164b803 (merge) | perf | |
| R50 | hints.rs:3215 (file_hints) | \input files analysed without format defs | C | 0049e2d3 | perf | |
| R51 | hints.rs:3080 (register_text, by_name registry) | hints of text pieces looked up by file name in a mutable registry | B | 7994f9c3/4ffe5e83 | perf | |
| R52 | hints.rs:3521-3655 (Everypar, declare_everypar_run, hint_arg_after) | slices after a setter declare EVERYPAR_RUN for 2 nonblank slices + trailing blank | C,D | de0b152d | perf | |
| R53 | hints.rs:3620 (EVERYPAR_RUN) | cs:par, clubpenalty, everypar, toks:, cs:if@nobreak, if@endpe, box:0, if@minipage, if@newlist, if@inlabel, if@noparitem | C,D | 34cd7bb2/2f50d5b6/924edfaf | perf | |
| R54 | hints.rs:3637 (sets_everypar) | substring \section..\part, \@afterheading, \@startsection, \item, \begin, \end, \maketitle -> declares everypar/toks: | C,D | 0047dff0 | perf | |
| R55 | hints.rs:2935 | blank slice declares count: (\par@deathcycles of LaTeX lists) | C,D | 924edfaf | perf | |
| R56 | hints.rs:3670 (reads) | declared reads: means, cattab record, record reads | B,C | 0049e2d3 | perf | PHITEX_HINTS_OFF |
| R57 | hints.rs:3695 (may_define) | record hint -> MayDefine::Names exact | C | 5fe34cbc | perf | PHITEX_HINTS_OFF |
| R58 | hints.rs:3736-3790 (record_hint, optimistic_hint) | record: names only, no state heads, no_scope/claim0 from last run; optimistic: guess defines nothing | B,C | 5fe34cbc | record no_scope/claim0: yes; rest perf | |
| R59 | hints.rs:3822 (verify) | catcode/meaning assumptions checked at run; violation voids check | C | 0049e2d3 | perf (diag) | |
| R60 | hints.rs:3872 (check_defs) | "undeclared definition" diagnostic (gate metric) | C | 0049e2d3 | perf (diag; gate) | |
| R61 | hints.rs:3114/3100 (kind_of, looks_initex) | \catcode`\{ before first { -> Initex (one node, not sliced) | A | bb1eef52 | yes | |
| R62 | hints.rs:3125 (routine_hint) | finisher bound from \output text shown as source | C | 116b8efc | perf | |
| R63 | incr.rs:498 (Open) | chunk continues while conditional, \makeatletter, \ExplSyntaxOn, \catcode, \newif-ifs open | A,D | 4ffe5e83 | yes (chunk start = slice start) | |
| R64 | incr.rs:63 (Input::of) | \def/\let/\edef/\gdef/\xdef\input disables \input slicing | A | ? | yes | |
| R65 | inputs.rs:69 (target) | slice exactly `\input name` / `\input{name}` replaced by file's slices (depth 8) | A | 86459b40 | yes | |
| R66 | inputs.rs:108 (document_file) | sliceable only if path lacks "texmf", Document kind, no \endinput/\inputlineno/\everyeof | A | 88e7a78a | yes | |
| R67 | inputs.rs:95 (plain_input) | text redefining \input not sliced | A | 88e7a78a | yes | |
| R68 | getnext.rs:782 (doc_file) + 734 (has_input) | doc file's rest becomes region children at a command start | A | 42201a39/d452282e | yes | |
| R69 | run.rs:2310 (doc_file_regions) | \input'd doc file's rest sliced + registered at run time | A,B | 951dafc0 | yes | |
| R70 | getnext.rs:354 (pull) | run takes next exec sibling's text; top-level eof else Starve | A | d1cb8d71 | yes | |
| R71 | getnext.rs:260 (next_tok_slow) | mid-command input end pulls sibling | A | d1cb8d71 | yes | |
| R72 | run.rs:2408 (run_all) | open paragraph/group/conditional at text end reads on (pull) | A | d452282e | yes | |
| R73 | run.rs:2415 | format/package text ending inside group: groups ended, error diag | A | ? | yes | |
| R74 | getnext.rs:421 (give_back_peeked) | last pulled sibling of which only a peeked token was read is given back unclaimed | A | a69f1d93 | yes | PHITEX_GIVEBACK_ONLY=k (debug bisect) |
| R75 | getnext.rs:594/657 (give_back_resumed, same_first_token) | give-back after park; token re-read under current codes must match | A | 4841160b/a69f1d93 | yes | |
| R76 | control.rs:225 (IGNORE_SPACES) | doc run in vmode: token from a pulled sibling ends the command, put back | A | 9569d904 | yes | |
| R77 | control.rs:255 | doc run with pulled>0: command-ending token put back for give-back | A | 9569d904 | yes | |
| R78 | getnext.rs:700/721/916 (next_continues_name, next_is_continuation, join_continuation) | a line cut by slicing continues into next sibling (control word name, near end) | A | 3207be2a/69012a75 | yes | |
| R79 | expand.rs:863 (end_input) + getnext.rs:1167 | \endinput pulls siblings until eof marker | A | 69012a75 | yes | |
| R80 | run.rs:2201 (leftover) | input left -> follow nodes (token levels one node, lines, file rest), carried conds into first | A | d452282e | yes | |
| R81 | run.rs:2903 (carried_conds) | conditionals open at scope close carried into next node | A,B | 0047dff0 | yes | |
| R82 | run.rs:2436 (can_leave_scope) | doc run with exactly one simple/semi-simple group, no conds, text not no_scope, text opens a group -> leave to engine | A,B | 011e12b0 | yes | |
| R83 | run.rs:2456 (text_opens_group) | counts "\begin{" - "\end{" + begingroup/bgroup substrings | A,D | 6c4b5ab6 | yes | |
| R84 | run.rs:2378/2479 (next_opens_group) | run ends before next \begingroup/\bgroup/\begin{ when it could leave scope | A,D | 011e12b0 | perf? unk | |
| R85 | build.rs:221 (leave_scope) | Effect::Scope with kind; group locals -> Locality::Scope; ag list scope's | B | 9dd8f741 | yes | |
| R86 | runstate.rs:1065 (abandon_to_scope) | group locals equal to base publish dropped; differing -> second entry | B | 840d5b8c | yes | |
| R87 | run.rs:2532 (closed_scope) | doc run that closes engine scope ends after that command | A | 0047dff0 | yes | |
| R88 | run.rs:2771 (claim_count) | claim = pulled (+ all following exec with claim_rest after \end) | A | 35053760 | yes | |
| R89 | build.rs:2301 (its_all_over) | \end: claim_rest, input dropped | A | 3207be2a | yes | |
| R90 | runstate.rs:983 (published) | local def equal to read value not published; global too at top level (rec_load) | B | 3ab8ecad | unk (§283 cases) | |
| R91 | ops.rs:372 | rec_load only at unit top level | B,C | 919990b2 | unk | PHITEX_PKG_NONET |
| R92 | run.rs:2652/2783 (pulled_only_blanks) | load recorded only if no rest claim and only blank siblings pulled | C | be0e30cd | yes (record trusted) | |
| R93 | run.rs:2839 (record_load flags) | record says claim0 only if pulled==0 | C | 02aecd71 ("text processed twice") | yes | PHITEX_PKG_NOCLAIM, PHITEX_PKG_NOSCOPE |
| R94 | pkgrec.rs:25/54 (LOADERS, load_key) | load key = command text sans blanks/comments + file hashes | C,D | 51b70348/5fe34cbc | yes (record match) | PHITEX_PKG_NOREADS |
| R95 | pkgrec.rs hint_for | matching record hint else optimistic | B,C | 5fe34cbc | see R58 | |
| R96 | ops.rs:256 (Exec::predicted_defs) | pkg record values as predictions | B | 69cc1d77 | perf | |
| R97 | ops.rs:304 (static_contribs) | `lines` contribution from text | B | c7103569 | perf | |
| R98 | ops.rs:319/338 (claim, claim_limit) | hint claim -> Claim::None/Count/Pending | A,C | 35053760 | yes | PHITEX_NO_CLAIMS |
| R99 | ops.rs:569 (client_only) | nodes never define file: fields | B | d452282e | perf | |
| R100 | ops.rs:446 (Put) | group-end carry definitions with static defs (exact) | B,C | d452282e | yes (UndeclaredDefinition) | |
| R101 | ops.rs:582 (Files) | publishes wfile:/wkeys:/wdef: loop-backs from streams in wplan order; makeindex runs | B,D | 92716ec7/41842eda | yes | |
| R102 | ops.rs:71 | parking/resume of stopped runs | A | 1b292e30 | perf | PHITEX_NO_PARK |
| R103 | ops.rs:126/129 (run_node_parking) | page fold pass loop; >16 passes: ends as it stands | A | 998c879b | yes (escape) | |
| R104 | run.rs:785/1014 (record, load_slow, eng_read) | cell read from record value when whole, else (stale) per cell | B | ff073d01 | yes | PHITEX_RECORD_CHECK |
| R105 | run.rs:929 (ready_records) | global cell def inside scope -> stale (empty) record; scope records | B | ff073d01 | yes | |
| R106 | run.rs:909 (drop_unchanged_republished) | resume republished bindings equal to engine value dropped | B | ff073d01 | yes | |
| R107 | fields.rs:353 (RECORDS) | cattab, parpar, pagepar, sftab, lctab records | B | ff073d01 | yes | |
| R108 | run.rs:718 (revert_read_back) | job-written line reads wpre:<field> (pre-read-back value) | B,D | 2656a4c3 | yes | |
| R109 | run.rs:1122 (note_pre) | fields defined while \document's read-back macro runs get wpre: | B,D | f542ca81 | yes | |
| R110 | run.rs:1519 (read_back) | \input of .aux: definitions from wdef: loop-backs, no text | B | bba90b14 | yes | |
| R111 | run.rs:1667 (interpret_line) | written line interpreted at write time under cat:64=11 (\makeatletter) | B,D | db064e1c | yes | |
| R112 | run.rs:3385 (is_read_back_definition) | excluded prefixes cat:,pdf:,pg:,pp:,write:,... | B | f55c534f | yes | |
| R113 | run.rs:1367 (file_inner) | file read order: wkeys loop-back, wfile loop-back, file loop-back, env | B | db064e1c | yes | |
| R114 | run.rs:1755 (written_decl) | \openout asks client for wfiledecl | B | db064e1c | yes | |
| R115 | control.rs:1635/1652 (take_wpending, file_end_read_back) | multi-line written text (\@writeckpt) continued; file end read back | B,D | f8206617 | yes | |
| R116 | control.rs:1588/1903 (shell_command) | \write18 only makeindex, in-process, .ind as written file | B,D | 41842eda | yes | |
| R117 | pages.rs:640 (finish_candidate) | candidate markers only at outer vlist; stop_at -> Halt::Cut | A | 61f285fe | yes | |
| R118 | pages.rs:655/1459 (put_page_params_reaching) | k=0 marker without values; fold resolves | B | 116b8efc | yes | |
| R119 | pages.rs:800 (pin_reaching_page_params) | piece that set vsize etc. pins reaching params (multicol) | B,D | 07e9d583 | yes | |
| R120 | pages.rs:845 (page_fold) | fold apart vs fold in own run | A | 116b8efc | yes | |
| R121 | pages.rs:900 (needs_own_fold) | Ops::fin_touch reads/defs -> fold in own run | A | 998c879b | perf (cut a round earlier) | |
| R122 | pages.rs:955 (last_candidate) | page finishing at last candidate counts as at end iff nothing changed after | A | 116b8efc | yes | |
| R123 | pages.rs:975 (fold_apart) | fold node gets reads + per-candidate defs for check_cut | A,B | 116b8efc | yes | |
| R124 | pages.rs:1056 (fold_here_) | claim, narrow(Names) before ps read; cut at fired candidate; finisher+resume placed | A,B | 116b8efc | yes | PHITEX_DEBUG_NODES (trace) |
| R125 | pages.rs:1208 (check_cut) | finisher asks cut if routine gdef changed a read value or piece read a field defined after candidate; else piece's standing defs restored over routine's | A,B | 116b8efc | yes | |
| R126 | pages.rs:1187 (read_cut_request) | piece reads env cut:<node> | A | 116b8efc | yes | |
| R127 | pages.rs:724 (streams_since) | wstream:<name> pseudo-fields order \write vs page | B | 7a44d55f | yes | |
| R128 | pages.rs:1690/1881 (cont_encode, cont_load) | resume continuation: input, nest, conds, state table; override by gdefs (definition) or value change | A,B | 0e182658 | yes | PHITEX_CONT_SHOW (trace) |
| R129 | pages.rs:1996 | override when value at cut != value now (by value) | B | 0e182658 | yes | |
| R130 | pages.rs:2263 (contribute_global_defs, "gdefs") | finisher's global defs accumulated for resumes | B | 07e9d583 | yes | |
| R131 | pages.rs:2114 (FINISHER_FIELDS) | ps, shipped, vtail, deadcycles, mark:, outputpenalty, inspen, box:, badness, write:, pdf: | C | 116b8efc | perf | |
| R132 | pages.rs:2139 (Finish::may_define) | Names from earlier runs of same routine (FinDefs.seen) | C | 998c879b | perf | PHITEX_NO_FIN_PREDICT |
| R133 | pages.rs:2103/2180 (FinDefs::stable, predicted_defs) | defs equal in last two finishers predicted | B | 998c879b | perf | PHITEX_NO_FIN_PREDICT |
| R134 | pages.rs:2280/2308 (fin_olds, record_finisher_defs) | fin_touch: routine gdefs only if value changed | B | e8394e44/116b8efc | perf | |
| R135 | pages.rs:2418 (deadcycles_for) | fold reads deadcycles only with \end item | B | 998c879b | perf | |
| R136 | pages.rs:2381 (Fold) | static may_define ps,vtail; reads ps,vtail,deadcycles,pagepar | C | 116b8efc | perf | |
| R137 | pages.rs:2551 (Resume) | Any, Pending effect and claim | A,C | 0e182658 | yes | |
| R138 | pdfrelaxed.rs:76, pdfout.rs:94, assemble.rs:242 | relaxed/exact writer fall-backs via needs | B | a18f18fb/406f882e | yes | |
| R139 | vprof.rs:60 | scan beside every fold, compared | - | 19df7adf | no (check) | PHITEX_SCAN_CHECK |
| R140 | exec.rs:69/364 | job grain / threads | - | 70052906 | no | PHITEX_GRAIN, PHITEX_THREADS |
| R141 | control.rs:85 | process exits after N commands | - | ? | debug | PHITEX_HEARTBEAT_EXIT |

Trace-only env vars (no behaviour change): PHITEX_AUX_TRACE, CLAIMS, CONT_SHOW, DEBUG_CLAIMS, DEBUG_NODES, DECL_TRACE, EXEC_TRACE, FIN_SHOW, GIVEBACK_TRACE, HEARTBEAT(_FROM), HINTS(=1/2/3), HINTS_DIGEST, ITEMS_TRACE, LOOP_TRACE, OBS, PAGE_TRACE, PDF_TRACE, PKG_STATS, PKG_TRACE, PUB_TRACE, READS, REC_TRACE, RERUN_DIFF, SLICES_TRACE, STEP_TRACE, TEX_TIME, TEX_TRACE, UNDEF_TRACE, WIDTHS; PHITEX_INCR_DOC (ignored test only).

crates/core/typeset: no graph rules found (only PageBuilder::measure_only for vprof).

## Not covered (time cut)
- commit messages not read per row: only hashes; subjects in subjects.txt; why column unquoted.
- hints.rs 1700-1900, 2420-2720 (Analysis::with_own, cats_of, may_start_par, add_set, guess_call, align_atoms, add_defs_with) only skimmed.
- run.rs park/from_parked (3129-3325), runstate.rs cont()/override_global/park internals, nest.rs, input.rs.
- pdfout.rs/assemble.rs/pdfprim.rs/pdfrelaxed.rs beyond the fallback sites (XPAGES, place node, oplan/fplan successors).
- etex.rs, align.rs, math.rs, scan.rs, expand.rs (except start_input/end_input), fields.rs beyond records.
- grep terms not swept file-by-file: `Locality::`, `Effect::` outside ops/pages/build, `scope`, `index`, `__`.
