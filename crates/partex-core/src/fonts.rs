//! Part 30: Font metric data (§539–§582): the font arrays and the null
//! font that `main_body` sets up (web2c allocates them in §1337).

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::font::Font;
use partex_engine::node::{FontId, GlueSpec};
use partex_engine::pack::Fonts;
use partex_ssa::{PVec, Value, Version};

use crate::arith::Scaled;
use crate::host::Host;
use crate::tex::Tex;
use crate::track::{Row, Tracker, font as field};
use crate::web::NULL_FONT;

/// §549: a halfword code that can't match a real character (`qi(256)`).
pub const NON_CHAR: i32 = 256;

/// §549–§550: the fonts, indexed by internal font number. The metrics
/// are the engine's (shared: packaging and output read them directly);
/// the rest is what TeX keeps beside them.
///
/// A checkpoint shares them with the running engine ([`FontArrays`]).
#[derive(Clone)]
pub(crate) struct FontData {
    pub(crate) metrics: Vec<Arc<Font>>,
    /// The TFM file each font was read from (format files keep fonts as
    /// their files).
    pub(crate) tfm: Vec<Arc<[u8]>>,
    pub(crate) name: Vec<i32>,
    pub(crate) area: Vec<i32>,
    /// §1042: the interword glue, once computed.
    pub(crate) glue: Vec<Option<GlueSpec>>,
    pub(crate) used: Vec<bool>,
    pub(crate) hyphen_char: Vec<i32>,
    pub(crate) skew_char: Vec<i32>,
    /// pdfTeX's character codes (`\lpcode` …), each table made on its
    /// first assignment. They are not kept in format files (pdfTeX
    /// doesn't keep the tables' bases).
    pub(crate) codes: Vec<Codes>,
    /// pdfTeX's font expansion of each font.
    pub(crate) expand: Vec<Expand>,
    /// The fonts in the order they were loaded (the null font first): a
    /// font's place in it is its number as tex.web gives it (§576,
    /// `font_ptr + 1`), and so pdfTeX's `/F` name and DVI's font number.
    /// Where a font sits in the arrays (its internal number here, its
    /// *slot*) is the same unless a machine gives fonts slots by what
    /// they are (`Host::font_slot`), so that a font loaded earlier in one
    /// run than another keeps its slot, and so does every later one.
    /// A persistent sequence, each font by its slot and identity: its
    /// version is the table's (DESIGN 7.17.12, `Row::FontTable`).
    pub(crate) order: FontOrder,
    /// Each slot's place in `order` (0: not loaded; the null font's is 0
    /// too).
    pub(crate) rank: Vec<i32>,
    /// pdfTeX's expanded fonts by base font and ratio (`pdf_font_elink`'s
    /// chain, as a map: what the chain is searched for).
    pub(crate) expanded: alloc::collections::BTreeMap<(i32, i32), i32>,
    /// Each slot's identity (what `Host::font_slot` was asked; 0 for the
    /// format's fonts).
    pub(crate) ident: Vec<u128>,
    /// Whether a slot's characters' tags were changed since it was loaded
    /// (`\pdfnoligatures`, `\tagcode`), or it was expanded or copied from
    /// such a font: its metrics are then not what its identity says.
    pub(crate) retagged: Vec<bool>,
    /// A running hash of `order` (a machine's `MCell::FontOrder`).
    pub(crate) order_hash: u128,
    /// Each slot's identity by content (the TFM file's contents, its name,
    /// area and size; an expanded or copied font's base font by its own
    /// identity): the version of its metrics as loaded (DESIGN 7.17.12).
    pub(crate) idv: Vec<u128>,
    /// Whether a slot's metrics were changed in place since it was loaded
    /// (its tags, `\pdfnoligatures` and `\tagcode`; its widths and name,
    /// `\letterspacefont`): their version is then made from them.
    pub(crate) remade: Vec<bool>,
    /// Each slot's character code tables' versions ([`Code`]): a sum of
    /// one hash per entry that is not the default, kept at each
    /// assignment (`set_code`), so a write costs one entry.
    pub(crate) code_sum: Vec<[u128; 8]>,
    /// Each slot's number as the program made it (§576: the fonts made
    /// before it in program order, the null font 0; 0 for one made with
    /// the table, whose number is its place in `order`), and the number
    /// of the last font made (`track::scalar::FONT_COUNT`): a font made
    /// takes the next, so a step that no longer makes a font moves the
    /// numbers of the fonts made after it, which their makers make again.
    pub(crate) num: Vec<i32>,
    pub(crate) count: i32,
    /// `XeTeX`: each slot's native font (`font_layout_engine`), if it is
    /// one.
    pub(crate) native: Vec<Option<Arc<crate::native::NativeFont>>>,
    /// `XeTeX`: each native font's direction state, the script its last
    /// word was shaped in (an OpenType tag, 0 for none): the default
    /// direction of its next word (the layout engine's buffer's script).
    pub(crate) native_dir: Vec<u32>,
}

