//! XeTeX (docs/spec-xetex.md): the XDV file equals `xetex -no-pdf`'s,
//! at one thread and at eight. The oracles are TeX Live 2026's
//! (`oracle/fixtures/xetex/bless.sh`, `SOURCE_DATE_EPOCH=0`,
//! `FORCE_SOURCE_DATE=1`): `ini/NAME.xdv` from `xetex -ini -etex
//! NAME.tex`; `plain/NAME.xdv` from `xetex -ini -etex` on plain.tex as
//! the plain gates load it followed by `oracle/fixtures/plain/NAME.tex`
//! (`-default`: with `\output={}` between them), one file.
//!
//! `PHITEX_GATE=NAME` runs that case alone; `PHITEX_GATE_OUT=DIR`
//! writes each XDV made there.

mod common;

use std::path::Path;

use phitex_tex::{Options, XeTeX};

fn xetex() -> XeTeX {
    XeTeX::default()
}

/// One document, its XDV against the oracle's at `threads`.
fn gate(dir: &Path, name: &str, src: &str, want: &[u8], threads: usize) -> Result<String, String> {
    let opts = Options {
        epoch: Some(0),
        threads: Some(threads),
        xetex: Some(xetex()),
        ..common::options(
            &format!("./{name}.tex"),
            name,
            &common::dvi_comment(want),
            dir,
        )
    };
    let b = common::build(opts, None, src);
    let (xdv, log) = b.output();
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.xdv"), &xdv);
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), log.as_bytes());
    }
    match common::first_difference(&xdv, want) {
        None => Ok(format!("{name} -j{threads}: {} bytes", xdv.len())),
        Some(d) => {
            let diags: Vec<String> = b
                .diagnostics()
                .iter()
                .take(12)
                .map(|d| {
                    format!(
                        "  {}:{} {}",
                        d.file,
                        d.line,
                        d.message.chars().take(300).collect::<String>()
                    )
                })
                .collect();
            let tail: String = log
                .chars()
                .rev()
                .take(1500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(format!(
                "{name} -j{threads}: FAIL {d}\n{}\n--- log tail ---\n{tail}",
                diags.join("\n")
            ))
        }
    }
}

fn run(cases: &[(&str, String, Vec<u8>, &Path)]) {
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    for (name, src, want, dir) in cases {
        if only.as_ref().is_some_and(|o| o != name) {
            continue;
        }
        for threads in [1, 8] {
            match gate(dir, name, src, want, threads) {
                Ok(r) => eprintln!("ok {r}"),
                Err(r) => {
                    eprintln!("{r}");
                    failed.push(format!("{name} -j{threads}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

fn ini_cases(names: &[&'static str]) -> Vec<(&'static str, String, Vec<u8>, std::path::PathBuf)> {
    let x = common::fixtures().join("xetex");
    names
        .iter()
        .map(|&n| {
            let src = common::read(&x.join(format!("ini/{n}.tex")));
            let want = std::fs::read(x.join(format!("oracle/ini/{n}.xdv"))).expect("the oracle");
            (n, src, want, x.join("ini"))
        })
        .collect()
}

fn run_owned(cases: &[(&'static str, String, Vec<u8>, std::path::PathBuf)]) {
    let c: Vec<(&str, String, Vec<u8>, &Path)> = cases
        .iter()
        .map(|(n, s, w, d)| (*n, s.clone(), w.clone(), d.as_path()))
        .collect();
    run(&c);
}

/// TFM fonts only: the writer's XDV opcodes, two pages.
#[test]
fn ini_tfm() {
    run_owned(&ini_cases(&["tfm"]));
}

/// The Unicode reader, `^^^^`, wide codes, `\Uchar`, `\Ucharcat`.
#[test]
fn ini_chars() {
    run_owned(&ini_cases(&["chars"]));
}

/// plain.tex's documents under XeTeX (TFM fonts).
#[test]
fn plain_docs() {
    let fx = common::fixtures();
    let plain = common::plain();
    let x = fx.join("xetex/oracle/plain");
    let mut cases = Vec::new();
    for body in ["story", "pages", "display", "table", "hyphens"] {
        let b = common::read(&fx.join(format!("plain/{body}.tex")));
        for default in [false, true] {
            let name: &'static str = Box::leak(
                if default {
                    format!("{body}-default")
                } else {
                    body.to_owned()
                }
                .into_boxed_str(),
            );
            let src = if default {
                format!("{plain}\\output={{}}\n{b}")
            } else {
                format!("{plain}{b}")
            };
            let want = std::fs::read(x.join(format!("{name}.xdv"))).expect("the oracle");
            cases.push((name, src, want, fx.join("plain")));
        }
    }
    run_owned(&cases);
}
