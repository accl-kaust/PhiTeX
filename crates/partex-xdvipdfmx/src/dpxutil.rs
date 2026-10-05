//! dpxutil.c, dpxutil.h: the stack, the hash table, small parsers.
//!
//! `ht_table` keeps C's bucket order (the hash, chains appended at the
//! end), so iterating it gives what C gives. Values are owned: the C
//! `hval_free_fn` is `Drop`, or, for values that need releasing (objects),
//! the removing calls hand the values back to the caller.

use crate::prelude::*;

/// `HASH_TABLE_SIZE`.
pub const HASH_TABLE_SIZE: usize = 503;

/// `INVALID_EPOCH_VALUE` (`(time_t)-1`).
pub const INVALID_EPOCH_VALUE: i64 = -1;

/// `struct ht_entry`.
#[derive(Clone, Debug)]
pub struct HtEntry<V> {
    pub key: Vec<u8>,
    pub value: V,
}

/// `struct ht_table`: `table[hash]` is the chain, in C's order.
#[derive(Clone, Debug)]
pub struct HtTable<V> {
    pub count: i32,
    pub table: Vec<Vec<HtEntry<V>>>,
}

impl<V> Default for HtTable<V> {
    fn default() -> Self {
        Self::ht_init_table()
    }
}

/// `struct ht_iter`: a position (`index` bucket, `pos` in its chain).
#[derive(Clone, Copy, Debug, Default)]
pub struct HtIter {
    pub index: usize,
    pub pos: usize,
}

/// `get_hash`: C's `char` is signed.
#[must_use]
pub fn get_hash(key: &[u8]) -> usize {
    let mut hkey: u32 = 0;
    for &c in key {
        hkey = (hkey << 5)
            .wrapping_add(hkey)
            .wrapping_add(c as i8 as i32 as u32);
    }
    (hkey % HASH_TABLE_SIZE as u32) as usize
}

impl<V> HtTable<V> {
    /// `ht_init_table`.
    #[must_use]
    pub fn ht_init_table() -> Self {
        let mut table = Vec::with_capacity(HASH_TABLE_SIZE);
        table.resize_with(HASH_TABLE_SIZE, Vec::new);
        HtTable { count: 0, table }
    }
    /// `ht_clear_table`: the values, in table order, for the caller to
    /// release (C's `hval_free_fn`).
    pub fn ht_clear_table(&mut self) -> Vec<V> {
        let mut out = Vec::new();
        for chain in &mut self.table {
            out.extend(chain.drain(..).map(|e| e.value));
        }
        self.count = 0;
        out
    }
    /// `ht_table_size`.
    #[must_use]
    pub fn ht_table_size(&self) -> i32 {
        self.count
    }
    /// `ht_lookup_table`.
    #[must_use]
    pub fn ht_lookup_table(&self, key: &[u8]) -> Option<&V> {
        self.table[get_hash(key)]
            .iter()
            .find(|e| e.key == key)
            .map(|e| &e.value)
    }
    /// `ht_lookup_table`, to change the value in place.
    pub fn ht_lookup_table_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        self.table[get_hash(key)]
            .iter_mut()
            .find(|e| e.key == key)
            .map(|e| &mut e.value)
    }
    /// `ht_append_table`: no check for an existing key.
    pub fn ht_append_table(&mut self, key: &[u8], value: V) {
        self.table[get_hash(key)].push(HtEntry {
            key: key.to_vec(),
            value,
        });
        self.count += 1;
    }
    /// `ht_remove_table`: the value removed (C returns 1), or none (0).
    pub fn ht_remove_table(&mut self, key: &[u8]) -> Option<V> {
        let chain = &mut self.table[get_hash(key)];
        let i = chain.iter().position(|e| e.key == key)?;
        self.count -= 1;
        Some(chain.remove(i).value)
    }
    /// `ht_insert_table`: replaces the value of an existing key (the old
    /// one is returned for the caller to release), else appends.
    pub fn ht_insert_table(&mut self, key: &[u8], value: V) -> Option<V> {
        let chain = &mut self.table[get_hash(key)];
        if let Some(e) = chain.iter_mut().find(|e| e.key == key) {
            return Some(core::mem::replace(&mut e.value, value));
        }
        chain.push(HtEntry {
            key: key.to_vec(),
            value,
        });
        self.count += 1;
        None
    }
    /// `ht_set_iter`: none for an empty table (C's -1).
    #[must_use]
    pub fn ht_set_iter(&self) -> Option<HtIter> {
        (0..HASH_TABLE_SIZE)
            .find(|&i| !self.table[i].is_empty())
            .map(|index| HtIter { index, pos: 0 })
    }
    /// `ht_iter_getkey`.
    #[must_use]
    pub fn ht_iter_getkey(&self, it: &HtIter) -> &[u8] {
        &self.table[it.index][it.pos].key
    }
    /// `ht_iter_getval`.
    #[must_use]
    pub fn ht_iter_getval(&self, it: &HtIter) -> &V {
        &self.table[it.index][it.pos].value
    }
    /// `ht_iter_getval`, mutable.
    pub fn ht_iter_getval_mut(&mut self, it: &HtIter) -> &mut V {
        &mut self.table[it.index][it.pos].value
    }
    /// `ht_iter_next`: 0, or -1 past the last entry.
    pub fn ht_iter_next(&self, it: &mut HtIter) -> i32 {
        it.pos += 1;
        while it.pos >= self.table[it.index].len() {
            it.index += 1;
            it.pos = 0;
            if it.index >= HASH_TABLE_SIZE {
                return -1;
            }
        }
        0
    }
    /// All entries in C's iteration order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &V)> {
        self.table
            .iter()
            .flatten()
            .map(|e| (e.key.as_slice(), &e.value))
    }
}

