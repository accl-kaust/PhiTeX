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
        "{name}: {} nodes run, {} runs, {} live, {} rounds, tails {}/{}, starved {}, critical path {}, hints {} bounded / {} any, {} regime violations, {} undeclared, stops {} resumed {} restarted {}, env_provided {}, restarted_by {:?}",
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
        s.restarted,
        s.env_provided,
        s.restarted_by
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
        ("dvi", "pargroup", false, false),
        ("dvi", "mathpar", false, false),
        ("dvi", "errors100", false, false),
        ("dvi", "catline", false, false),
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

/// §336-§339: an `\outer` macro met while a definition, an argument or a
/// text is scanned, or while conditional text is skipped (and stepped
/// over by `\expandafter` there): TeX's errors and recoveries.
#[test]
fn outer_validity() {
    run(&[
        ("plain", "outer-def", false, true),
        ("plain", "outer-use", false, true),
        ("plain", "outer-text", false, true),
        ("plain", "outer-skip", false, true),
        ("plain", "outer-after", false, true),
    ]);
}

/// A slice whose hint claims the blank line after it (its paragraph to
/// end there) but whose run takes nothing: the blank line is still read,
/// as its own node, and the lines after it numbered as TeX numbers them.
#[test]
fn line_claims() {
    run(&[("plain", "lineclaim", false, true)]);
}

/// A file read back whose lines build on each other (beamer's `.nav`:
/// each `\headcommand` advances a count and defines a macro named by
/// it): TeX's second run reads the lines in order, three heads named
/// `h1` to `h3`. Read back per field, each line on its own, all three
/// lines defined `h1` from a count of zero, and the page said
/// `Heads: 1: three, , .` (the plain/headcount oracle: TeX run twice).
#[test]
fn a_file_whose_lines_build_on_each_other_is_read_in_order() {
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("headcount.tex"));
    let want = std::fs::read(fx.join("headcount.dvi")).expect("the oracle");
    for threads in [1, 8] {
        let opts = phitex_tex::Options {
            fixpoint: true,
            threads: Some(threads),
            ..common::options("./headcount.tex", "headcount", &common::dvi_comment(&want), &fx)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        let (dvi, log) = b.output();
        assert_eq!(common::first_difference(&dvi, &want), None, "-j{threads}: DVI differs; log tail:\n{}", common::log_excerpt(&log));
    }
}
