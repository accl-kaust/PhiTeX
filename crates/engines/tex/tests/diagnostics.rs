//! `Build::diagnostics`: TeX's errors and warnings and the engine's
//! evaluation errors, each at its place in a file.

mod common;

use phitex_tex::{Build, Diagnostic, Options, Severity};

fn build(doc: &str, opts: impl FnOnce(Options) -> Options) -> Build {
    let dir = common::fixtures().join("diag");
    let o = opts(common::options("./doc.tex", "doc", "", &dir));
    common::build(o, Some(&common::plain()), doc)
}

fn show(ds: &[Diagnostic]) {
    for d in ds {
        eprintln!("{d:?}");
    }
}

#[test]
fn undefined_control_sequence() {
    let doc = "Hello \\foo world.\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let d = &ds[0];
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.message, "Undefined control sequence");
    assert_eq!(d.file, "./doc.tex");
    assert_eq!(&doc[d.range.0..d.range.1], "\\foo");
    assert_eq!(d.line, 1);
    assert_eq!(
        d.context,
        format!("l.1 Hello \\foo\n{} world.", " ".repeat(14))
    );
    assert!(d.help.is_some());
}

#[test]
fn an_error_in_an_input_file() {
    let doc = "\\input bad\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let d = &ds[0];
    assert_eq!(d.message, "Undefined control sequence");
    assert!(d.file.ends_with("bad.tex"), "{}", d.file);
    assert_eq!(d.range, (5, 9));
    assert_eq!(d.line, 1);
}

#[test]
fn box_reports_are_warnings() {
    let doc = "\\hbox to 10pt{Far too wide}\n\\hbox to 300pt{a b}\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    assert_eq!(ds.len(), 2);
    assert!(ds.iter().all(|d| d.severity == Severity::Warning));
    assert!(
        ds[0].message.starts_with("Overfull \\hbox ("),
        "{}",
        ds[0].message
    );
    assert!(ds[0].message.ends_with("pt too wide)"), "{}", ds[0].message);
    assert!(
        ds[1].message.starts_with("Underfull \\hbox (badness "),
        "{}",
        ds[1].message
    );
    assert_eq!((ds[0].line, ds[1].line), (1, 2));
}

#[test]
fn missing_dollar_missing_file_and_missing_character() {
    let doc = "\\tracinglostchars=1 \\char200\n\nx^2\n\n\\input nosuchfile\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let ms: Vec<&str> = ds.iter().map(|d| d.message.as_str()).collect();
    assert!(
        ms.iter()
            .any(|m| m.starts_with("Missing character: There is no")),
        "{ms:?}"
    );
    assert!(ms.contains(&"Missing $ inserted"), "{ms:?}");
    assert!(ms.iter().any(|m| m.contains("find file")), "{ms:?}");
    let lines: Vec<usize> = ds.iter().map(|d| d.line).collect();
    assert!(lines.windows(2).all(|w| w[0] <= w[1]), "{lines:?}");
}

#[test]
fn a_runaway_argument() {
    let doc = "\\def\\a#1{}\\a{one\n\npar}\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    assert!(
        ds.iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("Paragraph ended before")),
        "{ds:?}"
    );
}

#[test]
fn an_edit_that_fixes_the_error_removes_it() {
    let doc = "Hello \\foo world.\n\\bye\n";
    let fixed = "Hello \\TeX\\ world.\n\\bye\n";
    let mut b = build(doc, |o| o);
    assert_eq!(b.diagnostics().len(), 1);
    b.edit(fixed);
    assert!(b.diagnostics().is_empty(), "{:?}", b.diagnostics());
    let cold = build(fixed, |o| o);
    assert_eq!(b.output().0, cold.output().0);
}

#[test]
fn the_time_limit_is_an_error_not_a_panic() {
    let b = build("Hello.\n\\bye\n", |o| Options {
        time_limit: Some(std::time::Duration::ZERO),
        ..o
    });
    let ds = b.diagnostics();
    show(&ds);
    assert!(ds.iter().any(|d| d.severity == Severity::Error), "{ds:?}");
}

#[test]
fn did_you_mean() {
    let doc = "\\hbx{}\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let h = ds[0].help.as_deref().unwrap_or("");
    assert!(h.contains("Did you mean \\hbox"), "{h}");
}

