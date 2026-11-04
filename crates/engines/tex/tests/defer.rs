//! Deferred reads of page-layer registers (docs/spec-deferred-reads.md):
//! a paragraph that puts the page number into a deferred `\write`, a
//! macro, a `\mark`, or back into the register makes no read of it; one
//! that tests or typesets it does. Plain TeX, where `\count0` is the
//! page number and the oracle is a cold build of the same text.

mod common;

use std::fmt::Write as _;

use phitex_tex::Options;

fn runs_of_report(r: &phitex_tex::Report, op: &str) -> u64 {
    r.runs_by_op
        .iter()
        .find(|(o, _)| *o == op)
        .map_or(0, |(_, n)| *n)
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
        let _ = write!(
            s,
            "Paragraph {i} of the document, {sentence} Again, {sentence} Once more, {sentence}\n\n"
        );
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
    let out = b
        .writes()
        .into_iter()
        .filter(|(k, _)| *k == 1)
        .map(|(_, t)| t)
        .collect::<String>();
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
        assert!(
            want_out.contains("page=") && !want_out.contains("page=1."),
            "-j{threads}: the write names its page: {want_out:?}"
        );
        assert_eq!(
            b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
            0,
            "-j{threads}: no paragraph read the page number: {:?}",
            b.stats.refuted_by_field
        );
        // a paragraph inserted at the front: every page break moves
        let edited = doc(121, 60, mid).replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines, so the marked paragraph moves.\n\nParagraph 0 of", 1);
        common::rebuild(&mut b, &edited);
        let (want_dvi, want_out) = cold("write-edited", &edited, 1);
        assert_eq!(
            b.output().0,
            want_dvi,
            "-j{threads}: the edited document equals its cold build"
        );
        let got_out = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect::<String>();
        assert_eq!(
            got_out, want_out,
            "-j{threads}: the written page number followed the page"
        );
        assert_eq!(
            b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
            0,
            "-j{threads}: still no read of the page number by a paragraph"
        );
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
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "{:?}",
        b.stats.refuted_by_field
    );
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("macro-edited", &edited, 1).0);
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "after the edit: {r:?} {:?}",
        b.stats.refuted_by_field
    );
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
    assert_eq!(
        b.output().0,
        cold("cond-edited", &edited, 1).0,
        "the odd/even word follows the page"
    );
    assert!(
        runs_of_report(&r, "exec") + runs_of_report(&r, "resume") >= 2,
        "the conditional's paragraph ran again when its page moved: {r:?}"
    );
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
    assert_eq!(
        b.output().0,
        cold("restore", &src, 1).0,
        "page numbers after the probe are TeX's"
    );
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "{:?}",
        b.stats.refuted_by_field
    );
    assert_eq!(
        b.definitions_by_op("exec", "count:0") + b.definitions_by_op("resume", "count:0"),
        0,
        "no paragraph defined the page number"
    );
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("restore-edited", &edited, 1).0);
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "after the edit: {r:?} {:?}",
        b.stats.refuted_by_field
    );
}

/// The page number copied into another register (`\global\count9=\the
/// \count0`) is an alias definition (docs/spec-state-model.md §2.2): the
/// paragraph makes no read of it, and a `\write` of the copy expanded at
/// shipout reads the page number as of the paragraph through it.
#[test]
fn a_page_number_copied_into_another_register_is_an_alias() {
    let mid = "\\global\\count9=\\the\\count0\\relax\\write1{copy=\\the\\count9.}Paragraph that copies its page number into another register and writes the copy at shipout.";
    let src = doc(120, 60, mid);
    let dir = common::scratch("defer-alias");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    let (want_dvi, want_out) = cold("alias", &src, 1);
    assert_eq!(b.output().0, want_dvi);
    let got = |b: &phitex_tex::Build| {
        b.writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect::<String>()
    };
    assert!(
        want_out.contains("copy=") && !want_out.contains("copy=1."),
        "{want_out:?}"
    );
    assert_eq!(got(&b), want_out);
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "{:?}",
        b.stats.refuted_by_field
    );
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    let (want_dvi, want_out) = cold("alias-edited", &edited, 1);
    assert_eq!(b.output().0, want_dvi);
    assert_eq!(got(&b), want_out, "the copy followed the page");
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "after the edit: {r:?} {:?}",
        b.stats.refuted_by_field
    );
}

