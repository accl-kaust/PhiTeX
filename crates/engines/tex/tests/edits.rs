//! Gate 4: single-word edits on story.tex. After each edit the DVI file
//! equals a cold build of the same text, and the counters show how many
//! nodes the edit re-ran.

#![allow(clippy::many_single_char_names)]

mod common;

/// The byte ranges of the words (runs of ASCII letters not after `\`).
fn words(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic()
            && (i == 0 || b[i - 1] != b'\\' && !b[i - 1].is_ascii_alphabetic())
        {
            let s0 = i;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            if i - s0 >= 3 {
                out.push((s0, i));
            }
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn story_edits() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let want = std::fs::read(fx.join("story.dvi")).expect("story.dvi");
    let comment = common::dvi_comment(&want);
    let prelude = common::plain();
    let opts = || common::options("./story.tex", "story", &comment, &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    assert!(
        common::first_difference(&b.output().0, &want).is_none(),
        "cold story.dvi differs"
    );
    // the body: words after the first `\mark`
    let body = src.find("\\mark").unwrap_or(0);
    let ws: Vec<(usize, usize)> = words(&src).into_iter().filter(|&(s, _)| s > body).collect();
    let n: usize = std::env::var("PHITEX_EDITS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(50);
    let cold_every: usize = std::env::var("PHITEX_EDITS_COLD")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(10);
    let mut text = src.clone();
    let mut failed = Vec::new();
    for k in 0..n {
        let (s, e) = ws[(k * 7) % ws.len()];
        // the word in `text` (edits keep word lengths)
        let w = &text[s..e];
        let mut r: Vec<u8> = w.as_bytes().to_vec();
        r.rotate_left(1);
        let new = format!(
            "{}{}{}",
            &text[..s],
            String::from_utf8_lossy(&r),
            &text[e..]
        );
        let t0 = std::time::Instant::now();
        b.edit(&new);
        let dt = t0.elapsed();
        let st = &b.stats;
        eprintln!(
            "edit {k}: {w:?} -> {:?}: {} nodes run, {} runs, {} cutoffs, {} created, {} freed, {} live, tails {}/{}, starved {}, {dt:?}",
            String::from_utf8_lossy(&r),
            st.nodes_run,
            st.runs,
            st.cutoffs,
            st.created,
            st.freed,
            st.live,
            st.tails,
            st.tail_siblings,
            st.starved
        );
        text = new;
        if cold_every > 0 && (k + 1) % cold_every == 0 {
            let cold = common::build(opts(), Some(&prelude), &text);
            if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
                eprintln!("  edit {k}: differs from cold: {d}");
                if let Ok(o) = std::env::var("PHITEX_GATE_OUT") {
                    let _ = std::fs::write(format!("{o}/edit{k}.dvi"), b.output().0);
                    let _ = std::fs::write(format!("{o}/cold{k}.dvi"), cold.output().0);
                    let _ = std::fs::write(format!("{o}/edit{k}.log"), b.output().1);
                }
                failed.push(k);
            } else {
                eprintln!("  edit {k}: equals cold");
            }
        }
    }
    assert!(failed.is_empty(), "edits differing from cold: {failed:?}");
}

/// Edits on the INITEX fixtures, each checked against a cold build.
#[test]
fn small_edits() {
    let fx = common::fixtures().join("dvi");
    for name in ["hbox", "disc", "accent", "fonts"] {
        let src = common::read(&fx.join(format!("{name}.tex")));
        let want = std::fs::read(fx.join(format!("{name}.dvi"))).expect("dvi");
        let comment = common::dvi_comment(&want);
        let opts = || common::options(&format!("./{name}.tex"), name, &comment, &fx);
        let mut b = common::build(opts(), None, &src);
        let ws = words(&src);
        let mut text = src.clone();
        for k in 0..6 {
            let (s, e) = ws[(k * 5 + 3) % ws.len()];
            let w = text[s..e].to_owned();
            let mut r: Vec<u8> = w.as_bytes().to_vec();
            r.rotate_left(1);
            text = format!(
                "{}{}{}",
                &text[..s],
                String::from_utf8_lossy(&r),
                &text[e..]
            );
            b.edit(&text);
            let cold = common::build(opts(), None, &text);
            let d = common::first_difference(&b.output().0, &cold.output().0);
            eprintln!(
                "{name} edit {k} {w:?}: {} nodes run: {}",
                b.stats.nodes_run,
                d.as_deref().unwrap_or("equals cold")
            );
        }
    }
}

/// One edit on a small INITEX text (`PHITEX_TINY_FROM` → `_TO`).
#[test]
fn tiny_edit() {
    let fx = common::fixtures().join("dvi");
    let src = "\\catcode`\\{=1 \\catcode`\\}=2\n\\font\\tenrm=cmr10 \\tenrm \\hsize=100pt \\vsize=100pt\n\\hbox to 50pt{a\\hskip 1pt plus 2pt b}\n\\end\n".to_owned();
    let from = std::env::var("PHITEX_TINY_FROM").unwrap_or_else(|_| "plus".into());
    let to = std::env::var("PHITEX_TINY_TO").unwrap_or_else(|_| "lusp".into());
    let opts = || common::options("./t.tex", "t", "", &fx);
    let mut b = common::build(opts(), None, &src);
    let text = src.replacen(&from, &to, 1);
    eprintln!("--- edit");
    b.edit(&text);
    eprintln!("--- cold");
    let cold = common::build(opts(), None, &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    eprintln!(
        "tiny: {} nodes run: {}",
        b.stats.nodes_run,
        d.as_deref().unwrap_or("equals cold")
    );
    eprintln!(
        "edited log: {}\ncold log: {}",
        b.output().1,
        cold.output().1
    );
}

/// An edit inside a mark the output routine reads.
#[test]
fn tiny_mark_edit() {
    let fx = common::fixtures().join("dvi");
    let src = "\\catcode`\\{=1 \\catcode`\\}=2\n\\font\\tenrm=cmr10 \\tenrm \\hsize=100pt \\vsize=100pt\n\\output={\\shipout\\vbox{\\hbox{\\firstmark}\\box255}}\n\\vskip 1pt\n\\mark{Galaxy}\\hbox{x}\n\\end\n".to_owned();
    let opts = || common::options("./t.tex", "t", "", &fx);
    let mut b = common::build(opts(), None, &src);
    let text = src.replacen("Galaxy", "alaxyG", 1);
    eprintln!("--- edit");
    b.edit(&text);
    eprintln!("--- cold");
    let cold = common::build(opts(), None, &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    eprintln!(
        "tiny mark: {} nodes run: {}",
        b.stats.nodes_run,
        d.as_deref().unwrap_or("equals cold")
    );
    assert!(d.is_none());
}

/// A file `\input` by the document changes: the build equals a cold
/// build with the new file; the finder's answers list the inputs and
/// the TFMs.
#[test]
fn edit_input_file() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("edit-input");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("part.tex"), "Some words in a part.\n").unwrap();
    let prelude = common::plain();
    let doc = "\\font\\x=cmr12 \\input part\nAnd the {\\x main} text.\\par\\end\n";
    let opts = || common::options("./main.tex", "main", "", &dir);
    let mut b = common::build(opts(), Some(&prelude), doc);
    eprintln!("edit_file: first build {} bytes", b.output().0.len());
    let reads = b.reads();
    assert!(
        reads.iter().any(|(n, hit)| n == "part.tex" && *hit),
        "{reads:?}"
    );
    assert!(
        reads.iter().any(|(n, _)| std::path::Path::new(n)
            .extension()
            .is_some_and(|e| e == "tfm")),
        "{reads:?}"
    );
    let new = "Other words, quite different, in the part.\n";
    b.edit_file("part.tex", Some(new.as_bytes()));
    std::fs::write(dir.join("part.tex"), new).unwrap();
    let cold = common::build(opts(), Some(&prelude), doc);
    if let Some(d) = common::first_difference(&b.output().0, &cold.output().0) {
        eprintln!(
            "edit_file: {} nodes run: {d}\nedited log: {}\ncold log: {}",
            b.stats.nodes_run,
            b.output().1,
            cold.output().1
        );
        for x in cold.diagnostics() {
            eprintln!("cold: {}: {}", x.line, x.message);
        }
        eprintln!("cold reads: {:?}", cold.reads());
    }
    assert_eq!(b.output().0, cold.output().0, "edit_file differs from cold");
}

/// The prelude's environment exported and given back builds the same.
#[test]
fn prelude_round_trip() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let prelude = common::plain();
    let opts = || common::options("./story.tex", "story", "", &fx);
    let a = common::build(opts(), Some(&prelude), &src);
    let bytes = a.export_prelude().expect("the prelude finished");
    let mut o = opts();
    o.prelude_bytes = Some(bytes.clone());
    let b = common::build(o, Some(&prelude), &src);
    assert_eq!(a.prelude_key(), b.prelude_key());
    assert_eq!(b.export_prelude(), Some(bytes));
    assert_eq!(a.output().0, b.output().0);
}

/// A prelude whose evaluation ran out of time exports nothing.
#[test]
fn unfinished_prelude_is_not_exported() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let o = phitex_tex::Options {
        time_limit: Some(std::time::Duration::from_nanos(1)),
        ..common::options("./story.tex", "story", "", &fx)
    };
    // (a prelude text of its own: no finished one is cached under it)
    let prelude = format!("{}\n% unfinished\n", common::plain());
    let b = common::build(o, Some(&prelude), &src);
    assert!(!b.prelude_complete());
    assert!(b.export_prelude().is_none());
}

