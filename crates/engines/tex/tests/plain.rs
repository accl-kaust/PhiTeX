//! Gates 2 and 3: plain TeX documents; the DVI file equals TeX's.

mod common;

fn gate(dir: &str, body: &str, default: bool, plain: bool) -> Result<String, String> {
    let fx = common::fixtures().join(dir);
    let name = if default {
        format!("{body}-default")
    } else {
        body.to_owned()
    };
    let want =
        std::fs::read(fx.join(format!("{name}.dvi"))).map_err(|e| format!("{name}.dvi: {e}"))?;
    let src = common::read(&fx.join(format!("{body}.tex")));
    let doc = if default {
        format!("\\output={{}}\n{src}")
    } else {
        src
    };
    let prelude = plain.then(common::plain);
    let opts = common::options(
        &format!("./{body}.tex"),
        body,
        &common::dvi_comment(&want),
        &fx,
    );
    let b = common::build(opts, prelude.as_deref(), &doc);
    let (dvi, log) = b.output();
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}.dvi"), &dvi);
    }
    let s = &b.stats;
    let report = format!(
        "{name}: {} nodes run, {} runs, {} live, {} rounds, tails {}/{}, starved {}, critical path {}, hints {} bounded / {} any, {} regime violations, {} undeclared, stops {} resumed {} restarted {}",
        s.nodes_run,
        s.runs,
        s.live,
        s.rounds,
        s.tails,
        s.tail_siblings,
        s.starved,
        s.critical_path.unwrap_or(0),
        s.nodes_bounded,
        s.nodes_any,
        s.regime_violations,
        s.undeclared_definitions,
        s.stops,
        s.resumed,
        s.restarted
    );
    if s.regime_violations != 0 || s.undeclared_definitions != 0 {
        let diags: Vec<String> = b
            .diagnostics()
            .iter()
            .filter(|d| {
                d.message.contains("hint")
                    || d.message.contains("undeclared")
                    || d.message.contains("UndeclaredDefinition")
            })
            .map(|d| {
                format!(
                    "{}:{} {}",
                    d.file,
                    d.line,
                    d.message.chars().take(1200).collect::<String>()
                )
            })
            .collect();
        return Err(format!("{report}\n  FAIL hints: {}", diags.join("\n  ")));
    }
    match common::first_difference(&dvi, &want) {
        None => Ok(report),
        Some(d) => {
            let tail: String = log
                .chars()
                .rev()
                .take(1500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(format!("{report}\n  FAIL {d}\n--- log tail ---\n{tail}"))
        }
    }
}

fn run(cases: &[(&str, &str, bool, bool)]) {
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    for &(dir, body, default, plain) in cases {
        let name = format!("{body}{}", if default { "-default" } else { "" });
        if only.as_ref().is_some_and(|o| *o != name) {
            continue;
        }
        match gate(dir, body, default, plain) {
            Ok(r) => eprintln!("ok {r}"),
            Err(r) => {
                eprintln!("{r}");
                failed.push(name);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

#[test]
fn story() {
    run(&[
        ("plain", "story", false, true),
        ("plain", "story", true, true),
    ]);
}

#[test]
fn hyphens_pages() {
    run(&[
        ("plain", "hyphens", false, true),
        ("plain", "hyphens", true, true),
        ("plain", "pages", false, true),
        ("plain", "pages", true, true),
    ]);
}

#[test]
fn dvi_fixtures() {
    run(&[
        ("dvi", "hbox", false, false),
        ("dvi", "endgroup", false, false),
        ("dvi", "input", false, false),
        ("dvi", "fonts", false, false),
        ("dvi", "disc", false, false),
        ("dvi", "accent", false, false),
    ]);
}

#[test]
fn display() {
    run(&[
        ("plain", "display", false, true),
        ("plain", "display", true, true),
    ]);
}

#[test]
fn table() {
    run(&[
        ("plain", "table", false, true),
        ("plain", "table", true, true),
    ]);
}
