//! `\input` (CORE.md, decided): a file's pieces follow the `\input` node in
//! the sequence, the first entered with the state the node left, the rest
//! of the calling line with the state the file's last piece left. Each
//! document runs against `tex -ini` (where there is one: the DVI file), and
//! each edit — in the main text, in a file, one adding an `\input`, one
//! flipping an `\endinput` — gives what a cold build gives (DVI and log),
//! at 1 and 8 threads.

// (a DVI file is compared whole; `assert_eq!` would print its bytes)
#![allow(clippy::manual_assert_eq)]

use std::process::Command;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options, Reads};

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

const PRE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
\\font\\rm=cmr10 \\font\\bf=cmbx10 \\rm \\hsize=250pt \\vsize=150pt \\parindent=15pt \\baselineskip=12pt\n\
\\topskip=10pt \\maxdepth=2pt \\tolerance=10000 \\hbadness=10000 \\vbadness=10000 \\parfillskip=0pt plus 1fil\n\n";

fn para(words: &str, n: usize) -> String {
    let w: Vec<&str> = words.split_whitespace().collect();
    let mut s = String::new();
    for i in 0..n {
        s.push_str(w[(i * 7 + n) % w.len()]);
        s.push(' ');
    }
    s.push_str(".\n\n");
    s
}

const WORDS: &str = "a file read in the middle of a document goes on where the calling line stops and its pieces run like the others";

/// The main text: a paragraph, `\input sub` in the middle of a line, a
/// paragraph reading what the file defined.
fn main_text(extra: &str) -> String {
    format!(
        "{PRE}{}\\message{{A}}\\input sub \\message{{B}}\\def\\after{{after}}\n\n{}{extra}The file said \\fromsub, and \\after.\n\n{}\\end\n",
        para(WORDS, 40),
        para(WORDS, 30),
        para(WORDS, 50)
    )
}

fn sub(word: &str) -> String {
    format!(
        "\\def\\fromsub{{{word}}}\n\n{}\\message{{in sub}}\\input subsub \\message{{after subsub}}\n\n{}\\ifx\\stop\\relax \\endinput\\fi\n\n{}\\message{{end of sub}}\n",
        para(WORDS, 33),
        para(WORDS, 21),
        para(WORDS, 27)
    )
}

fn subsub() -> String {
    format!("{}\\def\\deep{{deep}}\\message{{\\deep}}\n", para(WORDS, 19))
}

fn extra() -> String {
    format!("{}\\message{{extra}}\n", para(WORDS, 15))
}

/// TeX's DVI file for these files (the first is the main one), if there is
/// a `tex`.
fn tex(name: &str, files: &[(&str, String)]) -> Option<Vec<u8>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("phitex-flow-input-{name}-{}-{call}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for (f, text) in files {
        std::fs::write(tmp.join(f), text).ok()?;
    }
    let st = Command::new("tex")
        .args(["-ini", "-interaction=batchmode", files[0].0])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let job = files[0].0.trim_end_matches(".tex");
    let dvi = std::fs::read(tmp.join(format!("{job}.dvi"))).ok();
    let log = std::fs::read_to_string(tmp.join(format!("{job}.log"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(st.status.success(), "tex failed on {name}:\n{log}");
    dvi
}

/// A DVI file with its preamble's comment masked.
fn masked(dvi: &[u8]) -> Vec<u8> {
    let mut d = dvi.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'?';
        }
    }
    d
}

/// A version the edits go through: its name, the main text, the files.
type Version = (&'static str, String, Vec<(&'static str, String)>);

/// The versions the edits go through.
fn versions() -> Vec<Version> {
    let files = |w: &str, more: bool| {
        let mut v = vec![("sub.tex", sub(w)), ("subsub.tex", subsub())];
        if more {
            v.push(("extra.tex", extra()));
        }
        v
    };
    vec![
        ("cold", main_text(""), files("the file", false)),
        ("file edited", main_text(""), files("the edited file", false)),
        ("input added", main_text("\\input extra \n\n"), files("the edited file", true)),
        ("endinput", main_text("").replace("\\message{A}", "\\let\\stop=\\relax \\message{A}"), files("the edited file", true)),
        ("endinput gone", main_text(""), files("the edited file", true)),
    ]
}

fn build_with(files: &[(&str, String)]) -> Build {
    let mut b = Build::new(font());
    for (n, t) in files {
        b.set_file(n, t);
    }
    b
}

