//! The pages as nodes (`crate::pages`): a document's pages finish where
//! TeX's do, shipped from finishers the pieces place; an edit that moves
//! no page break keeps the earlier pages.

mod common;

/// The number of pages a DVI file's postamble records.
fn dvi_total_pages(dvi: &[u8]) -> usize {
    let mut end = dvi.len();
    while end > 0 && dvi[end - 1] == 223 {
        end -= 1;
    }
    // post_post: 249, q[4], i[1]
    assert!(end >= 6 && dvi[end - 6] == 249, "no post_post");
    let q = u32::from_be_bytes([dvi[end - 5], dvi[end - 4], dvi[end - 3], dvi[end - 2]]) as usize;
    assert_eq!(dvi[q], 248, "no post at {q}");
    // post: 248, p[4], num[4], den[4], mag[4], l[4], u[4], s[2], t[2]
    let t = q + 1 + 4 * 6 + 2;
    usize::from(u16::from_be_bytes([dvi[t], dvi[t + 1]]))
}

#[test]
fn vsize_document_pages() {
    let dir = common::fixtures().join("plain/vsize");
    let src = common::read(&dir.join("vs.tex"));
    let want = std::fs::read(dir.join("vs.dvi")).expect("the oracle");
    let opts = common::options("./vs.tex", "vs", &common::dvi_comment(&want), &dir);
    let b = common::build(opts, Some(&common::plain()), &src);
    let (_, log) = b.output();
    let n = dvi_total_pages(&want);
    let diags: Vec<String> = b
        .diagnostics()
        .iter()
        .map(|d| format!("{}:{} {}", d.file, d.line, d.message))
        .collect();
    assert_eq!(
        b.pages().len(),
        n,
        "pages shipped (TeX ships {n}); log tail:\n{}\ndiagnostics:\n{}",
        common::log_excerpt(&log),
        diags.join("\n")
    );
    for (n, op, why) in &b.stats.reruns {
        eprintln!("  rerun {op} {n}: {why} [{}]", b.describe_node(*n));
    }
    // a node runs once: a piece waiting for the page state parks at its
    // fold and resumes there (docs/TARGET_GRAPH.md, Heights)
    assert!(
        b.stats.runs * 10 <= b.stats.nodes_run * 11,
        "{} runs for {} nodes (> 1.1 a node)",
        b.stats.runs,
        b.stats.nodes_run
    );
    // (`[j]`, or `[j.c1...]` when `\count1` and the others are set)
    for j in 1..=n {
        assert!(
            log.contains(&format!("[{j}]")) || log.contains(&format!("[{j}.")),
            "page {j} not in the log:\n{}",
            common::log_excerpt(&log)
        );
    }
    eprintln!(
        "{n} pages; {} nodes run, {} runs, {} rounds, critical path {} nodes / realised {} nodes {:?}, wall {:?}, busy {:?}, peak {} runs at once, stops {} resumed {} restarted {}",
        b.stats.nodes_run,
        b.stats.runs,
        b.stats.rounds,
        b.stats.critical_path.unwrap_or(0),
        b.stats.realised_critical_path,
        b.stats.realised_critical_time,
        b.stats.wall,
        b.stats.busy,
        b.stats.peak_running,
        b.stats.stops,
        b.stats.resumed,
        b.stats.restarted
    );
}

#[test]
fn an_edit_on_the_last_page_keeps_the_earlier_pages() {
    let (src, _) = common::plain_paragraphs(40, false);
    let dir = common::fixtures().join("plain/vsize");
    let opts = || common::options("./pp.tex", "pp", "", &dir);
    let mut b = common::build(opts(), Some(&common::plain()), &src);
    let pages = b.pages();
    assert!(pages.len() >= 3, "{} pages for 40 paragraphs", pages.len());
    let runs_cold = b.stats.runs;
    // a word of the last paragraph, same length: no break moves
    let at = src
        .rfind("Paragraph 39 has some")
        .expect("the last paragraph")
        + 17;
    let new = format!("{}sah{}", &src[..at], &src[at + 3..]);
    b.edit(&new);
    let after = b.pages();
    assert_eq!(pages.len(), after.len(), "the edit changed the page count");
    assert_eq!(
        pages[..pages.len() - 1],
        after[..after.len() - 1],
        "an earlier page changed"
    );
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert_eq!(cold.pages(), after, "edit != cold");
    eprintln!(
        "{} pages; cold {runs_cold} runs, edit {} runs",
        pages.len(),
        b.stats.runs
    );
}

/// (measure) 400 paragraphs: the stats, and with `PHITEX_TEX_TIME` the
/// page folds' cost (`pagefold:` lines) beside the runs' (`nodetime:`).
#[test]
#[ignore = "a measure"]
fn fold_cost() {
    let (src, _) = common::plain_paragraphs(400, false);
    let dir = common::fixtures().join("plain/vsize");
    let b = common::build(
        common::options("./pp.tex", "pp", "", &dir),
        Some(&common::plain()),
        &src,
    );
    eprintln!(
        "{} pages; {} nodes run, {} runs, realised cp {} nodes {:?}, wall {:?}, busy {:?}, peak {}, stops {} restarted {}",
        b.pages().len(),
        b.stats.nodes_run,
        b.stats.runs,
        b.stats.realised_critical_path,
        b.stats.realised_critical_time,
        b.stats.wall,
        b.stats.busy,
        b.stats.peak_running,
        b.stats.stops,
        b.stats.restarted
    );
}

