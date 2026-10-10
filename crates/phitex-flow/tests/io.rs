//! Input and output on the graph, where TeX's log is no oracle (errors) or
//! where what matters is what an edit runs again: a file is a read of
//! `Key::File`, the lines before a piece a read of `Key::LineBase`, the
//! job's name a read of `Key::Job` — each answered by the build, each
//! re-running exactly its readers when it changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phitex_flow::{Build, Font, Options, Output};

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\hsize=468pt \\vsize=643pt \
\\parindent=20pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 %\n";

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn core_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/core")
}

fn build_in(dir: &Path, src: &str) -> Output {
    let mut b = Build::new(font());
    b.set_input_dirs(vec![dir.to_path_buf()]);
    b.update(&format!("{PRELUDE}{src}"), Options::default())
}

/// A directory of its own for a test's files.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("phitex-flow-io-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The errors TeX gives (in its log, with their context: no oracle here)
/// are the graph's errors too.
#[test]
fn io_errors_are_reported() {
    let cases = [
        ("\\openin1=unbalanced.dat \\read1 to\\x", "File ended within \\read"),
        ("\\openin1=lines.dat \\read1 to x", "Missing control sequence inserted"),
        ("\\read1 to\\x", "cannot \\read from terminal"),
        ("\\setbox1\\hbox{}\\setbox0\\vsplit1 to 10pt", "\\vsplit needs a \\vbox"),
        ("\\setbox1\\vbox{}\\setbox0\\vsplit1 10pt", "Missing `to' inserted"),
        ("\\leaders\\hrule\\par", "Leaders not followed by proper glue"),
        ("\\setbox0\\vbox{\\leaders\\hbox{}\\hfil}", "Leaders not followed by proper glue"),
        ("\\insert255{}", "You can't \\insert255"),
        ("\\vadjust{}", "You can't use `\\vadjust' in vertical mode"),
        ("\\setbox0\\lastbox", "You can't use `\\lastbox' in vertical mode"),
        ("\\font\\x=cmr10 \\x \\hbox{\\accent'177\\setbox0\\hbox{} a}", "Improper \\setbox"),
        ("\\input nosuchfile ", "I can't find file `nosuchfile'"),
        ("\\begingroup\\dump", "You can't dump inside a group"),
    ];
    for (src, want) in cases {
        let o = build_in(&core_dir(), src);
        assert!(o.errors.iter().any(|e| e.contains(want)), "{src:?}: no {want:?} in {:?}", o.errors);
    }
    // (the macro `\read` defines when it was given no name, and the line)
    let o = build_in(&core_dir(), "\\openin1=lines.dat \\read1 to\\x \\message{\\meaning\\x}");
    assert_eq!(o.log, ["macro:->line one "]);
}

/// A changed input file runs again exactly the pieces reading it (the
/// `\input` node's, the file's own); the output is a cold build's.
#[test]
fn an_edited_input_file_reruns_its_readers() {
    let dir = scratch("edit");
    std::fs::write(dir.join("inc.tex"), "\\message{one}\n").unwrap();
    let src = format!("{PRELUDE}\\message{{a}}\n\n\\input inc \\message{{after}}\n\n\\message{{other}}\n");
    let opts = Options::default();
    let mut warm = Build::new(font());
    warm.set_input_dirs(vec![dir.clone()]);
    let first = warm.update(&src, opts);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert!(first.log.iter().any(|x| x == "one"), "{:?}", first.log);
    std::fs::write(dir.join("inc.tex"), "\\message{two}\n").unwrap();
    let edited = warm.update(&src, opts);
    let mut fresh = Build::new(font());
    fresh.set_input_dirs(vec![dir.clone()]);
    let cold = fresh.update(&src, opts);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(edited.log.iter().any(|x| x == "two"), "{:?}", edited.log);
    assert_eq!(edited.log, cold.log);
    // (the `\input` node reading the file, and the file's piece)
    assert_eq!(edited.stats.ran, 2, "only the pieces reading the file run: {:?}", edited.stats);
}

/// `\inputlineno` reads the lines before its piece: lines added above it
/// run that node again, and give a cold build's log.
#[test]
fn inputlineno_follows_the_lines_above() {
    let a = format!("{PRELUDE}\\message{{a}}\n\n\\message{{x}}\\message{{\\the\\inputlineno}}\n");
    let edited = a.replacen("\\message{a}\n", "\\message{a}\n\\message{b}\n\n", 1);
    let o = Options::default();
    let mut b = Build::new(font());
    let first = b.update(&a, o);
    assert_eq!(first.log, ["a", "x", "4"]);
    let e = b.update(&edited, o);
    let cold = Build::new(font()).update(&edited, o);
    assert_eq!(e.log, cold.log);
    assert_eq!(e.log, ["a", "b", "x", "6"]);
}

/// The job's name is a read too.
#[test]
fn jobname_is_a_read() {
    let src = format!("{PRELUDE}\\message{{\\jobname}}\n\n\\message{{x}}\n");
    let o = Options::default();
    let mut b = Build::new(font());
    assert_eq!(b.update(&src, o).log, ["texput", "x"]);
    b.set_jobname("doc");
    let e = b.update(&src, o);
    assert_eq!(e.log, ["doc", "x"]);
    assert_eq!(e.stats.ran, 1, "{:?}", e.stats);
}

/// Deferred `\write`s are expanded where their page is shipped, also after
/// an edit that moves the page's call into another piece.
#[test]
fn deferred_writes_after_edits_are_a_cold_builds() {
    let doc = |extra: &str| {
        format!(
            "{PRELUDE}\\font\\tenrm=cmr10 \\tenrm \\vsize=40pt \\maxdepth=2pt\n\\openout1=a.aux\n\n{extra}\
             \\def\\x{{one}}Text\\write1{{\\x\\space\\the\\count0}}\\par\n\n\\def\\x{{two}}\\count0=4 \\penalty-10000\n\n\
             \\def\\x{{three}}More\\write1{{\\x}}\\par\\end\n"
        )
    };
    let a = doc("");
    let edited = doc("\\def\\space{ }\n\n");
    let o = Options::default();
    let mut b = Build::new(font());
    let first = b.update(&a, o);
    let e = b.update(&edited, o);
    let cold = Build::new(font()).update(&edited, o);
    assert_eq!(e.writes, cold.writes);
    assert_eq!(e.log, cold.log);
    assert_eq!(cold.writes, [("a.aux".to_string(), "two 4\nthree\n".to_string())]);
    assert_ne!(first.writes, cold.writes);
}
