//! The build's file finder (`kpse::Kpathsea`, partex-kpse) against TeX
//! Live's `kpsewhich`: for the files the LaTeX documents, the plain and
//! the PDF tests read, the path found is `kpsewhich -progname=PROG
//! -format=FORMAT NAME`'s, or none where it finds none. Skipped where
//! there is no `kpsewhich`.

mod common;

use std::process::Command;

use phitex_flow::kpse::Kpathsea;
use phitex_flow::tfm::{Kpse as _, KpseFormat};

fn have_kpsewhich() -> bool {
    Command::new("kpsewhich").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// `kpsewhich -progname=PROG -format=FORMAT NAME`: the path, or none.
fn kpsewhich(prog: &str, format: KpseFormat, name: &str) -> Option<String> {
    let out = Command::new("kpsewhich")
        .arg(format!("-progname={prog}"))
        .arg(format!("-format={}", format.name()))
        .arg(name)
        .output()
        .expect("kpsewhich runs");
    let s = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
    (!s.is_empty()).then_some(s)
}

/// (program, format, name): what the documents and tests read.
const TABLE: &[(&str, KpseFormat, &str)] = &[
    // TFM files: plain's, LaTeX's OT1 and T1, AMS
    ("pdftex", KpseFormat::Tfm, "cmr10.tfm"),
    ("pdftex", KpseFormat::Tfm, "cmbx10.tfm"),
    ("pdftex", KpseFormat::Tfm, "cmex10.tfm"),
    ("pdftex", KpseFormat::Tfm, "cmmi7.tfm"),
    ("pdflatex", KpseFormat::Tfm, "ecrm1000.tfm"),
    ("pdflatex", KpseFormat::Tfm, "msam10.tfm"),
    ("latex", KpseFormat::Tfm, "cmr10.tfm"),
    ("pdftex", KpseFormat::Tfm, "nosuchfont.tfm"),
    // PDF output: the map file, Type 1 fonts, an encoding
    ("pdftex", KpseFormat::FontMap, "pdftex.map"),
    ("pdflatex", KpseFormat::FontMap, "pdftex.map"),
    ("pdftex", KpseFormat::Type1, "cmr10.pfb"),
    ("pdftex", KpseFormat::Type1, "cmmi10.pfb"),
    ("pdftex", KpseFormat::Type1, "cmex10.pfb"),
    ("pdflatex", KpseFormat::Type1, "sfrm1000.pfb"),
    ("pdflatex", KpseFormat::Type1, "msam10.pfb"),
    ("pdflatex", KpseFormat::Enc, "cm-super-t1.enc"),
    ("pdftex", KpseFormat::Type1, "nosuchfont.pfb"),
    // inputs: plain's and the formats' ini files
    ("pdftex", KpseFormat::Tex, "plain.tex"),
    ("pdftex", KpseFormat::Tex, "plain"),
    ("pdftex", KpseFormat::Tex, "pdfetex.ini"),
    ("pdftex", KpseFormat::Tex, "etex.src"),
    ("pdftex", KpseFormat::Tex, "hyphen.tex"),
    ("tex", KpseFormat::Tex, "tex.ini"),
    ("latex", KpseFormat::Tex, "latex.ini"),
    ("pdflatex", KpseFormat::Tex, "pdflatex.ini"),
    ("latex", KpseFormat::Tex, "latex.ltx"),
    ("latex", KpseFormat::Tex, "hyphen.cfg"),
    // LaTeX: the class, its options, packages, font definitions, drivers
    ("latex", KpseFormat::Tex, "article.cls"),
    ("pdflatex", KpseFormat::Tex, "article.cls"),
    ("latex", KpseFormat::Tex, "size10.clo"),
    ("latex", KpseFormat::Tex, "ot1cmr.fd"),
    ("pdflatex", KpseFormat::Tex, "t1cmr.fd"),
    ("latex", KpseFormat::Tex, "amsmath.sty"),
    ("latex", KpseFormat::Tex, "amssymb.sty"),
    ("latex", KpseFormat::Tex, "graphicx.sty"),
    ("latex", KpseFormat::Tex, "geometry.sty"),
    ("latex", KpseFormat::Tex, "fancyhdr.sty"),
    ("latex", KpseFormat::Tex, "enumitem.sty"),
    ("latex", KpseFormat::Tex, "babel.sty"),
    ("latex", KpseFormat::Tex, "english.ldf"),
    ("latex", KpseFormat::Tex, "dvips.def"),
    ("pdflatex", KpseFormat::Tex, "pdftex.def"),
    ("latex", KpseFormat::Tex, "color.cfg"),
    ("latex", KpseFormat::Tex, "article.cls.tex"),
    ("latex", KpseFormat::Tex, "nosuchpackage.sty"),
    // the current directory (the test's: this crate's)
    ("pdftex", KpseFormat::Tex, "Cargo.toml"),
];

#[test]
fn the_paths_are_kpsewhichs() {
    if !have_kpsewhich() {
        eprintln!("no kpsewhich: skipped");
        return;
    }
    let mut finders: Vec<(&str, Kpathsea)> = Vec::new();
    let one = common::kpathsea_as("pdftex", "pdftex").expect("TeX Live beside kpsewhich");
    let mut bad = Vec::new();
    let mut found = 0;
    for &(prog, format, name) in TABLE {
        let want = kpsewhich(prog, format, name);
        // (one instance switching programs, as a build's finder asks, and
        // one made for the program)
        let got = one.find(name, format, prog);
        if !finders.iter().any(|(p, _)| *p == prog) {
            finders.push((prog, common::kpathsea_as(prog, "pdftex").expect("TeX Live")));
        }
        let own = finders.iter().find(|(p, _)| *p == prog).expect("made").1.find(name, format, prog);
        eprintln!("{prog} {} {name}: kpsewhich {want:?}, ours {got:?}", format.name());
        if got != want || own != want {
            bad.push(format!("{prog} -format='{}' {name}: kpsewhich {want:?}, ours {got:?} (own instance {own:?})", format.name()));
        }
        found += usize::from(want.is_some());
    }
    assert!(bad.is_empty(), "paths that are not kpsewhich's:\n{}", bad.join("\n"));
    assert!(found > TABLE.len() / 2, "kpsewhich found only {found} of {} files", TABLE.len());
}

/// A format's finder never finds a file in the current directory.
#[test]
fn without_cwd_skips_the_current_directory() {
    if !have_kpsewhich() {
        eprintln!("no kpsewhich: skipped");
        return;
    }
    let k = common::kpathsea_as("pdftex", "pdftex").expect("TeX Live").without_cwd();
    assert_eq!(k.find("Cargo.toml", KpseFormat::Tex, "pdftex"), None);
    assert_eq!(k.find("plain.tex", KpseFormat::Tex, "pdftex"), kpsewhich("pdftex", KpseFormat::Tex, "plain.tex"));
}
