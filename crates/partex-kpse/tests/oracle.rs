//! Oracle tests: the native port must agree with TeX Live's `kpsewhich`
//! (skipped when `kpsewhich` or `tex` is not installed).

use partex_kpse::{Format, Kpse};
use std::path::PathBuf;
use std::process::Command;

fn kpsewhich(args: &[&str]) -> Option<Vec<u8>> {
    kpsewhich_as("tex", "tex", args)
}

fn kpsewhich_as(progname: &str, engine: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("kpsewhich")
        .arg(format!("-progname={progname}"))
        .arg(format!("-engine={engine}"))
        .args(args)
        .output()
        .ok()?;
    let mut o = out.stdout;
    while o.last().is_some_and(u8::is_ascii_whitespace) {
        o.pop();
    }
    out.status.success().then_some(o)
}

/// kpathsea as `tex` sees it: `SELFAUTOLOC` is the directory of `kpsewhich`.
fn instance() -> Option<Kpse> {
    instance_as("tex", "tex")
}

fn instance_as(progname: &str, engine: &str) -> Option<Kpse> {
    let path = std::env::var_os("PATH")?;
    let dir = std::env::split_paths(&path)
        .map(|d| d.join("kpsewhich"))
        .find(|p| p.is_file())?
        .canonicalize()
        .ok()?
        .parent()
        .map(PathBuf::from)?;
    Some(Kpse::new(&dir, progname, engine))
}

#[test]
fn search_paths_match_kpsewhich() {
    let Some(mut k) = instance() else { return };
    for (f, name) in [
        (Format::Tex, "tex"),
        (Format::Tfm, "tfm"),
        (Format::Fmt, "fmt"),
        (Format::Cnf, "cnf"),
        (Format::Db, "ls-R"),
    ] {
        let want = kpsewhich(&[&format!("-show-path={name}")]).unwrap();
        let got = k.show_path(f);
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&want),
            "search path for {name}"
        );
    }
}

#[test]
fn variables_match_kpsewhich() {
    let Some(mut k) = instance() else { return };
    for var in [
        "TEXMF",
        "TEXMFDBS",
        "TEXINPUTS",
        "TFMFONTS",
        "TEXMFCNF",
        "SELFAUTOLOC",
        "TEXMFDOTDIR",
    ] {
        let want = kpsewhich(&[&format!("-var-value={var}")]);
        let got = k.var_value(var);
        assert_eq!(got, want, "variable {var}");
    }
}

#[test]
fn lookups_match_kpsewhich() {
    let Some(mut k) = instance() else { return };
    for (name, f, fmt) in [
        ("plain", Format::Tex, "tex"),
        ("plain.tex", Format::Tex, "tex"),
        ("story", Format::Tex, "tex"),
        ("hyphen", Format::Tex, "tex"),
        ("cmr10", Format::Tfm, "tfm"),
        ("cmmi10.tfm", Format::Tfm, "tfm"),
        ("cmex10", Format::Tfm, "tfm"),
        ("nonexistent-file", Format::Tex, "tex"),
        ("texmf.cnf", Format::Cnf, "cnf"),
    ] {
        let want = kpsewhich(&[&format!("-format={fmt}"), "-must-exist", name]);
        let got = k.find_file(name.as_bytes(), f, true);
        assert_eq!(
            got.as_deref().map(String::from_utf8_lossy),
            want.as_deref().map(String::from_utf8_lossy),
            "lookup of {name}"
        );
    }
}

/// xdvipdfmx's formats, as `dvipdfmx` (run by `xetex`) and as the
/// programs dpxfile.c's `dpx_foolsearch` resets the name to, then back.
#[test]
fn dvipdfmx_paths_match_kpsewhich() {
    let Some(mut k) = instance_as("dvipdfmx", "xetex") else {
        return;
    };
    let formats = [
        (Format::FontMap, "map"),
        (Format::Type1, "type1 fonts"),
        (Format::TrueType, "truetype fonts"),
        (Format::OpenType, "opentype fonts"),
        (Format::Cmap, "cmap files"),
        (Format::Sfd, "subfont definition files"),
        (Format::Enc, "enc files"),
        (Format::Tfm, "tfm"),
        (Format::Ofm, "ofm"),
        (Format::Vf, "vf"),
        (Format::Ovf, "ovf"),
        (Format::Pict, "graphic/figure"),
        (Format::Tex, "tex"),
        (Format::ProgramText, "other text files"),
        (Format::ProgramBinary, "other binary files"),
    ];
    for prog in [
        "dvipdfmx", "cmap", "tex", "ttf2pk", "ttf2tfm", "dvips", "dvipdfmx",
    ] {
        k.reset_program_name(prog.as_bytes());
        for (f, name) in formats {
            let want = kpsewhich_as(prog, "xetex", &[&format!("-show-path={name}")]).unwrap();
            let got = k.show_path(f);
            assert_eq!(
                String::from_utf8_lossy(&got),
                String::from_utf8_lossy(&want),
                "search path for {name} as {prog}"
            );
        }
    }
    for (name, f, fmt) in [
        ("lmroman10-regular", Format::OpenType, "opentype fonts"),
        ("cmr10", Format::Ofm, "ofm"),
        ("cmr10.tfm", Format::Ofm, "ofm"),
        ("Identity-H", Format::Cmap, "cmap files"),
        ("dvipdfmx.cfg", Format::ProgramText, "other text files"),
        ("glyphlist.txt", Format::FontMap, "map"),
    ] {
        let want = kpsewhich_as("dvipdfmx", "xetex", &[&format!("-format={fmt}"), name]);
        let got = k.find_file(name.as_bytes(), f, false);
        assert_eq!(
            got.as_deref().map(String::from_utf8_lossy),
            want.as_deref().map(String::from_utf8_lossy),
            "lookup of {name}"
        );
    }
}
