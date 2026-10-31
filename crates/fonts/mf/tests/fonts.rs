//! Fonts made as TeX Live's mktexpk makes them, compared byte for byte
//! with TeX Live 2026's own mf, gftopk and TFM output (tests/bless.sh):
//! `logo10` at 600 dpi from the files in tests/fonts (always), and every
//! font of bless.sh's list when `PHITEX_MF_ORACLE` names its output
//! directory (`.../mf`, with `fonts/` and `mfbase/`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use phitex_mf::mktex::{self, Settings};
use phitex_mf::{FileKind, Io};

/// Files by name from directories, as kpathsea finds `.mf` sources.
#[derive(Clone)]
struct Dirs(HashMap<Vec<u8>, PathBuf>);

impl Dirs {
    fn new(dirs: &[PathBuf]) -> Dirs {
        let mut m = HashMap::new();
        for d in dirs.iter().rev() {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                m.insert(e.file_name().into_encoded_bytes(), e.path());
            }
        }
        Dirs(m)
    }
}

impl Io for Dirs {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut tries = vec![];
        if kind == FileKind::Mf && !name.ends_with(b".mf") {
            let mut n = name.to_vec();
            n.extend_from_slice(b".mf");
            tries.push(n);
        }
        tries.push(name.to_vec());
        tries.into_iter().find_map(|n| {
            let p = self.0.get(&n)?;
            Some((
                std::fs::read(p).ok()?,
                p.to_string_lossy().into_owned().into_bytes(),
            ))
        })
    }
}

fn settings() -> Settings {
    Settings {
        version_string: " (TeX Live 2026/Arch Linux)".into(),
        ..Settings::default()
    }
}

fn base(dirs: &Dirs) -> Vec<u8> {
    let tcx = dirs
        .0
        .get(b"cp227.tcx".as_slice())
        .map(|p| std::fs::read(p).unwrap());
    mktex::build_base(
        Box::new(dirs.clone()),
        tcx.as_deref().map(|t| ("cp227.tcx", t)),
        &settings(),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn logo10() {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fonts");
    let dirs = Dirs::new(std::slice::from_ref(&d));
    let base = base(&dirs);
    let made = mktex::mktexpk(Box::new(dirs), &base, "logo10", 600, 600, &settings())
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        made.gf == std::fs::read(d.join("logo10.600gf")).unwrap(),
        "logo10.600gf differs"
    );
    assert!(
        made.pk == std::fs::read(d.join("logo10.600pk")).unwrap(),
        "logo10.600pk differs"
    );
}

#[test]
fn oracle_fonts() {
    let Some(o) = std::env::var_os("PHITEX_MF_ORACLE") else {
        eprintln!("PHITEX_MF_ORACLE not set: the font list is not checked");
        return;
    };
    let o = Path::new(&o);
    let fonts = o.join("fonts");
    let dirs = Dirs::new(&[o.join("mfbase"), fonts.join("src")]);
    let base = base(&dirs);
    let list = std::fs::read_to_string(fonts.join("list.txt")).unwrap();
    let (mut ok, mut bad) = (0, Vec::new());
    for line in list.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        let (name, mag, dpi) = (w[0], w[1], w[2]);
        let line = format!("\\mode:=ljfour; mag:={mag}; nonstopmode; input {name}");
        let r = mktex::font_run(Box::new(dirs.clone()), &base, &line, &settings());
        let mut diffs = vec![];
        for (ours, theirs) in [
            (
                r.output(&format!("{name}.{dpi}gf")).map(<[u8]>::to_vec),
                format!("{name}.{dpi}gf"),
            ),
            (
                r.output(&format!("{name}.tfm")).map(<[u8]>::to_vec),
                format!("{name}.tfm"),
            ),
            (
                r.output(&format!("{name}.{dpi}gf"))
                    .and_then(|g| phitex_gftopk::gf_to_pk(g).ok()),
                format!("{name}.{dpi}pk"),
            ),
        ] {
            let t = std::fs::read(fonts.join(&theirs)).ok();
            if ours != t {
                diffs.push(theirs);
            }
        }
        if diffs.is_empty() {
            ok += 1;
        } else {
            bad.push(format!("{name} {mag}: {}", diffs.join(" ")));
        }
    }
    eprintln!("{ok} identical, {} differ", bad.len());
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
