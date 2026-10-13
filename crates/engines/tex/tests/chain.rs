//! The critical chain of plain documents, classified by cause: the
//! measure for the chain work (`PHITEX_CHAIN=1` prints every link).

mod common;

use std::collections::BTreeMap;

fn chain(dir: &std::path::Path, name: &str) {
    let src = common::read(&dir.join(format!("{name}.tex")));
    let opts = common::options(&format!("./{name}.tex"), name, "", dir);
    let b = common::build(opts, Some(&common::plain()), &src);
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
        "{name}: {} nodes run, critical path {:?}, chain {} links: {by:?}",
        b.stats.nodes_run,
        b.stats.critical_path,
        c.len()
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
