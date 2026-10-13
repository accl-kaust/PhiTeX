//! The critical chain of plain documents, classified by cause: the
//! measure for the chain work (`PHITEX_CHAIN=1` prints every link).

mod common;

use std::collections::BTreeMap;

fn chain(dir: &std::path::Path, name: &str) {
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
                "{name} at {t} threads: wall {:?}, busy {:?}, busy/wall {:.2}",
                b.stats.wall,
                b.stats.busy,
                b.stats.busy.as_secs_f64() / b.stats.wall.as_secs_f64().max(1e-9)
            );
        }
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
        "{name}: {} nodes run, {} runs, critical path {:?}, peak running {}, ready mean {:.2}, page steps {} ({} any), chain {} links: {by:?}",
        b.stats.nodes_run,
        b.stats.runs,
        b.stats.critical_path,
        b.stats.peak_running,
        b.stats.ready_mean(),
        b.stats.page_steps,
        b.stats.page_steps_any,
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
}

#[test]
#[ignore = "a measure: the critical chain by cause"]
fn chains() {
    let fx = common::fixtures().join("plain");
    for n in ["story", "pages", "hyphens", "display"] {
        chain(&fx, n);
    }
    let kt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/kt");
    if kt.join("s.tex").exists() {
        chain(&kt, "s");
    }
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
