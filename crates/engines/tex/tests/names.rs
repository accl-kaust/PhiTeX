//! The process's table of control sequence names (`val::CsId`): what a
//! long session interns, and what an internal name past its range is.
//! A program of its own: the table is the process's, and the counts are
//! read with no other test's builds beside them (one test at a time,
//! [`ONE`]).

mod common;

use std::path::{Path, PathBuf};

use phitex_tex::Options;

/// The tests one at a time: each reads or fills the process's table.
static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn prelude(_pdf: bool) -> String {
    "\\input pdflatex.ini\n".to_owned()
}

/// pdflatex's options, the format cached under `target/cache` as the
/// latex tests keep it.
fn pdf_opts(name: &str, dir: &Path) -> Options {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/cache/pdflatex.fmt");
    Options {
        epoch: Some(0),
        prelude_jobname: "pdflatex".to_owned(),
        prelude_bytes: std::fs::read(cache).ok(),
        ..common::options(&format!("./{name}.tex"), name, "", dir)
    }
}

/// An internal name met once the process has numbered more internal
/// names than their range holds is still frozen: the end of an alignment
/// template (`\u{10FFFE}endv:<char>`, made where the template's tab is
/// first met) ends the cell. Its frozenness was its number's, and a
/// session that had numbered 32768 internal names first (a symbol per
/// deferred counter read, per build) took the end for an undefined
/// control sequence: "Undefined control sequence", then "Missing \cr
/// inserted" a hundred times and no PDF (#32).
#[test]
fn an_internal_name_numbered_past_its_range_is_still_frozen() {
    let _one = ONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-frozen-past");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for i in 0..40_000 {
        phitex_tex::intern_name(&format!("\u{10FFFE}test-past-range\u{2}{i}"));
    }
    // (`!` a tab: its template end, `endv:33`, first made here)
    let doc = "\\documentclass{article}\n\\begin{document}\n\\catcode`\\!=4\n\\halign{#\\hfil!\\hfil#\\cr a!b\\cr c!d\\cr}\n\\begin{tabular}{cc}e!f\\\\ g!h\\end{tabular}\n\\end{document}\n";
    let b = common::build(pdf_opts("past", &dir), Some(&prelude(true)), doc);
    let errors: Vec<String> = b
        .diagnostics()
        .into_iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!b.output().0.is_empty(), "no PDF");
}

