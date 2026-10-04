//! The engine's only window to the outside world (Typst's "World" idea).

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::dviout::{DviWriter, Summary, TooLong};
use partex_engine::pageir::Page;

/// What kind of file TeX asks for. Selects the kpathsea search path and
/// default suffix on native hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `\input`, `\openin` (`.tex`).
    Tex,
    /// Font metrics (`.tfm`).
    Tfm,
    /// Format files (`.fmt`).
    Fmt,
    /// Font map files (`.map`, pdfTeX).
    FontMap,
    /// Type 1 fonts (`.pfb`, `.pfa`).
    Type1,
    /// Encoding vectors (`.enc`).
    Enc,
    /// Virtual fonts (`.vf`).
    Vf,
    /// TrueType fonts (`.ttf`).
    TrueType,
    /// BibTeX styles (`.bst`), for the build's own BibTeX (DESIGN 3.7,
    /// "Outside tools are nodes").
    Bst,
    /// BibTeX databases (`.bib`).
    Bib,
    /// makeindex styles (`.ist`).
    Ist,
    /// Everything else, looked up by exact name.
    Other,
}

/// A load as [`Host::unchanged`] is asked about it: the name, its kind,
/// and the contents [`Host::read_file`] gave (none: it found nothing).
pub type Load<'a> = (&'a [u8], FileKind, Option<&'a Arc<[u8]>>);

/// A file found and read by the host.
/// A memoized value (see [`Host::cached`]).
pub type Memo = Arc<dyn core::any::Any + Send + Sync>;

pub struct OpenedFile {
    /// The name TeX prints (`(./trip.tex`): the host's resolved path, as
    /// kpathsea would return it.
    pub name: Vec<u8>,
    /// Shared: the engine keeps open input files as they were read, and a
    /// host may keep them too (to see what a checkpoint has read).
    pub contents: Arc<[u8]>,
}

/// Handle for a file opened for writing (`\openout`, `.log`, `.dvi`, `.pdf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WriteId(pub u32);

partex_engine::persist_struct!(WriteId; 0);

/// Calendar time as TeX sees it (`\time`, `\day`, `\month`, `\year`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    /// Minutes since midnight.
    pub minutes: i32,
}

/// A DVI writer on another thread: pages are queued and written while
/// TeX goes on (see [`Host::page_sink`]). The engine queues a page only
/// when the file surely stays below 2^31 bytes, so §598's error still
/// happens at the page it would in TeX.
pub trait PageSink {
    /// Take over `writer`, which writes `file`.
    fn start(&mut self, writer: DviWriter, file: WriteId);
    /// Queue `page`; returns an empty page to reuse.
    fn page(&mut self, page: Page) -> Page;
    /// Write `page` after the queued ones, and wait: the page back, and
    /// the file's length.
    fn page_now(&mut self, page: Page) -> (Page, Result<i32, TooLong>);
    /// Finish the file (§642) and write it all out.
    fn finish(&mut self, mag: i32) -> Result<Summary, TooLong>;
}

/// What a command [`Host::system`] ran did.
#[derive(Clone, Debug, Default)]
pub struct Ran {
    /// The status C's `system` returns (web2c prints `system returned with
    /// code N` on the standard error when it is not 0; the host does).
    pub status: i32,
    /// The files the command made or changed, each by the names the job
    /// would read it by (relative to the output directory, and with
    /// one, the path to it from the working directory too), with its
    /// contents. An SSA build stores them, as the step's, so that loads
    /// read them from the build (DESIGN 3.7, "Commands").
    pub wrote: Vec<(Vec<u8>, Arc<[u8]>)>,
    /// The files it removed, by name as in `wrote`.
    pub removed: Vec<Vec<u8>>,
    /// What it wrote to its standard output: the engine puts it on the
    /// terminal at once, before what TeX has not flushed yet, as the
    /// child of pdfTeX's `system` writes to the terminal past pdfTeX's
    /// stdio buffer.
    pub stdout: Vec<u8>,
}

/// All effects of the engine. Implementations must be deterministic for a
/// given input tree and `SOURCE_DATE_EPOCH`; the memoization layers rely on it.
pub trait Host {
    /// Look up and read a whole file. `None` if it does not exist.
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile>;

