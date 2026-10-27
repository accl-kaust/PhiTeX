//! Continuations as fields and relocated placements
//! (docs/spec-continuations.md): the rest of a cut piece is keyed by the
//! piece and the candidate, a changed continuation reruns it in place,
//! a moved page fire relocates it and keeps its finisher, and an
//! edit history ends where a cold build does.

mod common;

use phitex_tex::Options;

fn doc(n: usize, k: usize, mid: &str) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i == k {
            s.push_str(mid);
            s.push_str("\n\n");
        }
        for _ in 0..3 {
            s.push_str(&format!("Paragraph {i} of the document, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches. "));
        }
        s.push_str("\n\n");
    }
    s.push_str("\\bye\n");
    s
}

/// A group of four paragraphs, cut at each paragraph end inside it.
fn group_mid() -> String {
    let mut s = String::from("\\begingroup\\count5=1 ");
    for j in 0..4 {
        s.push_str(&format!("\\advance\\count5 by 1 Group paragraph {j}, with a counter stepped before it, long enough to make a few lines when set in the plain format's ten point roman over the text width, and then some more words to be sure.\n\n"));
    }
    s.push_str("\\endgroup");
    s
}

fn build(name: &str, src: &str, threads: usize) -> phitex_tex::Build {
    let dir = common::scratch(&format!("cuts-{name}-{threads}"));
    let opts = Options {
        threads: Some(threads),
        ..common::options("./d.tex", "d", "", &dir)
    };
    common::build(opts, Some(&common::plain()), src)
}

fn cold(name: &str, src: &str) -> Vec<u8> {
    build(&format!("{name}-cold"), src, 1).output().0
}

#[test]
fn a_rest_of_a_piece_is_keyed_by_the_piece_and_the_candidate() {
    let src = doc(60, 30, &group_mid());
    for threads in [1, 8] {
        let b = build("keyed", &src, threads);
        assert_eq!(b.output().0, cold("keyed", &src), "-j{threads}");
        let live = b.nodes_by_op("resume");
        assert!(live >= 3, "-j{threads}: the group is cut at its paragraph ends: {live}");
        assert_eq!(b.stats.placements_of("resume"), live, "-j{threads}: every rest placed once over the build: {:?}", b.stats.runs_by_op);
    }
}

#[test]
fn a_changed_continuation_reruns_the_rest_in_place() {
    let src = doc(60, 30, &group_mid());
    let mut b = build("cont", &src, 8);
    let edited = src.replacen("\\begingroup\\count5=1 ", "\\begingroup\\count5=3 ", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("cont-edited", &edited));
    assert_eq!(r.placements_made("resume"), 0, "the rests rerun in place, none made anew: {r:?}");
    assert!(r.placements_moved == 0 || r.placements_moved <= 4, "{r:?}");
}

#[test]
fn a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher() {
    let src = doc(80, 40, &group_mid());
    let mut b = build("fire", &src, 8);
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines, so the page firing inside the group moves to another of its candidates.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("fire-edited", &edited));
    assert!(r.placements_made("finish") <= 1, "finishers kept by key, at most one new page: {r:?}");
    assert_eq!(r.placements_made("resume"), 0, "the rests are relocated, not remade: {r:?}");
}

#[test]
fn a_paragraph_insert_history_equals_the_cold_build() {
    let base = format!(
        "\\headline={{\\hfil\\topmark\\ \\number\\count0\\hfil}}\n\\openout1=\\jobname.out\n{}",
        doc(80, 40, &format!("\\mark{{m\\number\\count0}}\\write1{{page \\number\\count0}}{}", group_mid()))
    );
    let mut b = build("history", &base, 8);
    assert_eq!(b.output().0, cold("history-0", &base));
    let e1 = base.replacen("Paragraph 0 of", "A first inserted paragraph that moves every page break by a few lines.\n\nParagraph 0 of", 1);
    common::rebuild(&mut b, &e1);
    assert_eq!(b.output().0, cold("history-1", &e1), "after the first insert");
    let e2 = e1.replacen("Paragraph 50 of", "Paragraph 50 OF", 1);
    common::rebuild(&mut b, &e2);
    assert_eq!(b.output().0, cold("history-2", &e2), "after the word edit");
    let e3 = e2.replacen("Paragraph 20 of", "A second inserted paragraph, before the group, moving its page fire again.\n\nParagraph 20 of", 1);
    common::rebuild(&mut b, &e3);
    assert_eq!(b.output().0, cold("history-3", &e3), "after the second insert");
}
