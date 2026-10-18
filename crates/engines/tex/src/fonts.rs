//! Fonts (§539–§582): TFM files read through the client's `file:` fields,
//! parsed once per (file, size) for the process, and answered to
//! `phitex-typeset` through its `Fonts` trait.

use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{ValueId, Values};
use phitex_tfm::{ScaledFont, Tfm};
use phitex_typeset::fonts::{CharInfo, CharTag, FontId, Fonts, Left, LigKern, LigOp};

use crate::val::{int_of, list_of, str_of};

/// A loaded font.
pub(crate) struct Loaded {
    pub tfm: Tfm,
    pub at: ScaledFont,
}

type Cache = Mutex<FxHashMap<(u64, i32), Option<Arc<Loaded>>>>;

/// The process's parsed fonts: a pure function's memo, by the file's
/// value and the size.
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

pub(crate) fn bytes_of(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
        .collect()
}

/// The TFM in file value `file` (a `[path, bytes]` list) at size `s`
/// (§568: `s<0` is `scaled -s`).
pub(crate) fn load(v: &Values, file: ValueId, s: i32) -> Option<Arc<Loaded>> {
    let key = (file.raw(), s);
    if let Some(x) = cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return x.clone();
    }
    let xs = list_of(v, file);
    let loaded = xs.get(1).and_then(|&b| {
        let bytes = bytes_of(&str_of(v, b));
        let tfm = phitex_tfm::parse(&bytes).ok()?;
        let z = tfm.size_for(s);
        let at = tfm.at(z).ok()?;
        Some(Arc::new(Loaded { tfm, at }))
    });
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, loaded.clone());
    loaded
}

/// A font as a run's state has it ([`crate::run::Run::font_entry`]).
pub(crate) struct Entry {
    pub loaded: Arc<Loaded>,
    pub params: Vec<i32>,
    pub hc: i32,
    pub name: (String, i32),
}

/// The fonts a list uses, with their current `\fontdimen`s and
/// `\hyphenchar`s.
#[derive(Default)]
pub(crate) struct View {
    pub fonts: FontTable,
    pub hyphen: FxHashMap<FontId, i32>,
    pub names: FxHashMap<FontId, (String, i32)>,
}

/// A `fontinfo:<f>` value: name, size, design size, file value, `s`.
pub(crate) struct Info {
    pub name: String,
    pub size: i32,
    pub dsize: i32,
    pub file: Option<ValueId>,
    pub s: i32,
}

pub(crate) fn info(v: &Values, x: Option<ValueId>) -> Option<Info> {
    let x = x?;
    let xs = list_of(v, x);
    let n = |k: usize| {
        xs.get(k)
            .map_or(0, |&y| i32::try_from(int_of(v, y)).unwrap_or(0))
    };
    Some(Info {
        name: xs.first().map(|&y| str_of(v, y)).unwrap_or_default(),
        size: n(1),
        dsize: n(2),
        file: xs.get(3).copied(),
        s: n(4),
    })
}

/// A view's fonts by id: an array indexed by the font's number (the
/// metrics of a character a load and an index away, no hashing per
/// character); a number past [`FontTable::DIRECT`] in a map beside it.
#[derive(Default)]
pub(crate) struct FontTable {
    direct: Vec<Option<(Arc<Loaded>, Vec<i32>)>>,
    far: FxHashMap<FontId, (Arc<Loaded>, Vec<i32>)>,
}

impl FontTable {
    const DIRECT: usize = 1 << 16;

    #[inline]
    pub(crate) fn get(&self, f: FontId) -> Option<&(Arc<Loaded>, Vec<i32>)> {
        let i = f as usize;
        if i < Self::DIRECT {
            self.direct.get(i).and_then(Option::as_ref)
        } else {
            self.far.get(&f)
        }
    }

    pub(crate) fn contains_key(&self, f: FontId) -> bool {
        self.get(f).is_some()
    }

    /// Empties the table, its room kept.
    pub(crate) fn clear(&mut self) {
        self.direct.clear();
        self.far.clear();
    }

    pub(crate) fn insert(&mut self, f: FontId, x: (Arc<Loaded>, Vec<i32>)) {
        let i = f as usize;
        if i < Self::DIRECT {
            if self.direct.len() <= i {
                self.direct.resize_with(i + 1, || None);
            }
            self.direct[i] = Some(x);
        } else {
            self.far.insert(f, x);
        }
    }

    /// The fonts' numbers (the direct ones in order, then the others).
    pub(crate) fn ids(&self) -> Vec<FontId> {
        self.direct
            .iter()
            .enumerate()
            .filter(|(_, x)| x.is_some())
            .filter_map(|(i, _)| FontId::try_from(i).ok())
            .chain(self.far.keys().copied())
            .collect()
    }
}