/// A loaded font in the table's order: its slot and identity, versioned
/// when it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Loaded {
    pub(crate) slot: i32,
    idv: u128,
    ver: Version,
}

impl Loaded {
    fn new(slot: i32, idv: u128) -> Self {
        let ver = Version::node(
            0x6c6f_6164,
            &[Version(u128::from(slot.cast_unsigned())), Version(idv)],
        );
        Loaded { slot, idv, ver }
    }
}

impl Value for Loaded {
    fn version(&self) -> Version {
        self.ver
    }
}

/// The table of loaded fonts (§549, DESIGN 7.17.12's `fonts` row): a
/// persistent sequence, so a copy is O(1) and an append shares the prefix,
/// its version made from the fonts' as they are added.
#[derive(Clone, Default)]
pub(crate) struct FontOrder(PVec<Loaded>);

impl FontOrder {
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The slot of the `k`th font loaded.
    pub(crate) fn get(&self, k: usize) -> Option<i32> {
        self.0.get(k).map(|l| l.slot)
    }

    pub(crate) fn last(&self) -> Option<i32> {
        self.len().checked_sub(1).and_then(|k| self.get(k))
    }

    pub(crate) fn push(&mut self, slot: i32, idv: u128) {
        self.0.push(Loaded::new(slot, idv));
    }

    /// The slots in order, from the `from`th.
    pub(crate) fn slots(&self, from: usize) -> Vec<i32> {
        self.0.iter().skip(from).map(|l| l.slot).collect()
    }

    /// The table's version, made from its fonts' (O(1)).
    pub(crate) fn version(&self) -> u128 {
        self.0.version().0
    }
}

impl core::hash::Hash for FontOrder {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        self.len().hash(h);
        for l in &self.0 {
            l.slot.hash(h);
        }
    }
}

impl partex_engine::persist::Persist for FontOrder {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        let v: Vec<(i32, u128)> = self.0.iter().map(|l| (l.slot, l.idv)).collect();
        v.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let v: Vec<(i32, u128)> = partex_engine::persist::Persist::load(l)?;
        Some(FontOrder(PVec::from_vec(
            v.into_iter().map(|(s, i)| Loaded::new(s, i)).collect(),
        )))
    }
}

/// The fonts ([`FontData`]), shared with the checkpoints: a snapshot's
/// clone is one reference count, and the first change after it (a font
/// loaded, a `\fontdimen` or `\hyphenchar` set, an interword glue
/// cached, §1042) copies the arrays, once (DESIGN.md §7.16.3: 120 µs a
/// cut on the course, 9,001 slots of twelve arrays, before). The flag
/// stays outside what is shared, as scratch of this engine only.
#[derive(Clone)]
pub(crate) struct FontArrays {
    data: crate::cow::Shared<FontData>,
    /// The order was observed (a number, the fonts in order, the last
    /// one): a machine's region reads `MCell::FontOrder` (scratch).
    pub(crate) order_read: crate::relaxed::Flag,
}

impl core::ops::Deref for FontArrays {
    type Target = FontData;
    #[inline]
    fn deref(&self) -> &FontData {
        &self.data
    }
}

impl core::ops::DerefMut for FontArrays {
    #[inline]
    fn deref_mut(&mut self) -> &mut FontData {
        &mut self.data
    }
}

partex_engine::persist_struct!(FontArrays { data, order_read });

/// A machine's reads and writes of the fonts by name and of expanded fonts
/// (`Tex::font_cells`), logged for its region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FontTouch {
    /// The fonts named so (searched, or one loaded: written).
    Name(Arc<[u8]>, bool),
    /// The font expanded from a base font by a ratio.
    Expand(i32, i32, bool),
}

partex_engine::persist_enum!(FontTouch {
    Name(a0, a1),
    Expand(a0, a1, a2)
});

