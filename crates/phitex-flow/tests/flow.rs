//! The design's tests (`AGENTS_BRIEF.md`): any schedule gives the same
//! output; an edit gives what a cold build of the edited text gives; a
//! version is content only; φ is explicit; work spreads over threads.

// (a DVI file is compared whole; `assert_eq!` would print its bytes)
#![allow(clippy::manual_assert_eq)]

use std::fmt::Write as _;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options, Order, Output, Reads};

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../partex-core/testdata/cmr10.tfm"
    ))
    .unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn opts(threads: usize, reads: Reads, order: Order) -> Options {
    Options {
        threads,
        reads,
        order,
        dump: false,
        etex: false,
    }
}

/// A document of `n` paragraphs with definitions, groups, conditionals and
/// a category code change.
fn doc(n: usize) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\font\\tenrm=cmr10 \\tenrm \\tolerance=200 \\linepenalty=10\n\
         \\parindent=20pt \\baselineskip=12pt \\topskip=10pt \\maxdepth=4pt \\parfillskip=0pt plus 1fil\n\
         \\def\\lazy{lazy}\\def\\idle{idle}\n\\def\\pair#1#2{#2#1}\n\\def\\upto#1.{[#1]}\n\\hsize=300pt \\vsize=200pt\n\n",
    );
    for k in 0..n {
        if k % 10 == 7 {
            s.push_str("\\let\\lazy=\\idle\n\n");
        }
        if k % 10 == 9 {
            s.push_str("\\def\\lazy{lazy}\n\n");
        }
        let _ = write!(
            s,
            "The quick brown fox number {k} jumps over the \\lazy\\ dog; \\ifx\\lazy\\idle it rests\\else it runs\\fi. \
             {{\\def\\lazy{{local}}Inside a group the dog is \\lazy.}} \\pair ab and \\upto twelve words.\n\
             Pack my box with five dozen liquor jugs, and how vexingly quick daft zebras jump.\n\n"
        );
    }
    s
}

fn cold(src: &str, o: Options) -> Output {
    Build::new(font()).update(src, o)
}

#[test]
fn any_schedule_gives_the_same_dvi() {
    let src = doc(60);
    let base = cold(&src, opts(1, Reads::Suspend, Order::Priority));
    assert!(base.dvi.len() > 1000, "a DVI file");
    assert!(base.stats.pages >= 2, "{} pages", base.stats.pages);
    assert!(base.errors.is_empty(), "{:?}", base.errors);
    for reads in [Reads::Suspend, Reads::Speculate] {
        for threads in [1, 2, 8] {
            let o = cold(&src, opts(threads, reads, Order::Priority));
            assert!(
                o.dvi == base.dvi,
                "{threads} threads, {reads:?}: another DVI"
            );
        }
        for seed in 0..10 {
            let o = cold(&src, opts(8, reads, Order::Random(seed)));
            assert!(o.dvi == base.dvi, "seed {seed}, {reads:?}: another DVI");
        }
    }
}

#[test]
fn an_edit_is_a_cold_build_of_the_edited_text() {
    let a = doc(40);
    let b = a.replacen("number 21 jumps", "number 21 leaps", 1);
    for reads in [Reads::Suspend, Reads::Speculate] {
        let o = opts(4, reads, Order::Priority);
        let mut bld = Build::new(font());
        bld.update(&a, o);
        let e = bld.update(&b, o);
        assert!(
            e.dvi == cold(&b, o).dvi,
            "{reads:?}: the edit's DVI is not the cold build's"
        );
        assert_eq!(
            e.stats.ran, 1,
            "{reads:?}: only the edited piece runs: {:?}",
            e.stats
        );
        assert_eq!(e.stats.reruns, 0);
    }
}

#[test]
fn a_blank_line_above_a_definition_reruns_no_reader() {
    let a = doc(30);
    let b = a.replacen("\\let\\lazy=\\idle\n\n", "\\let\\lazy=\\idle\n\n\n", 1);
    assert_ne!(a, b);
    let o = opts(4, Reads::Suspend, Order::Priority);
    let mut bld = Build::new(font());
    bld.update(&a, o);
    let e = bld.update(&b, o);
    assert_eq!(e.stats.ran, 1, "the defining piece only: {:?}", e.stats);
    assert_eq!(e.stats.reruns, 0, "no reader of \\lazy runs again");
    assert!(e.dvi == cold(&b, o).dvi);
}

