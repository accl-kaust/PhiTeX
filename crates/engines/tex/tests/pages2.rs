//! The page glue rewritten (crates/engines/tex/src/pages: one page-builder
//! node with checkpoints, one finisher per page keyed by its page): the
//! structure these tests assert is the design (docs/spec-layers.md §2,
//! §4–§5), and the bytes are the oracles'. They are the spec of the
//! rewrite beside `pages/SPEC.md`; the old suites (`pages`, `page_steps`,
//! `plain`, `edits`, `pdftex`, `latex`) are its gate.

mod common;

use std::fmt::Write as _;

use phitex_tex::Options;

/// The runs of an operation in a build.
fn runs_of(b: &phitex_tex::Build, op: &str) -> u64 {
    b.stats
        .runs_by_op
        .iter()
        .find(|(o, _)| *o == op)
        .map_or(0, |(_, n)| *n)
}

/// The live nodes of an operation.
fn nodes_of(b: &phitex_tex::Build, op: &str) -> usize {
    b.nodes_by_op(op)
}

/// A plain document of `n` paragraphs over several pages (bench/para400's
/// shape, smaller).
fn paragraphs(n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "Paragraph {i} of the document, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches, so that pages fill.\n\n"
        );
    }
    s.push_str("\\bye\n");
    s
}

