//! pdfnames.c, pdfnames.h: name "trees" (hash tables of named objects)
//! and the PDF name trees made from them.
//!
//! The functions only touch objects, so they are `PdfOut` methods taking
//! the table (`self.o.pdf_names_add_object(&mut self.doc…names, …)`
//! borrows two disjoint fields). The tables live in their owners
//! (pdfdoc's `global_names`, `names[i].data`). Keys are bytes (C's
//! `key, keylen`).

use crate::dpxutil::HtTable;
use crate::obj::{PDF_ARRAY, PDF_DICT, PDF_OBJ_INVALID, PDF_STREAM, PDF_STRING};
use crate::prelude::*;

/// `MAX_KEY` (printable_key).
pub const MAX_KEY: usize = 32;
/// `NAME_CLUSTER`.
pub const NAME_CLUSTER: i32 = 4;

/// `struct obj_data`: a hash table value.
#[derive(Clone, Debug, Default)]
pub struct ObjData {
    pub reference: Option<Obj>,
    pub object: Option<Obj>,
    /// 1 if the object is closed.
    pub closed: i32,
}

/// `struct ht_table *` of `obj_data` (C's "name tree").
pub type NameTree = HtTable<ObjData>;

/// `struct named_object`.
#[derive(Clone, Debug, Default)]
pub struct NamedObject {
    pub key: Vec<u8>,
    pub value: Option<Obj>,
}

/// `printable_key`: at most `MAX_KEY` bytes, non-printables as `#XX`
/// (C's static buffer: an owned copy).
#[must_use]
pub fn printable_key(key: &[u8]) -> Vec<u8> {
    let mut pkey = Vec::new();
    for &c in key {
        if pkey.len() >= MAX_KEY {
            break;
        }
        if (0x20..0x7f).contains(&c) {
            pkey.push(c);
        } else {
            let (hi, lo) = (c >> 4, c & 0xff);
            pkey.push(b'#');
            pkey.push(if hi < 10 { hi + b'0' } else { hi - 10 + b'A' });
            pkey.push(if lo < 10 {
                lo + b'0'
            } else {
                lo.wrapping_sub(10).wrapping_add(b'A')
            });
        }
    }
    pkey
}

/// `pdf_new_name_tree`.
#[must_use]
pub fn pdf_new_name_tree() -> NameTree {
    HtTable::ht_init_table()
}

/// `cmp_key`: the order of `named_object`s (qsort comparator).
#[must_use]
pub fn cmp_key(d1: &NamedObject, d2: &NamedObject) -> core::cmp::Ordering {
    let n = d1.key.len().min(d2.key.len());
    match d1.key[..n].cmp(&d2.key[..n]) {
        core::cmp::Ordering::Equal => d1.key.len().cmp(&d2.key.len()),
        x => x,
    }
}