/// The names an undefined control sequence's help suggests, in order.
fn suggestions(doc: &str) -> Vec<String> {
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let d = ds
        .iter()
        .find(|d| d.message == "Undefined control sequence")
        .expect("an undefined control sequence");
    let h = d.help.as_deref().unwrap_or("");
    h.lines()
        .find_map(|l| l.strip_prefix("Did you mean "))
        .map(|l| {
            l.trim_end_matches('?')
                .split(", ")
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn did_you_mean_the_nearest_primitive_first() {
    let s = suggestions("\\hboxx{}\n\\bye\n");
    assert_eq!(s.first().map(String::as_str), Some("\\hbox"), "{s:?}");
}

#[test]
fn did_you_mean_a_name_the_document_uses_first() {
    let s = suggestions("\\def\\mymacro{x}\\mymacro\n\\mymacr\n\\bye\n");
    assert_eq!(s.first().map(String::as_str), Some("\\mymacro"), "{s:?}");
    // (before a nearer name the document does not have: \hbox is one
    // edit from \hbax, the document's \hbaxyz two)
    let s = suggestions("\\def\\hbaxyz{x}\\hbaxyz\n\\hbax\n\\bye\n");
    assert_eq!(s.first().map(String::as_str), Some("\\hbaxyz"), "{s:?}");
    assert!(s.iter().any(|n| n == "\\hbox"), "{s:?}");
}

#[test]
fn did_you_mean_no_names_with_characters_the_typo_lacks() {
    for typo in ["pp", "zz", "mne", "hbx", "foo"] {
        let s = suggestions(&format!("\\{typo}{{}}\n\\bye\n"));
        assert!(
            s.iter().all(|n| !n[1..].contains(['@', ':', '_'])),
            "\\{typo}: {s:?}"
        );
    }
}

/// A diagnostic after an edit that moved the text before it is where
/// the text now has it, as a cold build of the new text places it: the
/// nodes after the edit keep their texts, whose frames carry the byte
/// positions they were sliced at.
#[test]
fn a_diagnostic_after_a_deletion_before_it_is_where_its_text_now_is() {
    let par = |k: usize| format!("Paragraph {k} has some words in it.\n\n");
    let mut doc = String::new();
    for k in 0..12 {
        doc.push_str(&par(k));
    }
    doc.push_str("Hello \\foo world.\n\\bye\n");
    let mut b = build(&doc, |o| o);
    let edited = doc.replacen(&par(3), "", 1);
    b.edit(&edited);
    let got = b.diagnostics();
    let cold = build(&edited, |o| o).diagnostics();
    show(&got);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(&edited[got[0].range.0..got[0].range.1], "\\foo", "{got:?}");
    assert_eq!(
        (got[0].range, got[0].line),
        (cold[0].range, cold[0].line),
        "the edit's diagnostic against a cold build's"
    );
}

/// An `\input` file that ends inside the text an `\else` skips: TeX's
/// end of the file (§362) checks it (§336), "Incomplete \iftrue; all text
/// was ignored after line 1" and a `\fi` inserted there; the document's
/// `\fi` after the `\input` then matches no `\if` (§510): "Extra \fi",
/// ignored. Oracle: pdftex (TeX Live 2026, accl job 21719), those two
/// errors at l.1 and l.2 and the page with "x after text". Before, the
/// skip went on into the document to its `\fi` and neither was reported.
#[test]
fn an_input_file_ending_inside_a_skipped_else() {
    for threads in [1, 8] {
        let doc = "\\input skipend\nafter \\fi text\n\\bye\n";
        let b = build(doc, |o| Options {
            threads: Some(threads),
            ..o
        });
        let ds = b.diagnostics();
        show(&ds);
        let errs: Vec<(&str, usize)> = ds
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.message.as_str(), d.line))
            .collect();
        assert_eq!(
            errs,
            vec![
                ("Incomplete \\iftrue; all text was ignored after line 1", 1),
                ("Extra \\fi", 2)
            ],
            "-j{threads}"
        );
    }
}

/// §510's and §500's "Extra" errors: an `\else` outside every
/// conditional, an `\or` in an `\iftrue`'s taken text, an `\or` met in an
/// `\iffalse`'s skipped text. Each ignored, and reported. Oracle: pdftex
/// (TeX Live 2026, accl job 21793), the three errors in this order and
/// "a b c g" set.
#[test]
fn an_extra_else_or_or_is_reported() {
    for threads in [1, 8] {
        let doc =
            "\\iftrue a\\fi\\else b \\iftrue c\\or d\\fi\n\\iffalse e\\or f\\else g\\fi\n\\bye\n";
        let b = build(doc, |o| Options {
            threads: Some(threads),
            ..o
        });
        let ds = b.diagnostics();
        show(&ds);
        let errs: Vec<(&str, usize)> = ds
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.message.as_str(), d.line))
            .collect();
        assert_eq!(
            errs,
            vec![("Extra \\else", 1), ("Extra \\or", 1), ("Extra \\or", 2)],
            "-j{threads}"
        );
    }
}