/// A `\mark` holding the page number is resolved by the finisher that
/// shows it in the headline: the paragraph makes no read.
#[test]
fn a_mark_with_the_page_number_is_resolved_by_the_finisher() {
    let mid = "\\mark{\\number\\count0}Paragraph that marks its page number for the headline.";
    let src = format!(
        "\\headline={{\\hfil\\topmark\\hfil}}\n{}",
        doc(120, 60, mid)
    );
    let dir = common::scratch("defer-mark");
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src);
    assert_eq!(b.output().0, cold("mark", &src, 1).0);
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "{:?}",
        b.stats.refuted_by_field
    );
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(
        b.output().0,
        cold("mark-edited", &edited, 1).0,
        "the headline follows the page"
    );
    assert_eq!(
        b.stats.refuted_by_op("exec", "count:0") + b.stats.refuted_by_op("resume", "count:0"),
        0,
        "after the edit: {r:?} {:?}",
        b.stats.refuted_by_field
    );
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
        assert_eq!(
            dvi,
            want,
            "-j{threads}: DVI differs; log tail:\n{}",
            common::log_excerpt(&log)
        );
    }
}

/// A page-layer register past the static ones, saved through a macro by
/// one paragraph and set back from it by another that met other such
/// registers first (beamer's `\saved@c@figure` for its overlays): the
/// symbol in the macro names the register, not the location the first
/// run had for it, which in the second run is another register's. Named
/// by location, after an edit the second run's `\count300` became
/// `\count301` as of the first (beamer: "Figure 72819:", the second
/// run's location being a dimension there). The second run's registers
/// are met in one macro, so a cut does not put them in a run of their own.
#[test]
fn a_symbol_read_by_another_run_is_of_the_same_register() {
    let mark = |n: usize| format!("Paragraph {n} of");
    let text = |n: usize, k: usize| {
        let b = "\\restore Restored: \\the\\count300.\n\n";
        let d = doc(n, k, "\\xdef\\saved{\\the\\count300}Saved paragraph.");
        let d = d.replacen(&mark(80), &format!("{b}{}", mark(80)), 1);
        format!(
            "\\def\\restore{{\\count301=7 \\count302=8 \\global\\count300=\\saved\\relax}}\n\\output={{\\count255=\\count300 \\advance\\count255 by 1 \\global\\count300=\\count255 \\shipout\\box255 \\global\\advance\\count0 by 1 }}\n{d}"
        )
    };
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-reg-{threads}"));
        let src = text(120, 30);
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let mut b = common::build(opts, Some(&common::plain()), &src);
        let edited = src.replacen(
            &mark(0),
            &format!("A new page.\\vfill\\eject\n\n{}", mark(0)),
            1,
        );
        common::rebuild(&mut b, &edited);
        assert_eq!(
            b.output().0,
            cold("reg-edited", &edited, 1).0,
            "-j{threads}: the edited document equals its cold build"
        );
    }
}

