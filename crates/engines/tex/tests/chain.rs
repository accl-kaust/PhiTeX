//! The critical chain of plain documents, classified by cause: the
//! measure for the chain work (`PHITEX_CHAIN=1` prints every link).

mod common;

use std::collections::BTreeMap;

/// The target graph's forbidden links (`docs/TARGET_GRAPH.md`) and its
/// height bound, for one build: the failures found.
fn target_failures(name: &str, b: &phitex_tex::Build) -> Vec<String> {
    let mut out = Vec::new();
    // no paragraph cell, no condition-level cell: a region's paragraph
    // and conditionals are its run's own (docs/TARGET_GRAPH.md)
    for (what, kind, field) in b.critical_chain() {
        if field.starts_with("par:") || field.starts_with("if:") {
            out.push(format!("{name}: a link through {field} ({kind}: {what})"));
        }
    }
    let pages = b.pages().len();
    // the page chain is the design (docs/TARGET_GRAPH.md, Heights): no
    // bound on its height in nodes; a node runs once (a park resumes)
    let (runs, nodes) = (b.stats.runs, b.stats.nodes_run);
    if runs * 10 > nodes * 11 {
        out.push(format!("{name}: {runs} runs for {nodes} nodes (> 1.1 a node)"));
    }
    // a region runs as one node: at most a few nodes per region (the
    // region, its breaks, its loop-back) plus one step per page
    let regions = b.stats.nodes_text_frames;
    let pages = pages as u64;
    let run = b.stats.nodes_run;
    eprintln!("{name}: {run} nodes run, {regions} regions (text nodes), {pages} pages");
    if run > 3 * regions + pages + 8 {
        out.push(format!(
            "{name}: {run} nodes run > 3 * {regions} regions + {pages} pages + 8"
        ));
    }
    out
}

#[allow(clippy::too_many_lines)]
fn chain(dir: &std::path::Path, name: &str) -> Vec<String> {
    let src = common::read(&dir.join(format!("{name}.tex")));
    let opts = common::options(&format!("./{name}.tex"), name, "", dir);
    let b = common::build(opts, Some(&common::plain()), &src);
    {
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../../target/gate/chain_{name}.txt"));
        let _ = std::fs::write(out, b.critical_chain_lines().join("\n") + "\n");
    }
    if matches!(name, "story" | "s") {
        for t in [1, 4] {
            let opts = phitex_tex::Options {
                threads: Some(t),
                ..common::options(&format!("./{name}.tex"), name, "", dir)
            };
            let b = common::build(opts, Some(&common::plain()), &src);
            eprintln!(
                "{name} at {t} threads: wall {:?}, busy {:?}, busy/wall {:.2}, runs {}, stops {}, raises {}, speculations {}/{}/{}, waits_by_kind {:?}",
                b.stats.wall,
                b.stats.busy,
                b.stats.busy.as_secs_f64() / b.stats.wall.as_secs_f64().max(1e-9),
                b.stats.runs,
                b.stats.stops,
                b.stats.raises,
                b.stats.speculations,
                b.stats.confirmations,
                b.stats.mispredictions,
                b.stats.waits_by_kind
            );
        }
    }
    {
        let s = &b.stats;
        eprintln!(
            "{name}: {} token-frame nodes, {} text nodes; waits_by_kind {:?}, hints {} bounded / {} any / {} demoted / {} violations / {} undeclared, speculations {}, raises {}, stops {}, pending sets {} ({} defining)",
            s.nodes_token_frames,
            s.nodes_text_frames,
            s.waits_by_kind,
            s.nodes_bounded,
            s.nodes_any,
            s.hints_demoted,
            s.regime_violations,
            s.undeclared_definitions,
            s.speculations,
            s.raises,
            s.stops,
            s.pending_sets,
            s.pending_sets_defining
        );
        for (k, l) in b.chain_waits().iter().enumerate() {
            eprintln!("  {name} wait {k}: {l}");
        }
    }
    // (an evaluation that stopped early measures nothing)
    if b.stats.page_steps == 0 {
        let d: Vec<String> = b
            .diagnostics()
            .iter()
            .map(|d| d.message.chars().take(600).collect())
            .collect();
        return vec![format!(
            "{name}: no page steps ({} nodes run): {d:?}",
            b.stats.nodes_run
        )];
    }
    let c = b.critical_chain();
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for (what, kind, field) in &c {
        let key = if field.is_empty() {
            kind.clone()
        } else {
            let f = field.split(':').next().unwrap_or("");
            format!("{kind} {f}:")
        };
        *by.entry(key).or_default() += 1;
        if std::env::var_os("PHITEX_CHAIN").is_some() {
            eprintln!("  {kind:9} {field:24} {what}");
        }
    }
    eprintln!(
        "{name}: {} nodes run, {} runs, critical path {:?}, realised cp {}, peak running {}, ready mean {:.2}, page steps {}, chain {} links: {by:?}",
        b.stats.nodes_run,
        b.stats.runs,
        b.stats.critical_path,
        b.stats.realised_critical_path,
        b.stats.peak_running,
        b.stats.ready_mean(),
        b.stats.page_steps,
        c.len()
    );
    // the bar: the trace replay's critical path is 0.04-0.3% of the nodes
    #[allow(clippy::cast_precision_loss)]
    let frac = b
        .stats
        .critical_path
        .map(|p| p as f64 / b.stats.nodes_run.max(1) as f64);
    eprintln!(
        "{name}: oracle_fraction {} (critical path {:?} / {} nodes run; bar 0.0004-0.003), paragraph boundary violations {}, if depth violations {}",
        frac.map_or_else(|| "n/a".to_owned(), |f| format!("{f:.4}")),
        b.stats.critical_path,
        b.stats.nodes_run,
        b.stats.par_boundary_violations,
        b.stats.if_depth_violations
    );
    eprintln!("{name}: loop rounds {}", b.stats.loop_rounds);
    let fails = target_failures(name, &b);
    eprintln!(
        "{name}: height {:?}, {} token-frame nodes, {} target-graph failures",
        b.stats.critical_path,
        b.stats.nodes_token_frames,
        fails.len()
    );
    for f in &fails {
        eprintln!("  {f}");
    }
    fails
}

