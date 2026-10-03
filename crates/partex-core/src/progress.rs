//! Live progress for a terminal (the modern command line's status line,
//! DESIGN.md 2.5): a board of counters the engine posts with relaxed
//! atomics, which a renderer on another thread samples about ten times a
//! second.
//!
//! It is observability only: the engine never reads it back, no tracker
//! sees it, and it is not state (a snapshot, a replay or a rebuild
//! leaves it as it is). Every engine of the process posts to the one
//! board, [`BOARD`]: its counters are the process's work so far, which a
//! reader takes differences of.
//!
//! The engine posts at a coarse grain: every [`EVERY`] commands (the
//! commands run, and the file and line being read), at each page shipped
//! out, and when it starts to finish the PDF file. Between, a command
//! pays a test of its count's low bits; a name is copied only when the
//! file being read is another one.

use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, fence};

/// Commands between two posts of the commands run, the file and the line.
pub const EVERY: u64 = 1024;

/// The bytes of a file's name the board keeps (its end, if it is longer).
const NAME: usize = 120;
const WORDS: usize = NAME / 8;

/// What the engine is doing, beyond running commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Running commands (the default).
    Run,
    /// Finishing the PDF file: its fonts, its object streams and its
    /// cross-reference table (pdfTeX §794).
    FinishPdf,
}

/// A board of progress counters.
pub struct Board {
    commands: AtomicU64,
    pages: AtomicU64,
    count0: AtomicI32,
    line: AtomicI32,
    phase: AtomicU8,
    /// The name of the file being read, by address (to see it change).
    key: AtomicUsize,
    /// A sequence lock over the name: odd while it is written.
    seq: AtomicU32,
    len: AtomicU32,
    name: [AtomicU64; WORDS],
}

/// The process's board.
pub static BOARD: Board = Board::new();

/// What a reader sees on a board.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Commands run, to the last post.
    pub commands: u64,
    /// Pages shipped out.
    pub pages: u64,
    /// The last page's `\count0`.
    pub count0: i32,
    /// The line being read, in [`Snapshot::file`].
    pub line: i32,
    /// The file being read (`None` while its name is being written, or
    /// before any file was read).
    pub file: Option<alloc::vec::Vec<u8>>,
    /// Whether the engine is finishing the PDF file.
    pub finishing: bool,
}

impl Board {
    /// An empty board.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: AtomicU64::new(0),
            pages: AtomicU64::new(0),
            count0: AtomicI32::new(0),
            line: AtomicI32::new(0),
            phase: AtomicU8::new(0),
            key: AtomicUsize::new(0),
            seq: AtomicU32::new(0),
            len: AtomicU32::new(0),
            name: [const { AtomicU64::new(0) }; WORDS],
        }
    }

    /// `n` more commands run.
    #[inline]
    pub fn commands(&self, n: u64) {
        self.commands.fetch_add(n, Relaxed);
    }

    /// Line `line` of the file named `name` is being read. The name is
    /// copied only if it is another one (by a hash of its bytes).
    pub fn reading(&self, name: &[u8], line: i32) {
        self.line.store(line, Relaxed);
        let key = key(name);
        if self.key.load(Relaxed) == key {
            return;
        }
        // (one writer at a time: another engine's post is skipped)
        let s = self.seq.load(Relaxed);
        if s % 2 == 1
            || self
                .seq
                .compare_exchange(s, s.wrapping_add(1), Acquire, Relaxed)
                .is_err()
        {
            return;
        }
        fence(Release);
        let tail = &name[name.len().saturating_sub(NAME)..];
        for (i, w) in self.name.iter().enumerate() {
            let mut bytes = [0u8; 8];
            for (j, x) in bytes.iter_mut().enumerate() {
                *x = tail.get(i * 8 + j).copied().unwrap_or(0);
            }
            w.store(u64::from_le_bytes(bytes), Relaxed);
        }
        self.len
            .store(u32::try_from(tail.len()).unwrap_or(0), Relaxed);
        self.key.store(key, Relaxed);
        self.seq.store(s.wrapping_add(2), Release);
    }

    /// A page shipped out, its `\count0` being `count0`.
    #[inline]
    pub fn page(&self, count0: i32) {
        self.pages.fetch_add(1, Relaxed);
        self.count0.store(count0, Relaxed);
    }

    /// The engine's phase is now `p`.
    #[inline]
    pub fn phase(&self, p: Phase) {
        self.phase.store(p as u8, Relaxed);
    }

    /// The board as it is now.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            commands: self.commands.load(Relaxed),
            pages: self.pages.load(Relaxed),
            count0: self.count0.load(Relaxed),
            line: self.line.load(Relaxed),
            file: self.file(),
            finishing: self.phase.load(Relaxed) == Phase::FinishPdf as u8,
        }
    }

    /// The name of the file being read, unless it is being written.
    fn file(&self) -> Option<alloc::vec::Vec<u8>> {
        let s = self.seq.load(Acquire);
        if s % 2 == 1 || self.key.load(Relaxed) == 0 {
            return None;
        }
        let len = usize::try_from(self.len.load(Relaxed))
            .unwrap_or(0)
            .min(NAME);
        let mut out = alloc::vec::Vec::with_capacity(NAME);
        for w in &self.name {
            out.extend_from_slice(&w.load(Relaxed).to_le_bytes());
        }
        out.truncate(len);
        fence(Acquire);
        (self.seq.load(Relaxed) == s).then_some(out)
    }
}

/// A name's key: FNV-1a of its bytes (on 32-bit targets, their low
/// half), never 0 (the key of no name).
#[allow(clippy::cast_possible_truncation)]
fn key(name: &[u8]) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in name {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (h as usize).max(1)
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_keeps_the_file_the_line_and_the_counts() {
        let b = Board::new();
        assert_eq!(b.snapshot(), Snapshot::default());
        b.reading(b"./paper.tex", 12);
        let s = b.snapshot();
        assert_eq!(s.file.as_deref(), Some(&b"./paper.tex"[..]));
        assert_eq!(s.line, 12);
        b.reading(b"./paper.tex", 13);
        b.reading(b"chapter1.tex", 2);
        assert_eq!(b.snapshot().file.as_deref(), Some(&b"chapter1.tex"[..]));
        b.reading(b"chapter2.tex", 3);
        let s = b.snapshot();
        assert_eq!(s.file.as_deref(), Some(&b"chapter2.tex"[..]));
        assert_eq!(s.line, 3);
        // (a long name: its end)
        let long: alloc::vec::Vec<u8> = (0..200u8).map(|i| b'a' + i % 26).collect();
        b.reading(&long, 1);
        assert_eq!(b.snapshot().file.as_deref(), Some(&long[200 - NAME..]));
        b.commands(EVERY);
        b.commands(EVERY);
        b.page(7);
        b.phase(Phase::FinishPdf);
        let s = b.snapshot();
        assert_eq!((s.commands, s.pages, s.count0), (2 * EVERY, 1, 7));
        assert!(s.finishing);
    }
}
