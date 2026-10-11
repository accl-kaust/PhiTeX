//! Math layout against e-TeX with plain's fonts: each case of
//! `fixtures/math.txt` (made by `gen/math_fixture.py`) holds the mlist
//! `\showlists` prints before the closing `$` and the hlist TeX made of it
//! in a paragraph (so with penalties).

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

mod common;

use common::{FontSet, cases, parse_dimen, parse_list, set_font_names, show_list};
use phitex_typeset::fonts::FontId;
use phitex_typeset::glue::Glue;
use phitex_typeset::math::{
    Delimiter, Field, Fraction, Limits, MItem, MList, MathParams, Noad, NoadKind, Style,
    mlist_to_hlist,
};
use phitex_typeset::node::Node;

const SUBSIDIARY: &[u8] = b".^_/DTSs|";

/// Whether `rest` (a line after the current prefix) is an item, not a
/// subsidiary line: items start with `\` and a letter, or are `{}`.
fn is_item(rest: &str) -> bool {
    let b = rest.as_bytes();
    match b.first() {
        Some(b'\\') => b.get(1).is_some_and(u8::is_ascii_alphabetic),
        Some(c) => !SUBSIDIARY.contains(c),
        None => false,
    }
}

fn parse_char(s: &str) -> (u8, u16) {
    // "\famN c"
    let s = s.strip_prefix("\\fam").unwrap();
    let (n, c) = s.split_once(' ').unwrap();
    let b = c.as_bytes();
    let ch = if b.len() == 3 && b[0] == b'^' && b[1] == b'^' {
        let k = u16::from(b[2]);
        if k >= 64 { k - 64 } else { k + 64 }
    } else {
        u16::from(b[0])
    };
    (n.parse().unwrap(), ch)
}

fn parse_delim(hex: &str) -> Delimiter {
    let a = u32::from_str_radix(hex.trim_start_matches('"'), 16).unwrap();
    let (small, large) = (a >> 12, a & 0xFFF);
    Delimiter {
        small_fam: u8::try_from(small >> 8).unwrap(),
        small_char: u16::try_from(small & 0xFF).unwrap(),
        large_fam: u8::try_from(large >> 8).unwrap(),
        large_char: u16::try_from(large & 0xFF).unwrap(),
    }
}

/// The field shown by the lines `sub` with prefix `p` (§692).
fn parse_field(sub: &[&str], p: &str) -> Field {
    match sub.first() {
        None => Field::Empty,
        Some(l) => {
            let rest = &l[p.len()..];
            if rest.starts_with("\\fam") {
                let (fam, ch) = parse_char(rest);
                Field::MathChar { fam, ch }
            } else if rest == "{}" {
                Field::SubMlist(Vec::new())
            } else if rest.starts_with("\\hbox(") || rest.starts_with("\\vbox(") {
                match parse_list(sub, p).pop() {
                    Some(Node::Box(b)) => Field::SubBox(b),
                    _ => panic!("{l}"),
                }
            } else {
                Field::SubMlist(parse_mlist(sub, p))
            }
        }
    }
}

