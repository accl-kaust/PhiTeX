//! A run parked inside its text and resumed holds what it held: here the
//! map a document piece's writes of a counter past the fixed locations
//! make (`crate::regfold`), across a park at the piece's page fold.

mod common;

/// Paragraphs that each step `\count300` globally, filling a few pages;
/// the count shown at the end.
fn stepping_paragraphs(pars: usize) -> String {
    use std::fmt::Write as _;
    let mut src = String::from("\\hsize=6in \\vsize=8.5in \\countdef\\c=300 \\global\\c=0\n");
    for k in 0..pars {
        for _ in 0..4 {
            let _ = write!(
                src,
                "Paragraph {k} has some words in it, enough to fill a few lines of text set \
                 with plain's macros, so that the pages fill up one after another. "
            );
        }
        src.push_str("\\global\\advance\\c by 1\n\n");
    }
    src.push_str("\\message{[count=\\the\\c]}\n\\bye\n");
    src
}

/// A piece parked at its fold (waiting for the page state) resumes with
/// its counter maps: a map of a counter past 255 is the location's whose
/// field the table notes, and the resumed run restores its table before
/// it maps the parked maps back to locations (else the counter's name is
/// lost and so is the piece's step).
#[test]
fn a_parked_piece_keeps_its_step_of_a_counter_past_255() {
    let pars = 40;
    let src = stepping_paragraphs(pars);
    let dir = common::fixtures().join("plain/vsize");
    let mut resumed = 0;
    for threads in [1, 4, 8] {
        let opts = phitex_tex::Options {
            threads: Some(threads),
            ..common::options("./steps.tex", "steps", "", &dir)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        let (_, log) = b.output();
        eprintln!(
            "{threads} threads: {} pages, stops {} resumed {}",
            b.pages().len(),
            b.stats.stops,
            b.stats.resumed
        );
        resumed += b.stats.resumed;
        assert!(
            log.contains(&format!("[count={pars}]")),
            "at {threads} threads the count is not {pars}:\n{}",
            common::log_excerpt(&log)
        );
    }
    assert!(resumed > 0, "no run parked and resumed: the test shows nothing");
}