/// A counter that only counts (docs/note-relative-counters.md §2.1): each
/// paragraph steps it and carries its value, plus the steps it made, into
/// a macro and a deferred `\write` without reading it; a step added before
/// them moves the values in the file and in the last paragraph, which
/// typesets one, and asks no paragraph that only carries it again.
#[test]
fn a_counter_only_stepped_and_carried_is_no_read() {
    let src = |extra: &str| {
        let mut s = String::from("\\openout1=\\jobname.out\n\\count200=5\n");
        s.push_str(extra);
        for i in 0..80 {
            let _ = write!(
                s,
                "\\global\\advance\\count200 1 \\advance\\count200 2 \
                 \\edef\\w{{\\write1{{n{i}=\\the\\count200.}}}}\\w\\edef\\last{{\\number\\count200}}\
                 Paragraph {i} of the document, long enough to make a line or two when set in the \
                 plain format's ten point roman.\n\n"
            );
        }
        s.push_str("The last value is \\last.\n\n\\closeout1\n\\bye\n");
        s
    };
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-rel-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let first = src("");
        let mut b = common::build(opts, Some(&common::plain()), &first);
        let (want, out) = cold("rel", &first, 1);
        assert_eq!(b.output().0, want, "-j{threads}: cold");
        assert!(out.contains("n0=8.") && out.contains("n79=245."), "{out:?}");
        let edited = src("\\advance\\count200 1000\n");
        common::rebuild(&mut b, &edited);
        let (want, out) = cold("rel-edited", &edited, 1);
        assert_eq!(b.output().0, want, "-j{threads}: edited equals cold");
        assert!(out.contains("n0=1008."), "{out:?}");
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, out, "-j{threads}: the file follows");
        assert!(
            b.stats.refuted_by_op("exec", "count:200")
                + b.stats.refuted_by_op("resume", "count:200")
                <= 1,
            "-j{threads}: only the paragraph typesetting it reads it: {:?}",
            b.stats.refuted_by_field
        );
    }
}

/// Steps of a counter after candidates of a run that cannot be cut (a
/// group open over paragraphs): published as a relation, each page's routine,
/// which reads the counter at its candidate, reads what the steps before it
/// make. Against the same text setting the counter by value at each step.
#[test]
fn steps_after_candidates_reach_the_routine_as_values_do() {
    let src = |step: &str, front: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\count200=5\n\
             \\output={\\immediate\\write1{p\\the\\count0=\\the\\count200.}\\shipout\\box255\\global\\advance\\count0 1 }\n",
        );
        s.push_str(front);
        for b in 0..6 {
            s.push_str("\\begingroup\n");
            for i in 0..12 {
                let _ = write!(
                    s,
                    "Paragraph {b} {i}, long enough to make a line or two when set in the plain \
                     format's ten point roman, and one more clause to be sure. {step} Then more text \
                     after the step, which runs on to another line of the paragraph.\n\n"
                );
            }
            s.push_str("\\endgroup\n\n");
        }
        s.push_str("\\closeout1\n\\bye\n");
        s
    };
    let map = "\\global\\advance\\count200 1 ";
    let val = "\\global\\count200=\\numexpr\\count200+1\\relax ";
    let (want_dvi, want_out) = cold("steps-val", &src(val, ""), 1);
    assert!(want_out.contains("p2="), "several pages: {want_out:?}");
    let (want_dvi2, want_out2) = cold(
        "steps-val2",
        &src(val, "\\global\\advance\\count200 100\n"),
        1,
    );
    for threads in [1, 4, 8] {
        let dir = common::scratch(&format!("defer-steps-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let mut b = common::build(opts, Some(&common::plain()), &src(map, ""));
        assert_eq!(b.output().0, want_dvi, "-j{threads}: cold");
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, want_out, "-j{threads}: each page's value");
        common::rebuild(&mut b, &src(map, "\\global\\advance\\count200 100\n"));
        assert_eq!(b.output().0, want_dvi2, "-j{threads}: edited");
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(
            got, want_out2,
            "-j{threads}: each page's value after the edit"
        );
    }
}

