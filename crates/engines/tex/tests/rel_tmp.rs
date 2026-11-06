//! (scratch, not committed) variants of the restore-plus test, printing what differs.
mod common;
use phitex_tex::Options;
use std::fmt::Write as _;

fn writes(b: &phitex_tex::Build) -> String {
    b.writes()
        .into_iter()
        .filter(|(k, _)| *k == 1)
        .map(|(_, t)| t)
        .collect()
}

fn cold(name: &str, src: &str) -> (Vec<u8>, String) {
    let dir = common::scratch(&format!("reltmp-{name}-cold"));
    let opts = Options {
        threads: Some(1),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), src);
    (b.output().0, writes(&b))
}

fn variant(name: &str, body: &str, edit: &str, n: usize) {
    let src = |extra: &str| {
        let mut s = String::from("\\openout1=\\jobname.out\n\\count200=5\n");
        s.push_str(extra);
        for i in 0..n {
            let b = body.replace("@I@", &i.to_string());
            let _ = write!(
                s,
                "{b} Paragraph {i} of the document, long enough to make a line or two when set in the plain format's ten point roman.\n\n"
            );
        }
        s.push_str("\\closeout1\n\\bye\n");
        s
    };
    let dir = common::scratch(&format!("reltmp-{name}"));
    let opts = Options {
        threads: Some(1),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let first = src("");
    let mut b = common::build(opts, Some(&common::plain()), &first);
    let (_, o0) = cold(&format!("{name}-a"), &first);
    let w0 = writes(&b);
    let edited = src(edit);
    common::rebuild(&mut b, &edited);
    let (d1, o1) = cold(&format!("{name}-b"), &edited);
    let w1 = writes(&b);
    eprintln!(
        "VARIANT {name}: cold-first-ok {} edited-dvi-ok {} edited-writes-ok {} refuted {:?}\n  got  {}\n  want {}",
        w0 == o0,
        b.output().0 == d1,
        w1 == o1,
        b.stats.refuted_by_field,
        &w1.chars().take(160).collect::<String>(),
        &o1.chars().take(160).collect::<String>()
    );
}

#[test]
fn variants() {
    let w = "\\edef\\w{\\write1{n@I@=\\the\\count200.}}\\w";
    let g = "\\global\\advance\\count200 1\n";
    let l = "\\advance\\count200 1000\n";
    // (the existing test's shape, the edit global)
    if std::env::var("ONLY_SHORT").is_ok() {
        variant(
            "restore-g-short",
            &format!(
                "\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}\\global\\advance\\count200 7 \\global\\count200=\\saved\\relax {w}"
            ),
            g,
            6,
        );
        return;
    }
    variant(
        "stepped-g",
        &format!("\\global\\advance\\count200 1 \\advance\\count200 2 {w}"),
        g,
        80,
    );
    variant(
        "stepped-l",
        &format!("\\global\\advance\\count200 1 \\advance\\count200 2 {w}"),
        l,
        80,
    );
    variant(
        "onlyglobal-g",
        &format!("\\global\\advance\\count200 1 {w}"),
        g,
        80,
    );
    variant(
        "xdef-g",
        &format!("\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}{w}"),
        g,
        80,
    );
    variant(
        "restore-g",
        &format!(
            "\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}\\global\\advance\\count200 7 \\global\\count200=\\saved\\relax {w}"
        ),
        g,
        80,
    );
    variant(
        "restore-l",
        &format!(
            "\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}\\global\\advance\\count200 7 \\global\\count200=\\saved\\relax {w}"
        ),
        l,
        80,
    );
    variant(
        "restore-g-short",
        &format!(
            "\\global\\advance\\count200 1 \\xdef\\saved{{\\the\\count200}}\\global\\advance\\count200 7 \\global\\count200=\\saved\\relax {w}"
        ),
        g,
        6,
    );
}
