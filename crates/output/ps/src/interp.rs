//! The interpreter: the operand, execution and dictionary stacks,
//! name lookup, the execution loop, errors (`errordict`, `stopped`),
//! and the language's operators (stack, arithmetic, composite objects,
//! control, types, files, virtual memory). Graphics, fonts and the
//! device are in their own modules, on the same [`Ps`].

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::file::File;
use crate::obj::{Key, Name, Obj, PArr, PDict, PFile, PStr, V, Vm};
use crate::scan::{ScanErr, Tok};

/// The PostScript errors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum E {
    DictFull,
    DictStackOverflow,
    DictStackUnderflow,
    ExecStackOverflow,
    InvalidAccess,
    InvalidExit,
    InvalidFileAccess,
    InvalidFont,
    InvalidRestore,
    IoError,
    LimitCheck,
    NoCurrentPoint,
    RangeCheck,
    StackOverflow,
    StackUnderflow,
    SyntaxError,
    Timeout,
    TypeCheck,
    Undefined,
    UndefinedFilename,
    UndefinedResource,
    UndefinedResult,
    UnmatchedMark,
    Unregistered,
    VmError,
    ConfigurationError,
}

impl E {
    pub fn name(self) -> &'static str {
        match self {
            E::DictFull => "dictfull",
            E::DictStackOverflow => "dictstackoverflow",
            E::DictStackUnderflow => "dictstackunderflow",
            E::ExecStackOverflow => "execstackoverflow",
            E::InvalidAccess => "invalidaccess",
            E::InvalidExit => "invalidexit",
            E::InvalidFileAccess => "invalidfileaccess",
            E::InvalidFont => "invalidfont",
            E::InvalidRestore => "invalidrestore",
            E::IoError => "ioerror",
            E::LimitCheck => "limitcheck",
            E::NoCurrentPoint => "nocurrentpoint",
            E::RangeCheck => "rangecheck",
            E::StackOverflow => "stackoverflow",
            E::StackUnderflow => "stackunderflow",
            E::SyntaxError => "syntaxerror",
            E::Timeout => "timeout",
            E::TypeCheck => "typecheck",
            E::Undefined => "undefined",
            E::UndefinedFilename => "undefinedfilename",
            E::UndefinedResource => "undefinedresource",
            E::UndefinedResult => "undefinedresult",
            E::UnmatchedMark => "unmatchedmark",
            E::Unregistered => "unregistered",
            E::VmError => "VMerror",
            E::ConfigurationError => "configurationerror",
        }
    }
    pub const ALL: [E; 26] = [
        E::DictFull,
        E::DictStackOverflow,
        E::DictStackUnderflow,
        E::ExecStackOverflow,
        E::InvalidAccess,
        E::InvalidExit,
        E::InvalidFileAccess,
        E::InvalidFont,
        E::InvalidRestore,
        E::IoError,
        E::LimitCheck,
        E::NoCurrentPoint,
        E::RangeCheck,
        E::StackOverflow,
        E::StackUnderflow,
        E::SyntaxError,
        E::Timeout,
        E::TypeCheck,
        E::Undefined,
        E::UndefinedFilename,
        E::UndefinedResource,
        E::UndefinedResult,
        E::UnmatchedMark,
        E::Unregistered,
        E::VmError,
        E::ConfigurationError,
    ];
}

/// How execution leaves the normal flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sig {
    Err(E),
    Exit,
    Stop,
    Quit,
}

impl From<E> for Sig {
    fn from(e: E) -> Sig {
        Sig::Err(e)
    }
}

pub type R<T = ()> = Result<T, Sig>;

