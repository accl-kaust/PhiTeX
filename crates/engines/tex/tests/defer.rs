//! Deferred reads of page-layer registers (docs/spec-deferred-reads.md):
//! a paragraph that puts the page number into a deferred `\write`, a
//! macro, a `\mark`, or back into the register makes no read of it; one
//! that tests or typesets it does. Plain TeX, where `\count0` is the
//! page number and the oracle is a cold build of the same text.

mod common;

use phitex_tex::Options;

fn runs_of_report(r: &phitex_tex::Report, op: &str) -> u64 {
    r.runs_by_op.iter().find(|(o, _)| *o == op).map_or(0, |(_, n)| *n)
}

/// `n` paragraphs long enough to fill several pages, with `mid` inserted
/// as a paragraph of its own after the first `k`.
fn doc(n: usize, k: usize, mid: &str) -> String {
    let mut s = String::from("\\openout1=\\jobname.out\n");
    for i in 0..n {
        if i == k {
            s.push_str(mid);
            s.push_str("\n\n");
        }
        let sentence = "long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches, so that pages fill as the text goes on and on.";
        s.push_str(&format!("Paragraph {i} of the document, {sentence} Again, {sentence} Once more, {sentence}\n\n"));
    }
    s.push_str("\\closeout1\n\\bye\n");
    s
}

/// Builds `src` cold and returns the DVI bytes and the text written to
/// stream 1.
fn cold(name: &str, src: &str, threads: usize) -> (Vec<u8>, String) {
    let dir = common::scratch(&format!("defer-{name}-cold"));
    let opts = Options {
        threads: Some(threads),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), src);
    let (dvi, _) = b.output();
    let out = b.writes().into_iter().filter(|(k, _)| *k == 1).map(|(_, t)| t).collect::<Vec<_>>().join("");
    (dvi, out)
}

/// The page number written by a deferred `\write` is the page the
/// paragraph is on, the paragraph never reads it, and when a paragraph
/// inserted before it moves it to the next page the written number
/// follows without the writing paragraph running again.
#[test]
fn a_deferred_write_of_the_page_number_is_resolved_at_shipout_without_a_read_in_the_piece() {
    let mid = "\\write1{page=\\number\\count0.}Marked paragraph, with a deferred write of its page number.";
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-write-{threads}"));
        let src = doc(120, 60, mid);
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let mut b = common::build(opts, Some(&common::plain()), &src);
        let (want_dvi, want_out) = cold("write", &src, 1);
        assert_eq!(b.output().0, want_dvi, "-j{threads}: cold at one thread");
        assert!(want_out.contains("page=") && !want_out.contains("page=1."), "-j{threads}: the write names its page: {want_out:?}");
        assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "-j{threads}: no paragraph read the page number: {:?}", b.stats.refuted_by_field);
        // a paragraph inserted at the front: every page break moves
        let edited = doc(121, 60, mid).replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines, so the marked paragraph moves.\n\nParagraph 0 of", 1);
        common::rebuild(&mut b, &edited);
        let (want_dvi, want_out) = cold("write-edited", &edited, 1);
        assert_eq!(b.output().0, want_dvi, "-j{threads}: the edited document equals its cold build");
        let got_out = b.writes().into_iter().filter(|(k, _)| *k == 1).map(|(_, t)| t).collect::<Vec<_>>().join("");
        assert_eq!(got_out, want_out, "-j{threads}: the written page number followed the page");
        assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "-j{threads}: still no read of the page number by a paragraph");
    }
}

/// The page number stored in a macro nothing expands is no read.
#[test]
fn a_page_number_in_a_macro_never_expanded_is_no_read() {
    let mid = "\\edef\\thisbox{\\number\\count0}Paragraph that saves its page number into a macro and never uses it.";
    let src = doc(120, 60, mid);
    let dir = common::scratch("defer-macro");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    assert_eq!(b.output().0, cold("macro", &src, 1).0);
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "{:?}", b.stats.refuted_by_field);
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("macro-edited", &edited, 1).0);
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "after the edit: {r:?} {:?}", b.stats.refuted_by_field);
}

/// A conditional on the page number needs the digits: the paragraph
/// reads it, is refuted when its page moves, and its text follows.
#[test]
fn a_conditional_on_the_page_number_forces_the_read() {
    let mid = "\\edef\\thisbox{\\number\\count0}This paragraph is on an \\ifodd\\thisbox odd\\else even\\fi\\ page, and says so in its text.";
    let src = doc(120, 60, mid);
    let dir = common::scratch("defer-cond");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    assert_eq!(b.output().0, cold("cond", &src, 1).0);
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines, enough to move the marked paragraph to the next page.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("cond-edited", &edited, 1).0, "the odd/even word follows the page");
    assert!(runs_of_report(&r, "exec") + runs_of_report(&r, "resume") >= 2, "the conditional's paragraph ran again when its page moved: {r:?}");
}

/// cleveref's probe: save the page number, set it to 1, format it,
/// restore it. Neither a read nor a definition of the page number by the
/// paragraph, so a page move leaves it alone and nothing after it
/// reaches a definition of its making.
#[test]
fn a_save_and_restore_of_the_page_number_is_neither_a_read_nor_a_definition() {
    let mid = "Paragraph that probes the page number's format \\edef\\saved{\\count0=\\the\\count0\\relax}\\global\\count0=1 \\edef\\probe{\\number\\count0}\\global\\saved and puts the number back, then goes on for a while to make lines.";
    let src = doc(120, 60, mid);
    let dir = common::scratch("defer-restore");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    assert_eq!(b.output().0, cold("restore", &src, 1).0, "page numbers after the probe are TeX's");
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "{:?}", b.stats.refuted_by_field);
    assert_eq!(b.definitions_by_op("exec", "count:0") + b.definitions_by_op("resume", "count:0"), 0, "no paragraph defined the page number");
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("restore-edited", &edited, 1).0);
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "after the edit: {r:?} {:?}", b.stats.refuted_by_field);
}

/// A `\mark` holding the page number is resolved by the finisher that
/// shows it in the headline: the paragraph makes no read.
#[test]
fn a_mark_with_the_page_number_is_resolved_by_the_finisher() {
    let mid = "\\mark{\\number\\count0}Paragraph that marks its page number for the headline.";
    let src = format!("\\headline={{\\hfil\\topmark\\hfil}}\n{}", doc(120, 60, mid));
    let dir = common::scratch("defer-mark");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    assert_eq!(b.output().0, cold("mark", &src, 1).0);
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "{:?}", b.stats.refuted_by_field);
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("mark-edited", &edited, 1).0, "the headline follows the page");
    assert_eq!(b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"), 0, "after the edit: {r:?} {:?}", b.stats.refuted_by_field);
}

/// The fixture `oracle/fixtures/dvi/defer.tex` (every shape above in one
/// document) against pdfTeX's DVI, made once on accl.
#[test]
fn the_defer_fixture_is_byte_identical_to_pdftex() {
    let dir = common::fixtures().join("dvi");
    let tex = dir.join("defer.tex");
    let src = common::read(&tex);
    let want = std::fs::read(dir.join("defer.dvi")).expect("the oracle, made with pdfTeX on accl");
    for threads in [1, 8] {
        let opts = Options {
            threads: Some(threads),
            ..common::options("./defer.tex", "defer", &common::dvi_comment(&want), &dir)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        let (dvi, log) = b.output();
        assert_eq!(dvi, want, "-j{threads}: DVI differs; log tail:\n{}", common::log_excerpt(&log));
    }
}
