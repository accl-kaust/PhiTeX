# The tex suite's reds outside gate-fast (2026-10-30)

Status: a list, as the coordinator asked. These are the 16 tests that
`cargo test -p phitex-tex --no-fail-fast` fails on dev c0cadc834.

- **Run.** mc 76753, fastdev, on one mc node with 32 CPUs. The same 16
  fail on the branch that became dad7f24a3 (76752). That run also had one
  failure of the shell test below, which is intermittent on dev too.
- **Scope.** None of these is in a gate-fast shard.
- **How sure the causes are.** Each cause is read from the test's message
  and its source, not traced. "Real" means the engine does what the test
  says it must not. "Stale" means the assertion describes a design the
  engine has since left. "Environment" means the test cannot pass on the
  machine it ran on.

| Test (file) | What failed | Cause | Kind |
|---|---|---|---|
| `one_loop_node_and_one_finisher_per_page` (pages2) | "-j1: one page-builder node", 7 against 1 | Since cf684671c a loop's rests are nodes of their own: one per page that waits for a give-back. | stale |
| `the_loop_parks_at_holes_and_resumes` (pages2) | "the loop ran 34 times for 5 pages" | The bound counts the runs of one loop node; the runs are now spread over the rests and their resumptions. | stale, bound to restate |
| `a_paragraph_insert_refires_the_pages_after_it_once` (pages2) | "the loop once", 5 pages runs against 1 | The same: each rest after the insert runs once. | stale, bound to restate |
| `a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher` (cuts) | finishers made anew 13 against at most 1 | An insert that moves a page fire remakes the finishers after it instead of keeping them by key. That costs edits. | real |
| `chains` (chain) | "target graph: 4 failures": 53 runs for 36 nodes, above the 1.1 bound; oracle fraction 0.0278 against the 0.0004-0.003 bar | A cold build's reruns, and the engine's critical chain, which reports 1 link (`critical_chain_heights` is broken; docs/note-scaling.md). | real (bars red) |
| `census_plain` (common, via chain) | para400 runs a node 1.27 and 1.23 cold; para400pdf cold critical path 134 | Cold reruns above the census's bars. | real (bars red) |
| `independent_loads_run_side_by_side` (pkgrec) | second build: recorded 0, want 2 | The exported package records are not matched on the next build: no load is taken from its record. | real (records not applied) |
| `dependent_loads_keep_their_order` (pkgrec) | 0 against 2 | Same cause. | real |
| `changed_file_falls_back` (pkgrec) | 0 against 1 | Same cause. | real |
| `defining_outside_the_record_is_refuted` (pkgrec) | 0 against 2 | Same cause. | real |
| `prediction_is_right_when_nothing_changed` (pkgrec) | predicted 0, want 2 | Same cause (the predictions come from the records). | real |
| `prediction_is_wrong_when_an_earlier_load_changed` (pkgrec) | 0 against 1 | Same cause. | real |
| `latex_aux_on_disk_outside_a_fixpoint` (latex) | "stuck: node 2 (resume): the process holds 4.2 GB, past the limit of 4.0 GB", at expl3-code.tex:35568, 1.58M commands | The memory limit is the whole test process's, and the binary runs its tests in parallel. Whether this build alone passes 4 GB is not known yet: run it alone to decide. | to decide |
| `engine_failures_stay_errors` (tikzman_repros) | wants an error; gets "Label(s) may have changed: … still changed after 11 rounds" | The read-back of main.aux does not converge (`wdef:main.aux cs:ref`, `wstreams:61`), so the build ends at the round cap with a warning. | real |
| `ini_bidi` (xetex) | "! Font not loadable", 1424 bytes against 1900 | The fonts the test's xetex ini format loads are not on the mc nodes. | environment |
| `ini_features` (xetex) | 732 bytes against 4304 | Same cause. | environment |

Intermittent, outside this list:

- **`a_command_reads_what_the_job_wrote_and_its_output_is_read_after_it`
  (shell).** At -j8 a fakecmd runs once more, on empty input: the
  command's node runs before the write it reads has landed, and its
  output stays counted.
  - On dev c0cadc834 it failed 12 of 50 (mc 76762).
  - On dad7f24a3 it failed 14 of 50 (mc 76761).
  - The same rate, so the lines change did not cause it.
  - The failing runs' outputs are kept in those jobs' directories
    (`fail-<i>.txt`). Issue #3.
- **The defuse review tests `a_next_past_many_holes_goes_on_from_each` and
  `next_readers_follow_their_stops_across_splits`.** RunawayNode on every
  run since at least 739a81d73 (76710). Those are mine to fix next.

Found while fixing the pkgrec reds (2026-10-30):

- **Load records let nothing run early on this core.** The mergeless core
  runs a reader past a package load optimistically, with or without the
  load's record, and re-asks it by what the load defines. So a record's
  definition and read sets no longer change what runs when. Their only use
  left is the values they carry, the predictions under
  `Options::pkg_predict`. That is a parallelism question, not a defect.
