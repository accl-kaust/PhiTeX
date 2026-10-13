//! A document's `\input` files are sliced into paragraph regions like
//! the document's own text (`docs/TARGET_GRAPH.md`): the output equals
//! TeX's, each paragraph of an input file is about one node, and an
//! edit of one word in an input file reruns only that file's region.

mod common;

use std::path::{Path, PathBuf};

const PARAGRAPHS: u64 = 6;

fn fx() -> PathBuf {
    common::fixtures().join("inputs")
}

fn build(dir: &Path) -> phitex_tex::Build {
    let src = common::read(&fx().join("inputs.tex"));
    let want = std::fs::read(fx().join("inputs.dvi")).expect("inputs.dvi");
    let opts = common::options("./inputs.tex", "inputs", &common::dvi_comment(&want), dir);
    common::build(opts, Some(&common::plain()), &src)
}

fn stats(what: &str, b: &phitex_tex::Build) {
    let s = &b.stats;
    eprintln!(
        "{what}: {} nodes run, {} runs, {} text nodes, {} live, {} rounds, {} stops, wall {:?}",
        s.nodes_run, s.runs, s.nodes_text_frames, s.live, s.rounds, s.stops, s.wall
    );
}

/// `\input chap1 \input chap2`: TeX's DVI file, one node per paragraph
/// of the input files.
#[test]
fn inputs_sliced() {
    let t = std::time::Instant::now();
    let b = build(&fx());
    stats("cold", &b);
    eprintln!("cold: {:?}", t.elapsed());
    let want = std::fs::read(fx().join("inputs.dvi")).unwrap();
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("differs from tex: {d}\n{}", b.output().1);
    }
    // the input files' paragraphs, each a node of its own (plus the
    // document's own slices: `\bye`)
    let text = b.stats.nodes_text_frames;
    assert!(
        (PARAGRAPHS..=PARAGRAPHS + 4).contains(&text),
        "{text} text nodes for {PARAGRAPHS} paragraphs"
    );
}

/// One word of chap2 edited: only its region reruns (with the page
/// builder, the page step and the writer), and the output equals a cold
/// build of the edited file.
#[test]
fn input_edit_reruns_one_region() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("inputs-edit");
    let _ = std::fs::create_dir_all(&dir);
    for f in ["chap1.tex", "chap2.tex"] {
        std::fs::copy(fx().join(f), dir.join(f)).unwrap();
    }
    let mut b = build(&dir);
    stats("first", &b);
    let old = common::read(&dir.join("chap2.tex"));
    let new = old.replace("some italic words", "some italic verbs");
    assert_ne!(old, new);
    let t = std::time::Instant::now();
    b.edit_file("chap2.tex", Some(new.as_bytes()));
    stats("edit", &b);
    eprintln!("edit: {:?}", t.elapsed());
    let run = b.stats.nodes_run;
    std::fs::write(dir.join("chap2.tex"), &new).unwrap();
    let cold = build(&dir);
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        panic!("edit differs from cold: {d}");
    }
    // the region, `breaks`, the page step, the writer (and slack for a
    // loop-back node)
    assert!(run <= 6, "{run} nodes rerun for a one-word edit");
}

/// The same text inline (TeX's DVI file is the same): the regression
/// measure of the engine without `\input` (`{\bf ...}` opening a
/// paragraph).
#[test]
fn inline_equivalent() {
    let text = format!(
        "{}{}\\bye\n",
        common::read(&fx().join("chap1.tex")),
        common::read(&fx().join("chap2.tex"))
    );
    let want = std::fs::read(fx().join("inputs.dvi")).unwrap();
    let opts = common::options("./inputs.tex", "inputs", &common::dvi_comment(&want), &fx());
    let b = common::build(opts, Some(&common::plain()), &text);
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("inline differs from tex: {d}");
    }
}