/// `Build::pages` and §642's log line on `pages.tex`: four pages, the
/// line pdfTeX's `tex` writes; an edit on the last page changes that
/// page's hash only.
#[test]
fn page_hashes_and_output_line() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("pages.tex"));
    let want = std::fs::read(fx.join("pages.dvi")).expect("pages.dvi");
    let comment = common::dvi_comment(&want);
    let prelude = common::plain();
    let opts = || common::options("./pages.tex", "pages", &comment, &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    let (dvi, log) = b.output();
    assert_eq!(dvi, want);
    let before = b.pages();
    assert_eq!(before.len(), 4);
    let line = format!(
        "Output written on pages.dvi (4 pages, {} bytes).",
        want.len()
    );
    assert_eq!(log.trim_end().rsplit('\n').next(), Some(line.as_str()));
    let new = src.replace("Fourteen.", "Fourteem.");
    assert_ne!(new, src);
    b.edit(&new);
    let after = b.pages();
    assert_eq!(after.len(), 4);
    let changed: Vec<usize> = (0..4).filter(|&i| before[i] != after[i]).collect();
    assert_eq!(changed, vec![3], "pages whose hash changed");
    let cold = common::build(opts(), Some(&prelude), &new);
    assert_eq!(cold.pages(), after);
}

/// An edit costs what a cold build of the same text costs (a cold build
/// is an edit from nothing): `story` with `\pdfoutput=1` under the
/// `pdfetex.ini` prelude, then the text grown to three pages with marks
/// and footnotes (`target/gate/{orig,big}.tex`, from the CLI's repro).
#[test]
#[ignore = "heavy: the cost of an edit against a cold build (counters with PHITEX_TEX_TIME)"]
fn edit_costs_a_cold_build() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate");
    let orig = common::read(&dir.join("orig.tex"));
    let big = common::read(&dir.join("big.tex"));
    let opts = || phitex_tex::Options {
        epoch: Some(0),
        prelude_jobname: "pdftex".to_owned(),
        ..common::options("./story.tex", "story", "", &dir)
    };
    let pre = "\\input pdfetex.ini\n";
    let show = |what: &str, t: std::time::Duration, s: &phitex_tex::Stats| {
        eprintln!("== {what} {t:?}: {s:?}");
    };
    eprintln!("== cold big");
    let t0 = std::time::Instant::now();
    let cold = common::build(opts(), Some(pre), &big);
    show("cold big", t0.elapsed(), &cold.stats);
    eprintln!("== cold orig");
    let t0 = std::time::Instant::now();
    let mut b = common::build(opts(), Some(pre), &orig);
    show("cold orig", t0.elapsed(), &b.stats);
    eprintln!("== edit orig -> big");
    let t0 = std::time::Instant::now();
    b.edit(&big);
    show("edit", t0.elapsed(), &b.stats);
    assert_eq!(b.output().0, cold.output().0);
}