/// What pdfTeX keeps about a font's expansion (`pdf_font_step`, …);
/// fonts are internal font numbers, 0 for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Expand {
    /// `pdf_font_step`: 0 unless `\pdffontexpand` made the font
    /// expandable.
    pub step: i32,
    /// `pdf_font_auto_expand`
    pub auto: bool,
    /// `pdf_font_stretch`, `pdf_font_shrink`: the font at its limits.
    pub stretch: i32,
    pub shrink: i32,
    /// `pdf_font_expand_ratio`: how much this font is expanded.
    pub ratio: i32,
    /// `pdf_font_blink`: the font an expanded font was expanded from.
    pub blink: i32,
    /// `pdf_font_elink`: the next expanded version of a base font.
    pub elink: i32,
}

partex_engine::persist_struct!(Expand {
    step,
    auto,
    stretch,
    shrink,
    ratio,
    blink,
    elink
});

/// The character code tables of a font, indexed by [`Code`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Codes {
    tables: [Option<Arc<[i32; 256]>>; 8],
    /// `XeTeX`'s `\lpcode` and `\rpcode` (`hz.cpp`'s maps, by code and
    /// side: a TFM font's character or a native font's glyph, and the
    /// value as assigned).
    xetex: Option<Arc<alloc::collections::BTreeMap<(u32, bool), i32>>>,
}

partex_engine::persist_struct!(Codes { tables, xetex });

/// pdfTeX's per-character codes of a font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    /// `\lpcode`: left protrusion, in thousandths of an em.
    Lp,
    /// `\rpcode`
    Rp,
    /// `\efcode`: how much of the font's expansion the character takes.
    Ef,
    /// `\knbscode`, `\stbscode`, `\shbscode`: interword glue after it.
    KnBs,
    StBs,
    ShBs,
    /// `\knbccode`, `\knaccode`: kerns before and after it.
    KnBc,
    KnAc,
}

impl Code {
    /// The code of an `assign_font_int` primitive (pdfTeX's `lp_code_base`
    /// …), if it is one of these.
    pub(crate) fn of_chr(chr: i32) -> Option<Code> {
        use partex_engine::web::{
            EF_CODE_BASE, KN_AC_CODE_BASE, KN_BC_CODE_BASE, KN_BS_CODE_BASE, LP_CODE_BASE,
            RP_CODE_BASE, SH_BS_CODE_BASE, ST_BS_CODE_BASE,
        };
        Some(match chr {
            LP_CODE_BASE => Code::Lp,
            RP_CODE_BASE => Code::Rp,
            EF_CODE_BASE => Code::Ef,
            KN_BS_CODE_BASE => Code::KnBs,
            ST_BS_CODE_BASE => Code::StBs,
            SH_BS_CODE_BASE => Code::ShBs,
            KN_BC_CODE_BASE => Code::KnBc,
            KN_AC_CODE_BASE => Code::KnAc,
            _ => return None,
        })
    }

    /// The value of a character with no table (pdfTeX's `get_ef_code`
    /// and the rest).
    fn default(self) -> i32 {
        if self == Code::Ef { 1000 } else { 0 }
    }

    /// pdfTeX's `set_lp_code` …: the range values are clamped to.
    fn range(self) -> (i32, i32) {
        if self == Code::Ef {
            (0, 1000)
        } else {
            (-1000, 1000)
        }
    }
}

partex_engine::persist_struct!(FontData {
    metrics,
    tfm,
    name,
    area,
    glue,
    used,
    hyphen_char,
    skew_char,
    codes,
    expand,
    order,
    rank,
    expanded,
    ident,
    retagged,
    order_hash,
    idv,
    remade,
    code_sum,
    num,
    count,
    native,
    native_dir
});

impl FontArrays {
    pub(crate) fn new(font_max: i32) -> Self {
        Self {
            data: crate::cow::Shared::new(FontData::new(font_max)),
            order_read: crate::relaxed::Flag::default(),
        }
    }

    /// The loaded fonts but the null font, in the order they were loaded
    /// (observing the order).
    pub(crate) fn in_order(&self) -> Vec<i32> {
        self.order_read.set(true);
        self.loaded_fonts()
    }