/// `\showbox` (§1296): the box as `show_box` displays it (§182–§198),
/// then "! OK." (§1298), an error. Oracle: pdftex in batch mode (TeX
/// Live 2026, mc job 75792), each block from "> \box" to "! OK."; the
/// log here has no context lines after an error.
#[test]
fn showbox_displays_the_box_and_says_ok() {
    let doc = "\\batchmode\n\
\\setbox0\\hbox{A\\kern1pt\\vrule\\hskip 2pt plus 1fil B}\n\
\\showbox0\n\
\\showbox1\n\
\\setbox2\\hbox{a\\-b\\discretionary{x}{y}{zw}\\leaders\\hrule\\hskip3pt\\mark{q}\\vadjust{\\kern1pt}$x$\\special{s}\\write3{w}\\penalty7\\hbox to 20pt{\\hfil}\\lower2pt\\hbox{}}\n\
{\\showboxdepth=5 \\showboxbreadth=100 \\showbox2}\n\
\\setbox4\\hbox{\\hbox{\\hbox{x}}abc}\n\
{\\showboxdepth=1 \\showboxbreadth=2 \\showbox4}\n\
\\setbox6\\vbox{\\hsize=50pt \\noindent A long paragraph here.\\par}\n\
{\\showboxdepth=5 \\showboxbreadth=100 \\showbox6}\n\
\\showlists\n\
\\bye\n";
    let want = [
        "> \\box0=\n\\hbox(6.83331+0.0)x17.98337\n.\\tenrm A\n.\\kern 1.0\n.\\rule(*+*)x0.4\n.\\glue 2.0 plus 1.0fil\n.\\tenrm B\n\n! OK.\n",
        "> \\box1=void\n\n! OK.\n",
        "> \\box2=\n\\hbox(6.94444+2.0)x50.93753\n.\\tenrm a\n.\\discretionary\n..\\tenrm -\n.\\tenrm b\n\
.\\discretionary replacing 2\n..\\tenrm x\n.|\\tenrm y\n.\\tenrm z\n.\\tenrm w\n.\\leaders 3.0\n..\\rule(0.4+0.0)x*\n\
.\\mark{q}\n.\\vadjust\n..\\kern 1.0\n.\\mathon\n.\\teni x\n.\\mathoff\n.\\special{s}\n.\\write3{w}\n.\\penalty 7\n\
.\\hbox(0.0+0.0)x20.0, glue set 20.0fil\n..\\glue 0.0 plus 1.0fil\n.\\hbox(0.0+0.0)x0.0, shifted 2.0\n\n! OK.\n",
        "> \\box4=\n\\hbox(6.94444+0.0)x20.55562\n.\\hbox(4.30554+0.0)x5.2778 []\n.\\tenrm a\n.etc.\n\n! OK.\n",
        "> \\box6=\n\\vbox(18.94444+1.94444)x50.0\n.\\hbox(6.94444+1.94444)x50.0, glue set - 1.0\n..\\tenrm A\n\
..\\glue 3.33333 plus 1.66498 minus 1.11221\n..\\tenrm l\n..\\tenrm o\n..\\tenrm n\n..\\tenrm g\n\
..\\glue 3.33333 plus 1.66666 minus 1.11111\n..\\tenrm p\n..\\tenrm a\n..\\tenrm r\n..\\tenrm a\n..\\discretionary\n\
..\\tenrm -\n..\\glue(\\rightskip) 0.0\n..\\rule(*+*)x5.0\n.\\penalty 400\n.\\glue(\\baselineskip) 3.11111\n\
.\\hbox(6.94444+1.94444)x50.0, glue set 0.49991fil\n..\\tenrm g\n..\\tenrm r\n..\\tenrm a\n..\\tenrm p\n..\\tenrm h\n\
..\\glue 3.33333 plus 1.66666 minus 1.11111\n..\\tenrm h\n..\\tenrm e\n..\\tenrm r\n..\\tenrm e\n..\\tenrm .\n\
..\\penalty 10000\n..\\glue(\\parfillskip) 0.0 plus 1.0fil\n..\\glue(\\rightskip) 0.0\n\n! OK.\n",
    ];
    for threads in [1, 8] {
        let b = build(doc, |o| Options {
            threads: Some(threads),
            ..o
        });
        let (_, log) = b.output();
        for w in want {
            assert!(
                log.contains(w),
                "-j{threads}: the log has\n{w}\nlog:\n{log}"
            );
        }
        let oks = b.error_lines().iter().filter(|l| *l == "! OK.").count();
        assert_eq!(
            oks, 6,
            "-j{threads}: an error for each \\showbox and the \\showlists"
        );
    }
}

