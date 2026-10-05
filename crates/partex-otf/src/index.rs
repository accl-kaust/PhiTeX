//! The font index: every face a job may find by name, with its raw name
//! records and the few values the name matching reads, in the order of
//! the font list (fontconfig's `FcFontList` order for TeX Live's faces,
//! which the XeTeX ruleset depends on). No matching here: the rules are
//! [`crate::xetex::fontmgr`]'s (and a luaotfload ruleset could pick
//! differently from the same index).
//!
//! # Building it
//!
//! `scripts/sandbox cargo run -p partex-otf --example otf-index --release -- OUT`
//! runs `fc-list` with `scripts/xetex/fonts.conf` (TeX Live's
//! `fonts/opentype` and `fonts/truetype`, as the oracle sees them), reads
//! every listed face's bytes from its absolute TeX Live path, and writes
//! [`FontIndex::to_bytes`]. Paths in the index are those absolute paths.
//!
//! # The format (version 1, stable)
//!
//! All integers big-endian.
//! ```text
//! "PXFI" u16 version=1
//! u32 npaths, then per path: u16 len, UTF-8 bytes
//! u32 nentries, then per entry:
//!   u32 path index, u32 face index, u16 flags
//!   if flags & HAS_OS2:  u16 weight, u16 width, u16 fs_selection
//!   if flags & HAS_HEAD: u16 mac_style
//!   i32 italic angle (16.16; 0 without post)
//!   if flags & HAS_SIZE: u16 design, subfamily id, name id, range start, end
//!   if flags & HAS_FC:   u16 weight, u16 width, u16 slant (fontconfig's, for
//!                        a face without OS/2 weight and width)
//!   if flags & HAS_INSTANCE: u16 the named instance's subfamily name id
//!   u16 nrecords, then per record: u16 platform, encoding, language,
//!   name id, u16 len, bytes
//! ```

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::face::Face;
use crate::layout::{SizeParams, size_params_in};
use crate::names::{INDEXED_IDS, NameRecord, read_name_table};
use crate::{Tag, tag};

const MAGIC: &[u8; 4] = b"PXFI";
const VERSION: u16 = 1;

/// The face can be opened (FreeType opens it and it is scalable).
pub const LOADABLE: u16 = 1;
/// The face is an sfnt (`FT_IS_SFNT`): its names come from its name table.
pub const SFNT: u16 = 2;
const HAS_OS2: u16 = 4;
const HAS_HEAD: u16 = 8;
const HAS_SIZE: u16 = 16;
const HAS_FC: u16 = 32;
/// The file is WOFF or WOFF2 (FreeType reads them; this crate's faces not
/// yet).
pub const WOFF: u16 = 64;
/// fontconfig's entry for a variable font itself (its `0x8000…` id, listed
/// at index 0, beside the default instance): no style or full name. Set by
/// the index builder from `fc-list`.
pub const VARIABLE: u16 = 128;
const HAS_INSTANCE: u16 = 256;
const KEPT: u16 = LOADABLE | SFNT | WOFF | VARIABLE;

/// One face in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceEntry {
    /// The file's absolute path.
    pub path: Arc<str>,
    /// The face index (`FC_INDEX`; a named instance in the high 16 bits).
    pub index: u32,
    /// [`LOADABLE`], [`SFNT`], [`WOFF`].
    pub flags: u16,
    /// OS/2's `usWeightClass`, `usWidthClass`, `fsSelection`.
    pub os2: Option<(u16, u16, u16)>,
    /// head's `macStyle`.
    pub mac_style: Option<u16>,
    /// post's `italicAngle` (16.16), 0 without a post table.
    pub italic_angle: i32,
    /// The `size` feature (`hb_ot_layout_get_size_params`).
    pub size: Option<SizeParams>,
    /// fontconfig's weight, width and slant (only where XeTeX reads them:
    /// a face whose OS/2 weight and width are both 0 or missing).
    pub fc_style: Option<(u16, u16, u16)>,
    /// A named instance's subfamily name id (`fvar`), for an index with a
    /// named instance in its high 16 bits; its records are in `names`.
    /// (Named instances are not loadable here: no variations.)
    pub instance_subfamily: Option<u16>,
    /// Name records 1, 2, 4, 6, 16 and 17 as FreeType reads them.
    pub names: Vec<NameRecord>,
}