    /// Font `f`'s field `k` (`track::font`) as a version, made from its
    /// content (DESIGN 7.17.12, the tables' convention): what the writing
    /// accessor stores beside it and check mode's test of a read.
    pub(crate) fn field_version(&self, f: i32, k: u32) -> u128 {
        let i = fx(f);
        if i >= self.metrics.len() {
            return Version::node(0x6e6f_6e65, &[Version(u128::from(k))]).0;
        }
        if k == field::NUMBER {
            return Version::node(
                0x006e_756d,
                &[Version(u128::from(self.num[i].cast_unsigned()))],
            )
            .0;
        }
        let v = match k {
            field::METRICS if self.remade[i] => Version::node(
                0x6d31,
                &[
                    Version(self.idv[i]),
                    Version(metrics_version(&self.metrics[i])),
                    Version(u128::from(self.name[i].cast_unsigned())),
                ],
            ),
            field::METRICS => Version::node(0x6d30, &[Version(self.idv[i])]),
            field::PARAMS => Version::of(&self.metrics[i].params),
            field::HYPHEN_CHAR => Version::of(&(2u8, self.hyphen_char[i])),
            field::SKEW_CHAR => Version::of(&(3u8, self.skew_char[i])),
            field::EXPAND => {
                // (with the font's chain of expanded fonts, pdfTeX's
                // `pdf_font_elink`)
                let chain: Vec<(i32, i32)> = self
                    .expanded
                    .range((f, i32::MIN)..=(f, i32::MAX))
                    .map(|(&(_, r), &k)| (r, k))
                    .collect();
                Version::of(&(self.expand[i], chain))
            }
            field::GLUE => Version::of(&(5u8, self.glue[i])),
            field::NATIVE_DIR => Version::of(&(7u8, self.native_dir[i])),
            c => {
                let c = usize::try_from(c.saturating_sub(field::CODES))
                    .unwrap_or(0)
                    .min(7);
                Version::node(
                    0x636f_6465,
                    &[
                        Version(u128::from(u32::try_from(c).unwrap_or(0))),
                        Version(self.code_sum[i][c]),
                    ],
                )
            }
        };
        v.0
    }

    /// The table of loaded fonts as a version: the fonts in order, with
    /// `font_ptr` and `fmem_ptr` (§549, §580).
    pub(crate) fn table_version(&self, font_ptr: i32, fmem_ptr: i32) -> u128 {
        Version::node(
            0x7461_626c,
            &[
                Version(self.order.version()),
                Version(u128::from(font_ptr.cast_unsigned())),
                Version(u128::from(fmem_ptr.cast_unsigned())),
            ],
        )
        .0
    }

    /// Font `f`'s number as tex.web gives it (the fonts loaded before it,
    /// §576): pdfTeX's `/F` name, DVI's font number.
    #[inline]
    pub(crate) fn number(&self, f: i32) -> i32 {
        self.order_read.set(true);
        self.rank.get(fx(f)).copied().unwrap_or(f)
    }

    /// The font loaded last (§580: only its parameters can be added to).
    pub(crate) fn last_loaded(&self) -> i32 {
        self.order_read.set(true);
        self.order.last().unwrap_or(0)
    }
}

impl FontData {
    fn new(font_max: i32) -> Self {
        let n = usize::try_from(font_max).unwrap_or(0) + 1;
        Self {
            metrics: Vec::with_capacity(n),
            tfm: Vec::with_capacity(n),
            name: Vec::with_capacity(n),
            area: Vec::with_capacity(n),
            glue: Vec::with_capacity(n),
            used: Vec::with_capacity(n),
            hyphen_char: Vec::with_capacity(n),
            skew_char: Vec::with_capacity(n),
            codes: Vec::with_capacity(n),
            expand: Vec::with_capacity(n),
            order: FontOrder::default(),
            rank: Vec::with_capacity(n),
            expanded: alloc::collections::BTreeMap::new(),
            ident: Vec::with_capacity(n),
            retagged: Vec::with_capacity(n),
            order_hash: 0,
            idv: Vec::with_capacity(n),
            remade: Vec::with_capacity(n),
            code_sum: Vec::with_capacity(n),
            num: Vec::with_capacity(n),
            count: 0,
            native: Vec::with_capacity(n),
            native_dir: Vec::with_capacity(n),
        }
    }

