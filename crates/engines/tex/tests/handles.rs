//! The build's interned field handles outlive the engine's value sweeps:
//! a cold build, then edits whose nodes define through them (catcodes,
//! parameters, registers) in later evaluations, each equal to a cold
//! build of its text.

mod common;

use std::fmt::Write as _;

use phitex_tex::{Build, Options};

fn doc(k: usize) -> String {
    let mut s = format!("\\catcode`\\|={k} \\parindent={k}pt \\count1={k}\n");
    for n in 0..40 {
        let _ = writeln!(s, "Line {n} of text with \\the\\count1.\\par");
    }
    s.push_str("\\bye\n");
    s
}

fn build(text: &str) -> Build {
    let fx = common::fixtures().join("plain");
    let o = Options {
        time_limit: None,
        ..common::options("./h.tex", "h", "", &fx)
    };
    common::build(o, Some(&common::plain()), text)
}

#[test]
fn handles_survive_evaluations() {
    let mut a = build(&doc(12));
    for k in [11, 12, 10] {
        let text = doc(k);
        a.edit(&text);
        let cold = build(&text);
        assert!(
            common::first_difference(&a.output().0, &cold.output().0).is_none(),
            "edit {k}"
        );
    }
}
