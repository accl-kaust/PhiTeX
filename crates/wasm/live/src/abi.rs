//! The host's side of the module: its imports (`phitex.*`), wrapped as
//! safe functions, and the `ph_*` exports, each turning the host's
//! pointers into slices and calling the crate's safe function of that
//! name. Every pointer the host passes is a range it got from `ph_alloc`
//! and wrote; every pointer this passes out is read by the host before
//! its import returns. Every export but `ph_step` (and the memory's)
//! runs as a host's turn ([`session::host_turn`]): the build's yields
//! are made on `ph_step`'s stack alone.

use std::alloc::Layout;

use crate::session::{self, OUTPUT, lock};

#[link(wasm_import_module = "phitex")]
unsafe extern "C" {
    /// Asks the host for the file `name` (UTF-8): it answers with
    /// `ph_provide`, from its thread 0, later.
    #[link_name = "want"]
    fn host_want(name: *const u8, len: usize);
    /// Asks the host for Shelf's pack `id`: it answers with `ph_pack`.
    #[link_name = "fetch"]
    fn host_fetch(id: *const u8, len: usize);
    /// Posts `\write18` call `id` to the host (its frame, shell.rs): it
    /// answers with `ph_shell_done`.
    #[link_name = "shell"]
    fn host_shell(id: u32, frame: *const u8, len: usize);
    /// Asks the host for the bytes it stored under `key`: it answers with
    /// `ph_stored`.
    #[link_name = "store_get"]
    fn host_store_get(key: *const u8, len: usize);
    /// Gives the host bytes to store under `key` (it copies them before
    /// returning).
    #[link_name = "store_put"]
    fn host_store_put(key: *const u8, key_len: usize, data: *const u8, data_len: usize);
    /// Tells the host of an event: JSON.
    #[link_name = "event"]
    fn host_event(json: *const u8, len: usize);
    /// The language server's message to the editor: JSON-RPC, unframed.
    #[link_name = "lsp"]
    fn host_lsp(json: *const u8, len: usize);
}

pub fn want(name: &str) {
    // SAFETY: the host reads `len` bytes at `name` before returning.
    unsafe { host_want(name.as_ptr(), name.len()) }
}

pub fn fetch(pack: &str) {
    // SAFETY: as `want`.
    unsafe { host_fetch(pack.as_ptr(), pack.len()) }
}

pub fn shell(id: u32, frame: &[u8]) {
    // SAFETY: the host reads the frame before returning.
    unsafe { host_shell(id, frame.as_ptr(), frame.len()) }
}

pub fn store_get(key: &str) {
    // SAFETY: as `want`.
    unsafe { host_store_get(key.as_ptr(), key.len()) }
}

pub fn store_put(key: &str, data: &[u8]) {
    // SAFETY: the host copies both before returning.
    unsafe { host_store_put(key.as_ptr(), key.len(), data.as_ptr(), data.len()) }
}

pub fn event(json: &str) {
    // SAFETY: as `want`.
    unsafe { host_event(json.as_ptr(), json.len()) }
}

pub fn lsp(json: &str) {
    // SAFETY: as `want`.
    unsafe { host_lsp(json.as_ptr(), json.len()) }
}

/// The bytes `p..p + n` of the module's memory, which the host wrote.
fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 {
        return &[];
    }
    // SAFETY: the host passes a range it got from `ph_alloc` and wrote,
    // and frees it only after the call that reads it returns.
    unsafe { std::slice::from_raw_parts(p, n) }
}

fn text(p: *const u8, n: usize) -> String {
    String::from_utf8_lossy(bytes(p, n)).into_owned()
}

/// The layout of the host's `n` bytes (never zero-sized).
fn layout(n: usize) -> Layout {
    Layout::array::<u8>(n.max(1)).expect("a size the host can have")
}

#[unsafe(no_mangle)]
pub extern "C" fn ph_alloc(n: usize) -> *mut u8 {
    // SAFETY: the layout is not zero-sized.
    unsafe { std::alloc::alloc(layout(n)) }
}

#[unsafe(no_mangle)]
pub extern "C" fn ph_free(p: *mut u8, n: usize) {
    // SAFETY: `p` came from `ph_alloc(n)`, the same layout.
    unsafe { std::alloc::dealloc(p, layout(n)) }
}

#[unsafe(no_mangle)]
pub extern "C" fn ph_output_ptr() -> *const u8 {
    lock(&OUTPUT).as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn ph_output_len() -> usize {
    lock(&OUTPUT).len()
}

/// [`session::provide`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_provide(
    name: *const u8,
    name_len: usize,
    path: *const u8,
    path_len: usize,
    data: *const u8,
    data_len: usize,
    found: i32,
) {
    session::host_turn(|| {
        let file = (found != 0).then(|| (text(path, path_len), bytes(data, data_len).to_vec()));
        session::provide(&text(name, name_len), file);
    });
}

/// [`session::pack`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_pack(id: *const u8, id_len: usize, data: *const u8, data_len: usize, ok: i32) {
    session::host_turn(|| {
        session::pack(&text(id, id_len), (ok != 0).then(|| bytes(data, data_len)));
    });
}

/// [`session::stored`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_stored(
    key: *const u8,
    key_len: usize,
    data: *const u8,
    data_len: usize,
    found: i32,
) {
    session::host_turn(|| {
        session::stored(
            &text(key, key_len),
            (found != 0).then(|| bytes(data, data_len)),
        );
    });
}

/// [`session::shell_done`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_shell_done(id: u32, frame: *const u8, len: usize) {
    session::host_turn(|| {
        session::shell_done(id, bytes(frame, len));
    });
}

/// [`session::set_index`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_set_index(
    tsv: *const u8,
    tsv_len: usize,
    release: *const u8,
    release_len: usize,
    table: *const u8,
    table_len: usize,
    bundled: *const u8,
    bundled_len: usize,
) -> i32 {
    session::host_turn(|| {
        session::set_index(
            bytes(tsv, tsv_len),
            bytes(release, release_len),
            bytes(table, table_len),
            &text(bundled, bundled_len),
        )
    })
}

/// [`session::open`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_open(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::open(bytes(p, n)))
}

/// [`session::step_once`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_step() -> i32 {
    session::step_once()
}

/// [`session::close`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_close() {
    session::host_turn(|| {
        session::close();
    });
}

/// [`session::request`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_request(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::request(bytes(p, n)))
}

/// [`session::lsp_message`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_lsp(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::lsp_message(bytes(p, n)))
}

/// [`session::main_output`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_main_output() -> i32 {
    session::host_turn(session::main_output)
}

/// [`session::pdf`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_pdf() -> i32 {
    session::host_turn(session::pdf)
}

/// [`session::file`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_file(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::file(&text(p, n)))
}

/// [`session::strips`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_strips(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::strips(&text(p, n)))
}

/// [`session::frame`].
#[unsafe(no_mangle)]
pub extern "C" fn ph_frame(p: *const u8, n: usize) -> i32 {
    session::host_turn(|| session::frame(bytes(p, n)))
}
