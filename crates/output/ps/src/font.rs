//! Fonts (gsfont.c, zfont*.c, gs_fonts.ps): `definefont` and the font
//! dictionaries' identities, `findfont` over FontDirectory, Fontmap.GS's
//! aliases of the base 35 to Ghostscript's URW fonts (read through
//! [`crate::Resources::font`]) and gs's substitution of unknown names,
//! `scalefont`/`makefont`, `eexec`; and the Type 1 charstrings run for
//! their widths and outlines (gstype1.c).

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::file::File;
use crate::interp::{E, Frame, Ps, R, Sig};
use crate::matrix::M;
use crate::obj::{Obj, PDict, V};

/// A font as `definefont` made it (`gs_font`).
pub struct Font {
    pub id: u32,
    /// The font this one was scaled from (itself for a defined font): the
    /// PDF font resource follows it.
    pub base: u32,
    pub font_type: i32,
    /// `/FontName` (`font_name`).
    pub name: Vec<u8>,
    pub matrix: M,
    /// The base font's matrix.
    pub orig_matrix: M,
    pub bbox: [f64; 4],
    pub paint_type: i32,
    pub stroke_width: f64,
    pub dict: PDict,
    /// `nearest_encoding_index` and `encoding_index` (zbfont.c).
    pub nearest_encoding: i32,
    pub encoding_index: i32,
    /// Whether the font is one of gs's base 35 by its resource name.
    pub resource: Option<Vec<u8>>,
}

/// Fontmap.GS's names of the base 35.
pub const FONTMAP: &[(&str, &str)] = &[
    ("AvantGarde-BookOblique", "URWGothic-BookOblique"),
    ("AvantGarde-Book", "URWGothic-Book"),
    ("AvantGarde-DemiOblique", "URWGothic-DemiOblique"),
    ("AvantGarde-Demi", "URWGothic-Demi"),
    ("Bookman-DemiItalic", "URWBookman-DemiItalic"),
    ("Bookman-Demi", "URWBookman-Demi"),
    ("Bookman-LightItalic", "URWBookman-LightItalic"),
    ("Bookman-Light", "URWBookman-Light"),
    ("Courier-Bold", "NimbusMonoPS-Bold"),
    ("Courier-BoldOblique", "NimbusMonoPS-BoldItalic"),
    ("Courier", "NimbusMonoPS-Regular"),
    ("Courier-Oblique", "NimbusMonoPS-Italic"),
    ("Helvetica-Bold", "NimbusSans-Bold"),
    ("Helvetica-BoldOblique", "NimbusSans-BoldItalic"),
    ("Helvetica-Narrow-Bold", "NimbusSansNarrow-Bold"),
    (
        "Helvetica-Narrow-BoldOblique",
        "NimbusSansNarrow-BoldOblique",
    ),
    ("Helvetica-Narrow", "NimbusSansNarrow-Regular"),
    ("Helvetica-Narrow-Oblique", "NimbusSansNarrow-Oblique"),
    ("Helvetica", "NimbusSans-Regular"),
    ("Helvetica-Oblique", "NimbusSans-Italic"),
    ("NewCenturySchlbk-Bold", "C059-Bold"),
    ("NewCenturySchlbk-BoldItalic", "C059-BdIta"),
    ("NewCenturySchlbk-Italic", "C059-Italic"),
    ("NewCenturySchlbk-Roman", "C059-Roman"),
    ("Palatino-BoldItalic", "P052-BoldItalic"),
    ("Palatino-Bold", "P052-Bold"),
    ("Palatino-Italic", "P052-Italic"),
    ("Palatino-Roman", "P052-Roman"),
    ("Symbol", "StandardSymbolsPS"),
    ("Times-BoldItalic", "NimbusRoman-BoldItalic"),
    ("Times-Bold", "NimbusRoman-Bold"),
    ("Times-Italic", "NimbusRoman-Italic"),
    ("Times-Roman", "NimbusRoman-Regular"),
    ("ZapfChancery-MediumItalic", "Z003-MediumItalic"),
    ("ZapfDingbats", "D050000L"),
];

