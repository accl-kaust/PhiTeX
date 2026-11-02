//! The real-document corpus's failure causes (scripts/corpus), each as a
//! minimal standalone document: `oracle/fixtures/corpus-repros/NAME.tex`,
//! built by `phitex build --exact-pdf` and compared byte for byte with
//! `NAME.pdf`, pdflatex's (pdfTeX 1.40.29 in the CI image, to its
//! fixpoint, `SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC`, as
//! scripts/ci/common.py pins them). A test that fails is ignored with its cause
//! and how many corpus documents it stops; the passing ones guard the fix.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/fixtures/corpus-repros")
}

fn cache() -> PathBuf {
    std::env::var_os("PHITEX_CACHE_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
        PathBuf::from,
    )
}

/// `NAME.tex` built in a fresh directory: its PDF (empty: none) and the
/// terminal's output.
fn build(name: &str) -> (Vec<u8>, String) {
    build_j(name, 1)
}

/// Where [`build_j`] builds `NAME.tex` at `threads` threads.
fn scratch(name: &str, threads: usize) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("corpus-repros")
        .join(format!("{name}-j{threads}"))
}

/// [`build`] at `threads` threads.
fn build_j(name: &str, threads: usize) -> (Vec<u8>, String) {
    build_out(name, threads, "pdf")
}

/// [`build_j`]'s output file `NAME.ext` (a DVI's, `ext` `dvi`).
fn build_out(name: &str, threads: usize, ext: &str) -> (Vec<u8>, String) {
    let dir = scratch(name, threads);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let tex = format!("{name}.tex");
    std::fs::copy(fixtures().join(&tex), dir.join(&tex)).expect("the repro");
    // (the files it reads beside it: `NAME/`'s)
    if let Ok(files) = std::fs::read_dir(fixtures().join(name)) {
        for f in files.flatten() {
            std::fs::copy(f.path(), dir.join(f.file_name())).expect("a file of the repro");
        }
    }
    let out = Command::new(env!("CARGO_BIN_EXE_phitex"))
        .args([
            "build",
            "-v",
            "--exact-pdf",
            "-j",
            &threads.to_string(),
            &tex,
        ])
        .current_dir(&dir)
        .env("PHITEX_CACHE_DIR", cache())
        .env("SOURCE_DATE_EPOCH", "1758800000")
        .env("FORCE_SOURCE_DATE", "1")
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env_remove("PHITEX_COMPAT")
        .output()
        .expect("phitex runs");
    let pdf = std::fs::read(dir.join(format!("{name}.{ext}"))).unwrap_or_default();
    let term =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (pdf, term)
}

fn check(name: &str) {
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    let (got, term) = build(name);
    if got != want {
        let n = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
        let errors: Vec<&str> = term
            .lines()
            .filter(|l| l.starts_with("error"))
            .take(5)
            .collect();
        panic!(
            "{name}: the PDF differs from pdflatex's at byte {n} ({} vs {} bytes); first errors: {errors:?}",
            got.len(),
            want.len()
        );
    }
}

/// `NAME.tex` built: its `.aux` equals pdflatex's (`NAME.aux`, to its
/// fixpoint) and the build gives no error.
fn check_aux(name: &str) {
    let (_, term) = build(name);
    let dir = scratch(name, 1);
    let got = std::fs::read(dir.join(format!("{name}.aux"))).unwrap_or_default();
    let want = std::fs::read(fixtures().join(format!("{name}.aux"))).expect("the oracle");
    let errors: Vec<&str> = term
        .lines()
        .filter(|l| l.starts_with("error"))
        .take(5)
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&want),
        "{name}: the .aux differs from pdflatex's; errors: {errors:?}"
    );
    assert!(errors.is_empty(), "{name}: {errors:?}");
}

/// A package the document writes where it is missing (LaTeX's
/// file-writing environment) and then loads: from the second round the
/// file exists, the environment lets `\write` gobble its arguments in
/// its group, and the `\immediate\write` of `\relax` that
/// `\begin{document}` makes after opening the `.aux` must still reach
/// it. (The round core filed the environment's local definition, new in
/// the second round in a node whose scope was the same, at the node's
/// own level: every `\write` after the environment was gobbled, and the
/// `.aux` lost its `\relax`.)
#[test]
fn written_text_lost_write() {
    check_aux("written_text_lost_write");
}

