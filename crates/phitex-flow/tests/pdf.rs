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
const DOCS: [&str; 8] = ["hello", "compressed", "mag", "mixed1", "mixed2", "objstm", "rules", "guess"];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdf")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../partex-core/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// The schedules: one thread, eight, eight speculating, and three random
/// orders.
const RUNS: [Options; 6] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: true, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: true, main_memory: MAIN_MEMORY },
    Options { threads: 3, reads: Reads::Suspend, order: Order::Random(23), dump: false, etex: true, main_memory: MAIN_MEMORY },
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

/// Page 2 guesses its decimal digits (page 1 fixed 3) and which font
/// shows `cmr10 at 12pt` (`cmr10`, set on page 1, owns the object): both
/// wrong, so the fold makes it again.
#[test]
fn guess() {
    check_again("guess", true);
}
