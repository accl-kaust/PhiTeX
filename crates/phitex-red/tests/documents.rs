//! Document-level: DVI/PDF against oracle outputs made by tex/pdftex.
//! Fixtures are the inputs/oracles under crates/phitex-flow/tests/{dvi,pdf}
//! (data only). They load cmr fonts through kpse: TeX Live must be installed.
mod common;
use common::*;

fn dvi_case(name: &str) {
    let tex = read(&fixture("dvi", &format!("{name}.tex")));
    let want = std::fs::read(fixture("dvi", &format!("{name}.dvi"))).unwrap();
    let (_, out) = build_with(opts(false, false), &tex, &[]);
    let got = out.dvi.as_deref().unwrap_or_else(|| panic!("{name}: no DVI; diags {:?}", msgs(&out)));
    assert!(dvi_body(got) == dvi_body(&want), "{name}: DVI differs ({} vs {} bytes)", got.len(), want.len());
}

macro_rules! dvi_tests { ($($n:ident),*) => { $( #[test] fn $n() { dvi_case(stringify!($n)); } )* } }
dvi_tests!(hbox, vbox, para, accent, disc, halign, halign2, valign, leaders, math1, math2, math3, math4,
    display, inserts, lastbox, pages, pens, shape, skips, setbox, vsplit, ship, outroutine, outmarks,
    outinserts, outpen, pagegoal, middle, long, texxet, hyph1, hyph2, fonts, llfit);

#[test]
#[ignore = "loads plain.tex in full and typesets its text: slow"]
fn plaintext() {
    dvi_case("plaintext");
}

fn pdf_case(name: &str) {
    let tex = read(&fixture("pdf", &format!("{name}.tex")));
    let want = std::fs::read(fixture("pdf", &format!("{name}.pdf"))).unwrap();
    let (_, out) = build_with(opts(false, true), &tex, &[]);
    let got = out.pdf.as_deref().unwrap_or_else(|| panic!("{name}: no PDF; diags {:?}", msgs(&out)));
    assert!(got.starts_with(b"%PDF-"), "{name}: not a PDF");
    // Object numbers / resource names may differ (relaxed); pages may not.
    assert_eq!(pdf_pages(got), pdf_pages(&want), "{name}: page count");
}

macro_rules! pdf_tests { ($($n:ident),*) => { $( #[test] fn $n() { pdf_case(&stringify!($n)[4..]); } )* } }
pdf_tests!(pdf_hello, pdf_annot, pdf_rules, pdf_literal, pdf_mag, pdf_misc, pdf_outline, pdf_thread,
    pdf_xform, pdf_obj, pdf_compressed, pdf_colorstack, pdf_interword, pdf_tounicode, pdf_vf, pdf_ximage);

/// `\message` output of the core cases equals tex's log (between the
/// file's open paren and close paren).
#[test]
#[ignore = "needs Output::log (terminal transcript), not in the agreed Output yet"]
fn core_logs() {
    for n in ["arith", "case", "conds", "edef", "expandafter", "futurelet", "groups", "macros", "the"] {
        let tex = read(&fixture("core", &format!("{n}.tex")));
        let want = read(&fixture("core", &format!("{n}.log")));
        let (_, out) = build_with(opts(false, false), &tex, &[]);
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let got = norm(&out.log);
        let w = norm(&want);
        let body = w.split_once(' ').map_or(w.as_str(), |x| x.1).trim_end_matches(')').trim();
        assert!(got.contains(body), "{n}: log differs\n got: {got}\nwant: {body}");
    }
}
