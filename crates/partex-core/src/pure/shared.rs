//! One host for every engine of a pure SSA build: the build's thread's
//! and each worker's (DESIGN 3.17: per-worker copy-on-write views of the
//! format's state share the host). Every call goes to the host behind a
//! lock; a worker's step, in effects mode, calls it seldom (files read,
//! names made).

use alloc::sync::Arc;
use alloc::vec::Vec;
use std::sync::Mutex;

use crate::host::{DateTime, FileKind, Host, Load, Memo, OpenedFile, PageSink, Ran, WriteId};
use partex_engine::pageir::Page;

/// A host shared by the build's engines.
pub struct Shared<H>(pub Arc<Mutex<H>>);

impl<H> Clone for Shared<H> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

impl<H> Shared<H> {
    /// The host, locked.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, H> {
        self.0.lock().expect("pure SSA: the host")
    }
}

impl<H: Host> Host for Shared<H> {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        self.lock().read_file(name, kind)
    }
    fn unchanged(&mut self, loads: &[Load<'_>]) -> Vec<bool> {
        self.lock().unchanged(loads)
    }
    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        self.lock().open_write(name, kind)
    }
    fn open_write_again(
        &mut self,
        name: &[u8],
        kind: FileKind,
        id: WriteId,
    ) -> Option<(WriteId, Vec<u8>)> {
        self.lock().open_write_again(name, kind, id)
    }
    fn written_name(&mut self, name: &[u8]) -> Vec<u8> {
        self.lock().written_name(name)
    }
    fn output_edited(&mut self, name: &[u8]) -> bool {
        self.lock().output_edited(name)
    }
    fn open_write_later(
        &mut self,
        name: &[u8],
        kind: FileKind,
        again: Option<WriteId>,
    ) -> Option<(WriteId, Vec<u8>)> {
        self.lock().open_write_later(name, kind, again)
    }
    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        self.lock().write(file, bytes);
    }
    fn close(&mut self, file: WriteId) {
        self.lock().close(file);
    }
    fn close_pipe(&mut self, file: WriteId) -> i32 {
        self.lock().close_pipe(file)
    }
    fn term_write(&mut self, bytes: &[u8]) {
        self.lock().term_write(bytes);
    }
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        self.lock().term_read_line()
    }
    fn now(&self) -> DateTime {
        self.lock().now()
    }
    fn creation_date(&mut self) -> Vec<u8> {
        self.lock().creation_date()
    }
    fn file_mod_date(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.lock().file_mod_date(name)
    }
    fn seconds_and_micros(&mut self) -> (i32, i32) {
        self.lock().seconds_and_micros()
    }
    fn diagnostic(&mut self, diagnostic: &crate::diag::Diagnostic) {
        self.lock().diagnostic(diagnostic);
    }
    fn notes(&self) -> bool {
        self.lock().notes()
    }
    fn shipping(&mut self, count0: i32) {
        self.lock().shipping(count0);
    }
    fn page_written(&mut self, page: &Page) {
        self.lock().page_written(page);
    }
    fn wants_streams(&self) -> bool {
        self.lock().wants_streams()
    }
    fn stream_shipped(&mut self, page: Option<usize>, stream: crate::pagepdf::ShippedStream) {
        self.lock().stream_shipped(page, stream);
    }
    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        self.lock().deflate(level, data)
    }
    fn cached(&mut self, key: u128) -> Option<Memo> {
        self.lock().cached(key)
    }
    fn cache(&mut self, key: u128, value: Memo) {
        self.lock().cache(key, value);
    }
    fn font_slot(&mut self, ident: u128, fresh: i32) -> i32 {
        self.lock().font_slot(ident, fresh)
    }
    fn page_sink(&mut self) -> Option<&mut dyn PageSink> {
        // (a sink is the host's own, not to be lent out of the lock: the
        // DVI writer then writes through the effects)
        None
    }
    fn system(&mut self, command: &[u8], inputs: &[(Vec<u8>, Arc<[u8]>)]) -> Option<Ran> {
        self.lock().system(command, inputs)
    }
    fn out_name_ok(&mut self, name: &[u8]) -> bool {
        self.lock().out_name_ok(name)
    }
    fn runs_commands(&self) -> bool {
        self.lock().runs_commands()
    }
    fn cache_get(&mut self, key: u128) -> Option<Vec<u8>> {
        self.lock().cache_get(key)
    }
    fn cache_put(&mut self, key: u128, value: &[u8]) {
        self.lock().cache_put(key, value);
    }
    fn synctex_name(&mut self, found: &[u8]) -> Vec<u8> {
        self.lock().synctex_name(found)
    }
    fn remove_output(&mut self, name: &[u8]) {
        self.lock().remove_output(name);
    }
    fn output_name(&mut self, name: &[u8], kind: FileKind) -> Vec<u8> {
        self.lock().output_name(name, kind)
    }
}
