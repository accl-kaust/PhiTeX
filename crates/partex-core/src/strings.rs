//! Part 4: String handling (§38–§53).
//!
//! The pool starts with the 256 single-character strings, followed by the
//! strings of web2c's `tex.pool` (tangled from tex.web + change files by
//! `scripts/merge-web.sh`), so string numbers and pool statistics match the
//! reference binary exactly.

use crate::host::Host;
use crate::params::Flavor;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// web2c's `tex.pool`: each line is a two-digit length and the string; the
/// last line is `*` and a checksum.
const TEX_POOL: &[u8] = include_bytes!("tex.pool");
/// pdfTeX's pool (tangled from pdftex.web + change files).
const PDFTEX_POOL: &[u8] = include_bytes!("pdftex.pool");

/// A string number (§38, `str_number`).
pub type StrNumber = usize;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §40: the number of characters in string `s`.
    pub(crate) fn length(&self, s: StrNumber) -> usize {
        self.str_start[s + 1] - self.str_start[s]
    }

    /// §41: the length of the current (unfinished) string.
    pub(crate) fn cur_length(&self) -> usize {
        self.pool_ptr - self.str_start[self.str_ptr]
    }

    /// §42: append a character; the caller has checked room with `str_room`.
    pub(crate) fn append_char(&mut self, c: u8) {
        if self.pool_ptr == self.str_pool.len() {
            self.str_pool.push(c);
        } else {
            self.str_pool[self.pool_ptr] = c;
        }
        self.pool_ptr += 1;
    }

    /// Make `str_pool[..pool_ptr + l]` exist. The pool is only as long as
    /// its contents need (a checkpoint copies it); `pool_size` is the
    /// limit TeX reports.
    pub(crate) fn grow_pool(&mut self, l: usize) {
        // (to the length needed only: capacity grows by doubling, but a
        // checkpoint's clone copies the length)
        let need = self.pool_ptr + l;
        if need > self.str_pool.len() {
            self.str_pool.resize(need, 0);
        }
    }

    /// Make `str_start[..=k]` exist.
    pub(crate) fn grow_starts(&mut self, k: usize) {
        if k >= self.str_start.len() {
            self.str_start.resize(k + 1, 0);
        }
    }

    /// §42: forget the last character appended.
    pub(crate) fn flush_char(&mut self) {
        self.pool_ptr -= 1;
    }

    /// §42: make sure there is room for `l` more characters.
    pub(crate) fn str_room(&mut self, l: usize) -> Result<(), Jump> {
        if self.pool_ptr + l > self.pool_size() {
            let n = self.pool_size() - self.init_pool_ptr;
            return self.overflow(b"pool size", i32::try_from(n).unwrap_or(i32::MAX));
        }
        self.grow_pool(l);
        Ok(())
    }

    /// §43: the current string enters the pool.
    pub(crate) fn make_string(&mut self) -> Result<StrNumber, Jump> {
        if self.str_ptr == self.max_strings() {
            let n = self.max_strings() - self.init_str_ptr;
            self.overflow(b"number of strings", i32::try_from(n).unwrap_or(i32::MAX))?;
        }
        self.pool_top_read();
        self.str_ptr += 1;
        self.grow_starts(self.str_ptr + 1);
        self.str_start[self.str_ptr] = self.pool_ptr;
        self.pool_wrote(self.str_ptr - 1);
        Ok(self.str_ptr - 1)
    }

    /// §44: destroy the most recently made string.
    pub(crate) fn flush_string(&mut self) {
        self.pool_top_read();
        self.str_ptr -= 1;
        self.pool_ptr = self.str_start[self.str_ptr];
        self.str_index.truncate(self.str_ptr);
        self.strings_reopened();
        self.pool_wrote(self.str_ptr);
    }

    /// A read of the pool's end (the number the next string gets), for a
    /// tracker that keeps versions (DESIGN 7.17.12, the pool's row).
    #[inline]
    pub(crate) fn pool_top_read(&self) {
        if T::VALUES {
            self.tracker.row_read(
                crate::track::Row::Scalar(crate::track::scalar::STR_TOP),
                || crate::track::scalar_version(self.str_ptr),
            );
        }
    }

    /// `str_ptr` moved: strings `from..str_ptr` were made (each versioned
    /// by its bytes, at its making) and the pool's end is a write.
    #[inline]
    pub(crate) fn pool_wrote(&self, from: usize) {
        if T::VALUES {
            self.pool_wrote_slow(from);
        }
    }

    #[cold]
    #[inline(never)]
    fn pool_wrote_slow(&self, from: usize) {
        for n in from..self.str_ptr {
            self.tracker
                .row_wrote(crate::track::Row::Str(n), self.string_version(n));
        }
        self.tracker.row_wrote(
            crate::track::Row::Scalar(crate::track::scalar::STR_TOP),
            crate::track::scalar_version(self.str_ptr),
        );
    }

    /// String `n`'s version: its bytes.
    pub(crate) fn string_version(&self, n: usize) -> u128 {
        partex_ssa::Version::of(&self.str_bytes(n)).0
    }

    /// `str_ptr` went down: the strings from it on may be made again, so
    /// the string pool from their start and `str_start` past `str_ptr`
    /// may change before the next snapshot (`flat.rs`, `commit_live`).
    /// Below that, a made string is never written (§40: characters are
    /// appended at `pool_ptr`, and only the string being made, from
    /// `str_start[str_ptr]` on, is ever changed in place: §517, §260).
    #[inline]
    pub(crate) fn strings_reopened(&mut self) {
        let p = self.str_start.get_all(self.str_ptr);
        self.str_pool.lower_floor(p);
        self.str_start.lower_floor(self.str_ptr + 1);
    }

    /// The characters of string `s`.
    pub(crate) fn str_bytes(&self, s: StrNumber) -> &[u8] {
        &self.str_pool[self.str_start[s]..self.str_start[s + 1]]
    }

    /// §46: do strings `s` and `t` have the same characters?
    pub(crate) fn str_eq_str(&self, s: StrNumber, t: StrNumber) -> bool {
        self.str_bytes(s) == self.str_bytes(t)
    }

    /// §47: initialize the string pool (INITEX). Returns `false` if the
    /// pool strings don't fit, after telling the user (§51).
    pub(crate) fn get_strings_started(&mut self) -> Result<bool, Jump> {
        self.pool_ptr = 0;
        self.str_ptr = 0;
        self.str_start[0] = 0;
        self.strings_reopened();
        self.str_index.clear();
        // §48: make the first 256 strings.
        for k in 0..=255u8 {
            // §49: character `k` cannot be printed unless it is visible ASCII.
            if (b' '..=b'~').contains(&k) {
                self.append_char(k);
            } else {
                self.append_char(b'^');
                self.append_char(b'^');
                if k < 0o100 {
                    self.append_char(k + 0o100);
                } else if k < 0o200 {
                    self.append_char(k - 0o100);
                } else {
                    self.append_lc_hex(k / 16);
                    self.append_lc_hex(k % 16);
                }
            }
            self.make_string()?;
        }
        // §51 (web2c): `loadpoolstrings(pool_size - string_vacancies)`.
        let spare = self.pool_size().saturating_sub(self.string_vacancies());
        if !self.load_pool_strings(spare)? {
            self.term_bytes(b"! You have to increase POOLSIZE.\n");
            return Ok(false);
        }
        Ok(true)
    }

    /// §48, `app_lc_hex`.
    fn append_lc_hex(&mut self, l: u8) {
        if l < 10 {
            self.append_char(l + b'0');
        } else {
            self.append_char(l - 10 + b'a');
        }
    }

    /// web2c's generated `loadpoolstrings`: append every `tex.pool` string
    /// unless their cumulative length reaches `spare`.
    fn load_pool_strings(&mut self, spare: usize) -> Result<bool, Jump> {
        let mut total = 0;
        let mut made = false;
        let pool = match self.params.flavor {
            Flavor::Tex => TEX_POOL,
            Flavor::PdfTex => PDFTEX_POOL,
        };
        for line in pool.split(|&b| b == b'\n') {
            if line.first() == Some(&b'*') || line.len() < 2 {
                break;
            }
            let s = &line[2..];
            total += s.len();
            if total >= spare {
                return Ok(false);
            }
            for &c in s {
                self.append_char(c);
            }
            self.make_string()?;
            made = true;
        }
        Ok(made)
    }

    /// Look up a pool string by content (for primitives and messages that
    /// tex.web refers to by string number). Linear; used at initialization.
    /// The pool's strings are set up: remember where `""` is.
    pub(crate) fn pool_ready(&mut self) {
        let find = |t: &Self, s: &[u8]| {
            t.find_pool_string(s)
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(0)
        };
        self.str_index.empty = find(self, b"");
        self.str_index.copied = find(self, b"///...");
    }

    pub(crate) fn find_pool_string(&self, s: &[u8]) -> Option<StrNumber> {
        (256..self.init_str_ptr.max(self.str_ptr)).find(|&n| self.str_bytes(n) == s)
    }

    pub(crate) fn pool_size(&self) -> usize {
        usize::try_from(self.params.pool_size).unwrap_or(0)
    }

    pub(crate) fn max_strings(&self) -> usize {
        usize::try_from(self.params.max_strings).unwrap_or(0)
    }

    fn string_vacancies(&self) -> usize {
        usize::try_from(self.params.string_vacancies).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::engine;

    #[test]
    fn pool_matches_web2c() {
        let t = engine();
        // 256 single-character strings + the 1093 strings of tex.pool.
        assert_eq!(t.str_ptr, 256 + 1093);
        assert_eq!(t.str_bytes(0), b"^^@");
        assert_eq!(t.str_bytes(0x41), b"A");
        assert_eq!(t.str_bytes(0xff), b"^^ff");
        assert_eq!(t.str_bytes(256), b"buffer size");
        assert_eq!(t.find_pool_string(b"pool size"), Some(257));
    }

    #[test]
    fn make_and_flush() {
        let mut t = engine();
        let before = (t.str_ptr, t.pool_ptr);
        t.str_room(3).unwrap();
        for &c in b"abc" {
            t.append_char(c);
        }
        assert_eq!(t.cur_length(), 3);
        let s = t.make_string().unwrap();
        assert_eq!(t.str_bytes(s), b"abc");
        assert_eq!(t.length(s), 3);
        t.flush_string();
        assert_eq!((t.str_ptr, t.pool_ptr), before);
    }
}

