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
//!
//! The `etex` flavour (`check_etex`) runs a document through `etex -ini
//! -etex` and the graph in e-TeX's extended mode (`Options::etex`):
//! `TeXXeT`, `\lastlinefit`, the penalties arrays, `\middle`.

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

/// TeX's DVI file and log of a document, if there is a `tex` (`etex -ini
/// -etex` for e-TeX's extended mode).
fn tex(name: &str, src: &str, etex: bool) -> Option<(Vec<u8>, String)> {
    // (a directory per call: two tests run the same document at once)
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("phitex-flow-dvi-{name}-{}-{call}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    let (bin, args): (&str, &[&str]) = if etex { ("etex", &["-ini", "-etex"]) } else { ("tex", &["-ini"]) };
    let st = Command::new(bin)
        .args(args)
        .args(["-interaction=batchmode", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join(format!("{name}.log"))).ok();
    let dvi = std::fs::read(tmp.join(format!("{name}.dvi"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    // (`\showbox`, `\showlists` end in an "OK" error: no other error)
    let only_ok = log.lines().filter(|l| l.starts_with("! ")).all(|l| l == "! OK.");
    assert!(st.status.success() || only_ok, "tex failed on {name}:\n{log}");
    Some((dvi, log_body(&log)?))
}

/// TeX's error messages for a document, in order (the "! " lines).
fn tex_errors(src: &str) -> Option<Vec<String>> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-errs-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join("errs.tex"), src).ok()?;
    Command::new("tex").args(["-ini", "-interaction=batchmode", "errs.tex"]).current_dir(&tmp).output().ok()?;
    let log = std::fs::read_to_string(tmp.join("errs.log")).ok();
    let _ = std::fs::remove_dir_all(&tmp);
    Some(
        log?.lines()
            .filter_map(|l| l.strip_prefix("! "))
            .map(|l| l.strip_suffix('.').unwrap_or(l).to_string())
            .collect(),
    )
}

/// A document whose line `%%% hyphen.tex %%%` stands for TeX Live's
/// `hyphen.tex` (its patterns and exceptions, read by the `\patterns`
/// and `\hyphenation` nodes).
fn with_hyphen(name: &str) -> String {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    let h = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hyph/hyphen.tex")).unwrap();
    src.replace("%%% hyphen.tex %%%\n", &h)
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
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: false },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: false },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: false },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: false },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: false },
];

/// The same, in e-TeX's extended mode.
const RUNS_ETEX: [Options; 5] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: true },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: true },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: true },
];

fn check(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    check_src(name, &src, &RUNS);
}

/// A document of the `etex` flavour.
fn check_etex(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    check_src(name, &src, &RUNS_ETEX);
}

