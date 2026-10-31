# METAFONT in PhiTeX: state of the port (paused 2026-10-28)

The goal: fonts that exist only as METAFONT sources made in process, in
pure Rust, as TeX Live's kpathsea makes them with mktextfm/mktexpk
(`mf` then `gftopk`), byte-identical with TeX Live, deterministic, at
every thread count, building for wasm32-wasip1-threads. Six corpus
manuals need it: chemformula-ru, disser, russ, serbian-def-cyr (T2A LH
TFMs such as larm1000, from the lh .mf sources; glyphs come from
cm-super Type 1 through the map, so TFM only) and crbox, ghab
(`\font\font@corner=ghab`: ghab.tfm by mktextfm, and at shipout
ghab.600pk by mktexpk, which pdfTeX embeds as a Type 3 font: an extra
/F41 in the oracle that shifts every later font number by one).

The work was paused at the user's limit of two agents. This note says
what exists, what is left, and how to resume.

## The plan

1. `crates/fonts/mf` (package `phitex-mf`): mf.web 2.71828182 with
   web2c's mf.ch, ported section by section, TeX Live's semantics and
   limits; it must pass Knuth's trap test as `mftraptest.test` judges it
   (triptrap/: mftrap1.in, mftrap2.in, trap.mf; expected mftrapin.log,
   mftrap.fot, mftrap.log, mftrap.pl via tftopl, trap.typ via gftype -m -i;
   mftrap.diffs shows the accepted differences, the test's sed filter
   hides dates, version strings, capsule numbers and usage statistics).
   Trap settings (triptrap/texmf.cnf): error_line 64, half_error_line
   32, max_print_line 72, main_memory 3001 (mem_top 3000), buf_size 500,
   gf_buf_size 8, screen_width 100, screen_depth 200, MFTERM=trap (the
   screen calls are logged as window/trap.c does), run as
   `mf --progname=inimf` (ini mode, dump name inimf).
2. gftopk (gftopk.web + gftopk.ch) as a library `gf_to_pk(&[u8])`.
3. The mktextfm/mktexpk logic (TeX Live's scripts, mktex.opt,
   mktex.cnf): mode ljfour and 600 dpi unless configured, mag from the
   dpi, the `\mode:=ljfour; mag:=...; nonstopmode; input name` line, the
   base made by INIMF from mf.ini (`\input plain \input modes \dump`).
4. Integration: where crates/tools/kpse and the engine's finder miss a
   .tfm or .pk and find the font's .mf, generate in process, cache in
   ~/.cache/phitex keyed by the hash of every .mf the run read plus mode
   and dpi. A missing TFM today gives "Font not loadable" in control.rs
   read_font_info (another agent is making that pdfTeX's full error
   text); our ported texmf.cnf's TFMFONTS already searches
   {$TEXMF/fonts,$VARTEXFONTS}/tfm//.
5. PDF: a PK reader and a port of pdfTeX's writet3.c (Type 3 fonts from
   PK, exact bytes) into crates/output/pdf; today font.rs returns
   `Unsupported("PK (Type 3) font ...")`.

Order (coordinator): mf with trap first, then the TFMs for the four
Cyrillic manuals, then gftopk + PK + Type 3 for crbox and ghab.

## What exists (branch `mf`)

`crates/fonts/mf/src`, about 7,000 lines, **not yet compiling** (the
crate is listed under `exclude` in the workspace Cargo.toml until it
does; remove that line when it builds). Ported, section numbers of
mf.web:

