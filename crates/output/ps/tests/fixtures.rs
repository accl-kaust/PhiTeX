//! The converter against Ghostscript 10.08.0 (the CI image's): each
//! fixture `X.eps` with gs's PDFs through epstopdf (`X-e2p.pdf`) and the
//! D line (`X-dpx.pdf`), byte for byte but for the trailer's `/ID` and
//! the XMP `DocumentID` (gs hashes its process state into them). A
//! fixture without PDFs uses what the converter does not write as gs
//! does: it must give no PDF and say why. Fonts: Ghostscript's
//! `Resource/Font` (`PHITEX_GS_FONTS`, else the system's gs).

use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Dir(PathBuf);

impl phitex_ps::Resources for Dir {
    fn font(&self, name: &[u8]) -> Option<Rc<Vec<u8>>> {
        let n = std::str::from_utf8(name).ok()?;
        if n.contains('/') {
            return None;
        }
        std::fs::read(self.0.join(n)).ok().map(Rc::new)
    }
}

fn fonts() -> Option<PathBuf> {
    let d = std::env::var_os("PHITEX_GS_FONTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/ghostscript/Resource/Font"));
    d.join("NimbusRoman-Regular").is_file().then_some(d)
}

/// The file with `/ID [...]` and the DocumentID's uuid blanked.
fn norm(d: &[u8]) -> Vec<u8> {
    let mut d = d.to_vec();
    if let Some(i) = find(&d, b"/ID [")
        && let Some(j) = find(&d[i..], b"]\n/Index")
    {
        d.splice(i..i + j, b"/ID [X".iter().copied());
    }
    if let Some(i) = find(&d, b"DocumentID='uuid:") {
        let s = i + 17;
        let e = (s + 36).min(d.len());
        for b in &mut d[s..e] {
            *b = b'X';
        }
    }
    d
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

#[test]
fn eps_fixtures_as_ghostscript() {
    let Some(fonts) = fonts() else {
        eprintln!("eps_fixtures_as_ghostscript: no Ghostscript Resource/Font (PHITEX_GS_FONTS): skipped");
        return;
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let epoch = 1_758_800_000;
    let mut bad = Vec::new();
    let mut n = 0;
    let mut eps: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "eps"))
        .collect();
    eps.sort();
    for f in eps {
        let b = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
        let input = std::fs::read(&f).expect("eps");
        for mode in ["e2p", "dpx"] {
            let out = if mode == "e2p" {
                phitex_ps::convert(&phitex_ps::epstopdf_feed(&input), phitex_ps::Settings::epstopdf(epoch), &Dir(fonts.clone()))
            } else {
                phitex_ps::convert(&input, phitex_ps::Settings::dvipdfmx(15, epoch), &Dir(fonts.clone()))
            };
            n += 1;
            match std::fs::read(dir.join(format!("{b}-{mode}.pdf"))) {
                Ok(want) => {
                    if out.error.is_some() || norm(&out.pdf) != norm(&want) {
                        bad.push(format!("{b} {mode}: {}", out.error.unwrap_or_else(|| "differs".into())));
                    }
                }
                Err(_) => {
                    if out.error.is_none() || !out.pdf.is_empty() {
                        bad.push(format!("{b} {mode}: unsupported, yet converted"));
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} of {n} conversions differ from gs's:\n{}", bad.len(), bad.join("\n"));
}