/// The page loop's nodes since cf684671c (a node per stretch of pages,
/// `pages::fold_order_runs`): the main node, a rest per routine fire (one
/// per page here), and the last rest, which finds no start and stays
/// parked; and exactly one finisher per page shipped, at one and eight
/// threads: no loop node and no finisher made anew for a page that did
/// not move.
#[test]
fn one_loop_node_and_one_finisher_per_page() {
    let dir = common::scratch("pages2-one");
    let src = paragraphs(120);
    for threads in [1, 8] {
        let opts = Options {
            threads: Some(threads),
            ..common::options("./p.tex", "p", "", &dir)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        let pages = b.pages().len();
        assert!(pages >= 3, "a document of several pages: {pages}");
        assert_eq!(
            nodes_of(&b, "pages"),
            pages + 2,
            "-j{threads}: a loop node per page fire, the main node and the last rest: {:?}",
            b.stats.placed_by_op
        );
        assert_eq!(
            b.stats.placements_of("pages"),
            pages + 2,
            "-j{threads}: every loop node placed once, none made anew: {:?}",
            b.stats.placed_by_op
        );
        assert_eq!(
            nodes_of(&b, "finish"),
            pages,
            "-j{threads}: one finisher per page shipped"
        );
        assert_eq!(
            b.stats.placements_of("finish"),
            pages,
            "-j{threads}: every finisher placed once, none made anew: {:?}",
            b.stats.runs_by_op
        );
        assert!(
            runs_of(&b, "finish") <= 2 * pages as u64,
            "-j{threads}: a finisher runs on its prediction and at most once more: {:?}",
            b.stats.runs_by_op
        );
    }
}

/// The loop meets holes (pieces not yet committed at eight threads) and
/// parks: every run of a loop node after its first goes on from where it
/// parked (`Stats::resumptions`), none starts its stretch of pages again,
/// and no loop node is made anew (the main node, a rest per page and the
/// last rest, `pages::fold_order_runs`); and it never folds a
/// contributor twice (the engine's `park` test shape, on the document).
#[test]
fn the_loop_parks_at_holes_and_resumes() {
    let dir = common::scratch("pages2-park");
    let src = paragraphs(120);
    let opts = Options {
        threads: Some(8),
        ..common::options("./p.tex", "p", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), &src);
    let pages = b.pages().len() as u64;
    let loop_runs = runs_of(&b, "pages");
    let nodes = b.stats.placements_of("pages") as u64;
    eprintln!(
        "loop runs {loop_runs}, resumptions {}, loop nodes {nodes}, pages {pages}: {:?}",
        b.stats.resumptions, b.stats.runs_by_op
    );
    assert_eq!(
        nodes,
        pages + 2,
        "loop nodes for {pages} pages: none made anew ({:?})",
        b.stats.placed_by_op
    );
    assert_eq!(
        loop_runs - b.stats.resumptions,
        nodes,
        "the loop ran {loop_runs} times, {} of them resumptions, in {nodes} nodes: it parks and resumes, it does not restart ({:?})",
        b.stats.resumptions,
        b.stats.runs_by_op
    );
    assert_eq!(
        b.stats.units_folded("pages"),
        b.stats.units_contributed("vprof"),
        "every profile unit folded exactly once over all runs of the loop"
    );
}

/// A one-word edit that moves no page break: the edited piece, its
/// page's finisher, the link and the end run; the loop resumes from the
/// checkpoint before the piece and stops at the first page whose
/// content agrees, so no other finisher runs and none is made anew
/// (the user's bar: a one-letter edit is four runs).
#[test]
fn a_word_edit_reruns_the_piece_its_finisher_and_the_writers() {
    let dir = common::scratch("pages2-word");
    let src = paragraphs(120);
    let opts = Options {
        threads: Some(8),
        ..common::options("./p.tex", "p", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    let before = b.pages().clone();
    let edited = src.replacen("Paragraph 60 of", "Paragraph 60 OF", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(
        runs_of_report(&r, "exec") + runs_of_report(&r, "resume"),
        1,
        "one piece: {r:?}"
    );
    assert_eq!(
        runs_of_report(&r, "finish"),
        1,
        "one finisher, the edited piece's page: {r:?}"
    );
    assert!(
        runs_of_report(&r, "pages") <= 1,
        "the loop at most once, from its checkpoint (none where the edited paragraph's profile is unchanged: exact reads wake nobody): {r:?}"
    );
    assert_eq!(
        r.placements_made("finish"),
        0,
        "no finisher made anew: {r:?}"
    );
    assert!(r.runs <= 5, "piece, finisher, loop, link, end: {r:?}");
    let after = b.pages();
    assert_eq!(before.len(), after.len());
    assert!(
        before
            .iter()
            .zip(after.iter())
            .enumerate()
            .all(|(i, (x, y))| x == y || i == page_of_paragraph(&b, 60)),
        "only the edited page's bytes moved"
    );
}

/// A paragraph inserted at the front moves every page break: every page
/// from the first is fired again, but a finisher whose page content is
/// unchanged in value is not re-asked (the loop's early stop never
/// triggers here, so this measures the fire-again cost alone): the
/// finishers run at most once each, each loop node (the main node and
/// its rests, `pages::fold_order_runs`) once from its start, its other
/// runs going on from where it parked, at most the new page's rest made
/// anew, and no page's key below the inserted piece changes (keys are
/// `(piece, nth)`).
#[test]
fn a_paragraph_insert_refires_the_pages_after_it_once() {
    let dir = common::scratch("pages2-insert");
    let src = paragraphs(120);
    let opts = Options {
        threads: Some(8),
        ..common::options("./p.tex", "p", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    let pages = b.pages().len() as u64;
    let edited = src.replacen("Paragraph 1 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 1 of", 1);
    let r = common::rebuild(&mut b, &edited);
    // (the build's stats are the edit's)
    let fresh = runs_of_report(&r, "pages").saturating_sub(b.stats.resumptions);
    eprintln!(
        "edit: loop runs {}, resumptions {}, loop nodes {}: {r:?}",
        runs_of_report(&r, "pages"),
        b.stats.resumptions,
        nodes_of(&b, "pages")
    );
    assert!(
        fresh <= nodes_of(&b, "pages") as u64,
        "each loop node once from its start: {fresh} of {} nodes: {r:?}",
        nodes_of(&b, "pages")
    );
    assert!(
        r.placements_made("pages") <= 1,
        "at most the new page's rest made anew: {r:?}"
    );
    assert!(
        runs_of_report(&r, "finish") <= pages + 1,
        "each finisher at most once: {r:?}"
    );
    assert!(
        r.placements_made("finish") <= 1,
        "at most the one new page's finisher made anew; the others are kept by key: {r:?}"
    );
}

/// `\pagegoal` and `\pagetotal` read in the text answer as TeX's (the
/// dvi/pagegoal and dvi/pagequant oracles), and a piece that reads them
/// is re-asked only when the page it is on moves, not when any page
/// does.
#[test]
fn page_dimensions_read_in_text_are_exact_and_fine() {
    for name in ["pagegoal", "pagequant"] {
        let dir = common::fixtures().join("dvi");
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(dir.join(format!("{name}.dvi"))).expect("the oracle");
        let opts = common::options(
            &format!("./{name}.tex"),
            name,
            &common::dvi_comment(&want),
            &dir,
        );
        let b = common::build(opts, None, &src);
        let (dvi, log) = b.output();
        assert_eq!(
            dvi,
            want,
            "{name}: DVI differs; log tail:\n{}",
            common::log_excerpt(&log)
        );
        assert!(
            b.stats.refuted_by("pages:start") <= b.pages().len(),
            "{name}: a reader of the page is re-asked at most once per page: {:?}",
            b.stats.refuted_by_field
        );
    }
}

/// The page oracles the old glue carried, run as a gate: every page
/// fixture under oracle/fixtures/dvi and the pdf documents the spec
/// names; byte-identical.
#[test]
fn page_fixtures_are_byte_identical() {
    for name in [
        "pagegoal",
        "pagequant",
        "inserts",
        "outinserts",
        "vsplit",
        "outmarks",
        "outpen",
        "outroutine",
        "outsame",
        "endmid",
        "ship",
        "writes",
        "vsizeout",
        "errors100",
    ] {
        let dir = common::fixtures().join("dvi");
        let tex = dir.join(format!("{name}.tex"));
        if !tex.is_file() {
            continue;
        }
        let src = common::read(&tex);
        let want = std::fs::read(dir.join(format!("{name}.dvi"))).expect("the oracle");
        let opts = common::options(
            &format!("./{name}.tex"),
            name,
            &common::dvi_comment(&want),
            &dir,
        );
        let b = common::build(opts, None, &src);
        let (dvi, log) = b.output();
        assert_eq!(
            dvi,
            want,
            "{name}: DVI differs; log tail:\n{}",
            common::log_excerpt(&log)
        );
    }
}

fn runs_of_report(r: &phitex_tex::Report, op: &str) -> u64 {
    r.runs_by_op
        .iter()
        .find(|(o, _)| *o == op)
        .map_or(0, |(_, n)| *n)
}

fn page_of_paragraph(b: &phitex_tex::Build, i: usize) -> usize {
    b.page_of_text(&format!("Paragraph {i} "))
}