#[test]
fn a_changed_definition_reruns_its_readers_only() {
    let a = doc(30);
    let b = a.replacen("\\def\\idle{idle}", "\\def\\idle{sleepy}", 1);
    let o = opts(4, Reads::Speculate, Order::Priority);
    let mut bld = Build::new(font());
    let first = bld.update(&a, o);
    let e = bld.update(&b, o);
    assert!(e.dvi == cold(&b, o).dvi);
    // (every paragraph has a node reading \idle: that node runs, with what
    // depends on it; the other nodes do not)
    assert!(
        e.stats.commands_run * 10 < e.stats.commands,
        "only the reading nodes run: {:?}",
        e.stats
    );
    assert!(e.stats.commands_run < first.stats.commands_run / 10);
    assert_eq!(
        e.stats.kept, 3,
        "the three pieces reading nothing that changed: {:?}",
        e.stats
    );
}

#[test]
fn phi_is_explicit_at_fi() {
    let src = "\\catcode`\\{=1 \\catcode`\\}=2 \\def\\a{x}\\ifx\\a\\a \\def\\b{yes}\\else\\def\\b{no}\\fi\n";
    let o = cold(
        src,
        Options {
            dump: true,
            ..Options::default()
        },
    );
    assert!(o.dump.contains("φ test=true"), "{}", o.dump);
    assert!(o.dump.contains("\\b = φ("), "{}", o.dump);
}

#[test]
fn speculation_spreads_the_work() {
    let src = doc(200);
    let o = cold(&src, opts(8, Reads::Speculate, Order::Priority));
    let busy = o.stats.work.iter().filter(|&&w| w > 0).count();
    assert!(busy >= 2, "work by thread: {:?}", o.stats.work);
    assert!(o.dvi == cold(&src, Options::default()).dvi);
}

#[test]
fn reruns_from_dirty_nodes_give_the_same_dvi() {
    // (random order with speculation runs pieces before the definitions
    // they read: their nodes are marked and the pieces run again from them,
    // mid-piece)
    let src = doc(200);
    let base = cold(&src, Options::default());
    let mut reruns = 0;
    for seed in 0..6 {
        let o = cold(&src, opts(8, Reads::Speculate, Order::Random(seed)));
        reruns += o.stats.reruns;
        assert!(o.dvi == base.dvi, "seed {seed}: another DVI: {:?}", o.stats);
        assert_eq!(
            o.stats.commands, base.stats.commands,
            "seed {seed}: another graph"
        );
    }
    assert!(reruns > 0, "no piece ran again from a dirty node");
}

/// An edit of a document's text.
type Edit = Box<dyn Fn(&str) -> String>;

#[test]
fn edits_that_move_piece_boundaries_equal_cold_builds() {
    // (each edit applied to the previous text, as typing would: splitting
    // and merging pieces, a brace that changes the depth the cut sees,
    // edits at the start and at the end)
    let a = doc(30);
    let at = |s: &str, pat: &str| s.find(pat).unwrap();
    let edits: Vec<Edit> = vec![
        Box::new(|s| s.replacen("number 12 jumps", "number 12\n\njumps", 1)),
        Box::new(|s| s.replacen("number 12\n\njumps", "number 12 jumps", 1)),
        Box::new(move |s| {
            let i = at(s, "number 15 jumps");
            format!("{}{{{}", &s[..i], &s[i..])
        }),
        Box::new(|s| s.replacen("number 18 jumps", "number 18} jumps", 1)),
        Box::new(|s| format!("\\def\\idle{{dozing}}\n\n{s}")),
        Box::new(|s| format!("{s}A last word.\n")),
        Box::new(|s| s.replacen("Pack my box", "\n\n\nPack my box", 3)),
        Box::new(|s| s.replacen("\\let\\lazy=\\idle\n\n", "", 1)),
        // (lines added and removed: the page breaks after them move, and
        // the scan resumes before them and may meet the old pages again)
        Box::new(|s| {
            s.replacen("number 4 jumps", "number 4 jumps and jumps and jumps and jumps and jumps and jumps and jumps and jumps and jumps and jumps", 1)
        }),
        Box::new(|s| s.replacen("number 9 jumps", "number 9 leaps", 1)),
        Box::new(|s| {
            s.replacen(" and jumps and jumps and jumps and jumps and jumps and jumps and jumps and jumps and jumps", "", 1)
        }),
    ];
    for reads in [Reads::Suspend, Reads::Speculate] {
        let o = opts(4, reads, Order::Priority);
        let mut bld = Build::new(font());
        bld.update(&a, o);
        let mut s = a.clone();
        for (i, e) in edits.iter().enumerate() {
            s = e(&s);
            let got = bld.update(&s, o);
            let want = cold(&s, o);
            assert!(
                got.dvi == want.dvi,
                "{reads:?}: edit {i}: the edit's DVI is not the cold build's"
            );
            assert_eq!(
                got.stats.pieces, want.stats.pieces,
                "{reads:?}: edit {i}: another cut"
            );
            assert_eq!(
                got.stats.commands, want.stats.commands,
                "{reads:?}: edit {i}: another graph"
            );
        }
    }
}
