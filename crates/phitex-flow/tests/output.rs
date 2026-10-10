//! Output routines, the TeX side (`graph::output`): a page as the page
//! fold makes it, run through a routine with `output_entry` and
//! `run_routine`, against `tex -ini` running the same routine on the same
//! page.
//!
//! The documents are plain.tex (the prelude of `tests/dvi/plaintext.tex`)
//! and a body in `tests/output/NAME.tex` that fills one page and ends with
//! `\bye`. On the graph each piece runs in order (a sequential build), the
//! page fold makes the page with the default output (the fold does not
//! call routines yet: agent A's), and the routine runs from that page with
//! what the fold reads at the fire. TeX's outputs are recorded in
//! `tests/output/` (`NAME.dvi`, `errors.txt`); where a `tex` binary is
//! found they are checked against a fresh run; `PHITEX_FLOW_BLESS=1`
//! records them anew.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::build::CsTable;
use phitex_flow::dvi;
use phitex_flow::graph::output::{OutputEnd, OutputReads, fire_marks, output_end, output_entry, run_routine};
use phitex_flow::graph::{Env, Run, Src, cut, run_piece};
use phitex_flow::item::{BoxNode, Item, ShipParams};
use phitex_flow::linebreak::break_lines;
use phitex_flow::page::{Fired, FoldEnv, Lines, Page, Routine, Scan, material};
use phitex_flow::tfm::{FontFiles, default_dirs, null_font};
use phitex_flow::val::{Key, Tok, TokPar, Val, initial};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/output")
}

/// plain.tex, as `tests/dvi/plaintext.tex` gives it (before its body).
fn plain() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dvi/plaintext.tex");
    let s = std::fs::read_to_string(p).unwrap();
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    s[..at].to_string()
}

fn doc(name: &str) -> String {
    plain() + &std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap()
}

/// TeX's DVI file and errors for a document, if there is a `tex`: each
/// error's message (its "! " line), then its help lines (after the
/// context, up to the blank line).
fn tex(name: &str, src: &str) -> Option<(Vec<u8>, Vec<String>)> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-output-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    Command::new("tex")
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
    let lines: Vec<&str> = log.lines().collect();
    let mut errors = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(m) = l.strip_prefix("! ") else { continue };
        let mut e = m.strip_suffix('.').unwrap_or(m).to_string();
        // (the context ends with the file's line, two lines; the help
        // follows)
        if let Some(k) = lines[i + 1..].iter().position(|x| x.starts_with("l.") || x.starts_with("! ")) {
            let k = i + 1 + k;
            if lines[k].starts_with("l.") {
                for h in lines[k + 2..].iter().take_while(|x| !x.is_empty()) {
                    e.push('\n');
                    e.push_str(h);
                }
            }
        }
        errors.push(e);
    }
    Some((dvi, errors))
}

/// A DVI file with its preamble's comment masked (the date).
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

/// The definitions before a place: a sequential build's names.
struct Names {
    defs: HashMap<Key, Val>,
    fonts: FontFiles,
    cs: Arc<CsTable>,
}

impl Env for Names {
    fn cs(&self) -> &Arc<CsTable> {
        &self.cs
    }

    fn reach(&mut self, key: &Key, _node: u32) -> (Val, Src, u32, u32) {
        match self.defs.get(key) {
            Some(v) => (v.clone(), Src::Piece(0), 0, u32::MAX),
            None => (initial(key, &null_font(), false, Some(&self.cs)), Src::Initial, 0, u32::MAX),
        }
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.fonts.get(name)
    }
}

/// A document run piece by piece in order, and its pages as the page fold
/// makes them (the default output).
struct Doc {
    names: Names,
    fonts: Vec<Vec<Arc<str>>>,
    pages: Vec<Page>,
}

/// The page fold's reads in [`run_doc`]: the names as the pieces fed so
/// far left them, but `\output` empty (the default output makes the page
/// the tests run the routine on themselves).
struct DocFold<'a>(&'a Names);

impl FoldEnv for DocFold<'_> {
    fn read(&mut self, key: &Key) -> Val {
        if *key == Key::TokPar(TokPar::Output) {
            return Val::Toks(Arc::new(Vec::new()));
        }
        self.0.defs.get(key).cloned().unwrap_or_else(|| initial(key, &null_font(), true, Some(&self.0.cs)))
    }
    fn fire(&mut self, _defs: Vec<(Key, Val)>, _routine: Option<Routine>) -> Fired {
        Fired::default()
    }
}

