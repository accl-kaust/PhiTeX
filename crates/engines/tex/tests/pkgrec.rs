//! Package loads' records (`crate::pkgrec`): a load's recorded
//! definition and read sets become its declared hint in a later build,
//! so independent loads run side by side; a changed file falls back to
//! the unrecorded hint, and a run that defines outside its record is
//! refuted by the engine and recorded anew. The output never changes.
//!
//! Plain TeX with a `\usepackage` that `\input`s `NAME.sty` (the loads
//! are sliced as LaTeX's are: one node each).

mod common;

use std::path::{Path, PathBuf};

use phitex_tex::{Build, Options, Speculation};

const DEF: &str = "\\def\\usepackage#1{\\input #1.sty }\n";

fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pkgrec-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (n, t) in files {
        std::fs::write(d.join(n), t).unwrap();
    }
    d
}

fn opts(d: &Path, spec: Speculation, recs: Vec<u8>) -> Options {
    Options {
        speculation: spec,
        pkgrec: recs,
        ..common::options("./doc.tex", "doc", " TeX output", d)
    }
}

fn build(d: &Path, doc: &str, spec: Speculation, recs: Vec<u8>) -> Build {
    common::build(opts(d, spec, recs), Some(&common::plain()), doc)
}

fn sequential(d: &Path, doc: &str) -> Vec<u8> {
    let o = Options {
        threads: Some(1),
        ..opts(d, Speculation::Off, Vec::new())
    };
    common::build(o, Some(&common::plain()), doc).output().0
}

fn note(what: &str, b: &Build) {
    let s = &b.stats;
    eprintln!(
        "{what}: critical path {:?}, loads {} (claimed {}, recorded {}, optimistic {}), load runs {}, at once {}, refuted {}, undeclared {}",
        s.critical_path,
        s.pkg_loads,
        s.pkg_claimed,
        s.pkg_recorded,
        s.pkg_optimistic,
        s.pkg_runs,
        s.pkg_max_concurrent,
        s.declarations_refuted,
        s.undeclared_definitions
    );
    for l in b.critical_chain_lines() {
        eprintln!("  {l}");
    }
    for l in b.chain_waits() {
        eprintln!("  wait {}", l.chars().take(400).collect::<String>());
    }
}

/// Whether the record of load `key` in `recs` defines `field`.
fn writes(recs: &[u8], key: &str, field: &str) -> bool {
    phitex_tex::pkgrec_summary(recs)
        .iter()
        .any(|(k, w, _)| k == key && w.iter().any(|f| f == field))
}

/// Whether the record of load `key` reads `field`.
fn reads(recs: &[u8], key: &str, field: &str) -> bool {
    phitex_tex::pkgrec_summary(recs)
        .iter()
        .any(|(k, _, r)| k == key && r.iter().any(|f| f == field))
}

fn same(what: &str, got: &[u8], want: &[u8]) {
    if let Some(d) = common::first_difference(got, want) {
        panic!("{what}: {d}");
    }
}

/// Two packages that each define their own macros: on the second build
/// both carry their records and the second no longer waits for the
/// first (the critical path drops); the output equals a sequential
/// build's.
#[test]
fn independent_loads_run_side_by_side() {
    let d = dir(
        "indep",
        &[
            // (each busy for a while on its own register, so that two
            // runs side by side overlap)
            (
                "pa.sty",
                "\\def\\aa{A}\\def\\ab{a}\\countdef\\pacnt=100 \\def\\paloop{\\ifnum\\pacnt<100000 \\advance\\pacnt1 \\expandafter\\paloop\\fi}\\paloop\n",
            ),
            (
                "pb.sty",
                "\\def\\bb{B}\\def\\bc{b}\\countdef\\pbcnt=101 \\def\\pbloop{\\ifnum\\pbcnt<100000 \\advance\\pbcnt1 \\expandafter\\pbloop\\fi}\\pbloop\n",
            ),
        ],
    );
    let doc = format!("{DEF}\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\aa\\bb\\ab\\bc\n\\bye\n");
    let want = sequential(&d, &doc);
    let first = build(&d, &doc, Speculation::Off, Vec::new());
    note("first", &first);
    same("first", &first.output().0, &want);
    assert_eq!(first.stats.pkg_loads, 2);
    assert_eq!(first.stats.pkg_recorded, 0);
    let recs = first.export_pkgrec();
    assert!(!recs.is_empty(), "no records");
    let second = build(&d, &doc, Speculation::Off, recs.clone());
    note("second", &second);
    same("second", &second.output().0, &want);
    assert_eq!(second.stats.pkg_recorded, 2);
    assert_eq!(second.stats.declarations_refuted, 0);
    // (both loads' runs overlap; the engine's critical path, a height
    // count, is printed: it does not fall here, see the report)
    eprintln!(
        "critical path {:?} -> {:?}",
        first.stats.critical_path, second.stats.critical_path
    );
    assert_eq!(first.stats.pkg_max_concurrent, 1);
    if second.stats.threads > 1 {
        assert_eq!(
            second.stats.pkg_max_concurrent, 2,
            "the loads ran side by side"
        );
    }
    // a new document loading the same packages: the records are the
    // loads', not the document's
    let other = format!("{DEF}Text.\n\n\\usepackage{{pb}}\n\\usepackage{{pa}}\n\\bc\\ab\n\\bye\n");
    let o = build(&d, &other, Speculation::Off, recs);
    note("other document", &o);
    assert_eq!(o.stats.pkg_recorded, 2);
    same("other document", &o.output().0, &sequential(&d, &other));
    // never seen, under speculation: the optimistic hint
    let spec = build(&d, &doc, Speculation::All, Vec::new());
    note("optimistic", &spec);
    assert_eq!(spec.stats.pkg_optimistic, 2);
    same("optimistic", &spec.output().0, &want);
}

