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
        "/testdata/cmr10.tfm"
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
        ..Options::default()
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

/// F5: definitions moved into new pieces by a split, with the values
/// they had: their readers' reads are the same, nothing reading them runs
/// again (the edges move to the new definers).
#[test]
fn a_new_definer_of_the_same_value_reruns_no_reader() {
    let head = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x \\vsize=200pt ";
    let a = format!("{head}\\def\\fz{{lazy}}\\hsize=100pt\n\nA \\fz\\par\\message{{\\the\\hsize}}\n\n\\end\n");
    let b = a.replacen("{lazy}", "{lazy}\n\n", 1);
    for lean in [true, false] {
        for o in [opts(1, Reads::Suspend, Order::Priority), opts(4, Reads::Speculate, Order::Random(3))] {
            let mut bld = Build::new(font());
            bld.set_lean(lean);
            bld.update(&a, o);
            let e = bld.update(&b, o);
            assert!(e.dvi == cold(&b, o).dvi && e.log == cold(&b, o).log, "lean {lean} {o:?}");
            // (the two new pieces run; the readers, marked while the
            // definitions were between pieces, keep their runs — in order
            // at one thread: at four, speculating, a reader may start
            // before the new definers have run, and is run again)
            if o.threads > 1 {
                continue;
            }
            assert_eq!(e.stats.ran, 2, "no reader of \\fz or \\hsize runs again (lean {lean}, {o:?}): {:?}", e.stats);
        }
    }
}

/// F7 (B's): an output routine fired between a piece's nodes defines
/// names there (`\count0`); the piece's nodes before the fire that read
/// them do not run again, only those after it (and the piece keeps its
/// nodes until the page builder has passed it, lean or not).
#[test]
fn a_fire_between_a_pieces_nodes_reruns_only_the_nodes_after_it() {
    let mut a = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x \\hsize=100pt \\vsize=40pt \\parfillskip=0pt plus 1fil\n\
         \\output={\\global\\advance\\count0 by1 \\shipout\\box255}\n\
         \\message{[\\the\\count0]}",
    );
    for i in 0..60 {
        let _ = write!(a, "\nLine {i}.\\par\\message{{[\\the\\count0]}}");
    }
    a.push_str("\n\\end\n");
    for lean in [true, false] {
        let o = opts(1, Reads::Suspend, Order::Priority);
        let mut bld = Build::new(font());
        bld.set_lean(lean);
        let e = bld.update(&a, o);
        assert!(e.stats.pages > 5, "pages: {:?}", e.stats);
        assert!(
            // (every line reads \\count0: each fire runs the readers after
            // it again, about half of them; it ran the whole piece again,
            // 13 times its commands)
            e.stats.commands_run < e.stats.commands * 2,
            "each fire runs only the nodes after it again (lean {lean}): {:?}",
            e.stats
        );
    }
}

/// Item provenance: each glyph of a line, and of a box in it, leads to
/// the node that made it and that node's source line (`SyncTeX`'s tags).
#[test]
fn a_glyph_leads_to_its_source_line() {
    use phitex_flow::item::Item;
    let src = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x \\hsize=300pt \\vsize=200pt \\parfillskip=0pt plus 1fil\nAlpha\nBeta \\hbox{Gamma}\nDelta\\par\n\\end\n";
    for lean in [true, false] {
        let mut b = Build::new(font());
        b.set_lean(lean);
        let out = b.update(src, opts(1, Reads::Suspend, Order::Priority));
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let mat = b.material_of(0).expect("piece 0's material");
        let (at, line) = mat
            .iter()
            .enumerate()
            .find_map(|(i, it)| match it {
                Item::Append(l, _) => Some((i, l.clone())),
                _ => None,
            })
            .expect("a line");
        let mut seen = Vec::new();
        for (j, it) in line.list.iter().enumerate() {
            match it {
                Item::Char(_, c) if c.is_ascii_uppercase() => {
                    let n = b.material_node(0, u32::try_from(at).unwrap(), &[j]).expect("a node");
                    seen.push((char::from(*c), b.source_place(0, n).expect("a place").1));
                }
                // (the paragraph's indent is an empty box)
                Item::Box(bx) if !bx.list.is_empty() => {
                    let n = b.material_node(0, u32::try_from(at).unwrap(), &[j, 0]).expect("a node in the box");
                    let c = match &bx.list[0] {
                        Item::Char(_, c) => char::from(*c),
                        _ => '?',
                    };
                    seen.push((c, b.source_place(0, n).expect("a place").1));
                }
                _ => {}
            }
        }
        assert_eq!(seen, [('A', 2), ('B', 3), ('G', 3), ('D', 4)], "lean {lean}");
    }
}

