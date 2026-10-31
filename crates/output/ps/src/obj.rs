//! PostScript objects as Ghostscript holds them: integers (64-bit), reals
//! (single precision, as `float`), names, and the composite objects
//! (strings, arrays, dictionaries) shared by reference, in a virtual
//! memory that `save` and `restore` roll back (local VM; global VM is
//! not restored).

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

/// An interned name.
pub type Name = u32;

/// Access of a composite object.
pub const ACC_UNLIMITED: u8 = 3;
pub const ACC_READONLY: u8 = 2;
pub const ACC_EXECONLY: u8 = 1;
pub const ACC_NONE: u8 = 0;

/// A composite object's storage: its creation save level and whether it
/// was snapshotted since the current save.
pub struct Buf<T> {
    pub data: RefCell<T>,
    /// The save level it was made at (0: before any save).
    pub level: u32,
    /// The latest save level it was snapshotted for.
    pub snap: Cell<u32>,
    pub global: bool,
    pub acc: Cell<u8>,
}

impl<T> Buf<T> {
    pub fn new(data: T, level: u32, global: bool) -> Rc<Buf<T>> {
        Rc::new(Buf {
            data: RefCell::new(data),
            level,
            snap: Cell::new(level),
            global,
            acc: Cell::new(ACC_UNLIMITED),
        })
    }
}

/// A string: a window on a shared byte buffer.
#[derive(Clone)]
pub struct PStr {
    pub b: Rc<Buf<Vec<u8>>>,
    pub off: usize,
    pub len: usize,
}

impl PStr {
    pub fn bytes(&self) -> Vec<u8> {
        self.b.data.borrow()[self.off..self.off + self.len].to_vec()
    }
    pub fn with<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        f(&self.b.data.borrow()[self.off..self.off + self.len])
    }
    pub fn get(&self, i: usize) -> u8 {
        self.b.data.borrow()[self.off + i]
    }
    pub fn sub(&self, i: usize, n: usize) -> PStr {
        PStr {
            b: self.b.clone(),
            off: self.off + i,
            len: n,
        }
    }
    pub fn same(&self, o: &PStr) -> bool {
        Rc::ptr_eq(&self.b, &o.b) && self.off == o.off && self.len == o.len
    }
}

/// An array (or packed array): a window on a shared object buffer.
#[derive(Clone)]
pub struct PArr {
    pub b: Rc<Buf<Vec<Obj>>>,
    pub off: usize,
    pub len: usize,
    pub packed: bool,
}

impl PArr {
    pub fn get(&self, i: usize) -> Obj {
        self.b.data.borrow()[self.off + i].clone()
    }
    pub fn to_vec(&self) -> Vec<Obj> {
        self.b.data.borrow()[self.off..self.off + self.len].to_vec()
    }
    pub fn sub(&self, i: usize, n: usize) -> PArr {
        PArr {
            b: self.b.clone(),
            off: self.off + i,
            len: n,
            packed: self.packed,
        }
    }
    pub fn same(&self, o: &PArr) -> bool {
        Rc::ptr_eq(&self.b, &o.b) && self.off == o.off && self.len == o.len
    }
}

/// A dictionary key: names (strings are made names), integers (a real
/// with an integral value is made an integer), other reals by their
/// bits, booleans, and other objects by identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Key {
    Name(Name),
    Int(i64),
    Real(u32),
    Bool(bool),
    Null,
    Mark,
    Id(usize),
}

/// A dictionary's entries, in the order they were first defined.
#[derive(Clone, Default)]
pub struct DictData {
    pub keys: Vec<Key>,
    pub vals: Vec<Obj>,
    /// The key objects as given (a name key's object, for `forall`).
    pub kobjs: Vec<Obj>,
    pub index: BTreeMap<Key, usize>,
    pub maxlen: usize,
}

