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
            Node::Box(hi),
            Node::Kern(3 * PT, KernKind::Explicit),
            Node::Rule(rule),
            Node::Box(there),
        ],
        ..BoxNode::default()
    };
    assert_eq!(ship(&expected, &page), expected);
}