/// gs_fonts.ps's `.substitutefaces`.
const FACES: &[(&str, &str, i32)] = &[
    ("Charter", "Times", 0),
    ("Utopia", "Times", 0),
    ("Antiqua", "Times", 0),
    ("Calligraphic", "Helvetica", 0),
    ("Hershey", "Times", 0),
    ("Book", "NewCenturySchlbk", 0),
    ("Grot", "Helvetica", 0),
    ("Roman", "Times", 0),
    ("Chancery", "ZapfChancery-MediumItalic", 0),
    ("Arial", "Helvetica", 0),
    ("Avant", "AvantGarde", 0),
    ("Bookman", "Bookman", 0),
    ("Century", "NewCenturySchlbk", 0),
    ("Cour", "Courier", 0),
    ("Frut", "Helvetica", 0),
    ("Garamond", "Palatino", 0),
    ("Geneva", "Helvetica", 0),
    ("Helv", "Helvetica", 0),
    ("NewYork", "Bookman", 0),
    ("Pala", "Palatino", 0),
    ("Schlbk", "NewCenturySchlbk", 0),
    ("Swiss", "Helvetica", 0),
    ("Symbol", "Symbol", 0),
    ("Times", "Times", 0),
    ("Minion", "Times", 0),
    ("Myriad", "Helvetica", 0),
    ("Monospace", "Courier", 0),
    ("Typewriter", "Courier", 0),
    ("Albertus", "Palatino", 0),
    ("AntiqueOlive", "Helvetica", 0),
    ("Bodoni", "NewCenturySchlbk", 0),
    ("Chicago", "Helvetica", 2),
];

fn family(f: &str) -> &'static [&'static str] {
    match f {
        "AvantGarde" => &[
            "AvantGarde-Book",
            "AvantGarde-BookOblique",
            "AvantGarde-Demi",
            "AvantGarde-DemiOblique",
        ],
        "Bookman" => &[
            "Bookman-Demi",
            "Bookman-DemiItalic",
            "Bookman-Light",
            "Bookman-LightItalic",
        ],
        "Courier" => &[
            "Courier",
            "Courier-Oblique",
            "Courier-Bold",
            "Courier-BoldOblique",
        ],
        "Helvetica" => &[
            "Helvetica",
            "Helvetica-Oblique",
            "Helvetica-Bold",
            "Helvetica-BoldOblique",
            "Helvetica-Narrow",
            "Helvetica-Narrow-Oblique",
            "Helvetica-Narrow-Bold",
            "Helvetica-Narrow-BoldOblique",
        ],
        "NewCenturySchlbk" => &[
            "NewCenturySchlbk-Roman",
            "NewCenturySchlbk-Italic",
            "NewCenturySchlbk-Bold",
            "NewCenturySchlbk-BoldItalic",
        ],
        "Palatino" => &[
            "Palatino-Roman",
            "Palatino-Italic",
            "Palatino-Bold",
            "Palatino-BoldItalic",
        ],
        "Symbol" => &["Symbol", "Symbol", "Symbol", "Symbol"],
        "Times" => &[
            "Times-Roman",
            "Times-Italic",
            "Times-Bold",
            "Times-BoldItalic",
        ],
        _ => &["ZapfChancery-MediumItalic"],
    }
}

/// `.substitutefont`: the font gs uses for an unknown name.
fn substitute(name: &[u8]) -> &'static str {
    let s = core::str::from_utf8(name).unwrap_or("");
    // (.fontnameproperties: bold 1? the properties index as gs builds it:
    // italic 1, bold 2, narrow 4)
    let mut props = 0;
    if s.contains("Italic") || s.contains("Oblique") {
        props |= 1;
    }
    if s.contains("Bold") || s.contains("Black") || s.contains("Heavy") || s.contains("Demi") {
        props |= 2;
    }
    let mut fam: Option<&str> = None;
    for &(face, f, p) in FACES {
        if s.contains(face) {
            fam = Some(f);
            props |= p;
        }
    }
    match fam {
        Some(f) => {
            let l = family(f);
            l[props as usize % l.len()]
        }
        None => {
            // (defaultfontname and its variants)
            const D: [&str; 12] = [
                "Courier",
                "Helvetica-Oblique",
                "Helvetica-Bold",
                "Helvetica-BoldOblique",
                "Helvetica-Narrow",
                "Helvetica-Narrow-Oblique",
                "Helvetica-Narrow-Bold",
                "Helvetica-Narrow-BoldOblique",
                "Times-Roman",
                "Times-Italic",
                "Times-Bold",
                "Times-BoldItalic",
            ];
            D[props as usize % D.len()]
        }
    }
}

