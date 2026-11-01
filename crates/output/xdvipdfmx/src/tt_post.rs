//! tt_post.c, tt_post.h: the `post` table (glyph names).

use crate::prelude::*;
use crate::sfnt::{FWord, Fixed, Sfnt, ULONG, USHORT};

/// `NAME_STR_OFFSET`: offset from the beginning of the post table.
pub const NAME_STR_OFFSET: u32 = 32;

/// `struct tt_post_table`.
#[derive(Clone, Debug, Default)]
pub struct TtPostTable {
    /// C's `Version`.
    pub version: Fixed,
    pub italic_angle: Fixed,
    pub underline_position: FWord,
    pub underline_thickness: FWord,
    pub is_fixed_pitch: ULONG,
    pub min_mem_type42: ULONG,
    pub max_mem_type42: ULONG,
    pub min_mem_type1: ULONG,
    pub max_mem_type1: ULONG,
    pub number_of_glyphs: USHORT,
    /// `glyphNamePtr`: the name of each glyph (copies of the
    /// `MACGLYPHORDER` entry or of `names[i]` C pointed to, as C strings:
    /// up to a NUL), none when C's is NULL.
    pub glyph_name_ptr: Vec<Option<Vec<u8>>>,
    /// Non-standard glyph names (none: C's NULL, an empty Pascal string).
    pub names: Vec<Option<Vec<u8>>>,
    /// Number of glyph names in `names`.
    pub count: USHORT,
}

/// A C string: the bytes up to the first NUL.
fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `read_v2_post_names` (static): 0, or -1 for an invalid table.
fn read_v2_post_names(post: &mut TtPostTable, sfont: &mut Sfnt) -> Result<i32> {
    post.number_of_glyphs = sfont.sfnt_get_ushort()?;

    let mut indices: Vec<USHORT> = Vec::with_capacity(post.number_of_glyphs as usize);
    let mut maxidx: USHORT = 257;
    for _ in 0..post.number_of_glyphs {
        let idx = sfont.sfnt_get_ushort()?;
        if idx >= 258 && idx > maxidx {
            maxidx = idx;
        }
        indices.push(idx);
    }

    post.count = maxidx - 257;
    if post.count < 1 {
        post.names = Vec::new();
    } else {
        post.names = Vec::with_capacity(post.count as usize);
        for _ in 0..post.count {
            // Read Pascal strings.
            let len = sfont.sfnt_get_byte()? as usize;
            if len > 0 {
                let mut name = vec![0u8; len];
                sfont.sfnt_read(&mut name);
                post.names.push(Some(name));
            } else {
                post.names.push(None);
            }
        }
    }

    post.glyph_name_ptr = Vec::with_capacity(post.number_of_glyphs as usize);
    for i in 0..post.number_of_glyphs as usize {
        let idx = indices[i];
        if idx < 258 {
            post.glyph_name_ptr
                .push(Some(MACGLYPHORDER[idx as usize].to_vec()));
        } else if i32::from(idx) - 258 < i32::from(post.count) {
            let n = post.names[idx as usize - 258]
                .as_deref()
                .map(|s| c_str(s).to_vec());
            post.glyph_name_ptr.push(n);
        } else {
            warn!(
                "Invalid glyph name index number: {} (>= {})",
                idx,
                i32::from(post.count) + 258
            );
            return Ok(-1);
        }
    }

    Ok(0)
}

impl Sfnt {
    /// `tt_read_post_table`: none for an invalid version 2.0 table
    /// (`sfnt_locate_table` ERRORs without one).
    pub fn tt_read_post_table(&mut self) -> Result<Option<TtPostTable>> {
        self.sfnt_locate_table(b"post")?;

        let mut post = TtPostTable {
            version: self.sfnt_get_ulong()?,             /* Fixed */
            italic_angle: self.sfnt_get_ulong()?,        /* Fixed */
            underline_position: self.sfnt_get_short()?,  /* FWord */
            underline_thickness: self.sfnt_get_short()?, /* FWord */
            is_fixed_pitch: self.sfnt_get_ulong()?,
            min_mem_type42: self.sfnt_get_ulong()?,
            max_mem_type42: self.sfnt_get_ulong()?,
            min_mem_type1: self.sfnt_get_ulong()?,
            max_mem_type1: self.sfnt_get_ulong()?,
            number_of_glyphs: 0,
            glyph_name_ptr: Vec::new(),
            count: 0,
            names: Vec::new(),
        };

        if post.version == 0x0001_0000 {
            post.number_of_glyphs = 258; /* wrong */
            post.glyph_name_ptr = MACGLYPHORDER.iter().map(|s| Some(s.to_vec())).collect();
        } else if post.version == 0x0002_8000 {
            warn!("TrueType 'post' version 2.5 found (deprecated)");
        } else if post.version == 0x0002_0000 {
            if read_v2_post_names(&mut post, self)? < 0 {
                warn!("Invalid version 2.0 'post' table");
                return Ok(None);
            }
        } else if post.version == 0x0003_0000 || post.version == 0x0004_0000 {
            // No glyph names provided / Apple format for printer-based
            // fonts.
        } else {
            warn!(
                "Unknown 'post' version: {:08X}, assuming version 3.0",
                post.version
            );
        }

        Ok(Some(post))
    }
}

