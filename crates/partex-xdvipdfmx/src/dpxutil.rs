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

/// `min4`.
#[must_use]
pub fn min4(v1: f64, v2: f64, v3: f64, v4: f64) -> f64 {
    todo!()
}

/// `max4`.
#[must_use]
pub fn max4(v1: f64, v2: f64, v3: f64, v4: f64) -> f64 {
    todo!()
}

/// `skip_white_spaces`: advances `*p` over spaces (the end of `s` is
/// C's `endptr`, as for every `(pp, endptr)` parser).
pub fn skip_white_spaces(s: &[u8], p: &mut usize) {
    todo!()
}

/// `xtoi`: a hex digit's value, or -1.
#[must_use]
pub fn xtoi(c: u8) -> i32 {
    todo!()
}

/// `skip_white` (static).
fn skip_white(s: &[u8], pp: &mut usize) {
    todo!()
}

/// `read_c_escchar` (static): the bytes written to `r` (C's count; `r`
/// may be none to only count).
fn read_c_escchar(r: Option<&mut Vec<u8>>, s: &[u8], pp: &mut usize) -> i32 {
    todo!()
}

/// `read_c_litstrc` (static): the length read, or -1 on error; `q`
/// receives at most `len` bytes when given.
fn read_c_litstrc(q: Option<&mut Vec<u8>>, len: i32, s: &[u8], pp: &mut usize) -> i32 {
    todo!()
}

/// `parse_float_decimal`.
pub fn parse_float_decimal(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `parse_c_string`.
pub fn parse_c_string(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `parse_c_ident`.
pub fn parse_c_ident(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `dpx_util_read_length`: status (0 ok, -1 error) and the length in
/// big points (`mag` applied for `true` units).
pub fn dpx_util_read_length(mag: f64, s: &[u8], pp: &mut usize) -> (i32, f64) {
    todo!()
}

impl Dpx {
    /// `dpx_util_get_unique_time_if_given`: `SOURCE_DATE_EPOCH` (with
    /// `FORCE_SOURCE_DATE`), or `INVALID_EPOCH_VALUE`.
    pub fn dpx_util_get_unique_time_if_given(&mut self) -> i64 {
        todo!()
    }
    /// `dpx_util_format_asn_date`: the date string (`D:YYYYMMDDhhmmss…`).
    pub fn dpx_util_format_asn_date(&mut self, need_timezone: bool) -> Vec<u8> {
        todo!()
    }
}