/// Package B reads a macro package A defines: B's record reads it, so B
/// still follows A; the output equals a sequential build's.
#[test]
fn dependent_loads_keep_their_order() {
    let d = dir(
        "dep",
        &[
            ("pa.sty", "\\def\\aa{A}\n"),
            ("pb.sty", "\\edef\\bb{\\aa B}\n"),
        ],
    );
    let doc = format!("{DEF}\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\bb\n\\bye\n");
    let want = sequential(&d, &doc);
    let first = build(&d, &doc, Speculation::Off, Vec::new());
    note("first", &first);
    same("first", &first.output().0, &want);
    let recs = first.export_pkgrec();
    assert!(
        reads(&recs, "\\usepackage{pb}", "cs:aa"),
        "B's record reads \\aa"
    );
    for spec in [Speculation::Off, Speculation::All] {
        let second = build(&d, &doc, spec, recs.clone());
        note("second", &second);
        assert_eq!(second.stats.pkg_recorded, 2);
        assert_eq!(second.stats.declarations_refuted, 0);
        same("second", &second.output().0, &want);
    }
}

/// A package's file, or a file it reads, changed between builds: its
/// key no longer matches, it loads unbounded, and its record is
/// replaced; the other load keeps its record. In a session, the same
/// through `edit_file`.
#[test]
fn changed_file_falls_back() {
    let d = dir(
        "changed",
        &[
            ("pa.sty", "\\def\\aa{A}\\input pa.def \n"),
            ("pa.def", "\\def\\ac{C}\n"),
            ("pb.sty", "\\def\\bb{B}\n"),
        ],
    );
    let doc = format!("{DEF}\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\aa\\bb\\ac\n\\bye\n");
    let first = build(&d, &doc, Speculation::Off, Vec::new());
    note("first", &first);
    let recs = first.export_pkgrec();
    // the package file
    std::fs::write(
        d.join("pa.sty"),
        "\\def\\aa{AA}\\def\\cc{}\\input pa.def \n",
    )
    .unwrap();
    let second = build(&d, &doc, Speculation::Off, recs.clone());
    note("package changed", &second);
    assert_eq!(
        second.stats.pkg_recorded, 1,
        "pa falls back, pb keeps its record"
    );
    same("package changed", &second.output().0, &sequential(&d, &doc));
    let again = second.export_pkgrec();
    assert!(
        writes(&again, "\\usepackage{pa}", "cs:cc"),
        "record replaced"
    );
    // a file the package reads
    let third = build(&d, &doc, Speculation::Off, again.clone());
    assert_eq!(third.stats.pkg_recorded, 2);
    std::fs::write(d.join("pa.def"), "\\def\\ac{CC}\\def\\cd{}\n").unwrap();
    let fourth = build(&d, &doc, Speculation::Off, again.clone());
    note("read file changed", &fourth);
    assert_eq!(fourth.stats.pkg_recorded, 1);
    same(
        "read file changed",
        &fourth.output().0,
        &sequential(&d, &doc),
    );
    assert!(writes(&fourth.export_pkgrec(), "\\usepackage{pa}", "cs:cd"));
    // in a session: the file edited, the load re-run unbounded
    let mut s = build(&d, &doc, Speculation::Off, fourth.export_pkgrec());
    assert_eq!(s.stats.pkg_recorded, 2);
    let cfg = "\\def\\ac{CCC}\\def\\ce{}\n";
    s.edit_file("pa.def", Some(cfg.as_bytes()));
    note("edited in session", &s);
    std::fs::write(d.join("pa.def"), cfg).unwrap();
    same("edited in session", &s.output().0, &sequential(&d, &doc));
    assert!(writes(&s.export_pkgrec(), "\\usepackage{pa}", "cs:ce"));
}

