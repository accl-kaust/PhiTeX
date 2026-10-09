//! The session's shared interner (DESIGN 3.17): where a run's names
//! land depends on the names made before them, so a document run with
//! other names placed first (decoys on its names' hash codes, in two
//! orders) must write the same bytes as without the interner: every
//! name is printed by its characters, and no location reaches an output.
//! Token-list versions count such names by their spellings: two runs
//! whose names landed in different places make lists of equal versions.

#![cfg(feature = "std")]

use std::collections::BTreeMap;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::{Flavor, Params, Tex, Untracked};

#[derive(Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    names: BTreeMap<u32, Vec<u8>>,
    written: BTreeMap<u32, Vec<u8>>,
    next: u32,
}

impl Host for Disk {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut with = name.to_vec();
        with.extend_from_slice(match kind {
            FileKind::Tex => b".tex",
            FileKind::Tfm => b".tfm",
            _ => b"",
        });
        [with, name.to_vec()].into_iter().find_map(|n| {
            let c = self.files.get(&n)?;
            Some(OpenedFile {
                name: [b"./", &n[..]].concat(),
                contents: Arc::from(&c[..]),
            })
        })
    }
    fn open_write(&mut self, name: &[u8], _: FileKind) -> Option<(WriteId, Vec<u8>)> {
        self.next += 1;
        self.names.insert(self.next, name.to_vec());
        self.written.insert(self.next, Vec::new());
        Some((WriteId(self.next), name.to_vec()))
    }
    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        self.written
            .entry(file.0)
            .or_default()
            .extend_from_slice(bytes);
    }
    fn close(&mut self, _: WriteId) {}
    fn term_write(&mut self, _: &[u8]) {}
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn diagnostic(&mut self, _: &Diagnostic) {}
    fn now(&self) -> DateTime {
        DateTime {
            year: 1776,
            month: 7,
            day: 4,
            minutes: 720,
        }
    }
}

/// Names made by `\csname`, `\def` and `\let`, used in the text, in
/// `\write`s to an auxiliary file, printed by `\string` and `\meaning`,
/// tested by `\ifx\csname`, and in a token register.
const DOC: &str = r"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6 \catcode`\@=11 \pdfoutput=1 \pdfcompresslevel=0
\pdfmapline{cmr10 CMR10 <cmr10.pfb}
\font\rm=cmr10 \rm
\hsize=200pt \vsize=400pt \parindent=0pt
\output={\shipout\box255}
\immediate\openout1=doc.aux
\count1=0 \def\space{ }
\def\mk#1{\expandafter\def\csname n@#1\endcsname{#1-\the\count1}\advance\count1 by 1 }
\mk{alpha}\mk{beta}\mk{gamma}\mk{delta}\mk{epsilon}\mk{zeta}\mk{eta}\mk{theta}
\def\use#1{\csname n@#1\endcsname}
\let\aliasa=\mk
\toks0={\use{alpha} \use{beta} \string\aliasa}
\immediate\write1{\expandafter\string\csname n@alpha\endcsname\space\meaning\aliasa}
\immediate\write1{\the\toks0}
\immediate\write1{\expandafter\ifx\csname n@gamma\endcsname\relax no\else yes\fi
 \expandafter\ifx\csname n@none\endcsname\relax no\else yes\fi}
\def\b@one{1}\def\b@two{2}\def\r@x{\b@one\b@two}
\immediate\write1{\meaning\r@x}
\the\toks0. \use{gamma} \use{delta} \r@x \expandafter\string\csname n@zeta\endcsname\par
\penalty-10000
\end
";

/// The names the document makes.
const NAMES: &[&str] = &[
    "n@alpha",
    "n@beta",
    "n@gamma",
    "n@delta",
    "n@epsilon",
    "n@zeta",
    "n@eta",
    "n@theta",
    "mk",
    "use",
    "aliasa",
    "b@one",
    "b@two",
    "r@x",
    "n@none",
    "rm",
    "space",
];

fn host() -> Disk {
    let mut host = Disk::default();
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files
        .insert(b"doc.tex".to_vec(), DOC.as_bytes().to_vec());
    host.files.insert(
        b"cmr10.pfb".to_vec(),
        pfb().expect("TeX Live's cmr10.pfb (kpsewhich cmr10.pfb)"),
    );
    host
}

