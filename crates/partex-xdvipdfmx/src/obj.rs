//! PDF objects and the writer (dvipdfm-x's `pdfobj.c`).
//!
//! Objects live in an arena and are named by [`Obj`] handles. As in
//! dvipdfm-x they are reference counted, and an object that carries a
//! label (an object number) is written to the output when its count
//! drops to zero: the order of the PDF's objects is the order of those
//! releases, so it is kept exactly. An indirect reference made by
//! [`PdfOut::ref_obj`] does not hold its target, as in C.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::fmt::{Buf, sprint_number};

/// A handle on an object of the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Obj(u32);

/// `PDF_BOOLEAN` … `PDF_UNDEFINED`, as `pdf_obj_typeof` returns them.
pub const PDF_BOOLEAN: i32 = 1;
pub const PDF_NUMBER: i32 = 2;
pub const PDF_STRING: i32 = 3;
pub const PDF_NAME: i32 = 4;
pub const PDF_ARRAY: i32 = 5;
pub const PDF_DICT: i32 = 6;
pub const PDF_STREAM: i32 = 7;
pub const PDF_NULL: i32 = 8;
pub const PDF_INDIRECT: i32 = 9;
pub const PDF_UNDEFINED: i32 = 10;
pub const PDF_OBJ_INVALID: i32 = 0;

pub const STREAM_COMPRESS: i32 = 1 << 0;
pub const STREAM_USE_PREDICTOR: i32 = 1 << 1;
pub const PDF_OBJ_MAX_DEPTH: i32 = 30;

/// Objects with this flag are never put into an object stream.
pub const OBJ_NO_OBJSTM: i32 = 1 << 0;
/// Objects with this flag are never encrypted.
pub const OBJ_NO_ENCRYPT: i32 = 1 << 1;

const OBJSTM_MAX_OBJS: i32 = 200;
const PDF_NUM_INDIRECT_MAX: u32 = 8_388_607;
const BINARY_MARKER: &[u8] = b"%\xe4\xf0\xed\xf8\n";

/// `DecodeParms` of a stream with a predictor.
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodeParms {
    pub predictor: i32,
    pub colors: i32,
    pub bits_per_component: i32,
    pub columns: i32,
}

#[derive(Clone, Debug)]
pub struct Stream {
    pub dict: Obj,
    pub data: Vec<u8>,
    /// The offset table of an object stream (`objstm_data`), or of an
    /// object stream read from a file.
    pub objstm: Option<Vec<i32>>,
    pub flags: i32,
    pub parms: DecodeParms,
}

#[derive(Clone, Copy, Debug)]
pub struct Indirect {
    /// The file it refers into (`pdf_file *`), or none for this output.
    pub pf: Option<u32>,
    /// The object of this output it refers to (not counted).
    pub obj: Option<Obj>,
    pub label: u32,
    pub generation: u16,
}

#[derive(Clone, Debug)]
pub enum Data {
    Boolean(bool),
    Number(f64),
    Str(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Option<Obj>>),
    /// Keys are name objects, in insertion order.
    Dict(Vec<(Obj, Option<Obj>)>),
    Stream(Box<Stream>),
    Null,
    Indirect(Indirect),
    Undefined,
    /// A released slot.
    Free,
}

#[derive(Clone, Debug)]
struct Slot {
    data: Data,
    label: u32,
    generation: u16,
    refcount: i32,
    flags: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct XrefEntry {
    pub kind: u8,
    pub field2: u32,
    pub field3: u16,
}

/// A deflater: zlib's `compress2` at the given level.
pub type Deflate = Box<dyn FnMut(i32, &[u8]) -> Vec<u8>>;

/// The arena and the writer (`struct pdf_out`).
#[derive(Clone)]
pub struct PdfOut {
    slots: Vec<Slot>,
    free_slots: Vec<u32>,

    pub version_major: i32,
    pub version_minor: i32,
    pub compression_level: i32,
    pub use_predictor: bool,
    pub enable_encrypt: bool,
    pub use_objstm: bool,

    /// What was written so far, and not yet taken.
    out: Vec<u8>,
    /// Bytes written, whether taken or not (`file_position`).
    pub file_position: usize,
    line_position: usize,
    /// While writing into an object stream.
    output_stream: Option<Obj>,
    open: bool,

    next_label: u32,
    trailer: Option<Obj>,
    startxref: u32,
    xref: Vec<XrefEntry>,
    xref_stream: Option<Obj>,
    current_objstm: Option<Obj>,
    free_list: Vec<u8>,

    /// Shared by snapshots (none: `Default`'s writer, never deflating).
    deflate: Option<Rc<RefCell<Deflate>>>,
    /// Files read for their objects (`pdf_file`), by number.
    pub files: Vec<crate::pdfread::PdfFile>,
    /// Where `parse_xref_table` found `trailer`.
    pub(crate) trailer_pos: usize,
}

impl PdfOut {
    #[must_use]
    pub fn new(deflate: Deflate) -> Self {
        PdfOut {
            slots: Vec::new(),
            free_slots: Vec::new(),
            version_major: 1,
            version_minor: 5,
            compression_level: 9,
            use_predictor: true,
            enable_encrypt: false,
            use_objstm: true,
            out: Vec::new(),
            file_position: 0,
            line_position: 0,
            output_stream: None,
            open: false,
            next_label: 1,
            trailer: None,
            startxref: 0,
            xref: Vec::new(),
            xref_stream: None,
            current_objstm: None,
            free_list: Vec::new(),
            deflate: Some(Rc::new(RefCell::new(deflate))),
            files: Vec::new(),
            trailer_pos: 0,
        }
    }

