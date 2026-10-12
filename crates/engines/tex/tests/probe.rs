//! A tool, not a gate: builds the file `PHITEX_PROBE` (INITEX, or after
//! plain with `PHITEX_PROBE_PLAIN`) and prints its log.

mod common;

#[test]
fn probe() {
    let Ok(p) = std::env::var("PHITEX_PROBE") else {
        return;
    };
    let p = std::path::PathBuf::from(p);
    let doc = common::read(&p);
    let dir = p.parent().unwrap().to_path_buf();
    let opts = common::options("./core.tex", "texput", "", &dir);
    let pre = std::env::var("PHITEX_PROBE_PLAIN")
        .ok()
        .map(|_| common::plain());
    let b = common::build(opts, pre.as_deref(), &doc);
    let (dvi, log) = b.output();
    if let Ok(o) = std::env::var("PHITEX_PROBE_OUT") {
        let _ = std::fs::write(o, &dvi);
    }
    println!(
        "{log}\n-- {} dvi bytes, {} nodes run, {} live",
        dvi.len(),
        b.stats.nodes_run,
        b.stats.live
    );
}