/// A long document's pages at 8 threads equal those at one thread
/// (`PHITEX_PARAS` paragraphs, 1500 by default).
#[test]
#[ignore = "a long build"]
fn parallel_pages_equal_serial() {
    let n = std::env::var("PHITEX_PARAS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1500);
    let (src, _) = common::plain_paragraphs(n, false);
    // (`PHITEX_PDF=1`: the PDF file instead, relaxed objects unless
    // `PHITEX_EXACT=1`)
    let pdf = std::env::var("PHITEX_PDF").is_ok();
    let src = if pdf {
        format!("\\pdfoutput=1 {src}")
    } else {
        src
    };
    let dir = common::fixtures().join("plain/vsize");
    let at = |t: usize| {
        let mut o = common::options("./pp.tex", "pp", "", &dir);
        o.threads = Some(t);
        if pdf {
            let b = common::build(o, Some(&common::plain()), &src);
            let out = b.output().0;
            eprintln!(
                "{t} threads: {} bytes, {} runs for {} nodes",
                out.len(),
                b.stats.runs,
                b.stats.nodes_run
            );
            for d in b.diagnostics().iter().take(4) {
                eprintln!(
                    "  diagnostic {}:{} {}",
                    d.file,
                    d.line,
                    d.message.chars().take(300).collect::<String>()
                );
            }
            vec![phitex_tex_hash(&out)]
        } else {
            common::build(o, Some(&common::plain()), &src).pages()
        }
    };
    let one = at(1);
    for rep in 0..3 {
        let many = at(8);
        assert_eq!(one.len(), many.len(), "rep {rep}: page count");
        if let Some(j) = (0..one.len()).find(|&j| one[j] != many[j]) {
            panic!(
                "rep {rep}: page {} of {} differs at 8 threads",
                j + 1,
                one.len()
            );
        }
    }
}

fn phitex_tex_hash(b: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// A run that reads on across a page break (two groups of its own
/// open over lines set as paragraphs: the run claims the following
/// texts) is cut at the break, its resume claiming the rest; an edit
/// before it that moves the break out of the claimed span makes the run
/// claim them all itself, no cut. The output is the cold build's.
#[test]
fn an_edit_that_moves_a_break_out_of_a_claimed_span() {
    use std::fmt::Write as _;
    let (src, _) = common::plain_paragraphs(30, false);
    let at = |k: usize| src.find(&format!("Paragraph {k} has")).expect("paragraph");
    let mut block =
        String::from("\\csname begingroup\\endcsname\\csname begingroup\\endcsname\\obeylines\n");
    for j in 0..36 {
        let _ = writeln!(block, "Line {j} of the block, page \\the\\pageno");
        if j % 4 == 3 {
            block.push('\n');
        }
    }
    block.push_str("\\csname endgroup\\endcsname\\csname endgroup\\endcsname\n\n");
    let src = format!("{}{block}{}", &src[..at(12)], &src[at(12)..]);
    let dir = common::fixtures().join("plain/vsize");
    let opts = || common::options("./pp.tex", "pp", "", &dir);
    let mut bad = Vec::new();
    let block_at = src.find("\\csname begingroup").expect("the block");
    for k in 6..13 {
        let mut b = common::build(opts(), Some(&common::plain()), &src);
        // (12: right before the block)
        let at = if k == 12 {
            block_at
        } else {
            src.find(&format!("Paragraph {k} has")).expect("paragraph")
        };
        let new = format!("{}\\vfill\\eject {}", &src[..at], &src[at..]);
        b.edit(&new);
        let after = b.pages();
        let cold = common::build(opts(), Some(&common::plain()), &new);
        let errs: Vec<String> = b.diagnostics().iter().map(|d| d.message.clone()).collect();
        eprintln!(
            "eject before paragraph {k}: edit {} pages, cold {} pages, edit diagnostics {errs:?}",
            after.len(),
            cold.pages().len()
        );
        if cold.pages() != after {
            bad.push(k);
        }
    }
    assert!(
        bad.is_empty(),
        "edit != cold for the eject before paragraphs {bad:?}"
    );
}

/// The bytes do not hang on the finishers' predictions: para4000 at 8
/// threads with them off (the readers after a finisher wait for its run)
/// equals the one-thread build. Red while a block's scan step that reads
/// its page's field as unset and then places the continuation defining
/// it (before itself) is not checked again when that continuation's real
/// state arrives (the engine's stale reads, `PHITEX_CHECK_READS`).
#[test]
#[ignore = "red until the engine re-checks a read that nodes the reader placed before itself now answer"]
fn para4000_bytes_without_finisher_predictions() {
    let fx = common::fixtures().join("dvi");
    let bench =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/para4000.tex");
    let src = format!("\\pdfoutput=1 {}", common::read(&bench));
    let prelude = common::plain();
    let opts = |t: usize, pred: bool| phitex_tex::Options {
        threads: Some(t),
        finisher_predictions: pred,
        ..common::options("./para4000.tex", "para4000", "", &fx)
    };
    let want = common::build(opts(1, true), Some(&prelude), &src);
    for k in 0..3 {
        let got = common::build(opts(8, false), Some(&prelude), &src);
        let d = common::first_difference(&got.output().0, &want.output().0);
        if d.is_some()
            && let Ok(o) = std::env::var("PHITEX_GATE_OUT")
        {
            let _ = std::fs::write(format!("{o}/nopred-got.pdf"), got.output().0);
            let _ = std::fs::write(format!("{o}/nopred-want.pdf"), want.output().0);
            let _ = std::fs::write(format!("{o}/nopred-got.log"), got.output().1);
            let _ = std::fs::write(format!("{o}/nopred-want.log"), want.output().1);
        }
        assert!(
            d.is_none(),
            "run {k} at 8 threads without finisher predictions differs: {d:?}"
        );
    }
}