impl FaceEntry {
    /// The entry of face `index` of the font file `data` at `path`, read as
    /// the index builder does (for a project's own fonts, put in front of
    /// the index at run time).
    #[must_use]
    pub fn read(path: &str, data: Arc<[u8]>, index: u32) -> FaceEntry {
        let loadable = Face::new(data.clone(), index).is_some();
        let woff = crate::woff::is_woff(&data) || crate::woff::is_woff2(&data);
        let sfnt: Arc<[u8]> = if woff {
            crate::woff::unpack(&data).map_or(data, Arc::from)
        } else {
            data
        };
        let dir = crate::face::sfnt_offset(&sfnt, index & 0xFFFF)
            .and_then(|o| crate::face::table_directory(&sfnt, o))
            .unwrap_or_default();
        let table = |t: Tag| -> Option<&[u8]> {
            let &(_, o, l) = dir.iter().find(|e| e.0 == t)?;
            if t == tag(b"OS/2") {
                // read as FreeType does, up to the file's end
                return sfnt.get(o as usize..);
            }
            sfnt.get(o as usize..(o + l) as usize)
        };
        let mut e = FaceEntry::from_tables(path, index, loadable && index >> 16 == 0, &table);
        if woff {
            e.flags |= WOFF;
        }
        e
    }

    /// The entry of a face given by its tables (`table(tag)`), opened by
    /// FreeType or not (`loadable`).
    #[must_use]
    pub fn from_tables<'t>(
        path: &str,
        index: u32,
        loadable: bool,
        table: &dyn Fn(Tag) -> Option<&'t [u8]>,
    ) -> FaceEntry {
        let mut e = FaceEntry {
            path: Arc::from(path),
            index,
            flags: 0,
            os2: None,
            mac_style: None,
            italic_angle: 0,
            size: None,
            fc_style: None,
            instance_subfamily: None,
            names: table(tag(b"name"))
                .map(|d| read_name_table(d, &INDEXED_IDS))
                .unwrap_or_default(),
        };
        if let Some(fvar) = table(tag(b"fvar")) {
            let rd = |o: usize| crate::face::rd_u16(fvar, o);
            let (axes_off, axes, instances, isize) = (rd(4), rd(8), rd(12), rd(14));
            if let (Some(ao), Some(n_axes), Some(n_inst), Some(isz)) =
                (axes_off, axes, instances, isize)
                && n_axes > 0
            {
                let k = (index >> 16) as usize;
                if k >= 1 && k <= n_inst as usize {
                    let rec = ao as usize + 20 * n_axes as usize + (k - 1) * isz as usize;
                    if let Some(strid) = rd(rec) {
                        e.instance_subfamily = Some(strid);
                        if let Some(d) = table(tag(b"name")) {
                            e.names.extend(read_name_table(d, &[strid]));
                        }
                    }
                }
            }
        }
        if loadable {
            e.flags |= LOADABLE | SFNT;
            e.os2 = table(tag(b"OS/2"))
                .and_then(crate::face::read_os2)
                .map(|o| (o.weight_class, o.width_class, o.fs_selection));
            e.mac_style = table(tag(b"head")).and_then(|h| crate::face::rd_u16(h, 44));
            e.italic_angle = table(tag(b"post"))
                .filter(|p| p.len() >= 32)
                .and_then(|p| crate::face::rd_u32(p, 4))
                .map_or(0, |v| v as i32);
            e.size = table(tag(b"GPOS")).and_then(size_params_in);
        }
        e
    }
}

/// The face list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontIndex {
    pub entries: Vec<FaceEntry>,
}

struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u16(b.len() as u16);
        self.0.extend_from_slice(b);
    }
}

struct Reader<'a>(&'a [u8], usize);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.0.get(self.1..self.1 + n)?;
        self.1 += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u16()? as usize;
        self.take(n).map(<[u8]>::to_vec)
    }
}

impl FontIndex {
    /// The index with `faces` (a project's own fonts) in front.
    #[must_use]
    pub fn with_front(&self, faces: Vec<FaceEntry>) -> FontIndex {
        let mut entries = faces;
        entries.extend(self.entries.iter().cloned());
        FontIndex { entries }
    }

