//! What the gates share: the oracle's fixtures, the file finder (the
//! fixture's directory, then kpathsea for fonts), comparisons.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use phitex_tex::{Build, Finder, Options};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures")
}

pub fn read(p: &Path) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    b.iter().map(|&c| char::from(c)).collect()
}

fn kpse() -> &'static Mutex<Option<phitex_kpse::Kpse>> {
    static K: OnceLock<Mutex<Option<phitex_kpse::Kpse>>> = OnceLock::new();
    K.get_or_init(|| {
        let path = std::env::var("PATH").unwrap_or_default();
        let dir = std::env::split_paths(&path).find(|d| d.join("tex").is_file());
        Mutex::new(dir.map(|d| phitex_kpse::Kpse::new(&d, "tex", "tex")))
    })
}

/// Files from `dir` (printed as `./name`), fonts through kpathsea.
pub fn finder(dir: &Path) -> Finder {
    let dir = dir.to_path_buf();
    Arc::new(move |name: &str| {
        let p = dir.join(name);
        if p.is_file() {
            return Some((format!("./{name}"), std::fs::read(&p).ok()?));
        }
        let fmt = if name.ends_with(".tfm") {
            phitex_kpse::Format::Tfm
        } else {
            phitex_kpse::Format::Tex
        };
        let mut k = kpse().lock().ok()?;
        let path = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
        let path = String::from_utf8(path).ok()?;
        Some((path.clone(), std::fs::read(&path).ok()?))
    })
}

pub fn options(name: &str, jobname: &str, comment: &str, dir: &Path) -> Options {
    Options {
        name: name.to_owned(),
        jobname: jobname.to_owned(),
        comment: comment.to_owned(),
        finder: finder(dir),
    }
}

pub fn build(opts: Options, prelude: Option<&str>, doc: &str) -> Build {
    Build::with(opts, prelude, doc)
}

/// The first difference, for the report.
pub fn first_difference(got: &[u8], want: &[u8]) -> Option<String> {
    let n = got.iter().zip(want).take_while(|(a, b)| a == b).count();
    if n == got.len() && n == want.len() {
        return None;
    }
    let show = |b: &[u8]| -> String {
        b[n.saturating_sub(40).min(b.len())..(n + 40).min(b.len())]
            .iter()
            .map(|&c| {
                if (32..127).contains(&c) || c == b'\n' {
                    char::from(c)
                } else {
                    '?'
                }
            })
            .collect()
    };
    Some(format!(
        "byte {n} (got {} bytes, want {}):\n  got:  {:?}\n  want: {:?}",
        got.len(),
        want.len(),
        show(got),
        show(want)
    ))
}

/// The DVI preamble's comment of an oracle file.
pub fn dvi_comment(dvi: &[u8]) -> String {
    let k = usize::from(dvi.get(14).copied().unwrap_or(0));
    dvi.get(15..15 + k)
        .map(|b| b.iter().map(|&c| char::from(c)).collect())
        .unwrap_or_default()
}

/// plain.tex as the oracle's plain fixtures load it.
pub fn plain() -> String {
    let s = read(&fixtures().join("dvi/plaintext.tex"));
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = read(&fixtures().join("hyph/hyphen.tex"));
    s[..at].replace(
        "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
        &h,
    )
}
