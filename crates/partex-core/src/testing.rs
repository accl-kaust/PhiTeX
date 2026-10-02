//! A recording [`Host`] and helpers for unit tests.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use crate::params::Params;
use crate::tex::Tex;
use crate::track::Untracked;

#[derive(Default)]
pub struct TestHost {
    pub files: BTreeMap<Vec<u8>, Vec<u8>>,
    pub written: BTreeMap<u32, Vec<u8>>,
    pub term: Vec<u8>,
    pub diagnostics: Vec<crate::diag::Diagnostic>,
    next: u32,
}

impl Host for TestHost {
    /// Like kpathsea in the current directory: try the name with the
    /// kind's suffix, then as given; report it as `./name`.
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut candidates = alloc::vec![name.to_vec()];
        let suffix: &[u8] = match kind {
            FileKind::Tex => b".tex",
            FileKind::Tfm => b".tfm",
            _ => b"",
        };
        if !suffix.is_empty() {
            let mut n = name.to_vec();
            n.extend_from_slice(suffix);
            candidates.insert(0, n);
        }
        candidates.into_iter().find_map(|n| {
            self.files.get(&n).map(|c| {
                let mut shown = b"./".to_vec();
                shown.extend_from_slice(&n);
                OpenedFile {
                    name: shown,
                    contents: Arc::from(&c[..]),
                }
            })
        })
    }
    fn open_write(&mut self, name: &[u8], _: FileKind) -> Option<(WriteId, Vec<u8>)> {
        self.next += 1;
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
    fn term_write(&mut self, bytes: &[u8]) {
        self.term.extend_from_slice(bytes);
    }
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn diagnostic(&mut self, d: &crate::diag::Diagnostic) {
        self.diagnostics.push(d.clone());
    }
    fn now(&self) -> DateTime {
        DateTime {
            year: 1776,
            month: 7,
            day: 4,
            minutes: 720,
        }
    }
}

/// An engine with trip parameters, character set and string pool initialized.
pub fn engine() -> Tex<TestHost, Untracked> {
    engine_with(Untracked)
}

/// [`engine`] with tracker `tracker`.
pub fn engine_with<T: crate::track::Tracker>(tracker: T) -> Tex<TestHost, T> {
    let mut t = Tex::new(TestHost::default(), tracker, Params::trip());
    t.init_charset();
    t.init_output();
    assert!(t.get_strings_started().unwrap());
    t.init_eqtb();
    t.init_xeq_level();
    t.init_hash();
    t.init_nest();
    t
}

/// Run `f` with `selector = term_only` and return what reached the terminal.
pub fn term_output(
    t: &mut Tex<TestHost, Untracked>,
    f: impl FnOnce(&mut Tex<TestHost, Untracked>),
) -> Vec<u8> {
    t.selector = crate::print::TERM_ONLY;
    f(t);
    t.update_terminal();
    core::mem::take(&mut t.host.term)
}

/// Make `src` the only input: a file at the bottom of an empty input
/// stack, about to read its first line.
pub fn feed<T: crate::track::Tracker>(t: &mut Tex<TestHost, T>, src: &[u8]) {
    t.input_ptr = 0;
    t.in_open = 0;
    t.first = 1;
    t.begin_file_reading().unwrap();
    t.input_file[t.in_open] = Some(crate::input::AlphaFile {
        data: Arc::from(src),
        ..Default::default()
    });
    t.cur_input.name = 20; // a file (18 and 19 are e-TeX's pseudo files)
    t.cur_input.state = crate::web::NEW_LINE;
    t.cur_input.loc = 1;
    t.cur_input.limit = 0;
}
