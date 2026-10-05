//! cff.c, cff.h, cff_types.h, cff_limits.h, cff_stdstr.h: reading and
//! writing CFF fonts.
//!
//! A [`CffFont`] is caller-owned (not cached) and owns its stream: C
//! shares one `FILE *` between an sfnt and the CFF opened from it; here
//! the caller passes a clone of its [`MemFile`] (cheap: the bytes are an
//! `Arc`). Functions of a `cff_font` alone are methods of [`CffFont`].
//!
//! Output buffers `(card8 *dest, int destlen)` are `dest: &mut [u8]`
//! (`destlen` = `dest.len()`); the functions return the bytes written,
//! as in C. C pointers to heap structs are `Option<T>` (none = NULL);
//! pointer arrays (`cff_dict **`, `cff_index **`) are `Vec<Option<T>>`
//! (empty = NULL).

use crate::prelude::*;

// cff_types.h

pub const CFF_TYPE_UNKNOWN: i32 = 0;
pub const CFF_TYPE_INTEGER: i32 = 1 << 0;
pub const CFF_TYPE_REAL: i32 = 1 << 1;
pub const CFF_TYPE_NUMBER: i32 = CFF_TYPE_INTEGER | CFF_TYPE_REAL;
pub const CFF_TYPE_BOOLEAN: i32 = 1 << 2;
pub const CFF_TYPE_SID: i32 = 1 << 3;
pub const CFF_TYPE_ARRAY: i32 = 1 << 4;
pub const CFF_TYPE_DELTA: i32 = 1 << 5;
pub const CFF_TYPE_ROS: i32 = 1 << 6;
pub const CFF_TYPE_OFFSET: i32 = 1 << 7;
pub const CFF_TYPE_SZOFF: i32 = 1 << 8;

/// `card8` (cff_types.h).
pub type Card8 = u8;
/// `card16`.
pub type Card16 = u16;
/// `c_offsize`: the size of an offset, 1 to 4.
pub type COffsize = u8;
/// `l_offset`.
pub type LOffset = u32;
/// `s_SID`.
pub type SSid = u16;

/// `cff_index`: `offset` has `count + 1` entries (1-based offsets into
/// `data`); an empty `offset`/`data` is C's NULL.
#[derive(Clone, Debug, Default)]
pub struct CffIndex {
    pub count: Card16,
    pub offsize: COffsize,
    pub offset: Vec<LOffset>,
    pub data: Vec<Card8>,
}

/// `cff_header`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CffHeader {
    pub major: Card8,
    pub minor: Card8,
    pub hdr_size: Card8,
    pub offsize: COffsize,
}

/// `cff_dict_entry`: `key` is the opname from cff_dict.rs's `DICT_OPERATOR`.
#[derive(Clone, Debug, Default)]
pub struct CffDictEntry {
    pub id: i32,
    pub key: &'static [u8],
    pub count: i32,
    pub values: Vec<f64>,
}

/// `cff_dict`: `entries.len() == count`; `max` kept as C grows it.
#[derive(Clone, Debug, Default)]
pub struct CffDict {
    pub max: i32,
    pub count: i32,
    pub entries: Vec<CffDictEntry>,
}

/// `cff_range1`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CffRange1 {
    /// SID or CID, or card8 for Encoding.
    pub first: SSid,
    pub n_left: Card8,
}

/// `cff_range2`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CffRange2 {
    pub first: SSid,
    pub n_left: Card16,
}

/// `cff_map`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CffMap {
    pub code: Card8,
    pub glyph: SSid,
}

/// `cff_encoding`: C's `data` union is `codes` (format 0) or `range1`
/// (format 1); only the one of the format is used.
#[derive(Clone, Debug, Default)]
pub struct CffEncoding {
    /// `if (format & 0x80)` there is a supplement.
    pub format: Card8,
    pub num_entries: Card8,
    pub codes: Vec<Card8>,
    pub range1: Vec<CffRange1>,
    pub num_supps: Card8,
    pub supp: Vec<CffMap>,
}