/// Twenty word edits: the nodes each re-runs as a fraction of a cold
/// build's, p50 and p99, the output equal to cold after every edit (the
/// trace replay's bar: p99 <= 2.4%).
fn edit_fractions(dir: &std::path::Path, name: &str) {
    let src = common::read(&dir.join(format!("{name}.tex")));
    let prelude = common::plain();
    let opts = || common::options(&format!("./{name}.tex"), name, "", dir);
    let mut b = common::build(opts(), Some(&prelude), &src);
    let cold_nodes = b.stats.nodes_run.max(1);
    let body = src
        .find("\\mark")
        .or_else(|| src.find("\\begin"))
        .unwrap_or(0);
    let ws: Vec<(usize, usize)> = words(&src).into_iter().filter(|&(s, _)| s > body).collect();
    let mut text = src.clone();
    let mut fr = Vec::new();
    let mut failed = Vec::new();
    for k in 0..20 {
        let (s, e) = ws[(k * 7) % ws.len()];
        let mut r: Vec<u8> = text.as_bytes()[s..e].to_vec();
        r.rotate_left(1);
        text = format!(
            "{}{}{}",
            &text[..s],
            String::from_utf8_lossy(&r),
            &text[e..]
        );
        b.edit(&text);
        #[allow(clippy::cast_precision_loss)]
        fr.push(b.stats.nodes_run as f64 / cold_nodes as f64);
        let cold = common::build(opts(), Some(&prelude), &text);
        if common::first_difference(&b.output().0, &cold.output().0).is_some() {
            failed.push(k);
        }
    }
    fr.sort_by(f64::total_cmp);
    let at = |q: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let i = ((fr.len() - 1) as f64 * q).round() as usize;
        fr[i]
    };
    eprintln!(
        "{name}: edits re-run p50 {:.4} p99 {:.4} of {cold_nodes} nodes (bar p99 <= 0.024)",
        at(0.5),
        at(0.99)
    );
    assert!(
        failed.is_empty(),
        "{name}: edits differing from cold: {failed:?}"
    );
}

