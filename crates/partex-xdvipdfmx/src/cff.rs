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

use crate::cff_dict::cff_dict_unpack;

/// `get_unsigned` (`get_offset`): an `n`-byte big-endian number.
fn get_unsigned(stream: &mut MemFile, n: i32) -> u32 {
    let mut v: u32 = 0;
    let mut n = n;
    while n > 0 {
        n -= 1;
        v = v
            .wrapping_mul(0x100)
            .wrapping_add(u32::from(stream.get_unsigned_byte()));
    }
    v
}

impl CffFont {
    fn st(&mut self) -> &mut MemFile {
        self.stream.as_mut().expect("CFF font has no stream")
    }

    /// `cff_open`: the font at `offset` in `stream` (`idx` of a fontset);
    /// takes the stream (pass a clone to keep reading the file).
    pub fn cff_open(stream: MemFile, offset: i32, idx: i32) -> Option<CffFont> {
        let n = idx;
        let mut cff = CffFont {
            fontname: None,
            index: n,
            stream: Some(stream),
            offset: offset as LOffset,
            filter: 0,
            flag: 0,
            num_glyphs: 0,
            num_fds: 0,
            ..CffFont::default()
        };

        cff.cff_seek_set(0);
        cff.header.major = cff.st().get_unsigned_byte();
        cff.header.minor = cff.st().get_unsigned_byte();
        cff.header.hdr_size = cff.st().get_unsigned_byte();
        cff.header.offsize = cff.st().get_unsigned_byte();
        if cff.header.offsize < 1 || cff.header.offsize > 4 {
            error!("invalid offsize data");
        }

        if cff.header.major > 1 || cff.header.minor > 0 {
            warn!(
                "CFF: CFF version {}.{} not supported.",
                cff.header.major, cff.header.minor
            );
            return None;
        }

        cff.cff_seek_set(cff.header.hdr_size as usize);

        // Name INDEX
        let idx = cff.cff_get_index();
        if n > i32::from(idx.count) - 1 {
            warn!("CFF: Invalid CFF fontset index number.");
            return None;
        }

        cff.name = Some(idx);

        cff.fontname = Some(cff.cff_get_name());

        // Top DICT INDEX
        let idx = cff.cff_get_index();
        if n > i32::from(idx.count) - 1 {
            error!("CFF Top DICT not exist...");
        }
        let a = idx.offset[n as usize] as usize - 1;
        let b = idx.offset[n as usize + 1] as usize - 1;
        cff.topdict = cff_dict_unpack(&idx.data[a..b]);
        if cff.topdict.is_none() {
            error!("Parsing CFF Top DICT data failed...");
        }
        drop(idx);

        {
            let td = cff.topdict.as_ref().unwrap();
            if td.cff_dict_known(b"CharstringType") != 0
                && td.cff_dict_get(b"CharstringType", 0) != 2.0
            {
                warn!("Only Type 2 Charstrings supported...");
                return None;
            }

            if td.cff_dict_known(b"SyntheticBase") != 0 {
                warn!("CFF Synthetic font not supported.");
                return None;
            }
        }

        // String INDEX
        cff.string = Some(cff.cff_get_index());

        // offset to GSubr
        cff.gsubr_offset = (cff.st().tell() as i64 - i64::from(offset)) as LOffset;

        // Number of glyphs
        let offset = cff
            .topdict
            .as_ref()
            .unwrap()
            .cff_dict_get(b"CharStrings", 0) as i32;
        cff.cff_seek_set(offset as usize);
        cff.num_glyphs = cff.st().get_unsigned_pair();

        let td = cff.topdict.as_ref().unwrap();
        // Check for font type
        if td.cff_dict_known(b"ROS") != 0 {
            cff.flag |= FONTTYPE_CIDFONT;
        } else {
            cff.flag |= FONTTYPE_FONT;
        }

        // Check for encoding
        if td.cff_dict_known(b"Encoding") != 0 {
            let offset = td.cff_dict_get(b"Encoding", 0) as i32;
            if offset == 0 {
                // predefined
                cff.flag |= ENCODING_STANDARD;
            } else if offset == 1 {
                cff.flag |= ENCODING_EXPERT;
            }
        } else {
            cff.flag |= ENCODING_STANDARD;
        }

        // Check for charset
        let td = cff.topdict.as_ref().unwrap();
        if td.cff_dict_known(b"charset") != 0 {
            let offset = td.cff_dict_get(b"charset", 0) as i32;
            if offset == 0 {
                // predefined
                cff.flag |= CHARSETS_ISOADOBE;
            } else if offset == 1 {
                cff.flag |= CHARSETS_EXPERT;
            } else if offset == 2 {
                cff.flag |= CHARSETS_EXPSUB;
            }
        } else {
            cff.flag |= CHARSETS_ISOADOBE;
        }

        cff.cff_seek_set(cff.gsubr_offset as usize); // seek back to GSubr

        Some(cff)
    }
    /// `cff_close`.
    pub fn cff_close(self) {
        drop(self);
    }
    /// `cff_seek_set`: to `offset + p`.
    pub fn cff_seek_set(&mut self, p: usize) {
        let o = self.offset as usize;
        self.st().seek_absolute(o + p);
    }
    /// `cff_read_data`: `fread` of `len` bytes.
    pub fn cff_read_data(&mut self, len: usize) -> Vec<u8> {
        self.st().read(len).to_vec()
    }
    /// `cff_tell`.
    pub fn cff_tell(&self) -> usize {
        self.stream.as_ref().expect("CFF font has no stream").tell()
    }
    /// `cff_seek`: absolute.
    pub fn cff_seek(&mut self, p: usize) {
        self.st().seek_absolute(p);
    }
    /// `cff_put_header` (C also sets `header.offsize` to 4, which nothing
    /// reads after).
    pub fn cff_put_header(&self, dest: &mut [u8]) -> i32 {
        if dest.len() < 4 {
            error!("Not enough space available...");
        }
        dest[0] = self.header.major;
        dest[1] = self.header.minor;
        // Additional data in between header and Name INDEX ignored.
        dest[2] = 4;
        // We will set all offset (0) to four-byte integer.
        dest[3] = 4;
        4
    }
    /// `cff_get_index`: an INDEX with its data, at the stream position.
    pub fn cff_get_index(&mut self) -> CffIndex {
        let mut idx = CffIndex::default();
        let count = self.st().get_unsigned_pair();
        idx.count = count;
        if count > 0 {
            idx.offsize = self.st().get_unsigned_byte();
            if idx.offsize < 1 || idx.offsize > 4 {
                error!("invalid offsize data");
            }

            idx.offset = vec![0; count as usize + 1];
            for i in 0..=count as usize {
                idx.offset[i] = get_unsigned(self.st(), i32::from(idx.offsize));
            }

            if idx.offset[0] != 1 {
                error!("Invalid CFF Index offset data");
            }

            let length = idx.offset[count as usize].wrapping_sub(idx.offset[0]) as i32;

            if length > 0 {
                let d = self.st().read(length as usize).to_vec();
                if d.len() != length as usize {
                    // C loops on fread for ever.
                    error!("CFF: reading INDEX data failed");
                }
                idx.data = d;
            }
        } else {
            idx.offsize = 0;
            idx.offset = Vec::new();
            idx.data = Vec::new();
        }
        idx
    }
    /// `cff_get_index_header`: an INDEX's offsets; the stream is left at
    /// its data.
    pub fn cff_get_index_header(&mut self) -> CffIndex {
        let mut idx = CffIndex::default();
        let count = self.st().get_unsigned_pair();
        idx.count = count;
        if count > 0 {
            idx.offsize = self.st().get_unsigned_byte();
            if idx.offsize < 1 || idx.offsize > 4 {
                error!("invalid offsize data");
            }

            idx.offset = vec![0; count as usize + 1];
            let mut i = 0usize;
            while i < count as usize {
                idx.offset[i] = get_unsigned(self.st(), i32::from(idx.offsize));
                i += 1;
            }
            if count == 0xFFFF {
                let p = self.cff_tell() + idx.offsize as usize;
                self.cff_seek(p);
            } else {
                idx.offset[i] = get_unsigned(self.st(), i32::from(idx.offsize));
            }

            if idx.offset[0] != 1 {
                error!("cff_get_index(): invalid index data");
            }

            idx.data = Vec::new();
        } else {
            idx.offsize = 0;
            idx.offset = Vec::new();
            idx.data = Vec::new();
        }
        idx
    }
    /// `cff_get_name`: the font's name (a copy).
    pub fn cff_get_name(&self) -> Vec<u8> {
        let idx = self.name.as_ref().expect("CFF Name INDEX");
        let i = self.index as usize;
        let len = idx.offset[i + 1].wrapping_sub(idx.offset[i]) as usize;
        let a = idx.offset[i] as usize - 1;
        idx.data[a..a + len].to_vec()
    }
    /// `cff_set_name`.
    pub fn cff_set_name(&mut self, name: &[u8]) -> i32 {
        if name.len() > 127 {
            error!("FontName string length too large...");
        }

        self.name = Some(CffIndex {
            count: 1,
            offsize: 1,
            offset: vec![1, name.len() as LOffset + 1],
            data: name.to_vec(), // no trailing '\0'
        });

        5 + name.len() as i32
    }
    /// `cff_read_subrs`.
    pub fn cff_read_subrs(&mut self) -> i32 {
        let mut len = 0;

        if (self.flag & FONTTYPE_CIDFONT) != 0 && self.fdarray.is_empty() {
            self.cff_read_fdarray();
        }

        if self.private.is_empty() {
            self.cff_read_private();
        }

        if self.gsubr.is_none() {
            self.cff_seek_set(self.gsubr_offset as usize);
            self.gsubr = Some(self.cff_get_index());
        }

        self.subrs = vec![None; self.num_fds as usize];
        if (self.flag & FONTTYPE_CIDFONT) != 0 {
            for i in 0..self.num_fds as usize {
                let known = match &self.private[i] {
                    Some(p) => p.cff_dict_known(b"Subrs") != 0,
                    None => false,
                };
                if !known {
                    self.subrs[i] = None;
                } else {
                    let mut offset = self.fdarray[i]
                        .as_ref()
                        .expect("Font DICT")
                        .cff_dict_get(b"Private", 1) as i32;
                    offset += self.private[i].as_ref().unwrap().cff_dict_get(b"Subrs", 0) as i32;
                    self.cff_seek_set(offset as usize);
                    let s = self.cff_get_index();
                    len += s.cff_index_size();
                    self.subrs[i] = Some(s);
                }
            }
        } else {
            let known = match &self.private[0] {
                Some(p) => p.cff_dict_known(b"Subrs") != 0,
                None => false,
            };
            if !known {
                self.subrs[0] = None;
            } else {
                let mut offset = self.topdict.as_ref().unwrap().cff_dict_get(b"Private", 1) as i32;
                offset += self.private[0].as_ref().unwrap().cff_dict_get(b"Subrs", 0) as i32;
                self.cff_seek_set(offset as usize);
                let s = self.cff_get_index();
                len += s.cff_index_size();
                self.subrs[0] = Some(s);
            }
        }

        len
    }
    /// `cff_read_encoding`.
    pub fn cff_read_encoding(&mut self) -> i32 {
        if self.topdict.is_none() {
            error!("Top DICT data not found");
        }

        if self.topdict.as_ref().unwrap().cff_dict_known(b"Encoding") == 0 {
            self.flag |= ENCODING_STANDARD;
            self.encoding = None;
            return 0;
        }

        let offset = self.topdict.as_ref().unwrap().cff_dict_get(b"Encoding", 0) as i32;
        if offset == 0 {
            // predefined
            self.flag |= ENCODING_STANDARD;
            self.encoding = None;
            return 0;
        } else if offset == 1 {
            self.flag |= ENCODING_EXPERT;
            self.encoding = None;
            return 0;
        }

        self.cff_seek_set(offset as usize);
        let mut encoding = CffEncoding::default();
        encoding.format = self.st().get_unsigned_byte();
        let mut length = 1;

        match encoding.format & !0x80 {
            0 => {
                encoding.num_entries = self.st().get_unsigned_byte();
                encoding.codes = vec![0; encoding.num_entries as usize];
                for i in 0..encoding.num_entries as usize {
                    encoding.codes[i] = self.st().get_unsigned_byte();
                }
                length += i32::from(encoding.num_entries) + 1;
            }
            1 => {
                encoding.num_entries = self.st().get_unsigned_byte();
                encoding.range1 = vec![CffRange1::default(); encoding.num_entries as usize];
                for i in 0..encoding.num_entries as usize {
                    encoding.range1[i].first = SSid::from(self.st().get_unsigned_byte());
                    encoding.range1[i].n_left = self.st().get_unsigned_byte();
                }
                length += i32::from(encoding.num_entries) * 2 + 1;
            }
            _ => {
                error!("Unknown Encoding format");
            }
        }

        // Supplementary data
        if (encoding.format & 0x80) != 0 {
            encoding.num_supps = self.st().get_unsigned_byte();
            encoding.supp = vec![CffMap::default(); encoding.num_supps as usize];
            for i in 0..encoding.num_supps as usize {
                encoding.supp[i].code = self.st().get_unsigned_byte();
                encoding.supp[i].glyph = self.st().get_unsigned_pair(); // SID
            }
            length += i32::from(encoding.num_supps) * 3 + 1;
        } else {
            encoding.num_supps = 0;
            encoding.supp = Vec::new();
        }

        self.encoding = Some(encoding);
        length
    }
    /// `cff_pack_encoding`.
    pub fn cff_pack_encoding(&self, dest: &mut [u8]) -> i32 {
        let destlen = dest.len() as i32;
        let mut len: usize = 0;

        if (self.flag & HAVE_STANDARD_ENCODING) != 0 || self.encoding.is_none() {
            return 0;
        }

        if destlen < 2 {
            error!("in cff_pack_encoding(): Buffer overflow");
        }

        let encoding = self.encoding.as_ref().unwrap();

        dest[len] = encoding.format;
        len += 1;
        dest[len] = encoding.num_entries;
        len += 1;
        match encoding.format & !0x80 {
            0 => {
                if destlen < len as i32 + i32::from(encoding.num_entries) {
                    error!("in cff_pack_encoding(): Buffer overflow");
                }
                for i in 0..encoding.num_entries as usize {
                    dest[len] = encoding.codes[i];
                    len += 1;
                }
            }
            1 => {
                if destlen < len as i32 + i32::from(encoding.num_entries) * 2 {
                    error!("in cff_pack_encoding(): Buffer overflow");
                }
                for i in 0..encoding.num_entries as usize {
                    dest[len] = (encoding.range1[i].first & 0xff) as u8;
                    len += 1;
                    dest[len] = encoding.range1[i].n_left;
                    len += 1;
                }
            }
            _ => {
                error!("Unknown Encoding format");
            }
        }

        if (encoding.format & 0x80) != 0 {
            if destlen < len as i32 + i32::from(encoding.num_supps) * 3 + 1 {
                error!("in cff_pack_encoding(): Buffer overflow");
            }
            dest[len] = encoding.num_supps;
            len += 1;
            for i in 0..encoding.num_supps as usize {
                dest[len] = encoding.supp[i].code;
                len += 1;
                dest[len] = ((encoding.supp[i].glyph >> 8) & 0xff) as u8;
                len += 1;
                dest[len] = (encoding.supp[i].glyph & 0xff) as u8;
                len += 1;
            }
        }

        len as i32
    }
    /// `cff_encoding_lookup`: the GID of `code`.
    pub fn cff_encoding_lookup(&self, code: Card8) -> Card16 {
        if (self.flag & (ENCODING_STANDARD | ENCODING_EXPERT)) != 0 {
            error!("Predefined CFF encoding not supported yet");
        } else if self.encoding.is_none() {
            error!("Encoding data not available");
        }

        let encoding = self.encoding.as_ref().unwrap();

        let mut gid: Card16 = 0;
        match encoding.format & !0x80 {
            0 => {
                for i in 0..encoding.num_entries as usize {
                    if code == encoding.codes[i] {
                        gid = (i + 1) as Card16;
                        break;
                    }
                }
            }
            1 => {
                let mut i = 0usize;
                while i < encoding.num_entries as usize {
                    let r = encoding.range1[i];
                    if i32::from(code) >= i32::from(r.first)
                        && i32::from(code) <= i32::from(r.first) + i32::from(r.n_left)
                    {
                        gid = (i32::from(gid) + i32::from(code) - i32::from(r.first) + 1) as Card16;
                        break;
                    }
                    gid = (i32::from(gid) + i32::from(r.n_left) + 1) as Card16;
                    i += 1;
                }
                if i == encoding.num_entries as usize {
                    gid = 0;
                }
            }
            _ => {
                error!("Unknown Encoding format.");
            }
        }

        // Supplementary data
        if gid == 0 && (encoding.format & 0x80) != 0 {
            if encoding.supp.is_empty() && encoding.num_supps > 0 {
                error!("No CFF supplementary encoding data read.");
            }
            for i in 0..encoding.num_supps as usize {
                if code == encoding.supp[i].code {
                    gid = self.cff_charsets_lookup(encoding.supp[i].glyph);
                    break;
                }
            }
        }

        gid
    }
    /// `cff_read_charsets`.
    pub fn cff_read_charsets(&mut self) -> i32 {
        if self.topdict.is_none() {
            error!("Top DICT not available");
        }

        if self.topdict.as_ref().unwrap().cff_dict_known(b"charset") == 0 {
            self.flag |= CHARSETS_ISOADOBE;
            self.charsets = None;
            return 0;
        }

        let offset = self.topdict.as_ref().unwrap().cff_dict_get(b"charset", 0) as i32;

        if offset == 0 {
            // predefined
            self.flag |= CHARSETS_ISOADOBE;
            self.charsets = None;
            return 0;
        } else if offset == 1 {
            self.flag |= CHARSETS_EXPERT;
            self.charsets = None;
            return 0;
        } else if offset == 2 {
            self.flag |= CHARSETS_EXPSUB;
            self.charsets = None;
            return 0;
        }

        self.cff_seek_set(offset as usize);
        let mut charset = CffCharsets::default();
        charset.format = self.st().get_unsigned_byte();
        charset.num_entries = 0;

        let mut count: Card16 = self.num_glyphs.wrapping_sub(1);
        let mut length = 1;

        // Not sure. Not well documented.
        match charset.format {
            0 => {
                charset.num_entries = self.num_glyphs.wrapping_sub(1); // no .notdef
                charset.glyphs = vec![0; charset.num_entries as usize];
                length += i32::from(charset.num_entries) * 2;
                for i in 0..charset.num_entries as usize {
                    charset.glyphs[i] = self.st().get_unsigned_pair();
                }
                count = 0;
            }
            1 => {
                while count > 0 && charset.num_entries < self.num_glyphs {
                    let first = self.st().get_unsigned_pair();
                    let n_left = self.st().get_unsigned_byte();
                    charset.range1.push(CffRange1 { first, n_left });
                    count = (i32::from(count) - (i32::from(n_left) + 1)) as Card16; // no-overrap
                    charset.num_entries += 1;
                }
                length += i32::from(charset.num_entries) * 3;
            }
            2 => {
                while count > 0 && charset.num_entries < self.num_glyphs {
                    let first = self.st().get_unsigned_pair();
                    let n_left = self.st().get_unsigned_pair();
                    charset.range2.push(CffRange2 { first, n_left });
                    count = (i32::from(count) - (i32::from(n_left) + 1)) as Card16; // non-overrapping
                    charset.num_entries += 1;
                }
                length += i32::from(charset.num_entries) * 4;
            }
            _ => {
                error!("Unknown Charset format");
            }
        }
        self.charsets = Some(charset);

        if count > 0 {
            error!("Charset data possibly broken");
        }

        length
    }
    /// `cff_pack_charsets`.
    pub fn cff_pack_charsets(&self, dest: &mut [u8]) -> i32 {
        let destlen = dest.len() as i32;
        let mut len: usize = 0;

        if (self.flag & HAVE_STANDARD_CHARSETS) != 0 || self.charsets.is_none() {
            return 0;
        }

        if destlen < 1 {
            error!("in cff_pack_charsets(): Buffer overflow");
        }

        let charset = self.charsets.as_ref().unwrap();

        dest[len] = charset.format;
        len += 1;
        match charset.format {
            0 => {
                if destlen < len as i32 + i32::from(charset.num_entries) * 2 {
                    error!("in cff_pack_charsets(): Buffer overflow");
                }
                for i in 0..charset.num_entries as usize {
                    let sid = charset.glyphs[i]; // or CID
                    dest[len] = ((sid >> 8) & 0xff) as u8;
                    len += 1;
                    dest[len] = (sid & 0xff) as u8;
                    len += 1;
                }
            }
            1 => {
                if destlen < len as i32 + i32::from(charset.num_entries) * 3 {
                    error!("in cff_pack_charsets(): Buffer overflow");
                }
                for i in 0..charset.num_entries as usize {
                    let r = charset.range1[i];
                    dest[len] = ((r.first >> 8) & 0xff) as u8;
                    len += 1;
                    dest[len] = (r.first & 0xff) as u8;
                    len += 1;
                    dest[len] = r.n_left;
                    len += 1;
                }
            }
            2 => {
                if destlen < len as i32 + i32::from(charset.num_entries) * 4 {
                    error!("in cff_pack_charsets(): Buffer overflow");
                }
                for i in 0..charset.num_entries as usize {
                    let r = charset.range2[i];
                    dest[len] = ((r.first >> 8) & 0xff) as u8;
                    len += 1;
                    dest[len] = (r.first & 0xff) as u8;
                    len += 1;
                    dest[len] = ((r.n_left >> 8) & 0xff) as u8;
                    len += 1;
                    dest[len] = (r.n_left & 0xff) as u8;
                    len += 1;
                }
            }
            _ => {
                error!("Unknown Charset format");
            }
        }

        len as i32
    }
    /// `cff_glyph_lookup`: the GID of PS name `glyph`.
    pub fn cff_glyph_lookup(&self, glyph: &[u8]) -> Card16 {
        if (self.flag & (CHARSETS_ISOADOBE | CHARSETS_EXPERT | CHARSETS_EXPSUB)) != 0 {
            error!("Predefined CFF charsets not supported yet");
        } else if self.charsets.is_none() {
            error!("Charsets data not available");
        }

        // .notdef always have glyph index 0
        if glyph == b".notdef" {
            return 0;
        }

        let charset = self.charsets.as_ref().unwrap();

        let mut gid: Card16 = 0;
        match charset.format {
            0 => {
                for i in 0..charset.num_entries as usize {
                    gid = gid.wrapping_add(1);
                    if self.cff_match_string(glyph, charset.glyphs[i]) != 0 {
                        return gid;
                    }
                }
            }
            1 => {
                for i in 0..charset.num_entries as usize {
                    let r = charset.range1[i];
                    for n in 0..=u32::from(r.n_left) {
                        gid = gid.wrapping_add(1);
                        if self.cff_match_string(glyph, (u32::from(r.first) + n) as SSid) != 0 {
                            return gid;
                        }
                    }
                }
            }
            2 => {
                for i in 0..charset.num_entries as usize {
                    let r = charset.range2[i];
                    for n in 0..=u32::from(r.n_left) {
                        gid = gid.wrapping_add(1);
                        if self.cff_match_string(glyph, (u32::from(r.first) + n) as SSid) != 0 {
                            return gid;
                        }
                    }
                }
            }
            _ => {
                error!("Unknown Charset format");
            }
        }

        0 // not found, returns .notdef
    }
    /// `cff_get_glyphname`: the PS name of `gid` (a copy).
    pub fn cff_get_glyphname(&self, gid: Card16) -> Vec<u8> {
        let sid = self.cff_charsets_lookup_inverse(gid);
        self.cff_get_string(sid)
    }
    /// `cff_charsets_lookup`: the GID of SID/CID `cid`.
    pub fn cff_charsets_lookup(&self, cid: Card16) -> Card16 {
        if (self.flag & (CHARSETS_ISOADOBE | CHARSETS_EXPERT | CHARSETS_EXPSUB)) != 0 {
            error!("Predefined CFF charsets not supported yet");
        } else if self.charsets.is_none() {
            error!("Charsets data not available");
        }

        self.charsets.as_ref().unwrap().cff_charsets_lookup_gid(cid)
    }
    /// `cff_charsets_lookup_inverse`: the SID or CID of `gid`.
    pub fn cff_charsets_lookup_inverse(&self, gid: Card16) -> Card16 {
        if (self.flag & (CHARSETS_ISOADOBE | CHARSETS_EXPERT | CHARSETS_EXPSUB)) != 0 {
            error!("Predefined CFF charsets not supported yet");
        } else if self.charsets.is_none() {
            error!("Charsets data not available");
        }

        if gid == 0 {
            return 0; // .notdef
        }

        self.charsets.as_ref().unwrap().cff_charsets_lookup_cid(gid)
    }
    /// `cff_read_fdselect`.
    pub fn cff_read_fdselect(&mut self) -> i32 {
        if self.topdict.is_none() {
            error!("Top DICT not available");
        }

        if (self.flag & FONTTYPE_CIDFONT) == 0 {
            return 0;
        }

        let offset = self.topdict.as_ref().unwrap().cff_dict_get(b"FDSelect", 0) as i32;
        self.cff_seek_set(offset as usize);
        let mut fdsel = CffFdselect::default();
        fdsel.format = self.st().get_unsigned_byte();

        let mut length = 1;

        match fdsel.format {
            0 => {
                fdsel.num_entries = self.num_glyphs;
                fdsel.fds = vec![0; fdsel.num_entries as usize];
                for i in 0..fdsel.num_entries as usize {
                    fdsel.fds[i] = self.st().get_unsigned_byte();
                }
                length += i32::from(fdsel.num_entries);
            }
            3 => {
                fdsel.num_entries = self.st().get_unsigned_pair();
                fdsel.ranges = vec![CffRange3::default(); fdsel.num_entries as usize];
                for i in 0..fdsel.num_entries as usize {
                    fdsel.ranges[i].first = self.st().get_unsigned_pair();
                    fdsel.ranges[i].fd = self.st().get_unsigned_byte();
                }
                if fdsel.ranges[0].first != 0 {
                    error!("Range not starting with 0.");
                }
                if self.num_glyphs != self.st().get_unsigned_pair() {
                    error!("Sentinel value mismatched with number of glyphs.");
                }
                length += i32::from(fdsel.num_entries) * 3 + 4;
            }
            _ => {
                error!("Unknown FDSelect format.");
            }
        }
        self.fdselect = Some(fdsel);

        length
    }
    /// `cff_pack_fdselect`.
    pub fn cff_pack_fdselect(&self, dest: &mut [u8]) -> i32 {
        let destlen = dest.len() as i32;
        let mut len: usize = 0;

        let Some(fdsel) = self.fdselect.as_ref() else {
            return 0;
        };

        if destlen < 1 {
            error!("in cff_pack_fdselect(): Buffur overflow");
        }

        dest[len] = fdsel.format;
        len += 1;
        match fdsel.format {
            0 => {
                if fdsel.num_entries != self.num_glyphs {
                    error!("in cff_pack_fdselect(): Invalid data");
                }
                if destlen < len as i32 + i32::from(fdsel.num_entries) {
                    error!("in cff_pack_fdselect(): Buffer overflow");
                }
                for i in 0..fdsel.num_entries as usize {
                    dest[len] = fdsel.fds[i];
                    len += 1;
                }
            }
            3 => {
                if destlen < len as i32 + 2 {
                    error!("in cff_pack_fdselect(): Buffer overflow");
                }
                len += 2;
                for i in 0..fdsel.num_entries as usize {
                    if destlen < len as i32 + 3 {
                        error!("in cff_pack_fdselect(): Buffer overflow");
                    }
                    dest[len] = ((fdsel.ranges[i].first >> 8) & 0xff) as u8;
                    len += 1;
                    dest[len] = (fdsel.ranges[i].first & 0xff) as u8;
                    len += 1;
                    dest[len] = fdsel.ranges[i].fd;
                    len += 1;
                }
                if destlen < len as i32 + 2 {
                    error!("in cff_pack_fdselect(): Buffer overflow");
                }
                dest[len] = ((self.num_glyphs >> 8) & 0xff) as u8;
                len += 1;
                dest[len] = (self.num_glyphs & 0xff) as u8;
                len += 1;
                let n = (len / 3) as i32 - 1;
                dest[1] = ((n >> 8) & 0xff) as u8;
                dest[2] = (n & 0xff) as u8;
            }
            _ => {
                error!("Unknown FDSelect format.");
            }
        }

        len as i32
    }
    /// `cff_fdselect_lookup`.
    pub fn cff_fdselect_lookup(&self, gid: Card16) -> Card8 {
        let Some(fdsel) = self.fdselect.as_ref() else {
            error!("in cff_fdselect_lookup(): FDSelect not available");
        };

        if gid >= self.num_glyphs {
            error!("in cff_fdselect_lookup(): Invalid glyph index");
        }

        let fd: Card8 = match fdsel.format {
            0 => fdsel.fds[gid as usize],
            3 => {
                if gid == 0 {
                    fdsel.ranges[0].fd
                } else {
                    let mut i = 1usize;
                    while i < fdsel.num_entries as usize {
                        if gid < fdsel.ranges[i].first {
                            break;
                        }
                        i += 1;
                    }
                    fdsel.ranges[i - 1].fd
                }
            }
            _ => {
                error!("in cff_fdselect_lookup(): Invalid FDSelect format");
            }
        };

        if fd >= self.num_fds {
            error!("in cff_fdselect_lookup(): Invalid Font DICT index");
        }

        fd
    }
    /// `cff_read_fdarray`.
    pub fn cff_read_fdarray(&mut self) -> i32 {
        if self.topdict.is_none() {
            error!("in cff_read_fdarray(): Top DICT not found");
        }

        if (self.flag & FONTTYPE_CIDFONT) == 0 {
            return 0;
        }

        // must exist
        let offset = self.topdict.as_ref().unwrap().cff_dict_get(b"FDArray", 0) as i32;
        self.cff_seek_set(offset as usize);
        let idx = self.cff_get_index();
        self.num_fds = idx.count as Card8;
        self.fdarray = vec![None; idx.count as usize];
        for i in 0..idx.count as usize {
            let a = idx.offset[i] as usize - 1;
            let size = idx.offset[i + 1].wrapping_sub(idx.offset[i]) as i32;
            if size > 0 {
                self.fdarray[i] = cff_dict_unpack(&idx.data[a..a + size as usize]);
            } else {
                self.fdarray[i] = None;
            }
        }
        idx.cff_index_size()
    }
    /// `cff_read_private`.
    pub fn cff_read_private(&mut self) -> i32 {
        let mut len = 0;

        if (self.flag & FONTTYPE_CIDFONT) != 0 {
            if self.fdarray.is_empty() {
                self.cff_read_fdarray();
            }

            self.private = vec![None; self.num_fds as usize];
            for i in 0..self.num_fds as usize {
                let mut size = 0;
                let ok = match &self.fdarray[i] {
                    Some(fd) => {
                        fd.cff_dict_known(b"Private") != 0 && {
                            size = fd.cff_dict_get(b"Private", 0) as i32;
                            size > 0
                        }
                    }
                    None => false,
                };
                if ok {
                    let offset = self.fdarray[i]
                        .as_ref()
                        .unwrap()
                        .cff_dict_get(b"Private", 1) as i32;
                    self.cff_seek_set(offset as usize);
                    let data = self.cff_read_data(size as usize);
                    if data.len() != size as usize {
                        error!("reading file failed");
                    }
                    self.private[i] = cff_dict_unpack(&data);
                    len += size;
                } else {
                    self.private[i] = None;
                }
            }
        } else {
            self.num_fds = 1;
            self.private = vec![None; 1];
            let td = self.topdict.as_ref().unwrap();
            let mut size = 0;
            if td.cff_dict_known(b"Private") != 0 && {
                size = td.cff_dict_get(b"Private", 0) as i32;
                size > 0
            } {
                let offset = td.cff_dict_get(b"Private", 1) as i32;
                self.cff_seek_set(offset as usize);
                let data = self.cff_read_data(size as usize);
                if data.len() != size as usize {
                    error!("reading file failed");
                }
                self.private[0] = cff_dict_unpack(&data);
                len += size;
            } else {
                self.private[0] = None;
                len = 0;
            }
        }

        len
    }
    /// `cff_get_string`: SID `id`'s string (a copy); C's NULL (an SID
    /// past the String INDEX) is empty here, see [`Self::cff_get_string_opt`].
    pub fn cff_get_string(&self, id: SSid) -> Vec<u8> {
        self.cff_get_string_opt(id).unwrap_or_default()
    }
    /// `cff_get_string` with C's NULL as none.
    pub fn cff_get_string_opt(&self, id: SSid) -> Option<Vec<u8>> {
        if i32::from(id) < CFF_STDSTR_MAX {
            Some(CFF_STDSTR[id as usize].to_vec())
        } else if let Some(strings) = self.string.as_ref() {
            let id = (i32::from(id) - CFF_STDSTR_MAX) as usize;
            if id < strings.count as usize {
                let len = strings.offset[id + 1].wrapping_sub(strings.offset[id]) as usize;
                let a = strings.offset[id] as usize - 1;
                Some(strings.data[a..a + len].to_vec())
            } else {
                None
            }
        } else {
            None
        }
    }
    /// `cff_get_sid`: the SID of `str`, or -1.
    pub fn cff_get_sid(&self, str: &[u8]) -> i32 {
        // I search String INDEX first.
        if let Some(idx) = self.string.as_ref() {
            for i in 0..idx.count as usize {
                if str.len() as u32 == idx.offset[i + 1].wrapping_sub(idx.offset[i]) {
                    let a = idx.offset[i] as usize - 1;
                    if str == &idx.data[a..a + str.len()] {
                        return i as i32 + CFF_STDSTR_MAX;
                    }
                }
            }
        }

        for i in 0..CFF_STDSTR_MAX as usize {
            if str == CFF_STDSTR[i] {
                return i as i32;
            }
        }

        -1
    }
    /// `cff_get_seac_sid`: the standard-string SID of `str`, or -1.
    pub fn cff_get_seac_sid(&self, str: &[u8]) -> i32 {
        for i in 0..CFF_STDSTR_MAX as usize {
            if str == CFF_STDSTR[i] {
                return i as i32;
            }
        }
        -1
    }
    /// `cff_match_string` (static).
    fn cff_match_string(&self, str: &[u8], sid: SSid) -> i32 {
        if i32::from(sid) < CFF_STDSTR_MAX {
            return i32::from(str == CFF_STDSTR[sid as usize]);
        }
        let i = (i32::from(sid) - CFF_STDSTR_MAX) as usize;
        let Some(s) = self.string.as_ref().filter(|s| i < s.count as usize) else {
            error!("Invalid SID");
        };
        if str.len() as u32 == s.offset[i + 1].wrapping_sub(s.offset[i]) {
            let a = s.offset[i] as usize - 1;
            return i32::from(str == &s.data[a..a + str.len()]);
        }
        0
    }
    /// `cff_add_string`: the SID (in `_string`).
    pub fn cff_add_string(&mut self, str: &[u8], unique: i32) -> SSid {
        // Setting unique == 1 eliminates redundant or predefined strings.
        let len = str.len() as u32;

        let strings = self
            ._string
            .get_or_insert_with(|| CffIndex::cff_new_index(0));

        if unique != 0 {
            // TODO: do binary search to speed things up
            for idx in 0..CFF_STDSTR_MAX as usize {
                if CFF_STDSTR[idx] == str {
                    return idx as SSid;
                }
            }
            for idx in 0..strings.count as usize {
                let size = strings.offset[idx + 1].wrapping_sub(strings.offset[idx]);
                let offset = strings.offset[idx] as usize;
                if size == len && &strings.data[offset - 1..offset - 1 + len as usize] == str {
                    return (idx as i32 + CFF_STDSTR_MAX) as SSid;
                }
            }
        }

        let offset: LOffset = if strings.count > 0 {
            strings.offset[strings.count as usize]
        } else {
            1
        };
        strings.offset.resize(strings.count as usize + 2, 0);
        if strings.count == 0 {
            strings.offset[0] = 1;
        }
        let idx = strings.count;
        strings.count += 1;
        strings.offset[strings.count as usize] = offset + len;
        strings.data.resize((offset + len - 1) as usize, 0);
        strings.data[(offset - 1) as usize..(offset - 1 + len) as usize].copy_from_slice(str);

        (i32::from(idx) + CFF_STDSTR_MAX) as SSid
    }
    /// `cff_update_string`: `_string` becomes `string`.
    pub fn cff_update_string(&mut self) {
        self.string = self._string.take();
    }
}