/// `cff_charsets`: C's `data` union is `glyphs` (format 0), `range1`
/// (1) or `range2` (2).
#[derive(Clone, Debug, Default)]
pub struct CffCharsets {
    pub format: Card8,
    pub num_entries: Card16,
    pub glyphs: Vec<SSid>,
    pub range1: Vec<CffRange1>,
    pub range2: Vec<CffRange2>,
}

/// `cff_range3`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CffRange3 {
    pub first: Card16,
    pub fd: Card8,
}

/// `cff_fdselect`: C's `data` union is `fds` (format 0) or `ranges` (3).
#[derive(Clone, Debug, Default)]
pub struct CffFdselect {
    pub format: Card8,
    pub num_entries: Card16,
    pub fds: Vec<Card8>,
    pub ranges: Vec<CffRange3>,
}

// cff_limits.h

pub const CFF_INT_MAX: i32 = 0x7fff_ffff;
pub const CFF_INT_MIN: i32 = -0x7fff_ffff - 1;
pub const CFF_SID_MAX: i32 = 64999;
pub const CFF_STDSTR_MAX: i32 = 391;
pub const CS_NUM_SUBR_MAX: i32 = 65536;
pub const CS_STR_LEN_MAX: i32 = 65536;
pub const CS_STEM_ZONE_MAX: usize = 96;
pub const CS_ARG_STACK_MAX: usize = 48;
pub const CS_TRANS_ARRAY_MAX: usize = 32;
pub const CS_SUBR_NEST_MAX: i32 = 10;

// cff.h

pub const FONTTYPE_CIDFONT: i32 = 1 << 0;
pub const FONTTYPE_FONT: i32 = 1 << 1;
pub const FONTTYPE_MMASTER: i32 = 1 << 2;
pub const ENCODING_STANDARD: i32 = 1 << 3;
pub const ENCODING_EXPERT: i32 = 1 << 4;
pub const CHARSETS_ISOADOBE: i32 = 1 << 5;
pub const CHARSETS_EXPERT: i32 = 1 << 6;
pub const CHARSETS_EXPSUB: i32 = 1 << 7;
pub const HAVE_STANDARD_ENCODING: i32 = ENCODING_STANDARD | ENCODING_EXPERT;
pub const HAVE_STANDARD_CHARSETS: i32 = CHARSETS_ISOADOBE | CHARSETS_EXPERT | CHARSETS_EXPSUB;
pub const CFF_STRING_NOTDEF: i32 = 65535;

/// `cff_font`.
#[derive(Clone, Debug, Default)]
pub struct CffFont {
    /// FontName.
    pub fontname: Option<Vec<u8>>,
    pub header: CffHeader,
    /// Name INDEX.
    pub name: Option<CffIndex>,
    /// Top DICT (single).
    pub topdict: Option<CffDict>,
    /// String INDEX.
    pub string: Option<CffIndex>,
    /// Global Subr INDEX.
    pub gsubr: Option<CffIndex>,
    pub encoding: Option<CffEncoding>,
    pub charsets: Option<CffCharsets>,
    /// CIDFont only.
    pub fdselect: Option<CffFdselect>,
    /// CharStrings.
    pub cstrings: Option<CffIndex>,
    /// Font DICTs, CIDFont only (`num_fds` of them; empty = NULL).
    pub fdarray: Vec<Option<CffDict>>,
    /// Private DICTs, one per Font DICT (`private` in C).
    pub private: Vec<Option<CffDict>>,
    /// Local Subr INDEXes, one per Private DICT.
    pub subrs: Vec<Option<CffIndex>>,
    /// Non-zero for OpenType or PostScript wrapped.
    pub offset: LOffset,
    pub gsubr_offset: LOffset,
    /// Number of glyphs (CharStrings INDEX count).
    pub num_glyphs: Card16,
    /// Number of Font DICTs.
    pub num_fds: Card8,
    /// The updated String INDEX (`_string`).
    pub _string: Option<CffIndex>,
    /// The file (none for a font t1_load built).
    pub stream: Option<MemFile>,
    /// Not used.
    pub filter: i32,
    /// CFF fontset index.
    pub index: i32,
    /// `FONTTYPE_*`, `ENCODING_*`, `CHARSETS_*`.
    pub flag: i32,
    /// 1 if .notdef is not the first glyph.
    pub is_notdef_notzero: i32,
}

