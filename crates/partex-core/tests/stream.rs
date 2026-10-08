//! A build in slices, its pages as they are shipped (DESIGN 4.10): the
//! job run a few commands at a time is the job run at once, plain and SSA;
//! a page shipped and drawn from its own PDF has the hash the finished
//! PDF's page has when nothing but its stream draws it; and the font
//! files named ahead (`Host::will_need`) change nothing written.
//!
//! In a process of its own, as `view.rs`: an SSA build sets switches the
//! whole process shares.

use std::collections::BTreeMap;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::effects::Effect;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::pagepdf::{Shipments, ShippedStream};
use partex_core::ssa::{ColdRun, Recorder, SsaTracker, run, step_effects, take_step_changes};
use partex_core::{Flavor, Params, Sequential, Step, Tex, Untracked};

/// Files in memory, the outputs kept by name; the streams shipped and the
/// hints given, if asked for.
#[derive(Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    names: BTreeMap<u32, Vec<u8>>,
    written: BTreeMap<u32, Vec<u8>>,
    next: u32,
    streams: bool,
    shipped: Shipments,
    hints: Option<Vec<(Vec<u8>, FileKind)>>,
    term: Vec<u8>,
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
    fn term_write(&mut self, b: &[u8]) {
        self.term.extend_from_slice(b);
    }
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
    fn wants_streams(&self) -> bool {
        self.streams
    }
    fn stream_shipped(&mut self, page: Option<usize>, stream: ShippedStream) {
        self.shipped.add(page, stream);
    }
    fn wants_hints(&self) -> bool {
        self.hints.is_some()
    }
    fn will_need(&mut self, files: &[(Vec<u8>, FileKind)]) {
        if let Some(h) = &mut self.hints {
            h.extend_from_slice(files);
        }
    }
}

/// Three pages: text in a mapped font (its program a file: a name to
/// hint), a form; text alone; an annotation (not drawn from its stream).
const DOC: &str = r"\catcode`\{=1 \catcode`\}=2 \pdfoutput=1 \pdfcompresslevel=0
\pdfmapline{cmr10 CMR10 <cmr10.pfb}
\font\rm=cmr10 \rm
\hsize=100pt \vsize=40pt \parindent=0pt
\output={\shipout\box255}
\setbox0\hbox{form}\pdfxform0 \def\f{\pdfrefxform\pdflastxform}
Hello. \f\par \penalty-10000
World.\par \penalty-10000
\pdfannot width 10pt height 10pt depth 0pt {/Subtype /Text /Contents (x)}\par
\penalty-10000
\end
";

fn host(streams: bool, hints: bool) -> Disk {
    let mut host = Disk {
        streams,
        hints: hints.then(Vec::new),
        ..Disk::default()
    };
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files.insert(b"cmr10.pfb".to_vec(), pfb().to_vec());
    host.files
        .insert(b"doc.tex".to_vec(), DOC.as_bytes().to_vec());
    host
}

/// TeX Live's `cmr10.pfb` (the PDF embeds the font's program).
fn pfb() -> &'static [u8] {
    static PFB: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    PFB.get_or_init(|| {
        std::fs::read("/usr/share/texmf-dist/fonts/type1/public/amsfonts/cm/cmr10.pfb")
            .expect("TeX Live's cmr10.pfb")
    })
}

fn params() -> Params {
    Params {
        ini: true,
        flavor: Flavor::PdfTex,
        interaction: Some(0), // (batch mode, §73)
        ..Params::default()
    }
}

/// The log the host was written.
fn log(d: &Disk) -> String {
    d.names
        .iter()
        .filter(|(_, n)| n.ends_with(b".log"))
        .map(|(i, _)| String::from_utf8_lossy(&d.written[i]).into_owned())
        .collect()
}

/// The PDF the host was written.
fn pdf_of(d: &Disk) -> Vec<u8> {
    let id = d
        .names
        .iter()
        .find(|(_, n)| n.ends_with(b".pdf"))
        .map(|(i, _)| *i)
        .expect("a PDF file opened");
    d.written[&id].clone()
}

/// A plain run, at once (`slice` 0) or `slice` commands at a time.
fn plain(slice: u64, streams: bool, hints: bool) -> Disk {
    let mut tex = Tex::new(host(streams, hints), Untracked, params());
    tex.set_effects(false);
    let h = if slice == 0 {
        tex.run(b"doc")
    } else {
        tex.set_stop_at(slice);
        let mut step = tex.start(b"doc");
        let mut slices = 0;
        let h = loop {
            match step {
                Step::Checkpoint => {
                    tex.set_stop_at(tex.commands() + slice);
                    slices += 1;
                    step = tex.resume();
                }
                Step::Finished(h) => break h,
            }
        };
        assert!(slices > 3, "only {slices} slices");
        h
    };
    assert!(h <= 1, "the plain run failed: {h}\n{}", log(tex.host()));
    std::mem::take(tex.host_mut())
}

