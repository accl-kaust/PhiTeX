//! The DVI writer against tex.web's examples and TeX's own output.

#![allow(clippy::unreadable_literal)]

use phitex_dvi::{Dir, DviWriter, FontDef, op};
use phitex_tfm::ScaledFont;
use phitex_typeset::node::{BoxKind, BoxNode, KernKind, Node, RUNNING, Rule};

/// The bytes a fresh writer has after its preamble.
fn body(w: &DviWriter, start: usize) -> Vec<u8> {
    w.bytes()[start..].to_vec()
}

#[test]
fn movement_digits() {
    // §609: 3 1 4 1 5 9 2 6 5 3 5 8 9 gets the subscripts
    // 3z 1y 4d 1y 5y 9d 2d 6d 5y 3z 5y 8d 9d
    let mut w = DviWriter::new(1000, "");
    let start = w.location();
    for d in [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9] {
        w.movement(d, Dir::Down);
    }
    let (d1, y0, y1, z0, z1) = (op::DOWN1, op::Y0, op::Y1, op::Z0, op::Z1);
    assert_eq!(
        body(&w, start),
        [
            z1, 3, y1, 1, d1, 4, y0, y1, 5, d1, 9, d1, 2, d1, 6, y0, z0, y0, d1, 8, d1, 9
        ]
    );
}

#[test]
fn movement_nested() {
    // §608: in 2y 7d 1d ( 8z 2y 8z ) 1, the earlier 1 becomes 1z
    let mut w = DviWriter::new(1000, "");
    let start = w.location();
    for d in [2, 7, 1] {
        w.movement(d, Dir::Right);
    }
    w.dvi_out(op::PUSH);
    let save_loc = w.location();
    for d in [8, 2, 8] {
        w.movement(d, Dir::Right);
    }
    w.prune_movements(save_loc);
    w.dvi_pop(save_loc);
    w.movement(1, Dir::Right);
    let (r1, w0, w1, x0, x1) = (op::RIGHT1, op::W0, op::W1, op::X0, op::X1);
    assert_eq!(
        body(&w, start),
        [w1, 2, r1, 7, x1, 1, op::PUSH, x1, 8, w0, x0, op::POP, x0]
    );
}

#[test]
fn movement_sizes() {
    // §610
    let mut w = DviWriter::new(1000, "");
    let start = w.location();
    w.movement(-128, Dir::Down);
    w.movement(128, Dir::Down);
    w.movement(-0o100000, Dir::Down);
    w.movement(0o40000000, Dir::Down);
    let d1 = op::DOWN1;
    assert_eq!(
        body(&w, start),
        [
            d1 + 1,
            0xff,
            0x80,
            d1 + 1,
            0,
            0x80,
            d1 + 2,
            0xff,
            0x80,
            0,
            d1 + 3,
            0,
            0x80,
            0,
            0
        ]
    );
}

#[test]
fn push_pop_cancel() {
    // §601
    let mut w = DviWriter::new(1000, "");
    let start = w.location();
    w.dvi_out(op::PUSH);
    let l = w.location();
    w.dvi_pop(l);
    assert_eq!(body(&w, start), []);
    w.dvi_four(-1);
    assert_eq!(body(&w, start), [0xff; 4]);
}

const PT: i32 = 65536;

fn cmr10() -> ScaledFont {
    let path = format!(
        "{}/../../fonts/tfm/tests/fixtures/cmr10.tfm",
        env!("CARGO_MANIFEST_DIR")
    );
    let tfm = phitex_tfm::parse(&std::fs::read(path).unwrap()).unwrap();
    tfm.at(10 * PT).unwrap()
}

fn font_def(f: &ScaledFont, checksum: [u8; 4]) -> FontDef {
    FontDef {
        checksum,
        size: f.size,
        design_size: f.design_size,
        area: Vec::new(),
        name: b"cmr10".to_vec(),
        widths: f.chars.iter().map(|c| c.map(|d| d.width)).collect(),
        native: None,
    }
}

