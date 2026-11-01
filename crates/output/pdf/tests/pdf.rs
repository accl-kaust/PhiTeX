//! The PDF writer against pdfTeX's own output.
//!
//! Each fixture `<name>.pdf` is `pdftex -ini <name>.tex` (pdfTeX
//! 1.40.29, TeX Live 2026) with `SOURCE_DATE_EPOCH=1 FORCE_SOURCE_DATE=1`;
//! the same pages are built here from the TFMs, as TeX builds them, and
//! the whole file must come out the same.

#![allow(clippy::unreadable_literal)]

use std::collections::BTreeMap;

use phitex_pdf::map::{MapEntry, parse_map};
use phitex_pdf::{FontSource, PdfParams, PdfWriter};
use phitex_tfm::{ScaledFont, Tfm};
use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::{CharInfo, FontId, Fonts, Left, LigKern};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxNode, KernKind, Node, RUNNING, Rule};
use phitex_typeset::pack::{NATURAL, PackParams, Spec, hpack, vpackage};

const PT: Scaled = 65536;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn tfm(name: &str) -> Tfm {
    let path = format!(
        "{}/../../fonts/tfm/tests/fixtures/{name}.tfm",
        env!("CARGO_MANIFEST_DIR")
    );
    phitex_tfm::parse(&std::fs::read(path).unwrap()).unwrap()
}

/// One loaded font.
struct Font {
    name: &'static str,
    tfm: Tfm,
    scaled: ScaledFont,
}

/// The fonts of a test: font `k+1` is `fonts[k]` (`\font\x=cmr10` is
/// TeX's font 1 in `IniTeX`).
struct Src {
    fonts: Vec<Font>,
    map: BTreeMap<Vec<u8>, MapEntry>,
}

impl Src {
    fn new(names: &[&'static str]) -> Src {
        Src {
            fonts: names
                .iter()
                .map(|&name| {
                    let tfm = tfm(name);
                    let scaled = tfm.at(tfm.design_size).unwrap();
                    Font { name, tfm, scaled }
                })
                .collect(),
            map: parse_map(&fixture("cm.map")),
        }
    }

    fn get(&self, f: FontId) -> &Font {
        &self.fonts[f as usize - 1]
    }

    /// The characters of `text` in font `f` with the font's kerns between
    /// them (tex.web §1034–§1040; these texts have no ligatures).
    fn word(&self, f: FontId, text: &[u8]) -> Vec<Node> {
        let font = self.get(f);
        let mut list = Vec::new();
        for (i, &c) in text.iter().enumerate() {
            list.push(Node::Char {
                font: f,
                ch: u16::from(c),
            });
            if let Some(&n) = text.get(i + 1)
                && let Some(step) = font.tfm.lookup(Some(c), Some(n))
            {
                assert!(step.is_kern(), "a ligature in {text:?}");
                list.push(Node::Kern(font.scaled.char_kern(step), KernKind::Normal));
            }
        }
        list
    }

    /// The interword glue of font `f` at space factor 1000 (§1042).
    fn space(&self, f: FontId) -> Node {
        let s = &self.get(f).scaled;
        Node::glue(Glue::new(s.space(), s.space_stretch(), s.space_shrink()))
    }

    fn hbox(&self, list: Vec<Node>, spec: Spec) -> BoxNode {
        hpack(list, spec, self, &PackParams::default(), false).node
    }
}

impl Fonts for Src {
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        let d = self.get(font).scaled.char(u8::try_from(ch).ok()?)?;
        Some(CharInfo {
            width: d.width,
            height: d.height,
            depth: d.depth,
            italic: d.italic,
            ..CharInfo::default()
        })
    }
    fn lig_kern(&self, _: FontId, _: Left, _: u16) -> Option<LigKern> {
        None
    }
    fn has_left_boundary(&self, _: FontId) -> bool {
        false
    }
    fn bchar(&self, _: FontId) -> Option<u16> {
        None
    }
    fn param(&self, font: FontId, n: u16) -> Scaled {
        self.get(font).scaled.param(usize::from(n))
    }
    fn param_count(&self, font: FontId) -> u16 {
        u16::try_from(self.get(font).scaled.params.len()).unwrap()
    }
}

impl FontSource for Src {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        &self.get(f).scaled
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.get(f).name.as_bytes()
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.map.get(self.name(f)).cloned()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        Some(fixture(std::str::from_utf8(m.ff_name.as_deref()?).ok()?))
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        Some(fixture(std::str::from_utf8(m.encname.as_deref()?).ok()?))
    }
}

