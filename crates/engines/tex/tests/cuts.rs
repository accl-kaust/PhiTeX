//! Continuations as fields and relocated placements
//! (docs/spec-continuations.md): the rest of a cut piece is keyed by the
//! piece, a changed continuation or a cut at another candidate reruns it
//! in place, a moved page fire relocates it and keeps its finisher, and an
//! edit history ends where a cold build does.

use crate::common;

use std::fmt::Write as _;

use phitex_tex::Options;

fn doc(n: usize, k: usize, mid: &str) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i == k {
            s.push_str(mid);
            s.push_str("\n\n");
        }
        for _ in 0..3 {
            let _ = write!(
                s,
                "Paragraph {i} of the document, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches. "
            );
        }
        s.push_str("\n\n");
    }
    s.push_str("\\bye\n");
    s
}

/// A group of four paragraphs, cut at each paragraph end inside it.
fn group_mid() -> String {
    let mut s = String::from("\\begingroup\\count5=1 ");
    for j in 0..4 {
        let _ = write!(
            s,
            "\\advance\\count5 by 1 Group paragraph {j}, with a counter stepped before it, long enough to make a few lines when set in the plain format's ten point roman over the text width, and then some more words to be sure.\n\n"
        );
    }
    s.push_str("\\endgroup");
    s
}

fn build(name: &str, src: &str, threads: usize) -> phitex_tex::Build {
    let dir = common::scratch(&format!("cuts-{name}-{threads}"));
    let opts = Options {
        threads: Some(threads),
        ..common::options("./d.tex", "d", "", &dir)
    };
    common::build(opts, Some(&common::plain()), src)
}

fn cold(name: &str, src: &str) -> Vec<u8> {
    build(&format!("{name}-cold"), src, 1).output().0
}

#[test]
fn a_rest_of_a_piece_is_keyed_by_the_piece() {
    let src = doc(60, 30, &group_mid());
    for threads in [1, 8] {
        let b = build("keyed", &src, threads);
        assert_eq!(b.output().0, cold("keyed", &src), "-j{threads}");
        let live = b.nodes_by_op("resume");
        assert!(
            live >= 3,
            "-j{threads}: the group is cut at its paragraph ends: {live}"
        );
        assert_eq!(
            b.stats.placements_of("resume"),
            live,
            "-j{threads}: every rest placed once over the build: {:?}",
            b.stats.runs_by_op
        );
    }
}

#[test]
fn a_changed_continuation_reruns_the_rest_in_place() {
    let src = doc(60, 30, &group_mid());
    let mut b = build("cont", &src, 8);
    let edited = src.replacen("\\begingroup\\count5=1 ", "\\begingroup\\count5=3 ", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("cont-edited", &edited));
    assert_eq!(
        r.placements_made("resume"),
        0,
        "the rests rerun in place, none made anew: {r:?}"
    );
    assert!(r.placements_moved == 0 || r.placements_moved <= 4, "{r:?}");
}

#[test]
fn a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher() {
    let src = doc(80, 40, &group_mid());
    let mut b = build("fire", &src, 8);
    let edited = src.replacen("Paragraph 0 of", "An inserted paragraph that shifts every page break below it by a few lines, so the page firing inside the group moves to another of its candidates.\n\nParagraph 0 of", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("fire-edited", &edited));
    assert!(
        r.placements_made("finish") <= 1,
        "finishers kept by key, at most one new page: {r:?}"
    );
    assert_eq!(
        r.placements_made("resume"),
        0,
        "the rests are relocated, not remade: {r:?}"
    );
}

#[test]
fn a_paragraph_insert_history_equals_the_cold_build() {
    let base = format!(
        "\\headline={{\\hfil\\topmark\\ \\number\\count0\\hfil}}\n\\openout1=\\jobname.out\n{}",
        doc(
            80,
            40,
            &format!(
                "\\mark{{m\\number\\count0}}\\write1{{page \\number\\count0}}{}",
                group_mid()
            )
        )
    );
    let mut b = build("history", &base, 8);
    assert_eq!(b.output().0, cold("history-0", &base));
    let e1 = base.replacen(
        "Paragraph 0 of",
        "A first inserted paragraph that moves every page break by a few lines.\n\nParagraph 0 of",
        1,
    );
    common::rebuild(&mut b, &e1);
    assert_eq!(
        b.output().0,
        cold("history-1", &e1),
        "after the first insert"
    );
    let e2 = e1.replacen("Paragraph 50 of", "Paragraph 50 OF", 1);
    common::rebuild(&mut b, &e2);
    assert_eq!(b.output().0, cold("history-2", &e2), "after the word edit");
    let e3 = e2.replacen("Paragraph 20 of", "A second inserted paragraph, before the group, moving its page fire again.\n\nParagraph 20 of", 1);
    common::rebuild(&mut b, &e3);
    assert_eq!(
        b.output().0,
        cold("history-3", &e3),
        "after the second insert"
    );
}

