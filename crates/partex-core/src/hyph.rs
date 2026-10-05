//! Parts 40–43: Pre-hyphenation (§891–§899), Post-hyphenation
//! (§900–§918), Hyphenation (§919–§941) and Initializing the hyphenation
//! tables (§942–§966).

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use partex_engine::hyph::{
    Exceptions, HYPHENATABLE_LENGTH_LIMIT, MAX_WORD, Patterns, entry_version,
};
use partex_ssa::Version;

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::{Row, Tracker, hyph as slot};
use crate::web::*;

/// §11 (web2c): hyphenation op table sizes.
pub(crate) const TRIE_OP_SIZE: i32 = 35111;
const NEG_TRIE_OP_SIZE: i32 = -35111;
const MIN_TRIE_OP: i32 = 0;
const MAX_TRIE_OP: i32 = 65535;
/// §12 (web2c): `hyph_prime`.
pub(crate) const HYPH_PRIME: i32 = 607;

fn ux(a: i32) -> usize {
    usize::try_from(a).unwrap_or(0)
}

/// `XeTeX`'s `too_big_lang`: where `max_hyph_char` starts.
const TOO_BIG_LANG: i32 = 256;
/// `XeTeX`'s `biggest_char`: the trie's characters are UTF-16 units.
const BIGGEST_CHAR: i32 = 0xFFFF;

/// The hyphenation globals (§892, §900, §905, §921, §926, §943, §947, §950;
/// `hc` and `hyf` are the locals of `\patterns` and `\hyphenation`).
#[derive(Clone)]
pub(crate) struct HyphState {
    pub(crate) cur_lang: i32,
    /// §921: the packed trie (the engine hyphenates with it).
    pub(crate) patterns: Arc<Patterns>,
    // §926: the exception dictionary: `hyph_word` and `hyph_link` keep
    // tex.web's hash table (for its capacity and `hyph_count`); the
    // hyphen positions are in `exceptions`.
    /// (shared by checkpoints until written: a document rarely adds
    /// exceptions after its preamble)
    pub(crate) hyph_word: Arc<Vec<i32>>,
    pub(crate) hyph_link: Arc<Vec<i32>>,
    pub(crate) hyph_count: i32,
    pub(crate) hyph_next: i32,
    pub(crate) exceptions: Exceptions,
    // §943
    pub(crate) trie_op_hash: Vec<i32>,
    pub(crate) trie_used: [i32; 256],
    pub(crate) trie_op_lang: Vec<i32>,
    pub(crate) trie_op_val: Vec<i32>,
    pub(crate) trie_op_ptr: i32,
    pub(crate) max_op_used: i32,
    // §947: the linked trie.
    pub(crate) trie_c: Vec<i32>,
    pub(crate) trie_o: Vec<i32>,
    pub(crate) trie_l: Vec<i32>,
    pub(crate) trie_r: Vec<i32>,
    pub(crate) trie_ptr: i32,
    pub(crate) trie_hash: Vec<i32>,
    // §950
    pub(crate) trie_taken: Vec<bool>,
    /// (by character: TeX's 256, `XeTeX`'s up to `max_hyph_char`; made
    /// for packing)
    pub(crate) trie_min: Vec<i32>,
    pub(crate) trie_max: i32,
    pub(crate) trie_not_ready: bool,
    /// The patterns' version (DESIGN 7.17.12's `hyph` row): while INITEX
    /// builds the trie, made from the patterns and saved codes entered in
    /// order (§960–§965), which decide the linked trie; once packed
    /// (§966), from that; from a format, by the packed trie's content (0:
    /// not made yet, `Tex::version_hyph`). Fixed once packed.
    pub(crate) pat_ver: u128,
}

partex_engine::persist_struct!(HyphState {
    cur_lang,
    patterns,
    hyph_word,
    hyph_link,
    hyph_count,
    hyph_next,
    exceptions,
    trie_op_hash,
    trie_used,
    trie_op_lang,
    trie_op_val,
    trie_op_ptr,
    max_op_used,
    trie_c,
    trie_o,
    trie_l,
    trie_r,
    trie_ptr,
    trie_hash,
    trie_taken,
    trie_min,
    trie_max,
    trie_not_ready,
    pat_ver
});