    /// The serialized index (see the module documentation).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u16(VERSION);
        let mut paths: Vec<Arc<str>> = Vec::new();
        let mut path_ix = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let i = if let Some(i) = paths.iter().position(|p| *p == e.path) {
                i
            } else {
                paths.push(e.path.clone());
                paths.len() - 1
            };
            path_ix.push(i as u32);
        }
        w.u32(paths.len() as u32);
        for p in &paths {
            w.bytes(p.as_bytes());
        }
        w.u32(self.entries.len() as u32);
        for (e, &pi) in self.entries.iter().zip(&path_ix) {
            let mut flags = e.flags & KEPT;
            if e.instance_subfamily.is_some() {
                flags |= HAS_INSTANCE;
            }
            if e.os2.is_some() {
                flags |= HAS_OS2;
            }
            if e.mac_style.is_some() {
                flags |= HAS_HEAD;
            }
            if e.size.is_some() {
                flags |= HAS_SIZE;
            }
            if e.fc_style.is_some() {
                flags |= HAS_FC;
            }
            w.u32(pi);
            w.u32(e.index);
            w.u16(flags);
            if let Some((a, b, c)) = e.os2 {
                w.u16(a);
                w.u16(b);
                w.u16(c);
            }
            if let Some(m) = e.mac_style {
                w.u16(m);
            }
            w.u32(e.italic_angle as u32);
            if let Some(s) = e.size {
                for v in [
                    s.design_size,
                    s.subfamily_id,
                    s.subfamily_name_id,
                    s.range_start,
                    s.range_end,
                ] {
                    w.u16(v);
                }
            }
            if let Some((a, b, c)) = e.fc_style {
                w.u16(a);
                w.u16(b);
                w.u16(c);
            }
            if let Some(i) = e.instance_subfamily {
                w.u16(i);
            }
            w.u16(e.names.len() as u16);
            for r in &e.names {
                w.u16(r.platform);
                w.u16(r.encoding);
                w.u16(r.language);
                w.u16(r.name_id);
                w.bytes(&r.bytes);
            }
        }
        w.0
    }

    /// Reads [`FontIndex::to_bytes`]'s output; `None` if it is not one.
    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<FontIndex> {
        let mut r = Reader(b, 0);
        if r.take(4)? != MAGIC || r.u16()? != VERSION {
            return None;
        }
        let np = r.u32()? as usize;
        let mut paths: Vec<Arc<str>> = Vec::with_capacity(np);
        for _ in 0..np {
            let s = String::from_utf8(r.bytes()?).ok()?;
            paths.push(Arc::from(s.as_str()));
        }
        let ne = r.u32()? as usize;
        let mut entries = Vec::with_capacity(ne);
        for _ in 0..ne {
            let path = paths.get(r.u32()? as usize)?.clone();
            let index = r.u32()?;
            let flags = r.u16()?;
            let os2 = if flags & HAS_OS2 != 0 {
                Some((r.u16()?, r.u16()?, r.u16()?))
            } else {
                None
            };
            let mac_style = if flags & HAS_HEAD != 0 {
                Some(r.u16()?)
            } else {
                None
            };
            let italic_angle = r.u32()? as i32;
            let size = if flags & HAS_SIZE != 0 {
                Some(SizeParams {
                    design_size: r.u16()?,
                    subfamily_id: r.u16()?,
                    subfamily_name_id: r.u16()?,
                    range_start: r.u16()?,
                    range_end: r.u16()?,
                })
            } else {
                None
            };
            let fc_style = if flags & HAS_FC != 0 {
                Some((r.u16()?, r.u16()?, r.u16()?))
            } else {
                None
            };
            let instance_subfamily = if flags & HAS_INSTANCE != 0 {
                Some(r.u16()?)
            } else {
                None
            };
            let n = r.u16()? as usize;
            let mut names = Vec::with_capacity(n);
            for _ in 0..n {
                names.push(NameRecord {
                    platform: r.u16()?,
                    encoding: r.u16()?,
                    language: r.u16()?,
                    name_id: r.u16()?,
                    bytes: r.bytes()?,
                });
            }
            entries.push(FaceEntry {
                path,
                index,
                flags: flags & KEPT,
                os2,
                mac_style,
                italic_angle,
                size,
                fc_style,
                instance_subfamily,
                names,
            });
        }
        Some(FontIndex { entries })
    }
}