    /// For each load (a name, its kind, and the contents
    /// [`read_file`](Host::read_file) gave, or none), whether `read_file`
    /// would give the same again, known without making the lookup or
    /// reading the file (DESIGN 7.17.3, "A rebuild's file checks cost the
    /// files that changed"). False says nothing: the caller loads again
    /// and compares.
    fn unchanged(&mut self, loads: &[Load<'_>]) -> Vec<bool> {
        alloc::vec![false; loads.len()]
    }

    /// Create (or truncate) an output file. Returns the name TeX prints.
    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)>;
    /// [`open_write`](Host::open_write), on handle `id` if this host gave
    /// it for the same file: a step that runs again opens its files on the
    /// handles its last run had (DESIGN 7.17.3), so what the later steps
    /// hold of a file (a writer's state, their writes) still names it.
    /// The default opens the file on a new handle, which wakes them.
    fn open_write_again(
        &mut self,
        name: &[u8],
        kind: FileKind,
        id: WriteId,
    ) -> Option<(WriteId, Vec<u8>)> {
        let _ = id;
        self.open_write(name, kind)
    }
    /// The name a lookup of `name` would give for the file the job writes
    /// as `name` (where the job writes it, as the lookup names a file
    /// found there): what TeX prints when it reads it back. By default,
    /// [`Host::output_name`]'s.
    fn written_name(&mut self, name: &[u8]) -> Vec<u8> {
        self.output_name(name, FileKind::Other)
    }

    /// Whether the file the job writes as `name` was edited since the
    /// build's link last wrote it (by a user, not by the build): an SSA
    /// rebuild reads such a file's new contents as an edit; any other is
    /// the build's own, and its value is the one the build holds (DESIGN
    /// 3.7, "Files are a view"). The default, for a host that does not
    /// know who last wrote a file: edited, so the rebuild reads the file
    /// again as before, and its user's real edits come through the
    /// host's normal edit path (`read_file`, [`Host::unchanged`]); since
    /// an SSA build writes no output before its link, what it reads then
    /// is the link's own write or the user's.
    fn output_edited(&mut self, name: &[u8]) -> bool {
        let _ = name;
        true
    }

    /// A handle for output file `name` whose bytes the build keeps in
    /// memory and a link writes later (an SSA build, DESIGN 3.7, "Files
    /// are a view"): the host's file is not touched now. `again`: the
    /// handle a step that runs again had, as [`Host::open_write_again`].
    /// The default creates the file, as [`Host::open_write`] does.
    fn open_write_later(
        &mut self,
        name: &[u8],
        kind: FileKind,
        again: Option<WriteId>,
    ) -> Option<(WriteId, Vec<u8>)> {
        match again {
            Some(id) => self.open_write_again(name, kind, id),
            None => self.open_write(name, kind),
        }
    }
    fn write(&mut self, file: WriteId, bytes: &[u8]);
    fn close(&mut self, file: WriteId);

    /// Terminal output (TeX's `term_out`).
    fn term_write(&mut self, bytes: &[u8]);
    /// One line of terminal input (`\read16`, error prompts). `None` at EOF.
    fn term_read_line(&mut self) -> Option<Vec<u8>>;

    /// Time of the job start.
    fn now(&self) -> DateTime;

    /// pdfTeX's `\pdfcreationdate`: when the job started, as a PDF date
    /// (`D:YYYYmmddHHMMSS` and the zone: `Z`, or `+HH'MM'`). By default
    /// the minute of [`Host::now`], as if it were UTC.
    fn creation_date(&mut self) -> Vec<u8> {
        let t = self.now();
        alloc::format!(
            "D:{:04}{:02}{:02}{:02}{:02}00Z",
            t.year,
            t.month,
            t.day,
            t.minutes / 60,
            t.minutes % 60
        )
        .into_bytes()
    }

    /// pdfTeX's `\pdffilemoddate`: when the file `\input` would find as
    /// `name` last changed, as a PDF date; `None` if not found (or the
    /// host cannot tell).
    fn file_mod_date(&mut self, _name: &[u8]) -> Option<Vec<u8>> {
        None
    }

    /// Seconds since the epoch and microseconds, for pdfTeX's timer and
    /// random seed (web2c's `get_seconds_and_micros`). Hosts that must be
    /// reproducible keep the default.
    fn seconds_and_micros(&mut self) -> (i32, i32) {
        (0, 0)
    }

    /// A structured report of an error TeX has just printed (see
    /// [`crate::diag`]). Ignored unless the host implements it.
    fn diagnostic(&mut self, _diagnostic: &crate::diag::Diagnostic) {}

    /// Whether [`Host::diagnostic`] should also get warnings (overfull and
    /// underfull boxes) and notes (`\message`, `\write` to the terminal).
    /// Nothing TeX prints depends on it.
    fn notes(&self) -> bool {
        false
    }

    /// A page is being shipped out, `\count0` its number (for progress
    /// reports; nothing TeX prints depends on it).
    fn shipping(&mut self, _count0: i32) {}

    /// A page just written to the DVI file (not called while a
    /// [`PageSink`] has the file). A host that splices outputs keeps it,
    /// to write the page again elsewhere in the file.
    fn page_written(&mut self, _page: &Page) {}