impl Ps<'_> {
    /// The font of a font dictionary (its FID).
    pub fn font_of(&mut self, d: &PDict) -> Option<Rc<Font>> {
        let fid = self.dget_s(d, "FID")?;
        match fid.v {
            V::FontId(i) => self.fonts.get(i as usize).cloned().flatten(),
            _ => None,
        }
    }

    /// Registers a font for dictionary `d`.
    fn register_font(&mut self, d: &PDict, base: Option<&Rc<Font>>) -> R<Rc<Font>> {
        let ft = self
            .dget_s(d, "FontType")
            .and_then(|o| o.num())
            .ok_or(Sig::Err(E::InvalidFont))? as i32;
        let m = self
            .dget_s(d, "FontMatrix")
            .ok_or(Sig::Err(E::InvalidFont))?;
        let matrix = crate::gstate::matrix_of(self, &m)?;
        let name = match self.dget_s(d, "FontName").map(|o| o.v) {
            Some(V::Name(n)) => self.names.get(n).to_vec(),
            Some(V::Str(s)) => s.bytes(),
            _ => Vec::new(),
        };
        let bbox = match self.dget_s(d, "FontBBox") {
            Some(o) => {
                let v = self.nums_of(&o).unwrap_or_default();
                if v.len() == 4 {
                    [v[0], v[1], v[2], v[3]]
                } else {
                    [0.0; 4]
                }
            }
            None => [0.0; 4],
        };
        let paint_type = self
            .dget_s(d, "PaintType")
            .and_then(|o| o.num())
            .unwrap_or(0.0) as i32;
        let stroke_width = self
            .dget_s(d, "StrokeWidth")
            .and_then(|o| o.num())
            .unwrap_or(0.0);
        let (nearest, exact) = self.nearest_encoding(d);
        let id = self.fonts.len() as u32;
        let f = Rc::new(Font {
            id,
            base: base.map_or(id, |b| b.base),
            font_type: ft,
            name,
            matrix,
            orig_matrix: base.map_or(matrix, |b| b.orig_matrix),
            bbox,
            paint_type,
            stroke_width,
            dict: d.clone(),
            nearest_encoding: nearest,
            encoding_index: exact,
            resource: base.and_then(|b| b.resource.clone()),
        });
        self.fonts.push(Some(f.clone()));
        let fid = Obj {
            v: V::FontId(id),
            x: false,
        };
        // (FID is put in even a read-only dictionary)
        let k = self.nobj("FID");
        let key = self.key(&k)?;
        d.data.borrow_mut().put(key, k, fid);
        Ok(f)
    }

    /// `lookup_gs_simple_font_encoding`.
    fn nearest_encoding(&mut self, d: &PDict) -> (i32, i32) {
        let Some(V::Arr(a)) = self.dget_s(d, "Encoding").map(|o| o.v) else {
            return (-1, -1);
        };
        if a.len > 256 {
            return (-1, -1);
        }
        let esize = a.len;
        let names: Vec<Vec<u8>> = a
            .to_vec()
            .iter()
            .map(|o| match o.v {
                V::Name(n) => self.names.get(n).to_vec(),
                _ => Vec::new(),
            })
            .collect();
        let mut best = esize / 3;
        let mut near = -1;
        for index in 0..7 {
            let enc = crate::pdffont::known_encoding(index);
            let mut matched = esize;
            for i in (0..esize).rev() {
                if enc[i].as_bytes() == names[i].as_slice() {
                    continue;
                }
                matched -= 1;
                if matched <= best {
                    break;
                }
            }
            if matched > best {
                best = matched;
                near = index as i32;
                if best == esize {
                    break;
                }
            }
        }
        (near, if best == esize { near } else { -1 })
    }

    /// `findfont`.
    pub fn findfont(&mut self, name: &Obj) -> R<Obj> {
        let fd = self.fontdir.clone();
        if let Some(f) = self.dget(&fd, name)? {
            return Ok(f);
        }
        let n: Vec<u8> = match &name.v {
            V::Name(n) => self.names.get(*n).to_vec(),
            V::Str(s) => s.bytes(),
            _ => return Err(E::TypeCheck.into()),
        };
        // (Fontmap.GS: an alias of the base 35, else the name itself as a
        // resource file; else gs's substitute)
        let file = |n: &[u8]| {
            FONTMAP
                .iter()
                .find(|(a, _)| a.as_bytes() == n)
                .map(|(_, f)| f.as_bytes().to_vec())
        };
        let res_name = file(&n).unwrap_or_else(|| n.clone());
        if let Some(f) = self.load_font_resource(&res_name)? {
            if res_name == n {
                return Ok(f);
            }
            // (.aliasfont: a copy named as asked)
            let V::Dict(sd) = &f.v else {
                return Err(E::InvalidFont.into());
            };
            let sd = sd.clone();
            return self.alias_font(name, &sd, Some(res_name));
        }
        // (substitution: the substitute's font copied under the requested name)
        let sub = substitute(&n);
        let sname = self.nobj(sub);
        let f = self.findfont(&sname)?;
        let V::Dict(sd) = &f.v else {
            return Err(E::InvalidFont.into());
        };
        let sd = sd.clone();
        let res = self.font_of(&sd).and_then(|b| b.resource.clone());
        self.alias_font(name, &sd, res)
    }

    /// `.aliasfont`: font `sd` copied, its FontName `name`, defined
    /// under `name`.
    fn alias_font(&mut self, name: &Obj, sd: &PDict, resource: Option<Vec<u8>>) -> R<Obj> {
        let copy = {
            let data = sd.data.borrow().clone();
            let d = self.new_dict(data.len() + 2);
            *d.data.borrow_mut() = data;
            d
        };
        let fid = self.nobj("FID");
        let k = self.key(&fid)?;
        copy.data.borrow_mut().remove(&k);
        let nn = match &name.v {
            V::Str(s) => Obj::name(self.names.intern(&s.bytes())),
            _ => name.clone(),
        };
        self.dput_s(&copy, "FontName", nn);
        let font = self.register_font(&copy, None)?;
        if resource.is_some() {
            let mut fm = clone_font(&font);
            fm.resource = resource;
            let id = fm.id as usize;
            self.fonts[id] = Some(Rc::new(fm));
        }
        let fobj = Obj::dict(copy);
        let fd = self.fontdir.clone();
        self.dput(&fd, name.clone(), fobj.clone())?;
        Ok(fobj)
    }

    /// Runs Ghostscript's font file `name` (its `Resource/Font`) and
    /// returns the font it defined.
    fn load_font_resource(&mut self, name: &[u8]) -> R<Option<Obj>> {
        let Some(data) = self.res.font(name) else {
            return Ok(None);
        };
        let saved_global = self.vm.global;
        self.vm.global = true;
        let f = Rc::new(RefCell::new(File::new(data)));
        let depth = self.o.len();
        let r = self.call(Obj {
            v: V::File(f),
            x: true,
        });
        self.vm.global = saved_global;
        r?;
        self.o.truncate(depth);
        let fd = self.fontdir.clone();
        let nobj = Obj::name(self.names.intern(name));
        let found = self.dget(&fd, &nobj)?;
        if let Some(Obj { v: V::Dict(d), .. }) = &found
            && let Some(f) = self.font_of(d)
        {
            // (a resource font: gs's own file)
            let mut fm = clone_font(&f);
            fm.resource = Some(name.to_vec());
            let id = fm.id as usize;
            self.fonts[id] = Some(Rc::new(fm));
        }
        Ok(found)
    }
}

