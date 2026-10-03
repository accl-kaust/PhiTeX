//! Part 18: The hash table (§256–§267), with web2c's `hash_extra`.

use crate::cmds::{DONT_EXPAND, LET, LETTER};
use crate::eqtb::{
    ACTIVE_BASE, EQTB_SIZE, FONT_ID_BASE, FROZEN_CONTROL_SEQUENCE, FROZEN_DONT_EXPAND, HASH_BASE,
    HASH_PRIME, HASH_SIZE, LEVEL_ONE, NULL_CS, SINGLE_BASE, UNDEFINED_CONTROL_SEQUENCE,
};
use crate::host::Host;
use crate::params::Flavor;
use crate::tex::{Jump, Tex};
use crate::track::{Cell, Tracker};
use crate::web::{FROZEN_NULL_FONT, FROZEN_PRIMITIVE, IGNORE_SPACES, PRIM_EQTB_BASE, PRIM_SIZE};

impl<H: Host, T: Tracker> Tex<H, T> {
    #[inline]
    fn hash_idx(p: i32) -> usize {
        usize::try_from(p - HASH_BASE).expect("hash index below hash_base")
    }

    /// §256: `next(p)`.
    #[inline]
    pub(crate) fn next(&self, p: i32) -> i32 {
        self.tracker.read(Cell::HashNext(p));
        let w = self.hash[Self::hash_idx(p)];
        if T::VALUES {
            self.tracker.read_value(Cell::HashNext(p), w.bits());
        }
        if T::NAMES {
            self.tracker
                .read_content(Cell::HashNext(p), || self.name_content(Cell::HashNext(p)));
        }
        self.memo.read(Cell::Hash(p), w.bits());
        w.lh()
    }
    #[inline]
    pub(crate) fn set_next(&mut self, p: i32, v: i32) {
        self.tracker.write(Cell::HashNext(p));
        self.memo.wrote(Cell::Hash(p));
        let w = &mut self.hash[Self::hash_idx(p)];
        let old = *w;
        w.set_lh(v);
        if T::VALUES {
            let new = *w;
            self.tracker
                .write_value(Cell::HashNext(p), old.bits(), new.bits());
        }
        if T::NAMES {
            // (the slot's version, made at the write: DESIGN 7.17.12)
            self.tracker
                .wrote(Cell::HashNext(p), self.name_content(Cell::HashNext(p)));
        }
    }
    /// `text(p)` without telling the tracker or the memo: for what the
    /// host is told and TeX never prints (`diag.rs`'s suggestions).
    #[inline]
    pub(crate) fn peek_text(&self, p: i32) -> i32 {
        self.hash[Self::hash_idx(p)].rh()
    }
    /// §256: `text(p)`, the string number of the name.
    #[inline]
    pub(crate) fn text(&self, p: i32) -> i32 {
        self.tracker.read(Cell::Hash(p));
        let w = self.hash[Self::hash_idx(p)];
        if T::VALUES {
            self.tracker.read_value(Cell::Hash(p), w.bits());
        }
        if T::NAMES {
            self.tracker
                .read_content(Cell::Hash(p), || self.name_content(Cell::Hash(p)));
        }
        self.memo.read(Cell::Hash(p), w.bits());
        w.rh()
    }
    #[inline]
    pub(crate) fn set_text(&mut self, p: i32, v: i32) {
        self.tracker.write(Cell::Hash(p));
        self.memo.wrote(Cell::Hash(p));
        let w = &mut self.hash[Self::hash_idx(p)];
        let old = *w;
        w.set_rh(v);
        if T::VALUES {
            let new = *w;
            self.tracker
                .write_value(Cell::Hash(p), old.bits(), new.bits());
        }
        if T::NAMES {
            // (the slot's version, made at the write: DESIGN 7.17.12)
            self.tracker
                .wrote(Cell::Hash(p), self.name_content(Cell::Hash(p)));
        }
    }
    /// §256: `font_id_text(f)`.
    pub(crate) fn font_id_text(&self, f: i32) -> i32 {
        self.text(FONT_ID_BASE + f)
    }
    pub(crate) fn set_font_id_text(&mut self, f: i32, v: i32) {
        self.set_text(FONT_ID_BASE + f, v);
    }