/// A symbol among a macro's delimiters, as etoolbox's `\\ifinlist` makes them
/// (biblatex's `\\xifinlist{\\the\\c@refsection}{...}`): TeX has the digits
/// there, and text with those digits, from any read, matches.
#[test]
fn a_counter_among_a_macros_delimiters_matches_its_digits() {
    let src = "\\openout1=\\jobname.out\n\\count200=7\n\\edef\\n{\\the\\count200}\n\
               \\edef\\next{\\def\\noexpand\\find##1,\\n,##2&{\\noexpand\\write1{[##1]}}}\\next\n\
               \\find a,7,b&\n\\edef\\m{x,\\the\\count200,y&}\\expandafter\\find\\m\n\
               Text.\\par\n\\closeout1\n\\bye\n";
    let (_, out) = cold("delim", src, 1);
    assert_eq!(out.trim_end(), "[a][x]", "{out:?}");
}

/// `\xdef\list{\list item,}` is a relation: an item added earlier gives every
/// later adder another value without running it, and what the list holds is
/// what TeX's copy would be, in every shape of its use: the macro read back in
/// the same paragraph, a body that expands it again, a replacement after it, a
/// local one in a group, and a list that stops being plain text (its `\foo`
/// expanded by each `\xdef`, so each adder is run again).
#[test]
fn a_list_added_to_is_a_relation() {
    let sentence = "long enough to make a few lines when set in the plain format's ten point roman \
                    type, so that the paragraphs do fill a page or two of the document in the end.";
    let src = |first: &str, mid: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\def\\list{}\\def\\foo{x}\n\\def\\add#1{\\xdef\\list{\\list #1}}\n",
        );
        s.push_str(first);
        s.push_str("\n\n");
        for i in 0..12 {
            let _ = write!(s, "\\add{{a{i},}}Paragraph {i}, {sentence}\n\n");
            if i == 5 {
                s.push_str(mid);
                s.push_str("\n\n");
            }
        }
        s.push_str("\\write1{[\\list]}\nEnd.\\par\n\\closeout1\n\\bye\n");
        s
    };
    let items = |r: std::ops::Range<usize>| {
        let mut s = String::new();
        for i in r {
            let _ = write!(s, "a{i},");
        }
        s
    };
    for (name, first, mid, want) in [
        ("plain", "Start.", "Middle.", format!("[{}]", items(0..12))),
        (
            "added first",
            "\\add{z,}Start.",
            "Middle.",
            format!("[z,{}]", items(0..12)),
        ),
        (
            "read back",
            "Start.",
            "\\add{m,}\\edef\\copy{\\list}\\xdef\\list{\\copy n,}Middle.",
            format!("[{}m,n,{}]", items(0..6), items(6..12)),
        ),
        (
            "twice over",
            "Start.",
            "\\add{\\list}Middle.",
            format!("[{}{}{}]", items(0..6), items(0..6), items(6..12)),
        ),
        (
            "replaced",
            "Start.",
            "\\add{m,}\\gdef\\list{r,}Middle.",
            format!("[r,{}]", items(6..12)),
        ),
        (
            "local",
            "Start.",
            "{\\add{m,}\\def\\list{l,}\\add{k,}}Middle.",
            format!("[l,k,{}]", items(6..12)),
        ),
        (
            "not plain",
            "\\gdef\\list{\\foo,}Start.",
            "Middle.",
            format!("[x,{}]", items(0..12)),
        ),
        (
            "not plain later",
            "Start.",
            "\\gdef\\list{\\foo,}Middle.",
            format!("[x,{}]", items(6..12)),
        ),
    ] {
        let (dvi, out) = cold("append", &src(first, mid), 1);
        assert_eq!(out.trim_end(), want, "{name}");
        for threads in [1, 4] {
            let dir = common::scratch(&format!("defer-append-{threads}"));
            let opts = Options {
                threads: Some(threads),
                ..common::options("./d.tex", "d", "", &dir)
            };
            let mut b = common::build(opts, Some(&common::plain()), &src("Start.", "Middle."));
            common::rebuild(&mut b, &src(first, mid));
            let got: String = b
                .writes()
                .into_iter()
                .filter(|(k, _)| *k == 1)
                .map(|(_, t)| t)
                .collect();
            assert_eq!(got.trim_end(), want, "{name} -j{threads}: the edit");
            assert_eq!(b.output().0, dvi, "{name} -j{threads}: the edit's output");
            common::rebuild(&mut b, &src("Start.", "Middle."));
        }
    }
    // (the later adders stand: an item added first runs the piece it is in,
    // and what reads the whole list, not one adder)
    let dir = common::scratch("defer-append-runs");
    let opts = Options {
        threads: Some(1),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let mut b = common::build(opts, Some(&common::plain()), &src("Start.", "Middle."));
    let r = common::rebuild(&mut b, &src("\\add{z,}Start.", "Middle."));
    let execs = runs_of_report(&r, "exec") + runs_of_report(&r, "resume");
    assert!(execs <= 4, "runs after an item added first: {execs}");
}