    /// zlib's `compress` of `data` at `level` (1–9), as pdfTeX's zlib
    /// writes it; `None` if the host has no zlib (PDF streams are then
    /// stored uncompressed).
    fn deflate(&mut self, _level: i32, _data: &[u8]) -> Option<Vec<u8>> {
        None
    }

    /// What a pure computation made, by `key`, a hash of everything it
    /// read (a content-keyed memo: a machine's host keeps them across
    /// rebuilds, so a rebuild that ends the job again does not subset
    /// the same fonts again). `None` by default.
    fn cached(&mut self, _key: u128) -> Option<Memo> {
        None
    }

    /// Keep `value` under `key` (see [`Host::cached`]).
    fn cache(&mut self, _key: u128, _value: Memo) {}

    /// The slot (internal number) for a font of identity `ident` (a hash
    /// of what it is: its file, name and size, or the font it was
    /// expanded or copied from), `fresh` being tex.web's next number
    /// (§576). A machine's host keeps a registry shared by every run of a
    /// build, so a font keeps its slot however many fonts were loaded
    /// before it; a slot is never given to two identities. The default is
    /// `fresh`.
    fn font_slot(&mut self, _ident: u128, fresh: i32) -> i32 {
        fresh
    }

    /// A writer for the DVI file on another thread, if the host has one.
    fn page_sink(&mut self) -> Option<&mut dyn PageSink> {
        None
    }

    /// `\write18`'s command, allowed and quoted by the engine (web2c's
    /// `runsystem`, [`crate::shell`]): run it with the shell and wait, as
    /// C's `system` does. `inputs` are the job's own files (those it
    /// wrote with `\openout` and closed) as the build holds them where
    /// the command runs: a build that ran steps out of order may have left
    /// other bytes in them, and the command must see these. `None`: this
    /// host runs no commands. What TeX prints does not depend on the
    /// answer (pdfTeX's log says `executed` whatever the status).
    fn system(&mut self, _command: &[u8], _inputs: &[(Vec<u8>, Arc<[u8]>)]) -> Option<Ran> {
        None
    }

    /// Whether [`Host::system`] runs commands. A host that runs none (the
    /// default, the wasm extension's) logs every `\write18` it is given
    /// with shell escape on as `disabled (restricted)`, whatever
    /// `shell_escape_commands` allows.
    fn runs_commands(&self) -> bool {
        false
    }

    /// A value cached under `key` by an earlier run (of this same
    /// program), if the host keeps a cache. Cached values are results of
    /// pure functions of their key (a map file's table, for one): using
    /// one changes nothing but the time taken. Without one, a new engine
    /// reads the default font map (`pdftex.map`, 42 000 lines) in full at
    /// its first page, once: the engine then keeps what it found, for as
    /// long as the host hands out the same contents (`fontmap::MapCache`).
    fn cache_get(&mut self, _key: u128) -> Option<Vec<u8>> {
        None
    }

    /// Keep `value` under `key` for later runs (see [`Host::cache_get`]).
    fn cache_put(&mut self, _key: u128, _value: &[u8]) {}

    /// The name `SyncTeX` gives an input file the host found as `found`
    /// (web2c's `generic_synctex_get_current_name`: absolute, the working
    /// directory before a relative one). By default, `found`.
    fn synctex_name(&mut self, found: &[u8]) -> Vec<u8> {
        found.to_vec()
    }

    /// Remove output file `name` if there is one (`SyncTeX`'s file of an
    /// earlier run that this one does not write). By default, nothing.
    fn remove_output(&mut self, _name: &[u8]) {}

    /// The name [`Host::open_write`] would print for output file `name`,
    /// without opening it. By default, `name`.
    fn output_name(&mut self, name: &[u8], _kind: FileKind) -> Vec<u8> {
        name.to_vec()
    }
}

partex_engine::persist_struct!(DateTime {
    year,
    month,
    day,
    minutes
});
partex_engine::persist_enum!(FileKind {
    Tex,
    Tfm,
    Fmt,
    FontMap,
    Type1,
    Enc,
    Vf,
    TrueType,
    Other,
    Bst,
    Bib,
    Ist
});

/// A host with no files and no terminal: the engine that holds the
/// format's definitions runs nothing (`ssa::rebuild`, DESIGN 7.17.3).
pub(crate) struct NoHost;

impl Host for NoHost {
    fn read_file(&mut self, _name: &[u8], _kind: FileKind) -> Option<OpenedFile> {
        None
    }
    fn open_write(&mut self, _name: &[u8], _kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        None
    }
    fn write(&mut self, _file: WriteId, _bytes: &[u8]) {}
    fn close(&mut self, _file: WriteId) {}
    fn term_write(&mut self, _bytes: &[u8]) {}
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
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