/// An extra `}` (§1069): "Too many }'s" at the bottom level, "Extra },
/// or forgotten ..." in a semi-simple group or a formula; each ignored.
/// Oracle: pdftex (TeX Live 2026, mc job 75792).
#[test]
fn an_extra_right_brace_is_reported() {
    for threads in [1, 8] {
        let doc = "x}\n\\begingroup } \\endgroup\n$x}$\n\\bye\n";
        let b = build(doc, |o| Options {
            threads: Some(threads),
            ..o
        });
        let ds = b.diagnostics();
        show(&ds);
        let errs: Vec<(&str, usize)> = ds
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| (d.message.as_str(), d.line))
            .collect();
        assert_eq!(
            errs,
            vec![
                ("Too many }'s", 1),
                ("Extra }, or forgotten \\endgroup", 2),
                ("Extra }, or forgotten $", 3)
            ],
            "-j{threads}"
        );
    }
}

/// `\show`, `\showthe`, `\showtokens`, `\showgroups` and `\showifs` end
/// in `error` (§1293's `common_ending`): no "! " line, but the history is
/// `error_message_issued`, and pdftex exits 1 after each (TeX Live 2026,
/// mc job 75789: `\show\par`, `\showthe\count0`).
#[test]
fn a_show_is_an_error_for_the_history() {
    for (doc, logged) in [
        ("\\show\\par\nx\\bye\n", true),
        ("\\showthe\\count0\nx\\bye\n", true),
        ("\\showtokens{a}\nx\\bye\n", true),
        ("\\showgroups\nx\\bye\n", true),
        ("\\showifs\nx\\bye\n", true),
        ("x\\bye\n", false),
    ] {
        let b = build(doc, |o| o);
        assert_eq!(b.errors_logged(), logged, "{doc:?}");
        assert!(b.error_lines().is_empty(), "{doc:?}: no \"! \" line");
    }
}

/// A box report as TeX logs it (§663): the report and where (a
/// paragraph's lines, an `\hbox`'s line), its list's `short_display`,
/// the box shown; a paragraph's diagnostic at its start. After an edit
/// of another paragraph, the log has it still, and the diagnostic.
#[test]
fn box_reports_are_in_the_log_as_tex_writes_them() {
    let doc = "\\hsize=100pt \\font\\tt=cmtt10 \\parindent=0pt\nIts packs come from \\hbox{\\tt https://shelf.phitex.org/tl2026/packs}.\n\nOther text.\n\n\\hbox to 10pt{Far too wide}\n\\bye\n";
    let mut b = build(doc, |o| o);
    let check = |b: &Build, what: &str| {
        let (_, log) = b.output();
        for want in [
            "\nOverfull \\hbox (183.83177pt too wide) in paragraph at lines 2--3\n[]\\tenrm Its packs come from []. |\n\n\\hbox(6.94444+2.22223)x100.0, glue set - 1.0\n",
            "\nOverfull \\hbox (43.22232pt too wide) detected at line 6\n\\tenrm Far too wide|\n\n\\hbox(6.94444+0.0)x10.0, glue set - 1.0\n",
        ] {
            assert!(log.contains(want), "{what}: no {want:?} in the log:\n{log}");
        }
        let ds = b.diagnostics();
        show(&ds);
        let lines: Vec<(usize, &str)> = ds.iter().map(|d| (d.line, d.message.as_str())).collect();
        assert_eq!(
            lines,
            [
                (2, "Overfull \\hbox (183.83177pt too wide) in paragraph"),
                (6, "Overfull \\hbox (43.22232pt too wide)")
            ],
            "{what}"
        );
    };
    check(&b, "cold");
    common::rebuild(&mut b, &doc.replace("Other text.", "Other words."));
    check(&b, "after an edit of the paragraph between");
}

