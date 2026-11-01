//! Hyphenation against Knuth's TeX with plain.fmt: each case of
//! `fixtures/hyphenate.txt` (made by `gen/hyph.py`) holds the list TeX
//! built for a text in cmr10 and the paragraph after a second pass that
//! offers every word to the hyphenation routine.

mod common;

use common::{TableFont, cases, parse_list, show_list};
use phitex_typeset::glue::{Glue, Order};
use phitex_typeset::hyph::{Patterns, TexHyphenator};
use phitex_typeset::linebreak::{LineBreakParams, line_break};
use phitex_typeset::node::{BoxKind, Node};
use phitex_typeset::pack::{NATURAL, PackParams, vpack};

/// plain TeX's `\lccode`s.
fn lc(c: u16) -> u16 {
    match c {
        0x61..=0x7a => c,
        0x41..=0x5a => c + 32,
        _ => 0,
    }
}

fn plain_patterns() -> Patterns {
    let text = include_str!("fixtures/hyphen.tex");
    let body: String = text
        .lines()
        .map(|l| l.split('%').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    let (_, rest) = body.split_once("\\patterns{").unwrap();
    let (pats, rest) = rest.split_once('}').unwrap();
    let (_, rest) = rest.split_once("\\hyphenation{").unwrap();
    let (exc, _) = rest.split_once('}').unwrap();
    let mut p = Patterns::default();
    for w in pats.split_whitespace() {
        let r = p.add_pattern_str(0, w, &lc);
        assert!(!r.nonletter && !r.duplicate, "{w}");
    }
    for w in exc.split_whitespace() {
        assert!(p.add_exception_str(0, w, &lc));
    }
    p
}

#[test]
fn plain_patterns_hyphenate_words() {
    let p = plain_patterns();
    let word = |s: &str| s.bytes().map(u16::from).collect::<Vec<_>>();
    let show = |s: &str| {
        let w = word(s);
        let hyf = p.hyphenate_word(0, &w, 2, 3).unwrap();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            out.push(c);
            if hyf[i + 1] & 1 != 0 {
                out.push('-');
            }
        }
        out
    };
    assert_eq!(show("hyphenation"), "hy-phen-ation");
    assert_eq!(show("difficult"), "dif-fi-cult");
    assert_eq!(show("reciprocity"), "reci-procity");
    assert_eq!(show("table"), "ta-ble");
    assert_eq!(show("concatenation"), "con-cate-na-tion");
}

#[test]
fn paragraphs_match_tex() {
    let font = TableFont::parse(include_str!("fixtures/cmr10.txt"));
    let patterns = plain_patterns();
    let hyphen_char = |_| 45;
    let mut failures = Vec::new();
    for (n, (kv, expected)) in cases(include_str!("fixtures/hyphenate.txt"))
        .into_iter()
        .enumerate()
    {
        let text = &kv[0].1;
        let lines: Vec<&str> = expected.lines().collect();
        let split = 1 + lines[1..]
            .iter()
            .position(|l| l.starts_with("\\vbox"))
            .unwrap();
        // the first pass's single line, without \parfillskip and its penalty
        // and \rightskip
        let mut input = parse_list(&lines[2..split], "..");
        input.truncate(input.len() - 3);
        let want = lines[split..].join("\n") + "\n";
        let params = LineBreakParams {
            pretolerance: -1,
            tolerance: 10000,
            hsize: 16000 * 65536,
            par_fill_skip: Glue {
                stretch: 65536,
                stretch_order: Order::Fil,
                ..Glue::ZERO
            },
            pack: PackParams {
                badness: 10000,
                ..PackParams::default()
            },
            ..LineBreakParams::default()
        };
        let mut h = TexHyphenator {
            patterns: &patterns,
            lc_code: &lc,
            uc_hyph: 1,
            hyphen_char: &hyphen_char,
        };
        let para = line_break(input, false, &params, &font, &mut h);
        let v: Vec<Node> = para
            .lines
            .into_iter()
            .map(|l| Node::Box(Box::new(l.packed.node)))
            .collect();
        let mut b = vpack(v, NATURAL, &PackParams::default()).node;
        b.kind = BoxKind::V;
        let mut got = String::new();
        show_list(&[Node::Box(Box::new(b))], "", &mut got);
        if got != want {
            failures.push(format!(
                "case {n} ({text}):\n--- expected\n{want}--- got\n{got}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failing\n{}",
        failures.len(),
        failures.join("\n")
    );
}