    /// §257, §258: initialize the hash (INITEX).
    pub(crate) fn init_hash(&mut self) {
        self.no_new_control_sequence = true; // §257
        self.hash_used = FROZEN_CONTROL_SEQUENCE; // nothing is used
        self.hash_high = 0;
        self.cs_count = 0;
        self.set_eq_type(FROZEN_DONT_EXPAND, DONT_EXPAND);
        let s = self.pool_str(b"notexpanded:");
        self.set_text(FROZEN_DONT_EXPAND, s);
        if self.params.flavor == Flavor::PdfTex {
            // pdfTeX §277
            self.prims.used = PRIM_SIZE; // nothing is used
            self.set_eq_type(FROZEN_PRIMITIVE, IGNORE_SPACES);
            self.set_equiv(FROZEN_PRIMITIVE, 1);
            self.set_eq_level(FROZEN_PRIMITIVE, LEVEL_ONE);
            let s = self.pool_str(b"pdfprimitive");
            self.set_text(FROZEN_PRIMITIVE, s);
        }
    }

    /// §259: find the control sequence `buffer[j..j+l]`, inserting it
    /// unless `no_new_control_sequence`.
    pub(crate) fn id_lookup(&mut self, j: usize, l: usize) -> Result<i32, Jump> {
        // The shortcut: where this text was last found (a hint, checked;
        // a name keeps its location once entered).
        let key = CsCache::key(&self.buffer[j..j + l]);
        if let Some(p) = self.cs_cache.get(key) {
            let t = self.text(p);
            if t > 0 {
                let t = usize::try_from(t).unwrap_or(0);
                if self.length(t) == l && self.str_bytes(t) == &self.buffer[j..j + l] {
                    return Ok(p);
                }
            }
        }
        let p = self.id_lookup_chain(j, l)?;
        if p != UNDEFINED_CONTROL_SEQUENCE {
            self.cs_cache.insert(key, p);
        }
        Ok(p)
    }

    /// A hash slot's word, for a lookup's walk down its chain: the memo
    /// sees the read, the tracker does not (`id_lookup_chain` tells it
    /// what the lookup depended on).
    #[inline]
    /// A hash slot's content for a tracker that keeps versions (§7.17.1):
    /// the name's characters for `text`, the link for `next` (a slot, the
    /// table's layout, which the same names entered in the same order
    /// give again). Untracked.
    pub(crate) fn name_content(&self, cell: Cell) -> u128 {
        let w = usize::try_from(match cell {
            Cell::Hash(p) | Cell::HashNext(p) => p - HASH_BASE,
            _ => -1,
        })
        .ok()
        .filter(|&i| i < self.hash.len())
        .map_or_else(crate::mem::MemoryWord::default, |i| self.hash[i]);
        match cell {
            Cell::Hash(_) => {
                let t = usize::try_from(w.rh()).unwrap_or(0);
                let b: &[u8] = if t > 0 && t < self.str_ptr {
                    self.str_bytes(t)
                } else {
                    &[]
                };
                partex_ssa::Version::of(&(0u8, b)).0
            }
            _ => partex_ssa::Version::of(&(1u8, w.lh())).0,
        }
    }

    /// Where `id_lookup` would find the control sequence named `name` now,
    /// or 0, by tex.web's chains (§259–§261), without entering it or telling
    /// the tracker.
    pub(crate) fn peek_lookup(&self, name: &[u8]) -> i32 {
        let Some((&first, rest)) = name.split_first() else {
            return 0;
        };
        let mut h = i32::from(first);
        for &c in rest {
            h = h + h + i32::from(c);
            while h >= HASH_PRIME {
                h -= HASH_PRIME;
            }
        }
        let mut p = h + HASH_BASE;
        loop {
            let i = Self::hash_idx(p);
            if i >= self.hash.len() {
                return 0;
            }
            let w = self.hash[i];
            let t = usize::try_from(w.rh()).unwrap_or(0);
            if t > 0 && t < self.str_ptr && self.str_bytes(t) == name {
                return p;
            }
            if w.lh() == 0 {
                return 0;
            }
            p = w.lh();
        }
    }

    fn walk(&self, p: i32) -> crate::mem::MemoryWord {
        let w = self.hash[Self::hash_idx(p)];
        if T::VALUES {
            self.tracker.read_value(Cell::Hash(p), w.bits());
        }
        self.memo.read(Cell::Hash(p), w.bits());
        w
    }

