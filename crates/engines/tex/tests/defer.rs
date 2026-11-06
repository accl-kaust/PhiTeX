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

/// A counter that only counts (docs/spec-relative-counters.md §2.1): each
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

/// A counter saved after a piece stepped it and set back to that saved
/// value later in the piece (beamer's `\beamer@savecounter` and
/// `\beamer@restorecounter` at every overlay of a frame: `\xdef\saved
/// {\the\c@figure}`, then `\global\c@figure=\saved`) is the value
/// reaching the piece plus its steps, a step again: no paragraph reads the
/// counter, and a step added before them all asks none of them again.
/// Without it each paragraph's set defined the counter by value, which the
/// next one read: a chain through the document (beamer's delete-section
/// edit, 14 frames one after another).
#[test]
fn a_counter_set_back_to_its_value_after_steps_is_a_step() {
    let src = |extra: &str| {
        let mut s = String::from("\\openout1=\\jobname.out\n\\count200=5\n");
        s.push_str(extra);
        for i in 0..80 {
            let _ = write!(
                s,
                "\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}\\global\\advance\\count200 7 \\global\\count200=\\saved\\relax \
                 \\edef\\w{{\\write1{{n{i}=\\the\\count200.}}}}\\w \
                 Paragraph {i} of the document, long enough to make a line or two when set in the \
                 plain format's ten point roman.\n\n"
            );
        }
        s.push_str("\\closeout1\n\\bye\n");
        s
    };
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-restore-plus-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let first = src("");
        let mut b = common::build(opts, Some(&common::plain()), &first);
        let (want, out) = cold("restore-plus", &first, 1);
        assert_eq!(b.output().0, want, "-j{threads}: cold");
        assert!(out.contains("n0=6.") && out.contains("n79=85."), "{out:?}");
        let edited = src("\\global\\advance\\count200 1\n");
        common::rebuild(&mut b, &edited);
        let (want, out) = cold("restore-plus-edited", &edited, 1);
        assert_eq!(b.output().0, want, "-j{threads}: edited equals cold");
        assert!(out.contains("n0=7.") && out.contains("n79=86."), "{out:?}");
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, out, "-j{threads}: the file follows");
        assert_eq!(
            b.stats.refuted_by_op("exec", "count:200")
                + b.stats.refuted_by_op("resume", "count:200"),
            0,
            "-j{threads}: no paragraph reads the counter: {:?}",
            b.stats.refuted_by_field
        );
    }
}