/// A warning written to the log in the kernel's shapes (`LaTeX
/// Warning: `, `LaTeX Font Warning: `, `Package <name> Warning: `,
/// `Class <name> Warning: `; LaTeX's `\GenericWarning` is a `\write` on
/// a stream no file is open on) is a warning diagnostic where the write
/// was met: its first line the message, its continuation lines (without
/// their `(name)` lead) the help; the culprit the command whose
/// argument holds the key the message quotes, else the last control
/// sequence read. A text of another shape is none.
#[test]
fn written_warnings_are_diagnostics() {
    let doc = "\\def\\r#1{\\immediate\\write16{LaTeX Warning: Reference `#1' on page 1 undefined on input line 1.}}\n\
               \\def\\warn{\\immediate\\write16{LaTeX Font Warning: Font shape `OT1/cmr/bx/sc' undefined^^J(Font)              using `OT1/cmr/bx/n' instead on input line 2.}}\n\
               See \\r{lbl} here.\n\
               A \\warn B\n\
               \\immediate\\write16{Package foo Warning: Something odd.}\n\
               \\immediate\\write16{Class bar Warning: Odd too^^J(bar)                and more.}\n\
               \\immediate\\write16{A plain message}\\immediate\\write-1{LaTeX Warning: Not from a \\string\\typeout.}\n\
               \\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let ws: Vec<&Diagnostic> = ds
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect();
    let ms: Vec<&str> = ws.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        ms,
        [
            "LaTeX Warning: Reference `lbl' on page 1 undefined on input line 1.",
            "LaTeX Font Warning: Font shape `OT1/cmr/bx/sc' undefined",
            "Package foo Warning: Something odd.",
            "Class bar Warning: Odd too",
            "LaTeX Warning: Not from a \\typeout.",
        ]
    );
    assert_eq!(&doc[ws[0].range.0..ws[0].range.1], "\\r{lbl}");
    assert_eq!((ws[0].line, ws[0].help.as_deref()), (3, None));
    assert_eq!(&doc[ws[1].range.0..ws[1].range.1], "\\warn");
    assert_eq!(
        (ws[1].line, ws[1].help.as_deref()),
        (4, Some("using `OT1/cmr/bx/n' instead on input line 2."))
    );
    assert_eq!(ws[2].range.0, ws[2].range.1);
    assert_eq!(ws[3].help.as_deref(), Some("and more."));
    assert!(ds.iter().all(|d| d.severity != Severity::Error), "{ds:?}");
}

/// §362 at the end of the main file: what the text ended in is TeX's
/// error (§338, "File ended while scanning use of \a"), then what §339
/// inserted is read, and only at the end again is the job aborted (§360,
/// "*** (job aborted, no legal \end found)"), as pdfTeX's log has them.
/// Before, the job was aborted at once and the error never said: an
/// unclosed brace in an editor had no problem but the abort. The name as
/// `sprint_cs` shows it, a space in it kept (LaTeX's robust `\textbf `,
/// "use of \textbf ."). Oracle: pdflatex on the same shapes in LaTeX
/// (TeX Live 2026, accl 24838).
#[test]
fn the_text_ending_inside_an_argument_or_a_definition() {
    for threads in [1, 4] {
        for (doc, msg) in [
            (
                "\\long\\def\\a#1{}\nText \\a{one\ntwo.\n",
                "File ended while scanning use of \\a",
            ),
            (
                "Text.\n\\def\\x{abc\n",
                "File ended while scanning definition of \\x",
            ),
            (
                "\\expandafter\\long\\expandafter\\def\\csname b \\endcsname#1{}\nText \\csname b \\endcsname{one\n",
                "File ended while scanning use of \\b ",
            ),
            // (plain's `\bye` is `\outer`: §336's other error there, the
            // name as shown the same way; `\bye` then ends the job)
            (
                "\\expandafter\\long\\expandafter\\def\\csname b \\endcsname#1{}\nText \\csname b \\endcsname{one\n\\bye\n",
                "Forbidden control sequence found while scanning use of \\b ",
            ),
        ] {
            let b = build(doc, |o| Options {
                threads: Some(threads),
                ..o
            });
            let ds = b.diagnostics();
            show(&ds);
            let errs: Vec<&str> = ds
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message.as_str())
                .collect();
            assert!(
                errs.iter().any(|m| m.trim_end_matches('.') == msg),
                "-j{threads} {doc:?}: {errs:?}"
            );
            let at = errs.iter().position(|m| m.trim_end_matches('.') == msg);
            let abort = errs.iter().position(|m| m.contains("no legal \\end found"));
            assert!(
                msg.starts_with("Forbidden") || abort.is_some_and(|a| at.is_some_and(|i| i < a)),
                "-j{threads} {doc:?}: the abort after the error: {errs:?}"
            );
        }
    }
}