/// `NAME.tex` built at one and at four threads: its output file
/// (`NAME.ext`) equals the oracle's at both.
fn check_threads(name: &str, ext: &str) {
    let want = std::fs::read(fixtures().join(format!("{name}.{ext}"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_out(name, j, ext);
        if got != want {
            let n = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
            let errors: Vec<&str> = term
                .lines()
                .filter(|l| l.starts_with("error"))
                .take(5)
                .collect();
            panic!(
                "{name} -j{j}: the {ext} file differs from the oracle's at byte {n} ({} vs {} bytes); first errors: {errors:?}",
                got.len(),
                want.len()
            );
        }
    }
}

/// A plain TeX repro whose lines a box register carries out of the
/// output routine that sealed them (`crate::pages::seal`): its PDF
/// (pdftex's) and its DVI (`NAME_dvi.tex`, etex's), each at one and at
/// four threads.
macro_rules! sealed_repro {
    ($name:ident, $why:literal) => {
        #[doc = $why]
        #[test]
        fn $name() {
            check_threads(stringify!($name), "pdf");
            check_threads(concat!(stringify!($name), "_dvi"), "dvi");
        }
    };
}

sealed_repro!(
    sealed_box_held,
    "a routine's page lines kept in a box register and placed by the next piece of the document (\\box): sealed in the register, read from their pieces' units where the page is written (oracles: pdftex and etex in the CI image)"
);
sealed_repro!(
    sealed_unvbox_held,
    "a routine's page lines kept in a box register and unboxed by the next piece (\\unvbox outside the finisher that sealed them, `Run::unseal_open`; oracles: pdftex and etex in the CI image)"
);
sealed_repro!(
    sealed_lastbox_held,
    "one line of a routine's page (\\lastbox) kept in a box register and placed by the next piece: a sealed line itself carried out of its finisher (oracles: pdftex and etex in the CI image)"
);
sealed_repro!(
    sealed_lastbox_copied,
    "one line of a routine's page kept in a box register, copied and unhboxed by the next piece (\\copy, \\unhcopy; oracles: pdftex and etex in the CI image)"
);
sealed_repro!(
    sealed_given_back,
    "a routine's page lines kept in a box register and given back by the next routine (\\unvbox in the output routine): the last page's lines all from other finishers' pieces, shipped by a finisher with no sealed window of its own, written with its lines read by piece; the DVI's lines were empty boxes with the finishers' definitions sealed and no places on such a page (oracles: pdftex and etex in the CI image)"
);

macro_rules! repro {
    ($name:ident, $why:literal) => {
        #[doc = $why]
        #[test]
        fn $name() {
            check(stringify!($name));
        }
    };
}

repro!(
    display_align,
    "corpus (green since dev f3463b9a, cuts): \\halign in a display ($$\\halign$$: amsmath align/gather, eqnarray) is 'this alignment is not supported' (control.rs REDUCE-GAP), the first cause in 27 of 136 arXiv papers; ported on cuts"
);
repro!(
    row_starts_with_amp,
    "corpus (green on dev 4ce434f1, cuts): a tabular row that starts with & (an empty first cell) gives 'illegal command' / 'You can't use \\cr in restricted horizontal mode' and Extra alignment tab; amsldoc.tex:1211, arXiv a-2409.03762v1; owner cuts"
);
repro!(
    table_float_lost,
    "corpus (green since dev 301ef3af, pagechain's pgstfix): \\caption + \\centering + tabular in a table float lost the float ('Float(s) lost') on b7646c77, first cause in 8 of 136 arXiv papers"
);
repro!(
    pagestyle_in_group,
    "amsldoc (amscorrect): a page fired inside a group the text's run opens and closes runs its routine in that group's locals (§1025): \\cleardoublepage as amsldoc defines it, \\clearpage{\\pagestyle{empty}\\cleardouble@page}, printed the running head on the blank page"
);
repro!(
    amsmath_page_counter,
    "amsldoc (amscorrect): the text reads \\count0..9 as the output routines left them (§1025): amsmath's \\savecounters@/\\restorecounters@ in a display after a page break wrote a stale \\c@page back, and the page numbers after it restarted"
);
repro!(
    multicol_partial,
    "amsldoc (multicol): \\begin{multicols} forces a page whose routine ships nothing (multicol.dtx \\prepare@multicols: the partial page saved in \\partial@page) and \\end{multicols} balances the columns back onto the page (\\balance@columns@out); the partial page came out twice, then 2pt high (§1025)"
);
repro!(
    multicol_pages,
    "amsldoc (multicol): a multicols environment over several pages (multicol.dtx \\multi@column@out ships each page with the partial page on the first) and balanced at \\end{multicols} on the last"
);
repro!(
    multicol_theindex,
    "amsldoc (multicol): imakeidx's theindex is a multicols environment under a \\section* title (amsldoc's index, pdflatex's pages 36-40); it hung with '257 rounds without progress'"
);
repro!(
    routine_global_local,
    "multicol: TeX's state holds the younger assignment (§279): a routine's \\global made at an earlier page beats the text's local made before it (multicol's \\@colroom), and a local the text makes after it beats it, in the same group; after the group the global survives (§283)"
);
repro!(
    routine_local_dies,
    "multicol (TeX's assignment order): a routine's local assignment dies with its output group (§1025–§1026), its \\global one reaches the next page's routine"
);
repro!(
    text_global_in_group,
    "multicol (TeX's assignment order): a \\global in an inner group hides the outer group's save (§283): after both groups end, the page's routine reads the global"
);
repro!(
    box_use_group,
    "multicol (TeX's assignment order): a box register used in the text (\\box1) is voided in place, unsaved (§1079), at level one when a routine set it \\global: the next routine reads it void"
);
repro!(
    imakeidx_index,
    "amsldoc (index): imakeidx's \\printindex runs \\write18{makeindex ...} under TeX Live's restricted shell escape (\\pdfshellescape 2) and inputs the .ind at once; our makeindex runs in process on the .idx the job wrote"
);
repro!(
    devnull_exists,
    "research corpus (thuthesis, ctex): kpathsea's READABLE is a name readable and not a directory, so \\IfFileExists{/dev/null} is true on Unix (\\pdffilesize/\\filesize, expl3's file test); PhiTeX's kpse asked for a regular file, and thuthesis took the Windows fontset (SimSun, not loadable)"
);
repro!(
    siunitx_s_column,
    "research corpus (jmlr pmlr-sample, chem-common): siunitx's S column reads its u template's last token as a macro's delimiter; §390 pops the spent template there (align_state 0, §324), but PhiTeX's u-template end marker was reached only later, so the cell's \\if_false: { \\fi: left align_state at 1000001 and the \\cr ending the row was not an entry's end (Extra alignment tab)"
);
repro!(
    pdftexbanner_producer,
    "research corpus (acmart): hyperxmp makes /Producer of \\pdftexbanner, pdfTeX's pdftex_banner with TeX Live's and kpathsea's versions; PhiTeX's lacked ' (TeX Live 2026/Arch Linux) kpathsea version 6.4.2'"
);
repro!(
    letterspaced_vf_expansion,
    "research corpus (classicthesis): microtype's \\textls (\\letterspacefont, a virtual font) over mathpazo's virtual small caps pplrc9d, with font expansion: the expanded local font of an expanded virtual font is an expanded virtual font in turn (pdfTeX's auto_expand_vf through do_vf); PhiTeX took 'pplrc9d-9' for a real font, 'Font pplrc9d+7 not found (no entry in the font map)', no PDF"
);
repro!(
    aux_line_read_back_meaning,
    "research corpus under XeLaTeX (aomart, memoir, classicthesis: no /PageLabels): a line written to the .aux is interpreted with the meanings at its read-back point, \\begin{document}; a meaning this run had already loaded took the slot's fast path (set_cur_cs, cur_macro) and was read as the writer had it: hyperref's \\HyPL@Entry, let to \\@gobble after the read-back, gathered no labels"
);
repro!(
    microtype_expansion,
    "corpus (about 120 documents in sweep 14495): microtype's defaults, font expansion (\\pdfadjustspacing 2, \\pdffontexpand autoexpand: the paragraph broken with the expanded fonts' stretch and shrink, the lines set at a horizontal scale) and character protrusion; bench/corpus-repro/microtype-expansion.tex"
);
repro!(
    microtype_options,
    "microtype with every option under pdfLaTeX: expansion, protrusion, tracking (\\textls, \\letterspacefont), kerning (\\pdfprependkern, \\pdfappendkern: French punctuation, auto kerns in the main loop) and spacing (\\pdfadjustinterwordglue, \\knbscode...), in two columns"
);

repro!(
    unvbox_penalty_fires_at_par,
    "research corpus (revtex-aps, ltxgrid's \\do@output@MVL): a forced penalty an \\unvbox puts on the contributions (§1110: no build_page) fires at the next paragraph's start (new_graf's build_page, §1091), so the routine's \\global\\hsize is that paragraph's; PhiTeX's paragraph-start candidate was not cut when the penalty came from an earlier piece, and the paragraph was set at the old width (revtex's title at the column's width)"
);

repro!(
    aftergroup_brace_in_alignment,
    "TeX Live manual othelloboard (LaTeX's lrbox in a tabular's entry): an \\aftergroup} closes the box begun with {; the brace put back where the group ends is counted as back_input counts it (§282, §326), and PhiTeX's put it back uncounted, so align_state stayed off by one and the & after the environment was 'You can't use & in restricted horizontal mode'"
);

repro!(
    unvbox_penalty_fires_in_cond,
    "research corpus (revtex-aps's widetext equation): the forced penalty ltxgrid's \\unvbox left fires at the next paragraph's start inside a conditional too (amsmath's display begins in \\ifmmode's \\else): the routine's \\global\\hsize is the display's width; PhiTeX cut a paragraph's start only outside conditionals, and the equation was set at the column's width"
);

repro!(
    pdf_image_objects_counted,
    "research corpus (asmeconf: hyperxmp's /Metadata, the /OutputIntents and /ViewerPreferences objects numbered 336, 353 and 354, each the number of a destination too, a broken file): the objects writing an included PDF page makes after the page that shows it first (pdf_write_image: its fonts, streams and resources copied) are counted where a \\pdfobj after it is numbered; the pages' fold of allocations left them out"
);

repro!(
    pdfmapfile_applied,
    "TeX Live manual gillcm (its sample's \\pdfmapfile{+cmg.map}): \\pdfmapfile's kind was read after its text was scanned, from the text's last token, and the file was taken for a \\pdfmapline: no entry for the map's fonts, 'Font cmgbiu8r at 600 not found'"
);

/// An image the writer cannot write where the link writes it (a PNG
/// whose header reads and whose data does not) ends the job at every
/// thread count as pdftex's `pdftex_fail` does: `!pdfTeX error: pdftex
/// (file ./bad.png): libpng: internal error` and ` ==> Fatal error
/// occurred, no output PDF file produced!`, no PDF (mc 77541). The CLI
/// wrote the PDF with an error said twice ("pdfTeX error: pdfTeX error:
/// ./bad.png: ..."); the link placing a page whole also dropped the error
/// (`PdfState::replay_page`).
#[test]
fn pdf_image_write_error() {
    let fail = "!pdfTeX error: pdftex (file ./bad.png): libpng: internal error";
    for j in [1, 4] {
        let (pdf, term) = build_j("pdf_image_write_error", j);
        assert!(pdf.is_empty(), "-j{j}: a PDF was written:\n{term}");
        let log = std::fs::read_to_string(
            scratch("pdf_image_write_error", j).join("pdf_image_write_error.log"),
        )
        .unwrap_or_default();
        // (the lines between them, the input files closed, are the
        // engine's: the .log need not match)
        assert!(
            log.lines().any(|l| l == fail)
                && log.contains(" ==> Fatal error occurred, no output PDF file produced!"),
            "-j{j}: the log has not pdftex's fatal error:\n{log}"
        );
        assert!(
            !term.contains("pdfTeX error: pdfTeX error"),
            "-j{j}: the error said twice:\n{term}"
        );
    }
}

/// `\inputlineno` in the output routine is the line the page builder
/// was called at (the file stays on the input stack under the routine's
/// tokens, §1025): each page's routine writes it to the `.aux`, read back
/// into the text. A finisher's routine read only token lists and gave 0
/// (the user's acmart paper: every routine at line 0, pdfTeX's at the
/// lines it fired at). pdflatex's PDF at -j1 and -j4.
#[test]
fn output_inputlineno() {
    check("output_inputlineno");
    let want = std::fs::read(fixtures().join("output_inputlineno.pdf")).expect("the oracle");
    let (got, term) = build_j("output_inputlineno", 4);
    assert!(got == want, "-j4: the PDF differs from pdflatex's:\n{term}");
}

repro!(
    mktex_written_source,
    "TeX Live manual feynmf (its template): a font whose METAFONT source the job writes is made by kpathsea's `mktextfm` and `mktexpk` from the file the run before wrote; only the CLI's finder made fonts, from sources on disk: 'Metric (TFM) file not found' at every round"
);

repro!(
    mktex_metafont_font,
    "a font with only a METAFONT source (dancers), its TFM and PK made where the finder misses them: by the engine (one path, its sources through the finder), not the CLI's finder alone, so a Shelf host (the browser) makes them too ('Font dancers at 600 not found' there)"
);

repro!(
    pdfthread_in_hlist,
    "\\pdfthread in a paragraph: the command's kind was read after its specification was scanned, and \\pdfthread was taken for \\pdfstartthread ('\\pdfstartthread ended up in hlist')"
);

/// The objects of a page's fonts are its real fonts' (pdfTeX's
/// `pdf_init_font`, one per TFM name), a virtual font's characters
/// reaching them through its packets: newtx's `ntxsy` and `ntxsy7`,
/// virtual fonts over `txsys`, make one object, and a `\pdfobj` after
/// the pages is numbered after it; their characters in a paragraph's
/// lines (sealed: their stand-ins name the real fonts the characters
/// reach, `crate::vfreach`), in a box on the vertical list and in a line
/// that reads its position at shipping (not sealed). The read-time
/// count named the virtual fonts (asmeconf, a number one too high). At
/// -j1 and -j4 (oracle: pdflatex, mc 77635, the same in the CI image, mc 77658).
#[test]
fn vf_objects_by_real_font() {
    let name = "vf_objects_by_real_font";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_j(name, j);
        assert!(
            got == want,
            "{name} -j{j}: the PDF differs from pdflatex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}

/// A virtual font's local fonts whose map entries a `\pdfmapfile` gives
/// (gillcm's `cmgmi8r`, jamtimes' `jtmr8rw`): a page written apart is
/// written on a writer that has read the map items before the page, as
/// the link's has there (`pdfout::base_fold`); it had read none, the
/// local fonts had no entry there and their characters advanced as a PK
/// font's do (`getpkcharwidth`): kerns of 1 inside the words of the TJ
/// strings (the TeX Live manuals of gillcm and jamtimes). At -j1 and -j4
/// (oracle: pdftex, mc 77658).
#[test]
fn pdfmapfile_vf_local_fonts() {
    let name = "pdfmapfile_vf_local_fonts";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_j(name, j);
        assert!(
            got == want,
            "{name} -j{j}: the PDF differs from pdftex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}

/// `\input` of a name with a font's extension: CJK.sty's `\input
/// CJK.enc` is TeX's lookup, kpathsea's `tex` format (TEXINPUTS), as
/// `\openin`'s; the font side's lookups (a map line's encoding, a TFM)
/// are in the format the extension names. The finders took the format
/// from the extension for both, so CJK.enc was looked for among the
/// encodings and not found ("I can't find file": the manuals of cjk-ko,
/// kotex-utf and nstc-proposal). At -j1 and -j4 (oracle: pdflatex in
/// the CI image, accl 22458; passing in 22464).
#[test]
fn input_enc_named_file() {
    check_threads("input_enc_named_file", "pdf");
}

/// TeX's lookup of a name whose last component has an extension the
/// `tex` format does not know (`1.0.0-1`, spelatex's chunk files): as
/// kpathsea (TeX Live's `try_std_extension_first`), `name.tex` first,
/// then the name itself, for `\input`, `\openin` and `\pdffilesize`;
/// and `./name` the file the job writes as `name`. A dot anywhere in the
/// name (`./` too) took it as given, and `./` made another file than the
/// job's, so `\input{./example-spel/1.0.0.0.0.0-1}` found nothing and
/// LaTeX asked the terminal for another name (the spelatex manual:
/// "cannot \read from terminal in nonstop modes"). At -j1 and -j4
/// (oracle: pdflatex in the CI image, accl 22475; passing in 22505).
#[test]
fn input_dotted_name() {
    check_threads("input_dotted_name", "pdf");
}

/// `\input` of a name with an extension the `tex` format does not know,
/// a file the job writes: kpathsea finds `name` (no `name.tex`), and
/// what TeX keeps of the file is that one's. The lookup found `name` but
/// kept its read-back, read-back point and seed under `name.tex`, so a
/// `.dat` read back from the run before defined nothing (phitex-tex's
/// `auxloop::a_file_of_definitions_is_read_back_per_field_whatever_its_name`),
/// conv-xkv's `.cut` written and read in one run left `\fname`
/// undefined, and hyperref's `.out` and probsoln's `excluded.tex` never
/// settled. At -j1 and -j4 (oracle: pdflatex in the CI image).
#[test]
fn input_dotted_read_back() {
    check_threads("input_dotted_read_back", "pdf");
}

/// pdfTeX 1.40.26's structure destinations: `\pdfdest struct n` (a
/// destination naming structure element `n`'s object, not its page's)
/// and a `goto` action's `struct` (its `/SD`, with `file` the text as
/// given), each identifier of a namespace of its own (`num 3` and
/// `struct … num 3` two objects), one named by a link before its page
/// defines it and one by an outline. `\pdfdest struct` was an error
/// ("identifier type missing": the tex-vpat manual) and an action's
/// `struct` was read as its identifier. At -j1 and -j4 (oracle:
/// pdflatex, pdfTeX 1.40.29 in the CI image, accl 22464, passing there).
#[test]
fn pdfdest_struct() {
    check_threads("pdfdest_struct", "pdf");
}

/// A file looked for in vain, then the `.aux` read back as its text
/// (transparent's `\transparent@use` lines, the first defining
/// `\TRP.5`, which the next read): the lines written later are read
/// under the state at the read-back point, the values the read-back's
/// own definitions replaced. The vain lookup left a pending read-back
/// point that took those values for a file never written, so the lines
/// read `\TRP.5` as defined and the rounds alternated between reading
/// the `.aux` per field and as its text, never settling (the milog
/// manual: "still changed after 5 rounds"). At -j1 and -j4 (oracle:
/// pdflatex in the CI image, accl 22529, passing there).
#[test]
fn read_back_after_vain_lookup() {
    check_threads("read_back_after_vain_lookup", "pdf");
}

/// `\pdfinterwordspaceon`'s space font: its font object is made where a
/// page first uses it, as any font's (`pdf_use_font`); the fold of the
/// pages' objects counted the fonts the list names only, so an object
/// number TeX read after such a page was one short (tex-vpat's
/// structure elements, one number low from its first page on). Its
/// page also has `\pdfobj reserveobjnum` in the text, whose optional
/// space after the keyword was a space of the paragraph. At -j1 and -j4
/// (oracle: pdflatex in the CI image, accl 22529; passing in 22533).
#[test]
fn interword_space_font_object() {
    check_threads("interword_space_font_object", "pdf");
}

/// A raw object a page names with `\pdfrefobj`, neither a stream nor
/// `\immediate`, is written with the page ("Write out pending raw
/// objects", `pdf_begin_obj(k, 1)`), into an object stream. The pages'
/// fold of the objects counted the page's dictionary, resources and
/// annotations toward a stream's 100 but not those, so the next stream's
/// number fell later and every object number TeX read after it was one
/// short: iacrj's `/Metadata 183 0 R` for pdfTeX's 184 (its raw /Group
/// and info objects on page 1; `PHITEX_ALLOC_TRACE`, accl 22639). At -j1
/// and -j4 (oracle: pdflatex in the CI image).
#[test]
fn raw_objects_in_object_streams() {
    check_threads("raw_objects_in_object_streams", "pdf");
}

/// An included PDF page with a `/Group`: the page that first shows it
/// numbers the group's object (`out_image`), and the image's write puts
/// it into an object stream. The image's own writer (`ximage_own_info`),
/// showing no page, wrote the group in the image's dictionary and
/// counted no object for it, so every object number TeX read after that
/// page was one short: iacrj's logo, `/Metadata 183 0 R` for pdfTeX's 184
/// (`PHITEX_ALLOC_TRACE`, page 1's objects in object streams 21 for 22,
/// accl 22754). At -j1 and -j4 (oracle: pdflatex in the CI image).
#[test]
fn pdf_image_page_group() {
    check_threads("pdf_image_page_group", "pdf");
}

/// An included PDF page whose `/Group` is an indirect object: the image's
/// own writer, showing no page, copied the group's object into the
/// image (an object), and the count of the page's group was added on
/// top, so every object number TeX read after the page was one past
/// pdfTeX's (df0d049ac counted the group for every included page with
/// one). The own writer now shows the image as the page does
/// (`out_image` numbers the group), and the write makes what pdfTeX's
/// makes. At -j1 and -j4 (oracle: pdflatex in the CI image).
#[test]
fn pdf_image_page_group_ref() {
    check_threads("pdf_image_page_group_ref", "pdf");
}

/// A PNG with an alpha channel: reading it numbers the document's
/// transparency group, and its write writes the group into an object
/// stream, which that write opens. The image's own writer counted the
/// stream with the image's objects (`PdfState::objects_after` took only
/// the streams its later objects went into for streams), so every
/// object number TeX read after the page showing it was one past
/// pdfTeX's (beautynote's `/pgfsmask76` for 75, elegantnote-en; hidden
/// there until df0d049ac counted the included PDF's page group, one
/// short, beside it; `objects_after`'s fix is 9dd4e3020's, as
/// `png_alpha_objects_counted`'s). At -j1 and -j4 (oracle: pdflatex in the
/// CI image).
#[test]
fn pdf_image_png_group() {
    check_threads("pdf_image_png_group", "pdf");
}

/// A line a node boundary cuts right after a control word: its
/// continuation, the next sibling's first segment, began in a state of
/// its own, so the space after the name (skipped on one line, §354) was a
/// space token there. smartunits' `pdfinfo={Title={ smartunits -
/// \smart@version }}` ended in a space (`/Title <...0020>`), the cut
/// falling there once a `\NewDocumentCommand` was in the preamble
/// (`\showtokens` of the line: `\smart@version  .`, accl 22792). At -j1
/// and -j4 (oracle: pdflatex in the CI image).
#[test]
fn cut_after_control_word() {
    check_threads("cut_after_control_word", "pdf");
}

/// `\pdfnoligatures` (pdfTeX's `set_no_ligatures`): the font's
/// characters lose their lig/kern programs, no ligature and no kern. The
/// primitive set a mark nothing read, so microtype's tracked small caps
/// kept their ligatures: lie-hasse's section title "Three dimensional
/// effect" had the ff ligature and sat 0.275bp off pdfTeX's, centred
/// (accl 22608, 22724). At -j1 and -j4 (oracle: pdflatex in the CI image).
#[test]
fn pdfnoligatures() {
    check_threads("pdfnoligatures", "pdf");
}

/// `\tagcode` below 0 (pdfTeX's `set_tag_code`): the character's tags
/// taken off for good, -1 its lig/kern program. The code was kept and
/// read back, but the font kept its tags, so microtype's tracked small
/// caps (`no ligatures={f}`, a `\tagcode` of the f) kept the ff
/// ligature: lie-hasse's section title "Three dimensional effect" sat
/// 0.275bp off pdfTeX's (accl 22825, its own preamble). At -j1 and -j4
/// (oracle: pdflatex in the CI image).
#[test]
fn tagcode_ligatures() {
    check_threads("tagcode_ligatures", "pdf");
}

/// `\pdffontattr` and `\pdfnobuiltintounicode` of a virtual font (cmap's,
/// for every font it loads; libertine's T1 fonts are virtual) set its
/// fields without reading it in (no `do_vf`): pdfTeX numbers its local
/// fonts where a page first reads it in, after the fonts loaded before.
/// Their steps were given the font's virtual file and the link numbered
/// its local fonts where they ran (acmart: `/F86` for pdfTeX's `/F228`,
/// every font of the file another number). At -j1 and -j4 (oracle:
/// pdflatex in the CI image, mc 77664; the base differs at byte 702).
#[test]
fn fontattr_vf_not_read_in() {
    let name = "fontattr_vf_not_read_in";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_j(name, j);
        assert!(
            got == want,
            "{name} -j{j}: the PDF differs from pdflatex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}

/// A font microtype expands automatically (`\pdffontexpand ...
/// autoexpand`: `cmr10+5`, `cmr10-10`, ... where lines stretch and
/// shrink) has its base's font object (`pdf_init_font`'s second case):
/// the read-time count of the pages' objects counted an object for each
/// expansion's name, and a `\pdfobj` after the pages was numbered too high
/// (acmart: hyperxmp's `/Metadata`, 369 for 316). At -j1 and -j4 (oracle:
/// pdflatex in the CI image, mc 77682; the base differs at byte 2145).
#[test]
fn expanded_fonts_one_object() {
    let name = "expanded_fonts_one_object";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_j(name, j);
        assert!(
            got == want,
            "{name} -j{j}: the PDF differs from pdflatex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}

/// The research corpus's amsbook at -j4 (2 pages more than -j1 in about
/// half the builds): the piece that ends a title page whose `\topskip`
/// `\@maketitle` set in a group puts the page parameters on the main
/// list at its end; reading them stopped the run there and a marker of
/// zeros was committed without the read that would run it again
/// (`Run::put_page_params`). Built eight times at four threads, each
/// time pdflatex's PDF.
#[test]
fn amsbook_title_threads() {
    let name = "amsbook_title_threads";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for k in 0..8 {
        let (got, term) = build_j(name, 4);
        assert!(
            got == want,
            "{name} -j4, build {k}: the PDF differs from pdflatex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}
repro!(
    type3_pk_font,
    "bitmap fonts: a font with only a METAFONT source (yinit, no map line) is a Type 3 font in the PDF, from the PK file mktexpk makes (in process: `phitex-mf` with the base fmtutil builds, gftopk), at its design size and magnified (kpathsea's `MAKETEX_MAG` 1+264/600, 0+264/600); was a DVI file instead (oracle: pdflatex in the CI image, its PKs made by TeX Live's mktexpk, mc 77344)"
);
repro!(
    type3_pk_font_xe,
    "bitmap fonts under XeLaTeX: xdvipdfmx's pkfont.c (crates/output/xdvipdfmx/src/pkfont.rs) embeds yinit's PK files, made by mktexpk in process, as a Type 3 font; was xdvipdfmx's fatal error (oracle: xelatex in the CI image, mc 77373)"
);
repro!(
    eps_base35_font_xe,
    "an EPS figure with text in a base 35 font under XeLaTeX (revtex-aps-xe, revtex-aip-xe: `fig_1.eps`): the job's view of xdvipdfmx's files (phitex-live's `JobFiles`) did not pass Ghostscript's fonts on, so the in-process distiller's findfont found no Times-Roman and xdvipdfmx stopped (/invalidfont); no PDF (oracle: xelatex in the CI image)"
);
repro!(
    biber_macro_case,
    "biber: an @string macro used in another case than its definition's (acm-sigconf-biblatex: `journal = CACM` for `@String{CACM = ...}`) was undefined, the field lost (82 destinations moved); btparse defines them lowercased and looks them up lowercased (oracle: latexmk, pdflatex and biber in the CI image)"
);
repro!(
    prevgraf_vbox,
    "\\prevgraf in a \\vbox: read and assigned through the field of the enclosing list's name, never the level the run builds the box on, where the paragraph's end put its lines: 0 for 4 (oracle: pdflatex in the CI image)"
);
repro!(
    lineno_vbox_abstract,
    "AASTeX's abstract with line numbers: lineno's \\linenumberpar puts its penalty after a paragraph only when \\prevgraf grew, which read 0 in the abstract's \\vbox; each paragraph's last line went unnumbered, every number after one less (aastex, aastex-xe; oracle: pdflatex in the CI image)"
);
repro!(
    pk_fonts_own_objects,
    "bitmap (PK) fonts never share an object (`pdf_init_font` shares only scalable fonts of a TFM name): yinit at two sizes is two objects, and the read-time fold counted one, numbering an object after the page one too low (oracle: pdflatex in the CI image)"
);
repro!(
    output_step_then_local_group,
    "a register stepped globally by a constant it never read (the output routine's \\global\\advance\\c@page), then set locally in a group (mparhack's hook, {\\advance\\c@page\\m@ne}): the set dropped the step, the run published nothing and every page was page 1 (classicthesis: a blank page missing, every page number after it off; oracle: pdflatex in the CI image)"
);
repro!(
    mparhack_page_numbers,
    "mparhack's page numbers (classicthesis): \\c@page lost the output routine's step, so \\cleardoublepage made no blank page (oracle: pdflatex in the CI image)"
);
repro!(
    output_pagegoal,
    "research corpus (aastex, aastex-xe: Table 1 at the top of page 4, pdfTeX's on page 5): while the output routine is active \\pagegoal and its kin read page_so_far as the page's fire left it (§421), and PhiTeX read the emptied page's \\maxdimen and 0; ltxgrid's \\@add@float takes \\vsize-\\pagegoal into \\@pageht, which came out near -\\maxdimen, so every float fitted where it was met (oracle: pdflatex in the CI image, mc 77786)"
);
repro!(
    aftergroup_scope_paragraph,
    "research corpus (classicthesis, classicthesis-xe: the title 2.277 pt right of pdfTeX's, the title's colour on every page after): a group opened in vertical mode ends inside a paragraph begun in it (\\begingroup\\color{CTtitle}\\spacedallcaps{...}\\endgroup); the scope's \\aftergroup tokens (\\reset@color's pop) were read by the paragraph's level's id, not the scope's, and dropped, the space before them taken off the line (oracle: pdflatex in the CI image, mc 77793)"
);
repro!(
    lineno_last_page_xe,
    "research corpus (aastex-xe: page 8's text gone, its line numbers left): under XeLaTeX a page's sealed lines are filled by the end node from their pieces' units; lineno's routine gives every line back, so the last page's lines all came back from routines before and its finisher sealed none of its own: the page carried no places, and its lines were never filled (the PDF writer's `sealed_units` too); now an empty window, each line's piece's unit read by itself (oracle: xelatex in the CI image, mc 77824)"
);
repro!(
    protrusion_math_glue,
    "research corpus (acm-sigconf-biblatex: a bibliography line's spacing, -250 for -248, other expanded fonts made, one font fewer numbered, /F460 for /F461): margin protrusion passes over glue only when its spec is the shared zero_glue (cp_skipable); a math glue made ordinary (§732, biblatex's URLs' \\mskip; §766, the spacing between atoms) has a spec of its own, and PhiTeX took it for zero_glue and protruded the character before it (oracle: pdflatex in the CI image, mc 77829)"
);
repro!(
    list_depth_scope,
    "research corpus (classicthesis, classicthesis-xe: the title page's list one level deeper than pdfTeX's): a list's \\global\\advance\\@listdepth\\@ne, then a local \\advance\\@listdepth\\m@ne in the group the run leaves open (KOMA's addmargin); the run published the step as a relation and the scope's 0 beside it, a field holds a relation or values, not both, so inside it read 1; now the step is published as the value it makes beneath the scope's own (oracle: pdflatex in the CI image, mc 77852)"
);
repro!(
    output_aftergroup,
    "research corpus (thuthesis: the text after a float placed here 3.488 pt higher than pdfTeX's): the output group's \\aftergroup tokens are read in the main vertical mode after the routine, before the text's next ones (§1100); thuthesis's \\@addtocurcol leaves \\vskip\\intextsep\\nointerlineskip there, and the finisher ran them on its own list, dropped. Now the finisher defines them and the first piece after it reads them first (oracle: pdflatex in the CI image, mc 77860)"
);
repro!(
    pdf_fontname_share,
    "\\pdffontname and \\pdffontobjnum of one scalable TFM at several sizes: pdfTeX's `pdf_init_font` makes one object for them, named by the first font's number (`obj_info`); each font made an object of its own and was named by its own number (32 41 42 and objects 1 2 3 for pdfTeX's 32 32 32 and 1 1 1) (oracle: pdflatex in the CI image, mc 77924)"
);
repro!(
    page_counter_after_own_page,
    "manuals (gu's gudoc: pages 10-14 numbered 8,9,8,9,10, /PageLabels written): a page fires at a candidate inside a conditional the text's run opens, and a float after it reads \\c@page and puts it back (tabularx's trial runs, \\TX@ckpt); the run read the register at its own position, the value before that page, and the restore took the page's steps back; now a piece's read of a page-layer register after a candidate of its own a page fires at ends the run there, its rest a resume after the page's finisher (oracle: pdflatex in the CI image, accl 22558)"
);

/// TeX Live manuals euromoney, heria, pygmentex and zed2e (`\valign`):
/// "this alignment is not supported" (control.rs REDUCE-GAP). Now the
/// alignment's vertical variant, §768-§812 with tex.web's modes swapped:
/// the alignment in -hmode, its entries in -vmode (depth limit 0, a
/// paragraph in one ended by its `&`), columns on the horizontal list
/// with space factor 1000; \noalign, \tabskip, \span, \omit, \crcr,
/// \everycr, `to` and `spread`, nested in and around \halign. The next
/// entry's first token is read after the entry's group ends (§791), so
/// its \aftergroup tokens come first and its local definitions are
/// undone, \halign's too. At -j1 and -j4 (oracle: pdftex in the CI image).
#[test]
fn valign_columns() {
    check_threads("valign_columns", "pdf");
}

/// research corpus (classicthesis: a font number on page 31 three above
/// pdfTeX's): microtype's letter-spaced small caps over mathpazo's virtual
/// `pplrc9d`, expanded (`pplrc9d+80ls+20`), has a local font made anew
/// (`pplrc9d+20`, `auto_expand_vf`) that is an expanded virtual font in
/// turn, with local fonts of its own. pdfTeX makes them where the font is
/// first shipped and never again; the engine keyed the inner ones by the
/// page's handle of their virtual font, another on each page, and made
/// them again on a later page whose step read other local fonts first.
/// At -j1 and -j4 (oracle: pdflatex in the CI image, accl 22577; the base
/// differs at byte 974).
#[test]
fn vf_expanded_local_vf() {
    check_threads("vf_expanded_local_vf", "pdf");
}
repro!(
    marginpar_after_display,
    "manuals (hep-paper, verifiche: 'Float(s) lost'): a \\marginpar before a display in a paragraph and another after it; the first one's -10004 fires at the display's start (§1145's build_page) and its routine takes it off \\@currlist before the text after the display conses the second on. The piece ran on through the display's start (a list of its own open), its \\@currlist still holding the first, and that definition stood over the routine's: the second was never output. A display's start is now a cut where a page is made to fire, as a paragraph's start is (oracle: pdflatex in the CI image, accl 22589)"
);

/// TeX Live manual proofread: soul's highlighting puts a `TikZ` mark
/// (`\pdfsavepos` and a `\write` of `\pgfsyspdfmark`) into the pre-break
/// text of a discretionary at each hyphenation point; where the line does
/// not break there, `hlist_out` never meets it (§622) and nothing is
/// written. The shipout ran every whatsit the page's decoding held, a
/// discretionary's pre-break and post-break text's too: the .aux had a
/// mark at x 0 for each (oracle: pdflatex in the CI image, accl 22640).
#[test]
fn write_in_unbroken_discretionary() {
    check_aux("write_in_unbroken_discretionary");
    check_threads("write_in_unbroken_discretionary", "pdf");
}

/// TeX Live manuals hfutthesis, zhlineskip, highlightx, caspervector
/// (xelatex): a document line's comment read as text (verbatim's `%` is
/// no comment) is spliced back from the canonical text's mark as the
/// file's bytes; under XeTeX they are decoded in the file's input mode as
/// the line around them was. They were the bytes' characters ("Missing
/// character: There is no ^^80"). At -j1 and -j4 (oracle: xelatex in the
/// CI image, accl 22729; the base differs).
#[test]
fn xe_verbatim_comment() {
    check_threads("xe_verbatim_comment", "pdf");
}

/// TeX Live manual tkz-tab (xelatex): etoc reads the .toc with `\read`;
/// XeTeX's `input_line` decodes a `\read` or `\readline` line from UTF-8
/// (the input mode `\XeTeXdefaultencoding` named when the stream
/// opened), and the line's bytes were its characters: "DÃ©finition" in
/// the contents. At -j1 and -j4 (oracle: xelatex in the CI image, accl
/// 22748; the base differs).
#[test]
fn xe_read_utf8() {
    check_threads("xe_read_utf8", "pdf");
}
repro!(
    box_registers_past_255,
    "manuals (milsymb: forest's directory tree drawn without its node texts): \\setbox<n> past 255 was read with TeX82's box contexts (global_box_flag box_flag+256, ship_out_flag box_flag+512), so \\setbox300 set register 44 globally and \\setbox600 shipped its box; e-TeX's are box_flag+32768 and box_flag+65536 (etex.ch §1071). forest's node boxes are \\locbox registers counted down from 32767 (oracle: pdflatex in the CI image, accl 22653)"
);

/// TeX Live manual sample-polski (pdflatex): its first line is
/// `%& --translate-file=il2-pl`, a TCX web2c's `parse_first_line` reads
/// and pdfTeX applies: each byte of a file's line through its `xord` (the
/// ISO-8859-2 text in T1's codes), each written character through its
/// `xchr` where `xprn` says it prints, else in `^^` form, read back
/// through `xord` (the .toc, a `\read`). The TCX was not read, the bytes
/// were their own characters ("tre\xb6ci" for "treści"). At -j1 and -j4
/// (oracle: pdftex in the CI image, accl 22852; the base differs).
#[test]
fn tcx_first_line() {
    check_threads("tcx_il2pl", "pdf");
}

/// TeX Live manual beautybook-en (pdflatex): a PNG with an alpha channel
/// shown on a page; pdfTeX writes its soft mask after the page (an object
/// of its own) and its transparency group, numbered when it was read,
/// into the document's object stream. The image's own writer, measuring
/// what writing it makes, counted the object stream it opened for the
/// group as an object: every read-time number after the page was one
/// past pdfTeX's (pgf's `/pgfsmask146` for 144; here the `\pdflastobj`
/// typeset). At -j1 and -j4 (oracle: pdftex in the CI image, accl 22935;
/// the base differs).
#[test]
fn png_alpha_objects_counted() {
    check_threads("png_alpha_count", "pdf");
}

/// manuals (spath3: pgf's `\immediate\pdfobj` numbered 404 for pdfTeX's
/// 429, the 25 objects of the page firing before it missing; tex-vpat):
/// an object number read inside a conditional the text's run opens,
/// after a candidate inside it where a page fires. No pass of the run
/// could end at a paragraph's start or a paragraph's end inside its own
/// conditionals, so the number missed that page's objects
/// (`PAGES_OBJECTS_DEFECT`). Now, where a page fires at one of its
/// candidates, the run ends at the first such candidate a second pass
/// can end at, the continuation carrying the conditionals
/// (`Run::order_after_pages`; oracle: pdftex in the CI image, accl 23046,
/// 23055).
#[test]
fn pages_objects_in_conditional() {
    check_threads("pages_objects_in_conditional", "pdf");
}

/// TeX Live manual huaz (pdflatex): the text analysis took `\lstinline`
/// in `\newcommand{\code}{\lstinline}` for a verbatim run and cut the
/// body into pieces of a few tokens, one cut inside the word `''.` that
/// `\huqq{az asztal}.` makes. A run ending its piece inside a character
/// run (§1038's lookahead at the end of the node's text) ended the word
/// there, as if no character followed, and read the next piece on only
/// after: pdfTeX's kern between `''` (the ligature) and `.` was missing.
/// The run now reads the next piece on inside the lookahead, so a cut the
/// analysis put inside a word costs nothing. At -j1 and -j4 (oracle:
/// pdftex in the CI image, accl 23105; the base differs).
#[test]
fn word_across_cut() {
    check_threads("word_across_cut", "pdf");
}

/// TeX Live manual ryesample (pdflatex): the glossaries package's lists,
/// `STEM.glo` and `STEM.acn`, made into `STEM.gls` and `STEM.acr` by
/// makeindex in the style `STEM.ist` the package writes, as the corpus
/// loop runs them (`makeglossaries`'s calls; latexmk with that custom
/// dependency). No tool claimed them: the glossaries were empty and the
/// pages two short. At -j1 and -j4 (oracle: pdflatex and makeindex in
/// the CI image, latexmk with the loop's rule, accl 23161; the base
/// differs).
#[test]
fn glossaries_made() {
    check_threads("glossaries_tool", "pdf");
}

/// TeX Live manual ibycus-babel-test (pdflatex): ibyhyph.tex loads its
/// patterns with e-TeX's `\savinghyphcodes=1` after making the accent
/// characters (parentheses, quotes, `=`, `+`, `|`) letters by `\lccode`; e-TeX hyphenates an ibycus word with those
/// saved codes in the `\lccode`s' place (`store_hyph_codes`,
/// `set_lc_code`), the document's `\lccode`s of the accents being 0.
/// The codes were not saved: no ibycus word was hyphenated. At -j1 and
/// -j4 (oracle: pdflatex in the CI image, accl 23201; the base differs).
#[test]
fn saved_hyph_codes() {
    check_threads("saved_hyph_codes", "pdf");
}

/// manual subfigure's test at -j4 (a page shipped twice, the pages after
/// it numbered one higher, 6 of 8 runs): the lists of figures and tables
/// grow by a page in the second evaluation, the loop node that fired the
/// List of Tables' first page fires the next one itself, and its old rest,
/// woken by a hole filling before its start's change, folded on from its
/// parked tail and fired that page again. A rest whose start moved folds
/// from the new one, whatever woke it (`LoopState::began`; oracle:
/// pdflatex in the CI image, accl 23217: the base differs in 5 of 10 runs
/// at -j4).
#[test]
fn rest_start_moved() {
    check_threads("rest_start_moved", "pdf");
}

/// TeX Live manual focus-demo (xelatex, beamer): `\pdflastypos` is
/// measured from the page's top, the page's height being `\pdfpageheight`
/// when it is not zero, in XeTeX and in pdfTeX's DVI mode alike ("Calculate
/// DVI page dimensions and margins"); beamer sets it. The height was the
/// shipped box's with twice the offsets and the DVI origin, so tikz's
/// `remember picture` overlays were drawn 1in off. At -j1 and -j4
/// (oracle: xelatex in the CI image, accl 23258; the base differs).
#[test]
fn savepos_pageheight() {
    check_threads("savepos_pageheight", "pdf");
}

/// TeX Live manual snaptodo (pdflatex): `\pdflastxpos` past an unbroken
/// discretionary with text of its own (a hyphenated word's ligature or
/// kern, an explicit `\discretionary`): pdfTeX ships that text in the
/// line, tex.web's `replace_count` nodes after the discretionary, and the
/// positions' fold passed over it, so a later `\pdfsavepos` in the line was
/// short by its width (snaptodo's second note 4.72pt off). At -j1 and -j4
/// (oracle: pdftex in the CI image, accl 23274; the base differs).
#[test]
fn savepos_disc() {
    check_threads("savepos_disc", "pdf");
}

/// TeX Live manual pinlabdoc (pdflatex): pinlabel reads its figure's size
/// with `\pdfximagebbox`, an included PDF page's box (pdfTeX's
/// `pdf_ximage_bbox_code`: its origin, and that plus the page's size);
/// the image's record held the bitmap size `image_info` reads, 0 for a
/// PDF, so every `\pinlabel` was placed on a figure of no size. At -j1
/// and -j4 (oracle: pdftex in the CI image, accl 23321; the base differs).
#[test]
fn ximage_bbox() {
    check_threads("ximage_bbox", "pdf");
}

/// The document's text read to its end with no `\end` (an example
/// environment left open swallows `\end{document}`, as the biblatex manual
/// cut inside an `ltxexample`), under XeLaTeX: "! Emergency stop. ***
/// (job aborted, no legal \end found)", and the page shipped before is
/// xelatex's PDF. Was no output file ("the pages' writers ran before every
/// page had finished"): the last piece ended as one whose text goes on,
/// and the main list never ended. At -j1 and -j4 (oracle: xelatex in the
/// CI image, accl 23340).
#[test]
fn text_ends_without_end_xe() {
    let name = "text_ends_without_end_xe";
    let want = std::fs::read(fixtures().join(format!("{name}.pdf"))).expect("the oracle");
    for j in [1, 4] {
        let (got, term) = build_j(name, j);
        assert!(
            got == want,
            "{name} -j{j}: the PDF differs from xelatex's ({} vs {} bytes): {}",
            got.len(),
            want.len(),
            term.lines().last().unwrap_or("")
        );
    }
}