/// `dpx_stack`: the top is the end of `items`.
#[derive(Clone, Debug)]
pub struct DpxStack<T> {
    pub items: Vec<T>,
}

impl<T> Default for DpxStack<T> {
    fn default() -> Self {
        DpxStack { items: Vec::new() }
    }
}

impl<T> DpxStack<T> {
    /// `dpx_stack_init`.
    #[must_use]
    pub fn dpx_stack_init() -> Self {
        Self::default()
    }
    /// `dpx_stack_push`.
    pub fn dpx_stack_push(&mut self, data: T) {
        self.items.push(data);
    }
    /// `dpx_stack_pop`.
    pub fn dpx_stack_pop(&mut self) -> Option<T> {
        self.items.pop()
    }
    /// `dpx_stack_depth`.
    #[must_use]
    pub fn dpx_stack_depth(&self) -> i32 {
        self.items.len() as i32
    }
    /// `dpx_stack_top`.
    #[must_use]
    pub fn dpx_stack_top(&self) -> Option<&T> {
        self.items.last()
    }
    /// `dpx_stack_top`, mutable.
    pub fn dpx_stack_top_mut(&mut self) -> Option<&mut T> {
        self.items.last_mut()
    }
    /// `dpx_stack_at`: `pos` counted from the top (0).
    #[must_use]
    pub fn dpx_stack_at(&self, pos: i32) -> Option<&T> {
        let n = self.items.len();
        if pos < 0 || pos as usize >= n {
            return None;
        }
        self.items.get(n - 1 - pos as usize)
    }
    /// `dpx_stack_at`, mutable.
    pub fn dpx_stack_at_mut(&mut self, pos: i32) -> Option<&mut T> {
        let n = self.items.len();
        if pos < 0 || pos as usize >= n {
            return None;
        }
        self.items.get_mut(n - 1 - pos as usize)
    }
    /// `dpx_stack_roll`: the top `n` rolled `j` times (the top goes down
    /// to depth `n - 1` each time).
    pub fn dpx_stack_roll(&mut self, n: i32, j: i32) {
        let size = self.items.len() as i32;
        if n > size || n == 1 || n <= 0 {
            return;
        }
        let mut j = j % n;
        if j < 0 {
            j += n;
        }
        for _ in 0..j {
            let top = self.items.pop().unwrap();
            let len = self.items.len() + 1;
            self.items.insert(len - n as usize, top);
        }
    }
}

/// `xtoi` (dpxutil.c's).
#[must_use]
pub fn xtoi(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => i32::from(c - b'0'),
        b'a'..=b'f' => i32::from(c) - i32::from(b'W'),
        b'A'..=b'F' => i32::from(c) - i32::from(b'7'),
        _ => -1,
    }
}

#[must_use]
pub fn min4(x1: f64, x2: f64, x3: f64, x4: f64) -> f64 {
    let mut v = x1;
    if x2 < v {
        v = x2;
    }
    if x3 < v {
        v = x3;
    }
    if x4 < v {
        v = x4;
    }
    v
}

#[must_use]
pub fn max4(x1: f64, x2: f64, x3: f64, x4: f64) -> f64 {
    let mut v = x1;
    if x2 > v {
        v = x2;
    }
    if x3 > v {
        v = x3;
    }
    if x4 > v {
        v = x4;
    }
    v
}

fn skip_white(s: &[u8], p: &mut usize) {
    while *p < s.len() && matches!(s[*p], b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0) {
        *p += 1;
    }
}

/// `is_space` (dpxutil.h).
#[must_use]
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0)
}

