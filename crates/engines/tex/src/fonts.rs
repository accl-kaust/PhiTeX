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
    /// The digest of `at` a PDF writer's font state carries, made once
    /// ([`phitex_pdf::metrics_hash`]).
    pub at_hash: OnceLock<u64>,
    /// Each character's width, height and depth by code, made once
    /// (`phitex_typeset::fonts::Fonts::whd_table`).
    pub whd: OnceLock<Vec<(i32, i32, i32)>>,
}

impl Loaded {
    /// [`Loaded::whd`].
    pub(crate) fn whd_table(&self) -> &[(i32, i32, i32)] {
        self.whd.get_or_init(|| {
            self.at
                .chars
                .iter()
                .map(|d| d.map_or((0, 0, 0), |d| (d.width, d.height, d.depth)))
                .collect()
        })
    }
}

type Cache = Mutex<FxHashMap<(u64, i32, i32, bool), Option<Arc<Loaded>>>>;

/// The process's parsed fonts: a pure function's memo, by the file's
/// value and the size.
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

pub(crate) fn bytes_of(text: &str) -> Vec<u8> {
    // (ASCII: the bytes are the characters)
    if text.is_ascii() {
        return text.as_bytes().to_vec();
    }
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
        .collect()
}

/// The TFM in file value `file` (a `[path, bytes]` list) at size `s`
/// (§568: `s<0` is `scaled -s`).
pub(crate) fn load(v: &Values, file: ValueId, s: i32) -> Option<Arc<Loaded>> {
    load_spaced(v, file, s, 0, false)
}

/// The font `i` names: its TFM, letter-spaced if it is a letter-spaced
/// font ([`Ls`]), expanded if it is an expanded font ([`Info::ex`]).
pub(crate) fn load_info(v: &Values, i: &Info) -> Option<Arc<Loaded>> {
    let file = i.file?;
    let l = match i.ls {
        Some(ls) => load_spaced(v, file, i.s, ls.dw, ls.nolig),
        None => load(v, file, i.s),
    }?;
    match i.ex {
        Some((_, e, _)) => Some(expanded(&l, e)),
        None => Some(l),
    }
}

/// `auto_expand_font(f, e)` (pdfTeX): font `l` expanded by `e`
/// thousandths, its widths, italic corrections and kerns
/// `round_xn_over_d(x, 1000+e, 1000)`; a memo by the font (the loads'
/// cache keeps it) and `e`.
pub(crate) fn expanded(l: &Arc<Loaded>, e: i32) -> Arc<Loaded> {
    type Memo = Mutex<FxHashMap<(usize, i32), Arc<Loaded>>>;
    static M: OnceLock<Memo> = OnceLock::new();
    let key = (Arc::as_ptr(l) as usize, e);
    let m = M.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Some(x) = m.locked().get(&key) {
        return x.clone();
    }
    let r = |x: i32| phitex_typeset::arith::round_xn_over_d(x, 1000 + e, 1000);
    let mut at = l.at.clone();
    for d in at.chars.iter_mut().flatten() {
        d.width = r(d.width);
        d.italic = r(d.italic);
    }
    for k in &mut at.kerns {
        *k = r(*k);
    }
    let x = Arc::new(Loaded {
        tfm: l.tfm.clone(),
        at,
        at_hash: OnceLock::new(),
        whd: OnceLock::new(),
    });
    m.locked().insert(key, x.clone());
    x
}

/// An expanded font's name (`expand_font_name`): `name` with `+e`.
pub(crate) fn expanded_name(name: &str, e: i32) -> String {
    format!("{name}{e:+}")
}

/// The key of font `name` at `size` expanded by `e` (the key of a font of
/// its name and size, as `tfm_lookup` finds it).
pub(crate) fn expanded_key(name: &str, size: i32, e: i32) -> u32 {
    key_of(&expanded_name(name, e), size, 0)
}