    /// `id_lookup` by tex.web's hash chains.
    ///
    /// What a lookup that finds the name, or finds it missing without
    /// entering it, depends on is whether the name is there and where:
    /// the tracker is told the name (`Cell::Str`, keyed by its characters)
    /// and the slot found, not every slot walked past. Two builds whose
    /// tables differ only in other names then read nothing that differs
    /// (a `\label` renamed: its chain's last link differs, and every
    /// lookup down that chain walked past it). Entering a name depends on
    /// the chain's end and the free slots: those reads are told.
    fn id_lookup_chain(&mut self, j: usize, l: usize) -> Result<i32, Jump> {
        // §261: compute the hash code `h`.
        let mut h = i32::from(self.buffer[j]);
        for k in j + 1..j + l {
            h = h + h + i32::from(self.buffer[k]);
            while h >= HASH_PRIME {
                h -= HASH_PRIME;
            }
        }
        let mut p = h + HASH_BASE; // we start searching here
        if self.probe_names() {
            return self.id_lookup_probe(j, l, p);
        }
        let start = p;
        if !T::NAMES {
            self.tracker
                .read(crate::strings::str_cell(&self.buffer[j..j + l]));
        }
        loop {
            let w = self.walk(p);
            let t = w.rh();
            if t > 0 {
                let t = usize::try_from(t).unwrap_or(0);
                if self.length(t) == l && self.str_bytes(t) == &self.buffer[j..j + l] {
                    self.tracker.read(Cell::Hash(p));
                    if T::NAMES {
                        self.tracker.name_lookup(&self.buffer[j..j + l], p);
                    }
                    return Ok(p);
                }
            }
            if w.lh() == 0 {
                if T::NAMES {
                    self.tracker.name_lookup(&self.buffer[j..j + l], 0);
                }
                if self.name_cells {
                    // (a machine's: the name was not there, and is
                    // entered unless no new ones may be)
                    let name = alloc::sync::Arc::from(&self.buffer[j..j + l]);
                    self.name_log.push((name, !self.no_new_control_sequence));
                }
                if self.no_new_control_sequence {
                    p = UNDEFINED_CONTROL_SEQUENCE;
                } else {
                    // (the walk to the chain's end, told)
                    let mut q = start;
                    loop {
                        self.tracker.read(Cell::Hash(q));
                        self.tracker.read(Cell::HashNext(q));
                        if T::NAMES {
                            self.tracker
                                .read_content(Cell::Hash(q), || self.name_content(Cell::Hash(q)));
                            self.tracker.read_content(Cell::HashNext(q), || {
                                self.name_content(Cell::HashNext(q))
                            });
                        }
                        if q == p {
                            break;
                        }
                        q = self.hash[Self::hash_idx(q)].lh();
                    }
                    p = self.insert_cs_after(p, j, l)?;
                }
                return Ok(p);
            }
            p = w.lh();
        }
    }

    /// Whether the names a run makes are placed by probing and found the
    /// same way, outside the hash chains (a machine's, with names as cells
    /// and placed by name; not in INITEX, whose names a format keeps in
    /// chains).
    #[inline]
    fn probe_names(&self) -> bool {
        // (with a `hash_extra` region to probe: without one, as in the trip
        // test, §260's chains)
        self.probe_names
            && self.name_cells
            && self.cs_by_name
            && !self.params.ini
            && self.params.hash_extra > 0
    }

    /// The characters of hash slot `q`'s name equal `buffer[j..j + l]`
    /// (not tracked).
    fn slot_is(&self, q: i32, j: usize, l: usize) -> bool {
        let t = self.hash[Self::hash_idx(q)].rh();
        t > 0 && {
            let t = usize::try_from(t).unwrap_or(0);
            self.length(t) == l && self.str_bytes(t) == &self.buffer[j..j + l]
        }
    }

