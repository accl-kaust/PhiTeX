//! Fonts (§539–§582): TFM files read through the client's `file:` fields,
//! parsed once per (file, size) for the process, and answered to
//! `phitex-typeset` through its `Fonts` trait.

use crate::exec::LockWait as _;
use phitex_defuse::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
    if let Some(x) = cache().locked().get(&key) {
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
    cache().locked().insert(key, loaded.clone());
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

/// A view's fonts by id (a font's key, [`key_of`]): a slot per key's
/// low bits holding the key and its font (the metrics of a character a
/// load, a compare and an index away, no hashing per character; a key
/// is a hash, its low bits spread); a key whose slot another holds in a
/// map beside it.
#[derive(Default)]
pub(crate) struct FontTable {
    direct: Vec<Option<(FontId, (Arc<Loaded>, Vec<i32>))>>,
    far: FxHashMap<FontId, (Arc<Loaded>, Vec<i32>)>,
}

impl FontTable {
    const DIRECT: usize = 1 << 8;

    #[inline]
    fn slot(f: FontId) -> usize {
        f as usize & (Self::DIRECT - 1)
    }

    #[inline]
    pub(crate) fn get(&self, f: FontId) -> Option<&(Arc<Loaded>, Vec<i32>)> {
        match self.direct.get(Self::slot(f)) {
            Some(Some((k, x))) if *k == f => Some(x),
            _ if self.far.is_empty() => None,
            _ => self.far.get(&f),
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
        if self.direct.is_empty() {
            self.direct.resize_with(Self::DIRECT, || None);
        }
        let i = Self::slot(f);
        match &mut self.direct[i] {
            e @ None => *e = Some((f, x)),
            Some((k, y)) if *k == f => *y = x,
            Some(_) => {
                self.far.insert(f, x);
            }
        }
    }

    /// The fonts' keys, in order.
    pub(crate) fn ids(&self) -> Vec<FontId> {
        let mut ids: Vec<FontId> = self
            .direct
            .iter()
            .filter_map(|x| x.as_ref().map(|x| x.0))
            .chain(self.far.keys().copied())
            .collect();
        ids.sort_unstable();
        ids
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

/// The keys a font may have: above the null font (0), below the ids the
/// PDF link gives the local fonts of virtual fonts (`pdfout`'s
/// `LOCAL_FONTS`), and below 2³¹ (the writer stores a copy's base as a
/// negative number).
const KEY_SPAN: u32 = 0x5FFF_FFFE;

/// The key of the font of file `name` at `size` sp, `salt` 0 unless a
/// collision moved it ([`pick_key`]): what a font is known by in every
/// value and field. It depends on the font alone (TeX's §1260 identity:
/// the same name and the same size), never on when it was first loaded;
/// the number TeX gives it, its rank in the order of first loads, is
/// computed only where the output needs it (the DVI writer, the PDF
/// link's `/F` names, `\pdffontname`).
pub(crate) fn key_of(name: &str, size: i32, salt: u32) -> u32 {
    let h = phitex_defuse::hash_bytes(
        phitex_defuse::hash_words(0x666f_6e74_6b65_7900, &[u64::from(size.cast_unsigned()), u64::from(salt)]),
        name.as_bytes(),
    );
    1 + u32::try_from(h % u64::from(KEY_SPAN)).unwrap_or(0)
}

/// The key of the font `name` at `size`: [`key_of`], salted while the
/// key is another font's (`other(key)`: `Some(true)` another font has
/// it, `Some(false)` this font, `None` no font). A collision is reported
/// under `observe` (`PHITEX_FONT_TRACE`); only a collision makes a key
/// depend on which of the two fonts was loaded first.
pub(crate) fn pick_key<E>(
    name: &str,
    size: i32,
    other: &mut dyn FnMut(u32) -> Result<Option<bool>, E>,
) -> Result<u32, E> {
    let mut salt = 0;
    loop {
        let k = key_of(name, size, salt);
        if other(k)? == Some(true) {
            if crate::exec::obs_flag!("PHITEX_FONT_TRACE") {
                eprintln!("font-trace: key {k} of {name} at {size}sp is another font's: salted ({salt})");
            }
            salt += 1;
            continue;
        }
        return Ok(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_depends_on_the_font_alone() {
        assert_eq!(key_of("cmr10", 655_360, 0), key_of("cmr10", 655_360, 0));
        assert_ne!(key_of("cmr10", 655_360, 0), key_of("cmr10", 786_432, 0));
        assert_ne!(key_of("cmr10", 655_360, 0), key_of("cmbx10", 655_360, 0));
        let k = key_of("cmr10", 655_360, 0);
        assert!(k > 0 && k <= KEY_SPAN);
    }

    #[test]
    fn a_colliding_key_is_salted() {
        // (the first key of cmr10 at 10pt held by another font: the
        // salted key, which nothing holds)
        let first = key_of("cmr10", 655_360, 0);
        let mut asked = Vec::new();
        let k = pick_key::<()>("cmr10", 655_360, &mut |k| {
            asked.push(k);
            Ok((k == first).then_some(true))
        })
        .unwrap();
        assert_eq!(k, key_of("cmr10", 655_360, 1));
        assert_eq!(asked, vec![first, k]);
        // (the key held by the same font: that font)
        let k = pick_key::<()>("cmr10", 655_360, &mut |_| Ok(Some(false))).unwrap();
        assert_eq!(k, first);
    }
}
