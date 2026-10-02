//! Oracle tests: the native port must agree with TeX Live's `kpsewhich`
//! (skipped when `kpsewhich` or `tex` is not installed).

use partex_kpse::{Format, Kpse};
use std::path::PathBuf;
use std::process::Command;

fn kpsewhich(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("kpsewhich")
        .args(["-progname=tex", "-engine=tex"])
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
    let path = std::env::var_os("PATH")?;
    let dir = std::env::split_paths(&path)
        .map(|d| d.join("kpsewhich"))
        .find(|p| p.is_file())?
        .canonicalize()
        .ok()?
        .parent()
        .map(PathBuf::from)?;
    Some(Kpse::new(&dir, "tex", "tex"))
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