fn clone_font(rc: &Font) -> Font {
    Font {
        id: rc.id,
        base: rc.base,
        font_type: rc.font_type,
        name: rc.name.clone(),
        matrix: rc.matrix,
        orig_matrix: rc.orig_matrix,
        bbox: rc.bbox,
        paint_type: rc.paint_type,
        stroke_width: rc.stroke_width,
        dict: rc.dict.clone(),
        nearest_encoding: rc.nearest_encoding,
        encoding_index: rc.encoding_index,
        resource: rc.resource.clone(),
    }
}

fn op_definefont(ps: &mut Ps<'_>) -> R {
    let d = match &ps.top(0)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let key = ps.top(1)?.clone();
    ps.register_font(&d, None)?;
    let fd = ps.fontdir.clone();
    ps.dput(&fd, key, Obj::dict(d.clone()))?;
    ps.o.truncate(ps.o.len() - 2);
    ps.push(Obj::dict(d));
    Ok(())
}
fn op_undefinefont(ps: &mut Ps<'_>) -> R {
    let k = ps.pop()?;
    let fd = ps.fontdir.clone();
    let key = ps.key(&k)?;
    fd.data.borrow_mut().remove(&key);
    Ok(())
}
fn op_findfont(ps: &mut Ps<'_>) -> R {
    let n = ps.top(0)?.clone();
    let f = ps.findfont(&n)?;
    ps.o.pop();
    ps.push(f);
    Ok(())
}
/// A copy of font dictionary `d` with matrix `m` (`makefont`).
fn make_font(ps: &mut Ps<'_>, d: &PDict, m: &M) -> R<Obj> {
    let base = ps.font_of(d).ok_or(Sig::Err(E::InvalidFont))?;
    let fm = m.mul(&base.matrix);
    let data = d.data.borrow().clone();
    let nd = ps.new_dict(data.len() + 2);
    *nd.data.borrow_mut() = data;
    let ma = ps.arr_obj(fm.to_vec().iter().map(|&x| Obj::real(x)).collect());
    ps.dput_s(&nd, "FontMatrix", ma);
    let mut ob = ps.arr_obj(m.to_vec().iter().map(|&x| Obj::real(x)).collect());
    if let Some(prev) = ps.dget_s(d, "ScaleMatrix") {
        let pm = crate::gstate::matrix_of(ps, &prev)?;
        ob = ps.arr_obj(pm.mul(m).to_vec().iter().map(|&x| Obj::real(x)).collect());
    }
    ps.dput_s(&nd, "ScaleMatrix", ob);
    let f = ps.register_font(&nd, Some(&base))?;
    // (the copy's own matrix, the base's identity kept)
    let _ = f;
    Ok(Obj::dict(nd))
}
fn op_scalefont(ps: &mut Ps<'_>) -> R {
    let s = ps.num_at(0)?;
    let d = match &ps.top(1)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let f = make_font(ps, &d, &M::scaling(s, s))?;
    ps.o.truncate(ps.o.len() - 2);
    ps.push(f);
    Ok(())
}
fn op_makefont(ps: &mut Ps<'_>) -> R {
    let mo = ps.top(0)?.clone();
    let m = crate::gstate::matrix_of(ps, &mo)?;
    let d = match &ps.top(1)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    let f = make_font(ps, &d, &m)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.push(f);
    Ok(())
}
fn op_setfont(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let V::Dict(d) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    let f = ps.font_of(d).ok_or(Sig::Err(E::InvalidFont))?;
    ps.o.pop();
    ps.gs.font = Some(f);
    ps.gs.font_dict = Some(o);
    Ok(())
}
fn op_currentfont(ps: &mut Ps<'_>) -> R {
    let f = match &ps.gs.font_dict {
        Some(f) => f.clone(),
        None => {
            let n = ps.nobj("Courier");
            ps.findfont(&n)?
        }
    };
    ps.push(f);
    Ok(())
}
fn op_selectfont(ps: &mut Ps<'_>) -> R {
    let t = ps.top(0)?.clone();
    let n = ps.top(1)?.clone();
    let f = ps.findfont(&n)?;
    let V::Dict(d) = &f.v else {
        return Err(E::InvalidFont.into());
    };
    let m = if let V::Arr(_) = t.v {
        crate::gstate::matrix_of(ps, &t)?
    } else {
        let s = t.num().ok_or(Sig::Err(E::TypeCheck))?;
        M::scaling(s, s)
    };
    let nf = make_font(ps, d, &m)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.push(nf);
    op_setfont(ps)
}
fn op_eexec(ps: &mut Ps<'_>) -> R {
    let src = ps.pop()?;
    let (data, outer) = match &src.v {
        V::File(f) => {
            let b = f.borrow();
            (b.rest().to_vec(), Some(f.clone()))
        }
        V::Str(s) => (s.bytes(), None),
        _ => return Err(E::TypeCheck.into()),
    };
    // (hex if the first four bytes are hex digits)
    let mut start = 0;
    while start < data.len() && crate::scan::is_ws(data[start]) {
        start += 1;
    }
    // (s_exD_process: hex if the first 8 bytes are hex digits or spaces)
    let hex = data[start..]
        .iter()
        .take(8)
        .all(|&c| c.is_ascii_hexdigit() || crate::scan::is_ws(c));
    let (cipher, used) = if hex {
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        let mut i = start;
        while i < data.len() {
            let c = data[i];
            let dv = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ if crate::scan::is_ws(c) => {
                    i += 1;
                    continue;
                }
                _ => break,
            };
            match hi.take() {
                None => hi = Some(dv),
                Some(h) => out.push(h << 4 | dv),
            }
            i += 1;
        }
        (out, i)
    } else {
        (data[start..].to_vec(), data.len())
    };
    let plain = decrypt(&cipher, 55665, 4);
    // (the outer file goes on after the encrypted part: at the zeros that
    // end it, hex or binary)
    if let Some(o) = outer {
        let mut ob = o.borrow_mut();
        let base = ob.pos;
        let skip =
            find_zeros(&data[start..]).map_or(if hex { used } else { data.len() }, |k| start + k);
        ob.pos = base + skip;
    }
    let f = Rc::new(RefCell::new(File::new(Rc::new(plain))));
    // (eexec runs with systemdict begun, and ends it after)
    ps.e.push(Frame::EndDict);
    ps.e.push(Frame::File(f));
    let sd = ps.systemdict.clone();
    ps.d.push(sd);
    Ok(())
}
/// Where 64 ASCII zeros begin (a font's trailer).
fn find_zeros(d: &[u8]) -> Option<usize> {
    let mut run = 0;
    for (i, &c) in d.iter().enumerate() {
        if c == b'0' {
            run += 1;
            if run == 64 {
                return Some(i + 1 - 64);
            }
        } else if c != b'\n' && c != b'\r' {
            run = 0;
        }
    }
    None
}
/// Type 1 decryption (`r` the key, the first `skip` bytes dropped).
pub fn decrypt(c: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut r = key;
    let mut out = Vec::with_capacity(c.len());
    for &b in c {
        let p = b ^ (r >> 8) as u8;
        r = (u16::from(b).wrapping_add(r))
            .wrapping_mul(52845)
            .wrapping_add(22719);
        out.push(p);
    }
    out.into_iter().skip(skip).collect()
}

