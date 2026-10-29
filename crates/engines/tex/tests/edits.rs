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

/// A one-letter, same-length edit inside a paragraph of para4000 whose
/// lines come out the same size (`n` to `u` in a four-letter word: the
/// same width, height and depth, no kern or ligature, too short to
/// hyphenate), so the paragraph's vertical profile is unchanged: the edit
/// runs exactly that paragraph's exec and the resume piece after it, its
/// page's PDF side (which reads the page's lines; the page's finisher
/// reads their shape only, `crate::pages::seal`), the link and the end
/// node, once each, at 1 and at 8 threads. (A paragraph that begins a text unit is a cut at its start,
/// a page-builder candidate: its exec runs to it and the resume placed
/// after it reads the rest of the paragraph, so both read the edited
/// text.) No block scan, no node run twice, no speculative run dropped;
/// the PDF equals a cold build of the edited text. Every rerun's cause
/// is printed.
#[test]
fn one_word_edit_reruns_little() {
    let fx = common::fixtures().join("dvi");
    let bench =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/para4000.tex");
    let src = format!("\\pdfoutput=1 {}", common::read(&bench));
    // (paragraph 2003's first " line ": `line` to `liue`; no page fires in it)
    let par = src.match_indices("\n\n").nth(2003).expect("a paragraph").0;
    let at = par + src[par..].find(" line ").expect("a word `line`") + 3;
    let text = format!("{}u{}", &src[..at], &src[at + 1..]);
    assert_eq!(&src[at - 2..at + 2], "line");
    let prelude = common::plain();
    let opts = |t: usize| phitex_tex::Options {
        threads: Some(t),
        ..common::options("./para4000.tex", "para4000", "", &fx)
    };
    let cold = common::build(opts(8), Some(&prelude), &text);
    assert!(cold.is_pdf());
    for t in [1, 8] {
        let mut b = common::build(opts(t), Some(&prelude), &src);
        let before = b.pages();
        eprintln!("--- the edit at {t} threads");
        b.edit(&text);
        let st = &b.stats;
        eprintln!(
            "para4000 edit at {t} threads: runs by op {:?}, {} runs of {} nodes, pages touched {:?} of {}, speculations {} mispredicted {}, loop rounds {}",
            st.runs_by_op,
            st.runs,
            st.nodes_run,
            common::pages_touched(&before, &b.pages()),
            b.pages().len(),
            st.speculations,
            st.mispredictions,
            st.loop_rounds
        );
        for (n, op, why) in &st.reruns {
            eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
        }
        // (with PHITEX_RUN_LOG: every run attempt, in order)
        for (n, op, resumed, spec, end, round) in &st.run_log {
            eprintln!(
                "  run {op} {n}: resumed {resumed} speculative {spec} ended {} round {round}",
                char::from(*end)
            );
        }
        let d = common::first_difference(&b.output().0, &cold.output().0);
        assert!(d.is_none(), "edit at {t} threads differs from cold: {d:?}");
        let mut got: Vec<(&str, u64)> = st.runs_by_op.iter().copied().filter(|x| x.1 > 0).collect();
        got.sort_unstable();
        assert_eq!(
            got,
            [
                ("dvi", 1),
                ("exec", 1),
                ("link", 1),
                // (one node a paragraph: no rest after a cut at its
                // start)
                ("pdfpage", 1)
            ],
            "runs by op at {t} threads"
        );
        assert_eq!(st.runs, st.nodes_run, "a node ran twice at {t} threads");
        assert_eq!(st.mispredictions, 0, "speculative runs dropped at {t} threads");
    }
}

/// An edit on page 1 of 400 plain paragraphs in PDF mode re-runs that
/// page's step only: no page step reads another's writer state, so the
/// later pages' steps are not run again; the PDF equals a cold build.
/// What else re-runs is the node model's (`common::assert_edit_cost`).
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
    let before = b.pages();
    let text = src.replacen(
        "Paragraph 3 has some words",
        "Paragraph 3 has some wordz",
        1,
    );
    b.edit(&text);
    let steps: u64 = b.stats.page_steps;
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
    common::assert_edit_cost("p400", &b, 1, &before);
}