#[test]
fn input_is_tex_s() {
    for (name, main, files) in versions() {
        let out = build_with(&files).update(&main, Options::default());
        assert!(out.errors.is_empty(), "{name}: {:?}", out.errors);
        let mut all = vec![("main.tex", main.clone())];
        all.extend(files.iter().map(|(n, t)| (*n, t.clone())));
        if let Some(want) = tex(&name.replace(' ', "-"), &all) {
            assert!(masked(&out.dvi) == masked(&want), "{name}: not TeX's DVI ({} bytes, TeX {})", out.dvi.len(), want.len());
        }
        let log = phitex_flow::print::log("", &out.log, "");
        for m in ["A", "(./sub.tex", "in sub", "(./subsub.tex", "deep", "after subsub", "B"] {
            assert!(log.contains(m), "{name}: the log lacks {m:?}:\n{log}");
        }
        assert_eq!(log.contains("end of sub"), !name.starts_with("endinput") || name.ends_with("gone"), "{name}: {log}");
        // (each file is pieces of the sequence, not read in its caller:
        // the main text's pieces, the file's up to its `\endinput`, and a
        // rest of the line for each `\input`)
        let n = |t: &str| phitex_flow::graph::cut(t).len();
        let sub = files.iter().find(|f| f.0 == "sub.tex").map_or("", |f| f.1.as_str());
        let sub_read = if name == "endinput" { n(sub) - 2 } else { n(sub) };
        let inputs = 2 + usize::from(name == "input added");
        let extra = if name == "input added" { n(&extra()) } else { 0 };
        let want = n(&main) + sub_read + n(&subsub()) + extra + inputs;
        assert_eq!(out.stats.pieces, want, "{name}: pieces");
    }
}

/// An `\input` met in a macro's expansion, a conditional's arm or with
/// tokens pending after it is a piece too: the tokens pending and the rest
/// of the line are read after the file (the rest's piece), the
/// conditional's `\fi` there.
#[test]
fn input_in_a_macro_and_a_conditional_is_pieces() {
    let file = format!("{}\\message{{in f}}\n\n{}\\message{{end of f}}\n", para(WORDS, 23), para(WORDS, 17));
    let main = format!(
        "{PRE}\\def\\load#1{{\\iftrue \\input #1 \\message{{pending}}\\fi}}\n\n{}\\load{{f}}\\message{{rest}}\n\n{}\\end\n",
        para(WORDS, 20),
        para(WORDS, 31)
    );
    let files = [("f.tex", file.clone())];
    for threads in [1, 8] {
        for reads in [Reads::Suspend, Reads::Speculate] {
            let o = Options { threads, reads, ..Options::default() };
            let out = build_with(&files).update(&main, o);
            assert!(out.errors.is_empty(), "{:?}", out.errors);
            let log = phitex_flow::print::log("", &out.log, "");
            let at = |m: &str| log.find(m).unwrap_or_else(|| panic!("the log lacks {m:?}:\n{log}"));
            assert!(at("(./f.tex") < at("in f") && at("in f") < at("end of f") && at("end of f") < at("pending") && at("pending") < at("rest"), "{log}");
            let n = |t: &str| phitex_flow::graph::cut(t).len();
            assert_eq!(out.stats.pieces, n(&main) + n(&file) + 1, "pieces");
            let mut all = vec![("main.tex", main.clone())];
            all.push(("f.tex", file.clone()));
            if let Some(want) = tex("macro", &all) {
                assert!(masked(&out.dvi) == masked(&want), "not TeX's DVI");
            }
        }
    }
}

/// A file whose end leaves a paragraph open (the rest of the calling line
/// goes on with it) is read in its caller: TeX's DVI, the file's text
/// once.
#[test]
fn a_file_ending_inside_a_paragraph_is_read_in_its_caller() {
    let file = format!("{}words at the end of the file", para(WORDS, 21));
    let main = format!("{PRE}{}\\input g and the caller's line.\n\n{}\\end\n", para(WORDS, 20), para(WORDS, 12));
    let files = [("g.tex", file.clone())];
    for threads in [1, 8] {
        let o = Options { threads, ..Options::default() };
        let out = build_with(&files).update(&main, o);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let mut all = vec![("main.tex", main.clone())];
        all.push(("g.tex", file.clone()));
        if let Some(want) = tex("open-par", &all) {
            assert!(masked(&out.dvi) == masked(&want), "not TeX's DVI");
        }
    }
}

#[test]
fn input_edits_are_cold_builds() {
    for threads in [1, 8] {
        for reads in [Reads::Suspend, Reads::Speculate] {
            let o = Options { threads, reads, ..Options::default() };
            let vs = versions();
            let mut b = build_with(&vs[0].2);
            let first = b.update(&vs[0].1, o);
            assert!(first.errors.is_empty(), "{:?}", first.errors);
            for (name, main, files) in &vs[1..] {
                for (n, t) in files {
                    b.set_file(n, t);
                }
                let e = b.update(main, o);
                let cold = build_with(files).update(main, o);
                assert!(e.errors.is_empty() && cold.errors.is_empty(), "{name}: {:?} {:?}", e.errors, cold.errors);
                assert!(e.log == cold.log, "{name} at {threads} threads ({reads:?}): another log\n{:?}\n{:?}", e.log, cold.log);
                assert!(e.dvi == cold.dvi, "{name} at {threads} threads ({reads:?}): another DVI");
                eprintln!("{name}, {threads} threads: {:?}, {} rounds", e.stats.wall, e.stats.rounds);
            }
        }
    }
}
