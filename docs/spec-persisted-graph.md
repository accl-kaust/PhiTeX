# The persisted graph (spec, 2026-11-16)

Status: step 1 (the engine's export and import) and step 2 (the
client's tables, `Build::save` / `Build::load` in process, §2.2, §12)
built, with pdfTeX's link state (step 2b, §2.2.4); step 3 (the cache
file, its keys, the environment re-ask, across processes, §13) built;
step 5: on by default in `phitex build` and `phitex watch` (§15); the
builtin (wasm) engine of VS Code, the web demo and the Overleaf
extension keeps graphs too, saved at a pause (§16). Branch
`persist-design`: the design's observe-only probe (`PHITEX_PERSIST_PROBE`). It builds on
docs/note-persisted-graph.md (the 11-01 design note),
docs/spec-seeded-reads.md (branch `seeded-r2`: why seeding definitions alone
failed) and docs/note-intermediate-reruns.md §5-6 (why the whole graph and
not the page layer alone).

## 0. In short

- **What it is.** At the end of a build or a watch, the whole engine graph
  is written to the cache: nodes, their last records, the line's order,
  the claims, the paths, and the client's tables that a collection keeps
  (`crate::collect`). The next `phitex build`, `phitex watch` or editor
  session on the same document, engine, format and inputs loads it. That
  process starts from the state a watch has after its first build. Its
  first build is then an edit: the changed files are staged and the
  evaluation runs what the watch would run for that change. With nothing
  changed, nothing runs.
- **Measured** (acclnode11, local disk, -j8 release, §9):
  - The quiet graph after a build is consistent: every clean node's every
    read holds under the engine's own check (0 failures in 0.32-1.46 M
    reads on amsldoc, acm, Grail and para4000).
  - The encoding round-trips: 0 records differ.
  - Encoded whole, the graph is 25 / 30 / 52 / 43 MB, or 9 / 13 / 15 / 15 MB
    with zstd -3. Without the values the environment gives again (the
    format's and the files'), it is about 12 / 9 / 37 / 31 MB raw.
  - Times, one thread: encoding 120-260 ms, value decoding 82-138 ms,
    record decoding 15-78 ms.
  - An edit with nothing changed costs 0.07-0.5 ms in process and runs 0
    nodes.
- **Expected second build, unchanged** (-j8): about 0.3-0.4 s for
  amsldoc, acm and para4000, and 0.35-0.55 s for Grail. Today they take
  1.9-2.1 s, 2.05-2.3 s, 0.65-0.69 s and 7.1-7.4 s. With an edit between
  the processes, add the watch's time for that edit.
- **Recommendation: build it**, in the five steps of §11. Each step is
  green on its own and proven by its own gate. The main risk is a wrong
  PDF from state the file leaves out. It is closed by construction: the
  inventory is the collection's roots, every input is keyed, and every
  load is an edit. A gate mode builds each document cold, then
  loads it and rebuilds unchanged and edited, at -j1 and -j8, and
  requires identical bytes.

## 1. Why the seeds failed and the graph does not

Seeded reads (`seeded-r2`) gave a new process the last build's
*definitions* as predictions. The seeds were right (confirmed predictions
went from 4.7 k to 20.4 k), yet the runs did not fall: 8.8-11.9 k runs
against 9.0-9.7 k. Three things were still made anew:

- the page layer's feedback (vprof ranges, cuts, continuations,
  `pages:used`, `cont:N`);
- the claims, so siblings ran 3-4 times before the claim landed;
- the intermediate commits of the text chain (`skind`, `count:188`,
  `\everypar`), which re-ask every reader of a field at each change.

A seed is a guess that has to be confirmed by a run. A loaded record is
not a guess: it is the run, with the reads it made. The engine already
trusts such a record in the same process. A clean node is not run again
until one of its reads moves. Nothing in the loaded line moves unless an
input moved, so nothing runs. That is the watch's second build: 0 runs,
0.07-0.5 ms (§9, Table 2).

## 2. What is saved

The rule: **what a collection of the value store keeps is what a load
must restore; what a collection drops is never saved.** `Engine::roots`
and `crate::collect::roots` already list everything a later run or the
link decodes. A missing item there would already break collections, so
that list is the inventory.

### 2.1 The engine (`phitex_defuse`)

Per node that is not `Gone` (`Clean` or `Inert`: a quiet line holds no
other state; a load asserts it):

- its operation, saved by **name** (`Operation::name`), with a table of
  names in the header; and its arguments (`Node::text`, values);
- `parent`, `depth`, `unit`, the claim (`claimer`, `claim_end`) and state;
- `last`, its last committed `RunRecord`, field by field: `output`,
  `defs` (field, locality, value, relation), `contribs`, `reads` (kind,
  anchor node, path, filed bit, seen), `sources`, `placed`, `claim`,
  `claim_all`, `effect`, `state` (`Ctx::park`), `waits`, `text`, `told`;
- `prior` (`Node::prior`), the may-stand-again record, encoded the same
  way. It is optional: a load without it is still exact, only slower when
  a value moves and moves back.

The line:

- each unit's nodes in order (`Engine::order`). Labels and segments are
  made anew by a bulk layout, as `push_many` makes them.
- the paths table (`Paths::steps`). Path ids are interned in the order
  runs first meet them, which differs between runs at -j8, so the whole
  table is saved and reads keep their ids.
- the relations learnt (`Engine::shapes`) and the member paths
  (`member_paths`).

Not saved, because they are derived, rebuilt by filing the records
(§4.3):

- the segments' definitions, contributions and sums, and the fields read
  at their nodes (`Inner::guarded`);
- the field indexes (`defs`, `contribs`, `readers`) and the range index;
- the foreign readers and witnesses (`guard::Foreign`), the pullers and
  bounded readers;
- the settle marks;
- the scope chain caches.

Not saved, because they are per evaluation or empty at a quiet point:

- wakes, held wakes, `unwalked`, `movers`, `edited`, `pred_pending`;
- run counts and epochs, the frontier and the window;
- timings and the memo of applications (`Engine::memo`, a cache).

Not saved, because they are given again (§3.3): the environment's values
(`Env::values`), meaning the format's bindings and the files read. The
saved reads of the environment carry what they saw, which is how a load
finds what moved.

### 2.2 The client (`phitex_tex`, `phitex_live`): the inventory (step 2)

Step 2 built the client's half in process: `Build::save` writes the
engine's graph (`Engine::export`, every value the client's tables hold
among its roots) and the client's tables; `Build::load` makes a fresh
build of them (crates/engines/tex/src/api/persist.rs). Each item of the
client's state is one of:

- **saved**: in the bytes;
- **rebuilt**: made again on the load from what is saved or from the
  format, the same as the saved build's (checked where a check is cheap);
- **options**: taken from the options the load is given (the finder,
  the threads, the tools, the progress, the sender of texts), as a cold
  build takes them;
- **re-asked**: from the environment (step 3: the input manifest, §3.2);
- **dropped**: a cache, made again when asked, or per evaluation, empty
  at a quiet point;
- (none is left unsaved: pdfTeX's link state, the step-2 blocker, is
  saved since step 2b, §2.2.4).

**Values keep their places.** A product that hashes value ids (a page's
key, a read-time command's serial, a sealed line's head hash, a cut
file's line owner, the finishers' table key, the pages' shown ids, the
buckets of the fields a piece read across its candidates, the field a
routine's value left over a piece's later definition) is
only the same in two stores that hold every value under the same id. The
import puts each plain value at its place in the exporting store (its
payload less the store's base, `Values::stable_raw`), each sequence at
the id its content makes there (an element's hash is of its stable id),
and the products above hash stable ids. A client table's value is saved
as its stable id and found again by `Values::from_stable`. Two stores of
one process still give their values distinct raw ids (their bases), so
the process's caches keyed by raw ids (a font's file, a map's text,
hyphenation patterns) never take one store's value for another's. Before
this, a loaded build's runs remade those products under other ids: a
sealed line made before the save was not found by its head hash after
it, the finishers' table was missed, a cut file's lines were given under
another field, and every finisher reading `pages:used:<piece>` ran again
(its buckets by raw field ids). The link's pages and their records are
kept by the store's instance and key (their keys are the same in two
stores of one process that hold the same page).