/// A piece's `\immediate\write` of a counter it only steps (biblatex's
/// citation order in the `.bcf`, `\blx@bibreq`) neither forces nor reads
/// it: the line keeps the symbol, resolved where the file's text is read
/// (the files node, the file read back at the next pass, the build's
/// written files), docs/spec-relative-counters.md §2.7. A step added
/// before them leaves the paragraphs standing, and the file, the text
/// read back from it and the PDF are those of a cold build. A line whose
/// read-back defines (`\def\v<i>{<n>}`) keeps its digits forced.
#[test]
fn an_immediate_write_of_a_stepped_counter_is_resolved_where_the_file_is_read() {
    let src = |extra: &str| {
        let mut s = String::from(
            "\\count200=5\n\\openin2=d.out \\ifeof2 \\closein2 \\else\\closein2 Read back: \\input d.out \\par\\fi\n\
             \\immediate\\openout1=d.out\n",
        );
        s.push_str(extra);
        for i in 0..40 {
            let def = if i % 10 == 3 {
                format!("\\immediate\\write1{{\\string\\def\\string\\v{i}{{\\the\\count200}}}}")
            } else {
                String::new()
            };
            let _ = write!(
                s,
                "\\global\\advance\\count200 1 \\immediate\\write1{{n{i}=\\the\\count200.}}{def} \
                 Paragraph {i} of the document, long enough to make a line or two when set in the \
                 plain format's ten point roman.\n\n"
            );
        }
        s.push_str("\\immediate\\closeout1\n\\bye\n");
        s
    };
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-immediate-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let first = src("");
        let mut b = common::build(opts, Some(&common::plain()), &first);
        let (want, out) = cold("immediate", &first, 1);
        assert_eq!(b.output().0, want, "-j{threads}: cold");
        assert!(
            out.contains("n0=6.") && out.contains("n39=45.") && out.contains("\\def\\v13{19}"),
            "{out:?}"
        );
        let edited = src("\\global\\advance\\count200 1\n");
        common::rebuild(&mut b, &edited);
        let (want, out) = cold("immediate-edited", &edited, 1);
        assert_eq!(b.output().0, want, "-j{threads}: edited equals cold");
        assert!(
            out.contains("n0=7.") && out.contains("n39=46.") && out.contains("\\def\\v13{20}"),
            "{out:?}"
        );
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, out, "-j{threads}: the stream's lines");
        let file = b
            .files()
            .into_iter()
            .find(|(n, _)| n == "d.out")
            .map(|(_, t)| t.replace('\n', ""));
        assert_eq!(
            file.as_deref(),
            Some(out.as_str()),
            "-j{threads}: the written file"
        );
        let forced = b.stats.refuted_by_op("exec", "count:200")
            + b.stats.refuted_by_op("resume", "count:200");
        assert!(
            forced <= 4,
            "-j{threads}: only the four paragraphs whose line defines read the counter: {:?}",
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

/// Steps on both sides of candidates of a run that cannot be cut, a page's
/// routine reading the counter at each: the run does not read the counter
/// for them. Its value at a candidate is what reached the run plus the
/// steps before the candidate, which the page's finisher resolves, so a step
/// added before them wakes the finishers, not the text (LaTeX's
/// `\\g__mark_int` in amsldoc, every piece after an edit re-run in turn).
/// Against the same text setting the counter by value.
#[test]
fn steps_on_both_sides_of_candidates_are_no_read() {
    let src = |step: &str, front: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\count200=5\n\\
             \\output={\\immediate\\write1{p\\the\\count0=\\the\\count200.}\\shipout\\box255\\global\\advance\\count0 1 }\n",
        );
        s.push_str(front);
        for b in 0..4 {
            s.push_str("\\begingroup\n");
            for i in 0..16 {
                let _ = write!(
                    s,
                    "{step}Paragraph {b} {i}, long enough to make a line or two when set in the \
                     plain format's ten point roman, and one more clause to be sure. {step} Then \
                     more text after the step, which runs on to another line of the paragraph.\n\n"
                );
            }
            s.push_str("\\endgroup\n\n");
        }
        s.push_str("\\closeout1\n\\bye\n");
        s
    };
    let map = "\\global\\advance\\count200 1 ";
    let val = "\\global\\count200=\\numexpr\\count200+1\\relax ";
    let front = "\\global\\advance\\count200 100\n";
    let (want_dvi, want_out) = cold("both-val", &src(val, ""), 1);
    assert!(want_out.contains("p3="), "several pages: {want_out:?}");
    let (want_dvi2, want_out2) = cold("both-val2", &src(val, front), 1);
    let file = |b: &mut phitex_tex::Build| -> String {
        b.writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect()
    };
    for threads in [1, 4, 8] {
        let dir = common::scratch(&format!("defer-both-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let mut b = common::build(opts, Some(&common::plain()), &src(map, ""));
        assert_eq!(b.output().0, want_dvi, "-j{threads}: cold");
        assert_eq!(file(&mut b), want_out, "-j{threads}: each page's value");
        common::rebuild(&mut b, &src(map, front));
        assert_eq!(b.output().0, want_dvi2, "-j{threads}: edited");
        assert_eq!(
            file(&mut b),
            want_out2,
            "-j{threads}: each page's value after the edit"
        );
        assert_eq!(
            b.stats.refuted_by_op("exec", "count:200")
                + b.stats.refuted_by_op("resume", "count:200"),
            0,
            "-j{threads}: no text reads the counter: {:?}",
            b.stats.refuted_by_field
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

/// Steps that cancel around a candidate of a run that cannot be cut (a group
/// open over paragraphs), under a routine that steps the same counter and so
/// defines its value: the run publishes nothing for them, and what they leave
/// is still later in TeX's order than the routine, which gives way. As
/// `\\g__kernel_prg_map_int` in `latex_starray`, where the routine's value stood
/// and the next step made it -1. Against the same text setting by value.
#[test]
fn a_routines_value_gives_way_to_steps_that_cancel_after_its_candidate() {
    let src = |down: &str, up: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\count200=5\n\
             \\output={\\global\\advance\\count200 1 \\count1=\\count200 \\global\\advance\\count200 -1 \
             \\immediate\\write1{p\\the\\count0=\\the\\count200.}\\shipout\\box255\\global\\advance\\count0 1 }\n",
        );
        for b in 0..6 {
            s.push_str("\\begingroup\n");
            for i in 0..12 {
                let _ = write!(
                    s,
                    "{down}Paragraph {b} {i}, long enough to make a line or two when set in the \
                     plain format's ten point roman, and one more clause to be sure, then more \
                     text which runs on to another line of the paragraph.\n\n{up}"
                );
            }
            s.push_str("\\endgroup\n\n");
        }
        s.push_str("The counter ends at \\the\\count200.\n\n\\closeout1\n\\bye\n");
        s
    };
    let (want_dvi, want_out) = cold(
        "cancel-val",
        &src(
            "\\global\\count200=\\numexpr\\count200-1\\relax ",
            "\\global\\count200=\\numexpr\\count200+1\\relax ",
        ),
        1,
    );
    assert!(
        want_out.contains("p2=4."),
        "several pages, each at 4: {want_out:?}"
    );
    for threads in [1, 4, 8] {
        let dir = common::scratch(&format!("defer-cancel-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let b = common::build(
            opts,
            Some(&common::plain()),
            &src(
                "\\global\\advance\\count200 -1 ",
                "\\global\\advance\\count200 1 ",
            ),
        );
        assert_eq!(b.output().0, want_dvi, "-j{threads}: cold");
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, want_out, "-j{threads}: each page's value");
    }
}

/// Steps a piece makes of a register it never reads, after a candidate where
/// a page fires inside the piece, go on what the routine left there, as in
/// TeX: here every routine adds 1 to `\\count200` and the text steps it -1
/// and +1 around each paragraph's start, so each page's value counts the
/// routines before it (`p2=6.p3=8.`, as the text setting by value gives).
/// Putting
/// back the value reaching the finisher (the text's steps on what reached
/// the piece) dropped the routines' steps. Against the same text setting by
/// value.
#[test]
fn steps_after_a_candidate_go_on_the_routines_value() {
    let src = |down: &str, up: &str| {
        let mut s = String::from(
            "\\openout1=\\jobname.out\n\\count200=5\n\
             \\output={\\global\\advance\\count200 1 \\count1=\\count200 \
             \\immediate\\write1{p\\the\\count0=\\the\\count200.}\\shipout\\box255\\global\\advance\\count0 1 }\n",
        );
        for b in 0..6 {
            s.push_str("\\begingroup\n");
            for i in 0..12 {
                let _ = write!(
                    s,
                    "{down}Paragraph {b} {i}, long enough to make a line or two when set in the \
                     plain format's ten point roman, and one more clause to be sure, then more \
                     text which runs on to another line of the paragraph.\n\n{up}"
                );
            }
            s.push_str("\\endgroup\n\n");
        }
        s.push_str("The counter ends at \\the\\count200.\n\n\\closeout1\n\\bye\n");
        s
    };
    let (want_dvi, want_out) = cold(
        "steps-routine-val",
        &src(
            "\\global\\count200=\\numexpr\\count200-1\\relax ",
            "\\global\\count200=\\numexpr\\count200+1\\relax ",
        ),
        1,
    );
    assert!(
        want_out.contains("p2=6.") && want_out.contains("p3=8."),
        "the routines' steps counted: {want_out:?}"
    );
    for threads in [1, 4, 8] {
        let dir = common::scratch(&format!("defer-steps-routine-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let b = common::build(
            opts,
            Some(&common::plain()),
            &src(
                "\\global\\advance\\count200 -1 ",
                "\\global\\advance\\count200 1 ",
            ),
        );
        let got: String = b
            .writes()
            .into_iter()
            .filter(|(k, _)| *k == 1)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, want_out, "-j{threads}: each page's value");
        assert_eq!(b.output().0, want_dvi, "-j{threads}: cold");
    }
}

/// The first `\outer` macro, made by an edit before a run that scanned a list
/// holding its token (no format, so none was made before): that run stops at
/// the token with TeX's error (§357), and goes on past it once the edit is
/// reverted. A scan reads no token's meaning while none is made
/// (`val::OUTER_MADE`), so the run's read of that alone must bring it back.
#[test]
fn an_outer_macro_an_edit_makes_stops_a_later_scan() {
    let doc = |first: &str| {
        format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\catcode`\\#=6\n\
             \\def\\foo{{}}\\def\\list{{a\\foo b}}\\def\\m#1.{{}}\n\
             {first}First.\\par\n\n\
             Second.\\par\n\n\
             \\expandafter\\m\\list.\\par\n\n\
             \\end\n"
        )
    };
    let forbidden = |b: &phitex_tex::Build| {
        b.diagnostics()
            .iter()
            .any(|d| d.message.contains("Forbidden control sequence"))
    };
    for threads in [1, 4] {
        let dir = common::scratch(&format!("defer-outer-edit-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let mut b = common::build(opts, None, &doc(""));
        assert!(!forbidden(&b), "-j{threads}: cold");
        common::rebuild(&mut b, &doc("\\outer\\def\\foo{}"));
        assert!(forbidden(&b), "-j{threads}: the edit");
        common::rebuild(&mut b, &doc(""));
        assert!(!forbidden(&b), "-j{threads}: the revert");
    }
}

/// The text of [`a_name_built_from_a_stepped_counter_reads_through_the_name`]
/// (`lit`: each name's number written out, the oracle; the counter's
/// value reaching paragraph i is 5 + shift + i).
fn dname_text(extra: &str, lit: Option<i32>) -> String {
    let mut s = String::from("\\openout1=\\jobname.out\n\\count200=5\n\\countdef\\cn=200\n");
    // (pg@1 to pg@329 the same, pg@330 to pg@340 another, none past 350)
    for n in 1..=350 {
        let body = if (330..=340).contains(&n) {
            "other"
        } else {
            "same"
        };
        let _ = writeln!(s, "\\expandafter\\def\\csname pg@{n}\\endcsname{{{body}}}%");
    }
    s.push_str(extra);
    for i in 0..60 {
        let (num, less) = match lit {
            Some(sh) => {
                let v = 5 + sh + i + 1;
                (v.to_string(), (v - 1).to_string())
            }
            None => (
                "\\number\\cn".to_owned(),
                "\\number\\numexpr\\cn-1".to_owned(),
            ),
        };
        let cmp = match lit {
            Some(sh) => (5 + sh + i).to_string(),
            None => "\\numexpr\\cn-1".to_owned(),
        };
        let own = if i == 30 {
            format!("\\expandafter\\def\\csname pg@{num}\\endcsname{{own}}")
        } else {
            String::new()
        };
        let _ = write!(
            s,
            "\\global\\advance\\cn 1 \\edef\\w{{\\write1{{n{i}=\\the\\count200.}}}}\\w{{}}{own}\
             Paragraph {i} \\ifcsname pg@{num}\\endcsname has\\else lacks\\fi{{}} \
             \\expandafter\\ifx\\csname pg@{less}\\endcsname\\relax relax\\else set\\fi{{}} \
             [\\csname pg@{num}\\endcsname] \
             \\ifnum{cmp}>100 late\\else early\\fi{{}}, long enough to make a line \
             or two when set in the plain format's ten point roman.\n\n"
        );
    }
    s.push_str("\\closeout1\n\\bye\n");
    s
}

/// A name built from a counter a piece only steps is a relative name
/// (docs/spec-relative-counters.md §2.2): `\ifcsname`, `\ifx` of the
/// `\csname`, the `\csname` expanded, `\numexpr` of the counter plus a
/// constant inside the name and `\ifnum` of that against a constant read
/// through the derived name and the comparison, not the counter's digits
/// (biblatex's `\blx@iffirstonpage` in every citation). A step added
/// before every paragraph whose new names hold what the old ones held
/// asks none of them again; one that moves the names onto others gives
/// what a cold build gives, a name a paragraph defines itself and one
/// `\csname` makes `\relax` included.
#[test]
fn a_name_built_from_a_stepped_counter_reads_through_the_name() {
    let text = dname_text;
    let src = |extra: &str| text(extra, None);
    let oracle = |name: &str, extra: &str, sh: i32| {
        let (want, _) = cold(name, &text(extra, Some(sh)), 1);
        want
    };
    for threads in [1, 8] {
        let dir = common::scratch(&format!("defer-dname-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let first = src("");
        let mut b = common::build(opts, Some(&common::plain()), &first);
        let (want, out) = cold("dname", &first, 1);
        assert_eq!(b.output().0, want, "-j{threads}: cold");
        assert_eq!(
            want,
            oracle("dname-lit", "", 0),
            "cold equals the names written out"
        );
        let log = b.output().1;
        assert!(
            out.contains("n0=6.") && out.contains("n59=65."),
            "{out:?}\n{}",
            log.lines().rev().take(60).collect::<Vec<_>>().join("\n")
        );
        let asked = |b: &phitex_tex::Build| {
            b.stats.refuted_by_op("exec", "count:200")
                + b.stats.refuted_by_op("resume", "count:200")
        };
        // (every name one on, holding what the one before held)
        let near = src("\\global\\advance\\count200 1\n");
        let r0 = asked(&b);
        common::rebuild(&mut b, &near);
        let (want, out) = cold("dname-near", &near, 1);
        assert_eq!(b.output().0, want, "-j{threads}: one on equals cold");
        assert!(out.contains("n0=7."), "{out:?}");
        assert_eq!(
            want,
            oracle("dname-near-lit", "\\global\\advance\\count200 1\n", 1),
            "one on: cold equals the names written out"
        );
        // (p30 defines a name of that shape: its digits read; p31, p32 asked once and stand, no run)
        assert!(
            asked(&b) <= r0 + 3,
            "-j{threads}: no paragraph asked again on the counter when no name's meaning moved: {:?}",
            b.stats.refuted_by_field
        );
        // (names onto others, undefined ones, the comparison's outcome)
        let far = src("\\global\\advance\\count200 300\n");
        common::rebuild(&mut b, &far);
        let (want, out) = cold("dname-far", &far, 1);
        assert_eq!(b.output().0, want, "-j{threads}: moved equals cold");
        assert!(out.contains("n0=306."), "{out:?}");
        assert_eq!(
            want,
            oracle("dname-far-lit", "\\global\\advance\\count200 300\n", 300),
            "moved: cold equals the names written out"
        );
        common::rebuild(&mut b, &first);
        let (want, _) = cold("dname", &first, 1);
        assert_eq!(b.output().0, want, "-j{threads}: reverted equals cold");
    }
}