#[test]
#[ignore = "a measure: the edit gate"]
fn edit_gate() {
    edit_fractions(&common::fixtures().join("plain"), "story");
    let kt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/tmp/kt");
    if kt.join("s.tex").exists() {
        edit_fractions(&kt, "s");
    }
}

/// A file the job writes and reads back (`\jobname.aux`) is an
/// in-memory loop-back: the cold build converges in one evaluation
/// (the reference resolved, the file as the second pdfTeX run leaves
/// it), and an edit elsewhere equals a cold build with no loop round.
#[test]
fn edit_written_file() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("edit-written");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("main.aux"));
    let prelude = common::plain();
    let doc = "\\newread\\r \\openin\\r=\\jobname.aux \\ifeof\\r \\def\\ref{??}\\else\\closein\\r \\input \\jobname.aux \\fi\n\\newwrite\\w \\immediate\\openout\\w=\\jobname.aux \\immediate\\write\\w{\\relax}\n\nRef: \\ref.\\par\n\\write\\w{\\string\\def\\string\\ref{\\the\\count0}}\n\nSome text.\\par\n\\end\n";
    let opts = || phitex_tex::Options {
        fixpoint: true,
        ..common::options("./main.tex", "main", "", &dir)
    };
    let mut b = common::build(opts(), Some(&prelude), doc);
    let files = b.files();
    eprintln!(
        "written cold: {} loop rounds, {} evaluations, {} nodes run, files {files:?}\nlog: {}",
        b.stats.loop_rounds,
        b.stats.rounds,
        b.stats.nodes_run,
        b.output().1
    );
    let aux = files
        .iter()
        .find(|(n, _)| n == "main.aux")
        .map(|(_, t)| t.clone());
    assert_eq!(aux.as_deref(), Some("\\relax \n\\def\\ref{1}\n"));
    let text = doc.replacen("Some", "Other", 1);
    b.edit(&text);
    eprintln!(
        "written edit: {} loop rounds, {} evaluations, {} nodes run",
        b.stats.loop_rounds, b.stats.rounds, b.stats.nodes_run
    );
    let cold = common::build(opts(), Some(&prelude), &text);
    assert_eq!(b.output().0, cold.output().0, "edit differs from cold");
    assert_eq!(b.files(), cold.files());
}