/// An index of the pool's strings by contents, for web2c's
/// `search_string` (which scans every string made since the format: 7%
/// of a small LaTeX run, from file names). Strings from 256 up to
/// `covered` are indexed as they were then; every place that removes
/// strings truncates it, so an indexed string is never stale. A clone
/// (a checkpoint) starts empty and indexes again when first searched: a
/// copy per checkpoint cost 1.5 MB.
#[derive(Debug, Default)]
pub(crate) struct StrIndex {
    covered: usize,
    /// The newest indexed string with a contents hash.
    heads: crate::u64map::U64Map<u32>,
    /// For each indexed string (from 256): its hash, and the next older
    /// string with the same hash (0 for none).
    links: alloc::vec::Vec<(u64, u32)>,
    /// The pool strings `""` and `"///..."` (from `tex.pool`, so fixed
    /// once the pool is set up), or 0 if not known: file names without an
    /// area or extension, and every font lookup, want them.
    pub(crate) empty: i32,
    pub(crate) copied: i32,
}

impl Clone for StrIndex {
    fn clone(&self) -> Self {
        Self {
            empty: self.empty,
            copied: self.copied,
            ..Self::default()
        }
    }
}

/// The cell a search for a string with characters `b` reads.
#[must_use]
pub fn str_cell(b: &[u8]) -> crate::track::Cell {
    crate::track::Cell::Str(i32::try_from(contents_hash(b) & 0x7fff_ffff).unwrap_or(0))
}

