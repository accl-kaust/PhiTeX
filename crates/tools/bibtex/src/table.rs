//! BibTeX's hash table (§64–79): every name the program knows, keyed by
//! its text and its *ilk* (what kind of name it is), with one integer of
//! ilk-specific information and a function class per location.
//!
//! bibtex.web keeps the texts in a string pool; here each distinct text is
//! interned once and shared by all ilks, which is exactly when bibtex.web
//! makes a new pool string (§71), so the string counts in the `.blg`
//! statistics come out the same.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// A string value (shared across threads: a build's session moves with
/// its recorder to a worker's thread).
pub type Str = Arc<[u8]>;

/// A location in the table (0 is never one).
pub type Loc = u32;

pub const TEXT_ILK: u8 = 0;
pub const INTEGER_ILK: u8 = 1;
pub const AUX_COMMAND_ILK: u8 = 2;
pub const AUX_FILE_ILK: u8 = 3;
pub const BST_COMMAND_ILK: u8 = 4;
pub const BST_FILE_ILK: u8 = 5;
pub const BIB_FILE_ILK: u8 = 6;
pub const FILE_EXT_ILK: u8 = 7;
pub const FILE_AREA_ILK: u8 = 8;
pub const CITE_ILK: u8 = 9;
pub const LC_CITE_ILK: u8 = 10;
pub const BST_FN_ILK: u8 = 11;
pub const BIB_COMMAND_ILK: u8 = 12;
pub const MACRO_ILK: u8 = 13;
pub const CONTROL_SEQ_ILK: u8 = 14;

#[derive(Default)]
pub struct Table {
    ids: BTreeMap<Str, u32>,
    texts: Vec<Str>,
    chars: usize,
    locs: BTreeMap<(u8, u32), Loc>,
    loc_text: Vec<u32>,
    /// `ilk_info` (`fn_info` for functions).
    pub info: Vec<i64>,
    pub fn_type: Vec<u8>,
}

impl Table {
    pub fn new() -> Self {
        Self {
            loc_text: alloc::vec![0],
            info: alloc::vec![0],
            fn_type: alloc::vec![0],
            ..Self::default()
        }
    }

    /// `str_lookup(…, dont_insert)`: the location of `key` as an `ilk`.
    pub fn find(&self, ilk: u8, key: &[u8]) -> Option<Loc> {
        let id = *self.ids.get(key)?;
        self.locs.get(&(ilk, id)).copied()
    }

    /// `str_lookup(…, do_insert)`: the location, and whether it was
    /// already there (`hash_found`).
    pub fn insert(&mut self, ilk: u8, key: &[u8]) -> (Loc, bool) {
        let id = if let Some(&id) = self.ids.get(key) {
            id
        } else {
            let id = u32::try_from(self.texts.len()).unwrap_or(u32::MAX);
            let s: Str = key.into();
            self.chars += s.len();
            self.texts.push(s.clone());
            self.ids.insert(s, id);
            id
        };
        if let Some(&loc) = self.locs.get(&(ilk, id)) {
            return (loc, true);
        }
        let loc = Loc::try_from(self.loc_text.len()).unwrap_or(Loc::MAX);
        self.loc_text.push(id);
        self.info.push(0);
        self.fn_type.push(0);
        self.locs.insert((ilk, id), loc);
        (loc, false)
    }

    /// `hash_text[loc]`.
    pub fn text(&self, loc: Loc) -> &Str {
        &self.texts[self.loc_text[loc as usize] as usize]
    }

    /// The text numbered `id` (what `ilk_info` holds for a macro).
    pub fn text_id(&self, loc: Loc) -> i64 {
        i64::from(self.loc_text[loc as usize])
    }

    pub fn by_id(&self, id: i64) -> &Str {
        &self.texts[usize::try_from(id).unwrap_or(0)]
    }

    /// The pool statistics bibtex.web would print: strings (counting its
    /// unused string 0) and their characters.
    pub fn pool_stats(&self) -> (usize, usize) {
        (self.texts.len() + 1, self.chars)
    }

    pub fn info(&self, loc: Loc) -> i64 {
        self.info[loc as usize]
    }

    pub fn set_info(&mut self, loc: Loc, v: i64) {
        self.info[loc as usize] = v;
    }

    pub fn fn_type(&self, loc: Loc) -> u8 {
        self.fn_type[loc as usize]
    }

    pub fn set_fn_type(&mut self, loc: Loc, t: u8) {
        self.fn_type[loc as usize] = t;
    }
}