fn run_doc(src: &str) -> Doc {
    let mut names =
        Names { defs: HashMap::new(), fonts: FontFiles::new(Vec::new(), default_dirs()), cs: Arc::new(CsTable::new()) };
    let mut scan = Scan::default();
    let mut fonts = Vec::new();
    let pieces = cut(src);
    for (k, text) in pieces.iter().enumerate() {
        let run = run_piece(text, &mut names, None, &[], None);
        assert!(run.errors.is_empty(), "errors in piece {k}: {:?}", run.errors);
        for (key, val) in &run.exports {
            names.defs.insert(key.clone(), val.clone());
        }
        let paras: Vec<(Arc<phitex_flow::item::Para>, Lines)> = run
            .paras
            .iter()
            .map(|p| (p.clone(), Arc::new(if p.lb.is_some() { break_lines(p) } else { Vec::new() })))
            .collect();
        for (j, it) in material(&paras).into_iter().enumerate() {
            scan.feed(it, (u32::try_from(k).unwrap(), u32::try_from(j).unwrap()), &mut DocFold(&names));
        }
        fonts.push(run.fonts.clone());
    }
    scan.finish((u32::try_from(pieces.len()).unwrap(), 0), &mut DocFold(&names));
    let (pages, _) = scan.take_pages();
    Doc { names, fonts, pages }
}

/// The document's `\output`.
fn output(d: &Doc) -> Arc<Vec<Tok>> {
    match d.names.defs.get(&Key::TokPar(TokPar::Output)) {
        Some(Val::Toks(t)) => t.clone(),
        _ => panic!("no \\output"),
    }
}

/// The routine run on the document's only page, fired by `\bye`'s
/// `\supereject` (`\penalty-20000`), the first page (no marks before).
fn routine_on_page(d: &mut Doc) -> Run {
    assert_eq!(d.pages.len(), 1, "one page");
    let page = d.pages[0].clone();
    let reads = OutputReads {
        output_penalty: -20000,
        insert_penalties: 0,
        dead_cycles: 0,
        marks: fire_marks(&[], &page.b),
        page_so_far: [page.b.h, 0, 0, 0, 0, 0, 0, 0],
    };
    let out = output(d);
    let entry = output_entry(&page, &reads, &out);
    run_routine(&entry, &mut d.names)
}

/// The pages a run's `\shipout`s made, in order.
fn shipped(run: &Run) -> Vec<Page> {
    run.paras
        .iter()
        .flat_map(|p| p.before.iter().chain(&p.after))
        .filter_map(|it| match it {
            Item::Shipout(s) => Some(Page { b: s.b.clone(), ship: s.ship.clone(), builder: false }),
            _ => None,
        })
        .collect()
}

/// The DVI file of `pages`, fonts numbered as the document loaded them
/// (then the routine).
fn dvi_of(d: &Doc, run: &Run, pages: &[Page]) -> Vec<u8> {
    let numbers = dvi::numbers(d.fonts.iter().map(|v| &v[..]).chain(std::iter::once(&run.fonts[..])));
    let mut bodies: Vec<Arc<dvi::Body>> =
        pages.iter().map(|p| Arc::new(dvi::page_body(p, &numbers, &[], &[], 0))).collect();
    dvi::assemble(&numbers, pages, &mut bodies).0
}

fn export(run: &Run, key: &Key) -> Option<Val> {
    run.exports.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// plain.tex's `\output` (`\plainoutput`: `\shipout\vbox{\makeheadline
/// \pagebody\makefootline}\advancepageno…`) on a page with marks in its
/// headline: the page shipped is TeX's, byte for byte.
#[test]
fn plain_output_routine() {
    let src = doc("plainout");
    let mut d = run_doc(&src);
    let run = routine_on_page(&mut d);
    assert!(run.errors.is_empty(), "errors: {:?}", run.errors);
    let pages = shipped(&run);
    assert_eq!(pages.len(), 1);
    let got = dvi_of(&d, &run, &pages);
    let path = dir().join("plainout.dvi");
    if let Some((want, errors)) = tex("plainout", &src) {
        assert!(errors.is_empty(), "TeX's errors: {errors:?}");
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        assert!(masked(&want) == masked(&std::fs::read(&path).unwrap()), "TeX's DVI is not the recorded one");
    }
    let want = std::fs::read(&path).unwrap();
    assert!(masked(&got) == masked(&want), "the routine's page is not TeX's ({} vs {} bytes)", got.len(), want.len());
    // (what the fold takes back: nothing left, a page shipped)
    let (end, left) = output_end(&run).expect("the routine ended");
    assert_eq!((end, left), (&OutputEnd { shipped: 1, dead_cycles: 0 }, &[][..]));
    // (`\advancepageno`, global; `\box255` used)
    assert_eq!(export(&run, &Key::Count(0)), Some(Val::Int(2)));
    assert_eq!(export(&run, &Key::Box(255)), Some(Val::Void));
}

/// The two errors of a routine's end (tex.web 1100, 1028): the graph's
/// are TeX's, in order, with their help.
#[test]
fn output_routine_errors() {
    let path = dir().join("errors.txt");
    let mut lines = Vec::new();
    for name in ["unbalanced", "unused"] {
        let src = doc(name);
        let mut d = run_doc(&src);
        let run = routine_on_page(&mut d);
        if let Some((_, errors)) = tex(name, &src) {
            for l in errors.iter().flat_map(|e| e.lines()) {
                lines.push(format!("{name}: {l}"));
            }
        }
        let want: Vec<String> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.strip_prefix(&format!("{name}: ")).map(str::to_string))
            .collect();
        if std::env::var_os("PHITEX_FLOW_BLESS").is_none() {
            let got: Vec<&str> = run.errors.iter().flat_map(|e| e.lines()).collect();
            assert_eq!(got, want, "{name}: the graph's errors are not TeX's");
        }
        // (both routines ship a page; neither leaves material)
        assert_eq!(shipped(&run).len(), 1, "{name}");
        assert_eq!(output_end(&run).map(|o| o.1.len()), Some(0), "{name}");
        assert_eq!(export(&run, &Key::Box(255)), Some(Val::Void), "{name}");
    }
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    } else if !lines.is_empty() {
        let recorded = std::fs::read_to_string(&path).unwrap();
        assert_eq!(recorded, lines.join("\n") + "\n", "TeX's errors are not the recorded ones");
    }
}

