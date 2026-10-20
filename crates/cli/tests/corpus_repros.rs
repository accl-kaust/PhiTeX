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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("corpus-repros")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let tex = format!("{name}.tex");
    std::fs::copy(fixtures().join(&tex), dir.join(&tex)).expect("the repro");
    let out = Command::new(env!("CARGO_BIN_EXE_phitex"))
        .args(["build", "-v", "--exact-pdf", "-j", "1", &tex])
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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("corpus-repros")
        .join(name);
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