/// [`load`] with every width `dw` wider and, if `nolig`, no lig/kern
/// programs (`letter_space_font`, pdfTeX).
fn load_spaced(v: &Values, file: ValueId, s: i32, dw: i32, nolig: bool) -> Option<Arc<Loaded>> {
    let key = (file.raw(), s, dw, nolig);
    if let Some(x) = cache().locked().get(&key) {
        return x.clone();
    }
    let xs = list_of(v, file);
    let loaded = xs.get(1).and_then(|&b| {
        let bytes = bytes_of(&str_of(v, b));
        let mut tfm = phitex_tfm::parse(&bytes).ok()?;
        if nolig {
            tfm = tfm.without_ligatures();
        }
        let z = tfm.size_for(s);
        let mut at = tfm.at(z).ok()?;
        for d in at.chars.iter_mut().flatten() {
            d.width += dw;
        }
        Some(Arc::new(Loaded {
            tfm,
            at,
            at_hash: OnceLock::new(),
            whd: OnceLock::new(),
        }))
    });
    cache().locked().insert(key, loaded.clone());
    loaded
}

/// A letter-spaced font's making (`letter_space_font`, pdfTeX): the
/// font it spaces, how much wider each character is
/// (`round_xn_over_d(quad(k), e, 1000)`), the move on either side of
/// the character in its packet (`round_xn_over_d(quad(f), e, 2000)`, as
/// the packet's four bytes), whether `nolig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ls {
    pub base: i64,
    pub dw: i32,
    pub mv: [u8; 4],
    pub nolig: bool,
}

impl Ls {
    /// The `[base, dw, move, nolig]` value of a `fontinfo`.
    pub(crate) fn value(&self, v: &Values) -> ValueId {
        v.list(vec![
            v.int(self.base),
            v.int(i64::from(self.dw)),
            v.int(i64::from(u32::from_be_bytes(self.mv))),
            v.int(i64::from(self.nolig)),
        ])
    }

    fn of(v: &Values, x: ValueId) -> Option<Ls> {
        let xs = list_of(v, x);
        let n = |k: usize| xs.get(k).map(|&y| int_of(v, y));
        Some(Ls {
            base: n(0)?,
            dw: i32::try_from(n(1)?).ok()?,
            mv: u32::try_from(n(2)?).ok()?.to_be_bytes(),
            nolig: n(3)? != 0,
        })
    }

    /// The move `w` (scaled points, of a font at `size`) as a packet's
    /// four bytes (`letter_space_font`'s `vf_replace_z` and the bytes
    /// after it).
    pub(crate) fn move_bytes(w: i32, size: i32) -> [u8; 4] {
        let mut z = size;
        let mut alpha: i32 = 16;
        while z >= 0o40000000 {
            z /= 2;
            alpha += alpha;
        }
        let beta = 256 / alpha;
        let alpha = alpha * z;
        let (b0, w) = if w >= 0 { (0u8, w) } else { (255u8, alpha + w) };
        let mut r = w * beta;
        let b1 = r / z;
        r %= z;
        let b2 = if r == 0 {
            0
        } else {
            r *= 256;
            let b = r / z;
            r %= z;
            b
        };
        let b3 = if r == 0 {
            0
        } else {
            r *= 256;
            r / z
        };
        let byte = |x: i32| u8::try_from(x & 0xff).unwrap_or(0);
        [b0, byte(b1), byte(b2), byte(b3)]
    }

    /// The packets of the letter-spaced font of `tfm` (`right4`, the
    /// character, `right4`), every code from `bc` to `ec`.
    pub(crate) fn packets(&self, tfm: &Tfm) -> std::collections::BTreeMap<u8, Vec<u8>> {
        const RIGHT4: u8 = 146;
        const SET1: u8 = 128;
        let mut out = std::collections::BTreeMap::new();
        for c in tfm.bc..=tfm.ec.min(255) {
            let Ok(c) = u8::try_from(c) else { continue };
            let mut p = Vec::with_capacity(12);
            p.push(RIGHT4);
            p.extend(self.mv);
            if c >= SET1 {
                p.push(SET1);
            }
            p.push(c);
            p.push(RIGHT4);
            p.extend(self.mv);
            out.insert(c, p);
        }
        out
    }
}

