//! The job's end in the step that shipped the last page (SSA mode,
//! virtual object numbers). plain's `\bye` is `\par\vfill\supereject\end`:
//! the `\supereject` fires the output routine, which ships the page, and
//! the `\end` comes in the same expansion, so the job ends in the fire's
//! step. The objects a step makes are named by its count
//! (`1 + (step << 12 | count)`), and the end of the job began the count
//! again (machine mode's reset): the catalog and the pages tree were
//! named as the page's own objects, and the link failed with
//! `Unplaced { file: WriteId(1), num: 1 }` on every plain document.
//!
//! In a process of its own, as `view.rs`: an SSA build sets switches the
//! whole process shares.

use std::collections::BTreeMap;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::effects::Effect;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::ssa::{Recorder, SsaTracker, run, step_effects, take_step_changes};
use partex_core::{Flavor, Params, Sequential, Tex, Untracked};

/// Files in memory, the outputs kept by name.
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

fn host(doc: &str) -> Disk {
    let mut host = Disk::default();
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files
        .insert(b"doc.tex".to_vec(), doc.as_bytes().to_vec());
    host
}

fn params() -> Params {
    Params {
        ini: true,
        flavor: Flavor::PdfTex,
        interaction: Some(0), // (batch mode, §73)
        ..Params::default()
    }
}

/// The PDF file of `doc` built in SSA mode and linked from its steps.
fn ssa_pdf(doc: &str) -> Vec<u8> {
    let mut tex = Tex::new(host(doc), SsaTracker::new(Recorder::new()), params());
    let rep = run(&mut tex, b"doc", false, 0);
    assert!(rep.history <= 1, "the build failed: {}", rep.history);
    // (the steps' text rendered from the columns, as at every link)
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
    linked.files[&id.0].clone()
}

/// The PDF file of `doc` as the engine writes it with no tracker.
fn plain_pdf(doc: &str) -> Vec<u8> {
    let mut tex = Tex::new(host(doc), Untracked, params());
    tex.set_effects(false);
    let h = tex.run(b"doc");
    assert!(h <= 1, "the plain run failed: {h}");
    let d = tex.host();
    let id = d
        .names
        .iter()
        .find(|(_, n)| n.ends_with(b".pdf"))
        .map(|(i, _)| *i)
        .expect("a PDF file opened");
    d.written[&id].clone()
}

fn same_pdf(doc: &str) {
    let (ssa, plain) = (ssa_pdf(doc), plain_pdf(doc));
    assert!(ssa.starts_with(b"%PDF-"), "not a PDF file");
    assert!(
        ssa == plain,
        "the SSA build's PDF file is not the plain run's"
    );
}

/// The smallest form: a page shipped and the job ended in one macro's
/// expansion, so in one step.
#[test]
fn the_job_ends_in_the_step_that_shipped() {
    same_pdf(
        r"\catcode`\{=1 \catcode`\}=2 \pdfoutput=1
\def\foo{\shipout\hbox{}\end}
\foo
",
    );
}

/// plain's form: the output routine fired by `\bye`'s penalty ships the
/// page, and its `\end` follows in the same step; three pages (rules: no
/// font file to embed).
#[test]
fn bye_after_the_output_routine() {
    same_pdf(
        r"\catcode`\{=1 \catcode`\}=2 \pdfoutput=1
\hsize=100pt \vsize=20pt
\output={\shipout\box255}
\def\bye{\par\vfill\penalty-20000 \end}
\hrule height 15pt \hrule height 15pt \hrule height 15pt
\bye
",
    );
}