/// `is_delim` (dpxutil.h): with braces.
#[must_use]
pub fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%'
    )
}

/// `skip_white_spaces`.
pub fn skip_white_spaces(s: &[u8], p: &mut usize) {
    while *p < s.len() && is_space(s[*p]) {
        *p += 1;
    }
}

/// `dpx_util_read_length`: a length with a unit (`true` scales by
/// `1/mag`), in big points. Returns the status and the value.
pub fn dpx_util_read_length(mag: f64, s: &[u8], pp: &mut usize) -> (i32, f64) {
    let mut p = *pp;
    let Some(q) = parse_float_decimal(s, &mut p) else {
        *pp = p;
        return (-1, 0.0);
    };
    let v = crate::fmt::atof(&q);
    let mut u = 1.0f64;
    let mut error = 0;
    skip_white(s, &mut p);
    if let Some(q0) = parse_c_ident(s, &mut p) {
        let mut q: &[u8] = &q0;
        let owned;
        if q.len() >= 4 && &q[..4] == b"true" {
            u /= if mag != 0.0 { mag } else { 1.0 };
            q = &q[4..];
        }
        let mut have = Some(q);
        if q.is_empty() {
            skip_white(s, &mut p);
            owned = parse_c_ident(s, &mut p);
            have = owned.as_deref();
        }
        if let Some(q) = have {
            match q {
                b"pt" => u *= 72.0 / 72.27,
                b"in" => u *= 72.0,
                b"cm" => u *= 72.0 / 2.54,
                b"mm" => u *= 72.0 / 25.4,
                b"bp" => u *= 1.0,
                b"pc" => u *= 12.0 * 72.0 / 72.27,
                b"dd" => u *= 1238.0 / 1157.0 * 72.0 / 72.27,
                b"cc" => u *= 12.0 * 1238.0 / 1157.0 * 72.0 / 72.27,
                b"sp" => u *= 72.0 / (72.27 * 65536.0),
                _ => error = -1,
            }
        } else {
            error = -1;
        }
    }
    *pp = p;
    (error, v * u)
}

/// `gmtime`: year, month, day, hour, minute, second.
#[must_use]
pub fn gmtime(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, secs / 3600, (secs % 3600) / 60, secs % 60)
}
fn read_c_escchar(s: &[u8], pp: &mut usize) -> (i32, u8) {
    let mut p = *pp;
    let mut l = 1;
    let mut c: i32 = 0;
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    match at(p) {
        b'a' => {
            c = 7;
            p += 1;
        }
        b'b' => {
            c = 8;
            p += 1;
        }
        b'f' => {
            c = 12;
            p += 1;
        }
        b'n' => {
            c = 10;
            p += 1;
        }
        b'r' => {
            c = 13;
            p += 1;
        }
        b't' => {
            c = 9;
            p += 1;
        }
        b'v' => {
            c = 11;
            p += 1;
        }
        x @ (b'\\' | b'?' | b'\'' | b'"') => {
            c = i32::from(x);
            p += 1;
        }
        b'\n' => {
            l = 0;
            p += 1;
        }
        b'\r' => {
            p += 1;
            if p < s.len() && s[p] == b'\n' {
                p += 1;
            }
            l = 0;
        }
        b'0'..=b'7' => {
            let mut i = 0;
            while i < 3 && p < s.len() && (b'0'..=b'7').contains(&s[p]) {
                c = (c << 3) + i32::from(s[p] - b'0');
                i += 1;
                p += 1;
            }
        }
        b'x' => {
            p += 1;
            let mut i = 0;
            while i < 2 && p < s.len() && s[p].is_ascii_hexdigit() {
                let ch = s[p];
                let d = if ch.is_ascii_digit() {
                    ch - b'0'
                } else if ch.is_ascii_lowercase() {
                    ch - b'a' + 10
                } else {
                    ch - b'A' + 10
                };
                c = (c << 4) + i32::from(d);
                i += 1;
                p += 1;
            }
        }
        _ => {
            l = 0;
            p += 1;
        }
    }
    *pp = p;
    (l, c as u8)
}

/// `read_c_litstrc` with no buffer: the length, or a negative status.
fn read_c_litstrc(s: &[u8], pp: &mut usize, out: Option<&mut Vec<u8>>) -> i32 {
    let mut p = *pp;
    let mut l = 0i32;
    let mut st = -1; // Q_CONT
    let mut out = out;
    while st == -1 && p < s.len() {
        match s[p] {
            b'"' => {
                st = 0;
                p += 1;
            }
            b'\\' => {
                p += 1;
                let (n, c) = read_c_escchar(s, &mut p);
                if n > 0
                    && let Some(o) = out.as_deref_mut()
                {
                    o.push(c);
                }
                l += n;
            }
            b'\n' | b'\r' => st = -2,
            c => {
                if let Some(o) = out.as_deref_mut() {
                    o.push(c);
                }
                l += 1;
                p += 1;
            }
        }
    }
    *pp = p;
    if st == 0 { l } else { st }
}