/// Loop-back rounds, cold and after a one-word edit, on story, pages,
/// hyphens and e-TeX's marks fixture (measurement; prints only).
#[test]
#[ignore = "measurement"]
fn loop_rounds_measure() {
    let prelude = common::plain();
    let fx = common::fixtures().join("plain");
    for n in ["story", "pages", "hyphens"] {
        let src = common::read(&fx.join(format!("{n}.tex")));
        let opts = common::options(&format!("./{n}.tex"), n, "", &fx);
        let mut b = common::build(opts, Some(&prelude), &src);
        let cold = (b.stats.loop_rounds, b.stats.nodes_run);
        let i = src.len() / 2;
        let i = (i..src.len())
            .find(|&k| src.as_bytes()[k] == b'e')
            .unwrap_or(i);
        let mut new = src.clone();
        new.replace_range(i..=i, "a");
        b.edit(&new);
        eprintln!(
            "rounds {n}: cold {} loop rounds ({} nodes run), edit {} loop rounds ({} nodes run)",
            cold.0, cold.1, b.stats.loop_rounds, b.stats.nodes_run
        );
    }
    let dir = common::fixtures().join("etex");
    let pre = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";
    let src = format!("{pre}{}\\end\n", common::read(&dir.join("marks.tex")));
    let opts = common::options("./core.tex", "texput", "", &dir);
    let mut b = common::build(opts, None, &src);
    let cold = (b.stats.loop_rounds, b.stats.nodes_run);
    b.edit(&src.replacen("\\end\n", "\\relax\\end\n", 1));
    eprintln!(
        "rounds marks: cold {} loop rounds ({} nodes run), edit {} loop rounds ({} nodes run)",
        cold.0, cold.1, b.stats.loop_rounds, b.stats.nodes_run
    );
}

/// A read-back that never settles (the written value flips what the next
/// round reads): the build ends with the no-fixpoint error after
/// `max_rounds`, naming the written file, instead of running on.
#[test]
fn oscillating_written_file() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("oscillating-written");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("main.aux"));
    let prelude = common::plain();
    let doc = "\\newread\\r \\openin\\r=\\jobname.aux \\ifeof\\r \\def\\ref{??}\\else\\closein\\r \\input \\jobname.aux \\fi\n\\newwrite\\w \\immediate\\openout\\w=\\jobname.aux \\immediate\\write\\w{\\relax}\n\nRef: \\ref.\\par\n\\write\\w{\\string\\def\\string\\ref{\\if1\\ref 2\\else 1\\fi}}\n\n\\end\n";
    let opts = phitex_tex::Options {
        fixpoint: true,
        max_rounds: 8,
        ..common::options("./main.tex", "main", "", &dir)
    };
    let b = common::build(opts, Some(&prelude), doc);
    let msgs: Vec<String> = b.diagnostics().into_iter().map(|d| d.message).collect();
    eprintln!(
        "oscillating: {} loop rounds, {msgs:#?}",
        b.stats.loop_rounds
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("no fixpoint after 8 rounds") && m.contains("main.aux")),
        "{msgs:#?}"
    );
}