    /// Add a font at slot `slot` (unloaded: what `Host::font_slot` gave),
    /// the next loaded (its number, tex.web's, is the fonts loaded so
    /// far).
    #[allow(
        clippy::too_many_arguments,
        reason = "a font's parts, as §560 reads them"
    )]
    pub(crate) fn place(
        &mut self,
        slot: i32,
        font: Font,
        tfm: Arc<[u8]>,
        name: i32,
        area: i32,
        ident: u128,
        idv: u128,
    ) {
        let i = fx(slot);
        self.ensure(i);
        self.idv[i] = idv;
        self.remade[i] = false;
        self.code_sum[i] = [0; 8];
        self.metrics[i] = Arc::new(font);
        self.tfm[i] = tfm;
        self.name[i] = name;
        self.area[i] = area;
        self.glue[i] = None;
        self.used[i] = false;
        self.hyphen_char[i] = 0;
        self.skew_char[i] = 0;
        self.codes[i] = Codes::default();
        self.expand[i] = Expand::default();
        self.ident[i] = ident;
        self.retagged[i] = false;
        self.native[i] = None;
        self.native_dir[i] = 0;
        self.push_order(slot);
    }

    /// Room for slot `i` (unloaded slots hold the null font).
    pub(crate) fn ensure(&mut self, i: usize) {
        if self.metrics.len() <= i {
            let null = self
                .metrics
                .first()
                .cloned()
                .unwrap_or_else(|| Arc::new(Font::null()));
            let n = i + 1;
            self.metrics.resize(n, null);
            self.tfm.resize(n, Arc::from([]));
            self.name.resize(n, 0);
            self.area.resize(n, 0);
            self.glue.resize(n, None);
            self.used.resize(n, false);
            self.hyphen_char.resize(n, 0);
            self.skew_char.resize(n, 0);
            self.codes.resize(n, Codes::default());
            self.expand.resize(n, Expand::default());
            self.rank.resize(n, 0);
            self.ident.resize(n, 0);
            self.retagged.resize(n, false);
            self.idv.resize(n, 0);
            self.remade.resize(n, false);
            self.code_sum.resize(n, [0; 8]);
            self.num.resize(n, 0);
            self.native.resize(n, None);
            self.native_dir.resize(n, 0);
        }
    }

    /// Slot `slot`'s name is `name` now (pdfTeX's `letter_space_font`):
    /// the table holds the font by its identity and this name.
    pub(crate) fn rename(&mut self, slot: i32, name: i32) {
        let i = fx(slot);
        self.name[i] = name;
        if let Some(k) = self
            .rank
            .get(i)
            .and_then(|&r| usize::try_from(r).ok())
            .filter(|&k| self.order.get(k) == Some(slot))
        {
            let idv = Version::node(
                0x6e61_6d65,
                &[
                    Version(self.idv[i]),
                    Version(u128::from(name.cast_unsigned())),
                ],
            )
            .0;
            self.order.0.set(k, Loaded::new(slot, idv));
        }
    }

    /// Slot `slot` is the next font loaded.
    pub(crate) fn push_order(&mut self, slot: i32) {
        use core::hash::Hash;
        let i = fx(slot);
        if self.rank.len() <= i {
            self.rank.resize(i + 1, 0);
        }
        self.rank[i] = i32::try_from(self.order.len()).unwrap_or(i32::MAX);
        let idv = self.idv.get(i).copied().unwrap_or(0);
        self.order.push(slot, idv);
        let mut h = partex_engine::stablehash::StableHasher::new();
        (self.order_hash, slot).hash(&mut h);
        self.order_hash = h.finish128();
    }

    /// Take `other`'s order of loading (a machine's `FontOrder`, which
    /// accumulates), each slot's number following it.
    pub(crate) fn take_order_from(&mut self, other: &Self) {
        for f in self.order.slots(0) {
            if let Some(r) = self.rank.get_mut(fx(f)) {
                *r = 0;
            }
        }
        self.order.clone_from(&other.order);
        self.order_hash = other.order_hash;
        for (k, f) in self.order.slots(0).into_iter().enumerate() {
            let i = fx(f);
            if self.rank.len() <= i {
                self.rank.resize(i + 1, 0);
            }
            self.rank[i] = i32::try_from(k).unwrap_or(i32::MAX);
        }
    }

    /// The loaded fonts but the null font, in the order they were loaded,
    /// for a search whose result does not depend on it but among fonts of
    /// one name (read as a machine's `FontName` cell).
    pub(crate) fn loaded_fonts(&self) -> Vec<i32> {
        self.order.slots(1)
    }

    /// Whether slot `f` holds a loaded font.
    pub(crate) fn loaded(&self, f: i32) -> bool {
        f == 0 || self.rank.get(fx(f)).is_some_and(|&r| r > 0)
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::new(0);
    }

    /// The metrics of font `f`.
    #[inline]
    pub(crate) fn get(&self, f: i32) -> &Font {
        &self.metrics[fx(f)]
    }

    /// Code `code` of character `c` of font `f` (pdfTeX's `get_lp_code`
    /// …).
    /// An expanded font has its base font's codes (pdfTeX shares the
    /// tables).
    pub(crate) fn code(&self, f: i32, code: Code, c: u8) -> i32 {
        self.codes[fx(self.base(f))].tables[code as usize]
            .as_ref()
            .map_or(code.default(), |t| t[usize::from(c)])
    }

    /// The font `f` was expanded from, or `f`.
    pub(crate) fn base(&self, f: i32) -> i32 {
        match self.expand[fx(f)].blink {
            0 => f,
            b => b,
        }
    }

    /// Whether font `f` has a table for `code`.
    pub(crate) fn has_codes(&self, f: i32, code: Code) -> bool {
        self.codes[fx(f)].tables[code as usize].is_some()
    }

    /// pdfTeX's `set_lp_code` …
    /// The table's version sum moves by the entry's (a default entry adds
    /// nothing, so a table made by assigning defaults is the absent one).
    pub(crate) fn set_code(&mut self, f: i32, code: Code, c: u8, v: i32) {
        let (lo, hi) = code.range();
        let t = self.codes[fx(f)].tables[code as usize]
            .get_or_insert_with(|| Arc::new([code.default(); 256]));
        let e = &mut Arc::make_mut(t)[usize::from(c)];
        let (old, new) = (*e, v.clamp(lo, hi));
        *e = new;
        let entry = |x: i32| {
            if x == code.default() {
                0
            } else {
                Version::of(&(c, x)).0
            }
        };
        let sum = &mut self.code_sum[fx(f)][code as usize];
        *sum = sum.wrapping_sub(entry(old)).wrapping_add(entry(new));
    }

    /// `XeTeX`'s `get_cp_code(f, code, side)`: the `\lpcode` (or
    /// `\rpcode` if `right`) of character or glyph `code`, 0 if none was
    /// assigned.
    pub(crate) fn cp_code(&self, f: i32, code: u32, right: bool) -> i32 {
        self.codes[fx(f)]
            .xetex
            .as_ref()
            .and_then(|m| m.get(&(code, right)).copied())
            .unwrap_or(0)
    }

    /// `XeTeX`'s `set_cp_code(f, code, side, v)` (any value: `XeTeX`
    /// clamps none). The version sum moves as [`Self::set_code`]'s.
    pub(crate) fn set_cp_code(&mut self, f: i32, code: u32, right: bool, v: i32) {
        let m = self.codes[fx(f)].xetex.get_or_insert_with(Arc::default);
        let old = Arc::make_mut(m).insert((code, right), v).unwrap_or(0);
        let entry = |x: i32| {
            if x == 0 { 0 } else { Version::of(&(code, x)).0 }
        };
        let sum = &mut self.code_sum[fx(f)][usize::from(right)];
        *sum = sum.wrapping_sub(entry(old)).wrapping_add(entry(v));
    }

    /// The metrics of font `f`, to change (copied first if shared).
    pub(crate) fn metrics_mut(&mut self, f: i32) -> &mut Font {
        Arc::make_mut(&mut self.metrics[fx(f)])
    }

    /// `\fontdimen` storage of font `f` (copied first if shared).
    pub(crate) fn params_mut(&mut self, f: i32) -> &mut Vec<Scaled> {
        &mut Arc::make_mut(&mut self.metrics[fx(f)]).params
    }
}

