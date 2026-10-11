//! Edits: a one-word change re-reduces only its paragraph; speculation adopts.
mod common;
use common::*;

fn doc(n: usize) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\r=cmr10 \\r \\hsize=300pt \\vsize=600pt \
\\parindent=10pt \\baselineskip=12pt \\tolerance=1000 \\parfillskip=0pt plus 1fil\n\n",
    );
    for i in 0..n {
        s += &format!("Paragraph {i} has some words in it, enough to fill a line or two of text here.\n\n");
    }
    s + "\\end\n"
}

fn edit_word(b: &mut Build, src: &str, para: usize) -> Output {
    let at = src.find(&format!("Paragraph {para} has some")).unwrap() + format!("Paragraph {para} has ").len();
    let e = splice(b, at, at + 4, "many");
    b.update(vec![e])
}

#[test]
fn one_word_edit_re_reduces_one_paragraph() {
    let src = doc(60);
    let (mut b, cold) = build_with(opts(false, true), &src, &[]);
    assert!(cold.nodes_reduced > 0);
    let warm = edit_word(&mut b, &src, 30);
    // One paragraph is ~1/60 of the nodes; allow its line break, page fold
    // step and page stream on top.
    assert!(
        warm.nodes_reduced * 20 < cold.nodes_reduced,
        "edit reduced {} of {} nodes",
        warm.nodes_reduced,
        cold.nodes_reduced
    );
    assert!(warm.changed_pages.len() <= 1, "pages {:?}", warm.changed_pages);
}

#[test]
fn edit_output_equals_fresh_build() {
    let src = doc(20);
    let (mut b, _) = build_with(opts(false, true), &src, &[]);
    let warm = edit_word(&mut b, &src, 7);
    let at = src.find("Paragraph 7 has some").unwrap() + "Paragraph 7 has ".len();
    let edited = format!("{}many{}", &src[..at], &src[at + 4..]);
    let (_, fresh) = build_with(opts(false, true), &edited, &[]);
    assert_eq!(warm.pdf, fresh.pdf);
}

#[test]
fn no_op_update_reduces_nothing() {
    let src = doc(10);
    let (mut b, _) = build_with(opts(false, true), &src, &[]);
    let out = b.update(vec![]);
    assert_eq!(out.nodes_reduced, 0);
    assert!(out.changed_pages.is_empty());
}

#[test]
fn edit_and_undo_restores_bytes() {
    let src = doc(15);
    let (mut b, first) = build_with(opts(false, true), &src, &[]);
    let at = src.find("Paragraph 3 has some").unwrap() + "Paragraph 3 has ".len();
    let e = splice(&b, at, at + 4, "many");
    b.update(vec![e]);
    let e = splice(&b, at, at + 4, "some");
    let back = b.update(vec![e]);
    assert_eq!(back.pdf, first.pdf);
}

/// Speculative reduction (parallel, at predicted boundaries) is adopted only
/// when every read matches: the output equals the sequential one, also for a
/// document whose early catcode/macro changes defeat the prediction.
#[test]
fn speculation_adoption_is_exact() {
    let tricky = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\font\\r=cmr10 \\r \\hsize=300pt \\vsize=600pt \
\\baselineskip=12pt \\parfillskip=0pt plus 1fil\n\n\\def\\w{one}\n\n\\w\\ two.\n\n\\def\\w{three}\n\n\\w\\ four.\n\n\
\\catcode`\\Z=13 \\def Z{zed}\n\nZ and Z.\n\n"
        .to_string()
        + &"Plain paragraph text.\n\n".repeat(40)
        + "\\end\n";
    for src in [doc(80), tricky] {
        let seq = build_with(Options { threads: 1, ..opts(false, true) }, &src, &[]).1;
        let par = build_with(Options { threads: 8, ..opts(false, true) }, &src, &[]).1;
        assert_eq!(seq.pdf, par.pdf);
        assert_eq!(msgs(&seq), msgs(&par));
    }
}