/// A glyph's metrics and outline in character space.
#[derive(Default, Clone, Debug)]
pub struct Glyph {
    pub width: (f64, f64),
    pub sb: (f64, f64),
    /// The outline: moveto (0), lineto (1), curveto (2: three points),
    /// closepath (3).
    pub path: Vec<(u8, [f64; 6])>,
    pub seac: Option<(f64, f64, f64, u8, u8)>,
}

impl Glyph {
    /// The outline's bounding box (all points, control points too).
    pub fn bbox(&self) -> Option<[f64; 4]> {
        let mut b: Option<[f64; 4]> = None;
        let mut add = |x: f64, y: f64| {
            b = Some(match b {
                None => [x, y, x, y],
                Some(c) => [c[0].min(x), c[1].min(y), c[2].max(x), c[3].max(y)],
            });
        };
        for (op, p) in &self.path {
            match op {
                0 | 1 => add(p[0], p[1]),
                2 => {
                    add(p[0], p[1]);
                    add(p[2], p[3]);
                    add(p[4], p[5]);
                }
                _ => {}
            }
        }
        b
    }
}

impl Ps<'_> {
    /// A Type 1 glyph's charstring (decrypted), its font's Subrs, lenIV.
    pub fn charstring(&mut self, d: &PDict, glyph: &[u8]) -> Option<Vec<u8>> {
        let cs = self.dget_s(d, "CharStrings")?;
        let V::Dict(cs) = cs.v else { return None };
        let n = self.names.find(glyph)?;
        let s = self.dget_n(&cs, n)?;
        let V::Str(s) = s.v else { return None };
        Some(s.bytes())
    }
    fn len_iv(&mut self, d: &PDict) -> i64 {
        match self.dget_s(d, "Private").map(|o| o.v) {
            Some(V::Dict(p)) => self
                .dget_s(&p, "lenIV")
                .and_then(|o| o.num())
                .unwrap_or(4.0) as i64,
            _ => 4,
        }
    }
    fn subr(&mut self, d: &PDict, i: usize) -> Option<Vec<u8>> {
        let V::Dict(p) = self.dget_s(d, "Private")?.v else {
            return None;
        };
        let V::Arr(a) = self.dget_s(&p, "Subrs")?.v else {
            return None;
        };
        if i >= a.len {
            return None;
        }
        match a.get(i).v {
            V::Str(s) => Some(s.bytes()),
            _ => None,
        }
    }
    /// Runs glyph `glyph` of Type 1 font dictionary `d`.
    pub fn type1_glyph(&mut self, d: &PDict, glyph: &[u8]) -> Option<Glyph> {
        let cs = self.charstring(d, glyph)?;
        let liv = self.len_iv(d);
        let mut g = Glyph::default();
        let mut st = T1 {
            stack: Vec::new(),
            ps_stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            flex: Vec::new(),
            in_flex: false,
            done: false,
        };
        self.t1_run(d, &cs, liv, &mut g, &mut st, 0);
        // (Metrics overrides the width)
        if let Some(V::Dict(m)) = self.dget_s(d, "Metrics").map(|o| o.v)
            && let Some(n) = self.names.find(glyph)
            && let Some(w) = self.dget_n(&m, n)
        {
            match &w.v {
                V::Int(_) | V::Real(_) => g.width = (w.num().unwrap_or(0.0), 0.0),
                V::Arr(_) => {
                    let v = self.nums_of(&w).unwrap_or_default();
                    if v.len() == 2 {
                        g.width = (v[1], 0.0);
                    } else if v.len() == 4 {
                        g.width = (v[2], v[3]);
                    }
                }
                _ => {}
            }
        }
        Some(g)
    }
    fn t1_run(
        &mut self,
        d: &PDict,
        enc: &[u8],
        liv: i64,
        g: &mut Glyph,
        st: &mut T1,
        depth: usize,
    ) {
        if depth > 20 || st.done {
            return;
        }
        let code = if liv < 0 {
            enc.to_vec()
        } else {
            decrypt(enc, 4330, liv as usize)
        };
        let mut i = 0;
        while i < code.len() && !st.done {
            let v = code[i];
            i += 1;
            if v >= 32 {
                let n: i32 = match v {
                    32..=246 => i32::from(v) - 139,
                    247..=250 => {
                        let w = *code.get(i).unwrap_or(&0);
                        i += 1;
                        (i32::from(v) - 247) * 256 + i32::from(w) + 108
                    }
                    251..=254 => {
                        let w = *code.get(i).unwrap_or(&0);
                        i += 1;
                        -(i32::from(v) - 251) * 256 - i32::from(w) - 108
                    }
                    _ => {
                        let b = code.get(i..i + 4).unwrap_or(&[0, 0, 0, 0]);
                        i += 4;
                        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
                    }
                };
                st.stack.push(f64::from(n));
                continue;
            }
            let s = &mut st.stack;
            let arg = |s: &Vec<f64>, k: usize| s.get(k).copied().unwrap_or(0.0);
            match v {
                13 => {
                    // hsbw
                    g.sb = (arg(s, 0), 0.0);
                    g.width = (arg(s, 1), 0.0);
                    st.x = arg(s, 0);
                    st.y = 0.0;
                    s.clear();
                }
                9 => {
                    g.path.push((3, [0.0; 6]));
                    s.clear();
                }
                5 => {
                    st.x += arg(s, 0);
                    st.y += arg(s, 1);
                    g.path.push((1, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    s.clear();
                }
                6 => {
                    st.x += arg(s, 0);
                    g.path.push((1, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    s.clear();
                }
                7 => {
                    st.y += arg(s, 0);
                    g.path.push((1, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    s.clear();
                }
                8 => {
                    let (x1, y1) = (st.x + arg(s, 0), st.y + arg(s, 1));
                    let (x2, y2) = (x1 + arg(s, 2), y1 + arg(s, 3));
                    let (x3, y3) = (x2 + arg(s, 4), y2 + arg(s, 5));
                    g.path.push((2, [x1, y1, x2, y2, x3, y3]));
                    st.x = x3;
                    st.y = y3;
                    s.clear();
                }
                30 => {
                    // vhcurveto
                    let (x1, y1) = (st.x, st.y + arg(s, 0));
                    let (x2, y2) = (x1 + arg(s, 1), y1 + arg(s, 2));
                    let (x3, y3) = (x2 + arg(s, 3), y2);
                    g.path.push((2, [x1, y1, x2, y2, x3, y3]));
                    st.x = x3;
                    st.y = y3;
                    s.clear();
                }
                31 => {
                    // hvcurveto
                    let (x1, y1) = (st.x + arg(s, 0), st.y);
                    let (x2, y2) = (x1 + arg(s, 1), y1 + arg(s, 2));
                    let (x3, y3) = (x2, y2 + arg(s, 3));
                    g.path.push((2, [x1, y1, x2, y2, x3, y3]));
                    st.x = x3;
                    st.y = y3;
                    s.clear();
                }
                21 => {
                    st.x += arg(s, 0);
                    st.y += arg(s, 1);
                    if st.in_flex {
                        st.flex.push((st.x, st.y));
                    } else {
                        g.path.push((0, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    }
                    s.clear();
                }
                22 => {
                    st.x += arg(s, 0);
                    if st.in_flex {
                        st.flex.push((st.x, st.y));
                    } else {
                        g.path.push((0, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    }
                    s.clear();
                }
                4 => {
                    st.y += arg(s, 0);
                    if st.in_flex {
                        st.flex.push((st.x, st.y));
                    } else {
                        g.path.push((0, [st.x, st.y, 0.0, 0.0, 0.0, 0.0]));
                    }
                    s.clear();
                }
                1 | 3 => s.clear(),
                10 => {
                    let n = s.pop().unwrap_or(0.0) as usize;
                    if let Some(sub) = self.subr(d, n) {
                        self.t1_run(d, &sub, liv, g, st, depth + 1);
                    }
                }
                11 => return,
                14 => {
                    st.done = true;
                    s.clear();
                }
                12 => {
                    let e = *code.get(i).unwrap_or(&0);
                    i += 1;
                    match e {
                        0..=2 => s.clear(),
                        6 => {
                            // seac
                            g.seac = Some((
                                arg(s, 0),
                                arg(s, 1),
                                arg(s, 2),
                                arg(s, 3) as u8,
                                arg(s, 4) as u8,
                            ));
                            let (asb, adx, ady, bchar, achar) = (
                                arg(s, 0),
                                arg(s, 1),
                                arg(s, 2),
                                arg(s, 3) as usize,
                                arg(s, 4) as usize,
                            );
                            s.clear();
                            let std = crate::enc::STANDARD_ENCODING;
                            let (w, sb) = (g.width, g.sb);
                            if let Some(bg) = self.type1_glyph(d, std[bchar.min(255)].as_bytes()) {
                                g.path.extend(bg.path);
                            }
                            if let Some(ag) = self.type1_glyph(d, std[achar.min(255)].as_bytes()) {
                                let (ox, oy) = (adx - asb, ady);
                                for (op, mut p) in ag.path {
                                    for k in 0..3 {
                                        p[2 * k] += ox;
                                        p[2 * k + 1] += oy;
                                    }
                                    g.path.push((op, p));
                                }
                            }
                            g.width = w;
                            g.sb = sb;
                            st.done = true;
                        }
                        7 => {
                            // sbw
                            g.sb = (arg(s, 0), arg(s, 1));
                            g.width = (arg(s, 2), arg(s, 3));
                            st.x = arg(s, 0);
                            st.y = arg(s, 1);
                            s.clear();
                        }
                        12 => {
                            let b = s.pop().unwrap_or(1.0);
                            let a = s.pop().unwrap_or(0.0);
                            s.push(a / b);
                        }
                        16 => {
                            // callothersubr
                            let n = s.pop().unwrap_or(0.0) as i32;
                            let k = s.pop().unwrap_or(0.0) as usize;
                            let mut args: Vec<f64> = Vec::new();
                            for _ in 0..k {
                                args.push(s.pop().unwrap_or(0.0));
                            }
                            match n {
                                1 => {
                                    st.in_flex = true;
                                    st.flex.clear();
                                }
                                2 => {}
                                0 => {
                                    // (flex end: the seven points as two curves)
                                    st.in_flex = false;
                                    let f = core::mem::take(&mut st.flex);
                                    if f.len() >= 7 {
                                        g.path.push((
                                            2,
                                            [f[2].0, f[2].1, f[3].0, f[3].1, f[4].0, f[4].1],
                                        ));
                                        g.path.push((
                                            2,
                                            [f[4].0, f[4].1, f[5].0, f[5].1, f[6].0, f[6].1],
                                        ));
                                        st.x = f[6].0;
                                        st.y = f[6].1;
                                    }
                                    st.ps_stack = vec![st.y, st.x];
                                }
                                3 => st.ps_stack = vec![3.0],
                                _ => st.ps_stack = args,
                            }
                        }
                        17 => {
                            let v = st.ps_stack.pop().unwrap_or(0.0);
                            st.stack.push(v);
                        }
                        33 => {
                            st.x = arg(&st.stack, 0);
                            st.y = arg(&st.stack, 1);
                            st.stack.clear();
                        }
                        _ => st.stack.clear(),
                    }
                }
                _ => s.clear(),
            }
        }
    }
}

struct T1 {
    stack: Vec<f64>,
    ps_stack: Vec<f64>,
    x: f64,
    y: f64,
    flex: Vec<(f64, f64)>,
    in_flex: bool,
    done: bool,
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[
        ("definefont", op_definefont),
        ("undefinefont", op_undefinefont),
        ("findfont", op_findfont),
        ("scalefont", op_scalefont),
        ("makefont", op_makefont),
        ("setfont", op_setfont),
        ("currentfont", op_currentfont),
        ("rootfont", op_currentfont),
        ("selectfont", op_selectfont),
        ("eexec", op_eexec),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
