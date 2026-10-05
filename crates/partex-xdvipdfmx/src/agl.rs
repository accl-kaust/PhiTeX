//! agl.c, agl.h: the Adobe Glyph List (glyph name to Unicode).
//!
//! The map is `self.agl.aglmap`, an [`HtTable`] of [`AglName`]s; a name
//! listed twice chains the later entries through `alternate`.

use crate::dpxutil::HtTable;
use crate::prelude::*;

/// `AGL_DEFAULT_LISTFILE`.
pub const AGL_DEFAULT_LISTFILE: &[u8] = b"glyphlist.txt";
/// `AGL_PREDEF_LISTFILE`.
pub const AGL_PREDEF_LISTFILE: &[u8] = b"pdfglyphlist.txt";
/// `AGL_EXTRA_LISTFILE`.
pub const AGL_EXTRA_LISTFILE: &[u8] = b"texglyphlist.txt";
/// `AGL_MAX_UNICODES`.
pub const AGL_MAX_UNICODES: usize = 16;
/// `SUFFIX_LIST_MAX`.
pub const SUFFIX_LIST_MAX: usize = 16;
/// `AGL_VAR_SMCP_IDX`.
pub const AGL_VAR_SMCP_IDX: i32 = 0;
/// `WBUF_SIZE`.
pub const WBUF_SIZE: usize = 1024;

/// `agl_name`.
#[derive(Clone, Debug, Default)]
pub struct AglName {
    pub name: Option<Vec<u8>>,
    pub suffix: Option<Vec<u8>>,
    pub n_components: i32,
    pub unicodes: [i32; AGL_MAX_UNICODES],
    /// The next entry for the same name.
    pub alternate: Option<Box<AglName>>,
    pub is_predef: i32,
}

/// agl.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `aglmap`.
    pub aglmap: HtTable<AglName>,
}

/// `modifiers` (static; without the C terminator).
pub static MODIFIERS: [&[u8]; 20] = [
    b"acute",
    b"breve",
    b"caron",
    b"cedilla",
    b"circumflex",
    b"dieresis",
    b"dotaccent",
    b"grave",
    b"hungarumlaut",
    b"macron",
    b"ogonek",
    b"ring",
    b"tilde",
    b"commaaccent",
    b"slash",
    b"ampersand",
    b"exclam",
    b"exclamdown",
    b"question",
    b"questiondown",
];

/// An entry of `var_list`: key, OTL tag, suffixes.
#[derive(Clone, Copy, Debug)]
pub struct VarListEntry {
    pub key: &'static [u8],
    pub otl_tag: Option<&'static [u8]>,
    pub suffixes: &'static [&'static [u8]],
}

/// `var_list` (static; without the C terminator).
pub static VAR_LIST: [VarListEntry; 13] = [
    VarListEntry {
        key: b"small",
        otl_tag: Some(b"smcp"),
        suffixes: &[b"sc"],
    },
    VarListEntry {
        key: b"swash",
        otl_tag: Some(b"swsh"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"superior",
        otl_tag: Some(b"sups"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"inferior",
        otl_tag: Some(b"sinf"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"numerator",
        otl_tag: Some(b"numr"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"denominator",
        otl_tag: Some(b"dnom"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"oldstyle",
        otl_tag: Some(b"onum"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"display",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"text",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"big",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"bigg",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"Big",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"Bigg",
        otl_tag: None,
        suffixes: &[],
    },
];

/// `agl_new_name` (static).
fn agl_new_name() -> AglName {
    todo!()
}

/// `agl_chop_suffix`: the name (none when the glyph name starts with
/// `.`) and the suffix.
pub fn agl_chop_suffix(glyphname: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    todo!()
}

/// `skip_capital` (static).
fn skip_capital(s: &[u8], p: &mut usize) -> i32 {
    todo!()
}

/// `skip_modifier` (static).
fn skip_modifier(s: &[u8], p: &mut usize) -> i32 {
    todo!()
}

/// `is_smallcap` (static).
fn is_smallcap(glyphname: &[u8]) -> bool {
    todo!()
}

/// `agl_suffix_to_otltag`.
#[must_use]
pub fn agl_suffix_to_otltag(suffix: &[u8]) -> Option<&'static [u8]> {
    todo!()
}

/// `agl_guess_name` (static): an index into `VAR_LIST`, or -1.
fn agl_guess_name(glyphname: &[u8]) -> i32 {
    todo!()
}

/// `agl_normalized_name` (static).
fn agl_normalized_name(glyphname: &[u8]) -> Option<AglName> {
    todo!()
}

/// `agl_name_is_unicode`: `uniXXXX` or `uXXXX[XX]`.
#[must_use]
pub fn agl_name_is_unicode(glyphname: &[u8]) -> bool {
    todo!()
}

/// `agl_name_convert_unicode`: the code point, or -1.
#[must_use]
pub fn agl_name_convert_unicode(glyphname: &[u8]) -> i32 {
    todo!()
}

/// `xtol` (static).
fn xtol(s: &[u8], len: i32) -> i32 {
    todo!()
}

/// `put_unicode_glyph` (static): the bytes written at `dst[*dstp..]`.
fn put_unicode_glyph(name: &[u8], dst: &mut [u8], dstp: &mut usize) -> i32 {
    todo!()
}

impl Dpx {
    /// `agl_init_map`.
    pub fn agl_init_map(&mut self) {
        todo!()
    }

    /// `agl_close_map`.
    pub fn agl_close_map(&mut self) {
        todo!()
    }

    /// `agl_load_listfile` (static): the number of entries read, or -1.
    fn agl_load_listfile(&mut self, filename: &[u8], is_predef: i32) -> i32 {
        todo!()
    }

    /// `agl_lookup_list`: a copy of the entry (with its `alternate` chain).
    pub fn agl_lookup_list(&mut self, glyphname: &[u8]) -> Option<AglName> {
        todo!()
    }

    /// `agl_sput_UTF16BE`: the bytes written at `dst[*dstp..]` (`dst`
    /// ends at C's `limptr`) and the `fail_count`.
    #[allow(non_snake_case)]
    pub fn agl_sput_UTF16BE(
        &mut self,
        glyphstr: &[u8],
        dst: &mut [u8],
        dstp: &mut usize,
    ) -> (i32, i32) {
        todo!()
    }

    /// `agl_get_unicodes`: the count put in `unicodes` (its length is
    /// `max_unicodes`), or -1.
    pub fn agl_get_unicodes(&mut self, glyphstr: &[u8], unicodes: &mut [i32]) -> i32 {
        todo!()
    }
}