/// The target graph's gate (docs/TARGET_GRAPH.md, The test), printing
/// the critical chain by cause.
#[test]
fn chains() {
    let fx = common::fixtures().join("plain");
    let mut fails = Vec::new();
    for n in ["story", "pages", "hyphens", "display"] {
        fails.extend(chain(&fx, n));
    }
    let kt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/kt");
    if kt.join("s.tex").exists() {
        fails.extend(chain(&kt, "s"));
    }
    assert!(fails.is_empty(), "target graph: {} failures", fails.len());
}

/// Speculation off, on predicted nodes, on all candidates, at 1 and 4
/// threads: the output the same, and what the engine did.
#[test]
#[ignore = "a measure: speculation A/B"]
fn speculation_ab() {
    use phitex_tex::Speculation as S;
    let fx = common::fixtures().join("plain");
    let kt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/kt");
    let mut docs: Vec<(std::path::PathBuf, &str)> = ["story", "hyphens", "pages"]
        .iter()
        .map(|&n| (fx.clone(), n))
        .collect();
    if kt.join("s.tex").exists() {
        docs.push((kt, "s"));
    }
    let only = std::env::var("PHITEX_AB").ok();
    for (dir, name) in docs {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let src = common::read(&dir.join(format!("{name}.tex")));
        let mut base: Option<Vec<u8>> = None;
        for t in [1, 4] {
            for (label, sp) in [
                ("off", S::Off),
                ("predicted", S::Predicted),
                ("all", S::All),
            ] {
                let opts = phitex_tex::Options {
                    threads: Some(t),
                    speculation: sp,
                    ..common::options(&format!("./{name}.tex"), name, "", &dir)
                };
                let b = common::build(opts, Some(&common::plain()), &src);
                let out = b.output().0;
                let same = match &base {
                    None => {
                        base = Some(out);
                        true
                    }
                    Some(w) => common::first_difference(&out, w).is_none(),
                };
                let s = &b.stats;
                eprintln!(
                    "{name} t{t} {label:9} same {same} cp {:?} nodes {} runs {} stops {} raises {} spec {}/{}/{} busy/wall {:.2} wall {:?}",
                    s.critical_path,
                    s.nodes_run,
                    s.runs,
                    s.stops,
                    s.raises,
                    s.speculations,
                    s.confirmations,
                    s.mispredictions,
                    s.busy.as_secs_f64() / s.wall.as_secs_f64().max(1e-9),
                    s.wall
                );
                if label == "all" {
                    eprintln!(
                        "  refuted by field: {:?}",
                        s.refuted_by_field.iter().take(10).collect::<Vec<_>>()
                    );
                }
                if !same {
                    for d in b.diagnostics() {
                        eprintln!(
                            "  {:?} {}: {}",
                            d.severity,
                            d.line,
                            d.message.chars().take(600).collect::<String>()
                        );
                    }
                }
                assert!(same, "{name} t{t} {label}: the output differs");
            }
        }
    }
}

/// A chain link's cause is structural: a scope's kind where the nesting
/// changes, a condition level, a list's opener, the page builder's state
/// (one per page), the font, a range of items or pages, a node created.
/// A per-command state cell (read by every command, written by the one
/// before) is the sequential shape this engine exists to avoid.
fn structural(kind: &str, field: &str) -> bool {
    kind != "Changed"
        || field.is_empty()
        || [
            "font", "items", "pages", "lopen:", "scope", "kind",
        ]
        .iter()
        .any(|p| field.starts_with(p))
}

