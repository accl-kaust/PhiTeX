//! The streams as values (DESIGN 7.17.12's `write_file`, `write_open`,
//! `log_file`, `read_file`, `read_open` and `random` rows).
//!
//! - A `\write` stream's lines are stores (7.17.5): `\openout` makes one
//!   to the file's name (`Tracker::store_open`), and every line written
//!   to it is stored there (`Tracker::store_line`), so the file is a
//!   persistent sequence of lines the runtime keeps by name. Which file
//!   stream `n` stores to is the value `Out(n)`, read by `write_out`
//!   (§1370) and written by `\openout` and `\closeout` (§1374). No writer
//!   reads the lines, so a line written late does not make the next
//!   writers miss.
//! - The bytes the engine makes for its outputs (the log, the terminal,
//!   the `\write` files, the DVI and PDF files) are effects
//!   (`Tracker::output`), never read.
//! - The log's `Out(LOG)` is its file, if open (§534).
//! - An `\openin` stream (§480) is a file value and a position: its
//!   contents' version, made when it is loaded (§1275), and where the
//!   next `\read` begins (§483), made at each line read.
//! - pdfTeX's random number generator (pdfTeX §110) is a value made at
//!   each draw (pdfTeX §126, §127) and at `\pdfsetrandomseed` (pdfTeX
//!   §1581).
//! - The host's answers that are not loads (the clock, the creation
//!   date, a file's date, the terminal's lines) are reads of `Clock`,
//!   versioned by the answer: no probe finds them equal, so a call that
//!   read one runs again.

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_ssa::Version;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::{Output, Query, Row, Tracker};

/// The log's `Out` row.
pub(crate) const LOG: u8 = 16;

/// What the streams' values need beside TeX's own fields.
#[derive(Clone, Debug, Default)]
pub(crate) struct Streams {
    /// The file `\write` stream `n` stores to (`\openout`'s name), if
    /// open.
    out_name: [Option<Arc<[u8]>>; 16],
    /// The line being written to stream `n` (scratch: `write_out` ends
    /// every line it writes, §1370).
    lines: [Vec<u8>; 16],
    /// `\openin` stream `n`'s contents' version, made at the load.
    read_ver: [u128; 16],
}

impl partex_engine::persist::Persist for Streams {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.out_name.save(s);
        self.read_ver.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self {
            out_name: Persist::load(l)?,
            lines: Default::default(),
            read_ver: Persist::load(l)?,
        })
    }
}

use partex_engine::persist::Persist;

impl Streams {
    /// The file stream `n` stores to (`Out(n)`'s value, with the file).
    pub(crate) fn out_name(&self, n: usize) -> Option<Arc<[u8]>> {
        self.out_name.get(n).cloned().flatten()
    }

    /// `Out(n)`'s name stored back (its line being written is scratch).
    pub(crate) fn set_out_name_value(&mut self, n: usize, name: Option<Arc<[u8]>>) {
        if let Some(s) = self.out_name.get_mut(n) {
            *s = name;
        }
    }

    /// `\openin` stream `n`'s contents' version.
    pub(crate) fn read_ver(&self, n: usize) -> u128 {
        self.read_ver.get(n).copied().unwrap_or(0)
    }