impl Fonts for FontArrays {
    fn font(&self, f: FontId) -> &Font {
        &self.metrics[usize::from(f.0)]
    }
}

/// The fonts as packaging reads them (§649, §668, §719, §800): each
/// font's metrics a read of the value (DESIGN 7.17.12).
pub(crate) struct TrackedFonts<'a, T: Tracker> {
    pub(crate) fonts: &'a FontArrays,
    pub(crate) tracker: &'a T,
}

impl<T: Tracker> Fonts for TrackedFonts<'_, T> {
    #[inline]
    fn font(&self, f: FontId) -> &Font {
        if T::VALUES {
            let g = i32::from(f.0);
            self.tracker.row_read(font_row(g, field::METRICS), || {
                self.fonts.field_version(g, field::METRICS)
            });
        }
        &self.fonts.metrics[usize::from(f.0)]
    }
}

/// Field `k` of font slot `f` as a row.
#[inline]
pub(crate) fn font_row(f: i32, k: u32) -> Row {
    Row::Font(u32::try_from(f).unwrap_or(0) * field::FIELDS + k)
}

/// The version of a font's metrics apart from its parameters (a font
/// changed in place: its tags, its widths).
fn metrics_version(font: &Font) -> u128 {
    let glyphs: Vec<_> = (font.bc..=font.ec).map(|c| font.glyph(c)).collect();
    Version::of(&(
        font.check,
        font.design_size,
        font.size,
        font.bc,
        font.ec,
        glyphs,
        &font.lig_kerns,
        &font.kerns,
        &font.extensibles,
        font.bchar,
        font.false_bchar,
        font.bchar_label,
    ))
    .0
}