/// `parse_c_string`: a C string literal after its quote.
pub fn parse_c_string(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    let mut p = *pp;
    if p >= s.len() || s[p] != b'"' {
        return None;
    }
    p += 1;
    let mut q = None;
    let l = read_c_litstrc(s, &mut p, None);
    if l >= 0 {
        let mut v = Vec::new();
        p = *pp + 1;
        read_c_litstrc(s, &mut p, Some(&mut v));
        q = Some(v);
    }
    *pp = p;
    q
}

fn is_c_nondigit(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

/// `parse_c_ident`.
pub fn parse_c_ident(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    let mut p = *pp;
    if p >= s.len() || !is_c_nondigit(s[p]) {
        return None;
    }
    while p < s.len() && (is_c_nondigit(s[p]) || s[p].is_ascii_digit()) {
        p += 1;
    }
    let q = s[*pp..p].to_vec();
    *pp = p;
    Some(q)
}

/// `parse_float_decimal`.
pub fn parse_float_decimal(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    let mut p = *pp;
    if p >= s.len() {
        return None;
    }
    if s[p] == b'+' || s[p] == b'-' {
        p += 1;
    }
    let mut st = 0i32;
    let mut n = 0;
    while p < s.len() && st >= 0 {
        match s[p] {
            b'+' | b'-' => {
                if st != 2 {
                    st = -1;
                } else {
                    st = 3;
                    p += 1;
                }
            }
            b'.' => {
                if st > 0 {
                    st = -1;
                } else {
                    st = 1;
                    p += 1;
                }
            }
            b'0'..=b'9' => {
                n += 1;
                p += 1;
            }
            b'E' | b'e' => {
                if n == 0 || st == 2 {
                    st = -1;
                } else {
                    st = 2;
                    p += 1;
                }
            }
            _ => st = -1,
        }
    }
    let q = if n != 0 {
        Some(s[*pp..p].to_vec())
    } else {
        None
    };
    *pp = p;
    q
}

impl Dpx {
    /// `dpx_util_get_unique_time_if_given`: `SOURCE_DATE_EPOCH` as the
    /// host gave it (C reads the environment), or `INVALID_EPOCH_VALUE`.
    pub fn dpx_util_get_unique_time_if_given(&mut self) -> i64 {
        match self.session.source_date_epoch {
            Some(e) if e >= 0 => e,
            _ => INVALID_EPOCH_VALUE,
        }
    }

    /// `dpx_util_format_asn_date`: `D:YYYYmmddHHMMSS` and, if asked, the
    /// zone (`Z`, or `+HH'MM'`). Without `SOURCE_DATE_EPOCH` the time is
    /// the host's local time (`session.now`, `session.utc_offset_min`).
    pub fn dpx_util_format_asn_date(&mut self, need_timezone: bool) -> Vec<u8> {
        let given = self.dpx_util_get_unique_time_if_given();
        let (t, local) = if given == INVALID_EPOCH_VALUE {
            (
                self.session.now,
                i64::from(self.session.utc_offset_min) * 60,
            )
        } else {
            (given, 0)
        };
        let bd = gmtime(t + local);
        let gmt = gmtime(t);
        let mut b = crate::fmt::Buf::new();
        b.extend(b"D:");
        b.zero_padded(bd.0 as u64, 4);
        for v in [bd.1, bd.2, bd.3, bd.4, bd.5] {
            b.zero_padded(v as u64, 2);
        }
        // (%S may be 60 or 61 in C; never here.)
        let mut off = 60 * (bd.3 - gmt.3) + bd.4 - gmt.4;
        if bd.0 != gmt.0 {
            off += if bd.0 > gmt.0 { 1440 } else { -1440 };
        } else if (bd.1, bd.2) != (gmt.1, gmt.2) {
            off += if (bd.1, bd.2) > (gmt.1, gmt.2) {
                1440
            } else {
                -1440
            };
        }
        if need_timezone {
            if off == 0 {
                b.push(b'Z');
            } else {
                let h = off / 60;
                let m = (off - h * 60).abs();
                b.push(if h < 0 { b'-' } else { b'+' });
                b.zero_padded(h.unsigned_abs(), 2);
                b.push(b'\'');
                b.zero_padded(m as u64, 2);
                b.push(b'\'');
            }
        }
        b.0
    }
}