    // ---------------------------------------------------------------
    // The arena.

    fn alloc(&mut self, data: Data) -> Obj {
        let slot = Slot {
            data,
            label: 0,
            generation: 0,
            refcount: 1,
            flags: 0,
        };
        if let Some(i) = self.free_slots.pop() {
            self.slots[i as usize] = slot;
            Obj(i)
        } else {
            self.slots.push(slot);
            Obj(u32::try_from(self.slots.len() - 1).expect("too many objects"))
        }
    }

    fn slot(&self, o: Obj) -> &Slot {
        &self.slots[o.0 as usize]
    }

    fn slot_mut(&mut self, o: Obj) -> &mut Slot {
        &mut self.slots[o.0 as usize]
    }

    #[must_use]
    pub fn data(&self, o: Obj) -> &Data {
        &self.slot(o).data
    }

    pub fn data_mut(&mut self, o: Obj) -> &mut Data {
        &mut self.slot_mut(o).data
    }

    /// `pdf_obj_typeof`.
    #[must_use]
    pub fn type_of(&self, o: Option<Obj>) -> i32 {
        let Some(o) = o else { return PDF_OBJ_INVALID };
        match self.data(o) {
            Data::Boolean(_) => PDF_BOOLEAN,
            Data::Number(_) => PDF_NUMBER,
            Data::Str(_) => PDF_STRING,
            Data::Name(_) => PDF_NAME,
            Data::Array(_) => PDF_ARRAY,
            Data::Dict(_) => PDF_DICT,
            Data::Stream(_) => PDF_STREAM,
            Data::Null => PDF_NULL,
            Data::Indirect(_) => PDF_INDIRECT,
            Data::Undefined => PDF_UNDEFINED,
            Data::Free => PDF_OBJ_INVALID,
        }
    }

    #[must_use]
    pub fn is_number(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_NUMBER
    }
    #[must_use]
    pub fn is_boolean(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_BOOLEAN
    }
    #[must_use]
    pub fn is_string(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_STRING
    }
    #[must_use]
    pub fn is_name(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_NAME
    }
    #[must_use]
    pub fn is_array(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_ARRAY
    }
    #[must_use]
    pub fn is_dict(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_DICT
    }
    #[must_use]
    pub fn is_stream(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_STREAM
    }
    #[must_use]
    pub fn is_null(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_NULL
    }
    #[must_use]
    pub fn is_indirect(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_INDIRECT
    }
    #[must_use]
    pub fn is_undefined(&self, o: Option<Obj>) -> bool {
        self.type_of(o) == PDF_UNDEFINED
    }

    #[must_use]
    pub fn label(&self, o: Obj) -> u32 {
        self.slot(o).label
    }
    #[must_use]
    pub fn generation(&self, o: Obj) -> u16 {
        self.slot(o).generation
    }
    pub fn set_flags(&mut self, o: Obj, flags: i32) {
        self.slot_mut(o).flags |= flags;
    }

    // ---------------------------------------------------------------
    // Constructors and accessors.

    pub fn new_undefined(&mut self) -> Obj {
        self.alloc(Data::Undefined)
    }
    pub fn new_null(&mut self) -> Obj {
        self.alloc(Data::Null)
    }
    pub fn new_boolean(&mut self, v: bool) -> Obj {
        self.alloc(Data::Boolean(v))
    }
    pub fn new_number(&mut self, v: f64) -> Obj {
        self.alloc(Data::Number(v))
    }
    pub fn new_string(&mut self, s: &[u8]) -> Obj {
        self.alloc(Data::Str(s.to_vec()))
    }
    /// `pdf_new_name`: the name without its `/`. A C string: it ends at
    /// a NUL.
    pub fn new_name(&mut self, s: &[u8]) -> Obj {
        let s = s.split(|&c| c == 0).next().unwrap_or_default();
        self.alloc(Data::Name(s.to_vec()))
    }
    pub fn new_array(&mut self) -> Obj {
        self.alloc(Data::Array(Vec::new()))
    }
    pub fn new_dict(&mut self) -> Obj {
        self.alloc(Data::Dict(Vec::new()))
    }
    pub fn new_stream(&mut self, flags: i32) -> Obj {
        let dict = self.new_dict();
        let o = self.alloc(Data::Stream(Box::new(Stream {
            dict,
            data: Vec::new(),
            objstm: None,
            flags,
            parms: DecodeParms {
                predictor: 2,
                ..DecodeParms::default()
            },
        })));
        self.slot_mut(o).flags |= OBJ_NO_OBJSTM;
        o
    }
    pub fn new_indirect(&mut self, pf: Option<u32>, label: u32, generation: u16) -> Obj {
        self.alloc(Data::Indirect(Indirect {
            pf,
            obj: None,
            label,
            generation,
        }))
    }