    pub(crate) fn set_read_ver(&mut self, n: usize, v: u128) {
        if let Some(x) = self.read_ver.get_mut(n) {
            *x = v;
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `Out(n)`'s version: the name stream `n` stores to and the file
    /// its bytes go to, or the log's file.
    fn out_version(&self, n: u8) -> u128 {
        if n == LOG {
            return Version::of(&(0x006c_6f67u32, self.log_file.id.map(|f| f.0))).0;
        }
        let k = usize::from(n) & 15;
        Version::of(&(
            &self.streams.out_name[k],
            self.write_file.get(k).and_then(|f| f.id).map(|f| f.0),
        ))
        .0
    }

    /// A read of `Out(n)` (for `n < 16`, the machine's `Cell::Out`).
    #[inline]
    pub(crate) fn out_read(&self, n: u8) {
        if T::VALUES {
            self.tracker.row_read(Row::Out(n), || self.out_version(n));
        } else if n < 16 {
            self.tracker.read(crate::track::Cell::Out(i32::from(n)));
        }
    }

    /// `Out(n)` was written: its version, made from its value now (the
    /// machine's `Cell::Out` writes are at the routines' starts).
    #[inline]
    pub(crate) fn out_wrote(&mut self, n: u8) {
        if T::VALUES {
            self.tracker.row_wrote(Row::Out(n), self.out_version(n));
        }
    }

    /// Stream `n` stores to file `name` from now on (`\openout`, §1374),
    /// or to none.
    pub(crate) fn set_out_name(&mut self, n: usize, name: Option<&[u8]>) {
        if let Some(s) = self.streams.out_name.get_mut(n) {
            *s = name.map(Arc::from);
            self.streams.lines[n].clear();
        }
        if T::VALUES
            && let (Some(name), Ok(k)) = (name, u8::try_from(n))
        {
            self.tracker.store_open(k, name);
        }
    }

    /// The log's being open, read (§534).
    #[inline]
    pub(crate) fn log_open(&self) -> bool {
        self.out_read(LOG);
        self.log_file.id.is_some()
    }

    /// Byte `c` written to `\write` stream `n`'s file: an effect, and at
    /// the line's end a store (7.17.5).
    #[inline]
    pub(crate) fn stream_byte(&mut self, n: usize, c: u8) {
        if !T::VALUES {
            return;
        }
        self.tracker
            .output(Output::Write(u8::try_from(n & 15).unwrap_or(0)), &[c]);
        let s = &mut self.streams;
        let line = &mut s.lines[n & 15];
        if c == b'\n' {
            // (to the file the stream stores to as the engine holds it,
            // which a rebuild restores with the stream: not the file its
            // number opened last, in whatever run)
            if let Some(name) = s.out_name[n & 15].as_deref() {
                self.tracker.store_line(name, line);
            }
            line.clear();
        } else {
            line.push(c);
        }
    }

    /// `Read(n)`'s version: the contents' version and the position, or
    /// closed.
    fn read_version(&self, n: usize) -> u128 {
        match self.read_file.get(n).and_then(Option::as_ref) {
            None => Version::of(&0x636c_6f73u32).0,
            Some(f) => {
                Version::of(&(
                    self.streams.read_ver[n],
                    f.pos,
                    f.line_from,
                    f.line_open,
                    f.lines,
                ))
                .0
            }
        }
    }

    /// A read of `\openin` stream `n` (for the machine, `Cell::Read`).
    #[inline]
    pub(crate) fn read_file_read(&self, n: usize) {
        let Ok(k) = u8::try_from(n) else { return };
        if T::VALUES {
            self.tracker.row_read(Row::Read(k), || self.read_version(n));
        } else {
            self.tracker.read(crate::track::Cell::Read(i32::from(k)));
        }
    }

    /// `\openin` stream `n` was written (opened, read from or closed):
    /// its version, made from its value now (the machine's `Cell::Read`
    /// writes are at the routines' starts).
    #[inline]
    pub(crate) fn read_file_wrote(&mut self, n: usize) {
        if T::VALUES
            && let Ok(k) = u8::try_from(n)
        {
            self.tracker.row_wrote(Row::Read(k), self.read_version(n));
        }
    }

    /// Stream `n` opened on `contents` (§1275): its version, made at the
    /// load.
    pub(crate) fn read_file_loaded(&mut self, n: usize, contents: &[u8]) {
        if T::VALUES
            && let Some(v) = self.streams.read_ver.get_mut(n)
        {
            *v = Version::of(contents).0;
        }
    }

    /// A read of the random generator (for the machine, `Cell::Random`).
    #[inline]
    pub(crate) fn random_read(&self) {
        if T::VALUES {
            self.tracker
                .row_read(Row::Random, || Version::of(&self.random).0);
        } else {
            self.tracker.read(crate::track::Cell::Random);
        }
    }

    /// The random generator was written: its version, made from its
    /// state now (the machine's `Cell::Random` writes are at the draws'
    /// starts).
    #[inline]
    pub(crate) fn random_wrote(&mut self) {
        if T::VALUES {
            self.tracker
                .row_wrote(Row::Random, Version::of(&self.random).0);
        }
    }

    /// A read of the host's answer `v` to query `q`, one that is not a
    /// load (`Row::Clock`): a call that reads it is never a hit, and a
    /// rebuild asks it again (DESIGN 7.17.3, "A query is asked again").
    #[inline]
    pub(crate) fn clock_read<V: core::hash::Hash + ?Sized>(&mut self, q: Query, v: &V) {
        if T::VALUES {
            let ver = Version::of(v).0;
            self.tracker.value_read(Row::Clock, || ver);
            let answer = if q == Query::Now {
                self.clock_answer()
            } else {
                ver
            };
            self.tracker.queried(q, answer);
        }
    }

    /// The host's time as a query's answer (`Query::Now`): its date to
    /// the minute and its creation date, one answer per trip.
    pub(crate) fn clock_answer(&mut self) -> u128 {
        let n = self.host.now();
        let d = self.host.creation_date();
        Version::of(&(n.year, n.month, n.day, n.minutes, d)).0
    }

    /// Every stream row versioned from its value, as made wholesale (the
    /// engine as made, a format's load: `Tex::version_tables`).
    pub(crate) fn version_streams(&self) {
        if !T::VALUES {
            return;
        }
        for n in 0..=LOG {
            self.tracker.row_made(Row::Out(n), self.out_version(n));
        }
        for n in 0..16u8 {
            self.tracker
                .row_made(Row::Read(n), self.read_version(usize::from(n)));
        }
        self.tracker
            .row_made(Row::Random, Version::of(&self.random).0);
    }
}