/// An error's context and help in the log as pdfTeX's `error` writes them
/// in a run that cannot ask (§90): the list so far of a runaway (§306),
/// the message, the context's two lines a level (§311-§318: the line of
/// the file with its place read, `<inserted text>` and `<to be read
/// again>` above it, the terminal's `<*>` line once the main file ended),
/// the help lines and an empty line. The engine wrote the message alone.
/// Oracle: pdflatex on the same shapes under LaTeX (accl 24909, the
/// blocks byte for byte; bench/ux5-ctx), here under plain.
#[test]
fn an_error_has_its_context_and_help_in_the_log() {
    for threads in [1, 4] {
        for (doc, want) in [
            (
                "Bad \\nosuchcommand here.\n\\bye\n",
                "! Undefined control sequence.\nl.1 Bad \\nosuchcommand\n                       here.\nThe control sequence at the end of the top line\nof your error message was never \\def'ed. If you have\nmisspelled it (e.g., `\\hobx'), type `I' and the correct\nspelling (e.g., `I\\hbox'). Otherwise just continue,\nand I'll forget about whatever was undefined.\n\n",
            ),
            (
                "\\def\\x#1{}\nText \\x{abc\n\nrest.\n\\bye\n",
                "Runaway argument?\n{abc \n! Paragraph ended before \\x was complete.\n<to be read again> \n                   \\par \nl.3 \n    \nI suspect you've forgotten a `}', causing me to apply this\ncontrol sequence to too much text. How can we recover?\nMy plan is to forget the whole thing and hope for the best.\n\n",
            ),
            (
                "\\long\\def\\x#1{}\nText \\x{abc\ndef\n",
                "Runaway argument?\n{abc def \n! File ended while scanning use of \\x.\n<inserted text> \n                \\par \n<*> doc.tex\n           \nI suspect you have forgotten a `}', causing me\nto read past where you wanted me to stop.\nI'll try to recover; but if the error is serious,\nyou'd better type `E' or `X' now and fix your file.\n\n! Emergency stop.\n<*> doc.tex\n           \n*** (job aborted, no legal \\end found)\n\n",
            ),
        ] {
            let b = build(doc, |o| Options {
                threads: Some(threads),
                ..o
            });
            let (_, log) = b.output();
            assert!(
                log.contains(want),
                "-j{threads} {doc:?}: the log has not\n{want}\nlog:\n{log}"
            );
        }
    }
}

/// An error in a file the job writes and reads back (a `.bbl`, an
/// `.aux`): placed at its line in the text the build read. The finder
/// has no such file (or a stale copy on disk), and every error in one was
/// placed at its first line: the Overleaf extension's
/// "TeX capacity exceeded ... at paper.bbl:1" for an error on line 16.
#[test]
fn an_error_in_a_file_the_job_wrote_is_at_its_line() {
    let doc = "\\immediate\\openout1=gen.tex\n\
               \\immediate\\write1{first line}\n\
               \\immediate\\write1{second \\noexpand\\undefinedmacro\\space here}\n\
               \\immediate\\closeout1\n\
               \\input gen\n\\bye\n";
    let b = build(doc, |o| o);
    let ds = b.diagnostics();
    show(&ds);
    let d = ds
        .iter()
        .find(|d| d.message == "Undefined control sequence")
        .expect("the error");
    assert!(d.file.ends_with("gen.tex"), "{}", d.file);
    assert_eq!(d.line, 2, "{d:?}");
    assert_eq!(d.range, (18, 33), "{d:?}");
}