/// `cff_stdstr` (cff_stdstr.h): the CFF standard strings.
pub static CFF_STDSTR: [&[u8]; 391] = [
    b".notdef",
    b"space",
    b"exclam",
    b"quotedbl",
    b"numbersign",
    b"dollar",
    b"percent",
    b"ampersand",
    b"quoteright",
    b"parenleft",
    b"parenright",
    b"asterisk",
    b"plus",
    b"comma",
    b"hyphen",
    b"period",
    b"slash",
    b"zero",
    b"one",
    b"two",
    b"three",
    b"four",
    b"five",
    b"six",
    b"seven",
    b"eight",
    b"nine",
    b"colon",
    b"semicolon",
    b"less",
    b"equal",
    b"greater",
    b"question",
    b"at",
    b"A",
    b"B",
    b"C",
    b"D",
    b"E",
    b"F",
    b"G",
    b"H",
    b"I",
    b"J",
    b"K",
    b"L",
    b"M",
    b"N",
    b"O",
    b"P",
    b"Q",
    b"R",
    b"S",
    b"T",
    b"U",
    b"V",
    b"W",
    b"X",
    b"Y",
    b"Z",
    b"bracketleft",
    b"backslash",
    b"bracketright",
    b"asciicircum",
    b"underscore",
    b"quoteleft",
    b"a",
    b"b",
    b"c",
    b"d",
    b"e",
    b"f",
    b"g",
    b"h",
    b"i",
    b"j",
    b"k",
    b"l",
    b"m",
    b"n",
    b"o",
    b"p",
    b"q",
    b"r",
    b"s",
    b"t",
    b"u",
    b"v",
    b"w",
    b"x",
    b"y",
    b"z",
    b"braceleft",
    b"bar",
    b"braceright",
    b"asciitilde",
    b"exclamdown",
    b"cent",
    b"sterling",
    b"fraction",
    b"yen",
    b"florin",
    b"section",
    b"currency",
    b"quotesingle",
    b"quotedblleft",
    b"guillemotleft",
    b"guilsinglleft",
    b"guilsinglright",
    b"fi",
    b"fl",
    b"endash",
    b"dagger",
    b"daggerdbl",
    b"periodcentered",
    b"paragraph",
    b"bullet",
    b"quotesinglbase",
    b"quotedblbase",
    b"quotedblright",
    b"guillemotright",
    b"ellipsis",
    b"perthousand",
    b"questiondown",
    b"grave",
    b"acute",
    b"circumflex",
    b"tilde",
    b"macron",
    b"breve",
    b"dotaccent",
    b"dieresis",
    b"ring",
    b"cedilla",
    b"hungarumlaut",
    b"ogonek",
    b"caron",
    b"emdash",
    b"AE",
    b"ordfeminine",
    b"Lslash",
    b"Oslash",
    b"OE",
    b"ordmasculine",
    b"ae",
    b"dotlessi",
    b"lslash",
    b"oslash",
    b"oe",
    b"germandbls",
    b"onesuperior",
    b"logicalnot",
    b"mu",
    b"trademark",
    b"Eth",
    b"onehalf",
    b"plusminus",
    b"Thorn",
    b"onequarter",
    b"divide",
    b"brokenbar",
    b"degree",
    b"thorn",
    b"threequarters",
    b"twosuperior",
    b"registered",
    b"minus",
    b"eth",
    b"multiply",
    b"threesuperior",
    b"copyright",
    b"Aacute",
    b"Acircumflex",
    b"Adieresis",
    b"Agrave",
    b"Aring",
    b"Atilde",
    b"Ccedilla",
    b"Eacute",
    b"Ecircumflex",
    b"Edieresis",
    b"Egrave",
    b"Iacute",
    b"Icircumflex",
    b"Idieresis",
    b"Igrave",
    b"Ntilde",
    b"Oacute",
    b"Ocircumflex",
    b"Odieresis",
    b"Ograve",
    b"Otilde",
    b"Scaron",
    b"Uacute",
    b"Ucircumflex",
    b"Udieresis",
    b"Ugrave",
    b"Yacute",
    b"Ydieresis",
    b"Zcaron",
    b"aacute",
    b"acircumflex",
    b"adieresis",
    b"agrave",
    b"aring",
    b"atilde",
    b"ccedilla",
    b"eacute",
    b"ecircumflex",
    b"edieresis",
    b"egrave",
    b"iacute",
    b"icircumflex",
    b"idieresis",
    b"igrave",
    b"ntilde",
    b"oacute",
    b"ocircumflex",
    b"odieresis",
    b"ograve",
    b"otilde",
    b"scaron",
    b"uacute",
    b"ucircumflex",
    b"udieresis",
    b"ugrave",
    b"yacute",
    b"ydieresis",
    b"zcaron",
    b"exclamsmall",
    b"Hungarumlautsmall",
    b"dollaroldstyle",
    b"dollarsuperior",
    b"ampersandsmall",
    b"Acutesmall",
    b"parenleftsuperior",
    b"parenrightsuperior",
    b"twodotenleader",
    b"onedotenleader",
    b"zerooldstyle",
    b"oneoldstyle",
    b"twooldstyle",
    b"threeoldstyle",
    b"fouroldstyle",
    b"fiveoldstyle",
    b"sixoldstyle",
    b"sevenoldstyle",
    b"eightoldstyle",
    b"nineoldstyle",
    b"commasuperior",
    b"threequartersemdash",
    b"periodsuperior",
    b"questionsmall",
    b"asuperior",
    b"bsuperior",
    b"centsuperior",
    b"dsuperior",
    b"esuperior",
    b"isuperior",
    b"lsuperior",
    b"msuperior",
    b"nsuperior",
    b"osuperior",
    b"rsuperior",
    b"ssuperior",
    b"tsuperior",
    b"ff",
    b"ffi",
    b"ffl",
    b"parenleftinferior",
    b"parenrightinferior",
    b"Circumflexsmall",
    b"hyphensuperior",
    b"Gravesmall",
    b"Asmall",
    b"Bsmall",
    b"Csmall",
    b"Dsmall",
    b"Esmall",
    b"Fsmall",
    b"Gsmall",
    b"Hsmall",
    b"Ismall",
    b"Jsmall",
    b"Ksmall",
    b"Lsmall",
    b"Msmall",
    b"Nsmall",
    b"Osmall",
    b"Psmall",
    b"Qsmall",
    b"Rsmall",
    b"Ssmall",
    b"Tsmall",
    b"Usmall",
    b"Vsmall",
    b"Wsmall",
    b"Xsmall",
    b"Ysmall",
    b"Zsmall",
    b"colonmonetary",
    b"onefitted",
    b"rupiah",
    b"Tildesmall",
    b"exclamdownsmall",
    b"centoldstyle",
    b"Lslashsmall",
    b"Scaronsmall",
    b"Zcaronsmall",
    b"Dieresissmall",
    b"Brevesmall",
    b"Caronsmall",
    b"Dotaccentsmall",
    b"Macronsmall",
    b"figuredash",
    b"hypheninferior",
    b"Ogoneksmall",
    b"Ringsmall",
    b"Cedillasmall",
    b"questiondownsmall",
    b"oneeighth",
    b"threeeighths",
    b"fiveeighths",
    b"seveneighths",
    b"onethird",
    b"twothirds",
    b"zerosuperior",
    b"foursuperior",
    b"fivesuperior",
    b"sixsuperior",
    b"sevensuperior",
    b"eightsuperior",
    b"ninesuperior",
    b"zeroinferior",
    b"oneinferior",
    b"twoinferior",
    b"threeinferior",
    b"fourinferior",
    b"fiveinferior",
    b"sixinferior",
    b"seveninferior",
    b"eightinferior",
    b"nineinferior",
    b"centinferior",
    b"dollarinferior",
    b"periodinferior",
    b"commainferior",
    b"Agravesmall",
    b"Aacutesmall",
    b"Acircumflexsmall",
    b"Atildesmall",
    b"Adieresissmall",
    b"Aringsmall",
    b"AEsmall",
    b"Ccedillasmall",
    b"Egravesmall",
    b"Eacutesmall",
    b"Ecircumflexsmall",
    b"Edieresissmall",
    b"Igravesmall",
    b"Iacutesmall",
    b"Icircumflexsmall",
    b"Idieresissmall",
    b"Ethsmall",
    b"Ntildesmall",
    b"Ogravesmall",
    b"Oacutesmall",
    b"Ocircumflexsmall",
    b"Otildesmall",
    b"Odieresissmall",
    b"OEsmall",
    b"Oslashsmall",
    b"Ugravesmall",
    b"Uacutesmall",
    b"Ucircumflexsmall",
    b"Udieresissmall",
    b"Yacutesmall",
    b"Thornsmall",
    b"Ydieresissmall",
    b"001.000",
    b"001.001",
    b"001.002",
    b"001.003",
    b"Black",
    b"Bold",
    b"Book",
    b"Light",
    b"Medium",
    b"Regular",
    b"Roman",
    b"Semibold",
];