/// An SSA build linked, its trip run at once (`slice` 0) or in slices.
fn ssa(slice: u64, streams: bool, hints: bool) -> (Vec<u8>, Disk) {
    let mut tex = Tex::new(
        host(streams, hints),
        SsaTracker::new(Recorder::new()),
        params(),
    );
    let rep = if slice == 0 {
        run(&mut tex, b"doc", false, 0)
    } else {
        let mut r = ColdRun::start(&mut tex, b"doc", false, 0, false);
        let mut slices = 0;
        while !r.step(&mut tex, slice) {
            slices += 1;
        }
        assert!(slices > 3, "only {slices} slices");
        r.finish(&mut tex)
    };
    assert!(
        rep.history <= 1,
        "the build failed: {}\n{}",
        rep.history,
        log(tex.host())
    );
    take_step_changes(&mut tex.tracker().rec.borrow_mut());
    let chunks = step_effects(&tex.tracker().rec.borrow());
    let fx: Vec<&[Effect]> = chunks.iter().map(|(_, e)| &e.1[..]).collect();
    let linked = partex_core::effects::link(&fx, &Sequential, &mut |_, _| None)
        .unwrap_or_else(|e| panic!("the link failed: {e:?}"));
    let (id, _, _) = linked
        .opened
        .iter()
        .find(|(_, n, _)| n.ends_with(b".pdf"))
        .expect("a PDF file opened");
    (linked.files[&id.0].clone(), std::mem::take(tex.host_mut()))
}

/// Each page's hash as the finished PDF has it.
fn hashes(pdf: &[u8]) -> Vec<u64> {
    phitex_draw::pdfdraw::hashes(&Arc::from(pdf))
}

/// Each page shipped: its own PDF's hash, and whether it is whole.
fn shipped_hashes(s: &Shipments) -> Vec<(u64, bool)> {
    (0..s.pages())
        .map(|k| {
            let (pdf, whole) = s.page_pdf(k, &mut |_, _| None).expect("page shipped");
            (hashes(&pdf)[0], whole)
        })
        .collect()
}

#[test]
fn a_plain_run_in_slices_is_the_run() {
    let once = pdf_of(&plain(0, false, false));
    assert!(once.starts_with(b"%PDF-"), "not a PDF file");
    for slice in [1, 2, 7] {
        let d = plain(slice, true, false);
        assert!(pdf_of(&d) == once, "{slice} commands a slice: another PDF");
        assert_eq!(d.shipped.pages(), 3);
    }
}

#[test]
fn an_ssa_trip_in_slices_is_the_trip() {
    let (once, _) = ssa(0, false, false);
    let (sliced, d) = ssa(5, true, false);
    assert!(sliced == once, "the sliced trip's PDF is not the trip's");
    assert_eq!(d.shipped.pages(), 3);
    // (and the plain run's)
    assert!(
        once == pdf_of(&plain(0, false, false)),
        "SSA's PDF is not plain's"
    );
}

/// A page drawn from its stream alone is hashed as the PDF's page is; a
/// page its stream does not draw whole (an annotation) is not.
#[test]
fn whole_pages_shipped_keep_their_hash() {
    let d = plain(0, true, false);
    let fin = hashes(&pdf_of(&d));
    let got = shipped_hashes(&d.shipped);
    assert_eq!(got.len(), fin.len());
    assert_eq!(
        got.iter().map(|g| g.1).collect::<Vec<_>>(),
        [true, true, false]
    );
    for (k, (h, whole)) in got.iter().enumerate() {
        if *whole {
            assert_eq!(*h, fin[k], "page {}: shipped hash differs", k + 1);
        }
    }
    // (an SSA build's pages too)
    let (pdf, d) = ssa(0, true, false);
    let fin = hashes(&pdf);
    for (k, (h, whole)) in shipped_hashes(&d.shipped).iter().enumerate() {
        if *whole {
            assert_eq!(*h, fin[k], "SSA page {}: shipped hash differs", k + 1);
        }
    }
}

/// The font files named ahead: the map's encoding, once the map line is
/// read and the font loaded; and nothing written changes.
#[test]
fn hints_change_nothing() {
    let quiet = plain(0, false, false);
    let told = plain(0, false, true);
    let hints = told.hints.clone().unwrap();
    assert!(
        hints.contains(&(b"cmr10.pfb".to_vec(), FileKind::Type1)),
        "hints {hints:?}"
    );
    assert!(pdf_of(&quiet) == pdf_of(&told), "a hint changed the PDF");
    assert_eq!(quiet.written, told.written);
    let (a, _) = ssa(0, false, false);
    let (b, d) = ssa(0, false, true);
    assert!(a == b, "a hint changed the SSA build's PDF");
    assert!(
        d.hints
            .unwrap()
            .contains(&(b"cmr10.pfb".to_vec(), FileKind::Type1))
    );
}
