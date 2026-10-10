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
const DOCS: [&str; 26] = [
    "hello",
    "compressed",
    "mag",
    "mixed1",
    "mixed2",
    "objstm",
    "rules",
    "guess",
    "tounicode",
    "tounicode0",
    // (pdfTeX's PDF-only commands, a family each)
    "literal",
    "colorstack",
    "obj",
    "trailerid",
    "xform",
    "annot",
    "outline",
    "ximage",
    "ximage2",
    "random",
    "misc",
    "savepos",
    "rerun",
    "readback",
    "thread",
    "pageref",
];

/// The directories a document's files are found in: its own, and the
/// one holding `mwe`'s example images (as `kpsewhich` finds them).
fn input_dirs() -> Vec<PathBuf> {
    let mut v = vec![dir()];
    if let Ok(o) = Command::new("kpsewhich").arg("example-image.png").output() {
        let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if let Some(d) = Path::new(&p).parent()
            && !p.is_empty()
        {
            v.push(d.to_path_buf());
        }
    }
    v
}

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
/// `pdftex`; an empty file: it wrote none), and the file `NAME.pos` it
/// wrote, if any.
fn pdftex(name: &str, src: &str) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-pdf-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    // (the images as the graph finds them: kpathsea's plain path has no
    // `tex/latex`, where `mwe` keeps them; the trailing colon keeps the default)
    let inputs = input_dirs().iter().skip(1).fold(String::from("."), |s, d| format!("{s}:{}", d.display())) + ":";
    let st = Command::new("pdftex")
        .args(["-ini", "-etex", "-interaction=batchmode", &format!("{name}.tex")])
        .env("TEXINPUTS", inputs)
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let pdf = std::fs::read(tmp.join(format!("{name}.pdf"))).unwrap_or_default();
    let pos = std::fs::read(tmp.join(format!("{name}.pos"))).ok();
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = st;
    Some((pdf, pos))
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
        b.set_input_dirs(input_dirs());
        b.set_pdf_date(0);
        let out = b.update(&text, o);
        if let Some(d) = std::env::var_os("PHITEX_FLOW_PDF_OUT") {
            let _ = std::fs::write(Path::new(&d).join(format!("{name}.pdf")), &out.pdf);
        }
        assert!(out.errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", out.errors);
        assert!(out.pdf == want, "{name} ({o:?}): the PDF file differs {}", first_difference(&out.pdf, &want));
        assert!(!again || out.stats.bodies_again > 0, "{name} ({o:?}): no page made again");
        // (a file it writes: the oracle's)
        if let Ok(pos) = std::fs::read(dir().join(format!("{name}.pos"))) {
            let ours = out.writes.iter().find(|(n, _)| *n == format!("{name}.pos")).map(|(_, t)| t.clone());
            let want: String = pos.iter().map(|&c| char::from(c)).collect();
            assert_eq!(ours.as_deref(), Some(want.as_str()), "{name} ({o:?}): {name}.pos is not pdftex's");
        }
    }
}

/// Every recorded oracle is what `pdftex` makes of its document;
/// `PHITEX_FLOW_BLESS=1` records them.
#[test]
fn oracles_are_pdftex() {
    // (every document, the failures listed at the end: one blessing run
    // records all it can)
    let mut bad = Vec::new();
    for name in DOCS {
        let Some((pdf, pos)) = pdftex(name, &src(name)) else { return };
        if pdf.is_empty() {
            bad.push(format!("{name}: pdftex wrote no PDF"));
            continue;
        }
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            std::fs::write(dir().join(format!("{name}.pdf")), &pdf).unwrap();
            if let Some(p) = &pos {
                std::fs::write(dir().join(format!("{name}.pos")), p).unwrap();
            }
        }
        if std::fs::read(dir().join(format!("{name}.pdf"))).ok().as_deref() != Some(&pdf[..]) {
            bad.push(format!("{name}: pdftex's PDF is not the recorded one"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
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

/// `\pdfliteral` in its three modes, `\pdfsave`/`\pdfsetmatrix`/
/// `\pdfrestore`, `\special{pdf:…}` in its three modes and others
/// ignored.
#[test]
fn literal() {
    check("literal");
}

/// Colour stacks: `\pdfcolorstackinit` (page, direct, neither), push,
/// pop, set, current; a stack pushed on one page and popped on the next
/// (its top written at the next page's start).
#[test]
fn colorstack() {
    check("colorstack");
}

/// `\pdfobj` (plain, `\immediate`, `reserveobjnum`, `useobjnum`,
/// `stream` with `attr`, `file`), `\pdfrefobj`, `\pdfretval`,
/// `\pdfcatalog`, `\pdfinfo`, `\pdfnames`, `\pdftrailer`, the page
/// attributes and resources.
#[test]
fn obj() {
    check("obj");
}

/// `\pdftrailerid`, the classic trailer, `\pdfinfoomitdate`,
/// `\pdfsuppressptexinfo`, `\pdfcatalog`'s `openaction`.
#[test]
fn trailerid() {
    check("trailerid");
}

/// `\pdfxform` (`attr`, `resources`, `\immediate`, a form inside a
/// form) and `\pdfrefxform`.
#[test]
fn xform() {
    check("xform");
}

/// `\pdfannot` (`reserveobjnum`, `useobjnum`), `\pdfdest` of every
/// kind (and a duplicate, at the ship and at the command), links of every
/// action (`goto` name, num, page, file; `user`; `thread`), a link over a
/// line break, nested links, a link over a page break, threads.
#[test]
fn annot() {
    check("annot");
}

/// `\pdfoutline`: nested, open and closed, `attr`, every action.
#[test]
fn outline() {
    check("outline");
}

/// `\pdfximage` of a PNG, a JPEG and a PDF page, sized and not,
/// `attr`; `\pdfrefximage` in both modes; `\pdfximagebbox`,
/// `\pdflastximagepages`, `\pdflastximagecolordepth`.
#[test]
fn ximage() {
    check("ximage");
}

/// `\pdfinclusioncopyfonts`, `\pdfimageresolution`, `\pdfpagebox` and a
/// page box asked for.
#[test]
fn ximage2() {
    check("ximage2");
}

/// `\pdfsetrandomseed`, `\pdfuniformdeviate`, `\pdfnormaldeviate`.
#[test]
fn random() {
    check("random");
}

/// The file facts, MD5 sums, dumps, the creation date; `\pdffontname`,
/// `\pdffontobjnum`, `\pdffontsize`, `\pdffontattr`,
/// `\pdfincludechars`, `\pdfmapline`.
#[test]
fn misc() {
    check("misc");
}

/// `\pdfsavepos`: positions in a `\write` on the same page, and read
/// after the page.
#[test]
fn savepos() {
    check("savepos");
}

/// A reader of `\pdflastobj` whose run again allocates objects itself
/// (the numbering's rounds converge).
#[test]
fn rerun() {
    check("rerun");
}

/// Every read-back: `\pdflastobj`, `\pdflastxform`, `\pdflastximage`,
/// `\pdflastannot`, `\pdflastlink`, `\pdflastxpos`/`ypos`,
/// `\pdfretval`, typeset where they are read.
#[test]
fn readback() {
    check("readback");
}

/// `\pdfthread` and `\pdfstartthread`/`\pdfendthread` over pages, a
/// thread referred to and not made.
#[test]
fn thread() {
    check("thread");
}

/// `\pdfpageref` before the page is shipped (the page object made
/// early) and after.
#[test]
fn pageref() {
    check("pageref");
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
        let Some((pdf, _)) = pdftex(name, &plain_doc(name)) else { return };
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
