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
    // (`[j]`, or `[j.c1...]` when `\count1` and the others are set)
    for j in 1..=n {
        assert!(
            log.contains(&format!("[{j}]")) || log.contains(&format!("[{j}.")),
            "page {j} not in the log:\n{}",
            common::log_excerpt(&log)
        );
    }
    eprintln!(
        "{n} pages; {} nodes run, {} runs, {} rounds, critical path {}, stops {} resumed {} restarted {}",
        b.stats.nodes_run,
        b.stats.runs,
        b.stats.rounds,
        b.stats.critical_path.unwrap_or(0),
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