/// Exact PDF, a one-word edit on page k of n with read-time writer
/// commands (`\immediate\pdfobj`) on every page: the edited piece, its
/// page's finisher and that page's placement run, then the end node links
/// the chunks; no later page's placement and no later writer command runs
/// again (the writer state after the edited page holds no byte offset and
/// no used character, so it is the state the later steps read before). A
/// new character (`z`) on the page changes only the fonts' union at the
/// end. The PDF equals a cold build's.
#[test]
fn exact_pdf_edit_on_page_k_reruns_its_piece_finisher_place_and_the_link_only() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    let mut src = String::from(
        "\\pdfoutput=1 \\pdfcompresslevel=0 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n",
    );
    for k in 0..300 {
        let _ = writeln!(
            src,
            "Paragraph {k} has some words in it, enough to fill a few lines of text set \
             with plain's macros, so that the paragraph builder has work to do and the \
             pages fill up one after another.\n"
        );
        if k % 10 == 5 {
            let _ = writeln!(src, "\\immediate\\pdfobj{{<< /K {k} >>}}\n");
        }
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./pk.tex", "pk", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    assert!(b.is_pdf());
    let n = b.pages().len();
    let before = b.pages();
    let text = src.replacen(
        "Paragraph 150 has some words",
        "Paragraph 150 has some wordz",
        1,
    );
    b.edit(&text);
    let touched = common::pages_touched(&before, &b.pages());
    eprintln!(
        "page k edit: runs by op {:?}, pages touched {touched:?} of {n}",
        b.stats.runs_by_op
    );
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
    let cold = common::build(opts(), Some(&prelude), &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert_eq!(touched.len(), 1, "one page's record changes");
    assert!(
        touched[0] > 0 && touched[0] + 1 < n,
        "the edit is on a middle page"
    );
    assert_eq!(common::runs_of(&b, "exec"), 1, "only the edited piece");
    // (its page's finisher, and the page before's where that page fired
    // in the edited piece: a finisher reads its page's items up to the
    // piece it fired in, the page builder one node over the whole text;
    // the page equal, a cutoff)
    assert!(
        common::runs_of(&b, "finish") <= 2,
        "only its page's finisher and the page before's"
    );
    // (the placement's node may start once and stop at a pending
    // definition set before it, then run: two runs of the one node; no
    // other page's placement runs)
    assert!(
        common::runs_of(&b, "place") <= 2,
        "only its page's placement"
    );
    assert_eq!(common::runs_of(&b, "dvi"), 1, "one link");
}

/// Exact PDF, a link removed on page k of n where every paragraph
/// makes a link (`\pdfstartlink`): a link's object number is named only
/// by PDF bytes, so no later piece reads a count its loss moves; the
/// edited piece and its page's finisher run, no later piece, finisher or
/// continuation runs again, and the PDF equals a cold build's (the link
/// numbers the objects as pdfTeX does).
#[test]
fn a_link_removed_reruns_its_piece_and_page_only() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    let mut src = String::from(
        "\\pdfoutput=1 \\pdfcompresslevel=0 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n",
    );
    for k in 0..300 {
        let _ = writeln!(
            src,
            "Paragraph {k} has \\pdfstartlink user{{/S/URI/URI(u{k})}}a link\\pdfendlink{{}} in it, \
             and enough words to fill a few lines of text set with plain's macros, so \
             that the pages fill up one after another.\n"
        );
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./pl.tex", "pl", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    assert!(b.is_pdf());
    let n = b.pages().len();
    let text = src.replacen(
        "Paragraph 150 has \\pdfstartlink user{/S/URI/URI(u150)}a link\\pdfendlink{}",
        "Paragraph 150 has a link",
        1,
    );
    assert_ne!(text, src);
    b.edit(&text);
    eprintln!("link removed: runs by op {:?} ({n} pages)", b.stats.runs_by_op);
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
    let cold = common::build(opts(), Some(&prelude), &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert_eq!(common::runs_of(&b, "exec"), 1, "only the edited piece");
    // (a page fires at paragraph 150's start: its one piece lies in
    // both pages' ranges, and the earlier page's finisher reads the
    // piece's profile by range and runs again, its output unchanged. A
    // finisher reading only the part of a straddling piece it uses, its
    // range up to the candidate, would run once: an edits-phase item)
    assert!(common::runs_of(&b, "finish") <= 2, "only the finishers of the pages it is on");
    // (the edited piece's own continuation, where a page fires inside
    // it, and no other)
    assert!(common::runs_of(&b, "resume") <= 1, "no later continuation");
    // (every page placed as its own step wrote it: a page's link objects
    // are named past the objects of the writer it is written on, so none
    // is found scheduled there where the line's is not)
    for (what, x) in [("edit", &b), ("cold", &cold)] {
        let (placed, shipped) = x.link_pages();
        assert_eq!(shipped, 0, "{what}: pages shipped again by the link ({placed} placed)");
    }
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
        let steps: u64 = b.stats.page_steps;
        eprintln!(
            "pdf para{n}: {} pages, cold {cold:?} (critical path {:?}), edit {edit:?}: {steps} page steps, {} nodes run",
            b.pages().len(),
            b.stats.critical_path,
            b.stats.nodes_run
        );
    }
}

/// An edit that moves page breaks (a paragraph made two lines longer)
/// in a plain document whose sections end with `\vfill\eject`: the
/// page builder, one node over the whole text, runs again once (and
/// stops at most once for each piece run again), and its finishers
/// placed again run only where their pages changed: from the edited
/// piece's page to the first page whose break comes out where it was
/// (the section's `\eject`), and one more (the page that fired in the
/// edited piece reads its items to the candidate: a cutoff where its
/// page is equal). Bound: one finisher per touched page and one; the
/// output equals a cold build.
#[test]
fn moving_breaks_rescan_touched_pages_and_one() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    let mut src = String::from("\\hsize=4in \\vsize=3in \\parindent=20pt\n");
    for s in 0..12 {
        for p in 0..5 {
            let _ = writeln!(
                src,
                "Section {s} paragraph {p} has words enough to fill some lines of a narrow \
                 page, set with plain's macros, so that the page builder breaks the section \
                 over two pages and the eject ends it.\n"
            );
        }
        src.push_str("\\vfill\\eject\n");
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./mb.tex", "mb", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    let before = b.pages();
    let text = src.replacen(
        "Section 5 paragraph 1 has words",
        "Section 5 paragraph 1 has many more words than the others, many more words than \
         the others, many more words than the others, words",
        1,
    );
    b.edit(&text);
    let cold = common::build(opts(), Some(&prelude), &text);
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    let after = b.pages();
    let touched = common::pages_touched(&before, &after);
    eprintln!(
        "moving breaks: {} pages, touched {touched:?}, edit runs by op {:?}",
        after.len(),
        b.stats.runs_by_op
    );
    assert!(
        !touched.is_empty() && touched.len() <= 3,
        "pages touched {touched:?}"
    );
    let n = touched.len() as u64;
    let pages = common::runs_of(&b, "pages");
    let fins = common::runs_of(&b, "finish");
    // (one run of the one page builder node, and a stop at most for each
    // piece run again: the edited one and a neighbour)
    assert!(pages <= 3, "{pages} page builder runs, bound 3");
    assert!(fins <= n + 1, "{fins} finisher runs, bound {}", n + 1);
}

/// Blanks typed at the ends of lines, and taken away again, run no node:
/// TeX reads a line without them (web2c's `input_line`), and the
/// document's canonical text leaves them out; the output is the cold
/// build's of each text.
#[test]
fn blanks_at_line_ends_run_no_node() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let want = std::fs::read(fx.join("story.dvi")).expect("story.dvi");
    let comment = common::dvi_comment(&want);
    let prelude = common::plain();
    let opts = || common::options("./story.tex", "story", &comment, &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    let body = src.find("\\mark").unwrap_or(0);
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    // (one line's end, every line's end, a tab and spaces, and back)
    let one: String = {
        let mut at = 0;
        let mut out = String::new();
        let mut done = false;
        for l in &lines {
            at += l.len();
            if !done && at > body + 200 && l.len() > 2 {
                out.push_str(&l[..l.len() - 1]);
                out.push_str("  \n");
                done = true;
            } else {
                out.push_str(l);
            }
        }
        out
    };
    let all: String = lines
        .iter()
        .map(|l| l.strip_suffix('\n').map_or_else(|| format!("{l} \t "), |b| format!("{b} \t \n")))
        .collect();
    let mut last = src.clone();
    for (what, text) in [("one line", &one), ("every line", &all), ("none again", &src)] {
        assert_ne!(*text, last, "{what}: no edit");
        last.clone_from(text);
        b.edit(text);
        assert_eq!(b.stats.nodes_run, 0, "{what}: {} nodes run", b.stats.nodes_run);
        assert!(
            common::first_difference(&b.output().0, &want).is_none(),
            "{what}: the output is not the cold build's"
        );
    }
}

/// A line inserted at the top of a plain document whose paragraphs read
/// `\inputlineno` into a group's definition that nothing reads (a weak
/// read, taken back at the run's end) and one into the log (a strong
/// one): the line numbers come from where each piece's text is, so the
/// weak readers do not run again, the strong one does and logs the new
/// number, and the output and log are a cold build's.
#[test]
fn a_line_inserted_at_the_top_reruns_no_weak_reader_of_the_line_number() {
    use std::fmt::Write as _;
    let fx = common::fixtures().join("dvi");
    let mut src = String::from("\\vsize=200pt \\hsize=150pt \\parindent=0pt\n");
    for k in 0..12 {
        let _ = writeln!(src, "{{\\edef\\where{{\\the\\inputlineno}}}}Paragraph {k} has a few words.\n");
    }
    src.push_str("\\message{[at \\the\\inputlineno]}Last words.\n\\bye\n");
    let prelude = common::plain();
    let opts = || common::options("./ln.tex", "ln", "", &fx);
    let mut b = common::build(opts(), Some(&prelude), &src);
    let text = format!("\\relax\n{src}");
    b.edit(&text);
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
    let cold = common::build(opts(), Some(&prelude), &text);
    let (out, log) = b.output();
    let (cold_out, cold_log) = cold.output();
    assert!(common::first_difference(&out, &cold_out).is_none(), "the output is a cold build's");
    assert!(log.contains("[at 27]") && cold_log.contains("[at 27]"), "the log has the new line: {}", common::log_excerpt(&log));
    let weak = b
        .stats
        .reruns
        .iter()
        .filter(|(n, op, why)| *op == "exec" && !why.contains("Edited") && b.describe_node(*n).contains("\\where"))
        .count();
    assert_eq!(weak, 0, "no weak reader of the line runs again");
}

/// A long paragraph edited near its end: the first edit's run (from the
/// start) takes checkpoints, the next ones go on from one; an edit at its
/// start runs from the start (taking them again); each output is the
/// cold build's of its text.
#[test]
fn an_edited_paragraph_goes_on_from_a_checkpoint() {
    let fx = common::fixtures().join("plain");
    let prelude = common::plain();
    let opts = || common::options("./long.tex", "long", "", &fx);
    // (each word followed by a command: commands, as a real paragraph's
    // citations and acronyms make them)
    let words: Vec<String> = (0..700).map(|k| format!("w{}rd{}\\kern0pt", ["o", "a", "e"][k % 3], k % 17)).collect();
    let par = |ws: &[String]| format!("\\hsize=300pt\n{}\n\n\\bye\n", ws.join(" "));
    let mut ws = words.clone();
    let mut b = common::build(opts(), Some(&prelude), &par(&ws));
    for (k, at, resumes) in [(0, 650, false), (1, 680, true), (2, 3, false), (3, 690, true), (4, 695, true)] {
        ws[at] = format!("edit{k}\\kern0pt");
        let text = par(&ws);
        b.edit(&text);
        let cold = common::build(opts(), Some(&prelude), &text);
        assert!(
            common::first_difference(&b.output().0, &cold.output().0).is_none(),
            "edit {k} at word {at}: the output is not the cold build's"
        );
        assert_eq!(
            b.stats.resumed_at_mark > 0,
            resumes,
            "edit {k} at word {at}: {} runs went on from a checkpoint",
            b.stats.resumed_at_mark
        );
    }
}