impl HyphState {
    /// Hash the state later hyphenation depends on (`statehash`): the
    /// linked trie only while patterns can still be added.
    /// (`shared` are the hashes of `patterns`, `hyph_word` and
    /// `hyph_link`, which the caller may know by their addresses.)
    pub(crate) fn hash_state<S: core::hash::Hasher>(&self, h: &mut S, shared: [u128; 3]) {
        use core::hash::Hash;
        let Self {
            cur_lang,
            patterns: _,
            hyph_word: _,
            hyph_link: _,
            hyph_count,
            hyph_next,
            exceptions,
            trie_op_hash,
            trie_used,
            trie_op_lang,
            trie_op_val,
            trie_op_ptr,
            max_op_used,
            trie_c,
            trie_o,
            trie_l,
            trie_r,
            trie_ptr,
            trie_hash,
            trie_taken,
            trie_min,
            trie_max,
            trie_not_ready,
            pat_ver: _,
        } = self;
        (cur_lang, shared, hyph_count, hyph_next).hash(h);
        (
            exceptions,
            trie_used,
            trie_op_ptr,
            max_op_used,
            trie_min,
            trie_max,
        )
            .hash(h);
        trie_not_ready.hash(h);
        if *trie_not_ready {
            (trie_op_hash, trie_op_lang, trie_op_val).hash(h);
            (
                trie_c, trie_o, trie_l, trie_r, trie_ptr, trie_hash, trie_taken,
            )
                .hash(h);
        }
    }

    /// The tables; the ones that build the trie only for INITEX (`ini`).
    pub(crate) fn new(trie_size: i32, hyph_size: i32, ini: bool) -> Self {
        let t = ux(trie_size) + 1;
        let h = ux(hyph_size) + 1;
        let ops = ux(TRIE_OP_SIZE) + 1;
        let (bt, bops) = if ini { (t, ops) } else { (0, 1) };
        // §928
        let mut hyph_next = HYPH_PRIME + 1;
        if hyph_next > hyph_size {
            hyph_next = HYPH_PRIME;
        }
        Self {
            cur_lang: 0,
            patterns: Arc::new(Patterns {
                link: vec![0; t],
                op: vec![0; t],
                ch: vec![0; t],
                distance: vec![0; ops],
                num: vec![0; ops],
                next: vec![0; ops],
                op_start: vec![0; 256],
                hyph_codes: BTreeMap::new(),
                max_hyph_char: TOO_BIG_LANG,
            }),
            hyph_word: Arc::new(vec![0; h]),
            hyph_link: Arc::new(vec![0; h]),
            hyph_count: 0,
            hyph_next,
            exceptions: Exceptions::default(),
            // §946
            trie_op_hash: vec![0; 2 * bops - 1],
            trie_used: [MIN_TRIE_OP; 256],
            trie_op_lang: vec![0; bops],
            trie_op_val: vec![0; bops],
            trie_op_ptr: 0,
            max_op_used: MIN_TRIE_OP,
            trie_c: vec![0; bt],
            trie_o: vec![0; bt],
            trie_l: vec![0; bt],
            trie_r: vec![0; bt],
            trie_ptr: 0,
            trie_hash: vec![0; bt],
            trie_taken: vec![false; bt],
            trie_min: Vec::new(),
            trie_max: 0,
            trie_not_ready: true, // §951
            pat_ver: Version::node(0x7061_7400, &[]).0,
        }
    }

    /// The patterns' version from their packed content (a format's).
    fn packed_version(&self) -> u128 {
        let p = &self.patterns;
        let tm = ux(self.trie_max) + 1;
        let ops = ux(self.trie_op_ptr) + 1;
        let cut = |v: &[i32], n: usize| v.get(..n).unwrap_or(v).to_vec();
        Version::of(&(
            cut(&p.link, tm),
            cut(&p.op, tm),
            cut(&p.ch, tm),
            cut(&p.distance, ops),
            cut(&p.num, ops),
            cut(&p.next, ops),
            &p.op_start,
            &p.hyph_codes,
            self.trie_used,
            p.max_hyph_char,
        ))
        .0
    }

    /// The exception table's version: the map, with `hyph_count` and
    /// `hyph_next` (§926, §940).
    fn exceptions_version(&self) -> u128 {
        Version::node(
            0x6578_6300,
            &[
                self.exceptions.version(),
                Version(u128::from(self.hyph_count.cast_unsigned())),
                Version(u128::from(self.hyph_next.cast_unsigned())),
            ],
        )
        .0
    }

    /// The packed trie, to change (copied first if shared).
    pub(crate) fn pat(&mut self) -> &mut Patterns {
        Arc::make_mut(&mut self.patterns)
    }