/// `\hbox{<text>}` in font 1, packed by hand from the TFM (hpack, §649–§654:
/// no glue, no kerns or ligatures between these letters).
fn hbox(f: &ScaledFont, text: &[u8]) -> BoxNode {
    let mut b = BoxNode {
        kind: BoxKind::H,
        ..BoxNode::default()
    };
    for &c in text {
        let d = f.char(c).unwrap();
        b.width += d.width;
        b.height = b.height.max(d.height);
        b.depth = b.depth.max(d.depth);
        b.list.push(Node::Char {
            font: 1,
            ch: u16::from(c),
        });
    }
    b
}

/// The comment of a DVI file's preamble (§617), so that the date is TeX's.
fn comment(dvi: &[u8]) -> String {
    let k = usize::from(dvi[14]);
    String::from_utf8(dvi[15..15 + k].to_vec()).unwrap()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn ship(expected: &[u8], page: &BoxNode) -> Vec<u8> {
    let f = cmr10();
    let tfm_path = format!(
        "{}/../../fonts/tfm/tests/fixtures/cmr10.tfm",
        env!("CARGO_MANIFEST_DIR")
    );
    let checksum = phitex_tfm::parse(&std::fs::read(tfm_path).unwrap())
        .unwrap()
        .checksum;
    let def = font_def(&f, checksum);
    let mut w = DviWriter::new(1000, &comment(expected));
    w.ship_out(page, [0; 10], &|_| def.clone(), &mut |_, _| None)
        .unwrap();
    w.finish()
}

#[test]
fn golden_hi() {
    // tests/fixtures/hi.tex: tex -ini (TeX 3.141592653, TeX Live 2026)
    let expected = fixture("hi.dvi");
    let page = hbox(&cmr10(), b"Hi");
    assert_eq!(ship(&expected, &page), expected);
}

#[test]
fn golden_there() {
    // tests/fixtures/there.tex:
    // \shipout\vbox{\hbox{Hi}\kern3pt\hrule\hbox{there}}; initex's
    // \baselineskip and \lineskip are zero, and no interline glue
    // follows the rule (prev_depth=ignore_depth), so vpack (§668–§679)
    // by hand is a sum.
    let expected = fixture("there.dvi");
    let f = cmr10();
    let (hi, there) = (hbox(&f, b"Hi"), hbox(&f, b"there"));
    let rule = Rule {
        width: RUNNING,
        height: 26214,
        depth: 0,
    };
    let page = BoxNode {
        kind: BoxKind::V,
        width: hi.width.max(there.width),
        height: hi.height + hi.depth + 3 * PT + rule.height + rule.depth + there.height,
        depth: there.depth,
        list: vec![
            Node::Box(Box::new(hi)),
            Node::Kern(3 * PT, KernKind::Explicit),
            Node::Rule(rule),
            Node::Box(Box::new(there)),
        ],
        ..BoxNode::default()
    };
    assert_eq!(ship(&expected, &page), expected);
}

/// A page of `n` lines (a `vbox` of `hbox`es of text, kerns and empty
/// boxes between them), in font 1.
fn page_of(f: &ScaledFont, n: usize, seed: usize) -> BoxNode {
    let mut b = BoxNode {
        kind: BoxKind::V,
        ..BoxNode::default()
    };
    for k in 0..n {
        let words: [&[u8]; 4] = [b"the", b"quick", b"brown", b"fox"];
        let line = hbox(f, words[(k + seed) % 4]);
        b.height += line.height + line.depth + PT;
        b.width = b.width.max(line.width);
        b.list.push(Node::Box(Box::new(line)));
        b.list.push(Node::Kern(
            PT * i32::try_from((k + seed) % 3 + 1).unwrap(),
            KernKind::Explicit,
        ));
        if (k + seed).is_multiple_of(5) {
            b.list.push(Node::Box(Box::new(BoxNode {
                kind: BoxKind::H,
                ..BoxNode::default()
            })));
        }
    }
    b
}

#[test]
fn kept_pages_ship_again_as_written() {
    // §598, §601, §611: a small buffer, so that the pages' movements and
    // cancelled pushes read its state; each page kept, then shipped
    // again after a first page of every length, equal to writing it
    let f = cmr10();
    let def = font_def(&f, [1, 2, 3, 4]);
    let fonts = |_| def.clone();
    let pages: Vec<BoxNode> = (0..12).map(|k| page_of(&f, 3 + k * 2, k)).collect();
    let mut w = DviWriter::with_buf_size(1000, "", 64);
    let kept: Vec<_> = pages
        .iter()
        .map(|p| {
            w.ship_out_kept(p, [0; 10], &fonts, &mut |_, _| None)
                .unwrap()
        })
        .collect();
    let (mut reused, mut written) = (0, 0);
    for first in 0..24 {
        let lead = page_of(&f, first, first);
        let mut a = DviWriter::with_buf_size(1000, "", 64);
        let mut b = DviWriter::with_buf_size(1000, "", 64);
        a.ship_out(&lead, [0; 10], &fonts, &mut |_, _| None)
            .unwrap();
        b.ship_out(&lead, [0; 10], &fonts, &mut |_, _| None)
            .unwrap();
        for (p, k) in pages.iter().zip(&kept) {
            if a.ship_kept(k) {
                reused += 1;
            } else {
                written += 1;
                a.ship_out(p, [0; 10], &fonts, &mut |_, _| None).unwrap();
            }
            b.ship_out(p, [0; 10], &fonts, &mut |_, _| None).unwrap();
        }
        assert_eq!(
            a.finish(),
            b.finish(),
            "after a first page of {first} lines"
        );
    }
    eprintln!("kept pages: {reused} shipped again, {written} written");
    assert!(reused > 0 && written > 0);
}

#[test]
fn a_kept_page_defines_its_fonts_where_it_lands() {
    // §621: a kept page that defined a font is not shipped again where
    // the font is already defined, and the other way round
    let f = cmr10();
    let def = font_def(&f, [1, 2, 3, 4]);
    let fonts = |_| def.clone();
    let p = page_of(&f, 4, 0);
    let mut w = DviWriter::new(1000, "");
    let first = w
        .ship_out_kept(&p, [0; 10], &fonts, &mut |_, _| None)
        .unwrap();
    let second = w
        .ship_out_kept(&p, [0; 10], &fonts, &mut |_, _| None)
        .unwrap();
    let mut a = DviWriter::new(1000, "");
    assert!(!a.ship_kept(&second));
    assert!(a.ship_kept(&first));
    assert!(!a.ship_kept(&first));
    assert!(a.ship_kept(&second));
    assert_eq!(a.finish(), w.finish());
}

/// cmr10's metrics and Type 1 program, whatever the suffix asked: the
/// files xdvipdfmx reads for the pages below.
struct Cmr10Files;

impl phitex_xdvipdfmx::io::Files for Cmr10Files {
    fn find(&self, name: &[u8], f: phitex_xdvipdfmx::io::Format, _: &[u8]) -> Option<Vec<u8>> {
        use phitex_xdvipdfmx::io::Format;
        if !name.starts_with(b"cmr10") {
            return None;
        }
        match f {
            Format::Tfm => Some(b"cmr10.tfm".to_vec()),
            Format::Type1 => Some(b"cmr10.pfb".to_vec()),
            _ => None,
        }
    }
    fn read(&self, path: &[u8]) -> Option<std::sync::Arc<[u8]>> {
        let at = |p: &str| std::fs::read(format!("{}/{p}", env!("CARGO_MANIFEST_DIR"))).ok();
        match path {
            b"cmr10.tfm" => at("../../fonts/tfm/tests/fixtures/cmr10.tfm").map(std::sync::Arc::from),
            b"cmr10.pfb" => at("../pdf/tests/fixtures/cmr10.pfb").map(std::sync::Arc::from),
            _ => None,
        }
    }
}

/// Pages of lines, nested boxes (the empty ones only kerns: their
/// `push` cancelled), rules and specials: the PDF xdvipdfmx makes of the
/// XDV the writer writes, and the PDF its interpreter makes of the
/// operations the same writer says without writing bytes, each for a
/// buffer of `buf` bytes.
fn xdv_and_ops(buf: usize) -> (Vec<u8>, Vec<u8>) {
    use phitex_xdvipdfmx::api::{Options, Session};
    let f = cmr10();
    let tfm_path = format!(
        "{}/../../fonts/tfm/tests/fixtures/cmr10.tfm",
        env!("CARGO_MANIFEST_DIR")
    );
    let checksum = phitex_tfm::parse(&std::fs::read(tfm_path).unwrap())
        .unwrap()
        .checksum;
    let def = font_def(&f, checksum);
    let fonts = |_| def.clone();
    let specials: Vec<Vec<u8>> = vec![
        b"pdf:mapline cmr10 CMR10 <cmr10.pfb".to_vec(),
        b"pdf:literal 0 0 1 rg".to_vec(),
        b"pdf:dest (here) [@thispage /XYZ @xpos @ypos null]".to_vec(),
    ];
    let pages: Vec<BoxNode> = (0..9)
        .map(|k| {
            let mut p = page_of(&f, 4 + 3 * k, k);
            let mut inner = hbox(&f, b"nested");
            inner.list.push(Node::Box(Box::new(BoxNode {
                kind: BoxKind::H,
                width: PT,
                list: vec![Node::Kern(PT, KernKind::Explicit)],
                ..BoxNode::default()
            })));
            inner.list.push(Node::Whatsit(1 + u32::try_from(k % 2).unwrap(), None));
            inner.list.push(Node::Rule(Rule {
                width: 2 * PT,
                height: RUNNING,
                depth: RUNNING,
            }));
            p.list.insert(0, Node::Whatsit(0, None));
            p.list.push(Node::Box(Box::new(inner)));
            p.list.push(Node::Rule(Rule {
                width: RUNNING,
                height: PT / 2,
                depth: 0,
            }));
            p
        })
        .collect();
    let deflate = || -> phitex_xdvipdfmx::obj::Deflate {
        Box::new(|l, d: &[u8]| phitex_codecs::deflate::compress(d, l))
    };
    let options = || Options {
        pdf_filename: Some(b"pages.pdf".to_vec()),
        source_date_epoch: Some(0),
        ..Options::default()
    };
    let mut w = DviWriter::new_xdv_with(1000, " XeTeX output", buf, false);
    let mut o = DviWriter::new_xdv_with(1000, " XeTeX output", buf, true);
    let mut s = Session::direct(
        options(),
        std::sync::Arc::new(Cmr10Files),
        deflate(),
        1000,
        b" XeTeX output",
    )
    .unwrap();
    let mut direct = Vec::new();
    for p in &pages {
        let mut wh = |h: u32, _: bool| specials.get(h as usize).cloned();
        w.ship_out(p, [0; 10], &fonts, &mut wh).unwrap();
        o.ship_out(p, [0; 10], &fonts, &mut wh).unwrap();
        direct.extend(s.page_ops(&o.take_ops()).unwrap().pdf);
    }
    direct.extend(s.finish().unwrap());
    let xdv = w.finish();
    let port = Session::convert(options(), std::sync::Arc::new(Cmr10Files), deflate(), &xdv).unwrap();
    (port, direct)
}

#[test]
fn xdv_ops_make_the_xdvs_pdf() {
    // (the default buffer, and one so small that pushes are cancelled,
    // or not, across its halves, and movements find their entries gone)
    for buf in [phitex_dvi::DVI_BUF_SIZE, 64, 72] {
        let (port, direct) = xdv_and_ops(buf);
        assert!(port.starts_with(b"%PDF-"), "the port's PDF");
        let at = port.iter().zip(&direct).take_while(|(a, b)| a == b).count();
        assert!(
            port == direct,
            "buffer {buf}: the operations' PDF differs from the XDV's at byte {at} ({} and {} bytes)",
            port.len(),
            direct.len()
        );
    }
}
