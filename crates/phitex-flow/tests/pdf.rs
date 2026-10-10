//! pdfTeX's PDF mode on the graph: documents (`tests/pdf/NAME.tex`) whose
//! PDF file must be byte for byte what `pdftex -ini -etex` writes of them
//! (`SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1`), at every schedule. The
//! oracles are recorded (`NAME.pdf`); [`oracles_are_pdftex`] checks them
//! against the system's `pdftex`, `PHITEX_FLOW_BLESS=1` records them anew.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::build::MAIN_MEMORY;
use phitex_flow::{Build, Font, Options, Order, Reads};

/// The documents.
const DOCS: [&str; 10] =
    ["hello", "compressed", "mag", "mixed1", "mixed2", "objstm", "rules", "guess", "tounicode", "tounicode0"];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdf")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// The schedules: one thread, eight, eight speculating, and three random
/// orders.
const RUNS: [Options; 6] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
    Options { threads: 3, reads: Reads::Suspend, order: Order::Random(23), dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES },
];

/// `pdftex -ini -etex` on `src` as `NAME.tex`: the PDF file (`None`: no
/// `pdftex`; an empty file: it wrote none).
fn pdftex(name: &str, src: &str) -> Option<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-pdf-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    let st = Command::new("pdftex")
        .args(["-ini", "-etex", "-interaction=batchmode", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let pdf = std::fs::read(tmp.join(format!("{name}.pdf"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = st;
    Some(pdf)
}

fn src(name: &str) -> String {
    std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap()
}

fn recorded(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(format!("{name}.pdf"))).unwrap_or_else(|_| panic!("no {name}.pdf"))
}

/// Where two files first differ, with the bytes around it.
fn first_difference(a: &[u8], b: &[u8]) -> String {
    let at = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let show = |x: &[u8]| String::from_utf8_lossy(&x[at.saturating_sub(80)..(at + 80).min(x.len())]).into_owned();
    format!("at byte {at} (lengths {} and {}):\n graph: {:?}\n pdftex: {:?}", a.len(), b.len(), show(a), show(b))
}

/// The graph writes document `name`'s PDF file as pdfTeX does, at every
/// schedule.
fn check(name: &str) {
    check_again(name, false);
}

/// [`check`]; with `again`, some page's own guesses about the pages
/// before it are wrong (`pdf::content::page`) and the fold makes it
/// again: the file is still the same at every schedule.
fn check_again(name: &str, again: bool) {
    let text = src(name);
    let want = recorded(name);
    for &o in &RUNS {
        let mut b = Build::new(font());
        b.set_jobname(name);
        let out = b.update(&text, o);
        if let Some(d) = std::env::var_os("PHITEX_FLOW_PDF_OUT") {
            let _ = std::fs::write(Path::new(&d).join(format!("{name}.pdf")), &out.pdf);
        }
        assert!(out.errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", out.errors);
        assert!(out.pdf == want, "{name} ({o:?}): the PDF file differs {}", first_difference(&out.pdf, &want));
        assert!(!again || out.stats.bodies_again > 0, "{name} ({o:?}): no page made again");
    }
}

/// Every recorded oracle is what `pdftex` makes of its document;
/// `PHITEX_FLOW_BLESS=1` records them.
#[test]
fn oracles_are_pdftex() {
    for name in DOCS {
        let Some(pdf) = pdftex(name, &src(name)) else { return };
        assert!(!pdf.is_empty(), "{name}: pdftex wrote no PDF");
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            std::fs::write(dir().join(format!("{name}.pdf")), &pdf).unwrap();
        }
        assert!(pdf == recorded(name), "{name}: pdftex's PDF is not the recorded one");
    }
}

#[test]
fn hello() {
    check("hello");
}

#[test]
fn compressed() {
    check("compressed");
}

#[test]
fn mag() {
    check("mag");
}

#[test]
fn mixed1() {
    check("mixed1");
}

#[test]
fn mixed2() {
    check("mixed2");
}

#[test]
fn objstm() {
    check("objstm");
}

#[test]
fn rules() {
    check("rules");
}

/// `\pdfgentounicode`: the `ToUnicode` maps of a font with its own
/// encoding and of a re-encoded one, from `\pdfglyphtounicode`'s entries
/// (ranges, strings, a font's own, out of range, invalid, redefined).
#[test]
fn tounicode() {
    check("tounicode");
}

/// `\pdfgentounicode` without any entry: no map.
#[test]
fn tounicode0() {
    check("tounicode0");
}

/// Page 2 guesses its decimal digits (page 1 fixed 3) and which font
/// shows `cmr10 at 12pt` (`cmr10`, set on page 1, owns the object): both
/// wrong, so the fold makes it again.
#[test]
fn guess() {
    check_again("guess", true);
}

// ---- plain TeX in PDF mode (the documents of `tests/plain.rs`)

/// The plain documents, as `tests/plain.rs` has them (`NAME-default`:
/// TeX's default output).
const PLAIN: [&str; 10] = [
    "story",
    "story-default",
    "pages",
    "pages-default",
    "display",
    "display-default",
    "table",
    "table-default",
    "hyphens",
    "hyphens-default",
];

/// plain.tex: `tests/dvi/plaintext.tex` before its body, with
/// `hyphen.tex` (as `tests/plain.rs`).
fn plain() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let s = std::fs::read_to_string(root.join("tests/dvi/plaintext.tex")).unwrap();
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = std::fs::read_to_string(root.join("tests/hyph/hyphen.tex")).unwrap();
    s[..at].replace("% (\\input hyphen: no hyphenation patterns on the graph yet)\n", &h)
}

/// Plain document `name` in PDF mode.
fn plain_doc(name: &str) -> String {
    let (body, default) = match name.strip_suffix("-default") {
        Some(b) => (b, true),
        None => (name, false),
    };
    let b = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/plain/{body}.tex"))).unwrap();
    "\\pdfoutput=1\n".to_string() + &plain() + if default { "\\output={}\n" } else { "" } + &b
}

fn check_plain(name: &str) {
    let text = plain_doc(name);
    let want = recorded(&format!("plain-{name}"));
    for &o in &RUNS {
        let mut b = Build::new(font());
        b.set_jobname(name);
        let out = b.update(&text, o);
        if let Some(d) = std::env::var_os("PHITEX_FLOW_PDF_OUT") {
            let _ = std::fs::write(Path::new(&d).join(format!("plain-{name}.pdf")), &out.pdf);
        }
        assert!(out.errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", out.errors);
        assert!(out.pdf == want, "{name} ({o:?}): the PDF file differs {}", first_difference(&out.pdf, &want));
    }
}

/// Every recorded plain oracle is pdftex's.
#[test]
fn plain_oracles_are_pdftex() {
    for name in PLAIN {
        let Some(pdf) = pdftex(name, &plain_doc(name)) else { return };
        assert!(!pdf.is_empty(), "{name}: pdftex wrote no PDF");
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            std::fs::write(dir().join(format!("plain-{name}.pdf")), &pdf).unwrap();
        }
        assert!(pdf == recorded(&format!("plain-{name}")), "{name}: pdftex's PDF is not the recorded one");
    }
}

#[test]
fn plain_story() {
    check_plain("story");
}

#[test]
fn plain_story_default() {
    check_plain("story-default");
}

#[test]
fn plain_pages() {
    check_plain("pages");
}

#[test]
fn plain_pages_default() {
    check_plain("pages-default");
}

#[test]
fn plain_display() {
    check_plain("display");
}

#[test]
fn plain_display_default() {
    check_plain("display-default");
}

#[test]
fn plain_table() {
    check_plain("table");
}

#[test]
fn plain_table_default() {
    check_plain("table-default");
}

#[test]
fn plain_hyphens() {
    check_plain("hyphens");
}

#[test]
fn plain_hyphens_default() {
    check_plain("hyphens-default");
}