/// The fields of a font slot, each a value.
const FIELDS: [u32; 14] = [
    field::METRICS,
    field::PARAMS,
    field::HYPHEN_CHAR,
    field::SKEW_CHAR,
    field::EXPAND,
    field::GLUE,
    field::CODES,
    field::CODES + 1,
    field::CODES + 2,
    field::CODES + 3,
    field::CODES + 4,
    field::CODES + 5,
    field::CODES + 6,
    field::CODES + 7,
];

/// Index a font array.
#[inline]
pub(crate) fn fx(f: i32) -> usize {
    usize::try_from(f).expect("negative font number")
}

/// The engine's name for font `f`.
#[inline]
pub(crate) fn font_id(f: i32) -> FontId {
    FontId(u16::try_from(f).expect("font number"))
}

/// What a font is, for [`Host::font_slot`]: loaded from a TFM file (its
/// contents, name, area and size), expanded from a font by a ratio, or
/// copied from one.
pub(crate) enum FontIdent<'a> {
    Tfm {
        tfm: &'a [u8],
        name: &'a [u8],
        area: &'a [u8],
        size: Scaled,
    },
    Expanded {
        base: i32,
        ratio: i32,
    },
    Copied {
        from: i32,
    },
    /// `XeTeX`'s native font: its face (by content), its full name with
    /// its options, its size.
    Native {
        face: ([u64; 2], u64, u32),
        name: &'a [u8],
        size: Scaled,
    },
}