fn chain_cells(dir: &std::path::Path, name: &str) -> (Vec<String>, f64) {
    let src = common::read(&dir.join(format!("{name}.tex")));
    let opts = common::options(&format!("./{name}.tex"), name, "", dir);
    let b = common::build(opts, Some(&common::plain()), &src);
    assert!(b.stats.page_steps > 0, "{name}: no page steps");
    let bad: Vec<String> = b
        .critical_chain()
        .into_iter()
        .filter(|(_, k, f)| !structural(k, f))
        .map(|(w, k, f)| format!("{k} {f}: {w}"))
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let frac = b
        .stats
        .critical_path
        .map_or(1.0, |p| p as f64 / b.stats.nodes_run.max(1) as f64);
    eprintln!(
        "{name}: oracle_fraction {frac:.4}, {} per-command links",
        bad.len()
    );
    (bad, frac)
}

/// The gate: no chain link of the plain fixtures is caused by a
/// per-command state cell.
#[test]
#[ignore = "accl bar until the paragraph cells (par:<a>:<l>), count:0 and \\global's scope kind leave the chain"]
fn chain_links_are_structural() {
    let fx = common::fixtures().join("plain");
    let mut fails = Vec::new();
    for n in ["story", "pages", "hyphens", "display"] {
        let (bad, _) = chain_cells(&fx, n);
        for l in bad.iter().take(10) {
            eprintln!("  {n}: {l}");
        }
        if !bad.is_empty() {
            fails.push((n, bad.len()));
        }
    }
    assert!(fails.is_empty(), "per-command chain links: {fails:?}");
}

/// The bar on accl: the critical path under 5% of the nodes on every
/// fixture, and four threads busier than one on `s.tex`.
#[test]
#[ignore = "a measure for accl: oracle fraction and scaling"]
fn chain_bar() {
    let fx = common::fixtures().join("plain");
    for n in ["story", "pages", "hyphens", "display"] {
        let (_, frac) = chain_cells(&fx, n);
        assert!(frac < 0.05, "{n}: oracle_fraction {frac:.4}");
    }
    let kt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/kt");
    if kt.join("s.tex").exists() {
        let src = common::read(&kt.join("s.tex"));
        let mut ratio = Vec::new();
        for t in [1, 4] {
            let opts = phitex_tex::Options {
                threads: Some(t),
                ..common::options("./s.tex", "s", "", &kt)
            };
            let b = common::build(opts, Some(&common::plain()), &src);
            let r = b.stats.busy.as_secs_f64() / b.stats.wall.as_secs_f64().max(1e-9);
            eprintln!(
                "s at {t} threads: busy/wall {r:.2}, wall {:?}",
                b.stats.wall
            );
            ratio.push(r);
        }
        assert!(ratio[1] > ratio[0], "busy/wall {ratio:?}");
    }
}

/// A measure: 400 plain paragraphs (the chain of a long document).
#[test]
#[ignore = "a measure: the chain of 400 paragraphs"]
fn para400() {
    use std::fmt::Write as _;
    for n in [400, 4000] {
        let name = format!("para{n}");
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/p400");
        let _ = std::fs::create_dir_all(&dir);
        let mut src = String::from("\\vsize=6in\n");
        for k in 0..n {
            let _ = writeln!(
                src,
                "Paragraph {k} has some words in it, enough to fill a few lines of text set \
                 with plain's macros, so that the paragraph builder has work to do and the \
                 pages fill up one after another.\n"
            );
        }
        src.push_str("\\bye\n");
        std::fs::write(dir.join(format!("{name}.tex")), &src).expect("write");
        let t = std::time::Instant::now();
        let fails = chain(&dir, &name);
        eprintln!(
            "{name}: {} target-graph failures (a measure), {:?}",
            fails.len(),
            t.elapsed()
        );
    }
}

/// The zero page chain in exact PDF mode (`docs/TARGET_GRAPH.md`): a
/// generated plain document of 400 paragraphs with headings and
/// footnotes, `\pdfoutput=1`, numbered as pdfTeX numbers. No page step
/// reads what another page step made: the critical path stays a few
/// nodes, and no link of it joins two page steps.
/// The chain census gate (`common::Census`): plain documents of 400
/// paragraphs with footnotes, DVI and exact PDF, cold and after a
/// one-word edit: critical path at most 6, no page step reading
/// another's, at most 1.1 runs a node, at most 2 loop rounds cold and
/// none after the first on the edit, no undeclared definition, no
/// regime violation.
#[test]
fn census_plain() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/census");
    let _ = std::fs::create_dir_all(&dir);
    let mut rows = Vec::new();
    for (name, pdf) in [("para400dvi", false), ("para400pdf", true)] {
        let (src, new) = common::plain_paragraphs(400, pdf);
        std::fs::write(dir.join(format!("{name}.tex")), &src).expect("write");
        let opts = common::options(&format!("./{name}.tex"), name, "", &dir);
        let mut b = common::build(opts, Some(&common::plain()), &src);
        rows.push(common::Census::take(name, &mut b, &new));
    }
    common::assert_census(&rows);
}

