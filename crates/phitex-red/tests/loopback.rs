//! Loop-back: files the job writes are read back by the next round, never from disk.
mod common;
use common::*;

const DOC: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\@=11
\\font\\r=cmr10 \\r \\hsize=200pt \\vsize=300pt \\parindent=0pt \\baselineskip=12pt
\\parfillskip=0pt plus 1fil \\output={\\shipout\\box255 \\global\\advance\\count0 1 }
\\newlinechar=-1 \\immediate\\openin1=job.aux
\\ifeof1 \\else \\closein1 \\input job.aux \\fi
\\immediate\\openout2=job.aux
\\def\\label#1{\\write2{\\string\\def\\string\\l@#1{\\the\\count0}}}
\\def\\ref#1{\\expandafter\\ifx\\csname l@#1\\endcsname\\relax ??\\else\\csname l@#1\\endcsname\\fi}
\\count0=1
See page \\ref{b}.\\par\\penalty-10000
Second.\\label{b}\\par\\penalty-10000
\\count1=\\ifx\\l@b\\undefined 0\\else\\l@b\\fi
\\errmessage{ref=\\the\\count1}
\\end
";

#[test]
fn label_ref_converges_within_three_rounds() {
    let (_, out) = build_with(opts(false, true), DOC, &[]);
    assert!(out.rounds >= 2 && out.rounds <= 3, "rounds = {}", out.rounds);
    let m = msgs(&out);
    assert!(m.iter().any(|s| s == "ref=2"), "final round sees the label: {m:?}");
}

#[test]
fn no_loopback_one_round() {
    let out = run("\\errmessage{x}");
    assert_eq!(out.rounds, 1);
}

#[test]
fn edit_that_moves_label_reconverges() {
    let (mut b, _) = build_with(opts(false, true), DOC, &[]);
    let at = DOC.find("Second.").unwrap();
    let out = b.update(vec![Change::Splice {
        path: "job.tex".into(),
        start: at,
        end: at,
        text: "Extra.\\par\\penalty-10000 ".into(),
    }]);
    assert!(out.rounds <= 3, "rounds = {}", out.rounds);
    assert!(msgs(&out).iter().any(|s| s == "ref=3"), "{:?}", msgs(&out));
}