/// An operator's code.
pub type OpFn = fn(&mut Ps<'_>) -> R;

/// The name table.
#[derive(Default)]
pub struct Names {
    map: BTreeMap<Vec<u8>, Name>,
    list: Vec<Vec<u8>>,
}

impl Names {
    pub fn intern(&mut self, s: &[u8]) -> Name {
        if let Some(&n) = self.map.get(s) {
            return n;
        }
        let n = self.list.len() as Name;
        self.list.push(s.to_vec());
        self.map.insert(s.to_vec(), n);
        n
    }
    pub fn get(&self, n: Name) -> &[u8] {
        &self.list[n as usize]
    }
    pub fn find(&self, s: &[u8]) -> Option<Name> {
        self.map.get(s).copied()
    }
}

/// An execution stack entry.
pub enum Frame {
    Proc(PArr, usize),
    File(PFile),
    Stopped,
    /// A nested run's bottom ([`Ps::call`]).
    Barrier,
    For {
        i: f64,
        inc: f64,
        lim: f64,
        int: bool,
        proc: Obj,
    },
    Repeat {
        n: i64,
        proc: Obj,
    },
    Loop {
        proc: Obj,
    },
    ForallArr {
        a: PArr,
        i: usize,
        proc: Obj,
    },
    ForallStr {
        s: PStr,
        i: usize,
        proc: Obj,
    },
    ForallDict {
        d: PDict,
        i: usize,
        proc: Obj,
    },
    /// A list of objects to push, then execute `proc`, per step
    /// (`pathforall`, `resourceforall`, `filenameforall`).
    Each {
        items: Vec<Vec<Obj>>,
        i: usize,
        procs: Vec<Obj>,
    },
    /// Ends a dictionary (`eexec`'s systemdict) when reached.
    EndDict,
}

impl Frame {
    fn is_loop(&self) -> bool {
        matches!(
            self,
            Frame::For { .. }
                | Frame::Repeat { .. }
                | Frame::Loop { .. }
                | Frame::ForallArr { .. }
                | Frame::ForallStr { .. }
                | Frame::ForallDict { .. }
                | Frame::Each { .. }
        )
    }
}

/// Where the converter finds what Ghostscript reads from its
/// installation: the fonts of its `Resource/Font` (`NimbusRoman-Regular`
/// for `Times-Roman`, Fontmap.GS's names), by name.
pub trait Resources {
    /// A font program by its file name in Ghostscript's `Resource/Font`
    /// (or a font of TeX Live standing in for it).
    fn font(&self, name: &[u8]) -> Option<Rc<Vec<u8>>>;
}

/// The interpreter.
pub struct Ps<'a> {
    pub names: Names,
    pub o: Vec<Obj>,
    pub e: Vec<Frame>,
    pub d: Vec<PDict>,
    pub vm: Vm,
    pub ops: Vec<(&'static str, OpFn)>,
    pub systemdict: PDict,
    pub userdict: PDict,
    pub globaldict: PDict,
    pub errordict: PDict,
    pub statusdict: PDict,
    pub dollar_error: PDict,
    pub fontdir: PDict,
    /// The resource categories: category name → instances dict.
    pub resources: PDict,
    pub res: &'a dyn Resources,
    pub packing: bool,
    pub cur_files: Vec<PFile>,
    /// The graphics state and its saved copies.
    pub gs: crate::gstate::GState,
    pub gstack: Vec<crate::gstate::GState>,
    /// gstate save levels (`save` marks one).
    pub save_gdepth: Vec<(u32, usize)>,
    pub dev: crate::pdf::Device,
    pub rand: u32,
    /// The error that stopped the job, if one did.
    pub failed: Option<String>,
    /// Steps run, against runaway programs.
    pub steps: u64,
    pub max_steps: u64,
    pub font_ids: u32,
    pub fonts: Vec<Option<Rc<crate::font::Font>>>,
    pub(crate) n: crate::names::N,
    /// gs_epsf.ps's EPSBoundingBoxState (5: no more DSC comments read).
    pub eps_state: i32,
    /// `showpage`s run.
    pub showpages: u32,
    /// `nulldevice` in effect: nothing is drawn.
    pub null_device: bool,
    /// Glyph names' ranks in gs's name table (`crate::cff::glyph_key`).
    pub name_ranks: Option<BTreeMap<Vec<u8>, u64>>,
}

pub fn ok() -> R {
    Ok(())
}

impl<'a> Ps<'a> {
    pub fn new(res: &'a dyn Resources, dev: crate::pdf::Device) -> Ps<'a> {
        let vm = Vm::default();
        let mut names = Names::default();
        let n = crate::names::N::new(&mut names);
        let mk = |vm: &Vm, k| vm.new_dict(k);
        let mut ps = Ps {
            names,
            o: Vec::new(),
            e: Vec::new(),
            d: Vec::new(),
            systemdict: mk(&vm, 600),
            userdict: mk(&vm, 200),
            globaldict: mk(&vm, 100),
            errordict: mk(&vm, 40),
            statusdict: mk(&vm, 100),
            dollar_error: mk(&vm, 20),
            fontdir: mk(&vm, 100),
            resources: mk(&vm, 40),
            vm,
            ops: Vec::new(),
            res,
            packing: false,
            cur_files: Vec::new(),
            gs: crate::gstate::GState::default(),
            gstack: Vec::new(),
            save_gdepth: Vec::new(),
            dev,
            rand: 0,
            failed: None,
            steps: 0,
            max_steps: 2_000_000_000,
            font_ids: 0,
            fonts: Vec::new(),
            n,
            eps_state: 5,
            showpages: 0,
            null_device: false,
            name_ranks: None,
        };
        ps.d = vec![
            ps.systemdict.clone(),
            ps.globaldict.clone(),
            ps.userdict.clone(),
        ];
        crate::init::init(&mut ps);
        ps
    }

    // ------ objects ------

    pub fn name(&mut self, s: &str) -> Name {
        self.names.intern(s.as_bytes())
    }
    pub fn nobj(&mut self, s: &str) -> Obj {
        Obj::name(self.name(s))
    }
    pub fn new_str(&self, v: Vec<u8>) -> PStr {
        self.vm.new_str(v)
    }
    pub fn str_obj(&self, v: &[u8]) -> Obj {
        Obj::str(self.vm.new_str(v.to_vec()))
    }
    pub fn arr_obj(&self, v: Vec<Obj>) -> Obj {
        Obj::arr(self.vm.new_arr(v))
    }
    pub fn new_dict(&self, n: usize) -> PDict {
        self.vm.new_dict(n)
    }
    /// The dictionary key of an object.
    pub fn key(&mut self, o: &Obj) -> R<Key> {
        Ok(match &o.v {
            V::Name(n) => Key::Name(*n),
            V::Str(s) => {
                let b = s.bytes();
                Key::Name(self.names.intern(&b))
            }
            V::Int(i) => Key::Int(*i),
            V::Real(r) => {
                if libm::truncf(*r) == *r && r.abs() < 9.0e15 {
                    Key::Int(*r as i64)
                } else {
                    Key::Real(r.to_bits())
                }
            }
            V::Bool(b) => Key::Bool(*b),
            V::Null => return Err(E::TypeCheck.into()),
            V::Mark => Key::Mark,
            V::Arr(a) => Key::Id(Rc::as_ptr(&a.b).cast::<u8>() as usize ^ a.off),
            V::Dict(d) => Key::Id(Rc::as_ptr(d).cast::<u8>() as usize),
            V::Op(i) => Key::Id(0x8000_0000_0000 + usize::from(*i)),
            V::File(f) => Key::Id(Rc::as_ptr(f).cast::<u8>() as usize),
            V::Save(s) => Key::Id(0x9000_0000_0000 + *s as usize),
            V::FontId(s) => Key::Id(0xa000_0000_0000 + *s as usize),
            V::GState(g) => Key::Id(Rc::as_ptr(g).cast::<u8>() as usize),
        })
    }
    /// The key object stored for `o` (a string key as its name).
    fn key_obj(&mut self, o: &Obj) -> Obj {
        match &o.v {
            V::Str(s) => {
                let b = s.bytes();
                Obj::name(self.names.intern(&b))
            }
            _ => o.clone().lit(),
        }
    }
    pub fn dget(&mut self, d: &PDict, k: &Obj) -> R<Option<Obj>> {
        let k = self.key(k)?;
        Ok(d.data.borrow().get(&k).cloned())
    }
    pub fn dget_n(&self, d: &PDict, n: Name) -> Option<Obj> {
        d.data.borrow().get(&Key::Name(n)).cloned()
    }
    pub fn dget_s(&mut self, d: &PDict, s: &str) -> Option<Obj> {
        let n = self.name(s);
        self.dget_n(d, n)
    }
    pub fn dput(&mut self, d: &PDict, k: Obj, v: Obj) -> R {
        let key = self.key(&k)?;
        let ko = self.key_obj(&k);
        self.vm.note_dict(d);
        d.data.borrow_mut().put(key, ko, v);
        Ok(())
    }
    pub fn dput_s(&mut self, d: &PDict, s: &str, v: Obj) {
        let k = self.nobj(s);
        let _ = self.dput(d, k, v);
    }
    /// Looks a name up on the dictionary stack.
    pub fn lookup(&self, n: Name) -> Option<Obj> {
        let k = Key::Name(n);
        for d in self.d.iter().rev() {
            if let Some(v) = d.data.borrow().get(&k) {
                return Some(v.clone());
            }
        }
        None
    }
    pub fn lookup_s(&mut self, s: &str) -> Option<Obj> {
        let n = self.name(s);
        self.lookup(n)
    }

    // ------ the operand stack ------

    pub fn push(&mut self, o: Obj) {
        self.o.push(o);
    }
    pub fn pushi(&mut self, i: i64) {
        self.o.push(Obj::int(i));
    }
    pub fn pushr(&mut self, r: f64) {
        self.o.push(Obj::real(r as f32));
    }
    pub fn pushb(&mut self, b: bool) {
        self.o.push(Obj::bool(b));
    }
    pub fn need(&self, n: usize) -> R {
        if self.o.len() < n {
            Err(E::StackUnderflow.into())
        } else {
            Ok(())
        }
    }
    pub fn pop(&mut self) -> R<Obj> {
        self.o.pop().ok_or(Sig::Err(E::StackUnderflow))
    }
    /// The object `i` from the top (0: the top).
    pub fn top(&self, i: usize) -> R<&Obj> {
        if i >= self.o.len() {
            return Err(E::StackUnderflow.into());
        }
        Ok(&self.o[self.o.len() - 1 - i])
    }
    pub fn int_at(&self, i: usize) -> R<i64> {
        match self.top(i)?.v {
            V::Int(v) => Ok(v),
            _ => Err(E::TypeCheck.into()),
        }
    }
    pub fn num_at(&self, i: usize) -> R<f64> {
        self.top(i)?.num().ok_or(Sig::Err(E::TypeCheck))
    }
    pub fn pop_int(&mut self) -> R<i64> {
        let v = self.int_at(0)?;
        self.o.pop();
        Ok(v)
    }
    pub fn pop_num(&mut self) -> R<f64> {
        let v = self.num_at(0)?;
        self.o.pop();
        Ok(v)
    }
    pub fn pop_bool(&mut self) -> R<bool> {
        match self.top(0)?.v {
            V::Bool(b) => {
                self.o.pop();
                Ok(b)
            }
            _ => Err(E::TypeCheck.into()),
        }
    }
    pub fn pop_dict(&mut self) -> R<PDict> {
        match &self.top(0)?.v {
            V::Dict(d) => {
                let d = d.clone();
                self.o.pop();
                Ok(d)
            }
            _ => Err(E::TypeCheck.into()),
        }
    }
    pub fn pop_str(&mut self) -> R<PStr> {
        match &self.top(0)?.v {
            V::Str(s) => {
                let s = s.clone();
                self.o.pop();
                Ok(s)
            }
            _ => Err(E::TypeCheck.into()),
        }
    }
    pub fn pop_arr(&mut self) -> R<PArr> {
        match &self.top(0)?.v {
            V::Arr(a) => {
                let a = a.clone();
                self.o.pop();
                Ok(a)
            }
            _ => Err(E::TypeCheck.into()),
        }
    }
    pub fn pop_proc(&mut self) -> R<Obj> {
        let o = self.top(0)?;
        if !matches!(o.v, V::Arr(_)) {
            return Err(E::TypeCheck.into());
        }
        self.pop()
    }
    /// The numbers of an array object.
    pub fn nums_of(&self, o: &Obj) -> R<Vec<f64>> {
        match &o.v {
            V::Arr(a) => a
                .to_vec()
                .iter()
                .map(|x| x.num().ok_or(Sig::Err(E::TypeCheck)))
                .collect(),
            _ => Err(E::TypeCheck.into()),
        }
    }

    // ------ execution ------

    /// Executes `o` as a name's value or `exec`'s operand does.
    pub fn exec_value(&mut self, o: Obj) -> R {
        if !o.x {
            self.o.push(o);
            return Ok(());
        }
        match &o.v {
            V::Name(n) => {
                let n = *n;
                match self.lookup(n) {
                    Some(v) => {
                        if v.x {
                            match &v.v {
                                V::Op(i) => {
                                    let i = usize::from(*i);
                                    return self.call_op(i);
                                }
                                V::Arr(a) => {
                                    self.e.push(Frame::Proc(a.clone(), 0));
                                    return Ok(());
                                }
                                _ => {}
                            }
                        }
                        self.exec_value(v)
                    }
                    None => {
                        self.o.push(o.clone());
                        Err(E::Undefined.into())
                    }
                }
            }
            V::Arr(a) => {
                self.e.push(Frame::Proc(a.clone(), 0));
                Ok(())
            }
            V::Op(i) => {
                let i = usize::from(*i);
                self.call_op(i)
            }
            V::File(f) => {
                self.e.push(Frame::File(f.clone()));
                Ok(())
            }
            V::Str(s) => {
                let f = Rc::new(RefCell::new(File::new(Rc::new(s.bytes()))));
                self.e.push(Frame::File(f));
                Ok(())
            }
            V::Null => Ok(()),
            _ => {
                self.o.push(o);
                Ok(())
            }
        }
    }

    /// Executes an object met in a procedure's body or a file: a
    /// procedure is pushed, not run.
    fn exec_direct(&mut self, o: Obj) -> R {
        if o.x && matches!(o.v, V::Arr(_)) {
            self.o.push(o);
            return Ok(());
        }
        self.exec_value(o)
    }

    fn call_op(&mut self, i: usize) -> R {
        let f = self.ops[i].1;
        let depth = self.o.len();
        let r = f(self);
        if let Err(Sig::Err(_)) = r {
            // (operators check before they pop; one that popped and
            // failed leaves fewer)
            let _ = depth;
            self.o.push(Obj {
                v: V::Op(i as u16),
                x: true,
            });
        }
        r
    }

    /// Runs the execution stack down to `base` entries.
    pub fn run(&mut self, base: usize) -> R {
        loop {
            if self.e.len() <= base {
                return Ok(());
            }
            self.steps += 1;
            if self.steps > self.max_steps {
                return Err(Sig::Quit);
            }
            let r = self.step(base);
            match r {
                Ok(()) => {}
                Err(Sig::Err(e)) => {
                    // (the offending command is on the operand stack,
                    // pushed where the error was raised)
                    let r = self.raise(e);
                    if let Err(s) = r {
                        self.unwind(s, base)?;
                    }
                }
                Err(s) => self.unwind(s, base)?,
            }
        }
    }

    /// Calls `errordict`'s handler for `e`.
    fn raise(&mut self, e: E) -> R {
        let n = self.name(e.name());
        let h = self.dget_n(&self.errordict.clone(), n);
        match h {
            Some(h) => self.exec_value(h),
            None => self.default_error(e),
        }
    }

    /// The default error handler: records the error in `$error` and
    /// stops.
    pub fn default_error(&mut self, e: E) -> R {
        let cmd = self.o.pop().unwrap_or(Obj::NULL);
        let de = self.dollar_error.clone();
        self.dput_s(&de, "newerror", Obj::bool(true));
        let en = self.nobj(e.name());
        self.dput_s(&de, "errorname", en);
        self.dput_s(&de, "command", cmd.clone());
        if self.failed.is_none() {
            let c = self.show_obj(&cmd);
            self.failed = Some(format!("Error: /{} in {}", e.name(), c));
        }
        Err(Sig::Stop)
    }

    /// Unwinds the execution stack for `s` down to `base`: `stop` to the
    /// innermost `stopped`, `exit` to the innermost loop.
    fn unwind(&mut self, s: Sig, base: usize) -> R {
        match s {
            Sig::Stop => {
                while self.e.len() > base {
                    let f = self.e.pop();
                    if let Some(Frame::Stopped) = f {
                        self.o.push(Obj::bool(true));
                        return Ok(());
                    }
                    if let Some(Frame::File(fl)) = f {
                        let _ = fl;
                    }
                }
                Err(Sig::Stop)
            }
            Sig::Exit => {
                while self.e.len() > base {
                    let is_loop = self.e.last().is_some_and(Frame::is_loop);
                    let stopped = matches!(self.e.last(), Some(Frame::Stopped));
                    if stopped {
                        return self.raise(E::InvalidExit).or_else(|s| self.unwind(s, base));
                    }
                    self.e.pop();
                    if is_loop {
                        return Ok(());
                    }
                }
                Err(Sig::Exit)
            }
            Sig::Quit => {
                self.e.truncate(base);
                Err(Sig::Quit)
            }
            Sig::Err(e) => self.raise(e),
        }
    }

    /// Runs `proc` (or any executable object) to its end, here, in a
    /// nested run: what an operator calling back into PostScript uses
    /// (image data procedures, `BuildChar`, `kshow`).
    pub fn call(&mut self, proc: Obj) -> R {
        self.e.push(Frame::Barrier);
        let base = self.e.len();
        let r = self.exec_value(proc).and_then(|()| self.run(base));
        let r = match r {
            Err(Sig::Err(e)) => self.raise(e).and_then(|()| self.run(base)),
            r => r,
        };
        // (pop to the barrier, inclusive)
        while self.e.len() >= base {
            self.e.pop();
        }
        r
    }

    fn step(&mut self, base: usize) -> R {
        let len = self.e.len();
        let top = self.e.last_mut().ok_or(Sig::Quit)?;
        match top {
            Frame::Proc(a, i) => {
                if *i >= a.len {
                    self.e.pop();
                    return Ok(());
                }
                let o = a.get(*i);
                *i += 1;
                if *i >= a.len {
                    self.e.pop();
                }
                self.exec_direct(o)
            }
            Frame::File(f) => {
                let f = f.clone();
                let t = {
                    let mut fb = f.borrow_mut();
                    crate::scan::token(&mut fb)
                };
                match t {
                    Ok(None) => {
                        self.e.pop();
                        Ok(())
                    }
                    Ok(Some(Tok::Dsc(line))) => {
                        crate::device::dsc_comment(self, &line);
                        Ok(())
                    }
                    Ok(Some(t)) => {
                        self.cur_files.push(f.clone());
                        let o = self.tok_obj(t, &f);
                        self.cur_files.pop();
                        let o = o?;
                        self.cur_files.push(f);
                        let r = self.exec_direct(o);
                        self.cur_files.pop();
                        r
                    }
                    Err(ScanErr::Syntax) => {
                        let s = self.str_obj(b"--syntax--");
                        self.o.push(s);
                        Err(E::SyntaxError.into())
                    }
                }
            }
            Frame::Stopped => {
                self.e.pop();
                self.o.push(Obj::bool(false));
                Ok(())
            }
            Frame::Barrier => {
                let _ = (len, base);
                Ok(())
            }
            Frame::EndDict => {
                self.e.pop();
                if self.d.len() > 3 {
                    self.d.pop();
                }
                Ok(())
            }
            Frame::For {
                i,
                inc,
                lim,
                int,
                proc,
            } => {
                let done = if *inc > 0.0 { *i > *lim } else { *i < *lim };
                if done {
                    self.e.pop();
                    return Ok(());
                }
                let v = *i;
                let isint = *int;
                let p = proc.clone();
                if isint {
                    *i += *inc;
                } else {
                    // (the control variable as a float, accumulated in float)
                    *i = f64::from((v as f32) + (*inc as f32));
                }
                if isint {
                    self.o.push(Obj::int(v as i64));
                } else {
                    self.o.push(Obj::real(v as f32));
                }
                self.exec_value(p)
            }
            Frame::Repeat { n, proc } => {
                if *n <= 0 {
                    self.e.pop();
                    return Ok(());
                }
                *n -= 1;
                let p = proc.clone();
                self.exec_value(p)
            }
            Frame::Loop { proc } => {
                let p = proc.clone();
                self.exec_value(p)
            }
            Frame::ForallArr { a, i, proc } => {
                if *i >= a.len {
                    self.e.pop();
                    return Ok(());
                }
                let o = a.get(*i);
                *i += 1;
                let p = proc.clone();
                self.o.push(o);
                self.exec_value(p)
            }
            Frame::ForallStr { s, i, proc } => {
                if *i >= s.len {
                    self.e.pop();
                    return Ok(());
                }
                let c = s.get(*i);
                *i += 1;
                let p = proc.clone();
                self.o.push(Obj::int(i64::from(c)));
                self.exec_value(p)
            }
            Frame::ForallDict { d, i, proc } => {
                let entry = {
                    let dd = d.data.borrow();
                    if *i >= dd.len() {
                        None
                    } else {
                        Some((dd.kobjs[*i].clone(), dd.vals[*i].clone()))
                    }
                };
                let Some((k, v)) = entry else {
                    self.e.pop();
                    return Ok(());
                };
                *i += 1;
                let p = proc.clone();
                self.o.push(k);
                self.o.push(v);
                self.exec_value(p)
            }
            Frame::Each { items, i, procs } => {
                if *i >= items.len() {
                    self.e.pop();
                    return Ok(());
                }
                let it = core::mem::take(&mut items[*i]);
                *i += 1;
                let k = it.len();
                let p = procs
                    .get(k.saturating_sub(1))
                    .cloned()
                    .or_else(|| procs.first().cloned());
                // (pathforall: the procedure by the number of operands,
                // the list's last element its index)
                let mut it = it;
                let pi = if procs.len() > 1 {
                    match it.pop().map(|o| o.v) {
                        Some(V::Int(n)) => n as usize,
                        _ => 0,
                    }
                } else {
                    0
                };
                for o in it {
                    self.o.push(o);
                }
                let p = if procs.len() > 1 {
                    procs[pi].clone()
                } else {
                    p.unwrap_or(Obj::NULL)
                };
                self.exec_value(p)
            }
        }
    }

    /// A token as an object; a procedure is read whole.
    fn tok_obj(&mut self, t: Tok, f: &PFile) -> R<Obj> {
        Ok(match t {
            Tok::Int(i) => Obj::int(i),
            Tok::Real(r) => Obj::real(r),
            Tok::Str(s) => Obj::str(self.vm.new_str(s)),
            Tok::LitName(n) => Obj::name(self.names.intern(&n)),
            Tok::Name(n) => Obj::xname(self.names.intern(&n)),
            Tok::ImmName(n) => {
                let n = self.names.intern(&n);
                match self.lookup(n) {
                    Some(v) => v,
                    None => {
                        self.o.push(Obj::name(n));
                        return Err(E::Undefined.into());
                    }
                }
            }
            Tok::ProcStart => {
                let mut items = Vec::new();
                loop {
                    let t = {
                        let mut fb = f.borrow_mut();
                        crate::scan::token(&mut fb)
                    };
                    match t {
                        Ok(Some(Tok::ProcEnd)) => break,
                        Ok(Some(Tok::Dsc(_))) => {}
                        Ok(Some(t)) => {
                            let o = self.tok_obj(t, f)?;
                            items.push(o);
                        }
                        Ok(None) | Err(_) => {
                            let s = self.str_obj(b"--proc--");
                            self.o.push(s);
                            return Err(E::SyntaxError.into());
                        }
                    }
                }
                let mut a = self.vm.new_arr(items);
                a.packed = self.packing;
                Obj::arr(a).exec()
            }
            Tok::Dsc(_) => Obj::NULL,
            Tok::ProcEnd => {
                let s = self.str_obj(b"}");
                self.o.push(s);
                return Err(E::SyntaxError.into());
            }
        })
    }

    /// Runs a program (the EPS file) to its end; an error stops it, as
    /// Ghostscript's top level does, leaving what was drawn.
    pub fn run_program(&mut self, data: Rc<Vec<u8>>) {
        let f = Rc::new(RefCell::new(File::new(data)));
        self.e.push(Frame::File(f));
        let r = self.run(0);
        match r {
            Ok(()) | Err(Sig::Quit) => {}
            Err(_) => {
                self.e.clear();
            }
        }
    }

    /// `==`'s text of an object (for messages).
    pub fn show_obj(&self, o: &Obj) -> String {
        match &o.v {
            V::Int(i) => format!("{i}"),
            V::Real(r) => crate::num::g(f64::from(*r)),
            V::Name(n) => {
                let s = String::from_utf8_lossy(self.names.get(*n)).into_owned();
                if o.x { s } else { format!("/{s}") }
            }
            V::Op(i) => format!("--{}--", self.ops[usize::from(*i)].0),
            V::Str(s) => format!("({})", String::from_utf8_lossy(&s.bytes())),
            _ => format!("-{}-", o.type_name()),
        }
    }

    /// Registers operator `name` in `systemdict`.
    pub fn op(&mut self, name: &'static str, f: OpFn) {
        let i = self.ops.len() as u16;
        self.ops.push((name, f));
        let sd = self.systemdict.clone();
        let o = Obj {
            v: V::Op(i),
            x: true,
        };
        self.dput_s(&sd, name, o);
    }
}

// ------ operators ------

fn arith(ps: &mut Ps<'_>, fi: fn(i64, i64) -> Option<i64>, ff: fn(f32, f32) -> f32) -> R {
    ps.need(2)?;
    let (a, b) = (ps.top(1)?.clone(), ps.top(0)?.clone());
    let r = match (&a.v, &b.v) {
        (V::Int(x), V::Int(y)) => match fi(*x, *y) {
            Some(v) if i32::try_from(v).is_ok() => Obj::int(v),
            _ => Obj::real(ff(*x as f32, *y as f32)),
        },
        (V::Int(_) | V::Real(_), V::Int(_) | V::Real(_)) => {
            let x = match a.v {
                V::Int(i) => i as f32,
                V::Real(r) => r,
                _ => 0.0,
            };
            let y = match b.v {
                V::Int(i) => i as f32,
                V::Real(r) => r,
                _ => 0.0,
            };
            Obj::real(ff(x, y))
        }
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    ps.o.push(r);
    Ok(())
}

fn op_add(ps: &mut Ps<'_>) -> R {
    arith(ps, i64::checked_add, |a, b| a + b)
}
fn op_sub(ps: &mut Ps<'_>) -> R {
    arith(ps, i64::checked_sub, |a, b| a - b)
}
fn op_mul(ps: &mut Ps<'_>) -> R {
    arith(ps, i64::checked_mul, |a, b| a * b)
}
fn op_div(ps: &mut Ps<'_>) -> R {
    let b = ps.num_at(0)?;
    let a = ps.num_at(1)?;
    if b == 0.0 {
        return Err(E::UndefinedResult.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.o.push(Obj::real(a as f32 / b as f32));
    Ok(())
}
fn op_idiv(ps: &mut Ps<'_>) -> R {
    let b = ps.int_at(0)?;
    let a = ps.int_at(1)?;
    if b == 0 {
        return Err(E::UndefinedResult.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.pushi(a.wrapping_div(b));
    Ok(())
}
fn op_mod(ps: &mut Ps<'_>) -> R {
    let b = ps.int_at(0)?;
    let a = ps.int_at(1)?;
    if b == 0 {
        return Err(E::UndefinedResult.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.pushi(a.wrapping_rem(b));
    Ok(())
}
fn unary(ps: &mut Ps<'_>, fi: fn(i64) -> Option<i64>, ff: fn(f32) -> f32) -> R {
    let o = ps.top(0)?.clone();
    let r = match o.v {
        V::Int(i) => match fi(i) {
            Some(v) if i32::try_from(v).is_ok() => Obj::int(v),
            _ => Obj::real(ff(i as f32)),
        },
        V::Real(r) => Obj::real(ff(r)),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.pop();
    ps.o.push(r);
    Ok(())
}
fn op_abs(ps: &mut Ps<'_>) -> R {
    unary(ps, i64::checked_abs, f32::abs)
}
fn op_neg(ps: &mut Ps<'_>) -> R {
    unary(ps, i64::checked_neg, |x| -x)
}
fn op_ceiling(ps: &mut Ps<'_>) -> R {
    unary(ps, Some, libm::ceilf)
}
fn op_floor(ps: &mut Ps<'_>) -> R {
    unary(ps, Some, libm::floorf)
}
fn op_round(ps: &mut Ps<'_>) -> R {
    unary(ps, Some, |x| libm::floorf(x + 0.5))
}
fn op_truncate(ps: &mut Ps<'_>) -> R {
    unary(ps, Some, libm::truncf)
}
fn real1(ps: &mut Ps<'_>, f: fn(f64) -> Option<f64>) -> R {
    let x = ps.num_at(0)?;
    let r = f(x).ok_or(Sig::Err(E::RangeCheck))?;
    ps.o.pop();
    ps.pushr(r);
    Ok(())
}
fn op_sqrt(ps: &mut Ps<'_>) -> R {
    real1(ps, |x| (x >= 0.0).then(|| libm::sqrt(x)))
}
fn op_sin(ps: &mut Ps<'_>) -> R {
    real1(ps, |x| Some(libm::sin(x * core::f64::consts::PI / 180.0)))
}
fn op_cos(ps: &mut Ps<'_>) -> R {
    real1(ps, |x| Some(libm::cos(x * core::f64::consts::PI / 180.0)))
}
fn op_ln(ps: &mut Ps<'_>) -> R {
    real1(ps, |x| (x > 0.0).then(|| libm::log(x)))
}
fn op_log(ps: &mut Ps<'_>) -> R {
    real1(ps, |x| (x > 0.0).then(|| libm::log10(x)))
}
fn op_atan(ps: &mut Ps<'_>) -> R {
    let den = ps.num_at(0)?;
    let num = ps.num_at(1)?;
    if num == 0.0 && den == 0.0 {
        return Err(E::UndefinedResult.into());
    }
    let mut a = libm::atan2(num, den) * 180.0 / core::f64::consts::PI;
    if a < 0.0 {
        a += 360.0;
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.pushr(a);
    Ok(())
}
fn op_exp(ps: &mut Ps<'_>) -> R {
    let e = ps.num_at(0)?;
    let b = ps.num_at(1)?;
    if b < 0.0 && libm::trunc(e) != e {
        return Err(E::UndefinedResult.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.pushr(libm::pow(b, e));
    Ok(())
}
fn op_rand(ps: &mut Ps<'_>) -> R {
    // (gs's: a Park-Miller generator, zmath.c)
    let a: i64 = 16807;
    let m: i64 = 0x7fff_ffff;
    let q = m / a;
    let r = m % a;
    let mut s = i64::from(ps.rand);
    s = a * (s % q) - r * (s / q);
    if s <= 0 {
        s += m;
    }
    ps.rand = s as u32;
    ps.pushi(s);
    Ok(())
}
fn op_srand(ps: &mut Ps<'_>) -> R {
    let s = ps.pop_int()?;
    let mut s = s & 0x7fff_ffff;
    if s == 0 {
        s = 1;
    } else if s == 0x7fff_ffff {
        s = 0x7fff_fffe;
    }
    ps.rand = s as u32;
    Ok(())
}
fn op_rrand(ps: &mut Ps<'_>) -> R {
    let r = i64::from(ps.rand);
    ps.pushi(r);
    Ok(())
}

// stack

fn op_pop(ps: &mut Ps<'_>) -> R {
    ps.pop().map(|_| ())
}
fn op_exch(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    let n = ps.o.len();
    ps.o.swap(n - 1, n - 2);
    Ok(())
}
fn op_dup(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    ps.o.push(o);
    Ok(())
}
fn op_copy(ps: &mut Ps<'_>) -> R {
    let t = ps.top(0)?.clone();
    match &t.v {
        V::Int(n) => {
            let n = *n;
            if n < 0 {
                return Err(E::RangeCheck.into());
            }
            let n = n as usize;
            ps.need(n + 1)?;
            ps.o.pop();
            let l = ps.o.len();
            for i in l - n..l {
                let o = ps.o[i].clone();
                ps.o.push(o);
            }
            Ok(())
        }
        V::Arr(dst) => {
            let src = match &ps.top(1)?.v {
                V::Arr(a) => a.clone(),
                _ => return Err(E::TypeCheck.into()),
            };
            if src.len > dst.len {
                return Err(E::RangeCheck.into());
            }
            let items = src.to_vec();
            ps.vm.note_arr(&dst.b);
            {
                let mut d = dst.b.data.borrow_mut();
                for (k, o) in items.into_iter().enumerate() {
                    d[dst.off + k] = o;
                }
            }
            let r = Obj {
                v: V::Arr(dst.sub(0, src.len)),
                x: t.x,
            };
            ps.o.truncate(ps.o.len() - 2);
            ps.o.push(r);
            Ok(())
        }
        V::Str(dst) => {
            let src = match &ps.top(1)?.v {
                V::Str(s) => s.clone(),
                _ => return Err(E::TypeCheck.into()),
            };
            if src.len > dst.len {
                return Err(E::RangeCheck.into());
            }
            let b = src.bytes();
            ps.vm.note_str(&dst.b);
            dst.b.data.borrow_mut()[dst.off..dst.off + b.len()].copy_from_slice(&b);
            let r = Obj {
                v: V::Str(dst.sub(0, src.len)),
                x: t.x,
            };
            ps.o.truncate(ps.o.len() - 2);
            ps.o.push(r);
            Ok(())
        }
        V::Dict(dst) => {
            let src = match &ps.top(1)?.v {
                V::Dict(d) => d.clone(),
                _ => return Err(E::TypeCheck.into()),
            };
            let dst = dst.clone();
            let entries: Vec<(Obj, Obj)> = {
                let s = src.data.borrow();
                s.kobjs
                    .iter()
                    .cloned()
                    .zip(s.vals.iter().cloned())
                    .collect()
            };
            for (k, v) in entries {
                ps.dput(&dst, k, v)?;
            }
            ps.o.truncate(ps.o.len() - 2);
            ps.o.push(Obj::dict(dst));
            Ok(())
        }
        V::GState(g) => {
            let g = g.clone();
            let src = match &ps.top(1)?.v {
                V::GState(s) => s.clone(),
                _ => return Err(E::TypeCheck.into()),
            };
            let v = src.borrow().clone();
            *g.borrow_mut() = v;
            ps.o.truncate(ps.o.len() - 2);
            ps.o.push(Obj {
                v: V::GState(g),
                x: false,
            });
            Ok(())
        }
        _ => Err(E::TypeCheck.into()),
    }
}
fn op_index(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    let n = n as usize;
    ps.need(n + 2)?;
    ps.o.pop();
    let o = ps.o[ps.o.len() - 1 - n].clone();
    ps.o.push(o);
    Ok(())
}
fn op_roll(ps: &mut Ps<'_>) -> R {
    let j = ps.int_at(0)?;
    let n = ps.int_at(1)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    let n = n as usize;
    ps.need(n + 2)?;
    ps.o.truncate(ps.o.len() - 2);
    if n == 0 {
        return Ok(());
    }
    let l = ps.o.len();
    let s = &mut ps.o[l - n..];
    let j = j.rem_euclid(n as i64) as usize;
    s.rotate_right(j);
    Ok(())
}
fn op_clear(ps: &mut Ps<'_>) -> R {
    ps.o.clear();
    Ok(())
}
fn op_count(ps: &mut Ps<'_>) -> R {
    let n = ps.o.len() as i64;
    ps.pushi(n);
    Ok(())
}
fn op_mark(ps: &mut Ps<'_>) -> R {
    ps.o.push(Obj::mark());
    Ok(())
}
fn mark_pos(ps: &Ps<'_>) -> R<usize> {
    ps.o.iter()
        .rposition(|o| matches!(o.v, V::Mark))
        .ok_or(Sig::Err(E::UnmatchedMark))
}
fn op_cleartomark(ps: &mut Ps<'_>) -> R {
    let p = mark_pos(ps)?;
    ps.o.truncate(p);
    Ok(())
}
fn op_counttomark(ps: &mut Ps<'_>) -> R {
    let p = mark_pos(ps)?;
    let n = (ps.o.len() - p - 1) as i64;
    ps.pushi(n);
    Ok(())
}

// arrays

fn op_array(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    let a = ps.arr_obj(vec![Obj::NULL; n as usize]);
    ps.o.push(a);
    Ok(())
}
fn op_endarray(ps: &mut Ps<'_>) -> R {
    let p = mark_pos(ps)?;
    let items = ps.o.split_off(p + 1);
    ps.o.pop();
    let a = ps.arr_obj(items);
    ps.o.push(a);
    Ok(())
}
fn op_length(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let n = match &o.v {
        V::Arr(a) => a.len,
        V::Str(s) => s.len,
        V::Dict(d) => d.data.borrow().len(),
        V::Name(n) => ps.names.get(*n).len(),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.pop();
    ps.pushi(n as i64);
    Ok(())
}
fn op_get(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let c = ps.top(1)?.clone();
    let r = match &c.v {
        V::Arr(a) => {
            let V::Int(i) = k.v else {
                return Err(E::TypeCheck.into());
            };
            if i < 0 || i as usize >= a.len {
                return Err(E::RangeCheck.into());
            }
            a.get(i as usize)
        }
        V::Str(s) => {
            let V::Int(i) = k.v else {
                return Err(E::TypeCheck.into());
            };
            if i < 0 || i as usize >= s.len {
                return Err(E::RangeCheck.into());
            }
            Obj::int(i64::from(s.get(i as usize)))
        }
        V::Dict(d) => match ps.dget(d, &k)? {
            Some(v) => v,
            None => return Err(E::Undefined.into()),
        },
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    ps.o.push(r);
    Ok(())
}
fn op_put(ps: &mut Ps<'_>) -> R {
    ps.need(3)?;
    let v = ps.top(0)?.clone();
    let k = ps.top(1)?.clone();
    let c = ps.top(2)?.clone();
    match &c.v {
        V::Arr(a) => {
            let V::Int(i) = k.v else {
                return Err(E::TypeCheck.into());
            };
            if i < 0 || i as usize >= a.len {
                return Err(E::RangeCheck.into());
            }
            ps.vm.note_arr(&a.b);
            a.b.data.borrow_mut()[a.off + i as usize] = v;
        }
        V::Str(s) => {
            let V::Int(i) = k.v else {
                return Err(E::TypeCheck.into());
            };
            let V::Int(b) = v.v else {
                return Err(E::TypeCheck.into());
            };
            if i < 0 || i as usize >= s.len {
                return Err(E::RangeCheck.into());
            }
            ps.vm.note_str(&s.b);
            s.b.data.borrow_mut()[s.off + i as usize] = b as u8;
        }
        V::Dict(d) => ps.dput(d, k, v)?,
        _ => return Err(E::TypeCheck.into()),
    }
    ps.o.truncate(ps.o.len() - 3);
    Ok(())
}
fn op_getinterval(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    let i = ps.int_at(1)?;
    let c = ps.top(2)?.clone();
    if i < 0 || n < 0 {
        return Err(E::RangeCheck.into());
    }
    let (i, n) = (i as usize, n as usize);
    let r = match &c.v {
        V::Arr(a) => {
            if i + n > a.len {
                return Err(E::RangeCheck.into());
            }
            Obj {
                v: V::Arr(a.sub(i, n)),
                x: c.x,
            }
        }
        V::Str(s) => {
            if i + n > s.len {
                return Err(E::RangeCheck.into());
            }
            Obj {
                v: V::Str(s.sub(i, n)),
                x: c.x,
            }
        }
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 3);
    ps.o.push(r);
    Ok(())
}
fn op_putinterval(ps: &mut Ps<'_>) -> R {
    let src = ps.top(0)?.clone();
    let i = ps.int_at(1)?;
    let c = ps.top(2)?.clone();
    if i < 0 {
        return Err(E::RangeCheck.into());
    }
    let i = i as usize;
    match (&c.v, &src.v) {
        (V::Arr(a), V::Arr(s)) => {
            if i + s.len > a.len {
                return Err(E::RangeCheck.into());
            }
            let items = s.to_vec();
            ps.vm.note_arr(&a.b);
            let mut d = a.b.data.borrow_mut();
            for (k, o) in items.into_iter().enumerate() {
                d[a.off + i + k] = o;
            }
        }
        (V::Str(a), V::Str(s)) => {
            if i + s.len > a.len {
                return Err(E::RangeCheck.into());
            }
            let b = s.bytes();
            ps.vm.note_str(&a.b);
            a.b.data.borrow_mut()[a.off + i..a.off + i + b.len()].copy_from_slice(&b);
        }
        _ => return Err(E::TypeCheck.into()),
    }
    ps.o.truncate(ps.o.len() - 3);
    Ok(())
}
fn op_aload(ps: &mut Ps<'_>) -> R {
    let a = ps.pop_arr()?;
    let o = Obj::arr(a.clone());
    for x in a.to_vec() {
        ps.o.push(x);
    }
    ps.o.push(o);
    Ok(())
}
fn op_astore(ps: &mut Ps<'_>) -> R {
    let a = match &ps.top(0)?.v {
        V::Arr(a) => a.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.need(a.len + 1)?;
    let o = ps.o.pop().unwrap_or(Obj::NULL);
    let items = ps.o.split_off(ps.o.len() - a.len);
    ps.vm.note_arr(&a.b);
    {
        let mut d = a.b.data.borrow_mut();
        for (k, x) in items.into_iter().enumerate() {
            d[a.off + k] = x;
        }
    }
    ps.o.push(o);
    Ok(())
}
fn op_packedarray(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    let n = n as usize;
    ps.need(n + 1)?;
    ps.o.pop();
    let items = ps.o.split_off(ps.o.len() - n);
    let mut a = ps.vm.new_arr(items);
    a.packed = true;
    let b = a.b.clone();
    b.acc.set(crate::obj::ACC_READONLY);
    ps.o.push(Obj::arr(a));
    Ok(())
}
fn op_setpacking(ps: &mut Ps<'_>) -> R {
    ps.packing = ps.pop_bool()?;
    Ok(())
}
fn op_currentpacking(ps: &mut Ps<'_>) -> R {
    let p = ps.packing;
    ps.pushb(p);
    Ok(())
}
fn op_forall(ps: &mut Ps<'_>) -> R {
    let p = ps.top(0)?.clone();
    let c = ps.top(1)?.clone();
    let f = match &c.v {
        V::Arr(a) => Frame::ForallArr {
            a: a.clone(),
            i: 0,
            proc: p,
        },
        V::Str(s) => Frame::ForallStr {
            s: s.clone(),
            i: 0,
            proc: p,
        },
        V::Dict(d) => Frame::ForallDict {
            d: d.clone(),
            i: 0,
            proc: p,
        },
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    ps.e.push(f);
    Ok(())
}

// dictionaries

fn op_dict(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    let d = ps.new_dict(n as usize);
    ps.o.push(Obj::dict(d));
    Ok(())
}
fn op_dictbegin(ps: &mut Ps<'_>) -> R {
    ps.o.push(Obj::mark());
    Ok(())
}
fn op_dictend(ps: &mut Ps<'_>) -> R {
    let p = mark_pos(ps)?;
    let items = ps.o.split_off(p + 1);
    if !items.len().is_multiple_of(2) {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    let d = ps.new_dict(items.len() / 2);
    let mut it = items.into_iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        ps.dput(&d, k, v)?;
    }
    ps.o.push(Obj::dict(d));
    Ok(())
}
fn op_maxlength(ps: &mut Ps<'_>) -> R {
    let d = ps.pop_dict()?;
    let n = d.data.borrow().maxlen.max(d.data.borrow().len()) as i64;
    ps.pushi(n);
    Ok(())
}
fn op_begin(ps: &mut Ps<'_>) -> R {
    let d = ps.pop_dict()?;
    ps.d.push(d);
    Ok(())
}
fn op_end(ps: &mut Ps<'_>) -> R {
    if ps.d.len() <= 3 {
        return Err(E::DictStackUnderflow.into());
    }
    ps.d.pop();
    Ok(())
}
fn op_def(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    let v = ps.top(0)?.clone();
    let k = ps.top(1)?.clone();
    let d =
        ps.d.last()
            .cloned()
            .ok_or(Sig::Err(E::DictStackUnderflow))?;
    ps.dput(&d, k, v)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_load(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let key = ps.key(&k)?;
    for d in ps.d.iter().rev() {
        if let Some(v) = d.data.borrow().get(&key) {
            let v = v.clone();
            ps.o.pop();
            ps.o.push(v);
            return Ok(());
        }
    }
    Err(E::Undefined.into())
}
fn op_store(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    let v = ps.top(0)?.clone();
    let k = ps.top(1)?.clone();
    let key = ps.key(&k)?;
    let mut target =
        ps.d.last()
            .cloned()
            .ok_or(Sig::Err(E::DictStackUnderflow))?;
    for d in ps.d.iter().rev() {
        if d.data.borrow().get(&key).is_some() {
            target = d.clone();
            break;
        }
    }
    ps.dput(&target, k, v)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_undef(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let d = match &ps.top(1)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let key = ps.key(&k)?;
    ps.vm.note_dict(&d);
    d.data.borrow_mut().remove(&key);
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_known(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let d = match &ps.top(1)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let key = ps.key(&k)?;
    let r = d.data.borrow().get(&key).is_some();
    ps.o.truncate(ps.o.len() - 2);
    ps.pushb(r);
    Ok(())
}
fn op_where(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let key = ps.key(&k)?;
    let found =
        ps.d.iter()
            .rev()
            .find(|d| d.data.borrow().get(&key).is_some())
            .cloned();
    ps.o.pop();
    match found {
        Some(d) => {
            ps.o.push(Obj::dict(d));
            ps.pushb(true);
        }
        None => ps.pushb(false),
    }
    Ok(())
}
fn op_currentdict(ps: &mut Ps<'_>) -> R {
    let d =
        ps.d.last()
            .cloned()
            .ok_or(Sig::Err(E::DictStackUnderflow))?;
    ps.o.push(Obj::dict(d));
    Ok(())
}
fn op_countdictstack(ps: &mut Ps<'_>) -> R {
    let n = ps.d.len() as i64;
    ps.pushi(n);
    Ok(())
}
fn op_dictstack(ps: &mut Ps<'_>) -> R {
    let a = ps.pop_arr()?;
    if a.len < ps.d.len() {
        return Err(E::RangeCheck.into());
    }
    let ds: Vec<Obj> = ps.d.iter().map(|d| Obj::dict(d.clone())).collect();
    let n = ds.len();
    {
        let mut d = a.b.data.borrow_mut();
        for (k, o) in ds.into_iter().enumerate() {
            d[a.off + k] = o;
        }
    }
    ps.o.push(Obj::arr(a.sub(0, n)));
    Ok(())
}
fn op_cleardictstack(ps: &mut Ps<'_>) -> R {
    ps.d.truncate(3);
    Ok(())
}

// strings

fn op_string(ps: &mut Ps<'_>) -> R {
    let n = ps.int_at(0)?;
    if !(0..=65535 * 16).contains(&n) {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    let s = ps.new_str(vec![0; n as usize]);
    ps.o.push(Obj::str(s));
    Ok(())
}
fn op_anchorsearch(ps: &mut Ps<'_>) -> R {
    let seek = ps.top(0)?.clone();
    let s = ps.top(1)?.clone();
    let (V::Str(s), V::Str(k)) = (&s.v, &seek.v) else {
        return Err(E::TypeCheck.into());
    };
    let (sb, kb) = (s.bytes(), k.bytes());
    ps.o.truncate(ps.o.len() - 2);
    if sb.starts_with(&kb) {
        ps.o.push(Obj::str(s.sub(kb.len(), sb.len() - kb.len())));
        ps.o.push(Obj::str(s.sub(0, kb.len())));
        ps.pushb(true);
    } else {
        ps.o.push(Obj::str(s.clone()));
        ps.pushb(false);
    }
    Ok(())
}
fn op_search(ps: &mut Ps<'_>) -> R {
    let seek = ps.top(0)?.clone();
    let s = ps.top(1)?.clone();
    let (V::Str(s), V::Str(k)) = (&s.v, &seek.v) else {
        return Err(E::TypeCheck.into());
    };
    let (sb, kb) = (s.bytes(), k.bytes());
    ps.o.truncate(ps.o.len() - 2);
    let pos = if kb.is_empty() {
        Some(0)
    } else {
        sb.windows(kb.len()).position(|w| w == kb.as_slice())
    };
    match pos {
        Some(p) => {
            ps.o.push(Obj::str(s.sub(p + kb.len(), sb.len() - p - kb.len())));
            ps.o.push(Obj::str(s.sub(p, kb.len())));
            ps.o.push(Obj::str(s.sub(0, p)));
            ps.pushb(true);
        }
        None => {
            ps.o.push(Obj::str(s.clone()));
            ps.pushb(false);
        }
    }
    Ok(())
}
fn op_token(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    match &o.v {
        V::Str(s) => {
            let f = Rc::new(RefCell::new(File::new(Rc::new(s.bytes()))));
            let t = crate::scan::token(&mut f.borrow_mut());
            ps.o.pop();
            match t {
                Ok(None) => ps.pushb(false),
                Ok(Some(t)) => {
                    let obj = ps.tok_obj(t, &f)?;
                    let pos = f.borrow().pos;
                    ps.o.push(Obj::str(s.sub(pos, s.len - pos)));
                    ps.o.push(obj);
                    ps.pushb(true);
                }
                Err(_) => return Err(E::SyntaxError.into()),
            }
            Ok(())
        }
        V::File(f) => {
            let f = f.clone();
            let t = crate::scan::token(&mut f.borrow_mut());
            ps.o.pop();
            match t {
                Ok(None) => ps.pushb(false),
                Ok(Some(t)) => {
                    let obj = ps.tok_obj(t, &f)?;
                    ps.o.push(obj);
                    ps.pushb(true);
                }
                Err(_) => return Err(E::SyntaxError.into()),
            }
            Ok(())
        }
        _ => Err(E::TypeCheck.into()),
    }
}

// relational and boolean

fn op_eq(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    let b = ps.pop()?;
    let a = ps.pop()?;
    let r = a.eq_ps(&b, &ps.names);
    ps.pushb(r);
    Ok(())
}
fn op_ne(ps: &mut Ps<'_>) -> R {
    op_eq(ps)?;
    let b = ps.pop_bool()?;
    ps.pushb(!b);
    Ok(())
}
fn cmp(ps: &mut Ps<'_>) -> R<core::cmp::Ordering> {
    ps.need(2)?;
    let (a, b) = (ps.top(1)?.clone(), ps.top(0)?.clone());
    let r = match (&a.v, &b.v) {
        (V::Int(x), V::Int(y)) => x.cmp(y),
        (V::Str(x), V::Str(y)) => x.bytes().cmp(&y.bytes()),
        _ => {
            let (x, y) = (
                a.num().ok_or(Sig::Err(E::TypeCheck))?,
                b.num().ok_or(Sig::Err(E::TypeCheck))?,
            );
            let (x, y) = (x as f32, y as f32);
            x.partial_cmp(&y).unwrap_or(core::cmp::Ordering::Equal)
        }
    };
    ps.o.truncate(ps.o.len() - 2);
    Ok(r)
}
fn op_ge(ps: &mut Ps<'_>) -> R {
    let c = cmp(ps)?;
    ps.pushb(c != core::cmp::Ordering::Less);
    Ok(())
}
fn op_gt(ps: &mut Ps<'_>) -> R {
    let c = cmp(ps)?;
    ps.pushb(c == core::cmp::Ordering::Greater);
    Ok(())
}
fn op_le(ps: &mut Ps<'_>) -> R {
    let c = cmp(ps)?;
    ps.pushb(c != core::cmp::Ordering::Greater);
    Ok(())
}
fn op_lt(ps: &mut Ps<'_>) -> R {
    let c = cmp(ps)?;
    ps.pushb(c == core::cmp::Ordering::Less);
    Ok(())
}
fn logic(ps: &mut Ps<'_>, fb: fn(bool, bool) -> bool, fi: fn(i64, i64) -> i64) -> R {
    ps.need(2)?;
    let (a, b) = (ps.top(1)?.clone(), ps.top(0)?.clone());
    let r = match (&a.v, &b.v) {
        (V::Bool(x), V::Bool(y)) => Obj::bool(fb(*x, *y)),
        (V::Int(x), V::Int(y)) => Obj::int(fi(*x, *y)),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    ps.o.push(r);
    Ok(())
}
fn op_and(ps: &mut Ps<'_>) -> R {
    logic(ps, |a, b| a && b, |a, b| a & b)
}
fn op_or(ps: &mut Ps<'_>) -> R {
    logic(ps, |a, b| a || b, |a, b| a | b)
}
fn op_xor(ps: &mut Ps<'_>) -> R {
    logic(ps, |a, b| a ^ b, |a, b| a ^ b)
}
fn op_not(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let r = match o.v {
        V::Bool(b) => Obj::bool(!b),
        V::Int(i) => Obj::int(i64::from(!(i as i32))),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.pop();
    ps.o.push(r);
    Ok(())
}
fn op_bitshift(ps: &mut Ps<'_>) -> R {
    let s = ps.int_at(0)?;
    let i = ps.int_at(1)? as i32 as u32;
    ps.o.truncate(ps.o.len() - 2);
    let r = if s >= 32 || s <= -32 {
        0
    } else if s >= 0 {
        i << s
    } else {
        i >> (-s)
    };
    ps.pushi(i64::from(r as i32));
    Ok(())
}
fn op_true(ps: &mut Ps<'_>) -> R {
    ps.pushb(true);
    Ok(())
}
fn op_false(ps: &mut Ps<'_>) -> R {
    ps.pushb(false);
    Ok(())
}
fn op_null(ps: &mut Ps<'_>) -> R {
    ps.o.push(Obj::NULL);
    Ok(())
}

// control

fn op_exec(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    ps.exec_value(o)
}
fn op_if(ps: &mut Ps<'_>) -> R {
    let p = ps.top(0)?.clone();
    let V::Bool(b) = ps.top(1)?.v else {
        return Err(E::TypeCheck.into());
    };
    if !matches!(p.v, V::Arr(_)) {
        return Err(E::TypeCheck.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    if b { ps.exec_value(p) } else { Ok(()) }
}
fn op_ifelse(ps: &mut Ps<'_>) -> R {
    let p2 = ps.top(0)?.clone();
    let p1 = ps.top(1)?.clone();
    let V::Bool(b) = ps.top(2)?.v else {
        return Err(E::TypeCheck.into());
    };
    if !matches!(p1.v, V::Arr(_)) || !matches!(p2.v, V::Arr(_)) {
        return Err(E::TypeCheck.into());
    }
    ps.o.truncate(ps.o.len() - 3);
    ps.exec_value(if b { p1 } else { p2 })
}
fn op_for(ps: &mut Ps<'_>) -> R {
    ps.need(4)?;
    let p = ps.top(0)?.clone();
    let (a, b, c) = (ps.top(3)?.clone(), ps.top(2)?.clone(), ps.top(1)?.clone());
    let int = matches!((&a.v, &b.v), (V::Int(_), V::Int(_)));
    let (i, inc, lim) = (
        a.num().ok_or(Sig::Err(E::TypeCheck))?,
        b.num().ok_or(Sig::Err(E::TypeCheck))?,
        c.num().ok_or(Sig::Err(E::TypeCheck))?,
    );
    ps.o.truncate(ps.o.len() - 4);
    let (i, inc, lim) = if int {
        (i, inc, lim)
    } else {
        (
            f64::from(i as f32),
            f64::from(inc as f32),
            f64::from(lim as f32),
        )
    };
    ps.e.push(Frame::For {
        i,
        inc,
        lim,
        int,
        proc: p,
    });
    Ok(())
}
fn op_repeat(ps: &mut Ps<'_>) -> R {
    let p = ps.top(0)?.clone();
    let n = ps.int_at(1)?;
    if n < 0 {
        return Err(E::RangeCheck.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.e.push(Frame::Repeat { n, proc: p });
    Ok(())
}
fn op_loop(ps: &mut Ps<'_>) -> R {
    let p = ps.pop()?;
    ps.e.push(Frame::Loop { proc: p });
    Ok(())
}
fn op_exit(_: &mut Ps<'_>) -> R {
    Err(Sig::Exit)
}
fn op_stop(_: &mut Ps<'_>) -> R {
    Err(Sig::Stop)
}
fn op_stopped(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    ps.e.push(Frame::Stopped);
    ps.exec_value(o)
}
fn op_countexecstack(ps: &mut Ps<'_>) -> R {
    let n = ps.e.len() as i64;
    ps.pushi(n);
    Ok(())
}
fn op_execstack(ps: &mut Ps<'_>) -> R {
    let a = ps.pop_arr()?;
    let n = a.len.min(ps.e.len());
    ps.o.push(Obj::arr(a.sub(0, n)));
    Ok(())
}
fn op_quit(_: &mut Ps<'_>) -> R {
    Err(Sig::Quit)
}

// types

fn op_type(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    let t = o.type_name();
    let n = ps.name(t);
    ps.o.push(Obj::xname(n));
    Ok(())
}
fn op_cvlit(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    ps.o.push(o.lit());
    Ok(())
}
fn op_cvx(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    ps.o.push(o.exec());
    Ok(())
}
fn op_xcheck(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    ps.pushb(o.x);
    Ok(())
}
fn set_access(ps: &mut Ps<'_>, a: u8) -> R {
    let o = ps.top(0)?.clone();
    match &o.v {
        V::Arr(x) => x.b.acc.set(x.b.acc.get().min(a)),
        V::Str(x) => x.b.acc.set(x.b.acc.get().min(a)),
        V::Dict(x) => x.acc.set(x.acc.get().min(a)),
        V::File(_) => {}
        _ => return Err(E::TypeCheck.into()),
    }
    Ok(())
}
fn op_executeonly(ps: &mut Ps<'_>) -> R {
    set_access(ps, crate::obj::ACC_EXECONLY)
}
fn op_noaccess(ps: &mut Ps<'_>) -> R {
    set_access(ps, crate::obj::ACC_NONE)
}
fn op_readonly(ps: &mut Ps<'_>) -> R {
    set_access(ps, crate::obj::ACC_READONLY)
}
fn op_rcheck(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    let a = match &o.v {
        V::Arr(x) => x.b.acc.get(),
        V::Str(x) => x.b.acc.get(),
        V::Dict(x) => x.acc.get(),
        _ => 3,
    };
    ps.pushb(a >= crate::obj::ACC_READONLY);
    Ok(())
}
fn op_wcheck(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    let a = match &o.v {
        V::Arr(x) => x.b.acc.get(),
        V::Str(x) => x.b.acc.get(),
        V::Dict(x) => x.acc.get(),
        _ => 3,
    };
    ps.pushb(a >= crate::obj::ACC_UNLIMITED);
    Ok(())
}
fn op_cvi(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let v = match &o.v {
        V::Int(i) => *i,
        V::Real(r) => {
            if !(-2_147_483_649.0..2_147_483_648.0).contains(r) {
                return Err(E::RangeCheck.into());
            }
            *r as i64
        }
        V::Str(s) => {
            let b = s.bytes();
            match crate::scan::number(b.trim_ascii()) {
                Some(Tok::Int(i)) => i,
                Some(Tok::Real(r)) => r as i64,
                _ => return Err(E::SyntaxError.into()),
            }
        }
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.pop();
    ps.pushi(v);
    Ok(())
}
fn op_cvr(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let v = match &o.v {
        V::Int(i) => *i as f32,
        V::Real(r) => *r,
        V::Str(s) => {
            let b = s.bytes();
            match crate::scan::number(b.trim_ascii()) {
                Some(Tok::Int(i)) => i as f32,
                Some(Tok::Real(r)) => r,
                _ => return Err(E::SyntaxError.into()),
            }
        }
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.pop();
    ps.o.push(Obj::real(v));
    Ok(())
}
fn op_cvn(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let V::Str(s) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    let n = ps.names.intern(&s.bytes());
    ps.o.pop();
    ps.o.push(Obj {
        v: V::Name(n),
        x: o.x,
    });
    Ok(())
}
/// `cvs`'s text of an object.
pub fn cvs_text(ps: &Ps<'_>, o: &Obj) -> Vec<u8> {
    match &o.v {
        V::Int(i) => format!("{i}").into_bytes(),
        V::Real(r) => real_text(*r).into_bytes(),
        V::Bool(b) => {
            if *b {
                b"true".to_vec()
            } else {
                b"false".to_vec()
            }
        }
        V::Name(n) => ps.names.get(*n).to_vec(),
        V::Str(s) => s.bytes(),
        V::Op(i) => ps.ops[usize::from(*i)].0.as_bytes().to_vec(),
        _ => b"--nostringval--".to_vec(),
    }
}
/// A real as gs prints it (`%g` with precision 6, a `.0` added where it
/// looks like an integer).
pub fn real_text(r: f32) -> String {
    let s = crate::num::g(f64::from(r));
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("nan") {
        s
    } else {
        format!("{s}.0")
    }
}
fn op_cvs(ps: &mut Ps<'_>) -> R {
    let s = ps.top(0)?.clone();
    let o = ps.top(1)?.clone();
    let V::Str(s) = &s.v else {
        return Err(E::TypeCheck.into());
    };
    let t = cvs_text(ps, &o);
    if t.len() > s.len {
        return Err(E::RangeCheck.into());
    }
    ps.vm.note_str(&s.b);
    s.b.data.borrow_mut()[s.off..s.off + t.len()].copy_from_slice(&t);
    ps.o.truncate(ps.o.len() - 2);
    ps.o.push(Obj::str(s.sub(0, t.len())));
    Ok(())
}
fn op_cvrs(ps: &mut Ps<'_>) -> R {
    let s = ps.top(0)?.clone();
    let radix = ps.int_at(1)?;
    let n = ps.top(2)?.clone();
    let V::Str(s) = &s.v else {
        return Err(E::TypeCheck.into());
    };
    if !(2..=36).contains(&radix) {
        return Err(E::RangeCheck.into());
    }
    let t: Vec<u8> = if radix == 10 {
        cvs_text(ps, &n)
    } else {
        let v = match n.v {
            V::Int(i) => i as i32 as u32,
            V::Real(r) => r as i32 as u32,
            _ => return Err(E::TypeCheck.into()),
        };
        let mut d = Vec::new();
        let mut v = v;
        loop {
            let x = (v % radix as u32) as u8;
            d.push(if x < 10 { b'0' + x } else { b'A' + x - 10 });
            v /= radix as u32;
            if v == 0 {
                break;
            }
        }
        d.reverse();
        d
    };
    if t.len() > s.len {
        return Err(E::RangeCheck.into());
    }
    ps.vm.note_str(&s.b);
    s.b.data.borrow_mut()[s.off..s.off + t.len()].copy_from_slice(&t);
    ps.o.truncate(ps.o.len() - 3);
    ps.o.push(Obj::str(s.sub(0, t.len())));
    Ok(())
}

// files

fn op_currentfile(ps: &mut Ps<'_>) -> R {
    // (the innermost file being executed)
    let f = ps.e.iter().rev().find_map(|f| match f {
        Frame::File(f) => Some(f.clone()),
        _ => None,
    });
    let f = f.unwrap_or_else(|| Rc::new(RefCell::new(File::new(Rc::new(Vec::new())))));
    ps.o.push(Obj {
        v: V::File(f),
        x: false,
    });
    Ok(())
}
fn op_file(ps: &mut Ps<'_>) -> R {
    let _mode = ps.pop_str()?;
    let name = ps.pop_str()?;
    let n = name.bytes();
    let f = if n.starts_with(b"%stdout")
        || n.starts_with(b"%stderr")
        || _mode.bytes().starts_with(b"w")
    {
        File::writer()
    } else if n.starts_with(b"%stdin") {
        File::new(Rc::new(Vec::new()))
    } else {
        return Err(E::UndefinedFilename.into());
    };
    ps.o.push(Obj {
        v: V::File(Rc::new(RefCell::new(f))),
        x: false,
    });
    Ok(())
}
fn pop_file(ps: &mut Ps<'_>) -> R<PFile> {
    match &ps.top(0)?.v {
        V::File(f) => {
            let f = f.clone();
            ps.o.pop();
            Ok(f)
        }
        _ => Err(E::TypeCheck.into()),
    }
}
fn op_closefile(ps: &mut Ps<'_>) -> R {
    let f = pop_file(ps)?;
    f.borrow_mut().closed = true;
    Ok(())
}
fn op_read(ps: &mut Ps<'_>) -> R {
    let f = pop_file(ps)?;
    let c = f.borrow_mut().next();
    match c {
        Some(c) => {
            ps.pushi(i64::from(c));
            ps.pushb(true);
        }
        None => ps.pushb(false),
    }
    Ok(())
}
fn op_write(ps: &mut Ps<'_>) -> R {
    ps.pop_int()?;
    pop_file(ps)?;
    Ok(())
}
fn op_writestring(ps: &mut Ps<'_>) -> R {
    ps.pop_str()?;
    pop_file(ps)?;
    Ok(())
}
fn op_readstring(ps: &mut Ps<'_>) -> R {
    let s = match &ps.top(0)?.v {
        V::Str(s) => s.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let f = match &ps.top(1)?.v {
        V::File(f) => f.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    let mut fb = f.borrow_mut();
    let rest = fb.rest();
    let n = rest.len().min(s.len);
    let data = rest[..n].to_vec();
    fb.pos += n;
    drop(fb);
    ps.vm.note_str(&s.b);
    s.b.data.borrow_mut()[s.off..s.off + n].copy_from_slice(&data);
    ps.o.push(Obj::str(s.sub(0, n)));
    ps.pushb(n == s.len);
    Ok(())
}
fn op_readhexstring(ps: &mut Ps<'_>) -> R {
    let s = match &ps.top(0)?.v {
        V::Str(s) => s.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let f = match &ps.top(1)?.v {
        V::File(f) => f.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    let mut out = Vec::with_capacity(s.len);
    let mut hi: Option<u8> = None;
    {
        let mut fb = f.borrow_mut();
        while out.len() < s.len {
            let Some(c) = fb.next() else { break };
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => continue,
            };
            match hi.take() {
                None => hi = Some(d),
                Some(h) => out.push(h << 4 | d),
            }
        }
    }
    let n = out.len();
    ps.vm.note_str(&s.b);
    s.b.data.borrow_mut()[s.off..s.off + n].copy_from_slice(&out);
    ps.o.push(Obj::str(s.sub(0, n)));
    ps.pushb(n == s.len);
    Ok(())
}
fn op_readline(ps: &mut Ps<'_>) -> R {
    let s = match &ps.top(0)?.v {
        V::Str(s) => s.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let f = match &ps.top(1)?.v {
        V::File(f) => f.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    let mut out = Vec::new();
    let mut eol = false;
    {
        let mut fb = f.borrow_mut();
        while let Some(c) = fb.next() {
            if c == b'\n' {
                eol = true;
                break;
            }
            if c == b'\r' {
                if fb.peek() == Some(b'\n') {
                    fb.next();
                }
                eol = true;
                break;
            }
            out.push(c);
        }
    }
    if out.len() > s.len {
        return Err(E::RangeCheck.into());
    }
    let n = out.len();
    ps.vm.note_str(&s.b);
    s.b.data.borrow_mut()[s.off..s.off + n].copy_from_slice(&out);
    ps.o.push(Obj::str(s.sub(0, n)));
    ps.pushb(eol);
    Ok(())
}
fn op_bytesavailable(ps: &mut Ps<'_>) -> R {
    let f = pop_file(ps)?;
    let n = f.borrow().bytes_available();
    ps.pushi(n);
    Ok(())
}
fn op_flushfile(ps: &mut Ps<'_>) -> R {
    let f = pop_file(ps)?;
    // (an input file: to its end)
    let mut fb = f.borrow_mut();
    if !fb.write {
        fb.pos = fb.data.len();
    }
    Ok(())
}
fn op_resetfile(ps: &mut Ps<'_>) -> R {
    pop_file(ps).map(|_| ())
}
fn op_status(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    match &o.v {
        V::File(f) => {
            let c = f.borrow().closed;
            ps.pushb(!c);
        }
        V::Str(_) => ps.pushb(false),
        _ => return Err(E::TypeCheck.into()),
    }
    Ok(())
}
fn op_fileposition(ps: &mut Ps<'_>) -> R {
    let f = pop_file(ps)?;
    let p = f.borrow().pos as i64;
    ps.pushi(p);
    Ok(())
}
fn op_setfileposition(ps: &mut Ps<'_>) -> R {
    let p = ps.pop_int()?;
    let f = pop_file(ps)?;
    f.borrow_mut().pos = p.max(0) as usize;
    Ok(())
}
fn op_filter(ps: &mut Ps<'_>) -> R {
    use crate::file::{Filter, FilterParams};
    let n = match &ps.top(0)?.v {
        V::Name(n) => *n,
        _ => return Err(E::TypeCheck.into()),
    };
    let name = ps.names.get(n).to_vec();
    let f = match name.as_slice() {
        b"ASCIIHexDecode" => Filter::AsciiHex,
        b"ASCII85Decode" => Filter::Ascii85,
        b"RunLengthDecode" => Filter::RunLength,
        b"LZWDecode" => Filter::Lzw,
        b"FlateDecode" => Filter::Flate,
        b"DCTDecode" => Filter::Dct,
        b"SubFileDecode" => Filter::SubFile,
        b"NullEncode" | b"ASCIIHexEncode" | b"ASCII85Encode" | b"RunLengthEncode"
        | b"LZWEncode" | b"FlateEncode" | b"DCTEncode" => {
            // (an encoding filter writes somewhere unseen)
            ps.o.pop();
            let p = ps.pop()?;
            if let V::Dict(_) = p.v {
                ps.pop()?;
            }
            ps.o.push(Obj {
                v: V::File(Rc::new(RefCell::new(File::writer()))),
                x: false,
            });
            return Ok(());
        }
        _ => return Err(E::UndefinedResource.into()),
    };
    ps.o.pop();
    let mut p = FilterParams::default();
    // (parameters: SubFileDecode's count and string, a dictionary)
    if f == Filter::SubFile {
        let s = ps.pop()?;
        let c = ps.pop()?;
        match (&c.v, &s.v) {
            (V::Int(c), V::Str(s)) => {
                p.eod_count = *c;
                p.eod_string = s.bytes();
            }
            (V::Dict(_), _) | (_, V::Dict(_)) => {}
            _ => return Err(E::TypeCheck.into()),
        }
    }
    if let V::Dict(d) = &ps.top(0)?.v {
        let d = d.clone();
        ps.o.pop();
        let get = |ps: &mut Ps<'_>, k: &str| ps.dget_s(&d, k).and_then(|o| o.num());
        if let Some(v) = get(ps, "Predictor") {
            p.predictor = v as i64;
        }
        if let Some(v) = get(ps, "Colors") {
            p.colors = v as i64;
        }
        if let Some(v) = get(ps, "BitsPerComponent") {
            p.bpc = v as i64;
        }
        if let Some(v) = get(ps, "Columns") {
            p.columns = v as i64;
        }
        if let Some(v) = get(ps, "EarlyChange") {
            p.early_change = v != 0.0;
        }
        if let Some(v) = get(ps, "EODCount") {
            p.eod_count = v as i64;
        }
        if let Some(V::Str(s)) = ps.dget_s(&d, "EODString").map(|o| o.v) {
            p.eod_string = s.bytes();
        }
    }
    let src = ps.pop()?;
    let (data, dct) = match &src.v {
        V::File(sf) => {
            let mut fb = sf.borrow_mut();
            let (out, used) = crate::file::decode(f, fb.rest(), &p);
            fb.pos += used;
            let dct = (f == Filter::Dct).then(|| Rc::new(out.clone()));
            (
                out,
                dct.or_else(|| {
                    fb.dct
                        .clone()
                        .filter(|_| f == Filter::SubFile || f == Filter::Null)
                }),
            )
        }
        V::Str(s) => {
            let b = s.bytes();
            let (out, _) = crate::file::decode(f, &b, &p);
            let dct = (f == Filter::Dct).then(|| Rc::new(out.clone()));
            (out, dct)
        }
        V::Arr(_) => {
            // (a procedure source: called until it returns an empty string)
            let mut all = Vec::new();
            for _ in 0..1_000_000 {
                ps.call(src.clone())?;
                let s = ps.pop_str()?;
                if s.len == 0 {
                    break;
                }
                all.extend_from_slice(&s.bytes());
            }
            let (out, _) = crate::file::decode(f, &all, &p);
            let dct = (f == Filter::Dct).then(|| Rc::new(out.clone()));
            (out, dct)
        }
        _ => return Err(E::TypeCheck.into()),
    };
    let mut nf = File::new(Rc::new(data));
    nf.dct = dct;
    ps.o.push(Obj {
        v: V::File(Rc::new(RefCell::new(nf))),
        x: false,
    });
    Ok(())
}
fn op_print(ps: &mut Ps<'_>) -> R {
    ps.pop_str().map(|_| ())
}
fn op_pop1(ps: &mut Ps<'_>) -> R {
    ps.pop().map(|_| ())
}
fn op_nop(_: &mut Ps<'_>) -> R {
    Ok(())
}
fn op_run(ps: &mut Ps<'_>) -> R {
    ps.pop_str()?;
    Err(E::UndefinedFilename.into())
}
fn op_deletefile(ps: &mut Ps<'_>) -> R {
    ps.pop_str()?;
    Err(E::UndefinedFilename.into())
}

// virtual memory

fn op_save(ps: &mut Ps<'_>) -> R {
    let level = ps.vm.save();
    ps.save_gdepth.push((level, ps.gstack.len()));
    // (save does a gsave)
    let g = ps.gs.clone();
    ps.gstack.push(g);
    ps.o.push(Obj {
        v: V::Save(level),
        x: false,
    });
    Ok(())
}
fn op_restore(ps: &mut Ps<'_>) -> R {
    let V::Save(level) = ps.top(0)?.v else {
        return Err(E::TypeCheck.into());
    };
    if level > ps.vm.level || level == 0 {
        return Err(E::InvalidRestore.into());
    }
    ps.o.pop();
    // (the gstate saved by the save, gstates since dropped)
    if let Some(p) = ps.save_gdepth.iter().position(|&(l, _)| l == level) {
        let depth = ps.save_gdepth[p].1;
        ps.save_gdepth.truncate(p);
        if depth < ps.gstack.len() {
            let g = ps.gstack[depth].clone();
            ps.gstack.truncate(depth);
            crate::gstate::set_gstate(ps, g);
        }
    }
    ps.vm.restore(level);
    Ok(())
}
fn op_setglobal(ps: &mut Ps<'_>) -> R {
    ps.vm.global = ps.pop_bool()?;
    Ok(())
}
fn op_currentglobal(ps: &mut Ps<'_>) -> R {
    let g = ps.vm.global;
    ps.pushb(g);
    Ok(())
}
fn op_gcheck(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    let g = match &o.v {
        V::Arr(a) => a.b.global,
        V::Str(s) => s.b.global,
        V::Dict(d) => d.global,
        _ => true,
    };
    ps.pushb(g);
    Ok(())
}
fn op_vmstatus(ps: &mut Ps<'_>) -> R {
    let l = i64::from(ps.vm.level);
    ps.pushi(l);
    ps.pushi(1_000_000);
    ps.pushi(100_000_000);
    Ok(())
}

// misc

fn op_bind(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    if let V::Arr(a) = &o.v {
        bind(ps, a, 0);
    }
    Ok(())
}
fn bind(ps: &mut Ps<'_>, a: &PArr, depth: usize) {
    if depth > 50 {
        return;
    }
    for i in 0..a.len {
        let o = a.get(i);
        match &o.v {
            V::Name(n) if o.x => {
                if let Some(v) = ps.lookup(*n)
                    && let V::Op(_) = v.v
                    && v.x
                {
                    if a.b.acc.get() >= crate::obj::ACC_UNLIMITED || a.packed {
                        a.b.data.borrow_mut()[a.off + i] = v;
                    }
                }
            }
            V::Arr(inner) if o.x => {
                let inner = inner.clone();
                bind(ps, &inner, depth + 1);
                if inner.b.acc.get() >= crate::obj::ACC_UNLIMITED {
                    inner.b.acc.set(crate::obj::ACC_READONLY);
                }
            }
            _ => {}
        }
    }
}
fn op_version(ps: &mut Ps<'_>) -> R {
    let s = ps.str_obj(b"3010");
    ps.o.push(s);
    Ok(())
}
fn op_product(ps: &mut Ps<'_>) -> R {
    let s = ps.str_obj(b"GPL Ghostscript");
    ps.o.push(s);
    Ok(())
}
fn op_revision(ps: &mut Ps<'_>) -> R {
    ps.pushi(10080);
    Ok(())
}
fn op_realtime(ps: &mut Ps<'_>) -> R {
    ps.pushi(0);
    Ok(())
}
fn op_languagelevel(ps: &mut Ps<'_>) -> R {
    ps.pushi(3);
    Ok(())
}
fn op_serialnumber(ps: &mut Ps<'_>) -> R {
    ps.pushi(42);
    Ok(())
}
fn op_errorhandler(ps: &mut Ps<'_>) -> R {
    // (the default handler for whichever error: `$error` and stop; the
    // error's name is not known here, so `unregistered`)
    let _ = ps;
    Err(Sig::Stop)
}
fn op_handleerror(_: &mut Ps<'_>) -> R {
    Ok(())
}

/// The language's operators.
pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, OpFn)] = &[
        ("add", op_add),
        ("sub", op_sub),
        ("mul", op_mul),
        ("div", op_div),
        ("idiv", op_idiv),
        ("mod", op_mod),
        ("abs", op_abs),
        ("neg", op_neg),
        ("ceiling", op_ceiling),
        ("floor", op_floor),
        ("round", op_round),
        ("truncate", op_truncate),
        ("sqrt", op_sqrt),
        ("sin", op_sin),
        ("cos", op_cos),
        ("atan", op_atan),
        ("exp", op_exp),
        ("ln", op_ln),
        ("log", op_log),
        ("rand", op_rand),
        ("srand", op_srand),
        ("rrand", op_rrand),
        ("pop", op_pop),
        ("exch", op_exch),
        ("dup", op_dup),
        ("copy", op_copy),
        ("index", op_index),
        ("roll", op_roll),
        ("clear", op_clear),
        ("count", op_count),
        ("mark", op_mark),
        ("[", op_mark),
        ("<<", op_dictbegin),
        (">>", op_dictend),
        ("]", op_endarray),
        ("cleartomark", op_cleartomark),
        ("counttomark", op_counttomark),
        ("array", op_array),
        ("length", op_length),
        ("get", op_get),
        ("put", op_put),
        ("getinterval", op_getinterval),
        ("putinterval", op_putinterval),
        ("aload", op_aload),
        ("astore", op_astore),
        ("packedarray", op_packedarray),
        ("setpacking", op_setpacking),
        ("currentpacking", op_currentpacking),
        ("forall", op_forall),
        ("dict", op_dict),
        ("maxlength", op_maxlength),
        ("begin", op_begin),
        ("end", op_end),
        ("def", op_def),
        ("load", op_load),
        ("store", op_store),
        ("undef", op_undef),
        ("known", op_known),
        ("where", op_where),
        ("currentdict", op_currentdict),
        ("countdictstack", op_countdictstack),
        ("dictstack", op_dictstack),
        ("cleardictstack", op_cleardictstack),
        ("string", op_string),
        ("anchorsearch", op_anchorsearch),
        ("search", op_search),
        ("token", op_token),
        ("eq", op_eq),
        ("ne", op_ne),
        ("ge", op_ge),
        ("gt", op_gt),
        ("le", op_le),
        ("lt", op_lt),
        ("and", op_and),
        ("or", op_or),
        ("xor", op_xor),
        ("not", op_not),
        ("bitshift", op_bitshift),
        ("true", op_true),
        ("false", op_false),
        ("null", op_null),
        ("exec", op_exec),
        ("if", op_if),
        ("ifelse", op_ifelse),
        ("for", op_for),
        ("repeat", op_repeat),
        ("loop", op_loop),
        ("exit", op_exit),
        ("stop", op_stop),
        ("stopped", op_stopped),
        ("countexecstack", op_countexecstack),
        ("execstack", op_execstack),
        ("quit", op_quit),
        ("type", op_type),
        ("cvlit", op_cvlit),
        ("cvx", op_cvx),
        ("xcheck", op_xcheck),
        ("executeonly", op_executeonly),
        ("noaccess", op_noaccess),
        ("readonly", op_readonly),
        ("rcheck", op_rcheck),
        ("wcheck", op_wcheck),
        ("cvi", op_cvi),
        ("cvr", op_cvr),
        ("cvn", op_cvn),
        ("cvs", op_cvs),
        ("cvrs", op_cvrs),
        ("currentfile", op_currentfile),
        ("file", op_file),
        ("closefile", op_closefile),
        ("read", op_read),
        ("write", op_write),
        ("writestring", op_writestring),
        ("writehexstring", op_writestring),
        ("readstring", op_readstring),
        ("readhexstring", op_readhexstring),
        ("readline", op_readline),
        ("bytesavailable", op_bytesavailable),
        ("flush", op_nop),
        ("flushfile", op_flushfile),
        ("resetfile", op_resetfile),
        ("status", op_status),
        ("fileposition", op_fileposition),
        ("setfileposition", op_setfileposition),
        ("filter", op_filter),
        ("print", op_print),
        ("=", op_pop1),
        ("==", op_pop1),
        ("=only", op_pop1),
        ("==only", op_pop1),
        ("stack", op_nop),
        ("pstack", op_nop),
        ("run", op_run),
        ("deletefile", op_deletefile),
        ("save", op_save),
        ("restore", op_restore),
        ("setglobal", op_setglobal),
        ("currentglobal", op_currentglobal),
        ("gcheck", op_gcheck),
        ("vmstatus", op_vmstatus),
        ("vmreclaim", op_pop1),
        ("bind", op_bind),
        ("version", op_version),
        ("product", op_product),
        ("revision", op_revision),
        ("realtime", op_realtime),
        ("usertime", op_realtime),
        ("languagelevel", op_languagelevel),
        ("serialnumber", op_serialnumber),
        ("handleerror", op_handleerror),
        (".errorhandler", op_errorhandler),
        ("echo", op_pop1),
        ("executive", op_nop),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}

/// A closure kept for its type (unused).
pub type Thunk = Box<dyn FnOnce()>;