fn check_src(name: &str, src: &str, runs: &[Options]) {
    let (dvi_path, log_path) = (dir().join(format!("{name}.dvi")), dir().join(format!("{name}.log")));
    let t = tex(name, src, runs.iter().any(|o| o.etex));
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

documents!(para, skips, hbox, setbox, vbox, pages, fonts, shape, leave, show, ship, disc, math1, math2, math3, math4);

macro_rules! etex_documents {
    ($($name:ident),* $(,)?) => {
        $(#[test] fn $name() { check_etex(stringify!($name)); })*
    };
}

etex_documents!(texxet, llfit, pens, middle);
/// Paragraphs hyphenated with hyphen.tex's patterns (loaded by the
/// `\patterns` node), in every schedule: breaks at hyphens, ligatures and
/// kerns around them, the minima, `\uchyph`, languages, `\setlanguage`,
/// explicit discretionaries, `\hyphenchar`, the third pass, a display.
#[test]
fn hyph1() {
    let src = with_hyphen("hyph1");
    check_src("hyph1", &src, &RUNS);
}

/// More hyphenation: narrow lines, many fonts, `\showbox` of hyphenated
/// lines (the discretionaries' ligatures shown), paragraphs in boxes.
#[test]
fn hyph2() {
    let src = with_hyphen("hyph2");
    check_src("hyph2", &src, &RUNS);
}

/// `\showlists` and `\showbox` in math (the noads displayed, tex.web
/// 690–698), with their "OK" errors and contexts.
#[test]
fn showmath() {
    check("showmath");
}

/// The errors of a formula without math fonts, and of mismatched math
/// groups, are TeX's, in TeX's order (§1194: the fonts before the second
/// `$`; `off_save` in math, tex.web 1064).
#[test]
fn math_errors_in_order() {
    let src = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\^=7\n               \\font\\tenrm=cmr10 \\tenrm \\hsize=100pt\n               A $$x$ $ b $$y\\eqno z$ $ c ${a\\right)$ d $\\left(a$ e\n               $\\left(a\\endgroup$ $\\begingroup a$\\endgroup$ f\n\n\\end\n";
    let o = Build::new(font()).update(src, Options::default());
    let got: Vec<String> = o.errors.iter().map(|e| e.split_once(": ").map_or(e.as_str(), |x| x.1).to_string()).collect();
    let want = match tex_errors(src) {
        Some(w) => {
            let rec = w.join("\n") + "\n";
            if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
                std::fs::write(dir().join("matherrs.txt"), &rec).unwrap();
            }
            w
        }
        None => std::fs::read_to_string(dir().join("matherrs.txt")).unwrap().lines().map(str::to_string).collect(),
    };
    let rec: Vec<String> =
        std::fs::read_to_string(dir().join("matherrs.txt")).unwrap().lines().map(str::to_string).collect();
    assert_eq!(want, rec, "TeX's errors are not the recorded ones");
    assert_eq!(got, want);
}

/// plain.tex (all but `\input hyphen`), then text set with its macros and
/// the default output routine (`\output={}`), at 1 thread.
#[test]
fn plaintext() {
    let src = std::fs::read_to_string(dir().join("plaintext.tex")).unwrap();
    check_src("plaintext", &src, &RUNS[..1]);
}

/// plain.tex, then text using plain's math macros (`\sqrt`, `\int`,
/// `\sum`, `\left`/`\right`, `\root`, `\choose`, `\buildrel`,
/// displays with `\eqno`), at 1 thread.
#[test]
fn plainmath() {
    let src = std::fs::read_to_string(dir().join("plainmath.tex")).unwrap();
    check_src("plainmath", &src, &RUNS[..1]);
}

/// The same in every schedule.
#[test]
fn plainmath_any_schedule() {
    let src = std::fs::read_to_string(dir().join("plainmath.tex")).unwrap();
    check_src("plainmath", &src, &RUNS[1..]);
}

/// The same in every schedule (a page's `\count0` is `\pageno`'s 1).
#[test]
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
    check_src("long", &long_doc(), &RUNS[..1]);
}

/// The same in every schedule.
#[test]
fn long_any_schedule() {
    check_src("long", &long_doc(), &RUNS[1..]);
}

/// Edits give what a cold build of the edited text gives, the DVI file
/// and the log: a word, a paragraph's parameter, a font change, a page
/// break.
#[test]
fn edits_give_the_cold_dvi() {
    edits(1);
}

/// The same at 8 threads.
#[test]
fn edits_give_the_cold_dvi_at_eight_threads() {
    edits(8);
}

fn edits(threads: usize) {
    let a = long_doc();
    let edits = [
        ("quick brown", "quick red"),
        ("\\parskip=1pt plus 1pt", "\\parskip=2pt plus 1pt"),
        ("\\font\\sl=cmsl10", "\\font\\sl=cmsl10 scaled 1100"),
        ("\\penalty-10000\n\n", "\n"),
        ("\\font\\rm=cmr10", "\\font\\extra=cmr12 \\font\\rm=cmr10"),
    ];
    {
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

/// A font loaded before the others renumbers them in the DVI file (TeX's
/// internal numbers), but a font is its name and size: no paragraph runs
/// again, only the piece loading it.
#[test]
fn a_font_loaded_first_reruns_no_paragraph() {
    let a = long_doc();
    let b = a.replacen("\\font\\rm=cmr10", "\\font\\extra=cmr12 \\font\\rm=cmr10", 1);
    let o = Options::default();
    let mut bld = Build::new(font());
    let first = bld.update(&a, o);
    let e = bld.update(&b, o);
    let cold = Build::new(font()).update(&b, o);
    assert!(e.dvi == cold.dvi && e.log == cold.log, "the edit's output is not the cold build's");
    assert!(e.dvi != first.dvi, "the fonts are numbered anew");
    assert_eq!(e.stats.ran, 1, "only the piece loading the fonts runs: {:?}", e.stats);
}

/// A document whose DVI file is long (past 16 KB, several fonts, pages of
/// several KB) with dense pages whose bodies pass 8 KB: on each, a
/// movement equal to one more than half TeX's buffer back (`\kern5pt`
/// between paragraphs twice, `lines` lines apart), which TeX reuses or not
/// by where the page lands in the file (`dvi_gone`, §611). `early` goes
/// into the first page.
fn shift_doc(early: &str) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
         \\font\\rm=cmr10 \\font\\bf=cmbx10 \\font\\sl=cmsl10 \\font\\it=cmti10 \\font\\tt=cmtt10\n\
         \\rm \\hsize=225pt \\vsize=3600pt \\parindent=12pt \\baselineskip=11pt \\topskip=10pt\n\
         \\tolerance=10000 \\linepenalty=10 \\maxdepth=2pt \\parfillskip=0pt plus 1fil \\parskip=0pt\n\
         \\hbadness=10000 \\vbadness=10000 \\clubpenalty=0 \\widowpenalty=0\n\n",
    );
    let words = "office affluent efficient AVoid To Yes WAVE fjord baffle flight ruffle quick brown fox jumps \
                 over lazy dog typesetting paragraph glue penalty";
    let words: Vec<&str> = words.split_whitespace().collect();
    let mut seed: u32 = 777;
    let mut para = |s: &mut String, n: usize, font: Option<&str>| {
        for i in 0..n {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let word = words[(seed >> 16) as usize % words.len()];
            match font {
                Some(f) if i % 11 == 5 => {
                    let _ = write!(s, "{{{f} {word}}} ");
                }
                _ => {
                    s.push_str(word);
                    s.push(' ');
                }
            }
        }
        s.push_str(".\n\n");
    };
    // (pages of a few KB; `\bf` first used on the third)
    for page in 0..5 {
        if page == 0 {
            s.push_str(early);
        }
        for _ in 0..6 {
            para(&mut s, 60, [None, Some("\\sl"), Some("\\bf"), Some("\\it"), Some("\\tt")][page]);
        }
        s.push_str("\\penalty-10000\n\n");
    }
    // (dense pages: ~70 bytes a line)
    for lines in [130, 170, 210] {
        para(&mut s, 40, Some("\\it"));
        s.push_str("\\kern5pt\n\n");
        for _ in 0..lines / 10 {
            para(&mut s, 75, None);
        }
        s.push_str("\\kern5pt\n\n");
        para(&mut s, 40, Some("\\tt"));
        s.push_str("\\penalty-10000\n\n");
    }
    s.push_str("\\end\n");
    s
}

/// The DVI file as per-page values: edits on the first page move every
/// later page in the file (a word; a long paragraph; `\bf` used first
/// there, so later pages define it no more) and give what TeX gives, at 1
/// and 8 threads, the dense pages written again where they land.
#[test]
fn edits_shift_the_pages() {
    let long = "Long inserted paragraph ".repeat(140) + "\n\n";
    let versions = [
        ("shift0", shift_doc("")),
        ("shift1", shift_doc("Edited\n\n")),
        ("shift2", shift_doc(&long)),
        ("shift3", shift_doc(&format!("{long}{{\\bf bold}}\n\n"))),
    ];
    for (name, src) in &versions {
        check_src(name, src, &RUNS[..1]);
    }
    let cold = Build::new(font()).update(&versions[0].1, RUNS[0]);
    assert!(cold.dvi.len() > 3 * 16384, "{} bytes", cold.dvi.len());
    assert!(cold.stats.bodies_fixed >= 3, "{:?}", cold.stats);
    for threads in [1, 8] {
        let o = Options { threads, ..Options::default() };
        let mut b = Build::new(font());
        let first = b.update(&versions[0].1, o);
        eprintln!("{threads} threads: cold {:?}, {} bytes", first.stats.wall, first.dvi.len());
        for (name, src) in &versions[1..] {
            let e = b.update(src, o);
            let want = std::fs::read(dir().join(format!("{name}.dvi"))).unwrap();
            assert!(e.errors.is_empty(), "{:?}", e.errors);
            assert!(masked(&e.dvi) == masked(&want), "{name} at {threads} threads: not TeX's DVI");
            eprintln!(
                "{threads} threads: {name}: edit {:?} ({} pages shipped, {} written again at the join)",
                e.stats.wall, e.stats.pages_shipped, e.stats.bodies_again
            );
        }
    }
}