/// A font as a run's state has it ([`crate::run::Run::font_entry`]).
pub(crate) struct Entry {
    pub loaded: Arc<Loaded>,
    pub params: Vec<i32>,
    pub hc: i32,
    pub name: (String, i32),
}

/// The fonts a list uses, with their current `\fontdimen`s and
/// `\hyphenchar`s; for a paragraph's breaking with pdfTeX's protrusion or
/// expansion, the fonts' codes and expansion ([`Px`]) and the fonts
/// expanded as it asks ([`View::exp_log`]).
#[derive(Default)]
pub(crate) struct View {
    pub fonts: FontTable,
    pub hyphen: FxHashMap<FontId, i32>,
    pub names: FxHashMap<FontId, (String, i32)>,
    pub px: FxHashMap<FontId, Box<Px>>,
    /// An expanded font's base, for its codes.
    pub ex_base: FxHashMap<FontId, FontId>,
    /// The fonts expanded here: their base and the slot of their metrics.
    exp_index: Mutex<FxHashMap<FontId, (FontId, usize)>>,
    /// `get_expand_font`'s requests, in order (the first of each).
    pub exp_log: Mutex<Vec<(FontId, i32)>>,
}

/// A font's pdfTeX data in a paragraph's view: its expansion (and
/// whether automatic), `\lpcode`, `\rpcode`, `\efcode` of each
/// character, `quad`, and the slots of its expanded fonts' metrics (by
/// ratio, from `-shrink` in steps).
pub(crate) struct Px {
    pub expansion: Option<phitex_typeset::fonts::Expansion>,
    pub auto: bool,
    pub lp: Box<[i32; 256]>,
    pub rp: Box<[i32; 256]>,
    pub ef: Box<[i32; 256]>,
    pub quad: i32,
    pub slots: Box<[Slot]>,
}

/// An expanded font's metrics and `\fontdimen`s, once made ([`Px::slots`]).
pub(crate) type Slot = OnceLock<(Arc<Loaded>, Vec<i32>)>;

/// A `fontinfo:<f>` value: name, size, design size, file value, `s`,
/// a letter-spaced font's making, an expanded font's base, ratio and
/// whether it is auto-expanded (`pdf_font_blink`,
/// `pdf_font_expand_ratio`, `pdf_font_auto_expand`).
#[derive(Clone, Debug)]
pub(crate) struct Info {
    pub name: String,
    pub size: i32,
    pub dsize: i32,
    pub file: Option<ValueId>,
    pub s: i32,
    pub ls: Option<Ls>,
    pub ex: Option<(i64, i32, bool)>,
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
        ls: xs.get(5).and_then(|&y| Ls::of(v, y)),
        ex: xs.get(6).and_then(|&y| {
            let ys = list_of(v, y);
            let g = |k: usize| ys.get(k).map(|&z| int_of(v, z));
            Some((g(0)?, i32::try_from(g(1)?).ok()?, g(2)? != 0))
        }),
    })
}

/// A view's fonts by id (a font's key, [`key_of`]): a slot per key's
/// low bits holding the key and its font (the metrics of a character a
/// load, a compare and an index away, no hashing per character; a key
/// is a hash, its low bits spread); a key whose slot another holds in a
/// map beside it.
#[derive(Clone, Default)]
pub(crate) struct FontTable {
    direct: Vec<Option<(FontId, Metrics)>>,
    far: FxHashMap<FontId, Metrics>,
}

