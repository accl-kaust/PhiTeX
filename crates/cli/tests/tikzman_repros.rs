//! Repros from the `TikZ` manual run (agent tikzman); those `#[ignore]`d
//! wait for the fix of the defect they show.

use std::path::PathBuf;
use std::process::Command;

/// A diagnostic whose source line has bytes >= 0x80 (a koi8-r `\input`
/// file, shown as Latin-1 chars) must render: the renderer is given byte
/// offsets into the line's UTF-8 text, so it panicked "end byte index 7
/// is not a char boundary; it is inside 'Ò'" (exit 101).
#[test]
fn diagnostic_on_latin1_line_does_not_panic() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tikzman-latin1-diag");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("k7in.tex"),
        b"\\catcode `\\\xc1=12 \\catcode `\\\xc2=12 \\catcode `\\\xd2=12\n\
          \\lccode `\\\xc1=224 \\lccode `\\\xc2=225 \\lccode `\\\xd2=240\n\
          \\patterns{\n.\xc1\xc21\xd2\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("k7.tex"),
        b"\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\\input k7in\n\\message{[done]}\n\\end\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phitex"))
        .args([
            "build",
            "--ini",
            "--etex",
            "--output-format=pdf",
            "-v",
            "--exact-pdf",
            "-j",
            "4",
            "k7.tex",
        ])
        .current_dir(&dir)
        .env("PHITEX_CACHE_DIR", dir.join("cache"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(101), "panicked:\n{err}");
    assert!(!err.contains("panic"), "panicked:\n{err}");
}

/// `phitex build` of `main` (with `files` beside it) in a scratch
/// directory: its exit status and its log.
fn build_latex(name: &str, main: &str, files: &[(&str, &str)]) -> (Option<i32>, String) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.tex"), main).unwrap();
    for (f, t) in files {
        std::fs::write(dir.join(f), t).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_phitex"))
        .args(["build", "--exact-pdf", "-j", "4", "main.tex"])
        .current_dir(&dir)
        .env(
            "PHITEX_CACHE_DIR",
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tikzman-cache"),
        )
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let log = std::fs::read(dir.join("main.log"))
        .map(|b| b.iter().map(|&c| char::from(c)).collect())
        .unwrap_or_default();
    (out.status.code(), log)
}

const CHAPTER: &str = "Chapter text. Some words in a paragraph here.\n\nA second paragraph.\n";

/// `\include` of a one-paragraph file: pdflatex reads on after it (no
/// error, two pages); phitex stopped at the first one's end with "! ***
/// (job aborted, no legal \end found)." (the `TikZ` manual: after
/// `pgfmanual-en-introduction`).
#[test]
fn include_reads_on() {
    let (code, log) = build_latex(
        "tikzman-include",
        "\\documentclass{article}\n\\begin{document}\n\\include{ia}\n\\include{ib}\n\\end{document}\n",
        &[("ia.tex", CHAPTER), ("ib.tex", CHAPTER)],
    );
    assert!(
        !log.contains("\n! "),
        "errors where pdflatex has none:\n{log}"
    );
    assert_eq!(code, Some(0), "{log}");
}

/// `\include` with hyperref and a table of contents: also an evaluation
/// error, "conflicting publications for Field(..)" (`ConflictingPublication`).
#[test]
fn include_with_hyperref_toc() {
    let (code, log) = build_latex(
        "tikzman-include-hyperref",
        "\\documentclass{article}\n\\usepackage{hyperref}\n\\begin{document}\n\\tableofcontents\n\\section{One}\n\\include{ia}\n\\include{ib}\n\\end{document}\n",
        &[("ia.tex", CHAPTER), ("ib.tex", CHAPTER)],
    );
    assert!(
        !log.contains("\n! "),
        "errors where pdflatex has none:\n{log}"
    );
    assert_eq!(code, Some(0), "{log}");
}

/// The smallest form of the `\include` defect: an `.aux` file whose
/// `\@setckpt{..}{` line is closed by the next line (as `\@writeckpt`
/// writes it), read back through the main `.aux`. Read a line at a time,
/// the first line runs out of input and the job aborts.
#[test]
fn aux_group_across_lines() {
    let (code, log) = build_latex(
        "tikzman-ckpt",
        "\\documentclass{article}\n\\begin{document}\n\\makeatletter\n\
         \\immediate\\openout\\@partaux \"x.aux\"\n\
         \\immediate\\write\\@partaux{\\string\\@setckpt{x}\\@charlb}\n\
         \\immediate\\write\\@partaux{\\@charrb}\n\
         \\immediate\\closeout\\@partaux\n\
         \\immediate\\write\\@mainaux{\\string\\@input{x.aux}}\n\
         \\makeatother\nx\n\\end{document}\n",
        &[],
    );
    assert!(
        !log.contains("\n! "),
        "errors where pdflatex has none:\n{log}"
    );
    assert_eq!(code, Some(0), "{log}");
}

/// Hint-quality diagnostics (undeclared definitions, a hint's regime
/// violated) never fail a build: `envs.tex` of the LaTeX fixtures, whose
/// runs report undeclared definitions, exits 0 with its PDF.
#[test]
fn engine_diagnostics_do_not_fail_the_build() {
    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../oracle/fixtures/latex/pdfdocs/envs.tex"),
    )
    .unwrap();
    let (code, log) = build_latex("tikzman-envs", &src, &[]);
    assert!(
        !log.contains("\n! undeclared"),
        "diagnostics in the log:\n{log}"
    );
    assert!(
        !log.contains("\n! hint regime"),
        "diagnostics in the log:\n{log}"
    );
    // (exit 0 when TeX itself has no error: "Float(s) lost" in envs.tex on
    // dev at the time is another defect's)
    if !log.contains("\n! ") {
        assert_eq!(code, Some(0), "{log}");
    }
    let pdf = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tikzman-envs/main.pdf");
    assert!(pdf.is_file(), "no PDF");
}