impl View {
    /// Empties the view, its room kept.
    pub(crate) fn clear(&mut self) {
        self.fonts.clear();
        self.hyphen.clear();
        self.names.clear();
    }

    #[inline]
    fn get(&self, f: FontId) -> Option<&(Arc<Loaded>, Vec<i32>)> {
        self.fonts.get(f)
    }

    pub(crate) fn char_dims(&self, f: FontId, c: u32) -> Option<phitex_tfm::CharDims> {
        let (l, _) = self.get(f)?;
        l.at.chars.get(usize::try_from(c).ok()?).copied().flatten()
    }

    pub(crate) fn exists(&self, f: FontId, c: u32) -> bool {
        self.char_dims(f, c).is_some()
    }
}

fn lig_op(op: u8) -> LigOp {
    match op {
        1 => LigOp::KeepRight,
        5 => LigOp::KeepRightSkip,
        2 => LigOp::KeepLeft,
        6 => LigOp::KeepLeftSkip,
        3 => LigOp::KeepBoth,
        7 => LigOp::KeepBothSkip1,
        11 => LigOp::KeepBothSkip2,
        _ => LigOp::Plain,
    }
}

impl Fonts for View {
    fn char_width(&self, font: FontId, ch: u16) -> i32 {
        self.get(font)
            .and_then(|(l, _)| l.at.chars.get(usize::from(ch)).copied().flatten())
            .map_or(0, |d| d.width)
    }

    fn char_whd(&self, font: FontId, ch: u16) -> (i32, i32, i32) {
        self.get(font)
            .and_then(|(l, _)| l.at.chars.get(usize::from(ch)).copied().flatten())
            .map_or((0, 0, 0), |d| (d.width, d.height, d.depth))
    }

    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        let (l, _) = self.get(font)?;
        let d = l.at.chars.get(usize::from(ch)).copied().flatten()?;
        let tag = match l.tfm.char_info(u8::try_from(ch).ok()?) {
            Some(ci) if ci.tag == phitex_tfm::Tag::List => CharTag::List(u16::from(ci.remainder)),
            Some(ci) if ci.tag == phitex_tfm::Tag::Ext => {
                let e = l.tfm.extens().get(usize::from(ci.remainder)).copied();
                e.map_or(CharTag::None, |e| CharTag::Extensible {
                    top: (e.top != 0).then_some(u16::from(e.top)),
                    mid: (e.mid != 0).then_some(u16::from(e.mid)),
                    bot: (e.bot != 0).then_some(u16::from(e.bot)),
                    rep: u16::from(e.rep),
                })
            }
            _ => CharTag::None,
        };
        Some(CharInfo {
            width: d.width,
            height: d.height,
            depth: d.depth,
            italic: d.italic,
            tag,
        })
    }

    fn lig_kern(&self, font: FontId, left: Left, right: u16) -> Option<LigKern> {
        let (l, _) = self.get(font)?;
        let left = match left {
            Left::Char(c) => Some(u8::try_from(c).ok()?),
            Left::Boundary => None,
        };
        let step = l.tfm.lookup(left, Some(u8::try_from(right).ok()?))?;
        Some(if step.is_kern() {
            LigKern::Kern(l.at.char_kern(step))
        } else {
            LigKern::Lig(lig_op(step.op), u16::from(step.remainder))
        })
    }

    fn has_left_boundary(&self, font: FontId) -> bool {
        self.get(font)
            .is_some_and(|(l, _)| l.tfm.bchar_label().is_some())
    }

    fn bchar(&self, font: FontId) -> Option<u16> {
        self.get(font)
            .and_then(|(l, _)| l.tfm.bchar())
            .map(u16::from)
    }

    fn param(&self, font: FontId, n: u16) -> i32 {
        self.get(font)
            .and_then(|(_, p)| p.get(usize::from(n).checked_sub(1)?).copied())
            .unwrap_or(0)
    }

    fn param_count(&self, font: FontId) -> u16 {
        self.get(font)
            .map_or(0, |(_, p)| u16::try_from(p.len()).unwrap_or(0))
    }
}

/// What the DVI file says about a font (§602).
pub(crate) fn font_def(l: &Loaded, name: &str) -> phitex_dvi::FontDef {
    phitex_dvi::FontDef {
        checksum: l.tfm.checksum,
        size: l.at.size,
        design_size: l.at.design_size,
        area: Vec::new(),
        name: name.as_bytes().to_vec(),
        widths: l.at.chars.iter().map(|c| c.map(|d| d.width)).collect(),
    }
}