impl TtPostTable {
    /// `tt_release_post_table`.
    pub fn tt_release_post_table(self) {}
    /// `tt_lookup_post_table`: the gid, 0 if not found.
    #[must_use]
    pub fn tt_lookup_post_table(&self, glyphname: &[u8]) -> USHORT {
        let glyphname = c_str(glyphname);
        for gid in 0..self.number_of_glyphs {
            if let Some(Some(name)) = self.glyph_name_ptr.get(gid as usize) {
                if glyphname == &name[..] {
                    return gid;
                }
            }
        }
        0
    }
    /// `tt_get_glyphname`: a copy.
    #[must_use]
    pub fn tt_get_glyphname(&self, gid: USHORT) -> Option<Vec<u8>> {
        if gid < self.number_of_glyphs {
            if let Some(Some(name)) = self.glyph_name_ptr.get(gid as usize) {
                return Some(name.clone());
            }
        }
        None
    }
}

/// `macglyphorder`: the Macintosh glyph order (Apple's TTRefMan).
pub static MACGLYPHORDER: [&[u8]; 258] = [
    b".notdef",
    b".null",
    b"nonmarkingreturn",
    b"space",
    b"exclam",
    b"quotedbl",
    b"numbersign",
    b"dollar",
    b"percent",
    b"ampersand",
    b"quotesingle",
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
    b"grave",
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
    b"Adieresis",
    b"Aring",
    b"Ccedilla",
    b"Eacute",
    b"Ntilde",
    b"Odieresis",
    b"Udieresis",
    b"aacute",
    b"agrave",
    b"acircumflex",
    b"adieresis",
    b"atilde",
    b"aring",
    b"ccedilla",
    b"eacute",
    b"egrave",
    b"ecircumflex",
    b"edieresis",
    b"iacute",
    b"igrave",
    b"icircumflex",
    b"idieresis",
    b"ntilde",
    b"oacute",
    b"ograve",
    b"ocircumflex",
    b"odieresis",
    b"otilde",
    b"uacute",
    b"ugrave",
    b"ucircumflex",
    b"udieresis",
    b"dagger",
    b"degree",
    b"cent",
    b"sterling",
    b"section",
    b"bullet",
    b"paragraph",
    b"germandbls",
    b"registered",
    b"copyright",
    b"trademark",
    b"acute",
    b"dieresis",
    b"notequal",
    b"AE",
    b"Oslash",
    b"infinity",
    b"plusminus",
    b"lessequal",
    b"greaterequal",
    b"yen",
    b"mu",
    b"partialdiff",
    b"summation",
    b"product",
    b"pi",
    b"integral",
    b"ordfeminine",
    b"ordmasculine",
    b"Omega",
    b"ae",
    b"oslash",
    b"questiondown",
    b"exclamdown",
    b"logicalnot",
    b"radical",
    b"florin",
    b"approxequal",
    b"Delta",
    b"guillemotleft",
    b"guillemotright",
    b"ellipsis",
    b"nonbreakingspace",
    b"Agrave",
    b"Atilde",
    b"Otilde",
    b"OE",
    b"oe",
    b"endash",
    b"emdash",
    b"quotedblleft",
    b"quotedblright",
    b"quoteleft",
    b"quoteright",
    b"divide",
    b"lozenge",
    b"ydieresis",
    b"Ydieresis",
    b"fraction",
    b"currency",
    b"guilsinglleft",
    b"guilsinglright",
    b"fi",
    b"fl",
    b"daggerdbl",
    b"periodcentered",
    b"quotesinglbase",
    b"quotedblbase",
    b"perthousand",
    b"Acircumflex",
    b"Ecircumflex",
    b"Aacute",
    b"Edieresis",
    b"Egrave",
    b"Iacute",
    b"Icircumflex",
    b"Idieresis",
    b"Igrave",
    b"Oacute",
    b"Ocircumflex",
    b"apple",
    b"Ograve",
    b"Uacute",
    b"Ucircumflex",
    b"Ugrave",
    b"dotlessi",
    b"circumflex",
    b"tilde",
    b"macron",
    b"breve",
    b"dotaccent",
    b"ring",
    b"cedilla",
    b"hungarumlaut",
    b"ogonek",
    b"caron",
    b"Lslash",
    b"lslash",
    b"Scaron",
    b"scaron",
    b"Zcaron",
    b"zcaron",
    b"brokenbar",
    b"Eth",
    b"eth",
    b"Yacute",
    b"yacute",
    b"Thorn",
    b"thorn",
    b"minus",
    b"multiply",
    b"onesuperior",
    b"twosuperior",
    b"threesuperior",
    b"onehalf",
    b"onequarter",
    b"threequarters",
    b"franc",
    b"Gbreve",
    b"gbreve",
    b"Idotaccent",
    b"Scedilla",
    b"scedilla",
    b"Cacute",
    b"cacute",
    b"Ccaron",
    b"ccaron",
    b"dcroat",
];