fn contents_hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h = (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

impl StrIndex {
    /// Forget strings from `str_ptr` on (they were removed).
    pub(crate) fn truncate(&mut self, str_ptr: usize) {
        while self.covered > str_ptr.max(256) {
            self.covered -= 1;
            let (h, older) = self.links.pop().unwrap_or_default();
            if older == 0 {
                self.heads.remove(h);
            } else {
                self.heads.insert(h, older);
            }
        }
    }

    /// Forget everything (the pool was replaced).
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// web2c: `search_string`: the newest string older than `search` with
    /// the same characters, or 0 (strings below 256 are not candidates).
    pub(crate) fn search_string_indexed(&mut self, search: usize) -> i32 {
        let idx = &mut self.str_index;
        idx.truncate(self.str_ptr);
        if idx.covered < 256 {
            idx.covered = 256;
        }
        while idx.covered < search {
            let n = idx.covered;
            let h = contents_hash(&self.str_pool[self.str_start[n]..self.str_start[n + 1]]);
            let older = idx.heads.get(h).unwrap_or(0);
            idx.links.push((h, older));
            idx.heads.insert(h, u32::try_from(n).unwrap_or(0));
            idx.covered += 1;
        }
        let want = &self.str_pool[self.str_start[search]..self.str_start[search + 1]];
        self.tracker.read(str_cell(want));
        let mut s = idx.heads.get(contents_hash(want)).unwrap_or(0) as usize;
        let mut found = 0;
        while s > 255 {
            if s < search && self.str_pool[self.str_start[s]..self.str_start[s + 1]] == *want {
                found = i32::try_from(s).unwrap_or(0);
                break;
            }
            s = idx.links[s - 256].1 as usize;
        }
        if T::VALUES {
            self.tracker.string_search(want, found);
        }
        found
    }

    /// What `search_string` would find now for a new string with the
    /// characters `want`, untracked: the newest string above 255 with
    /// those characters, or 0. What a recorded search is verified against
    /// (DESIGN 7.17.12, the pool's reads by content).
    pub(crate) fn peek_search(&self, want: &[u8]) -> i32 {
        let idx = &self.str_index;
        let covered = idx.covered.clamp(256, self.str_ptr.max(256));
        for s in (covered..self.str_ptr).rev() {
            if self.str_pool[self.str_start[s]..self.str_start[s + 1]] == *want {
                return i32::try_from(s).unwrap_or(0);
            }
        }
        let mut s = idx.heads.get(contents_hash(want)).unwrap_or(0) as usize;
        while s > 255 {
            if s < covered
                && s < self.str_ptr
                && self.str_pool[self.str_start[s]..self.str_start[s + 1]] == *want
            {
                return i32::try_from(s).unwrap_or(0);
            }
            s = idx.links.get(s - 256).map_or(0, |l| l.1 as usize);
        }
        0
    }
}