/// A font's metrics and its `\fontdimen`s ([`FontTable`]).
type Metrics = (Arc<Loaded>, Vec<i32>);

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
    /// A copy of the fonts, their `\fontdimen`s and `\hyphenchar`s (no
    /// pdfTeX codes, nothing expanded).
    pub(crate) fn fonts_copy(&self) -> View {
        View {
            fonts: self.fonts.clone(),
            hyphen: self.hyphen.clone(),
            names: self.names.clone(),
            ex_base: self.ex_base.clone(),
            ..View::default()
        }
    }

    /// Empties the view, its room kept.
    pub(crate) fn clear(&mut self) {
        self.fonts.clear();
        self.hyphen.clear();
        self.names.clear();
    }

    #[inline]
    fn get(&self, f: FontId) -> Option<&(Arc<Loaded>, Vec<i32>)> {
        match self.fonts.get(f) {
            Some(x) => Some(x),
            None if self.px.is_empty() => None,
            None => self.get_expanded(f),
        }
    }

    /// A font expanded here ([`Fonts::expand_font`]).
    fn get_expanded(&self, f: FontId) -> Option<&(Arc<Loaded>, Vec<i32>)> {
        let (base, i) = self.exp_index.locked().get(&f).copied()?;
        self.px.get(&base)?.slots.get(i)?.get()
    }

    /// The font whose codes and quad `f` has: an expanded font's base.
    fn base_of(&self, f: FontId) -> FontId {
        if let Some(&b) = self.ex_base.get(&f) {
            return b;
        }
        self.exp_index.locked().get(&f).map_or(f, |x| x.0)
    }

    #[inline]
    pub(crate) fn char_dims(&self, f: FontId, c: u32) -> Option<phitex_tfm::CharDims> {
        let (l, _) = self.get(f)?;
        l.at.chars.get(usize::try_from(c).ok()?).copied().flatten()
    }

    #[inline]
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
    fn whd_table(&self, font: FontId) -> Option<&[(i32, i32, i32)]> {
        self.get(font).map(|(l, _)| l.whd_table())
    }

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

    fn expansion(&self, font: FontId) -> Option<phitex_typeset::fonts::Expansion> {
        self.px.get(&font).and_then(|p| p.expansion)
    }

    fn ef_code(&self, font: FontId, ch: u16) -> i32 {
        self.px
            .get(&self.base_of(font))
            .map_or(1000, |p| p.ef.get(usize::from(ch)).copied().unwrap_or(1000))
    }

    fn lp_code(&self, font: FontId, ch: u16) -> i32 {
        self.px
            .get(&self.base_of(font))
            .map_or(0, |p| p.lp.get(usize::from(ch)).copied().unwrap_or(0))
    }

    fn rp_code(&self, font: FontId, ch: u16) -> i32 {
        self.px
            .get(&self.base_of(font))
            .map_or(0, |p| p.rp.get(usize::from(ch)).copied().unwrap_or(0))
    }

    fn quad(&self, font: FontId) -> i32 {
        match self.px.get(&self.base_of(font)) {
            Some(p) => p.quad,
            None => self.param(font, 6),
        }
    }

    fn expand_font(&self, font: FontId, e: i32) -> FontId {
        self.expand(font, e, true)
    }
}

impl View {
    /// [`Fonts::expand_font`]; `log`: the request noted
    /// ([`View::exp_log`]), else the font only made ready (the fonts at
    /// the limits, made at `\pdffontexpand`).
    pub(crate) fn expand(&self, font: FontId, e: i32, log: bool) -> FontId {
        let Some(p) = self.px.get(&font) else {
            return font;
        };
        let Some(x) = p.expansion else { return font };
        // (pdfTeX loads a TFM of the expanded name where not automatic:
        // none here)
        if !p.auto || x.step <= 0 {
            return font;
        }
        let low = x.shrink.map_or(0, |s| s.1);
        let Some(i) = usize::try_from((e - low) / x.step)
            .ok()
            .filter(|&i| i < p.slots.len())
        else {
            return font;
        };
        let Some((name, size)) = self.names.get(&font) else {
            return font;
        };
        let k = expanded_key(name, *size, e);
        let Some((l, params)) = self.fonts.get(font) else {
            return font;
        };
        p.slots[i].get_or_init(|| (expanded(l, e), params.clone()));
        if self.exp_index.locked().insert(k, (font, i)).is_none() && log {
            self.exp_log.locked().push((font, e));
        }
        k
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