/// A line of blanks after a group ends the paragraph, as an empty line
/// does (§347: blanks at a line's start leave the state N, and the
/// line's end in it is `\par`). The slice that begins with that line's
/// end began mid-line, its run in state M: a space, the paragraph went
/// on (fontspec's fontload-sizefeatures-1 set two paragraphs as one
/// line; pdftex agrees with the empty line's DVI, mc 76382).
#[test]
fn a_line_of_blanks_after_a_group_ends_the_paragraph() {
    let dir = common::scratch("cuts-blank-line");
    let dvi = |src: &str, threads: usize| {
        let mut o = common::options("./blank.tex", "blank", "", &dir);
        o.threads = Some(threads);
        common::build(o, Some(&common::plain()), src).output().0
    };
    for (blanks, empty) in [
        ("{a}\n   \nb\\bye\n", "{a}\n\nb\\bye\n"),
        ("{a}\n\t \nb\\bye\n", "{a}\n\nb\\bye\n"),
        ("   {\\it a}\n   \n   b\\bye\n", "{\\it a}\n\nb\\bye\n"),
        (
            "\\def\\x{x}\n   {a \\x}\n   \n   b \\x\n   \n   {c \\x}\n\n\\bye\n",
            "\\def\\x{x}\n{a \\x}\n\nb \\x\n\n{c \\x}\n\n\\bye\n",
        ),
    ] {
        let want = dvi(empty, 1);
        for threads in [1, 4] {
            let got = dvi(blanks, threads);
            assert!(!got.is_empty(), "{blanks:?}: no DVI");
            if let Some(d) = common::first_difference(&got, &want) {
                panic!("{blanks:?} at {threads} threads differs from the empty lines' DVI: {d}");
            }
        }
    }
}

/// A piece that runs again and cuts at another candidate keeps its rest:
/// the rest is the piece's, whatever candidate it was cut at, and runs
/// again in place on the new continuation, with the chain after it kept
/// (keyed by the candidate, the rest and every rest after it were made
/// anew, one link a commit: lineno's manual never ended at -j1, the
/// beamer user guide at -j4 made 13,400 pieces where -j1 makes 5,400,
/// accl 24911, 24931). Inside a conditional of the run's own the piece
/// cuts only where the page is made to fire (`Run::cuttable_here`), at
/// candidate 8 here; the edit joins two paragraphs before that, so the
/// same piece cuts at candidate 6. Keyed by the candidate, the edit made
/// one rest anew (accl 25191).
#[test]
fn a_cut_moved_to_another_candidate_keeps_the_rest() {
    let src = doc(60, 30, &forced_mid());
    let edited = src.replacen(
        "roman.\n\nForced paragraph 1,",
        "roman. Forced paragraph 1,",
        1,
    );
    assert_ne!(edited, src);
    for threads in [1, 8] {
        let mut b = build(&format!("recut-{threads}"), &src, threads);
        let before = b.nodes_by_op("resume");
        let r = common::rebuild(&mut b, &edited);
        assert_eq!(b.output().0, cold("recut-edited", &edited), "-j{threads}");
        assert_eq!(
            r.placements_made("resume"),
            0,
            "-j{threads}: the rest runs again in place, none made anew: {r:?}"
        );
        assert_eq!(
            b.nodes_by_op("resume"),
            before,
            "-j{threads}: the same cuts, the same rests"
        );
    }
}

/// Eight paragraphs in a conditional, a page made to fire after the
/// fourth.
fn forced_mid() -> String {
    let mut s = String::from("\\iftrue ");
    for j in 0..8 {
        if j == 4 {
            s.push_str("\\penalty-10000\n\n");
        }
        let _ = write!(
            s,
            "Forced paragraph {j}, long enough to make a line or two when set in the plain format's ten point roman.\n\n"
        );
    }
    s.push_str("\\fi");
    s
}

/// A document whose `\slide` reads `\count200` only inside a group it
/// closes before the rest of its work, as beamer's `\beamer@setuplinks`
/// reads the `.nav` list in `{... \dohead}`: the group's `\loop` runs
/// `\count200` times and sets a global that does not depend on it; the
/// rest, inside the outer group and a conditional the macro opened (no
/// candidate can cut it there), sets three paragraphs with the outer
/// group's locals, that global and what an `\aftergroup` token the inner
/// group left does (docs/spec-group-cuts.md).
fn slide_doc(entries: u32, local: u32, after: &str) -> String {
    let mut s = format!(
        "\\count200={entries}\n\\def\\afterthing{{{after}}}\n\\def\\slide{{\\begingroup\\iftrue\n  {{\\count1=0 \\loop\\ifnum\\count1<\\count200 \\advance\\count1 by 1 \\global\\count7=3 \\repeat\\aftergroup\\afterthing}}%\n  \\count5={local} \\def\\x{{local}}%\n"
    );
    for j in 0..3 {
        let _ = write!(
            s,
            "  Slide paragraph {j} with the count \\the\\count5, the macro \\x, the global \\the\\count7 and \\the\\count6, long enough to make a line or two when set in the plain format's ten point roman.\\par\n"
        );
    }
    s.push_str("\\fi\\endgroup}\n\n");
    s.push_str(&doc(12, 6, "\\slide"));
    s
}