    /// The places the name in `buffer[j..j + l]` probes in the
    /// `hash_extra` region, in order (after 64 of them one by one, so that
    /// every place is tried).
    fn probes(&self, j: usize, l: usize) -> impl Iterator<Item = i32> + use<H, T> {
        let n = u64::try_from(self.params.hash_extra).unwrap_or(1).max(1);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &self.buffer[j..j + l] {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let step = (h >> 32) | 1;
        let mut k = h;
        (0..n + 64).map(move |i| {
            let q = EQTB_SIZE + 1 + i32::try_from(k % n).unwrap_or(0);
            k = if i < 64 {
                k.wrapping_add(step)
            } else {
                k % n + 1
            };
            q
        })
    }

    /// `id_lookup` with the run's names placed by probing (`probe_names`).
    ///
    /// A name goes to its hash code's slot `start` if that is free, else to
    /// the first free place of its probes; names are never taken away, so
    /// a name is found at `start` or before the first free place of its
    /// probes, and a name missing is missing there. The format's names are
    /// in `start`'s chain, which the run never changes (the names it makes
    /// are not chained). So a lookup depends on the names at `start` and at
    /// the places it probes, which it reads, and not on links: a name made
    /// elsewhere, even in the same chain, changes no lookup of another
    /// name (a `\label`'s `\r@key`, made by one run and not another, used
    /// to change the link of its chain's last slot, which every name
    /// entered later in that chain read).
    fn id_lookup_probe(&mut self, j: usize, l: usize, start: i32) -> Result<i32, Jump> {
        // (the slot of the hash code: a run's name there, or a format's
        // chain from it)
        self.tracker.read(Cell::Hash(start));
        if self.slot_is(start, j, l) {
            return Ok(start);
        }
        if self.hash[Self::hash_idx(start)].rh() == 0 {
            // (no name has this hash code: this one goes here)
            return self.new_name_at(start, j, l);
        }
        let mut q = start;
        loop {
            let next = self.walk(q).lh();
            if next == 0 {
                break;
            }
            q = next;
            if self.slot_is(q, j, l) {
                self.tracker.read(Cell::Hash(q));
                return Ok(q);
            }
        }
        for q in self.probes(j, l) {
            self.tracker.read(Cell::Hash(q));
            if self.slot_is(q, j, l) {
                return Ok(q);
            }
            if self.hash[Self::hash_idx(q)].rh() == 0 {
                return self.new_name_at(q, j, l);
            }
        }
        // (every place is taken: the table's capacity, not tex.web's
        // point of overflow, which the main table's free slots would move)
        let n = HASH_SIZE + self.params.hash_extra;
        self.overflow(b"hash size", n)
    }

    /// A name missing at free slot `q` (`id_lookup_probe`): entered there,
    /// unless no new names may be made.
    fn new_name_at(&mut self, q: i32, j: usize, l: usize) -> Result<i32, Jump> {
        if self.no_new_control_sequence {
            return Ok(UNDEFINED_CONTROL_SEQUENCE);
        }
        if q > EQTB_SIZE {
            // (the count is the table's capacity alone: where the name goes
            // is its own, so it is not read as a value, and a name made
            // depends on no other made before it, DESIGN 3.9's allocators)
            if self.hash_high >= self.params.hash_extra {
                let n = HASH_SIZE + self.params.hash_extra;
                return self.overflow(b"hash size", n);
            }
            self.hash_high += 1;
        }
        self.name_slot(q, j, l)
    }

    /// With `Tex::cs_by_name`: a free place in the `hash_extra` region for
    /// the name in `buffer[j..j + l]`, probed from places the name picks
    /// (so that where a name goes depends on the names made before it only
    /// when one of them took a place it probes). Its probes are told.
    fn extra_slot(&mut self, j: usize, l: usize) -> i32 {
        let n = u64::try_from(self.params.hash_extra).unwrap_or(1).max(1);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in &self.buffer[j..j + l] {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let step = (h >> 32) | 1;
        let mut k = h;
        for i in 0..n + 64 {
            // (after 64 probes, one by one: every place is tried)
            let q = EQTB_SIZE + 1 + i32::try_from(k % n).unwrap_or(0);
            self.tracker.read(Cell::Hash(q));
            if self.text(q) == 0 {
                return q;
            }
            k = if i < 64 {
                k.wrapping_add(step)
            } else {
                k % n + 1
            };
        }
        // (not reached: `hash_high < hash_extra` leaves a place free)
        self.hash_high + 1 + EQTB_SIZE
    }

    /// A read of the hash's extra area's allocator (`hash_high`), for a
    /// tracker that keeps versions (DESIGN 7.17.12, the allocator's row).
    fn hash_high_read(&self) {
        if T::VALUES {
            self.tracker.row_read(
                crate::track::Row::Scalar(crate::track::scalar::HASH_HIGH),
                || crate::track::scalar_version(usize::try_from(self.hash_high).unwrap_or(0)),
            );
        }
    }

    /// `hash_high` was written: its version, the value.
    fn hash_high_wrote(&self) {
        if T::VALUES {
            self.tracker.row_wrote(
                crate::track::Row::Scalar(crate::track::scalar::HASH_HIGH),
                crate::track::scalar_version(usize::try_from(self.hash_high).unwrap_or(0)),
            );
        }
    }

    /// §260: insert a new control sequence after `p`, then return it.
    fn insert_cs_after(&mut self, mut p: i32, j: usize, l: usize) -> Result<i32, Jump> {
        if self.text(p) > 0 {
            self.hash_high_read();
            if self.hash_high < self.params.hash_extra {
                // (not in INITEX: a format's names in `hash_extra` must be
                // packed from its start, where §260 of the run that loads
                // it goes on putting them)
                let q = if self.cs_by_name && !self.params.ini {
                    self.extra_slot(j, l)
                } else {
                    self.hash_high + 1 + EQTB_SIZE
                };
                self.hash_high += 1;
                self.hash_high_wrote();
                self.set_next(p, q);
                p = q;
            } else {
                if T::VALUES {
                    self.tracker.row_read(
                        crate::track::Row::Scalar(crate::track::scalar::HASH_USED),
                        || {
                            crate::track::scalar_version(
                                usize::try_from(self.hash_used).unwrap_or(0),
                            )
                        },
                    );
                }
                loop {
                    if self.hash_used == HASH_BASE {
                        // hash_is_full
                        let n = HASH_SIZE + self.params.hash_extra;
                        return self.overflow(b"hash size", n);
                    }
                    self.hash_used -= 1;
                    if T::VALUES {
                        self.tracker.row_wrote(
                            crate::track::Row::Scalar(crate::track::scalar::HASH_USED),
                            crate::track::scalar_version(
                                usize::try_from(self.hash_used).unwrap_or(0),
                            ),
                        );
                    }
                    if self.text(self.hash_used) == 0 {
                        break;
                    }
                }
                self.set_next(p, self.hash_used);
                p = self.hash_used;
            }
        }
        self.name_slot(p, j, l)
    }

    /// §260's end: slot `p` gets the name `buffer[j..j + l]`, a new string.
    fn name_slot(&mut self, p: i32, j: usize, l: usize) -> Result<i32, Jump> {
        self.str_room(l)?;
        let d = self.cur_length();
        // Move the current string up to make room for another.
        while self.pool_ptr > self.str_start[self.str_ptr] {
            self.pool_ptr -= 1;
            self.str_pool[self.pool_ptr + l] = self.str_pool[self.pool_ptr];
        }
        for k in j..j + l {
            self.append_char(self.buffer[k]);
        }
        let s = self.make_string()?;
        self.set_text(p, i32::try_from(s).unwrap_or(0));
        self.pool_ptr += d;
        self.cs_count += 1;
        Ok(p)
    }

    /// §262: print a purported control sequence (with web2c's encTeX hooks).
    pub(crate) fn print_cs(&mut self, p: i32) {
        if self.active_noconvert
            && !self.no_convert
            && self.eq_type(p) == LET
            && self.equiv(p) == crate::nodes::NORMAL + 11
        {
            // \noconvert
            self.no_convert = true;
            return;
        }
        // encTeX's `mubyte_cswrite` conversion needs `cs_converting`, which
        // is never set without encTeX.
        self.no_convert = false;
        if p < HASH_BASE {
            // single character
            if p >= SINGLE_BASE {
                if p == NULL_CS {
                    self.print_esc(b"csname");
                    self.print_esc(b"endcsname");
                    self.print_char(b' ');
                } else {
                    self.print_esc_num(p - SINGLE_BASE);
                    if self.cat_code(p - SINGLE_BASE) == LETTER {
                        self.print_char(b' ');
                    }
                }
            } else if p < ACTIVE_BASE {
                self.print_esc(b"IMPOSSIBLE.");
            } else {
                self.print(p - ACTIVE_BASE);
            }
        } else if (p >= UNDEFINED_CONTROL_SEQUENCE && p <= EQTB_SIZE) || p > self.eqtb_top {
            self.print_esc(b"IMPOSSIBLE.");
        } else if usize::try_from(self.text(p)).map_or(true, |t| t >= self.str_ptr) {
            self.print_esc(b"NONEXISTENT.");
        } else {
            self.print_esc_num(self.cs_text(p));
            self.print_char(b' ');
        }
    }

    /// §263: print a control sequence without error checks or a space.
    pub(crate) fn sprint_cs(&mut self, p: i32) {
        if p < HASH_BASE {
            if p < SINGLE_BASE {
                self.print(p - ACTIVE_BASE);
            } else if p < NULL_CS {
                self.print_esc_num(p - SINGLE_BASE);
            } else {
                self.print_esc(b"csname");
                self.print_esc(b"endcsname");
            }
        } else {
            self.print_esc_num(self.cs_text(p));
        }
    }

    /// §264: enter a primitive into eqtb (INITEX). `s` is its pool string.
    pub(crate) fn primitive(&mut self, s: &[u8], c: i32, o: i32) -> Result<(), Jump> {
        if self.params.flavor == Flavor::PdfTex {
            return self.pdftex_primitive(s, c, o);
        }
        if s.len() == 1 {
            self.cur_val = i32::from(s[0]) + SINGLE_BASE;
        } else {
            let s = self.pool_str(s);
            let su = usize::try_from(s).unwrap_or(0);
            let k = self.str_start[su];
            let l = self.str_start[su + 1] - k;
            // We move `s` into the (empty) buffer.
            for j in 0..l {
                self.buffer[j] = self.str_pool[k + j];
            }
            self.cur_val = self.id_lookup(0, l)?; // no_new_control_sequence is false
            self.flush_string();
            self.set_text(self.cur_val, s); // we don't want to have the string twice
        }
        self.set_eq_level(self.cur_val, LEVEL_ONE);
        self.set_eq_type(self.cur_val, c);
        self.set_equiv(self.cur_val, o);
        Ok(())
    }

    /// pdfTeX §286: `primitive`, which also enters the primitive table and
    /// works while the buffer holds the first line (e-TeX's primitives are
    /// generated then).
    fn pdftex_primitive(&mut self, s: &[u8], c: i32, o: i32) -> Result<(), Jump> {
        let prim_val = if s.len() == 1 {
            self.cur_val = i32::from(s[0]) + SINGLE_BASE;
            self.prim_lookup(i32::from(s[0]))?
        } else {
            let s = self.pool_str(s);
            let su = usize::try_from(s).unwrap_or(0);
            let k = self.str_start[su];
            let l = self.str_start[su + 1] - k;
            // We move `s` into the (possibly non-empty) buffer.
            if self.first + l > usize::try_from(self.params.buf_size).unwrap_or(0) + 1 {
                let n = self.params.buf_size;
                return self.overflow(b"buffer size", n);
            }
            let first = self.first;
            for j in 0..l {
                self.buffer[first + j] = self.str_pool[k + j];
            }
            self.cur_val = self.id_lookup(first, l)?; // no_new_control_sequence is false
            self.flush_string();
            self.set_text(self.cur_val, s); // we don't want to have the string twice
            self.prim_lookup(s)?
        };
        self.set_eq_level(self.cur_val, LEVEL_ONE);
        self.set_eq_type(self.cur_val, c);
        self.set_equiv(self.cur_val, o);
        self.prims.by_meaning = None;
        self.set_eq_level(PRIM_EQTB_BASE + prim_val, LEVEL_ONE);
        self.set_eq_type(PRIM_EQTB_BASE + prim_val, c);
        self.set_equiv(PRIM_EQTB_BASE + prim_val, o);
        Ok(())
    }

    /// pdfTeX §284: the name of `eqtb` location `p`: a primitive's
    /// permanent location names the primitive.
    fn cs_text(&self, p: i32) -> i32 {
        if (PRIM_EQTB_BASE..FROZEN_NULL_FONT).contains(&p) {
            self.prims.text[usize::try_from(p - PRIM_EQTB_BASE).unwrap_or(0)] - 1
        } else {
            self.text(p)
        }
    }

    /// The pool string number of a string that tex.web writes as a literal.
    /// Such strings exist in the pool by construction (they come from
    /// `tex.pool`); a miss is a porting bug.
    pub(crate) fn pool_str(&self, s: &[u8]) -> i32 {
        let known = match s {
            b"" => self.str_index.empty,
            b"///..." => self.str_index.copied,
            _ => 0,
        };
        if known > 0 {
            return known;
        }
        let n = self
            .find_pool_string(s)
            .unwrap_or_else(|| panic!("not a pool string: {:?}", core::str::from_utf8(s)));
        i32::try_from(n).unwrap_or(0)
    }

    /// §267: print the font identifier for font `f`.
    pub(crate) fn print_font_id(&mut self, f: i32) {
        let t = self.font_id_text(f);
        self.print_esc_num(t);
    }
}

#[cfg(test)]
mod tests {
    use crate::cmds::{ASSIGN_INT, RELAX};
    use crate::eqtb::{INT_BASE, SINGLE_BASE, TOLERANCE_CODE, UNDEFINED_CONTROL_SEQUENCE};
    use crate::testing::{engine, term_output};

    #[test]
    fn primitives_and_lookup() {
        let mut t = engine();
        t.no_new_control_sequence = false;
        t.primitive(b"relax", RELAX, 256).unwrap();
        let relax = t.cur_val;
        t.primitive(b"tolerance", ASSIGN_INT, INT_BASE + TOLERANCE_CODE)
            .unwrap();
        let tol = t.cur_val;
        t.primitive(b"/", 44, 0).unwrap();
        assert_eq!(t.cur_val, SINGLE_BASE + i32::from(b'/'));
        // `primitive` reuses the pool string instead of making a copy.
        assert_eq!(t.str_ptr, t.init_str_ptr.max(256 + 1093));
        assert_eq!(t.cs_count, 2);

        t.no_new_control_sequence = true;
        t.buffer[..5].copy_from_slice(b"relax");
        assert_eq!(t.id_lookup(0, 5).unwrap(), relax);
        t.buffer[..3].copy_from_slice(b"foo");
        assert_eq!(t.id_lookup(0, 3).unwrap(), UNDEFINED_CONTROL_SEQUENCE);
        assert_eq!(
            (t.eq_type(tol), t.equiv(tol)),
            (ASSIGN_INT, INT_BASE + TOLERANCE_CODE)
        );

        let out = term_output(&mut t, |t| {
            t.print_cs(relax);
            t.print_cs(SINGLE_BASE + i32::from(b'/'));
            t.print_cs(SINGLE_BASE + i32::from(b'a'));
            t.sprint_cs(tol);
        });
        assert_eq!(out, b"\\relax \\/\\a \\tolerance");
    }

    /// Oracle: `tex -ini '\dump'` reports "322 multiletter control
    /// sequences"; its `tex` has no `\synctex`.
    #[test]
    fn init_prim_matches_initex() {
        let mut t = engine();
        t.init_prim().unwrap();
        assert_eq!(t.cs_count, 322);
        // The primitives reuse pool strings: no new strings.
        assert_eq!(t.str_ptr, 256 + 1093);
        t.buffer[..7].copy_from_slice(b"synctex");
        assert_eq!(t.id_lookup(0, 7).unwrap(), UNDEFINED_CONTROL_SEQUENCE);
    }
}

/// `id_lookup`'s shortcut: from a hash of a name's text to where it was
/// found, in a direct-mapped table of tagged words. Every clone of an
/// engine shares it (a checkpoint's copy would cost megabytes): a
/// location is only a hint, checked against the name there, so one
/// entered by another run (a rebuild, or another thread's engine) is
/// harmless, and so are races (a relaxed load is a plain load).
#[derive(Clone, Debug)]
pub(crate) struct CsCache(alloc::sync::Arc<[core::sync::atomic::AtomicU64]>);

/// Slots of [`CsCache`] (a large document has ~100k names).
const CS_CACHE_BITS: u32 = 18;

impl Default for CsCache {
    fn default() -> Self {
        Self(
            (0..1usize << CS_CACHE_BITS)
                .map(|_| core::sync::atomic::AtomicU64::new(0))
                .collect(),
        )
    }
}

impl CsCache {
    /// The key of a name's text.
    #[inline]
    fn key(text: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in text {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    #[inline]
    fn slot(&self, key: u64) -> &core::sync::atomic::AtomicU64 {
        // (the low bits pick the slot, the high half is the tag)
        let mask = (1u64 << CS_CACHE_BITS) - 1;
        &self.0[usize::try_from(key & mask).unwrap_or(0)]
    }

    #[inline]
    fn get(&self, key: u64) -> Option<i32> {
        let v = self.slot(key).load(core::sync::atomic::Ordering::Relaxed);
        (v != 0 && v >> 32 == key >> 32).then(|| {
            // (the low half is the location)
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let p = v as u32 as i32;
            p
        })
    }

    fn insert(&self, key: u64, p: i32) {
        let v = (key >> 32) << 32 | u64::from(p.cast_unsigned());
        self.slot(key)
            .store(v, core::sync::atomic::Ordering::Relaxed);
    }
}