/// `get_unsigned` (`get_offset`): an `n`-byte big-endian number.
fn get_unsigned(stream: &mut MemFile, n: i32) -> u32 {
    todo!()
}

impl CffFont {
    /// `cff_open`: the font at `offset` in `stream` (`idx` of a fontset);
    /// takes the stream (pass a clone to keep reading the file).
    pub fn cff_open(stream: MemFile, offset: i32, idx: i32) -> Option<CffFont> {
        todo!()
    }
    /// `cff_close`.
    pub fn cff_close(self) {
        todo!()
    }
    /// `cff_seek_set`: to `offset + p`.
    pub fn cff_seek_set(&mut self, p: usize) {
        todo!()
    }
    /// `cff_read_data`: `fread` of `len` bytes.
    pub fn cff_read_data(&mut self, len: usize) -> Vec<u8> {
        todo!()
    }
    /// `cff_tell`.
    pub fn cff_tell(&self) -> usize {
        todo!()
    }
    /// `cff_seek`: absolute.
    pub fn cff_seek(&mut self, p: usize) {
        todo!()
    }
    /// `cff_put_header`.
    pub fn cff_put_header(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
    /// `cff_get_index`: an INDEX with its data, at the stream position.
    pub fn cff_get_index(&mut self) -> CffIndex {
        todo!()
    }
    /// `cff_get_index_header`: an INDEX's offsets; the stream is left at
    /// its data.
    pub fn cff_get_index_header(&mut self) -> CffIndex {
        todo!()
    }
    /// `cff_get_name`: the font's name (a copy).
    pub fn cff_get_name(&self) -> Vec<u8> {
        todo!()
    }
    /// `cff_set_name`.
    pub fn cff_set_name(&mut self, name: &[u8]) -> i32 {
        todo!()
    }
    /// `cff_read_subrs`.
    pub fn cff_read_subrs(&mut self) -> i32 {
        todo!()
    }
    /// `cff_read_encoding`.
    pub fn cff_read_encoding(&mut self) -> i32 {
        todo!()
    }
    /// `cff_pack_encoding`.
    pub fn cff_pack_encoding(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
    /// `cff_encoding_lookup`: the GID of `code`.
    pub fn cff_encoding_lookup(&self, code: Card8) -> Card16 {
        todo!()
    }
    /// `cff_read_charsets`.
    pub fn cff_read_charsets(&mut self) -> i32 {
        todo!()
    }
    /// `cff_pack_charsets`.
    pub fn cff_pack_charsets(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
    /// `cff_glyph_lookup`: the GID of PS name `glyph`.
    pub fn cff_glyph_lookup(&self, glyph: &[u8]) -> Card16 {
        todo!()
    }
    /// `cff_get_glyphname`: the PS name of `gid` (a copy).
    pub fn cff_get_glyphname(&self, gid: Card16) -> Vec<u8> {
        todo!()
    }
    /// `cff_charsets_lookup`: the GID of SID/CID `cid`.
    pub fn cff_charsets_lookup(&self, cid: Card16) -> Card16 {
        todo!()
    }
    /// `cff_charsets_lookup_inverse`: the SID or CID of `gid`.
    pub fn cff_charsets_lookup_inverse(&self, gid: Card16) -> Card16 {
        todo!()
    }
    /// `cff_read_fdselect`.
    pub fn cff_read_fdselect(&mut self) -> i32 {
        todo!()
    }
    /// `cff_pack_fdselect`.
    pub fn cff_pack_fdselect(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
    /// `cff_fdselect_lookup`.
    pub fn cff_fdselect_lookup(&self, gid: Card16) -> Card8 {
        todo!()
    }
    /// `cff_read_fdarray`.
    pub fn cff_read_fdarray(&mut self) -> i32 {
        todo!()
    }
    /// `cff_read_private`.
    pub fn cff_read_private(&mut self) -> i32 {
        todo!()
    }
    /// `cff_get_string`: SID `id`'s string (a copy).
    pub fn cff_get_string(&self, id: SSid) -> Vec<u8> {
        todo!()
    }
    /// `cff_get_sid`: the SID of `str`, or -1.
    pub fn cff_get_sid(&self, str: &[u8]) -> i32 {
        todo!()
    }
    /// `cff_get_seac_sid`: the standard-string SID of `str`, or -1.
    pub fn cff_get_seac_sid(&self, str: &[u8]) -> i32 {
        todo!()
    }
    /// `cff_match_string` (static).
    fn cff_match_string(&self, str: &[u8], sid: SSid) -> i32 {
        todo!()
    }
    /// `cff_add_string`: the SID (in `_string`).
    pub fn cff_add_string(&mut self, str: &[u8], unique: i32) -> SSid {
        todo!()
    }
    /// `cff_update_string`: `_string` becomes `string`.
    pub fn cff_update_string(&mut self) {
        todo!()
    }
}

impl CffIndex {
    /// `cff_new_index`.
    pub fn cff_new_index(count: Card16) -> CffIndex {
        todo!()
    }
    /// `cff_index_size`.
    pub fn cff_index_size(&self) -> i32 {
        todo!()
    }
    /// `cff_pack_index`.
    pub fn cff_pack_index(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
}

/// `cff_release_index`.
pub fn cff_release_index(idx: CffIndex) {
    drop(idx);
}

/// `cff_release_encoding`.
pub fn cff_release_encoding(encoding: CffEncoding) {
    drop(encoding);
}

/// `cff_release_charsets`.
pub fn cff_release_charsets(charset: CffCharsets) {
    drop(charset);
}

/// `cff_release_fdselect`.
pub fn cff_release_fdselect(fdselect: CffFdselect) {
    drop(fdselect);
}

impl CffCharsets {
    /// `cff_charsets_lookup_gid`: the GID of SID/CID `cid`.
    pub fn cff_charsets_lookup_gid(&self, cid: Card16) -> Card16 {
        todo!()
    }
    /// `cff_charsets_lookup_cid`: the SID/CID of `gid`.
    pub fn cff_charsets_lookup_cid(&self, gid: Card16) -> Card16 {
        todo!()
    }
}
