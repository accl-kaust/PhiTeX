//! The light page steps (OUTPUT.md §4.2a, §4.7): `\vsize` set by the
//! output routine reaches the next page, and an edit that moves no page
//! break re-runs no later page step.

mod common;

fn vsize_doc() -> (std::path::PathBuf, String, Vec<u8>) {
    let dir = common::fixtures().join("plain/vsize");
    let src = common::read(&dir.join("vs.tex"));
    let want = std::fs::read(dir.join("vs.dvi")).expect("the oracle");
    (dir, src, want)
}

#[test]
fn vsize_from_the_output_routine() {
    let (dir, src, want) = vsize_doc();
    let opts = common::options("./vs.tex", "vs", &common::dvi_comment(&want), &dir);
    let b = common::build(opts, Some(&common::plain()), &src);
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("vs.dvi: {d}");
    }
    assert!(b.stats.page_steps > 0, "no light page steps");
}

#[test]
fn an_edit_on_the_last_page_reruns_no_earlier_page() {
    let (dir, src, want) = vsize_doc();
    let comment = common::dvi_comment(&want);
    let opts = || common::options("./vs.tex", "vs", &comment, &dir);
    let mut b = common::build(opts(), Some(&common::plain()), &src);
    let pages = b.pages();
    let cold_steps = b.stats.page_steps;
    // a word of the last paragraph, same length: no break moves
    let at = src.rfind("Paragraph 39: the quick").expect("the last paragraph") + 18;
    let new = format!("{}kciuq{}", &src[..at], &src[at + 5..]);
    b.edit(&new);
    let after = b.pages();
    assert_eq!(pages.len(), after.len());
    assert_eq!(pages[..pages.len() - 1], after[..after.len() - 1]);
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "edit != cold"
    );
    eprintln!(
        "page steps: cold {cold_steps}, after the edit {}",
        b.stats.page_steps
    );
    assert!(
        b.stats.page_steps * 4 <= cold_steps.max(4),
        "an edit at the end re-ran {} of {cold_steps} page steps",
        b.stats.page_steps
    );
}
