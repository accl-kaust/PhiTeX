//! pdfnames.c, pdfnames.h: name "trees" (hash tables of named objects)
//! and the PDF name trees made from them.
//!
//! The functions only touch objects, so they are `PdfOut` methods taking
//! the table (`self.o.pdf_names_add_object(&mut self.doc…names, …)`
//! borrows two disjoint fields). The tables live in their owners
//! (pdfdoc's `global_names`, `names[i].data`). Keys are bytes (C's
//! `key, keylen`).

use crate::dpxutil::HtTable;
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
    todo!()
}

/// `pdf_new_name_tree`.
#[must_use]
pub fn pdf_new_name_tree() -> NameTree {
    todo!()
}

/// `cmp_key`: the order of `named_object`s (qsort comparator).
#[must_use]
pub fn cmp_key(d1: &NamedObject, d2: &NamedObject) -> core::cmp::Ordering {
    todo!()
}

impl PdfOut {
    /// `hval_free`: releases an entry's objects.
    fn hval_free(&mut self, value: ObjData) {
        todo!()
    }
    /// `check_objects_defined`.
    fn check_objects_defined(&mut self, names: &mut NameTree) {
        todo!()
    }
    /// `pdf_delete_name_tree`: checks, then releases every entry.
    pub fn pdf_delete_name_tree(&mut self, names: NameTree) {
        todo!()
    }
    /// `pdf_names_add_object`: 0, or -1 (`object` is then released).
    pub fn pdf_names_add_object(&mut self, names: &mut NameTree, key: &[u8], object: Obj) -> i32 {
        todo!()
    }
    /// `pdf_names_reserve`.
    pub fn pdf_names_reserve(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `pdf_names_lookup_reference`: a new link.
    pub fn pdf_names_lookup_reference(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `pdf_names_lookup_object`: the object itself (not linked), none
    /// if absent or undefined.
    pub fn pdf_names_lookup_object(&mut self, names: &mut NameTree, key: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `pdf_names_close_object`.
    pub fn pdf_names_close_object(&mut self, names: &mut NameTree, key: &[u8]) -> i32 {
        todo!()
    }
    /// `build_name_tree` over `first` (the leaves, sorted).
    fn build_name_tree(&mut self, first: &[NamedObject], is_root: i32) -> Obj {
        todo!()
    }
    /// `flat_table`: the entries (values linked), keys replaced through
    /// `filter` (a table of string objects) when given.
    fn flat_table(&mut self, names: &NameTree, filter: Option<&HtTable<Obj>>) -> Vec<NamedObject> {
        todo!()
    }
    /// `pdf_names_create_tree`: the tree and the entry count (`*count`).
    pub fn pdf_names_create_tree(
        &mut self,
        names: &mut NameTree,
        filter: Option<&HtTable<Obj>>,
    ) -> (Option<Obj>, i32) {
        todo!()
    }
}
