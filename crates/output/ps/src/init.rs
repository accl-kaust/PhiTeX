//! The initial VM (gs_init.ps and friends as far as EPS files reach):
//! the operators of every module, `errordict`, `$error`, `statusdict`,
//! the font directories, the standard encodings, the resource
//! categories (`findresource`, `defineresource`, ...), the parameter
//! operators, and running the input file (gs's `run`, `.runEPS`).

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::file::File;
use crate::interp::{E, Frame, OpFn, Ps, R, Sig};
use crate::obj::{Obj, PDict, V};

macro_rules! error_handlers {
    ($($f:ident = $e:ident),* $(,)?) => {
        $(fn $f(ps: &mut Ps<'_>) -> R { ps.default_error(E::$e) })*
        const HANDLERS: &[(E, OpFn)] = &[$((E::$e, $f)),*];
    };
}

error_handlers! {
    eh_dictfull = DictFull,
    eh_dictstackoverflow = DictStackOverflow,
    eh_dictstackunderflow = DictStackUnderflow,
    eh_execstackoverflow = ExecStackOverflow,
    eh_invalidaccess = InvalidAccess,
    eh_invalidexit = InvalidExit,
    eh_invalidfileaccess = InvalidFileAccess,
    eh_invalidfont = InvalidFont,
    eh_invalidrestore = InvalidRestore,
    eh_ioerror = IoError,
    eh_limitcheck = LimitCheck,
    eh_nocurrentpoint = NoCurrentPoint,
    eh_rangecheck = RangeCheck,
    eh_stackoverflow = StackOverflow,
    eh_stackunderflow = StackUnderflow,
    eh_syntaxerror = SyntaxError,
    eh_timeout = Timeout,
    eh_typecheck = TypeCheck,
    eh_undefined = Undefined,
    eh_undefinedfilename = UndefinedFilename,
    eh_undefinedresource = UndefinedResource,
    eh_undefinedresult = UndefinedResult,
    eh_unmatchedmark = UnmatchedMark,
    eh_unregistered = Unregistered,
    eh_vmerror = VmError,
    eh_configurationerror = ConfigurationError,
}

/// The resource categories gs defines.
const CATEGORIES: &[&str] = &[
    "Font",
    "Encoding",
    "Form",
    "Pattern",
    "ProcSet",
    "ColorSpace",
    "Halftone",
    "ColorRendering",
    "Filter",
    "ColorSpaceFamily",
    "Emulator",
    "IODevice",
    "ColorRenderingType",
    "FMapType",
    "FontType",
    "FormType",
    "HalftoneType",
    "ImageType",
    "PatternType",
    "ShadingType",
    "Shading",
    "Category",
    "Generic",
    "CMap",
    "CIDFont",
    "CIDSystemInfo",
    "InkParams",
    "TrapParams",
    "OutputDevice",
    "ControlLanguage",
    "Localization",
    "PDL",
    "HWOptions",
];

pub fn init(ps: &mut Ps<'_>) {
    crate::interp::register(ps);
    crate::gstate::register(ps);
    crate::color::register(ps);
    crate::paint::register(ps);
    crate::font::register(ps);
    crate::device::register(ps);
    crate::text::register(ps);
    crate::image::register(ps);
    crate::pattern::register(ps);
    crate::upath::register(ps);
    register(ps);
    let sd = ps.systemdict.clone();
    // (the dictionaries)
    let ud = Obj::dict(ps.userdict.clone());
    ps.dput_s(&sd, "userdict", ud);
    let sdo = Obj::dict(sd.clone());
    ps.dput_s(&sd, "systemdict", sdo);
    let gd = Obj::dict(ps.globaldict.clone());
    ps.dput_s(&sd, "globaldict", gd);
    let ed = Obj::dict(ps.errordict.clone());
    ps.dput_s(&sd, "errordict", ed);
    let de = Obj::dict(ps.dollar_error.clone());
    ps.dput_s(&sd, "$error", de);
    let st = Obj::dict(ps.statusdict.clone());
    ps.dput_s(&sd, "statusdict", st);
    let fd = Obj::dict(ps.fontdir.clone());
    ps.dput_s(&sd, "FontDirectory", fd.clone());
    ps.dput_s(&sd, "SharedFontDirectory", fd.clone());
    ps.dput_s(&sd, "GlobalFontDirectory", fd);
    // (errordict)
    let errd = ps.errordict.clone();
    for &(e, f) in HANDLERS {
        let i = ps.ops.len() as u16;
        ps.ops.push((e.name(), f));
        ps.dput_s(
            &errd,
            e.name(),
            Obj {
                v: V::Op(i),
                x: true,
            },
        );
    }
    let hn = ps.lookup_s("handleerror").unwrap_or(Obj::NULL);
    ps.dput_s(&errd, "handleerror", hn);
    let de = ps.dollar_error.clone();
    ps.dput_s(&de, "newerror", Obj::bool(false));
    ps.dput_s(&de, "recordstacks", Obj::bool(false));
    ps.dput_s(&de, "binary", Obj::bool(false));
    // (statusdict)
    let st = ps.statusdict.clone();
    ps.dput_s(&st, "product", Obj::NULL);
    let pn = ps.str_obj(b"GPL Ghostscript");
    ps.dput_s(&st, "product", pn);
    for (k, v) in [
        ("waittimeout", 0),
        ("jobtimeout", 0),
        ("manualfeedtimeout", 60),
        ("margins", 0),
    ] {
        ps.dput_s(&st, k, Obj::int(v));
    }
    for k in [
        "setpapertray",
        "settumble",
        "setduplexmode",
        "setmanualfeed",
        "setresolution",
        "setjobtimeout",
        "setwaittimeout",
        "setdefaulttimeouts",
        "setpagetype",
        "setdostartpage",
        "setprintername",
        "setpapersize",
    ] {
        let o = ps.lookup_s("pop").unwrap_or(Obj::NULL);
        ps.dput_s(&st, k, o);
    }
    for k in [
        "letter",
        "legal",
        "a4",
        "a3",
        "a5",
        "b5",
        "note",
        "lettertray",
        "legaltray",
        "a4tray",
        "11x17",
        "ledger",
    ] {
        let p = ps.arr_obj(Vec::new()).exec();
        ps.dput_s(&st, k, p);
    }
    let f = Obj::bool(false);
    ps.dput_s(&st, "duplexmode", f.clone());
    ps.dput_s(&st, "manualfeed", f);
    let ll = Obj::int(3);
    ps.dput_s(&st, "languagelevel", ll);
    // (encodings)
    let std = encoding(ps, &crate::enc::STANDARD_ENCODING);
    ps.dput_s(&sd, "StandardEncoding", std);
    let iso = encoding(ps, &crate::enc::ISO_LATIN1_ENCODING);
    ps.dput_s(&sd, "ISOLatin1Encoding", iso);
    // (resource categories)
    let rd = ps.resources.clone();
    for c in CATEGORIES {
        let d = ps.new_dict(20);
        ps.dput_s(&rd, c, Obj::dict(d));
    }
    for (n, e) in [
        ("StandardEncoding", &crate::enc::STANDARD_ENCODING),
        ("ISOLatin1Encoding", &crate::enc::ISO_LATIN1_ENCODING),
        ("SymbolEncoding", &crate::enc::SYMBOL_ENCODING),
        ("DingbatsEncoding", &crate::enc::DINGBATS_ENCODING),
        ("WinAnsiEncoding", &crate::enc::WIN_ANSI_ENCODING),
        ("MacRomanEncoding", &crate::enc::MAC_ROMAN_ENCODING),
        ("MacExpertEncoding", &crate::enc::MAC_EXPERT_ENCODING),
    ] {
        let a = if n == "StandardEncoding" || n == "ISOLatin1Encoding" {
            ps.dget_s(&sd, n).unwrap_or(Obj::NULL)
        } else {
            encoding(ps, e)
        };
        let cat = category(ps, "Encoding");
        ps.dput_s(&cat, n, a);
    }
    for t in [1, 2, 3, 6, 7] {
        let cat = category(ps, "ShadingType");
        let _ = ps.dput(&cat, Obj::int(t), Obj::int(t));
    }
    for t in [1, 2] {
        let cat = category(ps, "PatternType");
        let _ = ps.dput(&cat, Obj::int(t), Obj::int(t));
    }
    for t in [0, 1, 2, 3, 42] {
        let cat = category(ps, "FontType");
        let _ = ps.dput(&cat, Obj::int(t), Obj::int(t));
    }
    for t in [
        "DeviceGray",
        "DeviceRGB",
        "DeviceCMYK",
        "Indexed",
        "Separation",
        "DeviceN",
        "Pattern",
        "CIEBasedA",
        "CIEBasedABC",
        "CIEBasedDEF",
        "CIEBasedDEFG",
        "ICCBased",
    ] {
        let cat = category(ps, "ColorSpaceFamily");
        let n = ps.nobj(t);
        let _ = ps.dput(&cat, n.clone(), n);
    }
    for t in [
        "ASCIIHexDecode",
        "ASCII85Decode",
        "RunLengthDecode",
        "FlateDecode",
        "LZWDecode",
        "DCTDecode",
        "SubFileDecode",
        "NullEncode",
        "ASCIIHexEncode",
        "ASCII85Encode",
    ] {
        let cat = category(ps, "Filter");
        let n = ps.nobj(t);
        let _ = ps.dput(&cat, n.clone(), n);
    }
    // (userdict's own)
    let ud = ps.userdict.clone();
    let ue = ps.new_dict(10);
    ps.dput_s(&ud, "UserObjects", Obj::NULL);
    ps.dput_s(&ud, "serverdict", Obj::dict(ue));
}

/// An encoding as a read-only array of names.
fn encoding(ps: &mut Ps<'_>, e: &[&str]) -> Obj {
    let v: Vec<Obj> = e.iter().map(|n| ps.nobj(n)).collect();
    ps.arr_obj(v)
}

fn category(ps: &mut Ps<'_>, c: &str) -> PDict {
    let rd = ps.resources.clone();
    match ps.dget_s(&rd, c).map(|o| o.v) {
        Some(V::Dict(d)) => d,
        _ => {
            let d = ps.new_dict(20);
            ps.dput_s(&rd, c, Obj::dict(d.clone()));
            d
        }
    }
}

fn cat_of(ps: &mut Ps<'_>, o: &Obj) -> R<(PDict, Vec<u8>)> {
    let n = match &o.v {
        V::Name(n) => ps.names.get(*n).to_vec(),
        V::Str(s) => s.bytes(),
        _ => return Err(E::TypeCheck.into()),
    };
    let rd = ps.resources.clone();
    let c = ps.dget(&rd, o)?;
    match c.map(|o| o.v) {
        Some(V::Dict(d)) => Ok((d, n)),
        _ => Err(E::Undefined.into()),
    }
}

impl Ps<'_> {
    /// Looks up a resource instance (Font through `findfont`).
    pub fn find_resource(&mut self, key: &Obj, cat: &Obj) -> R<Option<Obj>> {
        let (d, name) = cat_of(self, cat)?;
        if let Some(v) = self.dget(&d, key)? {
            return Ok(Some(v));
        }
        if name == b"Font" {
            return self.findfont(key).map(Some);
        }
        if name == b"Encoding" {
            return Ok(None);
        }
        if name == b"ProcSet" {
            let k = match &key.v {
                V::Name(n) => self.names.get(*n).to_vec(),
                _ => Vec::new(),
            };
            if k == b"CIDInit" {
                let p = self.new_dict(10);
                return Ok(Some(Obj::dict(p)));
            }
        }
        Ok(None)
    }

    /// Runs the input as gs's `run` (the D line: `eps` an EPSF file, run
    /// as `.runEPS` does) or `-` (stdin).
    pub fn run_file(&mut self, data: Rc<Vec<u8>>, eps: bool) {
        let mut file = File::new(data);
        file.dsc = true;
        let f = Rc::new(RefCell::new(file));
        if !eps {
            self.eps_state = 5;
            self.e.push(Frame::File(f));
            self.run_top();
            return;
        }
        // (.runEPS: save, the DSC comments read, the file, a showpage if
        // it gave none, the stacks cleared, restore)
        let depth = self.o.len();
        let dcount = self.d.len();
        let level = self.vm.save();
        self.save_gdepth.push((level, self.gstack.len()));
        let g = self.gs.clone();
        self.gstack.push(g);
        self.eps_state = 0;
        self.e.push(Frame::File(f));
        let quit = self.run_top();
        if quit {
            return;
        }
        if self.showpages == 0 {
            let sp = self.lookup_s("showpage");
            if let Some(sp) = sp {
                self.e.push(Frame::Barrier);
                let b = self.e.len();
                let _ = self.exec_value(sp).and_then(|()| self.run(b));
                self.e.truncate(b - 1);
            }
        }
        self.o.truncate(depth);
        self.d.truncate(dcount.max(3));
        self.o.push(Obj {
            v: V::Save(level),
            x: false,
        });
        let ri = self.lookup_s("restore");
        if let Some(r) = ri {
            let _ = self.exec_value(r);
        }
        self.o.truncate(depth);
    }

    /// Runs the execution stack to its end; an error ends the job (gs
    /// prints it and quits, the page so far written): true if it did.
    fn run_top(&mut self) -> bool {
        match self.run(0) {
            Ok(()) => false,
            Err(Sig::Quit) => true,
            Err(_) => {
                self.e.clear();
                true
            }
        }
    }
}

// ------ operators ------

fn op_findresource(ps: &mut Ps<'_>) -> R {
    let cat = ps.top(0)?.clone();
    let key = ps.top(1)?.clone();
    match ps.find_resource(&key, &cat)? {
        Some(v) => {
            ps.o.truncate(ps.o.len() - 2);
            ps.push(v);
            Ok(())
        }
        None => Err(E::UndefinedResource.into()),
    }
}
fn op_defineresource(ps: &mut Ps<'_>) -> R {
    let cat = ps.top(0)?.clone();
    let inst = ps.top(1)?.clone();
    let key = ps.top(2)?.clone();
    let (d, name) = cat_of(ps, &cat)?;
    if name == b"Font" {
        if let V::Dict(fd) = &inst.v
            && ps.font_of(fd).is_none()
        {
            ps.push(key.clone());
            ps.push(inst.clone());
            let df = ps.lookup_s("definefont").ok_or(Sig::Err(E::Undefined))?;
            ps.call(df)?;
            ps.pop()?;
        }
        let fdir = ps.fontdir.clone();
        ps.dput(&fdir, key.clone(), inst.clone())?;
    }
    if name == b"Category" {
        let rd = ps.resources.clone();
        let nd = ps.new_dict(20);
        ps.dput(&rd, key.clone(), Obj::dict(nd))?;
    }
    ps.dput(&d, key, inst.clone())?;
    ps.o.truncate(ps.o.len() - 3);
    ps.push(inst);
    Ok(())
}
fn op_undefineresource(ps: &mut Ps<'_>) -> R {
    let cat = ps.top(0)?.clone();
    let key = ps.top(1)?.clone();
    let (d, _) = cat_of(ps, &cat)?;
    let k = ps.key(&key)?;
    ps.vm.note_dict(&d);
    d.data.borrow_mut().remove(&k);
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_resourcestatus(ps: &mut Ps<'_>) -> R {
    let cat = ps.top(0)?.clone();
    let key = ps.top(1)?.clone();
    let (d, name) = cat_of(ps, &cat)?;
    let mut found = ps.dget(&d, &key)?.is_some();
    if !found && name == b"Font" {
        let fd = ps.fontdir.clone();
        found = ps.dget(&fd, &key)?.is_some();
        if !found {
            let n = match &key.v {
                V::Name(n) => ps.names.get(*n).to_vec(),
                V::Str(s) => s.bytes(),
                _ => Vec::new(),
            };
            let file = crate::font::FONTMAP
                .iter()
                .find(|(a, _)| a.as_bytes() == n.as_slice())
                .map_or(n.clone(), |(_, f)| f.as_bytes().to_vec());
            found = ps.res.font(&file).is_some();
        }
    }
    if !found && name == b"ProcSet" {
        found = matches!(&key.v, V::Name(n) if ps.names.get(*n) == b"CIDInit");
    }
    ps.o.truncate(ps.o.len() - 2);
    if found {
        ps.pushi(1);
        ps.pushi(0);
    }
    ps.pushb(found);
    Ok(())
}
fn op_resourceforall(ps: &mut Ps<'_>) -> R {
    let cat = ps.top(0)?.clone();
    let scratch = ps.top(1)?.clone();
    let proc = ps.top(2)?.clone();
    let tmpl = ps.top(3)?.clone();
    let (d, _) = cat_of(ps, &cat)?;
    ps.o.truncate(ps.o.len() - 4);
    let V::Str(t) = &tmpl.v else {
        return Err(E::TypeCheck.into());
    };
    let t = t.bytes();
    let keys: Vec<Obj> = d.data.borrow().kobjs.clone();
    let mut items = Vec::new();
    for k in keys {
        let n = match &k.v {
            V::Name(n) => ps.names.get(*n).to_vec(),
            V::Str(s) => s.bytes(),
            _ => continue,
        };
        if !glob(&t, &n) {
            continue;
        }
        let s = match &scratch.v {
            V::Str(s) if s.len >= n.len() => {
                let sub = s.sub(0, n.len());
                ps.vm.note_str(&sub.b);
                for (i, &c) in n.iter().enumerate() {
                    sub.b.data.borrow_mut()[sub.off + i] = c;
                }
                Obj::str(sub)
            }
            _ => ps.str_obj(&n),
        };
        items.push(alloc::vec![s]);
    }
    ps.e.push(Frame::Each {
        items,
        i: 0,
        procs: alloc::vec![proc],
    });
    Ok(())
}

/// PostScript's `resourceforall` template match (`*`, `?`, `\`).
fn glob(t: &[u8], s: &[u8]) -> bool {
    match t.first() {
        None => s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| glob(&t[1..], &s[i..])),
        Some(b'?') => !s.is_empty() && glob(&t[1..], &s[1..]),
        Some(b'\\') if t.len() > 1 => s.first() == Some(&t[1]) && glob(&t[2..], &s[1..]),
        Some(&c) => s.first() == Some(&c) && glob(&t[1..], &s[1..]),
    }
}
fn op_findencoding(ps: &mut Ps<'_>) -> R {
    let k = ps.top(0)?.clone();
    let c = ps.nobj("Encoding");
    match ps.find_resource(&k, &c)? {
        Some(v) => {
            ps.o.pop();
            ps.push(v);
            Ok(())
        }
        None => Err(E::UndefinedResource.into()),
    }
}
fn op_pop_dict(ps: &mut Ps<'_>) -> R {
    ps.pop()?;
    Ok(())
}
fn op_empty_dict(ps: &mut Ps<'_>) -> R {
    let d = ps.new_dict(10);
    ps.push(Obj::dict(d));
    Ok(())
}
fn op_nop(_: &mut Ps<'_>) -> R {
    Ok(())
}
fn op_pop2(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_false(ps: &mut Ps<'_>) -> R {
    ps.pushb(false);
    Ok(())
}
fn op_zero(ps: &mut Ps<'_>) -> R {
    ps.pushi(0);
    Ok(())
}
fn op_cachestatus(ps: &mut Ps<'_>) -> R {
    for v in [0, 100_000, 0, 0, 0, 0, 0] {
        ps.pushi(v);
    }
    Ok(())
}
fn op_setcachedevice(ps: &mut Ps<'_>) -> R {
    ps.need(6)?;
    let wx = ps.num_at(5)?;
    let wy = ps.num_at(4)?;
    ps.o.truncate(ps.o.len() - 6);
    ps.gs.char_width = Some((wx, wy));
    Ok(())
}
fn op_setcachedevice2(ps: &mut Ps<'_>) -> R {
    ps.need(10)?;
    let wx = ps.num_at(9)?;
    let wy = ps.num_at(8)?;
    ps.o.truncate(ps.o.len() - 10);
    ps.gs.char_width = Some((wx, wy));
    Ok(())
}
fn op_setcharwidth(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    let wx = ps.num_at(1)?;
    let wy = ps.num_at(0)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.gs.char_width = Some((wx, wy));
    Ok(())
}
fn op_startjob(ps: &mut Ps<'_>) -> R {
    ps.need(2)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.pushb(false);
    Ok(())
}
fn op_defineuserobject(ps: &mut Ps<'_>) -> R {
    let v = ps.pop()?;
    let i = ps.pop_int()?;
    let ud = ps.userdict.clone();
    let cur = ps.dget_s(&ud, "UserObjects");
    let arr = match cur.map(|o| o.v) {
        Some(V::Arr(a)) => a,
        _ => {
            let a = ps
                .vm
                .new_arr(alloc::vec![Obj::NULL; 64.max(i as usize + 1)]);
            ps.dput_s(&ud, "UserObjects", Obj::arr(a.clone()));
            a
        }
    };
    let arr = if (i as usize) < arr.len {
        arr
    } else {
        let mut v = arr.to_vec();
        v.resize(i as usize + 1, Obj::NULL);
        let a = ps.vm.new_arr(v);
        ps.dput_s(&ud, "UserObjects", Obj::arr(a.clone()));
        a
    };
    ps.vm.note_arr(&arr.b);
    arr.b.data.borrow_mut()[arr.off + i as usize] = v;
    Ok(())
}
fn op_execuserobject(ps: &mut Ps<'_>) -> R {
    let i = ps.pop_int()?;
    let ud = ps.userdict.clone();
    let Some(V::Arr(a)) = ps.dget_s(&ud, "UserObjects").map(|o| o.v) else {
        return Err(E::Undefined.into());
    };
    if i < 0 || i as usize >= a.len {
        return Err(E::RangeCheck.into());
    }
    let o = a.get(i as usize);
    ps.exec_value(o)
}
fn op_undefineuserobject(ps: &mut Ps<'_>) -> R {
    ps.pop_int()?;
    Ok(())
}
fn op_bbox_nop(ps: &mut Ps<'_>) -> R {
    ps.need(4)?;
    ps.o.truncate(ps.o.len() - 4);
    Ok(())
}

fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, OpFn)] = &[
        ("findresource", op_findresource),
        ("defineresource", op_defineresource),
        ("undefineresource", op_undefineresource),
        ("resourcestatus", op_resourcestatus),
        ("resourceforall", op_resourceforall),
        ("findencoding", op_findencoding),
        ("setuserparams", op_pop_dict),
        ("currentuserparams", op_empty_dict),
        ("setsystemparams", op_pop_dict),
        ("currentsystemparams", op_empty_dict),
        ("setdevparams", op_pop2),
        ("currentdevparams", op_empty_dict),
        ("setucacheparams", op_nop),
        ("ucachestatus", op_cachestatus),
        ("ucache", op_nop),
        ("setcachelimit", op_pop_dict),
        ("setcacheparams", op_nop),
        ("currentcacheparams", op_nop),
        ("cachestatus", op_cachestatus),
        ("setcachedevice", op_setcachedevice),
        ("setcachedevice2", op_setcachedevice2),
        ("setcharwidth", op_setcharwidth),
        ("startjob", op_startjob),
        ("exitserver", op_pop_dict),
        ("setshared", op_pop_dict),
        ("currentshared", op_false),
        ("defineuserobject", op_defineuserobject),
        ("execuserobject", op_execuserobject),
        ("undefineuserobject", op_undefineuserobject),
        ("setbbox", op_bbox_nop),
        ("setvmthreshold", op_pop_dict),
        ("setobjectformat", op_pop_dict),
        ("currentobjectformat", op_zero),
        (".setlanguagelevel", op_pop_dict),
        ("deviceinfo", op_empty_dict),
        ("sethalftonephase", op_pop2),
        ("setaccuratescreens", op_pop_dict),
        ("currentaccuratescreens", op_false),
        ("clearinterrupt", op_nop),
        ("disableinterrupt", op_nop),
        ("enableinterrupt", op_nop),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