/// A membership test of a comma list as macros write it, whatever their names:
/// the same answers as the match it stands for, a list grown by another item
/// makes no read of its text, and an item that moves earlier changes the
/// answers it should.
#[test]
fn a_membership_test_of_a_list_reads_the_items_membership() {
    let sentence = "long enough to make a few lines when set in the plain format's ten point roman \
                    type, so that the paragraphs do fill a page or two of the document in the end.";
    let src = |first: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\def\\list{}\\def\\res{}\n\
             \\def\\test#1#2{\\def\\match##1,#1,##2##3\\nil{\\ifx##2\\empty \
             \\xdef\\res{\\res N}\\else\\ifx\\relax\\relax\\xdef\\res{\\res Y}\\fi\\fi}%\n\
             \\expandafter\\match\\expandafter,#2,#1,\\empty\\nil}\n\
             \\def\\add#1{\\xdef\\list{\\list #1,}}\n",
        );
        s.push_str(first);
        s.push_str("\n\n");
        for i in 0..8 {
            let _ = write!(
                s,
                "\\test{{f{i}}}\\list\\add{{f{i}}}\\test{{f{i}}}\\list Paragraph {i}, {sentence}\n\n"
            );
        }
        s.push_str("\\write1{[\\list|\\res]}\nEnd.\\par\n\\closeout1\n\\bye\n");
        s
    };
    let mut list = String::new();
    for i in 0..8 {
        let _ = write!(list, "f{i},");
    }
    for (first, want) in [
        (
            // (an item's next token is what the match compares: `\\empty`
            // after `f0`, as TeX's `\\ifx##2\\empty` has it; then the `\\xdef`
            // that adds to the list expands it away)
            "\\font\\fa=cmr10 \\gdef\\list{f0,\\empty,\\fa,}\\test{\\fa}\\list Start.",
            format!("[f0,,\\fa ,{list}|YNY{}]", "NY".repeat(7)),
        ),
        ("Start.", format!("[{list}|{}]", "NY".repeat(8))),
        (
            "\\add{f3}Start.",
            format!("[f3,{list}|{}YY{}]", "NY".repeat(3), "NY".repeat(4)),
        ),
    ] {
        let (dvi, out) = cold("clist", &src(first), 1);
        assert_eq!(out.trim_end(), want, "{first}");
        for threads in [1, 4] {
            let dir = common::scratch(&format!("defer-clist-{threads}"));
            let opts = Options {
                threads: Some(threads),
                ..common::options("./d.tex", "d", "", &dir)
            };
            let mut b = common::build(opts, Some(&common::plain()), &src("Start."));
            common::rebuild(&mut b, &src(first));
            let got: String = b
                .writes()
                .into_iter()
                .filter(|(k, _)| *k == 1)
                .map(|(_, t)| t)
                .collect();
            assert_eq!(got.trim_end(), want, "{first} -j{threads}");
            assert_eq!(b.output().0, dvi, "{first} -j{threads}: the output");
        }
    }
}
