//! The state model (docs/spec-state-model.md): every item its own field,
//! reads recorded per item used, writes published by one rule, page
//! layer values read as symbols, page-dependent macros predicted from
//! the page number. Plain TeX, with cold builds as the oracle.

mod common;

use phitex_tex::Options;

fn runs_of_report(r: &phitex_tex::Report, op: &str) -> u64 {
    r.runs_by_op.iter().find(|(o, _)| *o == op).map_or(0, |(_, n)| *n)
}

/// `n` paragraphs of four to five lines, with `mid` as a paragraph of
/// its own after the first `k`.
fn doc(n: usize, k: usize, mid: &str) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i == k {
            s.push_str(mid);
            s.push_str("\n\n");
        }
        for _ in 0..3 {
            s.push_str(&format!("Paragraph {i} of the document, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches. "));
        }
        s.push_str("\n\n");
    }
    s.push_str("\\bye\n");
    s
}

fn build8(name: &str, src: &str) -> phitex_tex::Build {
    let dir = common::scratch(&format!("state-{name}"));
    let opts = Options {
        threads: Some(8),
        ..common::options("./d.tex", "d", "", &dir)
    };
    common::build(opts, Some(&common::plain()), src)
}

fn cold(name: &str, src: &str) -> Vec<u8> {
    let dir = common::scratch(&format!("state-{name}-cold"));
    let b = common::build(common::options("./d.tex", "d", "", &dir), Some(&common::plain()), src);
    b.output().0
}

/// A paragraph sets the catcode of `@`; later paragraphs use `@` in a
/// name and other letters in words. Changing that catcode re-asks the
/// users of `@` and nobody else, and no table field exists.
#[test]
fn a_catcode_change_re_asks_only_the_readers_of_that_catcode() {
    let mid = "\\catcode`\\@=11 \\def\\a@b{A macro named with an at sign.} Paragraph that makes @ a letter.";
    let src = doc(120, 40, mid).replacen("Paragraph 80 of", "\\a@b\\ Paragraph 80 of", 1);
    let mut b = build8("cat", &src);
    assert_eq!(b.output().0, cold("cat", &src));
    assert_eq!(b.stats.refuted_by("cattab"), 0, "no table field is read: {:?}", b.stats.refuted_by_field);
    let edited = src.replacen("\\catcode`\\@=11 ", "\\catcode`\\@=11 \\catcode`\\_=11 ", 1);
    let r = common::rebuild(&mut b, &edited);
    assert_eq!(b.output().0, cold("cat-edited", &edited));
    assert!(runs_of_report(&r, "exec") + runs_of_report(&r, "resume") <= 2, "the setter alone, no reader of `_`: {r:?}");
}

/// The output routine sets a parameter to the value it already had
/// (`\parskip` as plain's `\output` leaves it): no reader is re-asked.
#[test]
fn a_parameter_set_by_the_output_routine_to_its_old_value_re_asks_nobody() {
    let src = format!("\\output={{\\parskip=0pt plus 1pt\\global\\setbox255=\\box255 \\shipout\\box255 \\advancepageno}}\n{}", doc(120, 60, "A paragraph in the middle."));
    let b = build8("parset", &src);
    assert_eq!(b.output().0, cold("parset", &src));
    assert_eq!(b.stats.refuted_by("param:parskip"), 0, "{:?}", b.stats.refuted_by_field);
    assert_eq!(b.definitions_by_op("finish", "param:parskip"), 0, "an equal write is no definition");
}

/// A global restore of a value the run read publishes nothing, and so
/// a page move does not relay through the restoring paragraph.
#[test]
fn a_global_restore_of_a_read_value_publishes_nothing() {
    let mid = "Paragraph that probes \\edef\\saved{\\count0=\\the\\count0\\relax}\\global\\count0=1 \\edef\\probe{\\number\\count0}\\global\\saved the page number and puts it back, then goes on for a while to make lines.";
    let src = doc(120, 60, mid);
    let b = build8("restore", &src);
    assert_eq!(b.output().0, cold("restore", &src));
    assert_eq!(b.definitions_by_op("exec", "count:0") + b.definitions_by_op("resume", "count:0"), 0);
    assert_eq!(b.stats.refuted_by("count:0"), 0, "{:?}", b.stats.refuted_by_field);
}