    /// `trie_op_hash[h]` for `h` in `neg_trie_op_size..trie_op_size`.
    fn op_hash(&mut self, h: i32) -> &mut i32 {
        &mut self.trie_op_hash[ux(h - NEG_TRIE_OP_SIZE)]
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// A read of the patterns (and of whether they are packed, §891).
    #[inline]
    pub(crate) fn patterns_read(&self) {
        if T::VALUES {
            self.tracker
                .row_read(Row::Hyph(slot::PATTERNS), || self.hyph.pat_ver);
        }
    }

    /// The patterns changed: `part` (a pattern entered, codes saved, the
    /// trie packed) is folded into their version.
    fn patterns_wrote(&mut self, part: u128) {
        let v = Version::node(0x7061_7401, &[Version(self.hyph.pat_ver), Version(part)]).0;
        self.hyph.pat_ver = v;
        if T::VALUES {
            self.tracker.row_wrote(Row::Hyph(slot::PATTERNS), v);
        }
    }

    /// A read of the exception table as a whole (what an insertion and
    /// the statistics read).
    pub(crate) fn exceptions_read(&self) {
        if T::VALUES {
            self.tracker.row_read(Row::Hyph(slot::EXCEPTIONS), || {
                self.hyph.exceptions_version()
            });
        }
    }

    /// The exception table was written (as a whole).
    pub(crate) fn exceptions_wrote(&self) {
        if T::VALUES {
            self.tracker
                .row_wrote(Row::Hyph(slot::EXCEPTIONS), self.hyph.exceptions_version());
        }
    }

    /// §930: the hyphen positions of the word `key` (`exception_key`), if
    /// it is an exception: a read of the word's entry, by the word
    /// (DESIGN 7.17.12: the exceptions a persistent map by word).
    #[allow(clippy::ptr_arg, reason = "the map's key, looked up as it is")]
    pub(crate) fn exception(&self, key: &Vec<u8>) -> Option<&[u16]> {
        let e = self.hyph.exceptions.entry(key);
        if T::VALUES {
            self.tracker.hyph_word_read(key, entry_version(e).0);
        }
        e.map(|p| &p.at[..])
    }

    /// Hyphenation's slots as versions, for a writer that stores them
    /// wholesale past the accessors (a format's load, §1325; the engine as
    /// made): `Tex::version_tables`.
    pub(crate) fn version_hyph(&mut self) {
        if !T::VALUES {
            return;
        }
        if self.hyph.pat_ver == 0 {
            self.hyph.pat_ver = self.hyph.packed_version();
        }
        // (a word's entry is read by content, so it has no version to make)
        self.tracker
            .row_made(Row::Hyph(slot::PATTERNS), self.hyph.pat_ver);
        self.tracker
            .row_made(Row::Hyph(slot::EXCEPTIONS), self.hyph.exceptions_version());
    }

    /// §934: `set_cur_lang`.
    pub(crate) fn set_cur_lang(&mut self) {
        let l = self.int_par(LANGUAGE_CODE);
        self.hyph.cur_lang = if (1..=255).contains(&l) { l } else { 0 };
    }

    /// The most letters a hyphenated word may have: TeX's 63, `XeTeX`'s
    /// `max_hyphenatable_length` (`\XeTeXhyphenatablelength`, at most
    /// `hyphenatable_length_limit`).
    pub(crate) fn max_hyphenatable_length(&self) -> i32 {
        if self.unicode {
            self.eqtb_int(ETEX_STATE_BASE + XETEX_HYPHENATABLE_LENGTH_CODE)
                .min(HYPHENATABLE_LENGTH_LIMIT)
        } else {
            MAX_WORD
        }
    }

    /// §934: enter new exceptions (`\hyphenation`).
    pub(crate) fn new_hyph_exceptions(&mut self) -> Result<(), Jump> {
        self.scan_left_brace()?; // a left brace must follow \hyphenation
        self.set_cur_lang();
        // §935: enter as many hyphenation exceptions as are listed, until
        // coming to a right brace; then `return`. (`hc[1..=n]`: `XeTeX`'s
        // are UTF-16 units, §991)
        let mut hc: Vec<i32> = Vec::new();
        let mut p: Vec<u16> = Vec::new();
        loop {
            self.get_x_token()?;
            loop {
                // reswitch:
                match self.cur_cmd {
                    LETTER | OTHER_CHAR | CHAR_GIVEN => {
                        // §937: append a new letter or hyphen.
                        let max = self.max_hyphenatable_length();
                        let n = i32::try_from(hc.len()).unwrap_or(i32::MAX);
                        if self.cur_chr == i32::from(b'-') {
                            // §938: append the value `n` to list `p`.
                            if n < max {
                                p.push(u16::try_from(n).unwrap_or(0));
                            }
                        } else {
                            let h = self.hyph_code(self.cur_chr);
                            if h == 0 {
                                self.print_err(b"Not a letter");
                                self.help(&[
                                    b"Letters in \\hyphenation words must have \\lccode>0.",
                                    b"Proceed; I'll ignore the character I just read.",
                                ]);
                                self.error()?;
                            } else if n < max {
                                if h < 0x1_0000 {
                                    hc.push(h);
                                } else {
                                    hc.push((h - 0x1_0000) / 0x400 + 0xD800);
                                    hc.push(h % 0x400 + 0xDC00);
                                }
                            }
                        }
                    }
                    CHAR_NUM => {
                        self.scan_char_num()?;
                        self.cur_chr = self.cur_val;
                        self.cur_cmd = CHAR_GIVEN;
                        continue;
                    }
                    SPACER | RIGHT_BRACE => {
                        if hc.len() > 1 {
                            self.enter_hyph_exception(&hc, core::mem::take(&mut p))?;
                        }
                        if self.cur_cmd == RIGHT_BRACE {
                            return Ok(());
                        }
                        hc.clear();
                        p.clear();
                    }
                    _ => {
                        // §936: give improper \hyphenation error.
                        self.print_err(b"Improper ");
                        self.print_esc(b"hyphenation");
                        self.print_str(b" will be flushed");
                        self.help(&[
                            b"Hyphenation exceptions must contain only letters",
                            b"and hyphens. But continue; I'll forgive and forget.",
                        ]);
                        self.error()?;
                    }
                }
                break;
            }
        }
    }

    /// §939: enter a hyphenation exception, the letters `hc[1..=n]`.
    fn enter_hyph_exception(&mut self, hc: &[i32], p: Vec<u16>) -> Result<(), Jump> {
        let lang = self.hyph.cur_lang;
        self.str_room(hc.len() + 1)?;
        let mut h = 0;
        for &c in hc.iter().chain(core::iter::once(&lang)) {
            h = (h + h + c) % HYPH_PRIME;
            self.append_char(c.cast_unsigned());
        }
        let mut s = i32::try_from(self.make_string()?).unwrap_or(0);
        // §940: insert the pair `(s,p)` into the exception table (a read of
        // the table as a whole, and of the word's entry: whether it is
        // there).
        let key = self.str_bytes(ux(s)).to_vec();
        self.exceptions_read();
        let _ = self.exception(&key);
        let hyph_size = self.params.hyph_size;
        if self.hyph.hyph_next <= HYPH_PRIME {
            while self.hyph.hyph_next > 0 && self.hyph.hyph_word[ux(self.hyph.hyph_next - 1)] > 0 {
                self.hyph.hyph_next -= 1;
            }
        }
        if self.hyph.hyph_count == hyph_size || self.hyph.hyph_next == 0 {
            return self.overflow(b"exception dictionary", hyph_size);
        }
        self.hyph.hyph_count += 1;
        while self.hyph.hyph_word[ux(h)] != 0 {
            // §941: if the string `hyph_word[h]` is less than or equal to
            // `s`, interchange `hyph_word[h]` with `s`.
            let k = self.hyph.hyph_word[ux(h)];
            if self.str_bytes(ux(k)) == self.str_bytes(ux(s)) {
                // repeat hyphenation exception; flushing old data
                self.flush_string();
                s = self.hyph.hyph_word[ux(h)]; // avoid `slow_make_string`!
                self.hyph.hyph_count -= 1;
                break;
            }
            // not_found:
            if self.hyph.hyph_link[ux(h)] == 0 {
                Arc::make_mut(&mut self.hyph.hyph_link)[ux(h)] = self.hyph.hyph_next;
                if self.hyph.hyph_next >= hyph_size {
                    self.hyph.hyph_next = HYPH_PRIME;
                }
                if self.hyph.hyph_next > HYPH_PRIME {
                    self.hyph.hyph_next += 1;
                }
            }
            h = self.hyph.hyph_link[ux(h)] - 1;
        }
        // found:
        Arc::make_mut(&mut self.hyph.hyph_word)[ux(h)] = s;
        let _ = self.hyph.exceptions.insert(key.clone(), p);
        self.exceptions_wrote();
        if T::VALUES {
            self.tracker.hyph_word_wrote(&key);
        }
        Ok(())
    }

    /// §944
    fn new_trie_op(&mut self, d: i32, n: i32, v: i32) -> Result<i32, Jump> {
        let cur_lang = self.hyph.cur_lang;
        let mut h = (n + 313 * d + 361 * v + 1009 * cur_lang).abs()
            % (TRIE_OP_SIZE - NEG_TRIE_OP_SIZE)
            + NEG_TRIE_OP_SIZE;
        loop {
            let l = *self.hyph.op_hash(h);
            if l == 0 {
                // empty position found for a new op
                if self.hyph.trie_op_ptr == TRIE_OP_SIZE {
                    return self.overflow(b"pattern memory ops", TRIE_OP_SIZE);
                }
                let mut u = self.hyph.trie_used[ux(cur_lang)];
                if u == MAX_TRIE_OP {
                    return self.overflow(
                        b"pattern memory ops per language",
                        MAX_TRIE_OP - MIN_TRIE_OP,
                    );
                }
                self.hyph.trie_op_ptr += 1;
                u += 1;
                self.hyph.trie_used[ux(cur_lang)] = u;
                if u > self.hyph.max_op_used {
                    self.hyph.max_op_used = u;
                }
                let ptr = ux(self.hyph.trie_op_ptr);
                self.hyph.pat().distance[ptr] = d;
                self.hyph.pat().num[ptr] = n;
                self.hyph.pat().next[ptr] = v;
                self.hyph.trie_op_lang[ptr] = cur_lang;
                *self.hyph.op_hash(h) = self.hyph.trie_op_ptr;
                self.hyph.trie_op_val[ptr] = u;
                return Ok(u);
            }
            let lu = ux(l);
            if self.hyph.patterns.distance[lu] == d
                && self.hyph.patterns.num[lu] == n
                && self.hyph.patterns.next[lu] == v
                && self.hyph.trie_op_lang[lu] == cur_lang
            {
                return Ok(self.hyph.trie_op_val[lu]);
            }
            if h > -TRIE_OP_SIZE {
                h -= 1;
            } else {
                h = TRIE_OP_SIZE;
            }
        }
    }

    /// §948: convert to a canonical form.
    fn trie_node(&mut self, p: i32) -> i32 {
        let y = &mut self.hyph;
        let pu = ux(p);
        let trie_size = self.params.trie_size;
        let hv = i64::from(y.trie_c[pu])
            + 1009 * i64::from(y.trie_o[pu])
            + 2718 * i64::from(y.trie_l[pu])
            + 3142 * i64::from(y.trie_r[pu]);
        let mut h = i32::try_from(hv.abs() % i64::from(trie_size)).unwrap_or(0);
        loop {
            let q = y.trie_hash[ux(h)];
            if q == 0 {
                y.trie_hash[ux(h)] = p;
                return p;
            }
            let qu = ux(q);
            if y.trie_c[qu] == y.trie_c[pu]
                && y.trie_o[qu] == y.trie_o[pu]
                && y.trie_l[qu] == y.trie_l[pu]
                && y.trie_r[qu] == y.trie_r[pu]
            {
                return q;
            }
            if h > 0 {
                h -= 1;
            } else {
                h = trie_size;
            }
        }
    }

    /// §949
    fn compress_trie(&mut self, p: i32) -> i32 {
        if p == 0 {
            return 0;
        }
        let l = self.compress_trie(self.hyph.trie_l[ux(p)]);
        self.hyph.trie_l[ux(p)] = l;
        let r = self.compress_trie(self.hyph.trie_r[ux(p)]);
        self.hyph.trie_r[ux(p)] = r;
        self.trie_node(p)
    }

    /// §953: pack a family into `trie`.
    fn first_fit(&mut self, p: i32) -> Result<(), Jump> {
        // (`XeTeX`: `max_hyph_char` where TeX has 256)
        let mhc = self.hyph.patterns.max_hyph_char;
        let c = self.hyph.trie_c[ux(p)];
        let mut z = self.hyph.trie_min[ux(c)]; // get the first conceivably good hole
        let h = 'found: loop {
            let h = z - c;
            // §954: ensure that `trie_max>=h+max_hyph_char`.
            if self.hyph.trie_max < h + mhc {
                if self.params.trie_size <= h + mhc {
                    return self.overflow(b"pattern memory", self.params.trie_size);
                }
                loop {
                    let y = &mut self.hyph;
                    y.trie_max += 1;
                    let m = ux(y.trie_max);
                    y.trie_taken[m] = false;
                    let (i, v) = (m, y.trie_max + 1);
                    y.pat().link[i] = v;
                    let (i, v) = (m, y.trie_max - 1); // `trie_back`
                    y.pat().op[i] = v;
                    if y.trie_max == h + mhc {
                        break;
                    }
                }
            }
            'not_found: {
                if self.hyph.trie_taken[ux(h)] {
                    break 'not_found;
                }
                // §955: if all characters of the family fit relative to
                // `h`, then `goto found`, otherwise `goto not_found`.
                let mut q = self.hyph.trie_r[ux(p)];
                while q > 0 {
                    if self.hyph.patterns.link[ux(h + self.hyph.trie_c[ux(q)])] == 0 {
                        break 'not_found;
                    }
                    q = self.hyph.trie_r[ux(q)];
                }
                break 'found h;
            }
            z = self.hyph.patterns.link[ux(z)]; // move to the next hole
        };
        // found: §956: pack the family into `trie` relative to `h`.
        let y = &mut self.hyph;
        y.trie_taken[ux(h)] = true;
        y.trie_hash[ux(p)] = h; // `trie_ref`
        let mut q = p;
        loop {
            let z = h + y.trie_c[ux(q)];
            let mut l = y.patterns.op[ux(z)];
            let r = y.patterns.link[ux(z)];
            y.pat().op[ux(r)] = l;
            y.pat().link[ux(l)] = r;
            y.pat().link[ux(z)] = 0;
            if l < mhc {
                let ll = if z < mhc { z } else { mhc };
                loop {
                    // (`XeTeX`'s `trie_min` ends at `biggest_char`: past
                    // it, a pattern's character above 0xFFFF writes out
                    // of the array, which nothing reads)
                    if let Some(m) = y.trie_min.get_mut(ux(l)) {
                        *m = r;
                    }
                    l += 1;
                    if l == ll {
                        break;
                    }
                }
            }
            q = y.trie_r[ux(q)];
            if q == 0 {
                break;
            }
        }
        Ok(())
    }

    /// §957: pack subtries of a family.
    fn trie_pack(&mut self, mut p: i32) -> Result<(), Jump> {
        loop {
            let q = self.hyph.trie_l[ux(p)];
            if q > 0 && self.hyph.trie_hash[ux(q)] == 0 {
                self.first_fit(q)?;
                self.trie_pack(q)?;
            }
            p = self.hyph.trie_r[ux(p)];
            if p == 0 {
                return Ok(());
            }
        }
    }

    /// §959: move `p` and its siblings into `trie`.
    fn trie_fix(&mut self, mut p: i32) {
        let z = self.hyph.trie_hash[ux(p)]; // `trie_ref`
        loop {
            let y = &mut self.hyph;
            let q = y.trie_l[ux(p)];
            let c = y.trie_c[ux(p)];
            let (i, v) = (ux(z + c), y.trie_hash[ux(q)]);
            y.pat().link[i] = v;
            y.pat().ch[ux(z + c)] = c;
            let (i, v) = (ux(z + c), y.trie_o[ux(p)]);
            y.pat().op[i] = v;
            if q > 0 {
                self.trie_fix(q);
            }
            p = self.hyph.trie_r[ux(p)];
            if p == 0 {
                break;
            }
        }
    }

    /// §960: initialize the hyphenation pattern data (`\patterns`).
    pub(crate) fn new_patterns(&mut self) -> Result<(), Jump> {
        self.patterns_read();
        if !self.hyph.trie_not_ready {
            self.print_err(b"Too late for ");
            self.print_esc(b"patterns");
            self.help(&[b"All patterns must be given before typesetting begins."]);
            self.error()?;
            self.scan_toks(false, false)?;
            self.def_ref.clear();
            return Ok(());
        }
        self.set_cur_lang();
        self.scan_left_brace()?; // a left brace must follow \patterns
        // §961: enter all of the patterns into a linked trie, until coming
        // to a right brace. (`hc[1..=k]` and `hyf[0..=k]`, from 1 on
        // with `k`)
        let mut k: i32 = 0;
        let mut hc: Vec<i32> = vec![0];
        let mut hyf: Vec<i32> = vec![0];
        let mut digit_sensed = false;
        loop {
            self.get_x_token()?;
            match self.cur_cmd {
                LETTER | OTHER_CHAR => {
                    // §962: append a new letter or a hyphen level.
                    if digit_sensed
                        || self.cur_chr < i32::from(b'0')
                        || self.cur_chr > i32::from(b'9')
                    {
                        if self.cur_chr == i32::from(b'.') {
                            self.cur_chr = 0; // edge-of-word delimiter
                        } else {
                            self.cur_chr = self.lc_code(self.cur_chr);
                            if self.cur_chr == 0 {
                                self.print_err(b"Nonletter");
                                self.help(&[b"(See Appendix H.)"]);
                                self.error()?;
                            }
                        }
                        if self.unicode && self.cur_chr > self.hyph.patterns.max_hyph_char {
                            // `XeTeX` §1016
                            self.hyph.pat().max_hyph_char = self.cur_chr;
                            self.patterns_wrote(Version::of(&(3u8, self.cur_chr)).0);
                        }
                        if k < self.max_hyphenatable_length() {
                            k += 1;
                            hc.push(self.cur_chr);
                            hyf.push(0);
                            digit_sensed = false;
                        }
                    } else if k < self.max_hyphenatable_length() {
                        hyf[ux(k)] = self.cur_chr - i32::from(b'0');
                        digit_sensed = true;
                    }
                }
                SPACER | RIGHT_BRACE => {
                    if k > 0 {
                        self.insert_pattern(&mut hc, &mut hyf)?;
                    }
                    if self.cur_cmd == RIGHT_BRACE {
                        if self.int_par(SAVING_HYPH_CODES_CODE) > 0 {
                            self.store_hyph_codes();
                        }
                        return Ok(());
                    }
                    k = 0;
                    hc.truncate(1);
                    hyf.clear();
                    hyf.push(0);
                    digit_sensed = false;
                }
                _ => {
                    self.print_err(b"Bad ");
                    self.print_esc(b"patterns");
                    self.help(&[b"(See Appendix H.)"]);
                    self.error()?;
                }
            }
        }
    }

    /// e-TeX: store the current `\lccode`s for the current language (as
    /// trie ops: `XeTeX`'s are 16 bits).
    fn store_hyph_codes(&mut self) {
        let codes = (0..256).map(|c| self.lc_code(c) & BIGGEST_CHAR).collect();
        let lang = u8::try_from(self.hyph.cur_lang).unwrap_or(0);
        let part = Version::of(&(1u8, lang, &codes)).0;
        self.hyph.pat().hyph_codes.insert(lang, codes);
        self.patterns_wrote(part);
    }

    /// e-TeX `set_lc_code`: the hyphenation code of `c` (the saved codes
    /// of the current language, once the patterns are packed, for
    /// characters up to 255).
    fn hyph_code(&self, c: i32) -> i32 {
        let Ok(c) = u8::try_from(c) else {
            return self.lc_code(c);
        };
        self.patterns_read();
        if self.hyph.trie_not_ready {
            return self.lc_code(i32::from(c));
        }
        self.hyph
            .patterns
            .hyph_code(self.hyph.cur_lang, c, |c| self.lc_code(i32::from(c)))
    }

    /// §963: insert a new pattern, `hc[1..=k]` with `hyf[0..=k]`, into
    /// the linked trie.
    fn insert_pattern(&mut self, hc: &mut [i32], hyf: &mut [i32]) -> Result<(), Jump> {
        let k = i32::try_from(hc.len() - 1).unwrap_or(0);
        // §965: compute the trie op code, `v`, and set `l:=0`.
        if hc[1] == 0 {
            hyf[0] = 0;
        }
        if hc[ux(k)] == 0 {
            hyf[ux(k)] = 0;
        }
        let mut l = k;
        let mut v = MIN_TRIE_OP;
        loop {
            if hyf[ux(l)] != 0 {
                v = self.new_trie_op(k - l, hyf[ux(l)], v)?;
            }
            if l > 0 {
                l -= 1;
            } else {
                break;
            }
        }
        let mut q = 0;
        hc[0] = self.hyph.cur_lang;
        while l <= k {
            // (`c` is an `ASCII_code`: `XeTeX`'s a UTF-16 unit)
            let c = hc[ux(l)] & BIGGEST_CHAR;
            l += 1;
            let mut p = self.hyph.trie_l[ux(q)];
            let mut first_child = true;
            while p > 0 && c > self.hyph.trie_c[ux(p)] {
                q = p;
                p = self.hyph.trie_r[ux(q)];
                first_child = false;
            }
            if p == 0 || c < self.hyph.trie_c[ux(p)] {
                // §964: insert a new trie node between `q` and `p`, and make
                // `p` point to it.
                if self.hyph.trie_ptr == self.params.trie_size {
                    return self.overflow(b"pattern memory", self.params.trie_size);
                }
                let y = &mut self.hyph;
                y.trie_ptr += 1;
                y.trie_r[ux(y.trie_ptr)] = p;
                p = y.trie_ptr;
                y.trie_l[ux(p)] = 0;
                if first_child {
                    y.trie_l[ux(q)] = p;
                } else {
                    y.trie_r[ux(q)] = p;
                }
                y.trie_c[ux(p)] = c;
                y.trie_o[ux(p)] = MIN_TRIE_OP;
            }
            q = p; // now node `q` represents p_1...p_{l-1}
        }
        // (the pattern entered, which decides the linked trie and the ops)
        let part = Version::of(&(0u8, self.hyph.cur_lang, &*hc, &*hyf)).0;
        if self.hyph.trie_o[ux(q)] != MIN_TRIE_OP {
            self.print_err(b"Duplicate pattern");
            self.help(&[b"(See Appendix H.)"]);
            self.error()?;
        }
        self.hyph.trie_o[ux(q)] = v;
        self.patterns_wrote(part);
        Ok(())
    }

    /// §966
    pub(crate) fn init_trie(&mut self) -> Result<(), Jump> {
        if self.unicode {
            self.hyph.pat().max_hyph_char += 1; // `XeTeX` §1020
        }
        let mhc = self.hyph.patterns.max_hyph_char;
        // §952: get ready to compress the trie.
        // §945: sort the hyphenation op tables into proper order.
        let y = &mut self.hyph;
        y.pat().op_start[0] = -MIN_TRIE_OP;
        for j in 1..256 {
            let (i, v) = (j, y.patterns.op_start[j - 1] + y.trie_used[j - 1]);
            y.pat().op_start[i] = v;
        }
        for j in 1..=y.trie_op_ptr {
            let ju = ux(j);
            let dest = y.patterns.op_start[ux(y.trie_op_lang[ju])] + y.trie_op_val[ju];
            *y.op_hash(j) = dest; // destination
        }
        for j in 1..=y.trie_op_ptr {
            let ju = ux(j);
            while *y.op_hash(j) > j {
                let k = *y.op_hash(j);
                let ku = ux(k);
                y.pat().distance.swap(ku, ju);
                y.pat().num.swap(ku, ju);
                y.pat().next.swap(ku, ju);
                let hk = *y.op_hash(k);
                *y.op_hash(j) = hk;
                *y.op_hash(k) = k;
            }
        }
        let trie_size = ux(self.params.trie_size);
        for p in 0..=trie_size {
            self.hyph.trie_hash[p] = 0;
        }
        let root = self.compress_trie(self.hyph.trie_l[0]); // identify equivalent subtries
        self.hyph.trie_l[0] = root;
        for p in 0..=ux(self.hyph.trie_ptr) {
            self.hyph.trie_hash[p] = 0; // `trie_ref`
        }
        let chars = if self.unicode { BIGGEST_CHAR + 1 } else { 256 };
        self.hyph.trie_min = (1..=chars).collect();
        self.hyph.pat().link[0] = 1;
        self.hyph.trie_max = 0;
        let root = self.hyph.trie_l[0];
        if root != 0 {
            self.first_fit(root)?;
            self.trie_pack(root)?;
        }
        // §958: move the data into `trie`.
        if root == 0 {
            // no patterns were given
            for r in 0..=mhc {
                self.clear_trie(r);
            }
            self.hyph.trie_max = mhc;
        } else {
            self.trie_fix(root); // this fixes the non-holes in `trie`
            let mut r = 0; // now we will zero out all the holes
            loop {
                let s = self.hyph.patterns.link[ux(r)];
                self.clear_trie(r);
                r = s;
                if r > self.hyph.trie_max {
                    break;
                }
            }
        }
        self.hyph.pat().ch[0] = i32::from(b'?'); // make `trie_char(c)<>c` for all `c`
        self.hyph.trie_not_ready = false;
        // (the linked trie and the op hash are not needed any more)
        let y = &mut self.hyph;
        for v in [
            &mut y.trie_c,
            &mut y.trie_o,
            &mut y.trie_l,
            &mut y.trie_r,
            &mut y.trie_hash,
        ] {
            *v = Vec::new();
        }
        y.trie_taken = Vec::new();
        y.trie_min = Vec::new();
        y.trie_op_hash = Vec::new();
        y.trie_op_lang = Vec::new();
        y.trie_op_val = Vec::new();
        // (packed: fixed from now on)
        self.patterns_wrote(Version::of(&2u8).0);
        Ok(())
    }

    /// §958: `clear_trie`.
    fn clear_trie(&mut self, r: i32) {
        let ru = ux(r);
        self.hyph.pat().link[ru] = 0;
        self.hyph.pat().op[ru] = MIN_TRIE_OP;
        self.hyph.pat().ch[ru] = MIN_QUARTERWORD;
    }
}
