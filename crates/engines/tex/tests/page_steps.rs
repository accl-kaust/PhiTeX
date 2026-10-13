//! The light page steps (OUTPUT.md §4.2a, §4.7): `\vsize` set by the
//! output routine reaches the next page, and an edit that moves no page
//! break re-runs no later page step.

mod common;

fn vsize_doc() -> (std::path::PathBuf, String, Vec<u8>) {
    let dir = common::fixtures().join("plain/vsize");
    let src = common::read(&dir.join("vs.tex"));
    let want = std::fs::read(dir.join("vs.dvi")).expect("the oracle");
    (dir, src, want)
}

#[test]
fn vsize_from_the_output_routine() {
    let (dir, src, want) = vsize_doc();
    let opts = common::options("./vs.tex", "vs", &common::dvi_comment(&want), &dir);
    let b = common::build(opts, Some(&common::plain()), &src);
    if let Some(d) = common::first_difference(&b.output().0, &want) {
        panic!("vs.dvi: {d}");
    }
    assert!(b.stats.page_steps > 0, "no light page steps");
}

#[test]
fn an_edit_on_the_last_page_reruns_no_earlier_page() {
    let (dir, src, want) = vsize_doc();
    let comment = common::dvi_comment(&want);
    let opts = || common::options("./vs.tex", "vs", &comment, &dir);
    let mut b = common::build(opts(), Some(&common::plain()), &src);
    let pages = b.pages();
    let cold_steps = b.stats.page_steps;
    // a word of the last paragraph, same length: no break moves
    let at = src
        .rfind("Paragraph 39: the quick")
        .expect("the last paragraph")
        + 18;
    let new = format!("{}kciuq{}", &src[..at], &src[at + 5..]);
    b.edit(&new);
    let after = b.pages();
    assert_eq!(pages.len(), after.len());
    assert_eq!(pages[..pages.len() - 1], after[..after.len() - 1]);
    let cold = common::build(opts(), Some(&common::plain()), &new);
    assert!(
        common::first_difference(&b.output().0, &cold.output().0).is_none(),
        "edit != cold"
    );
    eprintln!(
        "page steps: cold {cold_steps}, after the edit {}",
        b.stats.page_steps
    );
    assert!(
        b.stats.page_steps * 4 <= cold_steps.max(4),
        "an edit at the end re-ran {} of {cold_steps} page steps",
        b.stats.page_steps
    );
}

/// The generated documents of `PHITEX_GEN` (a directory with
/// `para400.tex`, `para4000.tex`): loop-back rounds, the critical path
/// and the runs, cold and after a one-word edit in the middle
/// paragraph (same length: no break moves).
#[test]
#[ignore = "needs PHITEX_GEN"]
fn generated_documents() {
    let Ok(dir) = std::env::var("PHITEX_GEN") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for name in ["para400", "para4000"] {
        let Ok(src) = std::fs::read_to_string(dir.join(format!("{name}.tex"))) else {
            continue;
        };
        let opts = || common::options(&format!("./{name}.tex"), name, "", &dir);
        for t in [1, 4, 8] {
            let o = phitex_tex::Options {
                threads: Some(t),
                ..opts()
            };
            let b = common::build(o, Some(&common::plain()), &src);
            eprintln!(
                "{name} at {t} threads: wall {:?}, loop rounds {}, critical path {:?}, mispredictions {}",
                b.stats.wall, b.stats.loop_rounds, b.stats.critical_path, b.stats.mispredictions
            );
        }
        let mut b = common::build(opts(), Some(&common::plain()), &src);
        let s = &b.stats;
        eprintln!(
            "{name} cold: pages {}, loop rounds {}, critical path {:?}, runs {}, nodes run {}, page steps {}",
            b.pages().len(),
            s.loop_rounds,
            s.critical_path,
            s.runs,
            s.nodes_run,
            s.page_steps
        );
        {
            let mut by = std::collections::BTreeMap::<String, usize>::new();
            for (_, kind, field) in b.critical_chain() {
                *by.entry(format!("{kind} {field}")).or_default() += 1;
            }
            eprintln!("{name} chain by cause: {by:?}");
            eprintln!("{name} refuted guesses: {:?}", s.refuted_guesses);
        }
        let (r0, l0, p0) = (s.runs, s.loop_rounds, s.page_steps);
        let mid = src.len() / 2;
        let at = mid + src[mid..].find(" the ").expect("a word") + 1;
        let new = format!("{}eht{}", &src[..at], &src[at + 3..]);
        b.edit(&new);
        let s = &b.stats;
        eprintln!(
            "{name} edit: loop rounds {}, critical path {:?}, runs {} (cumulative {}), nodes run {}, page steps {} (cumulative {})",
            s.loop_rounds - l0,
            s.critical_path,
            s.runs - r0,
            s.runs,
            s.nodes_run,
            s.page_steps - p0,
            s.page_steps
        );
        if name == "para400" {
            let cold = common::build(opts(), Some(&common::plain()), &new);
            assert!(
                common::first_difference(&b.output().0, &cold.output().0).is_none(),
                "{name}: edit != cold"
            );
        }
    }
}

/// One cold build per run of `PHITEX_GEN_DOC` (in `PHITEX_GEN`) at
/// `PHITEX_GEN_THREADS` threads: its wall, critical path, runs and
/// loop rounds, for timings that alternate builds.
#[test]
#[ignore = "needs PHITEX_GEN"]
fn generated_wall() {
    let (Ok(dir), Ok(name)) = (std::env::var("PHITEX_GEN"), std::env::var("PHITEX_GEN_DOC")) else {
        return;
    };
    let t: usize = std::env::var("PHITEX_GEN_THREADS")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(1);
    let dir = std::path::PathBuf::from(dir);
    let src = std::fs::read_to_string(dir.join(format!("{name}.tex"))).expect("the document");
    let o = phitex_tex::Options {
        threads: Some(t),
        ..common::options(&format!("./{name}.tex"), &name, "", &dir)
    };
    let b = common::build(o, Some(&common::plain()), &src);
    eprintln!(
        "WALL {name} t={t} wall_ms={:.1} cp={:?} runs={} rounds={} steps={}",
        b.stats.wall.as_secs_f64() * 1000.0,
        b.stats.critical_path,
        b.stats.runs,
        b.stats.loop_rounds,
        b.stats.page_steps
    );
}
