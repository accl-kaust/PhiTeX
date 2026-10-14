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
    let at = src.rfind("Paragraph 39 has some").expect("the last paragraph") + 17;
    let new = format!("{}sah{}", &src[..at], &src[at + 3..]);
    b.edit(&new);
    let after = b.pages();
    assert_eq!(pages.len(), after.len(), "the edit changed the page count");
    assert_eq!(pages[..pages.len() - 1], after[..after.len() - 1], "an earlier page changed");
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
    let b = common::build(common::options("./pp.tex", "pp", "", &dir), Some(&common::plain()), &src);
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
    let n = std::env::var("PHITEX_PARAS").ok().and_then(|s| s.parse().ok()).unwrap_or(1500);
    let (src, _) = common::plain_paragraphs(n, false);
    // (`PHITEX_PDF=1`: the PDF file instead, relaxed objects unless
    // `PHITEX_EXACT=1`)
    let pdf = std::env::var("PHITEX_PDF").is_ok();
    let src = if pdf { format!("\\pdfoutput=1 {src}") } else { src };
    let dir = common::fixtures().join("plain/vsize");
    let at = |t: usize| {
        let mut o = common::options("./pp.tex", "pp", "", &dir);
        o.threads = Some(t);
        if pdf {
            o.exact_objects = std::env::var("PHITEX_EXACT").is_ok();
            let b = common::build(o, Some(&common::plain()), &src);
            let out = b.output().0;
            eprintln!("{t} threads: {} bytes, {} runs for {} nodes", out.len(), b.stats.runs, b.stats.nodes_run);
            for d in b.diagnostics().iter().take(4) {
                eprintln!("  diagnostic {}:{} {}", d.file, d.line, d.message.chars().take(300).collect::<String>());
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
            panic!("rep {rep}: page {} of {} differs at 8 threads", j + 1, one.len());
        }
    }
}

fn phitex_tex_hash(b: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}