#### 2.2.1 The ops (`run::Ops`)

| item | class | why |
|---|---|---|
| `fin_defs`, `fin_defs_start` (keys by stable ids), `fin_class` | saved | learnt per routine: predictions and declarations depend on them |
| `docs` (each `DocFile`: path, text, slices, readers, shifts, placed, arguments, lines) | saved | the cut files' state; its `Incr` **rebuilt** by analysing its text, the slices checked equal to the saved ones (else the load is refused) |
| `rest_lines` (keys by stable ids) | saved | the lines a file's rest's pieces are given |
| `fmt_made` | rebuilt | a fresh `fmtenv::Made`: a format's value made again when first asked is found at its saved id (interning by content) |
| `held` | rebuilt | made from the environment, job and files at each evaluation's start |
| the pages the live runs name (`pdfout::PAGE_KEYS`'s pieces) | saved | by their values and fonts' record, §2.2.4 |
| `pdf_cmds` (the read-time commands' steps) | saved | as data (`pdfout::step::Step`), §2.2.4 |
| `defs` | rebuilt | the format's definitions (`prelude_defs`), as a cold build makes them |
| `hints`: the analyses registered (`by_name`) | saved | `TextHints` encoded (`hints::kept`); a node's hint of a file read before is the registry's, history-dependent |
| `hints`: node hints, decoded, answers, prepared | dropped | memos of the arguments and the registry |
| `hints.store`, `tool_opts`, `out_ok`, `xe`, `origins`, `node_rec`, `cut_min`, `fin_predict` | options | as `Build::with` sets them |
| `pkg_loads` (`pkgrec::LoadHints`: records, keys asked and changed, files' hashes, predictions given, counts) | saved | the speculation of a load's readers depends on them; the source and finder from the options |
| `pkg_log`, `rec_ran`, `fatal_starts`, `text_cuts`, `pic_inline`, `spec_window` | dropped | per evaluation (cleared or taken at each) |
| `syms` (the deferred counters' symbols, in their numbers' order) | saved | a symbol's number is in values |
| `counters` (the `\countdef` names) | saved | names interned again in this process's control-sequence table |
| `scope_learned`, `rel_keys` | saved | predictions of brackets; read-time object identities |
| `units_folded`, `siblings_pulled`, `give_back_guesses`, `edit_stall_ms` | saved | counters and the edit's stall bound |
| `stream_objs` | dropped | reset at each evaluation that streams no page |
| `tcx` | dropped | parsed once from the `tcx` field |
| `audit` | dropped | the state model's tests' record of each node's last run, filled again by the next run |
| `watch` | options | the progress |
| `fields`, the operations and projections | rebuilt | `Graph::new` registers them in the same order |

#### 2.2.2 The graph (`api::Graph`) and the build (`api::Build`)

| item | class | why |
|---|---|---|
| `start_env`, `eval_errors`, `eval_warnings`, `no_fixpoint`, `shell_store`, `env_pushed`, `absent_pushed`, `marks_pushed`, `xpages_pruned`, `env_calls`, `job`, `files`, `reads`, `queries` (with absent), `tool_reads`, `absent`, `written`, `seeds`, `made_seeds`, `opened_last`, `marks`, `wkeys_guess` | saved | what the next evaluation's environment, provider and loop-backs start from |
| `provided` | dropped | absorbed into `files`/`absent` at each evaluation's end: empty at a quiet point |
| `opened_memo` | dropped | a memo of the `streams` value |
| `env_base`, the environment provider | rebuilt | the format's bindings (`job_env`'s closure); the provider is installed again at each evaluation (`Graph::provide`) |
| `tool_finder`, `shell_cap`, `threads`, `watch` | options | |
| Build: `nodes` (key, node, place), `dlines`, `dvi`, `end`, `settled`, `text`, `canon`, `raw`, `ini`, `settles`, `loads_ran`, `evaluated`, `stages`, `doc`, the unit | saved | |
| Build: `stats` | saved (version, live nodes) | the rest is the last evaluation's report |
| Build: `incr` | rebuilt | the document's analysis made again from its text; its slices must be the saved nodes' (else the load is refused). Measured equal after every scripted edit of amsldoc, acm, Grail, para4000 |
| Build: `serial` | rebuilt | the process's own numbering |
| Build: `staged` | dropped | false at a quiet point |
| Build: `memo`, `lines`, `diag_memo`, `diags_last`, `spans_memo`, `origins` | dropped | caches |
| Build: `opts`, `edits`, `prelude` | options | the format as a build of it finds it |
| the output file (`assemble::linked_files`) | saved | its bytes, kept again under its hash for the new store (`File::Bytes`): a build that runs nothing gives it again; a viewer's layout of it is the next link's |

#### 2.2.3 The live session (`phitex_live::Session`) and the process

The step-2 check (`PHITEX_GRAPH_CHECK`) swaps the build only; the
session's tables are step 3's, with the cache file:

| item | class |
|---|---|
| `inputs` (name, found, path, ours), `stamps` | re-asked: the manifest's stat keys and hashes (§3.2) |
| `page_hashes`, `built_text` | rebuilt: the build's pages and text |
| `inputs_by` | rebuilt: an index of `inputs` |
| `stored`, `made_stored`, `wkeys_from`, `records_dirty` | dropped: what was last stored in the cache, which the load reads; the first flush after a load stores again |
| `synctex`, `lines` | dropped: caches |

| process static | class |
|---|---|
| `pic::REFUSED`, `pic::MANY` (pictures refused, pictures with more after them) | saved: they change what a run does with a picture; the sets are joined to the loading process's (both only grow) |
| `assemble::LINK_INPUTS`, `LINE_MEMO`, the filled and named pages; `vprof`'s decoded profiles; `nodes::MADE`; `pdfout`'s records, link memo, line cache, maps, PostScript maps, glyph tables, text bytes, allocation memo; `xdvpdf`'s kept files, link memo, packed streams; `hints::by_id`; the font, map, pattern and tool memos | dropped: memos keyed by the store's instance or epoch, by bytes, or by raw ids, which two stores never share (their bases) |
| `ops::RUN_LOG`, `COMMANDS`, `COMMANDS_BY`, `RECUTS`, `EXEC_CLAIMS`, `control::LINES_TAKEN`, `nodes::MADE_STATS`, `FIN_PREDICTED`/`FIN_UNPREDICTED`, `feedback::MISSES` | dropped: counters |
| `val::CS_BY_ID`, `TABLE` (control sequences by number) | safe: values hold a control sequence by its name (`Tok::encode`), never by the process's number |

#### 2.2.4 pdfTeX's link state (step 2b, saved)

The exact PDF's link (pdfTeX) places, besides the line's records (values,
saved), two tables of the process that are no values:

- **the pages** (`pdfout::PagePiece`, by key): the page's box, its
  whatsits, parameters and writer's inputs (all values, or made from
  them: the box is `nodes::decode` of the page value's box), and **its
  fonts** (`pdfout::Fonts`: the fonts as loaded, their map entries, font
  and encoding files, virtual fonts), which the page's run made by its
  reads (`fonts_for`: each font's info field, font files, `pdftex.map`,
  `\pdfmapfile`/`\pdfmapline`, expansion parameters, PK settings, the
  images' files);
- **the read-time commands** (`Ops::pdf_cmds`, by serial: `\pdfobj`,
  `\pdfannot`'s and `\pdfstartlink`'s numbers, outlines, `\pdfximage`,
  `\pdfxform`, map lines, `\pdfglyphtounicode`, colour stacks, ...), with
  their fonts.

Before step 2b a loaded build had neither, and an edit that ran the link
gave another PDF (29 pdfTeX documents of the corpus). How they are saved
now (crates/engines/tex/src/pdfout/save.rs, pdfout/step.rs):

1. **Fonts by their reads.** `fonts_for_with` and its helpers
   (`map_line_files`, `image_fonts_with`, the map and PostScript map, the
   writer's file lookups) read through `FontAsk`: of the run
   (`Recording`, each answer kept, in order: a value (a font's info
   field, a file found, a cell), an expanded font's parameters, a PK
   request), or of a record (`Replay`). The fonts keep their record
   (`FontsRec`: the question, `ids`, `space`, `read_in`, whether
   `image_fonts` was asked, and the answers). The save writes it (its
   values by stable id, roots of the export) and the fonts' digest; the
   load makes the fonts again by answering the same questions
   (`fonts_again`), refused where a question is not the one answered or
   the digest differs.
2. **A page by its values.** Its words, parameters, margins, whatsits
   (handle, value, special), its page value, fonts record, sealed lines'
   stand-ins and handles, pieces, places, writer's inputs, chain index,
   node and break; its box decoded again from the page value's box as the
   page's step decodes it, its stand-ins and handles checked equal to the
   saved ones. Kept under its key for the loading store.
3. **The commands as data.** The closures are gone: each command is a
   `Step` (fourteen kinds: font name/number, page reference, colour
   stack, object reserved, object, new object number, outline, font
   attributes, included characters, map file/line, glyph to Unicode, no
   built-in ToUnicode, image, form), applied by `Step::apply`. A kind
   whose bytes are a value's keeps the value: an object's `file{...}` and
   an image's file by their text values (bytes made again of them), an
   outline's action value, a form's box value (decoded again) and its
   whatsits' sources (a `\special`'s node or the writer's data value, the
   relocation map the run made of its links' numbers). The rest is bytes
   and numbers.
4. Collection keeps every value a command or a page's fonts record holds
   (`pdfout::collect_cmds`, `collect_pieces`).

**acm's extra resume** (24 vs 25 runs, then 19 vs 20 on sample-sigconf
at -j1, the twin running node 478, `resume`, once more) was not the
link's numbers: it stayed with the link saved. The run log shows the
build holding the node at the frontier before its run, the twin running
it and stopping at a read past the frontier (`cs:__hook para/begin`), the
run thrown away. The node's `held_frontier` flag (learnt where a run of
it stopped at the settling layer's frontier, never cleared) was not
exported. Now it is (`NF_HELD_FRONTIER`, the engine's bytes version 3),
and the twin runs what the build runs (19 / 19 and every other edit).

**The two -j8 report rules.** At more than one thread the gate compares
an edit's outputs (PDF, log, written files) after a load, not its run
counts, and the sweep's bar of two runs a node a round is reported
apart, not red. Both quantities depend on the schedule with the outputs
identical, shown without any load (n11, one binary, PHITEX_GRAPH_CHECK
off, the same history in three processes): acm sample-sigconf's first
two edits ran 62/63/63 and 43/42/41 runs, amsldoc's the same each time,
the PDFs identical; revtex-aip's line-308 edit at -j8 ran 250, 358 and
258 runs for 132, 136 and 132 nodes (at -j1: 209 for 131), the history's
PDF the cold build's each time. Runs ahead of their inputs are the
schedule's; the bar is -j1's (scripts/ci/editsweep.py) and stays red
there, where the persist gate compares runs, runs by operation and nodes
too.

### 2.3 Measured sizes

See §9, Table 1. Values make up 75-82 % of the bytes. Strings are 11-18
MB of them, nearly all the format's bindings and the files' texts, which
the environment gives again. Reads are 2.3-15 MB (Grail's 322-470 k
reads anchored `Before` are its pictures'). Nodes, records, paths and
the client's own values are under 2.5 MB together.

## 3. Keys and invalidation

### 3.1 The file's key (header): any mismatch means no load

- `engine_id()` (the source hash). Any code change gives a new engine,
  and the graph is dropped. There is no migration.
- the graph format's version (the encoding's own number);
- the format's key (`Build::prelude_key`, 32 bytes) and the format's
  name;
- the Shelf release (`tl2026.N`) and the TeX Live tree identity (the
  release stamp the finder serves from), or the distribution root's
  identity when there is no Shelf;
- the options that change output, hashed: `--exact-pdf`, the engine
  (pdf/xe), the output format, the shell-escape mode, the interaction
  mode, the job name, `SOURCE_DATE_EPOCH`/`FORCE_SOURCE_DATE`, the
  synctex and origins options, and every `PHITEX_*` variable that
  changes a run's product (each such variable is listed in one table
  that the key reads, never ad hoc);
- the document: its main file's absolute path and the output directory.

`-j` is not in the key. A committed record is a function of its text
and reads, not of the schedule. The probe shows the graph after -j1 and
after -j8 are both quiet and consistent (§9). The gate's mode loads a
-j8 graph at -j1 and the other way round.

### 3.2 The inputs: each changed one becomes an edit

The header carries the input manifest: every file a run read (`fread`,
`Build::files_read`; the `file:`, `dfile:`, `fsize:` and `tfile:`
environment fields), each with the finder's path and a content key.

- **The document's own files** (`inputs` with `ours`): the stat key
  (`file_stat_key`: length, mtime ns, inode). Where it differs, the
  content hash is taken. If the hash differs, the file is staged
  (`Build::stage_file`), exactly as the watch stages a changed file. The
  main file's text is compared with the saved text (`byte_ends`), as
  `Session::run` does.
- **Distribution files** (the Shelf or TeX Live): covered by the release
  in the header, plus the finder's resolutions. Every name the finder
  resolved, or did not find (`Graph::queries`, absent files too), is
  resolved again by the same finder. A document-local `article.cls` that
  now shadows the distribution's changes that name's resolution, and the
  file is staged. Measured: the document's inputs' keys take
  0.03-0.09 ms. Re-resolution is a stat per name (124-245 found names,
  under a millisecond).
- **Files written and read back** (`.aux`, `.toc`, `.out`): these are
  the graph's own loop-back state, its `At::Final` reads. They are not
  inputs. A user who deletes `paper.aux` changes nothing for the graph,
  as for the watch.

### 3.3 The environment's other answers

The environment provider (`Graph::provide`) answers more than files:
`dline:` from the cut documents, box defaults, tool files. On load the
environment is not restored. It starts empty, and every environment
field read by a saved record (2.4-8.6 k reads, §9) is answered by the
provider when first asked, as in any build. Each saved environment read
is checked against its new answer once at load
(`Engine::update_environment` with the fields whose answer moved). A
moved field re-asks its readers (`Cause::Environment`), which is the
edit path. A provider answer that depends on something outside the key
is caught this way, whatever it is.

## 4. A load becomes the watch's session

### 4.1 The invariant

At the end of an evaluation the line is quiet:

- every live node is `Clean` or `Inert`;
- no wake is pending;
- every clean node's every read holds.

The third point is not assumed. The probe checks it with the engine's own
`RunRecord::read_holds` (no epoch shortcut) over the whole line after
each build: 0 failures in 517 k reads (amsldoc), 322 k (acm), 1.42-1.46 M
(Grail) and 1.08 M (para4000).

A plain re-answer of a read's path differed for 2-3 acm reads: member
paths of a font identifier defined later on the line. The engine's check
holds them. A load relies on that check, never on a plain answer.

A loaded line meets the same invariant, so the engine cannot tell it from
a line that was just evaluated.

### 4.2 The order of a load

1. Read the file and check the header (§3.1). If it fails, there is no
   graph, and the build is a clean build.
2. Check the manifest (§3.2). Collect the staged files and the new main
   text; nothing is applied yet.
3. Decode everything into fresh structures: the values into the new
   store, then the records, the paths and the client's tables. Node ids
   are renumbered in table order; ops and projections are mapped by name.
   No engine is touched until the decode is whole.
4. Install, per segment, in parallel (§4.3). Then set `holds_on`: an
   evaluation has ended, so `sched::hold_for` orders reruns as after one.
5. The session holds a `Build` as after its first build. `Session::run`
   then takes its edit branch: the staged files (`stage_file`), the text
   (`edit`) and one evaluation. With nothing changed this is a no-change
   edit: 0 runs, measured at 0.07-0.5 ms in process.
6. Output:
   - nothing re-ran and the PDF on disk has the saved hash: the file is
     left as it is;
   - else the link runs as an in-process edit's does.

### 4.3 Filing a loaded record

A record is filed the way a commit files it (`commit::commit`'s filing
half), with nothing to re-ask, because nothing moved:

- the definitions go into the segment's `defs`, with the field index;
- the contributions go into `contribs`, with the segment's sums folded
  once per segment;
- the reads' fields go into `guarded`;
- the fold reads go into the range index;
- the anchored reads go into the anchor's `Foreign`;
- the sibling reads go into the pullers;
- the settling operations' fields go into the settle marks.

Per-evaluation numbers start afresh:

- each record's `serial` is 1, and each node's `serial` is 1;
- the reads' `epoch` is `u64::MAX`, so the first check of a loaded read
  asks it (`read::holds`) and never takes the epoch shortcut against a
  new engine's counters;
- `scope_gen` and `moves_gen` are the new engine's 0.

The filing half has to be a function of its own, used by both the commit
and the load. Two copies would drift (§10, risk 2).

### 4.4 What the first build after a load does

It is an edit with a cause per change:

- `Cause::Edited` for the main text's changed slices (`apply_text`);
- the staged files' re-asks;
- `Cause::Environment` for moved environment fields.

From there it is an in-process edit, in every respect: loop-back passes,
the page layer, claims, cutoff on equal products. A changed second build
therefore costs the load plus the watch's time for that edit.

## 5. Staleness is impossible, not unlikely

A stale graph would give a wrong PDF. It is closed off as follows.

1. **What a run sees comes through recorded reads, or through state the
   file holds.** A run's product is a function of its text, its reads
   and the client tables it decodes. The reads are in its record with
   what they saw. The client tables are in the file, because the
   collection roots name them (§2.2). Process-global caches are dropped,
   as at a collection, and made again.
2. **Every outside input is keyed or re-asked:**
   - the engine, the format, the distribution and the options are in the
     header (§3.1);
   - the document's files are in the manifest (§3.2);
   - every other environment answer is re-asked and compared (§3.3).
3. **A load is an edit, never a result.** Nothing is taken from the
   file as output. The file gives records. Whatever moved re-asks its
   readers through the engine's ordinary path, whose cutoff on equal
   products is the same as in the watch. The PDF is left on disk only
   when no node ran and the file's hash is the saved one.
4. **Checks that prove it:**
   - `PHITEX_GRAPH_VERIFY=1`: after install, every clean node's every
     read is checked with `read_holds`, the probe's check. It costs 58-83
     ms on amsldoc, acm and para4000 and 0.72-0.77 s on Grail. Any
     failure is a red, never a fallback.
   - `PHITEX_RERUN_CHECK=1` on a loaded graph (observe builds): every
     clean node is run again and its product compared
     (`Engine::rerun_check`).
   - **The gate's watch-equals-cold mode** (§11, step 3), per corpus
     document, at -j1 and at -j8:
     1. a clean build, saved;
     2. a second process, unchanged: 0 runs, PDF and log byte-identical;
     3. a second process after each scripted edit (the edit corpus of
        the watch tests): identical to a clean build of the edited
        document;
     4. a second process after a changed `\input` file, `.bib`, image
        and local `.sty` that shadows the distribution's, and after a
        `SOURCE_DATE_EPOCH` change: identical to a clean build;
     5. a -j8 graph loaded at -j1 and a -j1 graph loaded at -j8.
   - Fuzzing the file: truncation at every chunk boundary, a flipped bit
     per chunk, a header from another engine. Each must give a clean
     build with identical bytes, never a load.

## 6. File size and load-time limits

- **Size** (measured, §9): 9-15 MB zstd -3 whole; about 4-6 MB for the
  papers and 12-15 MB for Grail and para4000 once environment values are
  left out. A thesis of ten Grails is 150-500 MB raw, under the cache's
  4 GB LRU limit, which evicts graphs like formats (oldest first). A
  graph is one file per document, `docs/ID/graph`, beside `noderec`.
- **Bars:**
  - **Load:** unchanged second build at -j8 under 0.5 s on amsldoc, acm
    and para4000 and under 0.6 s on Grail (n11, local disk, median of
    5). That is the fixed start (0.24 s, measured on a one-line
    document) plus at most 0.3 s of load.
  - **Save:** at most +5 % on a clean build's wall at -j8, and 0 when
    nothing changed (the file stays). One thread today encodes in
    120-260 ms and compresses in 50-140 ms (zstd -1/-3), which is +5 %
    on Grail and up to +50 % on para4000, so the save must be:
    1. after the PDF and log are written, with the result printed;
    2. parallel: one chunk per segment, encoded and compressed by the
       pool, about 8x;
    3. incremental: chunks keyed by content, so a watch's save after an
       edit re-encodes only the segments the edit touched.
  - **Memory:** the load's peak at most the clean build's RSS (today
    0.9-2.0 GB at -j8).

## 7. A partial or corrupt file

- **Written whole:**
  - to `graph.tmp.<pid>`, then fsync, then rename. A crash leaves the
    old file or the new one, never a mix.
  - Two processes building one document both write, and the rename makes
    one of them win whole. The loser's graph is just older; its key
    still matches its own state.
- **Checked whole:** the header holds the total length and a 64-bit
  hash per chunk (`hash_bytes`). A short file, a bad hash, an unknown
  version, a value index out of range, a node reference past the table
  or a list part after its list all fail the decode: `FormatEnv::read`
  already refuses that last case.
- **All or nothing:** decoding ends before installing begins (§4.2), so
  a refused file has touched nothing. The build is then a clean build,
  which is always correct. The file is deleted and the refusal named in
  `-v` output.
- **A panic during install or the first evaluation** takes the existing
  path: `Session::run` drops the build and the next build is clean.
  That panic is a red in the gate, not a mitigation: it is reported and
  root-fixed.

## 8. Mergeless

Nothing serial is added, and no global lock or merge thread:

- **Save:** each segment's nodes are encoded by one task under that
  segment's read lock. The value table is per chunk, with a chunk
  referring to values in earlier chunks by number. The chunks are
  concatenated. The save runs at a quiet point, the end of an evaluation,
  when nothing commits.
- **Load:** values are decoded in height order, with strings and other
  leaves first, all in parallel: the store is sharded (`Values`'s 1024
  shards) and interning is concurrent as at runs. Each segment's records
  are filed by one task under that segment's write lock only. The field
  indexes are sharded and take concurrent inserts as commits do today.
- **The first evaluation** is an ordinary evaluation with dirty nodes,
  partitioned by position like any other.

## 9. Measured feasibility

Probe: `PHITEX_PERSIST_PROBE=<file>` on `phitex build` (CLI `modern.rs`;
`Session::persist_probe`, `Build::persist_probe`,
`Engine::persist_probe`, crate `defuse` module `persist_probe`). It is
observe-only: it runs after the PDF and the records are written, and it
reads under the locks a quiet line's readers take.

The probe:

- times three in-process builds with nothing changed;
- encodes the nodes, records (last and prior), order, paths, memo,
  environment and client roots (each value once, a sequence by its rope
  nodes);
- decodes the values into a fresh `Values`, and the records with their
  reads coded as a commit codes them (`Reads::new`);
- compares the decoded records with the engine's (the round trip);
- checks every clean node's reads with `read_holds`.

Setup: acclnode11, binary, cache and documents on local `/tmp`,
`--shelf --exact-pdf --no-synctex -j 8`. The cache holds two earlier
builds of the document, so this is a second build.

**Table 1.** The graph after a second build at -j8 (one build each; a
second probe build was within 3 %).

| | amsldoc | acm | Grail | para4000 |
|---|---:|---:|---:|---:|
| nodes live / inert / gone | 1351 / 338 / 159-1179 | 623 / 258 / 40-90 | 1256 / 663 / 101-321 | 6890 / 0 / 0 |
| prior records | 302-401 | 156-199 | 368-412 | 2-8 |
| reads | 568-605 k | 355-358 k | 1.75-2.01 M | 1.08 M |
| of them `Me` / `Before` / fold / final / env | 504 k / 1.2 k / 30 k / 25 k / 2.4 k | 291 k / 2.1 k / 15 k / 29 k / 3.4 k | 1.34 M / 322 k / 36 k / 28 k / 3.8 k | 770 k / 12 / 145 k / 94 k / 8.6 k |
| defs / contribs | 44.6 k / 9.0 k | 37.1 k / 4.5 k | 100 k / 9.0 k | 35.7 k / 48.7 k |
| distinct values (strings) | 575 k (363 k, 11.5 MB) | 528 k (373 k, 18.5 MB) | 900 k (546 k, 13.1 MB) | 827 k (332 k, 11.7 MB) |
| environment fields held | 24.5 k | 35.0 k | 150 k | 15.4 k |
| bytes, whole | 25.3 MB | 29.9 MB | 49.7 MB | 40.7 MB |
| values / reads / records / nodes+paths+env | 19.9 / 3.7 / 0.39 / 0.22 | 26.1 / 2.3 / 0.30 / 0.23 | 33.4 / 13.4 / 0.82 / 0.84 | 32.2 / 6.7 / 0.49 / 0.24 |
| values beyond the environment's (MB) | 7.9 (strings 1.0) | 6.5 (0.56) | 21.4 (2.7) | 23.1 (4.0) |
| bytes beyond the environment's (est.) | 12.2 MB | 9.4 MB | 36.5 MB | 30.6 MB |
| zstd -1 / -3, whole | 10.4 / 9.3 MB | 14.6 / 13.5 MB | 17.7 / 15.2 MB | 17.2 / 14.9 MB |
| the engine's own read coding (`Reads::bytes`) | 4.9 MB | 2.9 MB | 14.8 MB | 9.5 MB |
| PDF | 0.50 MB | 1.08 MB | 0.31 MB | 1.93 MB |

At -j1 the graph is the same size within 3-5 % (amsldoc 24.5 MB, Grail
49.8 MB). It has fewer gone nodes and priors (6 gone and 31 priors on
amsldoc).

The client's roots are 336-401 k values (the format's values made for
the job, `fmt_made`, and the pages). Only 0.03-1.5 k of those values are
not reached from the records as well.

**Table 2.** Times (ms, one thread unless said otherwise; the range of 2-3
builds).

| | amsldoc | acm | Grail | para4000 |
|---|---:|---:|---:|---:|
| second build today, -j8 wall (s) | 1.90-2.06 | 2.05-2.26 | 7.08-7.40 | 0.65-0.69 |
| in-process edit, nothing changed (runs) | 0.07-0.25 (0) | 0.05-0.18 (0) | 0.15-0.33 (0) | 0.27-0.51 (0) |
| encode | 113-138 | 118-124 | 251-264 | 203-231 |
| zstd -1 compress / decompress (CLI, a file) | 48 / 22 | 57 / 25 | 91 / 40 | 133 / 40 |
| decode values into a fresh store | 82-85 | 91-94 | 130-138 | 124-138 |
| decode records (reads coded) | 22-23 | 15 | 71-83 | 31-33 |
| check every clean read (`read_holds`) | 82-84 | 57-59 | 722-768 | 103-104 |
| failed reads in that check | 0 | 0 | 0 | 0 |
| records that differ after the round trip | 0 | 0 | 0 | 0 |
| keys of the document's inputs (stat) | 0.03 | 0.06 | 0.09 | 0.00 |

A one-line `article` document's second build takes 0.24-0.25 s
(process, format, Shelf index, a tiny evaluation): the fixed part a
loaded build keeps.

Filing (§4.3) was not measured. Its work is one hash insertion per
definition, contribution, anchored read and fold read, and per distinct
field read per segment: about 0.2 M insertions on amsldoc and 0.8 M on
Grail. At 50-100 ns each that is 10-80 ms on one thread, spread over
the segments in parallel.

**Expected second build, unchanged, -j8.** The fixed 0.24 s, plus
reading and decompressing (25-45 ms), the values (82-138 ms on one
thread, about 25-40 ms on 8), the records (15-80, about 5-20 on 8),
filing (10-80, about 5-20 on 8), the inputs' keys (under 1 ms) and the
no-change edit (under 1 ms):

| | today | one-thread load | parallel load |
|---|---:|---:|---:|
| amsldoc | 1.90-2.06 s | ~0.39 s | ~0.30 s |
| acm | 2.05-2.26 s | ~0.39 s | ~0.30 s |
| Grail | 7.08-7.40 s | ~0.56 s | ~0.35 s |
| para4000 | 0.65-0.69 s | ~0.47 s | ~0.33 s |

With an edit between the processes, add the watch's time for that edit
(ms to a few hundred ms). A clean build gains nothing and pays the save
(§6).

## 10. Risks

1. **State outside the engine left out**, which would give a wrong PDF.
   It is closed by the inventory rule (§2: the collection's roots), by
   dropping every cache, and by the gate's watch-equals-cold mode (§5.4)
   over the corpus at -j1 and -j8. Process-global tables that hold no
   values and are not caches would escape the roots list, so each
   one is audited in step 2: the font, map and file memos,
   `assemble::linked_files` and `pic::REFUSED`/`MANY`.
2. **Two copies of the filing logic** would drift. The commit's filing
   is split out first (step 1), and the load calls it.
3. **The save's cost on clean builds** (§6): parallel and incremental
   from the start, with the bar in the gate's timing series.
4. **The finder's resolutions** (shadowing, a file appearing on the
   search path): the absent queries are kept and re-resolved (§3.2).
5. **The value decode dominates the load.** Parallel decoding by height
   is the plan. Lazy decoding of reads' seen values (made only when a
   read is checked) is the fallback if the bar is missed. The engine
   compares ids, so a lazy id would need a store hook, which is not
   planned.
6. **Op and projection ids** are mapped by name. A missing name refuses
   the load, which cannot happen within one `engine_id`.
7. **Graph shape differs between -j8 runs** (gone, inert and prior
   counts vary). Each saved graph is still quiet and consistent; the
   output is the same.

## 11. Steps, each green

| step | what | size | the gate that proves it |
|---|---|---|---|
| 0 (done) | the observe-only probe; this spec | 1 000 lines | n11 measurements (§9) |
| 1 | `defuse`: the commit's filing half split out (`commit::file_record`); `Engine::export` / `import` (§2.1, §4.3), parallel per segment; ops and projections by name | ~1 500 | the defuse suite, plus round-trip tests at -j1 and -j8 on the suite's graphs (claims, scopes, relations, finals, continuations): export, then import into a fresh engine, then `check_invariants` and every read holds, then `evaluate` runs 0 nodes; plus an edit after import runs what the same edit runs in the original engine |
| 2 | `tex` and `live`: the client's tables (§2.2), `Build::save` / `Build::load` in process, the global tables audited (risk 1) | ~1 500 | tex suites, plus in-process round trips on ~10 corpus documents on mc (build, save, load into a new `Build`, then: no-change edit with 0 runs and the same PDF and log bytes; each scripted edit with the same runs and bytes as the in-process edit) |
| 3 | the cache file: header and keys (§3.1), manifest and environment re-ask (§3.2-3.3), atomic write and checks (§7); CLI `build`, `watch` and the editor load it behind `PHITEX_GRAPH=1` | ~1 000 | corpus gate in the second-process mode (§5.4) at -j1 and -j8, byte-identical, plus the file fuzz tests |
| 4 | speed: parallel value decode, per-segment chunks, incremental save, no save when unchanged | ~800 | n11 timing series (interleaved, local disk, 5 each): the §6 bars; the step 3 gate still green |
| 5 | on by default; the gate keeps the second-process mode for every document | small | full corpus gate; nightly |

Steps 1 and 2 change no output and load nothing across processes. Step
3's switch is off by default until its gate is green.

## 12. Step 2: what was built and measured

- `Build::save` / `Build::load` (crates/engines/tex/src/api/persist.rs):
  the client's tables (§2.2) beside the engine's graph, LEB128 with a
  mark per section, a hash over the client's bytes; `PHITEX_GRAPH_VERIFY=1`
  asks every read of the loaded graph (`Engine::verify_reads`), one
  failing an error. A refused load names the part.
- `defuse`: the import puts each value at its place (§2.2, "Values keep
  their places"): `Values::stable_raw`, `from_stable`, `place_plain`;
  a sequence's id hashes its elements' stable ids. Version 2 of the
  engine's bytes. Test: `an_import_keeps_every_values_place`.
- `phitex-live`: `PHITEX_GRAPH_CHECK=1` on a session (`build`, `watch`):
  after each build the build is saved, loaded into a twin in the process
  and evaluated with nothing changed (no node may run; PDF, log and
  written files the build's); the next change is given to the build and
  to the twin, and their runs, runs by operation and outputs compared;
  one `graph-check:` line each on stderr. The gate's mode:
  `scripts/accl/gate-fast persist [COMMIT] [N] [J]` (gate-shard.sh
  `persist`: ci/research.json under `editsweep.py --threads J
  --graph-check`; red for a refused or changed load, an edit whose
  twin ran otherwise at -j1, or one whose twin's output differs at any
  -j; nothing carved out since step 2b, §2.2.4). At more threads the
  runs are reported, not compared, and the sweep's two-runs-a-node bar
  reported apart: both are the schedule's, the outputs identical
  (§2.2.4, the two -j8 report rules).

Measured on acclnode11 (local disk, release, `--shelf --exact-pdf`,
the check's own lines; ms, one thread for save and load):

| document | -j | bytes (engine + client) | save | load (import + client) | unchanged: runs | edits after a load |
|---|---:|---:|---:|---:|---:|---|
| amsldoc | 8 | 29.4 MB (28.1 + 1.3) | 131-157 | 200-235 (164-180 + 23-54) | 0 (1.4-1.7 ms) | PDF differs (44 pages, 694 commands not saved) |
| amsldoc | 1 | 28.2-28.6 MB | 114-121 | 235-254 (152-165 + 82-86) | 0 | same, runs equal but where the link state moved them |
| acm (sample-sigconf) | 8 | 35.1 MB (32.1 + 3.0) | 115 | 190 (153 + 35) | 0 (1.0 ms) | (6 pages, 6017 commands not saved) |
| Grail (XeLaTeX, TikZ) | 8 | 61.4-61.5 MB (55.2 + 6.3) | 320-326 | 408-422 (342-352 + 60-67) | 0 (1.1-1.4 ms) | **exact**: same nodes, runs, PDF, log, files (3 of 3) |
| Grail | 1 | | | | 0 | **exact** (3 of 3) |
| para4000 (plain) | 8 | 54.0-54.1 MB (45.6 + 8.4-8.5) | 209-230 | 279-299 (260-278 + 15-18) | 0 (1.2-1.9 ms) | PDF differs (692 pages not saved) |
| a `\write18` document (--shell-escape) | 1, 8 | 11.0 MB | 22-23 | 46-50 | 0 (0.3-0.4 ms) | PDF differs (1 page not saved) |

Every unchanged load ran 0 nodes and gave the build's PDF, log and
written files, at -j1 and -j8, after the cold build and after every
scripted edit (amsldoc's every edit kind, three word edits of the
others). The client's analysis (the document's and the cut files'
`Incr`, made again) is most of the client's load time (23-86 ms).

## 13. Step 3: what was built and measured

**The file** (crates/editor/live/src/graph.rs): `docs/ID/graph` (beside
`noderec`, a job named otherwise with its own), written to a temporary
file, synced and renamed (`Store::put_durable`, the CLI's
`Cache::put_synced`). Head: magic, version, the payload's length, 1 MiB
chunks each hashed (`tex::words_hash`: a one-word difference, a flipped
bit, always changes it); payload: the keys by name, the format's key, the
manifest, the build (`Build::save`). A short or long file, a hash wrong,
a part out of range: refused before anything is decoded, the file
removed, a clean build. Unit test `a_cut_or_flipped_graph_file_is_refused`
cuts it at every chunk bound and flips every bit of its head and hashes
and a spread of its payload's.

**Keys** (`Session::graph_key`, taken at the session's first build and
kept for its save): the engine (`engine_id`), the file's version, the
files' release (`Host::release`: Shelf's release; TeX Live's trees), the
engine's name and output, `-ini`, the main file and the output directory
(canonical), the job name, `--spec`, wait-long, node records, SyncTeX,
origins, shell escape, the fixpoint, the job's search paths, the live
layout, every variable of `graph::KEY_VARS` (`SOURCE_DATE_EPOCH`,
`FORCE_SOURCE_DATE`, `TEXINPUTS`, `TEXMFHOME`, `BIBINPUTS`, `BSTINPUTS`,
`TEXMF_OUTPUT_DIRECTORY`, and the engine's switches that change a
product or the graph's shape), XeTeX's font index; then the format's key
against the store's format before decoding and the build's after. Not
`-j`. A refusal names the key (`-v`: "graph: not loaded: its key
`$SOURCE_DATE_EPOCH` differs").

**The manifest**: the main text the build is for, every name the builds
asked the finder for (found or not, where, whether the document's), the
stat keys taken when each was read, and what XeTeX's fonts and PDF writer
looked up and read through the host outside the environment
(`graph::KeyedFonts`, `keyed_dpx`: each lookup's answer, each file's key
when read).

**A load** (`Session::load_graph`, the session's first build, before
anything else is read):
1. keys; the format's key;
2. the files read outside the environment: a font file of the engine's
   (its runs shaped with it, unrecorded) moved or found elsewhere refuses
   the load; one of the PDF writer's (a picture XeTeX embeds) makes the
   output file again (`Build::relink`);
3. `Build::load`; the session's tables from the manifest;
4. every name resolved again (`Host::relocate`: Shelf's from its index,
   nothing fetched) and every file found keyed again: a name found
   elsewhere now (a document's `.sty` shadowing the distribution's), one
   found now and not then (but where the job wrote it), a stat key moved:
   staged, as a watch stages a changed file (its content compared in
   place: the same bytes, no change);
5. `Build::reask_environment`: the job's bindings made again from the
   options (the date where `SOURCE_DATE_EPOCH` gives none; XeTeX's PDF
   date is now the job's `epoch` binding, read by the output node, so a
   moved date links again); every answer the provider made of something
   besides the format and the code (`Graph::outside_answer`: files, a
   tool's files, a cut file's lines) and not keyed by the manifest asked
   of it again (`Engine::reask_provided`, the provider's fields marked as
   such in the environment, exported: the engine's bytes version 4), what
   moved re-asking its readers (`Cause::Environment`);
6. the first build is then an edit of the main text against the saved
   one. Nothing moved and no node run: the file is kept (no save), and
   outputs already on disk with the same bytes are left as they are
   (`Sink::keep_same`).

**Saved since step 2**: the pages' glyph origins (`origins::save_kept`:
what each page's writer showed, its pieces, its objects' codes; the
client's bytes version 3). A page's writer runs again only when the page
changes, so a loaded build's SyncTeX file held only the edited pages
(amsldoc: 1 481 lines of 22 566) until they were saved.

**The save** (`Session::save_graph`): after the outputs are written and
the report said. A synchronous save cost para4000 +30 % of its clean
build at -j8 (encoding 220 ms, writing 58 MB 125 ms), so `phitex build`
keeping its graph runs the build in a worker (this program again, from
the folder it was started in, `PHITEX_GRAPH_DONE`), which writes its exit status for the process
waiting on it once everything is said, then saves holding the file's
lock (`Store::lock`, the CLI's `graph.lock`), which the next load waits
for. `PHITEX_GRAPH=sync` saves in process. A watch saves when it ends.

**Gate**: `scripts/accl/gate-fast persist COMMIT N J process`
(`gate-shard.sh persist-process`, `scripts/ci/graphsweep.py`): per
research document at -j J, `phitex build` processes in one folder and
cache: clean (saved); unchanged (loaded, 0 nodes); three words typed; a
`.bib` title edited; a picture replaced; the document class shadowed by
a copy in the folder (it says so in the log); the other -j unchanged;
again; the file cut, then a bit flipped (refused); `SOURCE_DATE_EPOCH`
changed (refused by its key). PDF, log, aux and SyncTeX byte-identical to
a clean build of the same files in the same folder.


## 14. Step 3 status (2026-11-16)

Merged to dev behind `PHITEX_GRAPH` (cf366ab07), then on by default
(§15). The sweep at 886f454f3: gate-fast mc 88086; second process -j1
69/69 (mc 88087-91), -j8 68/69 (mc 88092-95): revtex-aps-xe red, #45.

**#45, fixed.** revtex-aps-xe at -j8, the step editing the `.bib` after a
load: about 1 in 15 gate runs (1 in 90 histories of the sweep's own),
the PDF the same wrong one each time (page 6 with the bibliography over
it), two `\bibcite` lines after page 6's writes in the `.aux`, the
loaded build 140-160 nodes in 2 rounds where a good one ran 14.
- Not the load alone, not the -j8 `.bib` edit alone: a -j8 watch with no
  graph making the same edits was right 320 of 320; graph histories kept
  the bad state in about 1 in 90, and a kept state re-ran red 24 of 24.
- The cause, in the engine: a placement matched to its placer's last
  node by (op, args) and kept where it stands (after the new node's
  children, nothing of a later slot between) kept the anchor, slot and
  continuation flag it was placed with first. Page 6's finisher, placed
  after the bibliography's piece 279 and kept after its first piece 229,
  stayed anchored at 279; the `.bib` edit re-ran 229, `stranded` found
  the finisher foreign between 229 and its continuation, the
  continuation was made anew, and the pages after it ran again from a
  wrong start. Kept anchors that differ from the placement's are common
  (finishers at every few builds, in a watch too); a later move of the
  anchor or a re-run of its neighbour exposes them.
- Fix (commit.rs, the keep): a kept node takes its placement's anchor,
  slot, rank and continuation flag. Tests `ranked::a_node_kept_after_another_node_moves_with_it`
  and `ranked::a_node_kept_after_an_earlier_node_stays_with_it`, red
  before; `Engine::check_invariants` checks that a placed node stands
  after its anchor.
- Proved: gate-fast green (mc 88223); second-process sweep 69/69 at -j1
  (mc 88225-28) and at -j8 twice (mc 88229-32, 88233-36); 212 histories
  of revtex-aps-xe's sweep at -j8, every `.bib` step 14 nodes and right.

**Found and filed, not graph bugs:**
- #43: a second `phitex build` in a used folder logs differently. The seeded `.aux` is read per field, and the nested part `.aux` reads are dropped. Seen on dev. The sweep's refused steps now compare against a clean build of the same folder, as its doc always said.
- #44: `defer::a_name_built_from_a_stepped_counter_reads_through_the_name` fails under load. It does so on dev too.
- #33 (pictures test) is already known.

**Fixed this round:**
- Glyph codes are saved as deltas (client v4): para4000 went from 114 MB to 98 MB. Every `phitex build` makes origins, because the view is on by default.
- A moved input's place is now taken into the session's table at load. Before, every later load found the shadowing class "moved" again.
- The harness keeps the history's own terminal output.

**Timings**, n11 local disk, fastdev binary of 886f454f3, medians of 5, seconds.
- Columns:
  - A: clean build.
  - A2: today's second build, no graph.
  - B: clean build that saves the graph asynchronously.
  - S: clean build that saves the graph synchronously.
  - C: unchanged build with the graph loaded.
  - D: one-word edit with the graph loaded.
- The Grail D column is a word in `evaluation.tex`, which changes the PDF. The word in the main file did not change it.

| doc | -j | A | A2 | B | S | C | D | sync save | graph |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| amsldoc | 1 | 7.86 | 1.49 | 7.87 | 8.05 | 0.42 | 0.47 | 195 ms | 29.4 MB |
| amsldoc | 8 | 8.17 | 1.78 | 8.15 | 8.50 | 0.36 | 0.40 | 210-243 ms | 30.1 MB |
| acm | 1 | 8.67 | 1.77 | 8.63 | 8.86 | 0.40 | 0.50 | 203 ms | 35.9 MB |
| acm | 8 | 8.49 | 1.78 | 8.45 | 8.54 | 0.39 | 0.49 | 211 ms | 36.2 MB |
| Grail | 1 | 21.16 | 8.47 | 21.05 | 21.63 | 0.83 | 1.01 | 476 ms | 61.2 MB |
| Grail | 8 | 19.04 | 7.61 | 18.84 | 19.07 | 0.83 | 1.02 | 489 ms | 63.2 MB |
| para4000 | 1 | 2.40 | 1.77 | 2.43 | 2.90 | 0.65 | 0.62 | 502 ms | 98.2 MB |
| para4000 | 8 | 1.32 | 0.69 | 1.36 | 1.84 | 0.61 | 0.58 | 506 ms | 98.3 MB |

The asynchronous save costs a clean build at most +3% wall (para4000 at -j8), within the +5% bar. The synchronous save costs up to +39%, which is why the save is asynchronous.

## 15. Step 5: on by default (2026-11-16)

`phitex build` and `phitex watch` (`--lsp` too: VS Code's "phitex watch"
engine) keep the document's graph in the cache and start from the last
one saved. `graph` in `phitex.toml` (on; `phitex config graph off`, a
project's too) and `PHITEX_GRAPH` over it (`0` off, `1` on, `sync` saved
before the build ends). With no cache (`cache-limit = 0`) none is kept.
`-v` says at the first build (a watch's too) whether the saved graph was
loaded, or why not (`graph: not loaded: none saved`, `... its key
`$SOURCE_DATE_EPOCH` differs`, `... chunk 10's hash is wrong`).

The worker of an asynchronous save starts from the folder `phitex`
started in (the target resolved, the process stands in the build's
folder) and before anything is said (the target's warnings were said by
both).

The CI's runs take the default: the second-process sweep's histories
run with no variable (its clean references `PHITEX_GRAPH=0`), every
`phitex build` of the gate's and the corpus's saves its graph, and a CLI
test's second build loads it (`build_rounds_are_latexmks_runs`: no round
loaded, one with `PHITEX_GRAPH=0`). Green at df332b4f4: gate-fast mc
88278; the default path's sweep (8cbca4c83) 69/69 at -j1 (mc 88256-59)
and -j8 (mc 88252-55).

**Timings**, n11 local disk, release binary of 8cbca4c83, medians of 5
(the first of 6 dropped), seconds. A: a clean build, `PHITEX_GRAPH=0`;
A2: its second build, no graph; B: a clean build by default (the save
asynchronous); S: `PHITEX_GRAPH=sync`; C: an unchanged second build, the
graph loaded; D: a one-word edit, loaded.

| doc | -j | A | A2 | B | B/A | S | C | D |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| amsldoc | 1 | 1.74 | 1.42 | 1.79 | +2.9% | 1.93 | 0.42 | 0.46 |
| amsldoc | 8 | 2.19 | 1.76 | 2.14 | -2.3% | 2.30 | 0.36 | 0.40 |
| acm | 1 | 2.50 | 1.71 | 2.51 | +0.4% | 2.70 | 0.39 | 0.49 |
| acm | 8 | 2.20 | 1.73 | 2.30 | +4.5% | 2.51 | 0.38 | 0.48 |
| Grail | 1 | 11.53 | 7.98 | 11.58 | +0.4% | 12.05 | 0.81 | 0.81 |
| Grail | 8 | 9.59 | 7.09 | 9.64 | +0.5% | 10.06 | 0.82 | 0.82 |
| para4000 | 1 | 1.69 | 1.68 | 1.73 | +2.4% | 2.17 | 0.62 | 0.65 |
| para4000 | 8 | 0.67 | 0.67 | 0.70 | +4.5% | 1.17 | 0.59 | 0.62 |

The default's clean build is within +5% of no graph at every point;
its second build 3.4-9.9x faster where the document is not one long
plain run (para4000 1.1-2.7x).

## 16. Step 4: the builtin (wasm) engine (2026-11-16)

VS Code's builtin engine, the web demo and the Overleaf extension run
`phitex_live::Session` in the wasm core (crates/wasm/live), so the load,
the keys and the checks are §13's, unchanged. What the core adds is how
the bytes reach the host and when the save runs.

**The core** (crates/wasm/live):
- `ph_graph_give(bytes)` before `ph_open`: the graph the host kept for
  the document (empty: none). The open's JSON says `graph: true` to
  keep graphs (off unless the host says). The session's first build
  takes it from the store's `doc/graph` key (`store::Kept`), which the
  core answers itself: tens of MB are never asked of the host or posted.
  A refused file (cut, a hash wrong) is reported `dropped` and the host
  deletes its copy; a key that differs is a clean build, as natively.
- `{"event": "graph", ...}` before the build's own event: the load's
  line (`loaded`, or why not) and `dropped`; then, when a save is made,
  `saved: N` and its time. The host takes the bytes with `ph_graph()`.
- **The save runs at a pause.** After a build, once nothing has been
  asked for 2 s (`GRAPH_IDLE`), on the session thread; without threads,
  the step answers 3 and the host steps again after the pause.
  `Session::save_graph_until` gives it up at any ask
  (`Engine::export_until`, a check every 32 nodes; `Build::save_until`),
  so an edit typed during a save waits for nothing: the save is made at
  the next pause. With JSPI (Chrome's single-threaded core) the save
  yields to the host as a build does; Firefox's core has no JSPI, so its
  save runs to its end once started (0.1-1.3 s, after 2 s of quiet).
- **Memory.** The core's memory is fixed (`heap.rs`): a save first asks
  for three times the last graph's size, or 64 bytes a value the store
  holds (`Session::graph_bytes_guess`; 44-54 measured), as a block taken
  and given back (`heap::room_for`, its refusal no loss). A save never
  takes what the document needs. The file is encoded in one buffer
  (`graph::encode`) and handed over without a copy
  (`Store::put_durable_owned`): its peak is about twice its size.
- **Keys of the host's files.** A file the host gives is keyed by its
  content (SHA-256 and length, made once at its first ask, off the host's
  thread), so a later session's host giving the same bytes gives the same
  key, and only a file whose bytes differ becomes an edit. Shelf's files
  are keyed by the release's index (the file's own sum, else its pack's
  id), nothing fetched, as the CLI's `--shelf` keys them. A loaded graph's
  inputs are resolved again from what the host gave and Shelf's index,
  nothing fetched (`Files::relocate`); the release key is `Shelf <release>`.
  A save made after another open is never handed over as the new
  document's: each session keeps its own (a thread-local slot), and the
  open's number is checked.

**The hosts** (PhiTeX-frontends, common/src/wasm/graphs.ts, core.ts):
`GraphStore` keeps a graph a document under the host's project name,
raw (no compression: a load decompresses nothing), and drops the least
recently used past a cap. `CoreSession` gives the kept graph before each
open and keeps each saved one.
- VS Code: the workspace's storage (`context.storageUri`, the global one
  without a workspace), a file a document (written aside, renamed in);
  at most 16 documents and 2 GB. Setting `phitex.graph` (on).
- The web demo and the Overleaf extension: IndexedDB (`phitex-graph`,
  the page's or the extension's origin), keyed by project and main file;
  at most 16 projects and 512 MB, the least recently opened dropped
  first. Setting: the demo's Clear cache dialog, the extension's popup
  (Package cache); off drops every graph kept. A Compare's core keeps
  none. Clear cache clears them with the build's state.

**Tests.** Native: `a_save_given_up_stores_nothing` (a save stopped at
each of its asks stores nothing; the one after is whole; the next session
loads it, no node run, the clean build's bytes). In the frontends:
apps/vscode/test/graph.ts (VS Code's Node host, each open on a core of
its own): the first open saves, the second loads and runs 0 nodes with
the same PDF, then an `\input` file, the main file, a figure and a
`.bib` changed while closed, a damaged graph (refused, dropped, saved
again), graphs off; every PDF byte for byte a clean build's.
apps/web/test/graph.mjs (the demo in Chromium) and
test/scenarios/ci-graph.json (the extension on the mock Overleaf): a
reload loads and runs 0 nodes, a change made while closed is built.

**Timings** (VS Code's Node host on the workstation, 4 engine threads, a
fresh core each open, the store's formats and packs warm; seconds from
the open's request to its first build; the core's start, 0.55-0.74 s,
apart):

| doc | graphs off | first open (saves) | second open | second open, a word typed while closed |
|---|---:|---:|---:|---:|
| amsldoc | 10.8-12.3 | 10.5-12.0 | 1.87-2.13 | 2.19-2.86 |
| Grail (XeLaTeX) | 75.6-79.0 | 75.3-80.2 | 3.25-3.34 | 4.03-4.12 |

- The save, in the core at the pause: amsldoc 0.41-0.56 s (28.8-29.0
  MB), Grail 1.14-1.25 s (61.1-61.2 MB). Nothing waits for it.
- The load: amsldoc 1.49-1.78 s (import 0.55-0.69, the client's
  0.82-0.94 of which the analyses 0.88), Grail 2.5-2.6 s (import
  1.15-1.22, the client's 0.38-0.43, XeTeX's font lookups checked again
  most of the rest). At one engine thread amsldoc's client part is 0.42 s
  (the analyses 0.37): the 4-thread pool's first use costs about 0.5 s
  on a fresh core, a follow-up (start the pool with the core).
