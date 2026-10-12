//! Gate 1: `oracle/fixtures/core/*.tex` under INITEX; the log's lines
//! from `(./core.tex` on equal TeX's.

mod common;

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

#[test]
fn core() {
    let dir = common::fixtures().join("core");
    let only = std::env::var("PHITEX_CORE").ok();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.ok()?.file_name().into_string().ok()?;
            n.strip_suffix(".tex").map(str::to_owned)
        })
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for n in &names {
        let src = common::read(&dir.join(format!("{n}.tex")));
        let want = common::read(&dir.join(format!("{n}.log")));
        let want = want.trim_end_matches('\n');
        let doc = format!("{PRELUDE}{src}\\end\n");
        let opts = common::options("./core.tex", "texput", "", &dir);
        let b = common::build(opts, None, &doc);
        let (_, log) = b.output();
        let log = common::log_excerpt(&log);
        match common::first_difference(log.as_bytes(), want.as_bytes()) {
            None => eprintln!("core/{n}: ok ({} nodes run)", b.stats.nodes_run),
            Some(d) => {
                eprintln!("core/{n}: FAIL {d}");
                failed.push(n.clone());
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// A node that makes one child per word of a long `\hyphenation` list
/// stays within the engine's limits (children are bounded by the text).
#[test]
fn long_hyphenation_list() {
    let mut words = String::new();
    for k in 0..10_000u32 {
        let mut w = String::from("a-b");
        let mut n = k;
        for _ in 0..3 {
            w.push(char::from(b'a' + u8::try_from(n % 26).unwrap_or(0)));
            n /= 26;
        }
        words.push_str(&w);
        words.push(' ');
    }
    let doc = format!("\\catcode`\\{{=1 \\catcode`\\}}=2 \\hyphenation{{{words}}}\\end\n");
    let dir = common::fixtures().join("core");
    let opts = common::options("./core.tex", "texput", "", &dir);
    let _ = common::build(opts, None, &doc);
}