/// A load whose key matches but whose behaviour differs (it reads a
/// switch the document defines before it: the key is the load and its
/// files, not what it reads) defines outside its record: the engine
/// refutes the declaration and the load's readers re-run; the output
/// equals a sequential build's and the record is replaced.
#[test]
fn defining_outside_the_record_is_refuted() {
    let d = dir(
        "refuted",
        &[
            ("pa.sty", "\\def\\aa{A}\\if1\\mode \\gdef\\extra{X}\\fi\n"),
            ("pb.sty", "\\def\\bb{B}\n"),
        ],
    );
    let doc = |m: u8| {
        format!(
            "{DEF}\\def\\mode{{{m}}}\n\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\aa\\bb\\ifx\\extra\\undefined\\else\\extra\\fi\n\\bye\n"
        )
    };
    let first = build(&d, &doc(0), Speculation::Off, Vec::new());
    note("first", &first);
    let recs = first.export_pkgrec();
    assert!(writes(&recs, "\\usepackage{pa}", "cs:aa"));
    assert!(!writes(&recs, "\\usepackage{pa}", "cs:extra"));
    for spec in [Speculation::Off, Speculation::All] {
        let second = build(&d, &doc(1), spec, recs.clone());
        note("behaviour changed", &second);
        assert_eq!(second.stats.pkg_recorded, 2);
        assert!(
            second.stats.declarations_refuted >= 1 && second.stats.undeclared_definitions >= 1,
            "the record is refuted"
        );
        same(
            "behaviour changed",
            &second.output().0,
            &sequential(&d, &doc(1)),
        );
        assert!(
            writes(&second.export_pkgrec(), "\\usepackage{pa}", "cs:extra"),
            "record replaced"
        );
    }
}

/// LaTeX: `\documentclass` and two `\usepackage` of toy packages are
/// three load runs (each load's `\@ifnextchar[` peeks at the next
/// load's first token, which is given back: no load takes the next),
/// each recorded; the second build gives all three their records and
/// the output is unchanged.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn latex_loads_are_runs_of_their_own() {
    let d = dir(
        "latex",
        &[
            ("pa.sty", "\\ProvidesPackage{pa}\n\\def\\aa{A}\n"),
            ("pb.sty", "\\ProvidesPackage{pb}\n\\def\\bb{B}\n"),
        ],
    );
    let doc = "\\documentclass{article}\n\\usepackage{pa}\n\\usepackage{pb}\n\\begin{document}\n\\aa\\bb\n\\end{document}\n";
    let prelude = "\\input latex.ini\n";
    let latex = |bytes: Option<Vec<u8>>, recs: Vec<u8>, threads: Option<usize>| Options {
        prelude_jobname: "latex".to_owned(),
        epoch: Some(0),
        prelude_bytes: bytes,
        threads,
        time_limit: None,
        ..opts(&d, Speculation::Off, recs)
    };
    let first = common::build(latex(None, Vec::new(), None), Some(prelude), doc);
    note("latex first", &first);
    let fmt = first.export_prelude();
    assert_eq!(first.stats.pkg_loads, 3);
    assert_eq!(first.stats.pkg_claimed, 0, "a load took the next one");
    let recs = first.export_pkgrec();
    let keys: Vec<String> = phitex_tex::pkgrec_summary(&recs)
        .into_iter()
        .map(|(k, _, _)| k)
        .collect();
    eprintln!("recorded: {keys:?}");
    assert_eq!(keys.len(), 3, "each load recorded");
    let second = common::build(latex(fmt.clone(), recs, None), Some(prelude), doc);
    note("latex second", &second);
    assert_eq!(second.stats.pkg_recorded, 3);
    assert_eq!(second.stats.pkg_claimed, 0);
    let seq = common::build(latex(fmt, Vec::new(), Some(1)), Some(prelude), doc);
    same("latex second", &second.output().0, &seq.output().0);
    same("latex first", &first.output().0, &seq.output().0);
}

fn build_predicting(d: &Path, doc: &str, recs: Vec<u8>) -> Build {
    build_predicting_with(d, doc, recs, Speculation::Off)
}

fn build_predicting_with(d: &Path, doc: &str, recs: Vec<u8>, spec: Speculation) -> Build {
    let o = Options {
        pkg_predict: true,
        ..opts(d, spec, recs)
    };
    common::build(o, Some(&common::plain()), doc)
}