/// What a routine reads where the fold fired, and what it hands back: the
/// page as `\box255`, `\outputpenalty`, the page's dimensions,
/// `\insertpenalties` and `\deadcycles` as the fold gives them, the marks;
/// the material left in its list; its own `\insertpenalties` gone after
/// it.
#[test]
fn a_routine_reads_the_fire_and_leaves_its_list() {
    let src = "\\catcode`\\{=1 \\catcode`\\}=2\n\
               \\output={\\global\\setbox1\\box255 \\global\\count1=\\outputpenalty\n\
               \\global\\dimen1=\\pagegoal \\global\\dimen2=\\pagetotal\n\
               \\global\\count2=\\insertpenalties \\global\\count3=\\deadcycles\n\
               \\global\\toks1=\\expandafter{\\topmark}\\global\\toks2=\\expandafter{\\botmark}\n\
               \\insertpenalties=7 \\hbox{}\\penalty7 \\kern2pt}\n";
    let mut d = run_doc(src);
    let page = Page {
        b: Arc::new(BoxNode { vertical: true, h: 100, ..BoxNode::default() }),
        ship: Arc::new(ShipParams { counts: [0; 10], hoffset: 0, voffset: 0, mag: 1000, date: [1970, 1, 1, 0], pdf: None, xdv: None }),
        builder: false,
    };
    let t = |s: &str| -> Arc<Vec<Tok>> { Arc::new(s.chars().map(|c| Tok::Char(12, c)).collect()) };
    let reads = OutputReads {
        output_penalty: -123,
        insert_penalties: 4,
        dead_cycles: 2,
        marks: vec![(0, [Some(t("top")), Some(t("first")), Some(t("bot"))])],
        page_so_far: [65536 * 100, 65536 * 90, 0, 0, 0, 0, 0, 0],
    };
    let out = output(&d);
    let run = run_routine(&output_entry(&page, &reads, &out), &mut d.names);
    assert!(run.errors.is_empty(), "errors: {:?}", run.errors);
    assert_eq!(export(&run, &Key::Box(1)), Some(Val::Box(page.b.clone())));
    assert_eq!(export(&run, &Key::Count(1)), Some(Val::Int(-123)));
    assert_eq!(export(&run, &Key::Dimen(1)), Some(Val::Int(65536 * 100)));
    assert_eq!(export(&run, &Key::Dimen(2)), Some(Val::Int(65536 * 90)));
    assert_eq!(export(&run, &Key::Count(2)), Some(Val::Int(4)));
    // (§1025's increment)
    assert_eq!(export(&run, &Key::Count(3)), Some(Val::Int(3)));
    assert_eq!(export(&run, &Key::Toks(1)), Some(Val::Toks(t("top"))));
    assert_eq!(export(&run, &Key::Toks(2)), Some(Val::Toks(t("bot"))));
    // (the routine's `\insertpenalties` ends with it, §1026)
    assert_eq!(export(&run, &Key::Page(9)), None);
    let (end, left) = output_end(&run).expect("the routine ended");
    assert_eq!(end.dead_cycles, 3);
    assert!(matches!(left, [Item::Box(_), Item::Penalty(7), Item::Kern { .. }]), "left: {left:?}");
}