#[test]
fn a_changed_definition_reruns_its_readers_only() {
    let a = doc(30);
    let b = a.replacen("\\def\\idle{idle}", "\\def\\idle{sleepy}", 1);
    let o = opts(4, Reads::Speculate, Order::Priority);
    // (pieces keeping their nodes: a lean piece runs every node again)
    let mut bld = Build::new(font());
    bld.set_lean(false);
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

/// Lean pieces (the default): the first edit of a definition runs its
/// readers' pieces again from their text; those pieces keep their nodes
/// for the next edits, which run only the reading nodes again.
#[test]
fn edited_pieces_keep_their_nodes_a_while() {
    let a = doc(30);
    let b = a.replacen("\\def\\idle{idle}", "\\def\\idle{sleepy}", 1);
    let o = opts(4, Reads::Speculate, Order::Priority);
    let mut bld = Build::new(font());
    bld.update(&a, o);
    let e1 = bld.update(&b, o);
    assert!(e1.dvi == cold(&b, o).dvi);
    let e2 = bld.update(&a, o);
    assert!(e2.dvi == cold(&a, o).dvi);
    assert!(
        e2.stats.commands_run * 10 < e2.stats.commands,
        "the second edit runs only the reading nodes: {:?} (the first ran {})",
        e2.stats,
        e1.stats.commands_run
    );
}

#[test]
fn fi_defines_nothing() {
    let src = "\\catcode`\\{=1 \\catcode`\\}=2 \\def\\a{x}\\ifx\\a\\a \\def\\b{yes}\\else\\def\\b{no}\\fi\n";
    let o = cold(
        src,
        Options {
            dump: true,
            ..Options::default()
        },
    );
    // (only the taken arm is read, and its definitions are the only ones:
    // the test is a node, `\fi` none, no φ)
    assert!(o.dump.contains("test=true"), "{}", o.dump);
    assert!(!o.dump.contains("φ("), "{}", o.dump);
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

/// A blank line inserted inside an open group, above a `\begingroup`, a
/// `\showgroups` and a `\showifs`: the nodes after it keep their reads and
/// move by a node — where their open groups and conditionals began is a
/// position, not content (`AGENTS_BRIEF.md`: no ids in a version) — so they
/// are kept, not run again; only the nodes reading the changed text run.
/// The log, with the lines where the groups began, is the cold log.
#[test]
fn an_edit_inside_a_group_keeps_the_nodes_after_it() {
    let a = "\\catcode`\\{=1 \\catcode`\\}=2\n{\\count1=1\n\\count3=3\n\\begingroup\\count2=2 \\showgroups\n\
             \\iftrue\\count4=4 \\showifs\\count5=5 \\fi\\endgroup\\count6=6 }\n";
    let b = a.replacen("\\count3=3\n", "\\count3=3\n\n", 1);
    let o = Options {
        etex: true,
        ..opts(1, Reads::Suspend, Order::Priority)
    };
    let mut bld = Build::new(font());
    bld.set_lean(false);
    let first = bld.update(a, o);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let e = bld.update(&b, o);
    let cold = Build::new(font()).update(&b, o);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(e.log, cold.log, "the edit's log is not the cold log");
    assert!(
        e.log.iter().any(|l| l.contains("entered at line 5")),
        "{:?}",
        e.log
    );
    eprintln!(
        "nodes: {} run of {} (cold: {})",
        e.stats.commands_run, e.stats.commands, cold.stats.commands_run
    );
    // (the `\count3` whose text the edit touched, the blank line's new
    // `\par`, and the end, whose material since the last paragraph now
    // holds that `\par`: the `\begingroup`, `\iftrue` and `\fi` after it,
    // moved by a node, are kept)
    assert!(
        e.stats.commands_run <= 3,
        "{} of {} nodes ran",
        e.stats.commands_run,
        e.stats.commands
    );
}

/// A document whose pieces each scan `scanned` as an argument they drop
/// (`\c{...}`), and one piece scanning it and using `\a`.
fn scanning_doc(def: &str, scanned: &str) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\font\\tenrm=cmr10 \\tenrm\n\
         \\hsize=300pt \\vsize=200pt \\parfillskip=0pt plus 1fil \\baselineskip=12pt\n",
    );
    s.push_str(def);
    s.push_str("\\def\\c#1{}\n\n");
    for k in 0..12 {
        let _ = write!(s, "Paragraph {k} scans \\c{{{scanned}}}an argument it drops.\n\n");
    }
    // (a piece that both scans the name and uses it: its scanning node is
    // kept while the user runs again)
    let _ = write!(s, "The last one scans \\c{{{scanned}}}and then uses \\a.\n\n");
    s
}