    #[must_use]
    pub fn boolean_value(&self, o: Obj) -> bool {
        match self.data(o) {
            Data::Boolean(b) => *b,
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn number_value(&self, o: Obj) -> f64 {
        match self.data(o) {
            Data::Number(v) => *v,
            _ => typecheck(),
        }
    }
    pub fn set_number(&mut self, o: Obj, v: f64) {
        match self.data_mut(o) {
            Data::Number(x) => *x = v,
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn string_value(&self, o: Obj) -> &[u8] {
        match self.data(o) {
            Data::Str(s) => s,
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn string_length(&self, o: Obj) -> usize {
        self.string_value(o).len()
    }
    pub fn set_string(&mut self, o: Obj, v: &[u8]) {
        match self.data_mut(o) {
            Data::Str(s) => *s = v.to_vec(),
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn name_value(&self, o: Obj) -> &[u8] {
        match self.data(o) {
            Data::Name(s) => s,
            _ => typecheck(),
        }
    }

    /// `pdf_link_obj`.
    pub fn link(&mut self, o: Obj) -> Obj {
        assert!(
            !matches!(self.data(o), Data::Free),
            "pdf_link_obj(): invalid object"
        );
        self.slot_mut(o).refcount += 1;
        o
    }
    pub fn link_opt(&mut self, o: Option<Obj>) -> Option<Obj> {
        o.map(|o| self.link(o))
    }

    /// `pdf_transfer_label`.
    pub fn transfer_label(&mut self, dst: Obj, src: Obj) {
        let (label, generation) = (self.slot(src).label, self.slot(src).generation);
        assert!(self.slot(dst).label == 0);
        let d = self.slot_mut(dst);
        d.label = label;
        d.generation = generation;
        let s = self.slot_mut(src);
        s.label = 0;
        s.generation = 0;
    }

    fn label_obj(&mut self, o: Obj) {
        if self.slot(o).label == 0 {
            assert!(
                self.next_label != PDF_NUM_INDIRECT_MAX,
                "Number of indirect object has reached its maximum value!"
            );
            let l = self.next_label;
            self.next_label += 1;
            let s = self.slot_mut(o);
            s.label = l;
            s.generation = 0;
        }
    }

    /// `pdf_ref_obj`.
    pub fn ref_obj(&mut self, o: Obj) -> Obj {
        assert!(
            self.slot(o).refcount > 0,
            "Trying to refer already released object!!!"
        );
        if self.is_indirect(Some(o)) {
            self.link(o)
        } else {
            self.new_ref(o)
        }
    }

    fn new_ref(&mut self, o: Obj) -> Obj {
        if self.slot(o).label == 0 {
            self.label_obj(o);
        }
        let (label, generation) = (self.slot(o).label, self.slot(o).generation);
        let r = self.new_indirect(None, label, generation);
        if let Data::Indirect(i) = self.data_mut(r) {
            i.obj = Some(o);
        }
        r
    }

    /// The object an indirect object of this output points to.
    pub fn set_indirect_target(&mut self, r: Obj, target: Option<Obj>) {
        if let Data::Indirect(i) = self.data_mut(r) {
            i.obj = target;
        }
    }

    #[must_use]
    pub fn indirect(&self, o: Obj) -> Indirect {
        match self.data(o) {
            Data::Indirect(i) => *i,
            _ => typecheck(),
        }
    }

    /// `pdf_set_label` (used by imports): label and generation as given.
    pub fn set_label(&mut self, o: Obj, label: u32, generation: u16) {
        let s = self.slot_mut(o);
        s.label = label;
        s.generation = generation;
    }

    // Arrays.

    pub fn add_array(&mut self, a: Obj, o: Obj) {
        match self.data_mut(a) {
            Data::Array(v) => v.push(Some(o)),
            _ => typecheck(),
        }
    }
    /// `pdf_add_array` with a C null pointer allowed.
    pub fn add_array_opt(&mut self, a: Obj, o: Option<Obj>) {
        match self.data_mut(a) {
            Data::Array(v) => v.push(o),
            _ => typecheck(),
        }
    }
    pub fn unshift_array(&mut self, a: Obj, o: Obj) {
        match self.data_mut(a) {
            Data::Array(v) => v.insert(0, Some(o)),
            _ => typecheck(),
        }
    }
    /// `pdf_get_array`: a negative index counts from the end.
    #[must_use]
    pub fn get_array(&self, a: Obj, idx: i32) -> Option<Obj> {
        match self.data(a) {
            Data::Array(v) => {
                if idx < 0 {
                    let i = v.len().checked_add_signed(idx as isize)?;
                    v.get(i).copied().flatten()
                } else {
                    v.get(idx as usize).copied().flatten()
                }
            }
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn array_length(&self, a: Obj) -> usize {
        match self.data(a) {
            Data::Array(v) => v.len(),
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn array_items(&self, a: Obj) -> Vec<Option<Obj>> {
        match self.data(a) {
            Data::Array(v) => v.clone(),
            _ => typecheck(),
        }
    }
    /// Removes and returns the last element (`pdf_pop_array`'s use).
    pub fn pop_array(&mut self, a: Obj) -> Option<Obj> {
        match self.data_mut(a) {
            Data::Array(v) => v.pop().flatten(),
            _ => typecheck(),
        }
    }
    /// Replaces element `i` (the caller releases the old one).
    pub fn put_array_raw(&mut self, a: Obj, i: usize, o: Option<Obj>) -> Option<Obj> {
        match self.data_mut(a) {
            Data::Array(v) => core::mem::replace(&mut v[i], o),
            _ => typecheck(),
        }
    }

    // Dictionaries.

    /// `pdf_add_dict`: returns whether the key was there already.
    pub fn add_dict(&mut self, d: Obj, key: Obj, value: Option<Obj>) -> bool {
        let found = {
            let kname = self.name_value(key);
            let Data::Dict(entries) = self.data(d) else {
                typecheck()
            };
            entries
                .iter()
                .position(|&(k, _)| self.name_value(k) == kname)
        };
        if let Some(i) = found {
            let old = {
                let Data::Dict(entries) = self.data_mut(d) else {
                    unreachable!()
                };
                core::mem::replace(&mut entries[i].1, value)
            };
            self.release_opt(old);
            self.release(key);
            true
        } else {
            let Data::Dict(entries) = self.data_mut(d) else {
                unreachable!()
            };
            entries.push((key, value));
            false
        }
    }
    /// `pdf_add_dict(d, pdf_new_name(key), value)`.
    pub fn put(&mut self, d: Obj, key: &[u8], value: Obj) -> bool {
        let k = self.new_name(key);
        self.add_dict(d, k, Some(value))
    }
    pub fn put_opt(&mut self, d: Obj, key: &[u8], value: Option<Obj>) -> bool {
        let k = self.new_name(key);
        self.add_dict(d, k, value)
    }
    pub fn put_name(&mut self, d: Obj, key: &[u8], value: &[u8]) -> bool {
        let v = self.new_name(value);
        self.put(d, key, v)
    }
    pub fn put_number(&mut self, d: Obj, key: &[u8], value: f64) -> bool {
        let v = self.new_number(value);
        self.put(d, key, v)
    }
    pub fn put_string(&mut self, d: Obj, key: &[u8], value: &[u8]) -> bool {
        let v = self.new_string(value);
        self.put(d, key, v)
    }
    pub fn put_boolean(&mut self, d: Obj, key: &[u8], value: bool) -> bool {
        let v = self.new_boolean(value);
        self.put(d, key, v)
    }

    /// `pdf_lookup_dict`.
    #[must_use]
    pub fn lookup_dict(&self, d: Obj, name: &[u8]) -> Option<Obj> {
        match self.data(d) {
            Data::Dict(entries) => entries
                .iter()
                .find(|&&(k, _)| self.name_value(k) == name)
                .and_then(|&(_, v)| v),
            _ => typecheck(),
        }
    }
    /// The entries, in order (`pdf_foreach_dict`).
    #[must_use]
    pub fn dict_entries(&self, d: Obj) -> Vec<(Obj, Option<Obj>)> {
        match self.data(d) {
            Data::Dict(entries) => entries.clone(),
            _ => typecheck(),
        }
    }
    /// `pdf_dict_keys`: a new array of new names.
    pub fn dict_keys(&mut self, d: Obj) -> Obj {
        let names: Vec<Vec<u8>> = self
            .dict_entries(d)
            .iter()
            .map(|&(k, _)| self.name_value(k).to_vec())
            .collect();
        let keys = self.new_array();
        for n in names {
            let k = self.new_name(&n);
            self.add_array(keys, k);
        }
        keys
    }
    /// `pdf_remove_dict`.
    pub fn remove_dict(&mut self, d: Obj, name: &[u8]) {
        let found = {
            let Data::Dict(entries) = self.data(d) else {
                typecheck()
            };
            entries
                .iter()
                .position(|&(k, _)| self.name_value(k) == name)
        };
        if let Some(i) = found {
            let (k, v) = {
                let Data::Dict(entries) = self.data_mut(d) else {
                    unreachable!()
                };
                entries.remove(i)
            };
            self.release(k);
            self.release_opt(v);
        }
    }
    /// `pdf_merge_dict`.
    pub fn merge_dict(&mut self, d1: Obj, d2: Obj) {
        for (k, v) in self.dict_entries(d2) {
            let k = self.link(k);
            let v = self.link_opt(v);
            self.add_dict(d1, k, v);
        }
    }

    // Streams.

    fn stream(&self, s: Obj) -> &Stream {
        match self.data(s) {
            Data::Stream(st) => st,
            _ => typecheck(),
        }
    }
    pub fn stream_mut(&mut self, s: Obj) -> &mut Stream {
        match self.data_mut(s) {
            Data::Stream(st) => st,
            _ => typecheck(),
        }
    }
    #[must_use]
    pub fn stream_dict(&self, s: Obj) -> Obj {
        self.stream(s).dict
    }
    #[must_use]
    pub fn stream_data(&self, s: Obj) -> &[u8] {
        &self.stream(s).data
    }
    #[must_use]
    pub fn stream_length(&self, s: Obj) -> usize {
        self.stream(s).data.len()
    }
    pub fn add_stream(&mut self, s: Obj, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.stream_mut(s).data.extend_from_slice(bytes);
    }
    pub fn stream_set_predictor(
        &mut self,
        s: Obj,
        predictor: i32,
        columns: i32,
        bpc: i32,
        colors: i32,
    ) {
        if !self.is_stream(Some(s)) || columns < 0 || bpc < 0 || colors < 0 {
            return;
        }
        let st = self.stream_mut(s);
        st.parms = DecodeParms {
            predictor,
            colors,
            bits_per_component: bpc,
            columns,
        };
        st.flags |= STREAM_USE_PREDICTOR;
    }

    // ---------------------------------------------------------------
    // Release, and writing what is released.

    /// `pdf_release_obj`.
    pub fn release(&mut self, o: Obj) {
        assert!(
            self.slot(o).refcount > 0 && !matches!(self.data(o), Data::Free),
            "pdf_release_obj: Called with invalid object."
        );
        self.slot_mut(o).refcount -= 1;
        if self.slot(o).refcount != 0 {
            return;
        }
        let label = self.slot(o).label;
        if label != 0 {
            self.mark_freed(label);
            if self.open {
                let flags = self.slot(o).flags;
                if !self.use_objstm
                    || flags & OBJ_NO_OBJSTM != 0
                    || (self.enable_encrypt && flags & OBJ_NO_ENCRYPT != 0)
                    || self.slot(o).generation != 0
                {
                    self.flush_obj(o);
                } else {
                    let objstm = if let Some(s) = self.current_objstm {
                        s
                    } else {
                        let s = self.new_stream(STREAM_COMPRESS);
                        self.stream_mut(s).objstm = Some(vec![0; 2 * OBJSTM_MAX_OBJS as usize + 2]);
                        self.label_obj(s);
                        self.current_objstm = Some(s);
                        s
                    };
                    if self.add_objstm(objstm, o) == OBJSTM_MAX_OBJS {
                        self.release_objstm(objstm);
                        self.current_objstm = None;
                    }
                }
            }
        }
        let data = core::mem::replace(&mut self.slot_mut(o).data, Data::Free);
        match data {
            Data::Array(v) => {
                for x in v.into_iter().flatten() {
                    self.release(x);
                }
            }
            Data::Dict(v) => {
                for (k, x) in v {
                    self.release(k);
                    self.release_opt(x);
                }
            }
            Data::Stream(s) => self.release(s.dict),
            _ => {}
        }
        self.free_slots.push(o.0);
    }

    pub fn release_opt(&mut self, o: Option<Obj>) {
        if let Some(o) = o {
            self.release(o);
        }
    }

    fn mark_freed(&mut self, label: u32) {
        let i = (label / 8) as usize;
        if self.free_list.len() <= i {
            self.free_list.resize(i + 1, 0);
        }
        self.free_list[i] |= 1 << (7 - (label % 8));
    }

    #[must_use]
    pub fn is_freed(&self, label: u32) -> bool {
        self.free_list
            .get((label / 8) as usize)
            .is_some_and(|b| b & (1 << (7 - (label % 8))) != 0)
    }

    fn add_xref_entry(&mut self, label: u32, kind: u8, field2: u32, field3: u16) {
        let i = label as usize;
        if i >= self.xref.len() {
            self.xref.resize(i + 1, XrefEntry::default());
        }
        self.xref[i] = XrefEntry {
            kind,
            field2,
            field3,
        };
    }

    fn flush_obj(&mut self, o: Obj) {
        let (label, generation) = (self.slot(o).label, self.slot(o).generation);
        self.add_xref_entry(label, 1, self.file_position as u32, generation);
        let mut b = Buf::new();
        b.uint(label);
        b.push(b' ');
        b.uint(u32::from(generation));
        b.extend(b" obj\n");
        self.out_str(&b.0);
        self.write_obj(Some(o));
        self.out_str(b"\nendobj\n");
    }

    fn add_objstm(&mut self, objstm: Obj, o: Obj) -> i32 {
        let label = self.slot(o).label;
        let len = self.stream_length(objstm) as i32;
        let pos = {
            let data = self
                .stream_mut(objstm)
                .objstm
                .as_mut()
                .expect("object stream");
            data[0] += 1;
            let pos = data[0] as usize;
            data[2 * pos] = label as i32;
            data[2 * pos + 1] = len;
            pos as i32
        };
        let objstm_label = self.slot(objstm).label;
        self.add_xref_entry(label, 2, objstm_label, (pos - 1) as u16);
        self.output_stream = Some(objstm);
        self.write_obj(Some(o));
        self.out_char(b'\n');
        self.output_stream = None;
        pos
    }

    fn release_objstm(&mut self, objstm: Obj) {
        let (data, old) = {
            let st = self.stream_mut(objstm);
            (
                st.objstm.clone().expect("object stream"),
                core::mem::take(&mut st.data),
            )
        };
        let pos = data[0] as usize;
        let mut head = Buf::new();
        for v in &data[2..2 + 2 * pos] {
            head.int(*v);
            head.push(b' ');
        }
        self.stream_mut(objstm).data = head.0;
        let dict = self.stream_dict(objstm);
        self.put_name(dict, b"Type", b"ObjStm");
        self.put_number(dict, b"N", pos as f64);
        let first = self.stream_length(objstm) as f64;
        self.put_number(dict, b"First", first);
        self.add_stream(objstm, &old);
        self.release(objstm);
    }

    // The output.

    fn out_char(&mut self, c: u8) {
        if let Some(s) = self.output_stream {
            self.add_stream(s, &[c]);
        } else {
            self.out.push(c);
            self.file_position += 1;
            if c == b'\n' {
                self.line_position = 0;
            } else {
                self.line_position += 1;
            }
        }
    }

    fn out_xchar(&mut self, c: u8) {
        const X: &[u8; 16] = b"0123456789abcdef";
        self.out_char(X[(c >> 4) as usize]);
        self.out_char(X[(c & 15) as usize]);
    }

    fn out_str(&mut self, s: &[u8]) {
        if let Some(st) = self.output_stream {
            self.add_stream(st, s);
        } else {
            self.out.extend_from_slice(s);
            self.file_position += s.len();
            self.line_position += s.len();
            if s.last() == Some(&b'\n') {
                self.line_position = 0;
            }
        }
    }

    fn out_white(&mut self) {
        if self.line_position >= 80 {
            self.out_char(b'\n');
        } else {
            self.out_char(b' ');
        }
    }

    fn write_obj(&mut self, o: Option<Obj>) {
        let Some(o) = o else {
            self.out_str(b"null");
            return;
        };
        match self.type_of(Some(o)) {
            PDF_BOOLEAN => {
                if self.boolean_value(o) {
                    self.out_str(b"true");
                } else {
                    self.out_str(b"false");
                }
            }
            PDF_NUMBER => {
                let mut b = Buf::new();
                sprint_number(&mut b, self.number_value(o));
                self.out_str(&b.0);
            }
            PDF_STRING => {
                let s = self.string_value(o).to_vec();
                self.write_string(&s);
            }
            PDF_NAME => {
                let s = self.name_value(o).to_vec();
                self.write_name(&s);
            }
            PDF_ARRAY => {
                self.out_char(b'[');
                let items = self.array_items(o);
                let mut type1 = PDF_UNDEFINED;
                for x in items.into_iter().flatten() {
                    let type2 = self.type_of(Some(x));
                    if type1 != PDF_UNDEFINED && need_white(type1, type2) {
                        self.out_white();
                    }
                    type1 = type2;
                    self.write_obj(Some(x));
                }
                self.out_char(b']');
            }
            PDF_DICT => self.write_dict(o),
            PDF_STREAM => self.write_stream(o),
            PDF_NULL => self.out_str(b"null"),
            PDF_INDIRECT => {
                let i = self.indirect(o);
                debug_assert!(i.pf.is_none());
                let mut b = Buf::new();
                b.uint(i.label);
                b.push(b' ');
                b.uint(u32::from(i.generation));
                b.extend(b" R");
                self.out_str(&b.0);
            }
            t => panic!("pdf_write_obj: Invalid object, type = {t}"),
        }
    }

    fn write_dict(&mut self, d: Obj) {
        self.out_str(b"<<");
        for (k, v) in self.dict_entries(d) {
            self.write_obj(Some(k));
            let t = self.type_of(v);
            // (a C null value is written as null; its "type" for spacing
            // is that of the null pointer, which crashes in C: never seen)
            if need_white(PDF_NAME, if v.is_some() { t } else { PDF_NULL }) {
                self.out_white();
            }
            self.write_obj(v);
        }
        self.out_str(b">>");
    }

    fn write_string(&mut self, s: &[u8]) {
        let nescc = s.iter().filter(|&&c| !(32..=126).contains(&c)).count();
        if nescc > s.len() / 3 {
            self.out_char(b'<');
            for &c in s {
                self.out_xchar(c);
            }
            self.out_char(b'>');
        } else {
            self.out_char(b'(');
            for &c in s {
                let mut b = Buf::new();
                escape_char(&mut b, c);
                self.out_str(&b.0);
            }
            self.out_char(b')');
        }
    }

    fn write_name(&mut self, s: &[u8]) {
        self.out_char(b'/');
        for &c in s {
            if !(b'!'..=b'~').contains(&c) || c == b'#' || is_name_delim(c) {
                self.out_char(b'#');
                self.out_xchar(c);
            } else {
                self.out_char(c);
            }
        }
    }

    fn write_stream(&mut self, s: Obj) {
        let dict = self.stream_dict(s);
        let mut filtered = self.stream_data(s).to_vec();
        if let Some(t) = self.lookup_dict(dict, b"Type")
            && self.name_value(t) == b"Metadata"
        {
            self.stream_mut(s).flags &= !STREAM_COMPRESS;
        }
        let (flags, parms) = {
            let st = self.stream(s);
            (st.flags, st.parms)
        };
        if !filtered.is_empty() && flags & STREAM_COMPRESS != 0 && self.compression_level > 0 {
            if self.use_predictor
                && flags & STREAM_USE_PREDICTOR != 0
                && self.lookup_dict(dict, b"DecodeParms").is_none()
            {
                let bits_per_pixel = parms.colors * parms.bits_per_component;
                let len = (parms.columns * bits_per_pixel + 7) / 8;
                let rows = filtered.len() as i32 / len;
                let p = self.new_dict();
                self.put_number(p, b"BitsPerComponent", f64::from(parms.bits_per_component));
                self.put_number(p, b"Colors", f64::from(parms.colors));
                self.put_number(p, b"Columns", f64::from(parms.columns));
                self.put_number(p, b"Predictor", f64::from(parms.predictor));
                let filtered2 = match parms.predictor {
                    2 => Some(crate::filter::tiff2_apply(
                        &filtered,
                        parms.columns,
                        rows,
                        parms.bits_per_component,
                        parms.colors,
                    )),
                    15 => Some(crate::filter::png15_apply(
                        &filtered,
                        parms.columns,
                        rows,
                        parms.bits_per_component,
                        parms.colors,
                    )),
                    _ => None,
                };
                if let Some(f2) = filtered2 {
                    filtered = f2;
                    self.put(dict, b"DecodeParms", p);
                } else {
                    // (C leaks the dictionary; it is not written)
                    self.release(p);
                }
            }
            let filters = self.lookup_dict(dict, b"Filter");
            let name = self.new_name(b"FlateDecode");
            if let Some(f) = filters {
                self.unshift_array(f, name);
            } else {
                self.put(dict, b"Filter", name);
            }
            filtered = (self.deflate.as_ref().expect("deflater").borrow_mut())(
                self.compression_level,
                &filtered,
            );
        }
        let n = self.new_number(filtered.len() as f64);
        self.put(dict, b"Length", n);
        self.write_obj(Some(dict));
        self.out_str(b"\nstream\n");
        if !filtered.is_empty() {
            self.out_str(&filtered);
        }
        self.out_str(b"\n");
        self.out_str(b"endstream");
    }

    // ---------------------------------------------------------------
    // The file.

    /// `pdf_out_init`.
    pub fn init(
        &mut self,
        id1: &[u8; 16],
        id2: &[u8; 16],
        ver_major: i32,
        ver_minor: i32,
        compression_level: i32,
        enable_objstm: bool,
        enable_predictor: bool,
    ) {
        let version = ver_major * 10 + ver_minor;
        if (crate::PDF_VERSION_MIN..=crate::PDF_VERSION_MAX).contains(&version) {
            self.version_major = ver_major;
            self.version_minor = ver_minor;
        }
        if (0..=9).contains(&compression_level) {
            self.compression_level = compression_level;
        }
        self.add_xref_entry(0, 0, 0, 0xffff);
        self.enable_encrypt = false;
        if self.check_version(1, 5) == 0 && enable_objstm {
            let xs = self.new_stream(STREAM_COMPRESS);
            self.set_flags(xs, OBJ_NO_ENCRYPT);
            let t = self.stream_dict(xs);
            self.put_name(t, b"Type", b"XRef");
            self.xref_stream = Some(xs);
            self.trailer = Some(t);
            self.use_objstm = true;
        } else {
            self.xref_stream = None;
            self.trailer = Some(self.new_dict());
            self.use_objstm = false;
        }
        self.open = true;
        self.out_str(b"%PDF-");
        let v = [
            b'0' + self.version_major as u8,
            b'.',
            b'0' + self.version_minor as u8,
            b'\n',
        ];
        self.out_str(&v);
        self.out_str(BINARY_MARKER);
        let ids = self.new_array();
        let a = self.new_string(id1);
        self.add_array(ids, a);
        let b = self.new_string(id2);
        self.add_array(ids, b);
        let t = self.trailer.expect("trailer");
        self.put(t, b"ID", ids);
        self.use_predictor = enable_predictor;
    }

    #[must_use]
    pub fn get_version(&self) -> i32 {
        self.version_major * 10 + self.version_minor
    }

    /// `pdf_check_version`: 0 if the output's version is at least
    /// `major.minor`, -1 otherwise.
    #[must_use]
    pub fn check_version(&self, major: i32, minor: i32) -> i32 {
        if self.version_major > major {
            0
        } else if self.version_major < major {
            -1
        } else if self.version_minor >= minor {
            0
        } else {
            -1
        }
    }

    pub fn set_root(&mut self, o: Obj) {
        let t = self.trailer.expect("trailer");
        assert!(
            self.lookup_dict(t, b"Root").is_none(),
            "Root object already set!"
        );
        let r = self.ref_obj(o);
        self.put(t, b"Root", r);
    }

    pub fn set_info(&mut self, o: Obj) {
        let t = self.trailer.expect("trailer");
        assert!(
            self.lookup_dict(t, b"Info").is_none(),
            "Info object already set!"
        );
        let r = self.ref_obj(o);
        self.put(t, b"Info", r);
    }

    /// `pdf_out_flush`: the last object stream, the cross-reference
    /// section and the trailer.
    pub fn flush(&mut self) {
        if !self.open {
            return;
        }
        if let Some(s) = self.current_objstm.take() {
            self.release_objstm(s);
        }
        if let Some(xs) = self.xref_stream {
            self.label_obj(xs);
        }
        self.startxref = self.file_position as u32;
        let t = self.trailer.expect("trailer");
        self.put_number(t, b"Size", f64::from(self.next_label));
        if self.xref_stream.is_some() {
            self.dump_xref_stream();
        } else {
            self.dump_xref_table();
            self.out_str(b"trailer\n");
            self.write_dict(t);
            self.release(t);
            self.trailer = None;
            self.out_char(b'\n');
        }
        self.xref.clear();
        self.out_str(b"startxref\n");
        let mut b = Buf::new();
        b.uint(self.startxref);
        b.push(b'\n');
        self.out_str(&b.0);
        self.out_str(b"%%EOF\n");
        self.open = false;
    }

    fn dump_xref_table(&mut self) {
        self.out_str(b"xref\n");
        let mut b = Buf::new();
        b.extend(b"0 ");
        b.uint(self.next_label);
        b.push(b'\n');
        self.out_str(&b.0);
        for i in 0..self.next_label as usize {
            let e = self.xref.get(i).copied().unwrap_or_default();
            assert!(
                e.kind <= 1,
                "object type {} not allowed in xref table",
                e.kind
            );
            let mut b = Buf::new();
            b.zero_padded(u64::from(e.field2), 10);
            b.push(b' ');
            b.zero_padded(u64::from(e.field3), 5);
            b.push(b' ');
            b.push(if e.kind != 0 { b'n' } else { b'f' });
            b.extend(b" \n");
            self.out_str(&b.0);
        }
    }

    fn dump_xref_stream(&mut self) {
        let xs = self.xref_stream.expect("xref stream");
        let mut pos = self.startxref;
        let mut poslen = 1usize;
        loop {
            pos >>= 8;
            if pos == 0 {
                break;
            }
            poslen += 1;
        }
        let w = self.new_array();
        for v in [1.0, poslen as f64, 2.0] {
            let n = self.new_number(v);
            self.add_array(w, n);
        }
        let t = self.trailer.expect("trailer");
        self.put(t, b"W", w);
        let label = self.next_label - 1;
        self.add_xref_entry(label, 1, self.startxref, 0);
        for i in 0..self.next_label as usize {
            let e = self.xref.get(i).copied().unwrap_or_default();
            let mut buf = [0u8; 7];
            buf[0] = e.kind;
            let mut pos = e.field2;
            for j in (0..poslen).rev() {
                buf[1 + j] = pos as u8;
                pos >>= 8;
            }
            buf[poslen + 1] = (e.field3 >> 8) as u8;
            buf[poslen + 2] = e.field3 as u8;
            self.add_stream(xs, &buf[..poslen + 3]);
        }
        self.release(xs);
        self.xref_stream = None;
    }

    /// Takes what was written since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    // ---------------------------------------------------------------
    // Comparing (`pdf_compare_reference`, `pdf_compare_object`).

    #[must_use]
    pub fn compare_reference(&self, r1: Obj, r2: Obj) -> bool {
        let (a, b) = (self.indirect(r1), self.indirect(r2));
        a.pf != b.pf || a.label != b.label || a.generation != b.generation
    }

    /// 0 if equal.
    pub fn compare_object(&mut self, o1: Option<Obj>, o2: Option<Obj>) -> i32 {
        let (o1, o2) = match (o1, o2) {
            (None, None) => return 0,
            (Some(a), Some(b)) => (a, b),
            _ => return 1,
        };
        if self.type_of(Some(o1)) != self.type_of(Some(o2)) {
            return 1;
        }
        match self.type_of(Some(o1)) {
            PDF_BOOLEAN => i32::from(self.boolean_value(o1)) - i32::from(self.boolean_value(o2)),
            PDF_NUMBER => {
                let (a, b) = (self.number_value(o1), self.number_value(o2));
                if a < b {
                    -1
                } else if a > b {
                    1
                } else {
                    0
                }
            }
            PDF_STRING => {
                let (a, b) = (self.string_value(o1), self.string_value(o2));
                match a.len().cmp(&b.len()) {
                    core::cmp::Ordering::Less => -1,
                    core::cmp::Ordering::Greater => 1,
                    core::cmp::Ordering::Equal => memcmp(a, b),
                }
            }
            PDF_NAME => memcmp(self.name_value(o1), self.name_value(o2)),
            PDF_NULL => 0,
            PDF_INDIRECT => i32::from(self.compare_reference(o1, o2)),
            PDF_ARRAY => {
                let (n1, n2) = (self.array_length(o1), self.array_length(o2));
                if n1 < n2 {
                    return -1;
                } else if n1 > n2 {
                    return 1;
                }
                let mut r = 0;
                for i in 0..n1 {
                    if r != 0 {
                        break;
                    }
                    let v1 = self.get_array(o1, i as i32);
                    let v2 = self.get_array(o2, i as i32);
                    r = self.compare_object(v1, v2);
                }
                r
            }
            PDF_DICT => {
                let k1 = self.dict_keys(o1);
                let k2 = self.dict_keys(o2);
                let mut r = self.compare_object(Some(k1), Some(k2));
                if r == 0 {
                    for i in 0..self.array_length(k1) {
                        if r != 0 {
                            break;
                        }
                        let key = self.get_array(k1, i as i32).expect("key");
                        let name = self.name_value(key).to_vec();
                        let v1 = self.lookup_dict(o1, &name);
                        let v2 = self.lookup_dict(o2, &name);
                        r = self.compare_object(v1, v2);
                    }
                }
                self.release(k1);
                self.release(k2);
                r
            }
            PDF_STREAM => {
                let (d1, d2) = (self.stream_dict(o1), self.stream_dict(o2));
                let mut r = self.compare_object(Some(d1), Some(d2));
                if r == 0 {
                    let (l1, l2) = (self.stream_length(o1), self.stream_length(o2));
                    r = match l1.cmp(&l2) {
                        core::cmp::Ordering::Less => -1,
                        core::cmp::Ordering::Greater => 1,
                        core::cmp::Ordering::Equal => 0,
                    };
                }
                r
            }
            _ => 1,
        }
    }
}

/// C's `memcmp`/`strcmp` sign.
#[must_use]
pub fn memcmp(a: &[u8], b: &[u8]) -> i32 {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return i32::from(*x) - i32::from(*y);
        }
    }
    match a.len().cmp(&b.len()) {
        core::cmp::Ordering::Less => -i32::from(b[a.len()]),
        core::cmp::Ordering::Greater => i32::from(a[b.len()]),
        core::cmp::Ordering::Equal => 0,
    }
}

fn typecheck() -> ! {
    panic!("typecheck: Invalid object type")
}

fn need_white(type1: i32, type2: i32) -> bool {
    !(type1 == PDF_STRING
        || type1 == PDF_ARRAY
        || type1 == PDF_DICT
        || type2 == PDF_STRING
        || type2 == PDF_NAME
        || type2 == PDF_ARRAY
        || type2 == PDF_DICT)
}

fn is_name_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%'
    )
}

/// `pdfobj_escape_str` for one character.
pub fn escape_char(b: &mut Buf, c: u8) {
    if !(32..=126).contains(&c) {
        b.push(b'\\');
        b.push(b'0' + (c >> 6));
        b.push(b'0' + ((c >> 3) & 7));
        b.push(b'0' + (c & 7));
    } else {
        match c {
            b'(' | b')' | b'\\' => {
                b.push(b'\\');
                b.push(c);
            }
            _ => b.push(c),
        }
    }
}

/// `pdfobj_escape_str`.
pub fn escape_str(b: &mut Buf, s: &[u8]) {
    for &c in s {
        escape_char(b, c);
    }
}

impl Default for PdfOut {
    /// An empty writer whose deflater is never run: what `core::mem::take`
    /// leaves in `Dpx::o` while the objects are lent to a parser
    /// (specials.rs, `Dpx::with_dpx_unknown`). Allocates nothing.
    fn default() -> Self {
        let mut o = PdfOut::new(Box::new(|_, _| Vec::new()));
        o.deflate = None;
        o
    }
}