/// The runs of operation `op` in an evaluation.
fn runs(r: &phitex_tex::Report, op: &str) -> u64 {
    r.runs_by_op.iter().filter(|x| x.0 == op).map(|x| x.1).sum()
}

/// The group's close inside `\slide` is a cut: the piece up to it runs
/// again for the edited `\count200` it reads there, and the rest, its
/// continuation the same (the closed group's locals gone, its `\loop`'s
/// `\iterate` among them), does not run. Without the cut the slide is
/// one node, run again whole: no rest of its own.
#[test]
fn group_cut_a_read_inside_a_closed_group_reruns_only_the_piece_up_to_it() {
    let src = slide_doc(5, 7, "\\count6=9 ");
    let more = slide_doc(40, 7, "\\count6=9 ");
    for threads in [1, 8] {
        let mut b = build(&format!("gcut-read-{threads}"), &src, threads);
        assert_eq!(b.output().0, cold("gcut-read", &src), "-j{threads}");
        assert!(
            b.stats.group_cuts >= 1,
            "-j{threads}: the slide is cut at its inner group's close"
        );
        assert!(
            b.nodes_by_op("resume") >= 1,
            "-j{threads}: the slide's rest is a node of its own"
        );
        for (k, text) in [&more, &src].into_iter().enumerate() {
            let r = common::rebuild(&mut b, text);
            assert_eq!(
                b.output().0,
                cold(&format!("gcut-read-{k}"), text),
                "-j{threads} edit {k}"
            );
            assert!(
                runs(&r, "exec") >= 1,
                "-j{threads} edit {k}: the piece reading the count runs again: {r:?}"
            );
            assert_eq!(
                runs(&r, "resume"),
                0,
                "-j{threads} edit {k}: the slide's rest does not run: {r:?}"
            );
        }
    }
}

/// What goes across a group's cut is the state there: the outer group's
/// locals, the conditional open, the `\aftergroup` token on the input.
/// Edits of each run what reads them, the output a cold build's; a build
/// that cuts nowhere makes the same output.
#[test]
fn group_cut_carries_the_open_groups_the_conditional_and_the_aftergroup_tokens() {
    let src = slide_doc(5, 7, "\\count6=9 ");
    let uncut = {
        let dir = common::scratch("cuts-gcut-uncut");
        let opts = Options {
            threads: Some(1),
            cut_min: Some(u64::MAX),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        assert_eq!(b.stats.group_cuts, 0, "a build that cuts nowhere");
        b.output().0
    };
    assert!(
        common::first_difference(&cold("gcut-state", &src), &uncut).is_none(),
        "the cut build's output is the uncut one's"
    );
    for threads in [1, 8] {
        let mut b = build(&format!("gcut-state-{threads}"), &src, threads);
        for (k, text) in [
            slide_doc(5, 8, "\\count6=9 "),
            slide_doc(5, 8, "\\count6=4 "),
            slide_doc(6, 8, "\\count6=4 \\count5=1 "),
            src.clone(),
        ]
        .iter()
        .enumerate()
        {
            let r = common::rebuild(&mut b, text);
            assert_eq!(
                b.output().0,
                cold(&format!("gcut-state-{k}"), text),
                "-j{threads} edit {k}"
            );
            // (the rest reads what each edit moved: the local set after
            // the cut is in its input, the `\aftergroup` macro's meaning
            // a read of its own)
            if k < 3 {
                assert!(
                    runs(&r, "resume") >= 1,
                    "-j{threads} edit {k}: the rest runs for what it reads: {r:?}"
                );
            }
        }
    }
}

/// A `\begingroup` scope a macro opens and closes is no cut, nor is a
/// brace group opened at a line's brace: only a brace group or a box
/// opened in a macro's expansion still read at its close
/// (docs/spec-group-cuts.md).
#[test]
fn group_cut_not_at_a_semi_simple_group_or_a_line_s_brace() {
    let semi = slide_doc(5, 7, "\\count6=9 ")
        .replace("{\\count1=0 \\loop", "\\begingroup\\count1=0 \\loop");
    let semi = semi.replace(
        "\\aftergroup\\afterthing}%",
        "\\aftergroup\\afterthing\\endgroup%",
    );
    let b = build("gcut-semi", &semi, 1);
    assert_eq!(b.output().0, cold("gcut-semi", &semi));
    assert_eq!(b.stats.group_cuts, 0, "a \\begingroup scope is no cut");
    let line = doc(12, 6, "{\\count1=5 \\count2=6 }");
    let b = build("gcut-line", &line, 1);
    assert_eq!(b.output().0, cold("gcut-line", &line));
    assert_eq!(b.stats.group_cuts, 0, "a line's brace group is no cut");
}