/// A name scanned (its outer-ness read) and then read whole by each of
/// many nodes: the run holds one `Read` per distinct read, not a copy per
/// node (the copies held 18.5M `Read`s of latex.ltx's expl3 piece where
/// 1M are distinct, 3 GB).
#[test]
fn a_scanned_and_expanded_name_is_one_read() {
    let mut s = String::from("\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\def\\x{}\\def\\y#1{#1}");
    for _ in 0..400 {
        s.push_str("\\y{\\x\\x}\\relax");
    }
    s.push_str("\n\\end\n");
    let mut b = Build::new(font());
    b.set_lean(false);
    let out = b.update(&s, opts(1, Reads::Suspend, Order::Priority));
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let (reads, distinct) = b.reads_held(0).expect("piece 0 holds its nodes");
    assert!(reads >= 800, "{reads} reads");
    assert!(distinct <= 60, "{distinct} distinct reads of {reads}: a copy per node");
}

#[test]
fn a_scanned_name_reads_only_whether_it_is_outer() {
    let opt = opts(4, Reads::Suspend, Order::Priority);
    // (lean pieces, as builds keep them, and pieces keeping their nodes)
    for lean in [true, false] {
        // (the nodes run when `\a` is redefined: the same whether the
        // pieces scan `\a` or a name never defined)
        let edit = |scanned: &str, to: &str| {
            let before = scanning_doc("\\def\\a{x}", scanned);
            let after = scanning_doc(to, scanned);
            let mut bld = Build::new(font());
            bld.set_lean(lean);
            let first = bld.update(&before, opt);
            assert!(first.errors.is_empty(), "{:?}", first.errors);
            let e = bld.update(&after, opt);
            let c = cold(&after, opt);
            assert!(e.dvi == c.dvi, "the edit's DVI is not the cold build's");
            assert_eq!(e.errors, c.errors, "the edit's errors are not the cold build's");
            e
        };
        let scanning = edit("\\a", "\\def\\a{yy}");
        let not = edit("\\z", "\\def\\a{yy}");
        assert!(scanning.errors.is_empty(), "{:?}", scanning.errors);
        assert!(scanning.stats.commands_run > 0, "the user of \\a runs again");
        assert_eq!(
            (scanning.stats.commands_run, scanning.stats.reruns),
            (not.stats.commands_run, not.stats.reruns),
            "lean {lean}: a node that only scanned \\a ran again: {:?} against {:?}",
            scanning.stats,
            not.stats
        );
        // (made `\outer`, the scanning nodes see it: TeX's error, each piece)
        let outer = edit("\\a", "\\outer\\def\\a{x}");
        let forbidden = outer.errors.iter().filter(|e| e.contains("Forbidden control sequence")).count();
        assert_eq!(forbidden, 13, "lean {lean}: {:?}", outer.errors);
    }
}

/// Edges go into the readers index before the scheduler's lock
/// (`run_and_install`): a piece reading `\x` installs its edge while a slow
/// piece before it installs a new definition of `\x`. Every reader must
/// still be marked or see the new value: any schedule, either policy,
/// gives the one-thread DVI.
#[test]
fn edges_outside_the_lock_lose_no_mark() {
    let mut src = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\font\\tenrm=cmr10 \\tenrm \\tolerance=200 \\linepenalty=10\n\
         \\parindent=20pt \\baselineskip=12pt \\topskip=10pt \\maxdepth=4pt \\parfillskip=0pt plus 1fil\n\
         \\hsize=300pt \\vsize=200pt \\def\\x{A}\\def\\y{C}\n\n\
         \\count1=0 \\def\\l{\\advance\\count1 1 \\ifnum\\count1<4000 \\expandafter\\l\\fi}\\l \\gdef\\x{B}\n\n",
    );
    for k in 0..300 {
        if k % 50 == 25 {
            // (a second slow definer, of another name, in the middle)
            src.push_str("\\count2=0 \\def\\m{\\advance\\count2 1 \\ifnum\\count2<2000 \\expandafter\\m\\fi}\\m \\gdef\\y{D}\n\n");
        }
        let _ = write!(src, "Word \\x\\ and \\y\\ number {k}.\n\n");
    }
    let base = cold(&src, opts(1, Reads::Suspend, Order::Priority));
    assert!(base.errors.is_empty(), "{:?}", base.errors);
    assert!(base.dvi.len() > 1000, "a DVI file");
    let mut reruns = 0;
    for reads in [Reads::Suspend, Reads::Speculate] {
        for threads in [2, 8, 16] {
            for order in std::iter::once(Order::Priority).chain((0..12).map(Order::Random)) {
                let o = cold(&src, opts(threads, reads, order));
                reruns += o.stats.reruns;
                assert!(o.dvi == base.dvi, "{threads} threads, {reads:?}, {order:?}: another DVI: {:?}", o.stats);
                assert!(o.errors == base.errors, "{threads} threads, {reads:?}, {order:?}: {:?}", o.errors);
            }
        }
    }
    assert!(reruns > 0, "no reader ran again: the race was never there");
}
