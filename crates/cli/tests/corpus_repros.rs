//! The real-document corpus's failure causes (scripts/corpus), each as a
//! minimal standalone document: `oracle/fixtures/corpus-repros/NAME.tex`,
//! built by `phitex build --exact-pdf` and compared byte for byte with
//! `NAME.pdf`, pdflatex's (pdfTeX 1.40.29 in the CI image, to its
//! fixpoint, `SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC`, as
//! scripts/ci/common.py pins them). Each test is ignored with its cause and
//! how many corpus documents it stops; un-ignore it with the fix.

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

macro_rules! repro {
    ($name:ident, $why:literal) => {
        #[test]
        #[ignore = $why]
        fn $name() {
            check(stringify!($name));
        }
    };
}

repro!(
    display_align,
    "corpus: \\halign in a display ($$\\halign$$: amsmath align/gather, eqnarray) is 'this alignment is not supported' (control.rs REDUCE-GAP), the first cause in 27 of 136 arXiv papers; ported on cuts"
);
repro!(
    row_starts_with_amp,
    "corpus: a tabular row that starts with & (an empty first cell) gives 'illegal command' / 'You can't use \\cr in restricted horizontal mode' and Extra alignment tab; amsldoc.tex:1211, arXiv a-2409.03762v1; owner cuts"
);
repro!(
    table_float_lost,
    "corpus: \\caption + \\centering + tabular in a table float: 'Float(s) lost' (b7646c77), first cause in 8 of 136 arXiv papers; owner pagechain (pgstfix on dev 301ef3af)"
);
