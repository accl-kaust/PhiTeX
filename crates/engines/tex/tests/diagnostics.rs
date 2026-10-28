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
        let b = build(doc, |o| Options { threads: Some(threads), ..o });
        let ds = b.diagnostics();
        show(&ds);
        let errs: Vec<(&str, usize)> = ds.iter().filter(|d| d.severity == Severity::Error).map(|d| (d.message.as_str(), d.line)).collect();
        assert_eq!(
            errs,
            vec![("Incomplete \\iftrue; all text was ignored after line 1", 1), ("Extra \\fi", 2)],
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
        let doc = "\\iftrue a\\fi\\else b \\iftrue c\\or d\\fi\n\\iffalse e\\or f\\else g\\fi\n\\bye\n";
        let b = build(doc, |o| Options { threads: Some(threads), ..o });
        let ds = b.diagnostics();
        show(&ds);
        let errs: Vec<(&str, usize)> = ds.iter().filter(|d| d.severity == Severity::Error).map(|d| (d.message.as_str(), d.line)).collect();
        assert_eq!(errs, vec![("Extra \\else", 1), ("Extra \\or", 1), ("Extra \\or", 2)], "-j{threads}");
    }
}
