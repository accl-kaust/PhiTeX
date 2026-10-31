//! Line breaking against e-TeX: `tests/fixtures/linebreak.txt` is made by
//! `tests/gen/linebreak.py` and holds `\showbox` of paragraphs of rules,
//! glue, penalties, kerns and discretionaries under varied parameters.

mod common;

use common::{NoFonts, cases, parse_glue, show_list};
use phitex_typeset::glue::Glue;
use phitex_typeset::linebreak::{LineBreakParams, NoHyphenation, interline_glue, line_break};
use phitex_typeset::node::{BoxKind, Disc, GlueKind, GlueNode, KernKind, Node, RUNNING, Rule};
use phitex_typeset::pack::{NATURAL, PackParams, vpack};

fn rule(w: &str) -> Node {
    Node::Rule(Rule {
        width: w.parse().unwrap(),
        height: 65536,
        depth: RUNNING,
    })
}

fn list(spec: &str) -> Vec<Node> {
    spec.split(' ')
        .map(|t| {
            let (c, v) = t.split_at(1);
            match c {
                "w" => rule(v),
                "g" => {
                    let g = parse_glue(v);
                    Node::Glue(GlueNode {
                        spec: Glue { ..g },
                        kind: GlueKind::Normal,
                        own_spec: false,
                        leader: None,
                    })
                }
                "p" => Node::Penalty(v.parse().unwrap()),
                "k" => Node::Kern(v.parse().unwrap(), KernKind::Explicit),
                "d" => {
                    let p: Vec<Vec<Node>> = v
                        .split(',')
                        .map(|x| if x == "0" { vec![] } else { vec![rule(x)] })
                        .collect();
                    Node::Disc(Box::new(Disc {
                        pre: p[0].clone(),
                        post: p[1].clone(),
                        replace: p[2].clone(),
                    }))
                }
                _ => panic!("{t}"),
            }
        })
        .collect()
}

fn ints(s: &str) -> Vec<i32> {
    s.split_whitespace().map(|x| x.parse().unwrap()).collect()
}

#[test]
fn paragraphs_match_etex() {
    let text = include_str!("fixtures/linebreak.txt");
    let mut failures = Vec::new();
    for (n, (kv, expected)) in cases(text).into_iter().enumerate() {
        let get = |k: &str| kv.iter().find(|x| x.0 == k).map(|x| x.1.as_str()).unwrap();
        let int = |k: &str| get(k).parse::<i32>().unwrap();
        let p = LineBreakParams {
            pretolerance: int("pretolerance"),
            tolerance: int("tolerance"),
            emergency_stretch: int("emergencystretch"),
            looseness: int("looseness"),
            line_penalty: int("linepenalty"),
            hyphen_penalty: int("hyphenpenalty"),
            ex_hyphen_penalty: int("exhyphenpenalty"),
            adj_demerits: int("adjdemerits"),
            double_hyphen_demerits: int("doublehyphendemerits"),
            final_hyphen_demerits: int("finalhyphendemerits"),
            left_skip: parse_glue(get("leftskip")),
            right_skip: parse_glue(get("rightskip")),
            par_fill_skip: parse_glue(get("parfillskip")),
            hsize: int("hsize"),
            hang_indent: int("hangindent"),
            hang_after: int("hangafter"),
            par_shape: get("parshape")
                .split_whitespace()
                .map(|x| {
                    let (a, b) = x.split_once(':').unwrap();
                    (a.parse().unwrap(), b.parse().unwrap())
                })
                .collect(),
            last_line_fit: int("lastlinefit"),
            inter_line_penalty: int("interlinepenalty"),
            club_penalty: int("clubpenalty"),
            widow_penalty: int("widowpenalty"),
            broken_penalty: int("brokenpenalty"),
            inter_line_penalties: ints(get("interlinepenalties")),
            club_penalties: ints(get("clubpenalties")),
            widow_penalties: ints(get("widowpenalties")),
            pack: PackParams {
                badness: 10000,
                fuzz: 1000 * 65536,
                overfull_rule: 0,
            },
            ..LineBreakParams::default()
        };
        let para = line_break(list(get("list")), false, &p, &NoFonts, &mut NoHyphenation);
        let mut v = Vec::new();
        let mut prev_depth = -65_536_000;
        for l in para.lines {
            if let Some(g) =
                interline_glue(prev_depth, l.packed.node.height, Glue::ZERO, Glue::ZERO, 0)
            {
                v.push(g);
            }
            prev_depth = l.packed.node.depth;
            v.push(Node::Box(Box::new(l.packed.node)));
            v.extend(l.packed.adjustments);
            if let Some(pen) = l.penalty {
                v.push(Node::Penalty(pen));
            }
        }
        let mut b = vpack(v, NATURAL, &PackParams::default()).node;
        b.kind = BoxKind::V;
        let mut got = String::new();
        show_list(&[Node::Box(Box::new(b))], "", &mut got);
        if got != expected {
            failures.push(format!("case {n}:\n--- expected\n{expected}--- got\n{got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failing\n{}",
        failures.len(),
        failures.join("\n")
    );
}