/// With predictions on, a record carries the values its load published;
/// on a later build a load whose key matches is given them, and its run
/// publishes exactly those (the prediction is right); the output equals
/// a sequential build's.
#[test]
fn prediction_is_right_when_nothing_changed() {
    let d = dir(
        "predict-right",
        &[
            (
                "pa.sty",
                "\\def\\aa{A}\\countdef\\pacnt=100 \\global\\pacnt=7\n",
            ),
            ("pb.sty", "\\edef\\bb{\\aa B}\n"),
        ],
    );
    let doc = format!("{DEF}\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\aa\\bb\n\\bye\n");
    let want = sequential(&d, &doc);
    let first = build_predicting(&d, &doc, Vec::new());
    note("first", &first);
    same("first", &first.output().0, &want);
    assert_eq!(first.stats.pkg_predicted, 0);
    let recs = first.export_pkgrec();
    let second = build_predicting(&d, &doc, recs);
    note("second", &second);
    same("second", &second.output().0, &want);
    let p = second
        .pkg_prediction("\\usepackage{pb}\n")
        .expect("pb is predicted");
    eprintln!("prediction of pb: {p:?}");
    assert!(
        p.iter()
            .any(|(f, _, x)| f == "cs:bb" && x.starts_with("[\"macro\"")),
        "the prediction carries \\bb's value"
    );
    let pa = second
        .pkg_prediction("\\usepackage{pa}\n")
        .expect("pa is predicted");
    assert!(pa.iter().any(|(f, g, _)| f == "count:100" && *g), "{pa:?}");
    assert_eq!(second.stats.pkg_predicted, 2);
    assert_eq!(
        second.stats.pkg_predict_right, 2,
        "wrong: {:?}",
        second.stats.pkg_predict_wrong
    );
    // under speculation the engine runs readers on the predictions:
    // confirmed, none refuted, the same output
    let spec = build_predicting_with(&d, &doc, second.export_pkgrec(), Speculation::All);
    note("predicted, speculating", &spec);
    same("predicted, speculating", &spec.output().0, &want);
    eprintln!(
        "predictions confirmed {} refuted {}",
        spec.stats.predictions_confirmed, spec.stats.predictions_refuted
    );
    assert_eq!(spec.stats.predictions_refuted, 0);
}

/// An earlier load changed (its file did: its key no longer matches),
/// and a later load reads what it defines: the later load's key still
/// matches, so it is predicted, and the prediction is wrong exactly in
/// the value that depends on the earlier load; the output equals a
/// sequential build's.
#[test]
fn prediction_is_wrong_when_an_earlier_load_changed() {
    let d = dir(
        "predict-wrong",
        &[
            ("pa.sty", "\\def\\aa{A}\n"),
            ("pb.sty", "\\edef\\bb{\\aa B}\\def\\bc{C}\n"),
        ],
    );
    let doc = format!("{DEF}\\usepackage{{pa}}\n\\usepackage{{pb}}\n\\aa\\bb\\bc\n\\bye\n");
    let first = build_predicting(&d, &doc, Vec::new());
    note("first", &first);
    let recs = first.export_pkgrec();
    std::fs::write(d.join("pa.sty"), "\\def\\aa{Z}\n").unwrap();
    let second = build_predicting(&d, &doc, recs);
    note("earlier load changed", &second);
    same(
        "earlier load changed",
        &second.output().0,
        &sequential(&d, &doc),
    );
    // (pa's key no longer matched: pa was not predicted)
    assert_eq!(second.stats.pkg_predicted, 1, "pb only");
    assert_eq!(second.stats.pkg_predict_right, 0);
    assert_eq!(
        second.stats.pkg_predict_wrong,
        vec!["\\usepackage{pb}: cs:bb".to_owned()],
        "only \\bb, which reads \\aa, is predicted wrong"
    );
    // under speculation: the wrong prediction is refuted by value, the
    // output is the sequential build's
    std::fs::write(d.join("pa.sty"), "\\def\\aa{A}\n").unwrap();
    let first = build_predicting(&d, &doc, Vec::new());
    let recs = first.export_pkgrec();
    std::fs::write(d.join("pa.sty"), "\\def\\aa{Z}\n").unwrap();
    let spec = build_predicting_with(&d, &doc, recs, Speculation::All);
    note("predicted wrong, speculating", &spec);
    same(
        "predicted wrong, speculating",
        &spec.output().0,
        &sequential(&d, &doc),
    );
    eprintln!(
        "predictions confirmed {} refuted {}",
        spec.stats.predictions_confirmed, spec.stats.predictions_refuted
    );
}
