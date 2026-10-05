//! A step that runs the page builder reads the nest: whether there is an
//! outer level decides where the contributions are (`Tex::nest_at`,
//! `Tex::contrib`). With the nest empty, `build_page` read `cur_list`'s
//! fields and not the nest, and a rebuild that took such a step's reads
//! to predict a new step's (the PGF manual's table of contents, read for
//! the first time) placed the fields, a paragraph's, over the job's end's
//! empty nest: `pop_nest` found none.
//!
//! In a process of its own, as `view.rs`: an SSA build sets switches the
//! whole process shares.

use std::collections::BTreeMap;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::ssa::{Fam, Recorder, Slot, SsaTracker, run};
use partex_core::{Params, Tex};

/// Files in memory, the outputs dropped.
#[derive(Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, Vec<u8>>,
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
        Some((WriteId(self.next), name.to_vec()))
    }
    fn write(&mut self, _: WriteId, _: &[u8]) {}
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

/// For INITEX: a penalty in vertical mode, a step of its own that runs
/// the page builder with no paragraph (no `push_nest` to read the nest),
/// then a paragraph and the end.
const DOC: &str = r"\catcode`\{=1 \catcode`\}=2
\font\rm=cmr10 \rm \hsize=100pt \vsize=100pt

\penalty0

A paragraph.

\end
";

#[test]
fn the_page_builder_reads_the_nest() {
    let mut host = Disk::default();
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files
        .insert(b"doc.tex".to_vec(), DOC.as_bytes().to_vec());
    let params = Params {
        ini: true,
        interaction: Some(0), // (batch mode, §73)
        ..Params::default()
    };
    let mut tex = Tex::new(host, SsaTracker::new(Recorder::new()), params);
    let rep = run(&mut tex, b"doc", false, 0);
    assert!(rep.history <= 1, "the build failed: {}", rep.history);
    let rec = tex.tracker().rec.borrow();
    let fold = &rec.rt.fold;
    let nest = Slot(Fam::List, i64::from(partex_core::track::list::COUNT));
    let mut builders = 0;
    for &s in &fold.order {
        let reads: Vec<Slot> = fold.reads_of(s).copied().collect();
        if reads.iter().any(|a| a.0 == Fam::Page) {
            builders += 1;
            assert!(
                reads.contains(&nest),
                "step {s} ran the page builder and did not read the nest"
            );
        }
    }
    // (the penalty's step, the paragraph's and the end's)
    assert!(builders >= 2, "{builders} steps ran the page builder");
}
