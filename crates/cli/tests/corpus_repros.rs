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
    let pdf = std::fs::read(dir.join(format!("{name}.pdf"))).unwrap_or_default();
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
/// whose header reads and whose data does not): the job's error at every
/// thread count, on a page the link places whole as on one it ships
/// again (pdftex: `!pdfTeX error: pdftex (file ./bad.png): libpng:
/// internal error`, fatal, mc 77541). The link placing a page whole
/// dropped the error (`PdfState::replay_page`); this page, its image's
/// number read-time, is shipped again, which reported it already.
#[test]
fn pdf_image_write_error() {
    for j in [1, 4] {
        let (_, term) = build_j("pdf_image_write_error", j);
        assert!(
            term.lines()
                .any(|l| l.starts_with("error") && l.contains("bad.png: libpng: internal error")),
            "-j{j}: no error for the image:\n{term}"
        );
    }
}

repro!(
    pdfthread_in_hlist,
    "\\pdfthread in a paragraph: the command's kind was read after its specification was scanned, and \\pdfthread was taken for \\pdfstartthread ('\\pdfstartthread ended up in hlist')"
);

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