| file | parts | sections |
|---|---|---|
| lib.rs | globals, `Word`, field accessors, `Config`, `Io` | (§13 ff. globals as far as part 47, some guessed ahead: check them as the parts are read) |
| consts.rs | every `@d` constant of parts 1 to 20 | |
| print.rs | 3 to 6, §99, §103-104, §195-197 | input_ln (web2c's `input_line`), t_open_in (`topenin`), init_terminal, the string pool, printing, error (with the interaction menu), fatal_error, overflow, confusion, check_arith |
| arith.rs | 7, 8 | Hobby's floating-point make/take_fraction/scaled of lib/texmfmp.c (web2c builds without FIXPT), cpascal.h's half/halfp/floor/round macros, square_rt, pyth_add/sub, m_log, m_exp, n_arg, n_sin_cos, randoms |
| memory.rs | 10, 11 | get_avail, get_node, free_node, sort_avail, flush_list |
| tokens.rs | 14 to 16 | token lists, variables (find_variable etc.), save stack |
| paths.rs | 17, 18 | print_path, copy_path, make_choices, solve_choices, curl_ratio, set_controls |
| moves.rs | 19 | make_moves, smooth_moves |
| edges.rs | 20 | edge structures, line_edges, move_to_edges, cull/merge/xy_swap |
| octants.rs | 21 | skew, crossing_point, make_spec, quadrant/octant subdivision, xy_round, diag_round, new_boundary |
| mf.pool | | TeX Live's string pool, `tangle mf.web mf.ch` in the accl image (job 21878: 828 strings, check sum 118588231); `get_strings_started` loads it as mf-binpool.ch's `loadpoolstrings` does, so string numbers and usage equal TeX Live's |

Design decisions (keep them):

- One struct `Mf` with mf.web's globals; procedures are methods named
  as in mf.web, `// §n` marking each section.
- `Word` is web2c's little-endian `memory_word` with 32-bit halfwords
  (texmfmem.h): `lh` then `rh`; `int`/`sc` is `rh`; `hh.b0`/`hh.b1` are
  the high/low shorts of `lh`. Field accessors by the `fields!` macro
  (`link`/`set_link`, `info`, `type_`, `name_type`, `value`, knots,
  edge headers ...).
- `jump_out` (fatal error, overflow, 100 errors, `X`) returns
  `Err(Stop)` after `close_files_and_terminate`; every procedure that
  can reach it returns `R<T>` and callers use `?` (no unwinding: panics
  abort on wasm).
- String constants that mf.web only prints are `&str` (`prints`,
  `print_nl`, `print_err`, `help(&[...])`); a constant mf.web stores as a
  string number must use its pool number (the pool is TeX Live's, so
  "???" is 259). `primitive` must take the pool string so that its
  flush_string behaviour (§210) matches.
- Files are bytes through the `Io` trait (`open_in(name, kind)` returns
  the bytes and the found name, `./` stripped as open_input does);
  outputs collect in `Mf::outputs`; the terminal is `Config::
  terminal_input` in and `Mf::term_out` out.
- Date and time come from `Config::date` (for byte-identity runs use
  SOURCE_DATE_EPOCH with FORCE_SOURCE_DATE=1 on TeX Live's side).

`crates/fonts/mf/webcode.awk` prints only the code parts of mf.web with
section numbers (`awk -f webcode.awk upstream/texlive-source/texk/
web2c/mf.web > mf.code`, 15,334 lines); the parts' line numbers in it
were the reading index.

## What is left

- mf.web parts 12-13's code: `print_type`, `print_op`, `id_lookup`,
  `primitive`, `init_prim` (all `primitive` calls, spread over the
  program) and `print_cmd_mod` (its cases are spread too): planned in
  hash.rs.
- Parts 22 to 49 (§460-§1211): filling a contour, polygonal pens,
  envelopes, elliptical pens, direction and intersection times, online
  graphics (the trap screen), dynamic linear and nonlinear equations,
  input stacks, get_next, macros, expansion, conditionals, loops, file
  names (with mf.ch's quoting, area/ext rules, make_name_string resetting
  cur_name), the parser and every operation, statements and commands,
  TFM output (mf.ch: put_2_bytes/put_4_bytes), GF output (half buffers,
  gf_buf_size), dump/undump (own format is fine: it only round-trips,
  but mf.ch dumps xord/xchr/xprn and checks the engine name), the main
  program (mf.ch's setup_bound_var, const_chk, ini_version), close_files
  _and_terminate. Modules named in lib.rs but not written: dump, envelope,
  exprs, files, gf, hash, input, linear, ops, pens, scan, screen,
  statements, tfm.
- A test binary or test that runs trap: mftrap1.in then mftrap2.in on
  the trap settings, writing trapin.log/trap.log/fot/trap.tfm/
  trap.72270gf, compared on accl with TeX Live's tftopl and gftype and
  mftraptest.test's filter (commit the triptrap inputs and expected
  files as fixtures, or read them from upstream/, which accl jobs have).
- The mktex logic (mktextfm/mktexpk in Rust), the finder hook and the
  cache: not started.
- gftopk, PK reader, writet3: on branch `mf-pk` (below).

## Facts found on accl (job 21878, the tools image)

- `mf --version`: "Metafont 2.71828182 (TeX Live 2026/Arch Linux)";
  tangle, tftopl, gftype, gftopk are in /usr/sbin.
- main_memory is 5,000,000 (texmf.cnf /usr/share/texmf-dist/web2c);
  mktex.cnf is /etc/texmf/web2c/mktex.cnf; MAKETEX_BASE_DPI 600.
- The tools image has **no mf.base** and no cp227.tcx (`kpsewhich`
  empty for both). The corpus oracles were built in the CI image
  (phitex-ci-2026-09-25) with TEXMFVAR=/w/texmf-var: crbox's
  term-pass2.txt shows `</w/texmf-var/fonts/pk/ljfour/public/ghab/
  ghab.600pk>`; oracle cache entries are
  phitex-ci/oracle/phitex-ci-2026-09-25/latex_crbox_crbox-doc-839ba8a2d0af7d7d
  and latex_ghab_ghab-doc-599cabece9045621 on acclhead1. TeX Live
  fixtures for mf/gftopk byte-identity must be made in that image (or
  with mf.base built first), with SOURCE_DATE_EPOCH and
  FORCE_SOURCE_DATE=1 so the GF/PK comment is fixed.
- mc (CLUSTER=mc scripts/accl/accl ..., gate-fast) is live with the
  same TeX Live; use it when accl queues.

## Branch `mf-pk` (the PK half, from dev 0a03f583)

- b897c2b2 (done, gated: build, lint, latex0-2, ams, amsedit8/16 green;
  suites red only on pages::vsize_document_pages; jobs 21935-21943):
  `crates/fonts/gftopk` (`phitex-gftopk`), gftopk.web 2.4 + gftopk.ch,
  `phitex_gftopk::gf_to_pk(gf: &[u8]) -> Result<Vec<u8>, String>`, no
  files, no output (as mktexpk runs it, without -v). Byte-identical with
  TeX Live's gftopk on 7 GF files made by TeX Live 2026's mf, mode
  ljfour, SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 (fixtures job
  21876, comparison job 21879): ghab, cmr10, cmti10, logo10, cminch at
  600 dpi, cmr5 at 60, cmtt8 at 120 (ghab.600pk 2624 bytes md5
  481b5c837e39ff298dd8399a5adc1e57; cmr10.600pk 10740 bytes md5
  fdd0cdba2f6f54380548c556cce5fd85; all md5s in
  tests/fixtures/list.txt). TeX Live's .tfm files of those fonts
  (ghab.tfm too) are in ~/code/flinner/phitex-runs/cmd-21876/fixtures/
  (fixtures for the mf byte-identity tests). wasm32-wasip1-threads check
  passes (run locally: the accl tools image lacks that target).
- cf64d479, 2dbe2c4d (WIP, built only by a local cargo check of pdf,
  tex, kpse; never tested on accl): crates/output/pdf/src/t3.rs, pdfTeX's
  pkin.c and writet3.c (writepk path) replacing the Unsupported branch;
  `PdfParams::pk_resolution` fixed at the first shipout (§792);
  `FontSource::pk_file(f, dpi)` asks for `<name>.<dpi>pk`; the engine
  passes \pdfpkresolution, `pdfout.rs::pk_request` builds the request,
  `fonts_for` prefetches it; kpse `Format::Pk`, `Kpse::init_prog(dpi,
  mode)`, `Kpse::find_pk(name, dpi)` (NAME.DPIpk, dpiDPI/NAME.pk, nearby
  resolutions), `phitex_kpse::pk_name`; the CLI's `Tree::pk(font, dpi)`
  in crates/cli/src/tex.rs (and the wasm and tests/common finders).
  **mktexpk plugs in where `find_pk` returns None in `Tree::pk`** (and
  the same spots in wasm/src/main.rs, tests/common/mod.rs); \pdfpkmode
  is not passed to the finder yet (`pk_request` is where to thread it).
- Left there: build and test on accl (clippy -D warnings, the pdf
  crate's tests, gate); an end-to-end latexmk fixture (pk-ghab: ghab.tfm,
  ghab.600pk, a 720 dpi PK for an `at 12pt` use) blessed with
  scripts/bless-latexmk.sh; pdfTeX's .pgc lookup before the PK;
  texfonts.map aliases and kpathsea's fallback resolutions in find_pk;
  a PK font used before the first shipout takes its dpi from
  \pdfpkresolution then, so a later change before the first shipout
  fails with "Font X at N not found"; rebase onto dev 77d3ec47 (mc
  scripts) under a new branch name (no force-push).
- Note: job 21876 ran TeX Live's mf in the tools image without trouble,
  so the "no mf.base" finding above (kpsewhich printed nothing) needs a
  second look before relying on it.

## How to resume

1. `git worktree add ~/code/tmp/phitex-mf mf` (or reuse it); read this
   note, mf.ch (all of it: the web2c changes) and the code parts of
   mf.web from §460 on, writing each part's Rust as it is read, in the
   style of the existing files.
2. Write hash.rs (§186-§213 code), the remaining parts, then
   `nice -n 19 cargo check -j2 -p phitex-mf` locally after removing the
   workspace `exclude`; fix until it builds; clippy runs with -D warnings
   on the gate (the crate allows the pedantic set and Pascal-shaped
   lints at its top).
3. The trap test on accl/mc; then gate-fast; report the hash.
4. Then the TFMs for the four Cyrillic manuals (mktextfm logic +
   finder hook + cache), then gftopk/PK/Type 3 for crbox and ghab.