fn parse_mlist(lines: &[&str], prefix: &str) -> MList {
    let mut out = MList::new();
    let mut i = 0;
    while i < lines.len() {
        let head = &lines[i][prefix.len()..];
        assert!(is_item(head), "not an item: {}", lines[i]);
        let start = i + 1;
        i += 1;
        while i < lines.len() && !is_item(&lines[i][prefix.len()..]) {
            i += 1;
        }
        let sub = &lines[start..i];
        let field = |c: &str| -> Field {
            let p = format!("{prefix}{c}");
            let ls: Vec<&str> = sub
                .iter()
                .copied()
                .filter(|l| l.starts_with(&p) && (c != "\\" || !is_item(&l[prefix.len()..])))
                .collect();
            parse_field(&ls, &p)
        };
        let mlist = |c: &str| -> MList {
            match field(c) {
                Field::SubMlist(m) => m,
                Field::Empty => Vec::new(),
                f => panic!("{f:?}"),
            }
        };
        let noad = |kind: NoadKind| {
            MItem::Noad(Noad {
                kind,
                nucleus: field("."),
                supscr: field("^"),
                subscr: field("_"),
            })
        };
        let op = match head {
            "\\mathop" => Some(Limits::Normal),
            "\\mathop\\limits" => Some(Limits::Limits),
            "\\mathop\\nolimits" => Some(Limits::NoLimits),
            _ => None,
        };
        let item = if let Some(l) = op {
            noad(NoadKind::Op(l))
        } else if let Some(rest) = head.strip_prefix("\\radical") {
            noad(NoadKind::Radical(parse_delim(rest)))
        } else if let Some(rest) = head.strip_prefix("\\accent") {
            let (fam, ch) = parse_char(rest);
            noad(NoadKind::Accent(fam, ch))
        } else if let Some(rest) = head.strip_prefix("\\left") {
            MItem::Left(parse_delim(rest))
        } else if let Some(rest) = head.strip_prefix("\\right") {
            MItem::Right(parse_delim(rest), false)
        } else if let Some(rest) = head.strip_prefix("\\middle") {
            MItem::Right(parse_delim(rest), true)
        } else if let Some(rest) = head.strip_prefix("\\fraction, thickness ") {
            let mut parts = rest.split(", ");
            let t = parts.next().unwrap();
            let mut f = Fraction {
                thickness: if t == "= default" {
                    None
                } else {
                    Some(parse_dimen(t))
                },
                numerator: mlist("\\"),
                denominator: mlist("/"),
                left: Delimiter::default(),
                right: Delimiter::default(),
            };
            for p in parts {
                if let Some(d) = p.strip_prefix("left-delimiter ") {
                    f.left = parse_delim(d);
                } else if let Some(d) = p.strip_prefix("right-delimiter ") {
                    f.right = parse_delim(d);
                }
            }
            MItem::Fraction(f)
        } else if head == "\\mathchoice" {
            MItem::Choice(Box::new([mlist("D"), mlist("T"), mlist("S"), mlist("s")]))
        } else if let Some(s) = [
            "\\displaystyle",
            "\\textstyle",
            "\\scriptstyle",
            "\\scriptscriptstyle",
        ]
        .iter()
        .position(|s| *s == head)
        {
            MItem::Style(Style(u8::try_from(2 * s).unwrap()))
        } else {
            let kind = match head {
                "\\mathord" => Some(NoadKind::Ord),
                "\\mathbin" => Some(NoadKind::Bin),
                "\\mathrel" => Some(NoadKind::Rel),
                "\\mathopen" => Some(NoadKind::Open),
                "\\mathclose" => Some(NoadKind::Close),
                "\\mathpunct" => Some(NoadKind::Punct),
                "\\mathinner" => Some(NoadKind::Inner),
                "\\overline" => Some(NoadKind::Over),
                "\\underline" => Some(NoadKind::Under),
                "\\vcenter" => Some(NoadKind::Vcenter),
                _ => None,
            };
            if let Some(k) = kind {
                noad(k)
            } else {
                let mut all = vec![lines[start - 1]];
                all.extend_from_slice(sub);
                let mut n = parse_list(&all, prefix);
                assert_eq!(n.len(), 1, "{head}");
                MItem::Node(n.pop().unwrap())
            }
        };
        out.push(item);
    }
    out
}

#[test]
fn formulas_match_etex() {
    let (fonts, names) = FontSet::parse(include_str!("fixtures/math-fonts.txt"));
    set_font_names(names.clone());
    let id = |n: &str| Some(FontId::try_from(names.iter().position(|x| x == n).unwrap()).unwrap());
    let mut fam_fnt = [[None; 16]; 3];
    for (fam, sizes) in [
        (0, ["tenrm", "sevenrm", "fiverm"]),
        (1, ["teni", "seveni", "fivei"]),
        (2, ["tensy", "sevensy", "fivesy"]),
        (3, ["tenex", "tenex", "tenex"]),
        (6, ["tenbf", "sevenbf", "fivebf"]),
    ] {
        for (s, n) in sizes.iter().enumerate() {
            fam_fnt[s][fam] = id(n);
        }
    }
    fam_fnt[0][4] = id("tenit");
    let skew_names = names.clone();
    let skew = move |f: FontId| -> i32 {
        match skew_names[usize::try_from(f).unwrap()].as_str() {
            "teni" | "seveni" | "fivei" => 0o177,
            "tensy" | "sevensy" | "fivesy" => 0o60,
            _ => -1,
        }
    };
    let mu = |w: i32, st: i32, sh: i32| Glue::new(w * 65536, st * 65536, sh * 65536);
    let params = MathParams {
        fam_fnt,
        thin_mu_skip: mu(3, 0, 0),
        med_mu_skip: mu(4, 2, 4),
        thick_mu_skip: mu(5, 5, 0),
        script_space: 32768,
        null_delimiter_space: parse_dimen("1.2"),
        delimiter_factor: 901,
        delimiter_shortfall: 5 * 65536,
        bin_op_penalty: 700,
        rel_penalty: 500,
        skew_char: &skew,
    };
    let mut failures = Vec::new();
    for (n, (kv, expected)) in cases(include_str!("fixtures/math.txt"))
        .into_iter()
        .enumerate()
    {
        let text = &kv[0].1;
        let (mtext, htext) = expected.split_once("%\n").unwrap();
        let mlines: Vec<&str> = mtext.lines().collect();
        // an incompleat fraction (§219): the list so far is its denominator
        let mlist = match mlines.iter().position(|l| l.starts_with("this will")) {
            Some(k) => {
                let mut f = parse_mlist(&mlines[k + 1..], "");
                if let Some(MItem::Fraction(fr)) = f.first_mut() {
                    fr.denominator = parse_mlist(&mlines[..k], "");
                }
                f
            }
            None => parse_mlist(&mlines, ""),
        };
        let (hlist, diags) = mlist_to_hlist(mlist, Style::TEXT, true, &params, &fonts);
        let mut got = String::new();
        show_list(&hlist, "", &mut got);
        if got != htext || !diags.is_empty() {
            failures.push(format!(
                "case {n} ({text}) {diags:?}:\n--- expected\n{htext}--- got\n{got}"
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