/// TeX Live's cmr10.pfb: Arch's or Debian's place, else kpsewhich.
fn pfb() -> Option<Vec<u8>> {
    let tail = "fonts/type1/public/amsfonts/cm/cmr10.pfb";
    ["/usr/share/texmf-dist", "/usr/share/texlive/texmf-dist"]
        .iter()
        .find_map(|d| std::fs::read(format!("{d}/{tail}")).ok())
        .or_else(|| {
            let o = std::process::Command::new("kpsewhich")
                .arg("cmr10.pfb")
                .output()
                .ok()?;
            std::fs::read(String::from_utf8(o.stdout).ok()?.trim()).ok()
        })
}

fn params() -> Params {
    Params {
        ini: true,
        flavor: Flavor::PdfTex,
        interaction: Some(0),
        hash_extra: 20_000,
        ..Params::default()
    }
}

/// TeX's hash code of `name` (§261).
fn code(name: &[u8]) -> i32 {
    let mut h = i32::from(name[0]);
    for &c in &name[1..] {
        h = h + h + i32::from(c);
        while h >= 8501 {
            h -= 8501;
        }
    }
    h
}

/// Names with the hash codes of the document's: `k` of them each
/// (`zq`, a letter for the copy, then 14 letters that land the code:
/// their weights are powers of 2, so the digits a binary greedy picks
/// reach any code).
fn decoys(k: usize) -> Vec<Vec<u8>> {
    const N: u32 = 14;
    let mut out = std::collections::BTreeSet::new();
    for n in NAMES {
        let want = code(n.as_bytes());
        for copy in 0..k {
            let mut d = vec![b'z', b'q', b'a' + u8::try_from(copy).expect("a letter")];
            let mut r = i64::from(want) - (i64::from(code(&d)) << N);
            r -= (0..N).map(|i| 97i64 << i).sum::<i64>();
            let mut r = r.rem_euclid(8501);
            for i in (0..N).rev() {
                let digit = (r >> i).min(25);
                r -= digit << i;
                d.push(b'a' + u8::try_from(digit).expect("a letter"));
            }
            assert_eq!(code(&d), want);
            out.insert(d);
        }
    }
    out.into_iter().collect()
}

/// The run's outputs (PDF and `.aux`), and where its names landed.
type Outputs = Vec<(Vec<u8>, Vec<u8>)>;

fn run(shared: bool, decoys: &[Vec<u8>]) -> (Outputs, Vec<i32>) {
    let mut tex = Tex::new(host(), Untracked, params());
    tex.set_effects(false);
    let mut places = Vec::new();
    if shared {
        tex.share_names_after_format(decoys.to_vec());
    }
    let h = tex.run(b"doc");
    if h > 1 {
        let d = tex.host();
        let log: String = d
            .names
            .iter()
            .filter(|(_, n)| n.ends_with(b".log"))
            .map(|(i, _)| String::from_utf8_lossy(&d.written[i]).into_owned())
            .collect();
        panic!("the run failed: {h}\n{log}");
    }
    if shared {
        for n in NAMES {
            places.push(tex.intern_name(n.as_bytes()).expect("placed"));
        }
    }
    partex_core::interner::SharedNames::leave();
    let d = tex.host();
    let outs = d
        .names
        .iter()
        .filter(|(_, n)| !n.ends_with(b".log"))
        .map(|(i, n)| (n.clone(), d.written[i].clone()))
        .collect();
    (outs, places)
}

#[test]
fn a_documents_bytes_do_not_depend_on_where_its_names_landed() {
    let (plain, _) = run(false, &[]);
    assert!(plain.iter().any(|(n, _)| n.ends_with(b".pdf")));
    assert!(
        plain
            .iter()
            .any(|(n, b)| n.ends_with(b".aux") && !b.is_empty())
    );
    let (shared, p0) = run(true, &[]);
    let mut ds = decoys(2);
    let (first, p1) = run(true, &ds);
    ds.reverse();
    let (second, _) = run(true, &ds);
    let (third, _) = run(true, &decoys(3));
    let (fourth, p4) = run(true, &partex_core::interner::random_names(7, 4000));
    // (the names did land elsewhere)
    assert_ne!(p0, p1);
    assert_ne!(p0, p4);
    for (what, o) in [
        ("shared", &shared),
        ("decoys", &first),
        ("decoys reversed", &second),
        ("more decoys", &third),
        ("random decoys", &fourth),
    ] {
        assert_eq!(o.len(), plain.len(), "{what}: the files written");
        for ((n, a), (m, b)) in plain.iter().zip(o.iter()) {
            assert_eq!(n, m);
            assert!(
                a == b,
                "{what}: {} differs:\n{}\n---\n{}",
                String::from_utf8_lossy(n),
                String::from_utf8_lossy(&a[..a.len().min(2000)]),
                String::from_utf8_lossy(&b[..b.len().min(2000)])
            );
        }
    }
}
