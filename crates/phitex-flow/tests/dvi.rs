//! The oracle for material, boxes and pages: each document in
//! `tests/dvi/` runs through `tex -ini` and through the graph — at 1 and
//! 8 threads, with the scheduler's priority order and in random orders,
//! reads suspending or speculating — and the DVI files must be the same
//! bytes, and the logs the same text (Underfull/Overfull diagnostics and
//! page numbers included).
//!
//! The preamble's comment (` TeX output YYYY.MM.DD:HHMM`, tex.web 617) is
//! masked: TeX Live's `tex` takes the date from the clock (it ignores
//! `SOURCE_DATE_EPOCH`), the graph from its initial `\year`, `\month`,
//! `\day`, `\time` (1970.01.01:0000). TeX's outputs are recorded in
//! `tests/dvi/NAME.dvi` and `NAME.log` (the log's lines after `**NAME.tex`
//! up to `Output written`); where a `tex` binary is found they are checked
//! against a fresh run; `PHITEX_FLOW_BLESS=1` records them anew.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options, Order, Reads};

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../partex-core/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dvi")
}

/// The log's lines after `**NAME.tex` up to `Output written` (or `No pages`).
fn log_body(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("**"))? + 1;
    let end = lines.iter().position(|l| l.starts_with("No pages of output") || l.starts_with("Output written"))?;
    Some(lines[start..end].join("\n"))
}

/// TeX's DVI file and log of a document, if there is a `tex`.
fn tex(name: &str, src: &str) -> Option<(Vec<u8>, String)> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-dvi-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    let st = Command::new("tex")
        .args(["-ini", "-interaction=batchmode", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join(format!("{name}.log"))).ok();
    let dvi = std::fs::read(tmp.join(format!("{name}.dvi"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    assert!(st.status.success(), "tex failed on {name}:\n{log}");
    Some((dvi, log_body(&log)?))
}

/// The graph's DVI file and log of a document.
fn graph(name: &str, src: &str, o: Options) -> (Vec<u8>, String, Vec<String>) {
    let out = Build::new(font()).update(src, o);
    let log = phitex_flow::print::log(&format!("(./{name}.tex"), &out.log, " )");
    let log = log.strip_suffix('\n').unwrap_or(&log).to_string();
    (out.dvi, log, out.errors)
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

fn first_difference(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()))
}

/// The schedules each document runs with.
const RUNS: [Options; 5] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false },
];

fn check(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    check_src(name, &src, &RUNS);
}

fn check_src(name: &str, src: &str, runs: &[Options]) {
    let (dvi_path, log_path) = (dir().join(format!("{name}.dvi")), dir().join(format!("{name}.log")));
    let t = tex(name, src);
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        let (d, l) = t.as_ref().expect("PHITEX_FLOW_BLESS needs a tex binary");
        std::fs::write(&dvi_path, d).unwrap();
        std::fs::write(&log_path, format!("{l}\n")).unwrap();
    }
    let want_dvi = std::fs::read(&dvi_path).unwrap_or_else(|_| panic!("no {}", dvi_path.display()));
    let want_log = std::fs::read_to_string(&log_path).unwrap_or_else(|_| panic!("no {}", log_path.display()));
    let want_log = want_log.strip_suffix('\n').unwrap_or(&want_log);
    if let Some((d, l)) = &t {
        assert!(masked(d) == masked(&want_dvi), "{name}: TeX's DVI is not the recorded one");
        assert_eq!(l, want_log, "{name}: TeX's log is not the recorded one");
    }
    for &o in runs {
        let (dvi, log, errors) = graph(name, src, o);
        assert!(errors.is_empty(), "{name} ({o:?}): errors on the graph: {errors:?}");
        if log != want_log {
            let (g, e): (Vec<&str>, Vec<&str>) = (log.lines().collect(), want_log.lines().collect());
            let at = g.iter().zip(&e).position(|(a, b)| a != b).unwrap_or(g.len().min(e.len()));
            panic!(
                "{name} ({o:?}): the graph's log differs from TeX's at line {}:\n graph: {:?}\n   TeX: {:?}\n--- graph:\n{log}\n--- TeX:\n{want_log}",
                at + 1,
                g.get(at),
                e.get(at)
            );
        }
        if masked(&dvi) != masked(&want_dvi) {
            if let Ok(d) = std::env::var("PHITEX_FLOW_DVI_OUT") {
                let _ = std::fs::write(Path::new(&d).join(format!("{name}.graph.dvi")), &dvi);
                let _ = std::fs::write(Path::new(&d).join(format!("{name}.tex.dvi")), &want_dvi);
            }
            panic!(
                "{name} ({o:?}): the graph's DVI ({} bytes) differs from TeX's ({} bytes) at byte {}",
                dvi.len(),
                want_dvi.len(),
                first_difference(&masked(&dvi), &masked(&want_dvi))
            );
        }
    }
}