impl CffIndex {
    /// `cff_new_index`.
    pub fn cff_new_index(count: Card16) -> CffIndex {
        let mut idx = CffIndex {
            count,
            offsize: 0,
            offset: Vec::new(),
            data: Vec::new(),
        };
        if count > 0 {
            idx.offset = vec![0; count as usize + 1];
            idx.offset[0] = 1;
        }
        idx
    }
    /// `cff_index_size` (C also sets `offsize`, which nothing reads
    /// after).
    pub fn cff_index_size(&self) -> i32 {
        if self.count > 0 {
            let datalen = self.offset[self.count as usize].wrapping_sub(1);
            let offsize: u32 = if datalen < 0xff {
                1
            } else if datalen < 0xffff {
                2
            } else if datalen < 0xff_ffff {
                3
            } else {
                4
            };
            (3 + offsize * (u32::from(self.count) + 1) + datalen) as i32
        } else {
            2
        }
    }
    /// `cff_pack_index`.
    pub fn cff_pack_index(&self, dest: &mut [u8]) -> i32 {
        let destlen = dest.len() as i32;

        if self.count < 1 {
            if destlen < 2 {
                error!("Not enough space available...");
            }
            dest[0] = 0;
            dest[1] = 0;
            return 2;
        }

        let len = self.cff_index_size();
        let datalen = self.offset[self.count as usize].wrapping_sub(1);

        if destlen < len {
            error!("Not enough space available...");
        }

        let mut d = 0usize;
        dest[d] = ((self.count >> 8) & 0xff) as u8;
        d += 1;
        dest[d] = (self.count & 0xff) as u8;
        d += 1;

        let n = self.count as usize;
        if datalen < 0xff {
            dest[d] = 1;
            d += 1;
            for i in 0..=n {
                dest[d] = (self.offset[i] & 0xff) as u8;
                d += 1;
            }
        } else if datalen < 0xffff {
            dest[d] = 2;
            d += 1;
            for i in 0..=n {
                dest[d] = ((self.offset[i] >> 8) & 0xff) as u8;
                dest[d + 1] = (self.offset[i] & 0xff) as u8;
                d += 2;
            }
        } else if datalen < 0xff_ffff {
            dest[d] = 3;
            d += 1;
            for i in 0..=n {
                dest[d] = ((self.offset[i] >> 16) & 0xff) as u8;
                dest[d + 1] = ((self.offset[i] >> 8) & 0xff) as u8;
                dest[d + 2] = (self.offset[i] & 0xff) as u8;
                d += 3;
            }
        } else {
            dest[d] = 4;
            d += 1;
            for i in 0..=n {
                dest[d] = ((self.offset[i] >> 24) & 0xff) as u8;
                dest[d + 1] = ((self.offset[i] >> 16) & 0xff) as u8;
                dest[d + 2] = ((self.offset[i] >> 8) & 0xff) as u8;
                dest[d + 3] = (self.offset[i] & 0xff) as u8;
                d += 4;
            }
        }

        let dl = datalen as usize;
        dest[d..d + dl].copy_from_slice(&self.data[..dl]);

        len
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
        let mut gid: Card16 = 0;

        if cid == 0 {
            return 0; // GID 0 (.notdef)
        }

        match self.format {
            0 => {
                for i in 0..self.num_entries as usize {
                    if cid == self.glyphs[i] {
                        gid = (i + 1) as Card16;
                        return gid;
                    }
                }
            }
            1 => {
                for i in 0..self.num_entries as usize {
                    let r = self.range1[i];
                    if cid >= r.first && i32::from(cid) <= i32::from(r.first) + i32::from(r.n_left)
                    {
                        gid = (i32::from(gid) + i32::from(cid) - i32::from(r.first) + 1) as Card16;
                        return gid;
                    }
                    gid = (i32::from(gid) + i32::from(r.n_left) + 1) as Card16;
                }
            }
            2 => {
                for i in 0..self.num_entries as usize {
                    let r = self.range2[i];
                    if cid >= r.first && i32::from(cid) <= i32::from(r.first) + i32::from(r.n_left)
                    {
                        gid = (i32::from(gid) + i32::from(cid) - i32::from(r.first) + 1) as Card16;
                        return gid;
                    }
                    gid = (i32::from(gid) + i32::from(r.n_left) + 1) as Card16;
                }
            }
            _ => {
                error!("Unknown Charset format");
            }
        }

        0 // not found
    }
    /// `cff_charsets_lookup_cid`: the SID/CID of `gid`.
    pub fn cff_charsets_lookup_cid(&self, gid: Card16) -> Card16 {
        let mut sid: Card16 = 0;
        let mut gid = gid;

        match self.format {
            0 => {
                if i32::from(gid) - 1 >= i32::from(self.num_entries) {
                    error!("Invalid GID.");
                }
                sid = self.glyphs[gid as usize - 1];
            }
            1 => {
                let mut i = 0usize;
                while i < self.num_entries as usize {
                    let r = self.range1[i];
                    if i32::from(gid) <= i32::from(r.n_left) + 1 {
                        sid = (i32::from(gid) + i32::from(r.first) - 1) as Card16;
                        break;
                    }
                    gid = (i32::from(gid) - (i32::from(r.n_left) + 1)) as Card16;
                    i += 1;
                }
                if i == self.num_entries as usize {
                    error!("Invalid GID");
                }
            }
            2 => {
                let mut i = 0usize;
                while i < self.num_entries as usize {
                    let r = self.range2[i];
                    if i32::from(gid) <= i32::from(r.n_left) + 1 {
                        sid = (i32::from(gid) + i32::from(r.first) - 1) as Card16;
                        break;
                    }
                    gid = (i32::from(gid) - (i32::from(r.n_left) + 1)) as Card16;
                    i += 1;
                }
                if i == self.num_entries as usize {
                    error!("Invalid GID");
                }
            }
            _ => {
                error!("Unknown Charset format");
            }
        }

        sid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_pack_and_size() {
        let mut idx = CffIndex::cff_new_index(2);
        idx.offset[1] = 3;
        idx.offset[2] = 4;
        idx.data = vec![b'a', b'b', b'c'];
        assert_eq!(idx.cff_index_size(), 3 + 3 + 3);
        let mut d = [0u8; 9];
        assert_eq!(idx.cff_pack_index(&mut d), 9);
        assert_eq!(d, [0, 2, 1, 1, 3, 4, b'a', b'b', b'c']);
        let e = CffIndex::cff_new_index(0);
        assert_eq!(e.cff_index_size(), 2);
    }

    #[test]
    fn strings() {
        let mut cff = CffFont::default();
        assert_eq!(cff.cff_add_string(b"space", 1), 1);
        assert_eq!(cff.cff_add_string(b"Foo", 1), 391);
        assert_eq!(cff.cff_add_string(b"Bar", 1), 392);
        assert_eq!(cff.cff_add_string(b"Foo", 1), 391);
        assert_eq!(cff.cff_add_string(b"Foo", 0), 393);
        cff.cff_update_string();
        assert_eq!(cff.cff_get_string(392), b"Bar");
        assert_eq!(cff.cff_get_sid(b"Foo"), 391);
        assert_eq!(cff.cff_get_sid(b"A"), 34);
        assert_eq!(cff.cff_get_string_opt(400), None);
    }

    #[test]
    fn charsets() {
        let cs = CffCharsets {
            format: 1,
            num_entries: 2,
            range1: vec![
                CffRange1 {
                    first: 10,
                    n_left: 2,
                },
                CffRange1 {
                    first: 100,
                    n_left: 0,
                },
            ],
            ..CffCharsets::default()
        };
        assert_eq!(cs.cff_charsets_lookup_gid(11), 2);
        assert_eq!(cs.cff_charsets_lookup_gid(100), 4);
        assert_eq!(cs.cff_charsets_lookup_gid(13), 0);
        assert_eq!(cs.cff_charsets_lookup_cid(3), 12);
        assert_eq!(cs.cff_charsets_lookup_cid(4), 100);
    }

    #[test]
    fn fdselect_pack() {
        let cff = CffFont {
            num_glyphs: 5,
            fdselect: Some(CffFdselect {
                format: 3,
                num_entries: 2,
                ranges: vec![CffRange3 { first: 0, fd: 0 }, CffRange3 { first: 3, fd: 1 }],
                ..CffFdselect::default()
            }),
            ..CffFont::default()
        };
        let mut d = [0u8; 11];
        assert_eq!(cff.cff_pack_fdselect(&mut d), 11);
        assert_eq!(d, [3, 0, 2, 0, 0, 0, 0, 3, 1, 0, 5]);
    }
}