/// A one-word edit on a generated plain document of 40 paragraphs:
/// the edited region, its page's `brk`, the page step and the writer
/// re-run (the plan too: it folds the list), no loop round, and the PDF
/// equals a cold build of the edited text.
#[test]
fn one_word_edit_reruns_little() {
    let fx = common::fixtures().join("dvi");
    let words = [
        "galley", "proof", "margin", "printer", "ligature", "kern", "reader", "spacing",
    ];
    let mut src = String::from("\\pdfoutput=1 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n");
    for p in 0..40 {
        for w in 0..90 {
            src.push_str(words[(p * 7 + w * 3 + w / 5) % words.len()]);
            src.push(if w == 89 { '.' } else { ' ' });
        }
        src.push_str("\n\n");
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./w.tex", "w", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    assert!(b.is_pdf());
    let at = src.match_indices("\n\n").nth(37).expect("a paragraph").0 + 2;
    let text = format!("{}Q{}", &src[..at], &src[at + 1..]);
    b.edit(&text);
    let (runs, rounds) = (b.stats.nodes_run, b.stats.loop_rounds);
    let cold = common::build(opts(), Some(&prelude), &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert!(rounds <= 1, "{rounds} loop rounds");
    assert!(runs <= 5, "{runs} nodes run");
}

/// An edit on page 1 of 400 plain paragraphs in PDF mode re-runs that
/// page's step only: no page step reads another's writer state, so the
/// later pages' steps are not run again; the PDF equals a cold build.
#[test]
fn pdf_edit_reruns_one_page_step() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    let mut src = String::from("\\pdfoutput=1 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n");
    for k in 0..400 {
        let _ = writeln!(
            src,
            "Paragraph {k} has some words in it, enough to fill a few lines of text set \
             with plain's macros, so that the paragraph builder has work to do and the \
             pages fill up one after another.\n"
        );
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./p400.tex", "p400", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    assert!(b.is_pdf());
    let text = src.replacen(
        "Paragraph 3 has some words",
        "Paragraph 3 has some wordz",
        1,
    );
    b.edit(&text);
    let steps: u64 = b.stats.page_steps_completed.iter().sum();
    eprintln!(
        "p400 edit: {steps} page steps, {} nodes run, {} loop rounds, {} pages",
        b.stats.nodes_run,
        b.stats.loop_rounds,
        b.pages().len()
    );
    let cold = common::build(opts(), Some(&prelude), &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert!(steps <= 1, "{steps} page steps re-run");
}

/// A measure: 400 and 4000 plain paragraphs with `\pdfoutput=1`, cold
/// and after a one-word edit on page 1 (page steps re-run, wall).
#[test]
#[ignore = "a measure: exact PDF edits on long documents"]
fn pdf_para_edit_measure() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    for n in [400, 4000] {
        let mut src = String::from("\\pdfoutput=1 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n");
        for k in 0..n {
            let _ = writeln!(
                src,
                "Paragraph {k} has some words in it, enough to fill a few lines of text set \
                 with plain's macros, so that the paragraph builder has work to do and the \
                 pages fill up one after another.\n"
            );
        }
        src.push_str("\\bye\n");
        let prelude = common::plain();
        let opts = || phitex_tex::Options {
            threads: Some(1),
            ..common::options("./pm.tex", "pm", "", &fx)
        };
        let t = std::time::Instant::now();
        let mut b = common::build(opts(), Some(&prelude), &src);
        let cold = t.elapsed();
        let text = src.replacen(
            "Paragraph 3 has some words",
            "Paragraph 3 has some wordz",
            1,
        );
        let t = std::time::Instant::now();
        b.edit(&text);
        let edit = t.elapsed();
        let steps: u64 = b.stats.page_steps_completed.iter().sum();
        eprintln!(
            "pdf para{n}: {} pages, cold {cold:?} (critical path {:?}), edit {edit:?}: {steps} page steps, {} nodes run",
            b.pages().len(),
            b.stats.critical_path,
            b.stats.nodes_run
        );
    }
}