/// A local write inside a group the run closes publishes nothing; a
/// reader after the paragraph sees the value before the group.
#[test]
fn a_local_write_in_a_closed_group_publishes_nothing() {
    let mid = "{\\count5=7 \\def\\tmp{seven}Paragraph with a group that sets a register and a macro locally.}";
    let src = doc(120, 60, mid).replacen("Paragraph 100 of", "Register five is \\number\\count5. Paragraph 100 of", 1);
    let b = build8("closed", &src);
    assert_eq!(b.output().0, cold("closed", &src));
    assert_eq!(b.definitions_by_op("exec", "count:5"), 0);
    assert_eq!(b.definitions_by_op("exec", "cs:tmp"), 0);
}

/// A local write in a group the run leaves open (an environment
/// spanning paragraphs) is published at that depth and reaches the
/// paragraphs inside the group only.
#[test]
fn a_local_write_in_a_group_left_open_publishes_at_its_depth() {
    let src = doc(120, 40, "\\begingroup\\count5=7 Paragraph that opens a group and sets register five in it.")
        .replacen("Paragraph 60 of", "Inside the group register five is \\number\\count5. Paragraph 60 of", 1)
        .replacen("Paragraph 80 of", "\\endgroup Paragraph 80 of", 1)
        .replacen("Paragraph 100 of", "Outside the group register five is \\number\\count5. Paragraph 100 of", 1);
    let b = build8("open", &src);
    assert_eq!(b.output().0, cold("open", &src));
    assert_eq!(b.definitions_by_op("exec", "count:5"), 1, "one definition, local to the open group");
}

/// `\advance` of a register the paragraph never reads is a relation:
/// the next reader gets it without the paragraph defining a value.
#[test]
fn a_counter_step_without_a_read_is_a_relation() {
    let src = doc(120, 40, "\\global\\advance\\count7 by 1 Paragraph that steps register seven.")
        .replacen("Paragraph 80 of", "Register seven is \\number\\count7. Paragraph 80 of", 1);
    let b = build8("rel", &src);
    assert_eq!(b.output().0, cold("rel", &src));
    assert_eq!(b.definitions_by_op("exec", "count:7"), 0, "a relation, not a value");
    assert!(b.stats.relations_by_op("exec", "count:7") >= 1, "the step is filed as a relation");
}

/// A macro a finisher defines with the page number in it (a running
/// head's `\folio`-like macro) is predicted from the page number for
/// the next page, so its readers are not re-asked when the prediction is
/// confirmed.
#[test]
fn a_page_dependent_macro_is_predicted_from_the_page_number() {
    let src = format!("\\output={{\\xdef\\lastfolio{{page \\number\\count0}}\\global\\setbox255=\\box255 \\shipout\\box255 \\advancepageno}}\n{}", doc(160, 80, "The last folio was \\lastfolio. A paragraph in the middle."));
    let b = build8("template", &src);
    assert_eq!(b.output().0, cold("template", &src));
    assert!(b.stats.predictions_refuted <= b.pages().len() as u64, "a miss at most once per page, not once per macro: {}", b.stats.predictions_refuted);
}

/// A paragraph that uses three catcodes and two parameters records
/// reads of those five items and no others of their families.
#[test]
fn a_run_records_a_read_only_of_the_items_it_used() {
    let src = doc(20, 10, "Ab, a short paragraph.");
    let b = build8("used", &src);
    assert_eq!(b.output().0, cold("used", &src));
    let reads = b.reads_by_op("exec", "cat:");
    assert!(reads > 0 && reads < 40 * 20, "per item used, not per table: {reads}");
    assert_eq!(b.reads_by_op("exec", "cattab"), 0);
}

/// Every DVI fixture is byte-identical on the new model.
#[test]
fn the_state_fixtures_are_byte_identical() {
    let dir = common::fixtures().join("dvi");
    let mut names: Vec<_> = std::fs::read_dir(&dir).expect("fixtures").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "tex")).collect();
    names.sort();
    for tex in names {
        let name = tex.file_stem().unwrap().to_string_lossy().to_string();
        let Ok(want) = std::fs::read(dir.join(format!("{name}.dvi"))) else { continue };
        let src = common::read(&tex);
        let opts = common::options(&format!("./{name}.tex"), &name, &common::dvi_comment(&want), &dir);
        let b = common::build(opts, None, &src);
        let (dvi, log) = b.output();
        assert_eq!(dvi, want, "{name}: DVI differs; log tail:\n{}", common::log_excerpt(&log));
    }
}