/// pdfTeX's banner on the machine that made the fixtures.
const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

fn params(name: &str, compress_level: i32) -> PdfParams {
    PdfParams {
        compress_level,
        date: phitex_pdf::pdf_date(1),
        banner: BANNER.to_owned(),
        output_file_name: format!("{name}.pdf"),
        ..PdfParams::default()
    }
}

/// Ships `pages` and compares the file with `<name>.pdf`, showing the
/// first difference.
fn golden(name: &str, compress_level: i32, src: &Src, pages: &[BoxNode]) {
    let mut w = PdfWriter::new(params(name, compress_level));
    for p in pages {
        w.ship_out(p, src, &mut |_, _| None).unwrap();
    }
    let got = w.finish().unwrap();
    let want = fixture(&format!("{name}.pdf"));
    if got != want {
        let at = got
            .iter()
            .zip(&want)
            .position(|(a, b)| a != b)
            .unwrap_or(got.len().min(want.len()));
        let show = |b: &[u8]| {
            String::from_utf8_lossy(&b[at.saturating_sub(300)..(at + 200).min(b.len())])
                .into_owned()
        };
        panic!(
            "{name}.pdf: first difference at byte {at} (got {} bytes, want {})\n--- got:\n{}\n--- want:\n{}",
            got.len(),
            want.len(),
            show(&got),
            show(&want)
        );
    }
}

#[test]
fn dates() {
    assert_eq!(phitex_pdf::pdf_date(1), "D:19700101000001Z");
    assert_eq!(phitex_pdf::pdf_date(1_700_000_000), "D:20231114221320Z");
}

fn hi(src: &Src) -> BoxNode {
    src.hbox(src.word(1, b"Hi"), NATURAL)
}

#[test]
fn golden_hi() {
    // \font\x=cmr10 \x \shipout\hbox{Hi}, \pdfcompresslevel=0
    let src = Src::new(&["cmr10"]);
    golden("hi", 0, &src, &[hi(&src)]);
}

#[test]
fn golden_hi9() {
    // the same at \pdfcompresslevel=9
    let src = Src::new(&["cmr10"]);
    golden("hi9", 9, &src, &[hi(&src)]);
}

#[test]
fn golden_two() {
    // \shipout\vbox{\hbox{Hi}\kern3pt\hrule\hbox{there}}
    // \shipout\hbox{\y Bold} with \font\y=cmbx10: IniTeX's \baselineskip
    // and \lineskip are zero and no interline glue follows the rule
    // (prev_depth=ignore_depth); \boxmaxdepth is zero
    let src = Src::new(&["cmr10", "cmbx10"]);
    let rule = Rule {
        width: RUNNING,
        height: 26214,
        depth: 0,
    };
    let list = vec![
        Node::Box(Box::new(hi(&src))),
        Node::Kern(3 * PT, KernKind::Explicit),
        Node::Rule(rule),
        Node::Box(Box::new(src.hbox(src.word(1, b"there"), NATURAL))),
    ];
    let page1 = vpackage(list, NATURAL, 0, &PackParams::default()).node;
    let page2 = src.hbox(src.word(2, b"Bold"), NATURAL);
    golden("two", 0, &src, &[page1, page2]);
}

#[test]
fn golden_par() {
    // \hsize=100pt \tolerance=10000 \setbox0\vbox{\noindent The quick
    // brown fox jumps over the lazy dog again.\par}\shipout\box0: the
    // lines as TeX breaks them, each with \rightskip, the last with the
    // \penalty10000 and \parfillskip (zero) of §816; between them
    // \lineskip (zero: \baselineskip is zero)
    let src = Src::new(&["cmr10"]);
    let line = |words: &[&[u8]], last: bool| {
        let mut list = Vec::new();
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                list.push(src.space(1));
            }
            list.extend(src.word(1, w));
        }
        if last {
            list.push(Node::Penalty(10000));
            list.push(Node::glue(Glue::ZERO));
        }
        list.push(Node::glue(Glue::ZERO));
        Node::Box(Box::new(src.hbox(list, Spec::Exactly(100 * PT))))
    };
    let list = vec![
        line(&[b"The", b"quick", b"brown", b"fox"], false),
        Node::glue(Glue::ZERO),
        line(&[b"jumps", b"over", b"the", b"lazy"], false),
        Node::glue(Glue::ZERO),
        line(&[b"dog", b"again."], true),
    ];
    let page = vpackage(list, NATURAL, 0, &PackParams::default()).node;
    golden("par", 0, &src, &[page]);
}