impl PdfOut {
    /// `hval_free`: releases an entry's objects.
    fn hval_free(&mut self, value: ObjData) {
        self.release_opt(value.reference);
        self.release_opt(value.object);
    }
    /// `check_objects_defined`.
    fn check_objects_defined(&mut self, names: &mut NameTree) {
        let keys: Vec<Vec<u8>> = names
            .iter()
            .filter(|(_, v)| self.is_undefined(v.object))
            .map(|(k, _)| k.to_vec())
            .collect();
        for k in keys {
            let n = self.new_null();
            self.pdf_names_add_object(names, &k, n);
        }
    }
    /// `pdf_delete_name_tree`: checks, then releases every entry.
    pub fn pdf_delete_name_tree(&mut self, mut names: NameTree) {
        self.check_objects_defined(&mut names);
        for v in names.ht_clear_table() {
            self.hval_free(v);
        }
    }
    /// `pdf_names_add_object`: 0, or -1 (`object` is then released).
    pub fn pdf_names_add_object(&mut self, names: &mut NameTree, key: &[u8], object: Obj) -> i32 {
        if key.is_empty() {
            return -1;
        }
        match names.ht_lookup_table_mut(key) {
            None => {
                names.ht_append_table(
                    key,
                    ObjData {
                        object: Some(object),
                        reference: None,
                        closed: 0,
                    },
                );
                0
            }
            Some(value) => {
                if let Some(old) = value.object
                    && self.is_undefined(Some(old))
                {
                    self.transfer_label(object, old);
                    self.release(old);
                    value.object = Some(object);
                    0
                } else {
                    self.release(object);
                    -1
                }
            }
        }
    }
    /// `pdf_names_reserve`.
    pub fn pdf_names_reserve(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        if key.is_empty() {
            return None;
        }
        match names.ht_lookup_table_mut(key) {
            None => {
                let u = self.new_undefined();
                names.ht_append_table(
                    key,
                    ObjData {
                        object: Some(u),
                        reference: None,
                        closed: 0,
                    },
                );
                Some(self.ref_obj(u))
            }
            Some(value) => {
                if let Some(obj) = value.object
                    && self.is_undefined(Some(obj))
                {
                    let r = match value.reference {
                        Some(r) => r,
                        None => {
                            let r = self.ref_obj(obj);
                            value.reference = Some(r);
                            r
                        }
                    };
                    Some(self.link(r))
                } else {
                    None
                }
            }
        }
    }
    /// `pdf_names_lookup_reference`: a new link.
    pub fn pdf_names_lookup_reference(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        match names.ht_lookup_table_mut(key) {
            Some(value) => {
                if value.reference.is_none()
                    && let Some(obj) = value.object
                {
                    value.reference = Some(self.ref_obj(obj));
                }
                // (C links a NULL reference to NULL.)
                value.reference.map(|r| self.link(r))
            }
            None => self.pdf_names_reserve(names, key),
        }
    }
    /// `pdf_names_lookup_object`: the object itself (not linked), none
    /// if absent or undefined.
    pub fn pdf_names_lookup_object(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        let value = names.ht_lookup_table(key)?;
        if self.is_undefined(value.object) {
            return None;
        }
        value.object
    }
    /// `pdf_names_close_object`.
    pub fn pdf_names_close_object(&mut self, names: &mut NameTree, key: &[u8]) -> i32 {
        let Some(value) = names.ht_lookup_table_mut(key) else {
            return -1;
        };
        if self.is_undefined(value.object) {
            return -1;
        }
        if value.closed != 0 {
            return -1;
        }
        if value.reference.is_some() {
            let o = value.object.take();
            self.release_opt(o);
        }
        value.closed = 1;
        0
    }
    /// `build_name_tree` over `first` (the leaves, sorted).
    fn build_name_tree(&mut self, first: &mut [NamedObject], is_root: i32) -> Result<Obj> {
        let result = self.new_dict();
        let num_leaves = first.len();
        if is_root == 0 {
            let limits = self.new_array();
            let a = self.new_string(&first[0].key);
            self.add_array(limits, a);
            let b = self.new_string(&first[num_leaves - 1].key);
            self.add_array(limits, b);
            self.put(result, b"Limits", limits);
        }
        if num_leaves > 0 && num_leaves <= 2 * NAME_CLUSTER as usize {
            let names = self.new_array();
            for cur in first.iter_mut() {
                let k = self.new_string(&cur.key);
                self.add_array(names, k);
                let v = cur.value.take();
                match self.type_of(v) {
                    PDF_ARRAY | PDF_DICT | PDF_STREAM | PDF_STRING => {
                        let r = self.ref_obj(v.expect("value"));
                        self.add_array(names, r);
                    }
                    PDF_OBJ_INVALID => {
                        crate::fatal!("Invalid object...: {:?}", printable_key(&cur.key))
                    }
                    _ => {
                        let l = self.link(v.expect("value"));
                        self.add_array(names, l);
                    }
                }
                self.release_opt(v);
            }
            self.put(result, b"Names", names);
        } else if num_leaves > 0 {
            let kids = self.new_array();
            for i in 0..NAME_CLUSTER as usize {
                let start = (i * num_leaves) / NAME_CLUSTER as usize;
                let end = ((i + 1) * num_leaves) / NAME_CLUSTER as usize;
                let subtree = self.build_name_tree(&mut first[start..end], 0)?;
                let r = self.ref_obj(subtree);
                self.add_array(kids, r);
                self.release(subtree);
            }
            self.put(result, b"Kids", kids);
        }
        Ok(result)
    }
    /// `flat_table`: the entries (values linked), keys replaced through
    /// `filter` (a table of string objects) when given.
    fn flat_table(
        &mut self,
        names: &NameTree,
        filter: Option<&HtTable<Obj>>,
    ) -> Result<Vec<NamedObject>> {
        let mut objects = Vec::new();
        for (key, value) in names.iter() {
            let key = if let Some(f) = filter {
                let Some(&new_obj) = f.ht_lookup_table(key) else {
                    continue;
                };
                self.string_value(new_obj).to_vec()
            } else {
                key.to_vec()
            };
            let Some(obj) = value.object else {
                crate::fatal!("flat_table: released object in a name tree");
            };
            let v = if self.is_undefined(Some(obj)) {
                self.new_null()
            } else {
                self.link(obj)
            };
            objects.push(NamedObject {
                key,
                value: Some(v),
            });
        }
        Ok(objects)
    }
    /// `pdf_names_create_tree`: the tree and the entry count (`*count`).
    pub fn pdf_names_create_tree(
        &mut self,
        names: &mut NameTree,
        filter: Option<&HtTable<Obj>>,
    ) -> Result<(Option<Obj>, i32)> {
        let mut flat = self.flat_table(names, filter)?;
        let count = flat.len() as i32;
        if flat.is_empty() {
            return Ok((None, count));
        }
        flat.sort_by(cmp_key);
        let t = self.build_name_tree(&mut flat, 1)?;
        Ok((Some(t), count))
    }
}