/// A session's edits mint no control sequence names once their texts
/// repeat: amsldoc built once, then the same one-word edits made again
/// and again in one process (a watch's, the language server's). Names
/// live as long as the process, so what a session interns must stop
/// growing: a name minted per build for each deferred counter's symbol
/// (`\u{10FFFE}sym…`, by register, node and steps) filled the internal
/// names' range in a long run, and the alignments after it broke (#32).
#[test]
fn a_session_of_repeated_edits_interns_no_new_names() {
    let _one = ONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../bench/amsldoc");
    // (the process's own: copies of this test run at once on a loaded
    // machine, each with its own table)
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("amsldoc-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::copy(src_dir.join("amsldoc.tex"), dir.join("amsldoc.tex")).expect("the source");
    let src = common::read(&dir.join("amsldoc.tex"));
    let opts = Options {
        threads: Some(4),
        ..pdf_opts("amsldoc", &dir)
    };
    let mut b = common::build(opts, Some(&prelude(true)), &src);
    // (one-word edits at a few places, each put back after)
    let words = [
        "pagebreak after a given line",
        "the document class",
        "a numbered equation",
    ];
    let edits: Vec<String> = words
        .iter()
        .filter(|w| src.contains(*w))
        .map(|w| src.replacen(w, &w.replacen(' ', " wurds ", 1), 1))
        .collect();
    assert!(!edits.is_empty(), "no edit place found");
    let round = |b: &mut phitex_tex::Build| {
        for e in &edits {
            b.edit(e);
            b.edit(&src);
        }
    };
    for _ in 0..3 {
        round(&mut b);
    }
    let warm = phitex_tex::interned_names();
    // (`PHITEX_SESSION_ROUNDS`: more rounds, a measure of a long session)
    let rounds: usize = std::env::var("PHITEX_SESSION_ROUNDS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(20);
    for _ in 0..rounds {
        round(&mut b);
    }
    let after = phitex_tex::interned_names();
    eprintln!(
        "interned names (internal, other): after warm-up {warm:?}, after {} more edits {after:?}",
        rounds * 2 * edits.len()
    );
    assert_eq!(
        after.0, warm.0,
        "internal names minted by edits that repeat"
    );
    // (and builds of it again from nothing, as a session opens it again.
    // The symbols are numbered per engine and per kind, a symbol's name
    // its kind's and its number's: a build adds names only where it
    // numbers more symbols of a kind than any engine before it. How many
    // a build numbers is its runs', a run thrown away numbering its own
    // too, so it moves with the machine's load (amsldoc 842 to 1673 in one
    // -j8 build on a loaded node): the measure is what the names are, not
    // how many. A name per register, piece and steps added some 400 a
    // build and filled the range (#32))
    let mut most = b.symbols_numbered();
    let mut cold = vec![after];
    let mut numbered = Vec::new();
    for threads in [8, 4, 8, 1, 8, 4, 8, 1, 8, 4] {
        let opts = Options {
            threads: Some(threads),
            ..pdf_opts("amsldoc", &dir)
        };
        let c = common::build(opts, Some(&prelude(true)), &src);
        let n = c.symbols_numbered();
        most = (most.0.max(n.0), most.1.max(n.1));
        numbered.push((threads, n));
        drop(c);
        cold.push(phitex_tex::interned_names());
    }
    eprintln!(
        "interned names after cold builds at -j8/4/8/1/8/4/8/1/8/4: {cold:?}; symbols numbered (registers, names) by each: {numbered:?}"
    );
    // (each engine numbers its own from nothing: one thread runs nothing
    // ahead, so its builds number the same)
    let one: Vec<(u32, u32)> = numbered
        .iter()
        .filter(|(t, _)| *t == 1)
        .map(|(_, n)| *n)
        .collect();
    assert!(
        one.windows(2).all(|w| w[0] == w[1]),
        "builds at one thread numbered different symbols: {numbered:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let minted = phitex_tex::internal_names_from(after.0);
    let other: Vec<String> = minted
        .iter()
        .filter(|n| !bounded_name(n, most))
        .map(|n| n.replace('\u{10FFFE}', "^").replace('\u{2}', "|"))
        .collect();
    assert!(
        other.is_empty(),
        "the cold builds minted {} internal names, {} of them no symbol below the most numbered ({most:?}), font identifier or fixed name: {other:?}",
        minted.len(),
        other.len()
    );
}

/// Whether internal name `n` is one a build may first meet after others
/// of the session did not: a deferred counter's symbol numbered below
/// `most` (the most register symbols and derived names an engine
/// numbered), a font's identifier (§1257: one per font and name), or a
/// fixed one, letters only (`\u{10FFFE}cr`, a frozen primitive's copy).
/// A run thrown away may meet what TeX's order does not: a font loaded at
/// a size the guess it ran on gave, "Missing \cr inserted" (the frozen
/// `\cr`, 58 of 104 loaded runs on mc 87745). Each is one of a set the
/// document bounds, not one a build.
fn bounded_name(n: &str, most: (u32, u32)) -> bool {
    let Some(r) = n.strip_prefix('\u{10FFFE}') else {
        return false;
    };
    if !r.is_empty() && r.chars().all(|c| c.is_ascii_alphabetic()) {
        return true;
    }
    if let Some(i) = r.strip_prefix("sym\u{2}dn\u{2}") {
        return i.parse::<u32>().is_ok_and(|i| i < most.1);
    }
    if let Some(i) = r.strip_prefix("sym\u{2}") {
        return i.parse::<u32>().is_ok_and(|i| i < most.0);
    }
    r.strip_prefix('F')
        .and_then(|x| x.split_once('\u{2}'))
        .is_some_and(|(k, _)| k.parse::<u32>().is_ok())
}