impl FontIdent<'_> {
    /// The identity by content: a TFM font's is its hash; an expanded or
    /// copied font's is made from its base font's identity (whose slot
    /// may differ between runs).
    pub(crate) fn content(&self, fonts: &FontData) -> u128 {
        let of = |f: i32| fonts.idv.get(fx(f)).copied().unwrap_or(0);
        match self {
            FontIdent::Tfm { .. } | FontIdent::Native { .. } => self.hash(),
            FontIdent::Expanded { base, ratio } => {
                Version::node(
                    0x6578_7061,
                    &[
                        Version(of(*base)),
                        Version(u128::from(ratio.cast_unsigned())),
                    ],
                )
                .0
            }
            FontIdent::Copied { from } => Version::node(0x636f_7079, &[Version(of(*from))]).0,
        }
    }

    fn hash(&self) -> u128 {
        use core::hash::Hash;
        let mut h = partex_engine::stablehash::StableHasher::new();
        match self {
            FontIdent::Tfm {
                tfm,
                name,
                area,
                size,
            } => (
                0u8,
                partex_engine::stablehash::StableHasher::of(*tfm),
                name,
                area,
                size,
            )
                .hash(&mut h),
            FontIdent::Expanded { base, ratio } => (1u8, base, ratio).hash(&mut h),
            FontIdent::Copied { from } => (2u8, from).hash(&mut h),
            FontIdent::Native { face, name, size } => (3u8, face, name, size).hash(&mut h),
        }
        h.finish128()
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The slot for a new font of identity `id` ([`Host::font_slot`]; in
    /// INITEX, whose fonts a format keeps in order, the next number), and
    /// the identity to keep with it. Two fonts of the same identity
    /// loaded in one run (a font loaded twice, not found by §1260's
    /// search) are told apart by how many came before.
    /// With them, the identity by content (the version of the metrics,
    /// [`FontIdent::content`]).
    pub(crate) fn new_font_slot(&mut self, id: &FontIdent<'_>, name: &[u8]) -> (i32, u128, u128) {
        let ident = id.hash();
        let idv = match id {
            FontIdent::Tfm { .. } | FontIdent::Native { .. } => ident,
            _ => id.content(&self.fonts),
        };
        let fresh = self.font_ptr + 1;
        if self.params.ini {
            return (fresh, ident, idv);
        }
        // (the fonts of an identity have one name: those read)
        self.font_touch(FontTouch::Name(Arc::from(name), false));
        let dup = self
            .fonts
            .loaded_fonts()
            .into_iter()
            .filter(|&f| self.fonts.ident.get(fx(f)) == Some(&ident))
            .count();
        let key = {
            use core::hash::Hash;
            let mut h = partex_engine::stablehash::StableHasher::new();
            (ident, dup).hash(&mut h);
            h.finish128()
        };
        (self.host.font_slot(key, fresh), ident, idv)
    }

    /// A read of font `f`'s field `k` (`track::font`): the version beside
    /// it (DESIGN 7.17.12, the tables' convention).
    #[inline]
    pub(crate) fn font_read(&self, f: i32, k: u32) {
        if T::VALUES {
            self.tracker
                .row_read(font_row(f, k), || self.fonts.field_version(f, k));
        }
    }

    /// Font `f`'s field `k` was written: its version from the content
    /// written (an equal write gives the same version).
    #[inline]
    pub(crate) fn font_wrote(&self, f: i32, k: u32) {
        if T::VALUES {
            self.tracker
                .row_wrote(font_row(f, k), self.fonts.field_version(f, k));
        }
    }

    /// Font slot `f` was made (loaded, expanded, copied): its fields, and
    /// the table it joined.
    pub(crate) fn font_made(&mut self, f: i32) {
        self.tracker.font_loaded(f);
        if f != NULL_FONT {
            // (its number: the next after the last font made, in program
            // order, §576)
            let row = Row::Scalar(crate::track::scalar::FONT_COUNT);
            self.scalar_read(row, self.fonts.count);
            self.fonts.count += 1;
            let n = self.fonts.count;
            self.scalar_wrote(row, n);
            if let Some(x) = self.fonts.num.get_mut(fx(f)) {
                *x = n;
            }
            self.font_wrote(f, field::NUMBER);
        }
        if T::VALUES {
            for k in FIELDS {
                self.font_wrote(f, k);
            }
            self.font_table_wrote();
        }
    }

    /// A read of the table of loaded fonts (which fonts, in what order, by
    /// what identity; `font_ptr`, `fmem_ptr`).
    #[inline]
    pub(crate) fn font_table_read(&self) {
        if T::VALUES {
            self.tracker.row_read(Row::FontTable, || {
                self.fonts.table_version(self.font_ptr, self.fmem_ptr)
            });
        }
    }

    /// The table of loaded fonts was written.
    pub(crate) fn font_table_wrote(&self) {
        if T::VALUES {
            self.tracker.row_wrote(
                Row::FontTable,
                self.fonts.table_version(self.font_ptr, self.fmem_ptr),
            );
        }
    }

    /// Every font slot's fields and the table as versions, for a writer
    /// that stores the fonts wholesale past the accessors (a format's
    /// load, §1321; the engine as made): `Tex::version_tables`.
    pub(crate) fn version_fonts(&self) {
        if !T::VALUES {
            return;
        }
        for f in 0..i32::try_from(self.fonts.metrics.len()).unwrap_or(0) {
            for k in FIELDS {
                self.tracker
                    .row_made(font_row(f, k), self.fonts.field_version(f, k));
            }
        }
        self.tracker.row_made(
            Row::FontTable,
            self.fonts.table_version(self.font_ptr, self.fmem_ptr),
        );
    }

    /// The fonts as packaging reads them, each font's metrics a read.
    #[inline]
    pub(crate) fn tracked_fonts(&self) -> TrackedFonts<'_, T> {
        TrackedFonts {
            fonts: &self.fonts,
            tracker: &self.tracker,
        }
    }

    /// Log a machine's read or write of the fonts by name or of an
    /// expanded font (with `font_cells`).
    #[inline]
    pub(crate) fn font_touch(&mut self, t: FontTouch) {
        if self.font_cells {
            self.font_log.push(t);
        }
    }

    /// Font slot `f` was loaded (next): with fonts' numbers the link's,
    /// an effect (`Effect::FontLoad`).
    pub(crate) fn font_loaded(&mut self, f: i32) {
        if self.pdf.out.font_refs {
            self.out_mark(crate::effects::Effect::FontLoad(f));
        }
    }

    /// A font named `name` was loaded.
    pub(crate) fn font_named(&mut self, name: i32) {
        if self.font_cells {
            let b = Arc::from(self.str_bytes(crate::input::ux(name)));
            self.font_log.push(FontTouch::Name(b, true));
        }
    }

    /// §552 (moved to web2c's `main_body`): the null font, in INITEX.
    pub(crate) fn init_null_font(&mut self) {
        self.font_ptr = NULL_FONT;
        self.fmem_ptr = 7;
        let nullfont = self.pool_str(b"nullfont");
        let empty = self.pool_str(b"");
        self.fonts.clear();
        let idv = Version::node(0x6e75_6c6c, &[]).0;
        self.fonts.place(
            NULL_FONT,
            Font::null(),
            Arc::from([]),
            nullfont,
            empty,
            0,
            idv,
        );
        let f = fx(NULL_FONT);
        self.fonts.hyphen_char[f] = i32::from(b'-');
        self.fonts.skew_char[f] = -1;
        self.font_made(NULL_FONT);
    }
}