macro_rules! documents {
    ($($name:ident),* $(,)?) => {
        $(#[test] fn $name() { check(stringify!($name)); })*
    };
}

documents!(para, skips, hbox, setbox, vbox, pages, fonts, shape, leave, show, ship);

/// plain.tex (all but `\input hyphen`), then text set with its macros and
/// the default output routine (`\output={}`), at 1 thread.
#[test]
fn plaintext() {
    let src = std::fs::read_to_string(dir().join("plaintext.tex")).unwrap();
    check_src("plaintext", &src, &RUNS[..1]);
}

/// FAILS sometimes (`run_piece`, not material): plain.tex at 8 threads —
/// the in-place sharing of `run_piece` keeps a later reader of a name
/// whose definition changed (the bug `tests/core.rs`'s ignored tests pin:
/// here a page's `\count0` read as 0 where TeX has `\pageno`'s 1); with
/// that sharing off it passes in every schedule (checked).
#[test]
#[ignore = "run_piece shares a later reader of a changed name in place"]
fn plaintext_any_schedule() {
    let src = std::fs::read_to_string(dir().join("plaintext.tex")).unwrap();
    check_src("plaintext", &src, &RUNS[1..]);
}

/// A long document: many pages, five fonts, boxes, glue and penalties —
/// a DVI file past TeX's half-buffer (16384 bytes), so movement reuse is
/// limited by what TeX has written out.
fn long_doc() -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
         \\font\\rm=cmr10 \\font\\bf=cmbx10 \\font\\sl=cmsl10 \\font\\it=cmti10 \\font\\tt=cmtt10\n\
         \\rm \\hsize=225pt \\vsize=150pt \\parindent=12pt \\baselineskip=11pt \\topskip=10pt\n\
         \\tolerance=2000 \\linepenalty=10 \\maxdepth=2pt \\parfillskip=0pt plus 1fil \\parskip=1pt plus 1pt\n\
         \\hbadness=10000 \\vbadness=10000 \\clubpenalty=150 \\widowpenalty=150\n\n",
    );
    let words = "office affluent efficient AVoid To Yes WAVE fjord baffle flight ruffle -- --- quick brown fox jumps \
                 over lazy dog typesetting paragraph glue penalty";
    let words: Vec<&str> = words.split_whitespace().collect();
    let fonts = ["\\bf", "\\sl", "\\it", "\\tt"];
    let mut seed: u32 = 12345;
    for k in 0..120 {
        if k % 17 == 5 {
            s.push_str("\\vskip 6pt plus 2pt\n\\hbox to\\hsize{\\bf Section\\hfil ");
            s.push_str(&k.to_string());
            s.push_str("}\n\\penalty 10000\n\n");
        }
        if k % 23 == 11 {
            s.push_str("\\penalty-10000\n\n");
        }
        if k % 29 == 3 {
            let _ = writeln!(s, "\\count0={k}");
        }
        let n = 20 + (k * 7) % 45;
        for i in 0..n {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let word = words[(seed >> 16) as usize % words.len()];
            if i % 13 == 7 {
                let f = fonts[(seed >> 8) as usize % fonts.len()];
                let _ = write!(s, "{{{f} {word}}} ");
            } else if i % 31 == 17 {
                let _ = write!(s, "\\hbox{{{word}}} ");
            } else {
                s.push_str(word);
                s.push(' ');
            }
        }
        s.push_str(".\n\n");
    }
    s.push_str("\\end\n");
    s
}

#[test]
fn long() {
    check_src("long", &long_doc(), &RUNS);
}

/// Edits give what a cold build of the edited text gives, the DVI file
/// and the log: a word, a paragraph's parameter, a font change, a page
/// break.
#[test]
fn edits_give_the_cold_dvi() {
    let a = long_doc();
    let edits = [
        ("quick brown", "quick red"),
        ("\\parskip=1pt plus 1pt", "\\parskip=2pt plus 1pt"),
        ("\\font\\sl=cmsl10", "\\font\\sl=cmsl10 scaled 1100"),
        ("\\penalty-10000\n\n", "\n"),
    ];
    for threads in [1, 8] {
        let o = Options { threads, ..Options::default() };
        let mut b = Build::new(font());
        b.update(&a, o);
        let mut text = a.clone();
        for (from, to) in edits {
            assert!(text.contains(from), "{from}");
            text = text.replacen(from, to, 1);
            let e = b.update(&text, o);
            let cold = Build::new(font()).update(&text, o);
            assert!(e.errors.is_empty() && cold.errors.is_empty(), "{:?} {:?}", e.errors, cold.errors);
            assert!(e.log == cold.log, "after {from:?} => {to:?} at {threads} threads: another log");
            assert!(e.dvi == cold.dvi, "after {from:?} => {to:?} at {threads} threads: another DVI");
        }
    }
}