impl DictData {
    pub fn get(&self, k: &Key) -> Option<&Obj> {
        self.index.get(k).map(|&i| &self.vals[i])
    }
    pub fn put(&mut self, k: Key, ko: Obj, v: Obj) {
        if let Some(&i) = self.index.get(&k) {
            self.vals[i] = v;
        } else {
            self.index.insert(k, self.keys.len());
            self.keys.push(k);
            self.kobjs.push(ko);
            self.vals.push(v);
            if self.keys.len() > self.maxlen {
                self.maxlen = self.keys.len();
            }
        }
    }
    pub fn remove(&mut self, k: &Key) -> bool {
        let Some(i) = self.index.remove(k) else {
            return false;
        };
        self.keys.remove(i);
        self.kobjs.remove(i);
        self.vals.remove(i);
        for (j, k) in self.keys.iter().enumerate().skip(i) {
            self.index.insert(*k, j);
        }
        true
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

pub type PDict = Rc<Buf<DictData>>;

/// A file: a source of bytes being read (the program, a string, or a
/// decoding filter over another source).
pub type PFile = Rc<RefCell<crate::file::File>>;

/// An object's value.
#[derive(Clone)]
pub enum V {
    Null,
    Int(i64),
    Real(f32),
    Bool(bool),
    Name(Name),
    Str(PStr),
    Arr(PArr),
    Dict(PDict),
    Op(u16),
    Mark,
    File(PFile),
    Save(u32),
    FontId(u32),
    GState(Rc<RefCell<crate::gstate::GState>>),
}

/// An object: a value, executable or literal.
#[derive(Clone)]
pub struct Obj {
    pub v: V,
    pub x: bool,
}

impl Obj {
    pub const NULL: Obj = Obj {
        v: V::Null,
        x: false,
    };
    pub fn int(i: i64) -> Obj {
        Obj {
            v: V::Int(i),
            x: false,
        }
    }
    pub fn real(r: f32) -> Obj {
        Obj {
            v: V::Real(r),
            x: false,
        }
    }
    pub fn bool(b: bool) -> Obj {
        Obj {
            v: V::Bool(b),
            x: false,
        }
    }
    pub fn name(n: Name) -> Obj {
        Obj {
            v: V::Name(n),
            x: false,
        }
    }
    pub fn xname(n: Name) -> Obj {
        Obj {
            v: V::Name(n),
            x: true,
        }
    }
    pub fn mark() -> Obj {
        Obj {
            v: V::Mark,
            x: false,
        }
    }
    pub fn dict(d: PDict) -> Obj {
        Obj {
            v: V::Dict(d),
            x: false,
        }
    }
    pub fn arr(a: PArr) -> Obj {
        Obj {
            v: V::Arr(a),
            x: false,
        }
    }
    pub fn str(s: PStr) -> Obj {
        Obj {
            v: V::Str(s),
            x: false,
        }
    }
    pub fn exec(mut self) -> Obj {
        self.x = true;
        self
    }
    pub fn lit(mut self) -> Obj {
        self.x = false;
        self
    }
    /// The type name (`type`).
    pub fn type_name(&self) -> &'static str {
        match &self.v {
            V::Null => "nulltype",
            V::Int(_) => "integertype",
            V::Real(_) => "realtype",
            V::Bool(_) => "booleantype",
            V::Name(_) => "nametype",
            V::Str(_) => "stringtype",
            V::Arr(a) => {
                if a.packed {
                    "packedarraytype"
                } else {
                    "arraytype"
                }
            }
            V::Dict(_) => "dicttype",
            V::Op(_) => "operatortype",
            V::Mark => "marktype",
            V::File(_) => "filetype",
            V::Save(_) => "savetype",
            V::FontId(_) => "fonttype",
            V::GState(_) => "gstatetype",
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self.v {
            V::Int(i) => Some(i as f64),
            V::Real(r) => Some(f64::from(r)),
            _ => None,
        }
    }
    pub fn is_proc(&self) -> bool {
        self.x && matches!(self.v, V::Arr(_))
    }
    /// Equality as `eq` sees it.
    pub fn eq_ps(&self, o: &Obj, names: &crate::interp::Names) -> bool {
        match (&self.v, &o.v) {
            (V::Int(a), V::Int(b)) => a == b,
            (V::Int(_) | V::Real(_), V::Int(_) | V::Real(_)) => {
                // (gs compares an integer and a real as reals)
                let a = match self.v {
                    V::Int(i) => i as f32,
                    V::Real(r) => r,
                    _ => 0.0,
                };
                let b = match o.v {
                    V::Int(i) => i as f32,
                    V::Real(r) => r,
                    _ => 0.0,
                };
                a == b
            }
            (V::Bool(a), V::Bool(b)) => a == b,
            (V::Null, V::Null) | (V::Mark, V::Mark) => true,
            (V::Name(a), V::Name(b)) => a == b,
            (V::Str(a), V::Str(b)) => a.with(|x| b.with(|y| x == y)),
            (V::Name(a), V::Str(b)) | (V::Str(b), V::Name(a)) => b.with(|y| names.get(*a) == y),
            (V::Arr(a), V::Arr(b)) => a.same(b),
            (V::Dict(a), V::Dict(b)) => Rc::ptr_eq(a, b),
            (V::Op(a), V::Op(b)) => a == b,
            (V::File(a), V::File(b)) => Rc::ptr_eq(a, b),
            (V::Save(a), V::Save(b)) => a == b,
            (V::FontId(a), V::FontId(b)) => a == b,
            (V::GState(a), V::GState(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// What `restore` puts back.
pub enum Undo {
    Str(Rc<Buf<Vec<u8>>>, Vec<u8>),
    Arr(Rc<Buf<Vec<Obj>>>, Vec<Obj>),
    Dict(PDict, DictData),
}

/// The virtual memory's save state.
#[derive(Default)]
pub struct Vm {
    /// The current save level (0: none).
    pub level: u32,
    /// Each open save's undo log, innermost last.
    pub logs: Vec<Vec<Undo>>,
    /// Allocation in global VM.
    pub global: bool,
}

impl Vm {
    fn note<T: Clone>(&mut self, b: &Rc<Buf<T>>, wrap: impl FnOnce(Rc<Buf<T>>, T) -> Undo) {
        if b.global || b.level >= self.level || b.snap.get() >= self.level {
            return;
        }
        b.snap.set(self.level);
        let old = b.data.borrow().clone();
        if let Some(l) = self.logs.last_mut() {
            l.push(wrap(b.clone(), old));
        }
    }
    pub fn note_str(&mut self, b: &Rc<Buf<Vec<u8>>>) {
        self.note(b, Undo::Str);
    }
    pub fn note_arr(&mut self, b: &Rc<Buf<Vec<Obj>>>) {
        self.note(b, Undo::Arr);
    }
    pub fn note_dict(&mut self, b: &PDict) {
        self.note(b, Undo::Dict);
    }
    pub fn new_str(&self, v: Vec<u8>) -> PStr {
        let len = v.len();
        PStr {
            b: Buf::new(v, self.level, self.global),
            off: 0,
            len,
        }
    }
    pub fn new_arr(&self, v: Vec<Obj>) -> PArr {
        let len = v.len();
        PArr {
            b: Buf::new(v, self.level, self.global),
            off: 0,
            len,
            packed: false,
        }
    }
    pub fn new_dict(&self, n: usize) -> PDict {
        Buf::new(
            DictData {
                maxlen: n,
                ..DictData::default()
            },
            self.level,
            self.global,
        )
    }
    /// `save`: a new level.
    pub fn save(&mut self) -> u32 {
        self.level += 1;
        self.logs.push(Vec::new());
        self.level
    }
    /// `restore` to just before save `level` was made.
    pub fn restore(&mut self, level: u32) {
        while self.level >= level && self.level > 0 {
            let log = self.logs.pop().unwrap_or_default();
            for u in log.into_iter().rev() {
                match u {
                    Undo::Str(b, d) => {
                        *b.data.borrow_mut() = d;
                        b.snap.set(self.level - 1);
                    }
                    Undo::Arr(b, d) => {
                        *b.data.borrow_mut() = d;
                        b.snap.set(self.level - 1);
                    }
                    Undo::Dict(b, d) => {
                        *b.data.borrow_mut() = d;
                        b.snap.set(self.level - 1);
                    }
                }
            }
            self.level -= 1;
        }
    }
}
