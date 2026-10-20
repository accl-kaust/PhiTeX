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
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
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

/// `\endinput` in a file read by a file read by the document (§362):
/// only that file ends; the file that read it and the document go on.
/// (The LaTeX format read `omlenc.def`, the first nested file ending in
/// `\endinput`, and then nothing: a 20-file format in place of 79.)
#[test]
fn endinput_ends_only_the_nested_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("endinput-nested");
    let _ = std::fs::create_dir_all(&dir);
    // (the file is read from a macro, LaTeX's `\InputIfFileExists`:
    // its body's rest lies above the reading file's line)
    std::fs::write(
        dir.join("outer.tex"),
        "\\def\\inp#1{\\begingroup\\input #1 \\endgroup}A\\inp{inner}\\relax C\\par\n",
    )
    .unwrap();
    std::fs::write(dir.join("inner.tex"), "B\n\\endinput\nNEVER\n").unwrap();
    let opts = || common::options("./endinput.tex", "endinput", "", &dir);
    let b = common::build(opts(), Some(&common::plain()), "\\input outer D\\bye\n");
    let want = common::build(opts(), Some(&common::plain()), "AB \\relax C\\par D\\bye\n");
    if let Some(d) = common::first_difference(&b.output().0, &want.output().0) {
        panic!("differs from the inline text: {d}\n{}", b.output().1);
    }
    assert!(
        !b.output().1.contains("NEVER"),
        "the text after \\endinput was read:\n{}",
        b.output().1
    );
}

/// A file read by `\input` whose bytes are not UTF-8 (koi8-r, Latin-1:
/// ruhyphen's `catkoi.tex`) is read one byte per character (§31), as the
/// main file is: the catcode assignment to byte 0xC1 sets character 193's.
/// (Each byte past 127 was read as two characters, its UTF-8 encoding's:
/// an undefined control symbol, then typeset text, and a LaTeX format
/// built with the Russian patterns lost the kernel's last lines.)
#[test]
fn input_file_bytes_past_127_are_one_character_each() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("input-8bit");
    let _ = std::fs::create_dir_all(&dir);
    let mut inner: Vec<u8> = b"\\catcode`\\".to_vec();
    inner.extend([0xC1]);
    inner.extend(b"=12 \\lccode`\\");
    inner.extend([0xC1]);
    inner.extend(b"=224 \\message{[\\the\\catcode193][\\the\\lccode193]}\n");
    std::fs::write(dir.join("eightbit.tex"), &inner).unwrap();
    let opts = || common::options("./eightbit-main.tex", "eightbit", "", &dir);
    let b = common::build(opts(), Some(&common::plain()), "\\input eightbit \\bye\n");
    let log = b.output().1;
    assert!(
        log.contains("[12][224]"),
        "the byte was not one character:\n{log}"
    );
    assert!(!log.contains("Undefined control sequence"), "{log}");
}
