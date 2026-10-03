//! An 8-bit font's encoding as xpdf's `GfxFont::makeFont` builds it
//! (`GfxFont.cc`, xpdf 4.05 as TeX Live has it): the type its dictionary
//! and its embedded file give (`getFontType`), then `Gfx8BitFont`'s
//! encoding: a base (the dictionary's `/Encoding` or `/BaseEncoding`, else
//! the embedded Type 1 or CFF file's own, else a default), with the
//! `/Differences` over it. pdfTeX's font replacement in PDF inclusion
//! (`writeEncodings`) writes it as the replaced font's `/Encoding`.

use alloc::vec::Vec;

use crate::fofi::{self, Encoding};
use crate::fofi_tables as tables;
use crate::pdfread::{Dict, Doc, Obj, Ref};

/// `GfxFontType` (the order matters: from `CidType0` on, a CID font).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GfxType {
    Unknown,
    Type1,
    Type1C,
    Type1COT,
    Type3,
    TrueType,
    TrueTypeOT,
    CidType0,
    CidType0C,
    CidType0COT,
    CidType2,
    CidType2OT,
}

/// `base14FontMap`: the names (spaces removed) that are a Base 14 font,
/// with the default encoding of the built-in font each names
/// (`builtinFonts`' `defaultBaseEnc`).
const BASE14: &[(&str, Builtin)] = &[
    ("Arial", Builtin::Standard),
    ("Arial,Bold", Builtin::Standard),
    ("Arial,BoldItalic", Builtin::Standard),
    ("Arial,Italic", Builtin::Standard),
    ("Arial-Bold", Builtin::Standard),
    ("Arial-BoldItalic", Builtin::Standard),
    ("Arial-BoldItalicMT", Builtin::Standard),
    ("Arial-BoldMT", Builtin::Standard),
    ("Arial-Italic", Builtin::Standard),
    ("Arial-ItalicMT", Builtin::Standard),
    ("ArialMT", Builtin::Standard),
    ("Courier", Builtin::Standard),
    ("Courier,Bold", Builtin::Standard),
    ("Courier,BoldItalic", Builtin::Standard),
    ("Courier,Italic", Builtin::Standard),
    ("Courier-Bold", Builtin::Standard),
    ("Courier-BoldOblique", Builtin::Standard),
    ("Courier-Oblique", Builtin::Standard),
    ("CourierNew", Builtin::Standard),
    ("CourierNew,Bold", Builtin::Standard),
    ("CourierNew,BoldItalic", Builtin::Standard),
    ("CourierNew,Italic", Builtin::Standard),
    ("CourierNew-Bold", Builtin::Standard),
    ("CourierNew-BoldItalic", Builtin::Standard),
    ("CourierNew-Italic", Builtin::Standard),
    ("CourierNewPS-BoldItalicMT", Builtin::Standard),
    ("CourierNewPS-BoldMT", Builtin::Standard),
    ("CourierNewPS-ItalicMT", Builtin::Standard),
    ("CourierNewPSMT", Builtin::Standard),
    ("Helvetica", Builtin::Standard),
    ("Helvetica,Bold", Builtin::Standard),
    ("Helvetica,BoldItalic", Builtin::Standard),
    ("Helvetica,Italic", Builtin::Standard),
    ("Helvetica-Bold", Builtin::Standard),
    ("Helvetica-BoldItalic", Builtin::Standard),
    ("Helvetica-BoldOblique", Builtin::Standard),
    ("Helvetica-Italic", Builtin::Standard),
    ("Helvetica-Oblique", Builtin::Standard),
    ("Symbol", Builtin::Symbol),
    ("Symbol,Bold", Builtin::Symbol),
    ("Symbol,BoldItalic", Builtin::Symbol),
    ("Symbol,Italic", Builtin::Symbol),
    ("Times-Bold", Builtin::Standard),
    ("Times-BoldItalic", Builtin::Standard),
    ("Times-Italic", Builtin::Standard),
    ("Times-Roman", Builtin::Standard),
    ("TimesNewRoman", Builtin::Standard),
    ("TimesNewRoman,Bold", Builtin::Standard),
    ("TimesNewRoman,BoldItalic", Builtin::Standard),
    ("TimesNewRoman,Italic", Builtin::Standard),
    ("TimesNewRoman-Bold", Builtin::Standard),
    ("TimesNewRoman-BoldItalic", Builtin::Standard),
    ("TimesNewRoman-Italic", Builtin::Standard),
    ("TimesNewRomanPS", Builtin::Standard),
    ("TimesNewRomanPS-Bold", Builtin::Standard),
    ("TimesNewRomanPS-BoldItalic", Builtin::Standard),
    ("TimesNewRomanPS-BoldItalicMT", Builtin::Standard),
    ("TimesNewRomanPS-BoldMT", Builtin::Standard),
    ("TimesNewRomanPS-Italic", Builtin::Standard),
    ("TimesNewRomanPS-ItalicMT", Builtin::Standard),
    ("TimesNewRomanPSMT", Builtin::Standard),
    ("TimesNewRomanPSMT,Bold", Builtin::Standard),
    ("TimesNewRomanPSMT,BoldItalic", Builtin::Standard),
    ("TimesNewRomanPSMT,Italic", Builtin::Standard),
    ("ZapfDingbats", Builtin::ZapfDingbats),
];

/// A built-in font's default encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Standard,
    Symbol,
    ZapfDingbats,
}

impl Builtin {
    fn encoding(self) -> Encoding {
        fofi::table(match self {
            Builtin::Standard => &tables::STANDARD,
            Builtin::Symbol => &tables::SYMBOL,
            Builtin::ZapfDingbats => &tables::ZAPF_DINGBATS,
        })
    }
}

/// `getFontType`: the font's type and its embedded file (`None` when
/// there is none, or it is not a stream, or its kind is not known).
fn font_type(doc: &Doc, font: &Dict) -> (GfxType, Option<Ref>) {
    let mut expected = GfxType::Unknown;
    let mut is_type0 = false;
    match doc.lookup(font, b"Subtype").as_name() {
        Some(b"Type1" | b"MMType1") => expected = GfxType::Type1,
        Some(b"Type1C") => expected = GfxType::Type1C,
        Some(b"Type3") => expected = GfxType::Type3,
        Some(b"TrueType") => expected = GfxType::TrueType,
        Some(b"Type0") => is_type0 = true,
        _ => {}
    }
    let mut font2 = None;
    if let Obj::Array(a) = doc.lookup(font, b"DescendantFonts")
        && let Some(first) = a.first()
        && let Obj::Dict(d) = doc.follow(first)
    {
        match doc.lookup(&d, b"Subtype").as_name() {
            Some(b"CIDFontType0") if is_type0 => expected = GfxType::CidType0,
            Some(b"CIDFontType2") if is_type0 => expected = GfxType::CidType2,
            _ => {}
        }
        font2 = Some(d);
    }
    let font2 = font2.as_ref().unwrap_or(font);
    let mut emb = None;
    // (Adobe uses FontFile3 over FontFile2 if both are there)
    if let Obj::Dict(desc) = doc.lookup(font2, b"FontDescriptor") {
        if let Some(Obj::Ref(r)) = desc.get(b"FontFile3") {
            emb = Some(*r);
            if let Obj::Stream(s) = doc.fetch(*r) {
                let t0 = is_type0;
                match doc.lookup(&s.dict, b"Subtype").as_name() {
                    Some(b"Type1") => {
                        if expected != GfxType::Type1 {
                            expected = if t0 { GfxType::CidType0 } else { GfxType::Type1 };
                        }
                    }
                    Some(b"Type1C") => {
                        if expected == GfxType::Type1 {
                            expected = GfxType::Type1C;
                        } else if expected != GfxType::Type1C {
                            expected = if t0 { GfxType::CidType0C } else { GfxType::Type1C };
                        }
                    }
                    Some(b"TrueType") => {
                        if expected != GfxType::TrueType {
                            expected = if t0 { GfxType::CidType2 } else { GfxType::TrueType };
                        }
                    }
                    Some(b"CIDFontType0C") => {
                        expected = if expected == GfxType::CidType0 || t0 {
                            GfxType::CidType0C
                        } else {
                            GfxType::Type1C
                        };
                    }
                    Some(b"OpenType") => {
                        expected = match expected {
                            GfxType::TrueType => GfxType::TrueTypeOT,
                            GfxType::Type1 => GfxType::Type1COT,
                            GfxType::CidType0 => GfxType::CidType0COT,
                            GfxType::CidType2 => GfxType::CidType2OT,
                            e => e,
                        };
                    }
                    _ => {}
                }
            }
        }
        if emb.is_none()
            && let Some(Obj::Ref(r)) = desc.get(b"FontFile2")
        {
            emb = Some(*r);
            if is_type0 {
                expected = GfxType::CidType2;
            }
        }
        if emb.is_none()
            && let Some(Obj::Ref(r)) = desc.get(b"FontFile")
        {
            emb = Some(*r);
        }
    }
    let mut t = GfxType::Unknown;
    if let Some(r) = emb
        && let Obj::Stream(s) = doc.fetch(r)
    {
        t = match fofi::identify(&doc.decode(&s)) {
            fofi::Kind::Type1Pfa | fofi::Kind::Type1Pfb => GfxType::Type1,
            fofi::Kind::Cff8Bit if is_type0 => GfxType::CidType0C,
            fofi::Kind::Cff8Bit => GfxType::Type1C,
            fofi::Kind::CffCid => GfxType::CidType0C,
            fofi::Kind::TrueType | fofi::Kind::TrueTypeCollection if is_type0 => GfxType::CidType2,
            fofi::Kind::TrueType | fofi::Kind::TrueTypeCollection => GfxType::TrueType,
            fofi::Kind::OpenTypeCff8Bit if is_type0 => GfxType::CidType0COT,
            fofi::Kind::OpenTypeCff8Bit => GfxType::Type1COT,
            fofi::Kind::OpenTypeCffCid => GfxType::CidType0COT,
            fofi::Kind::Unknown => GfxType::Unknown,
        };
    }
    if t == GfxType::Unknown {
        // (no file, not a stream, or of no kind known: nothing uses it)
        return (expected, None);
    }
    (t, emb)
}

/// The encoding `Gfx8BitFont`'s constructor builds for the font `font`
/// (`getCharName` of each code: `None` for `NULL`); `None` for a font
/// `makeFont` makes a `GfxCIDFont`.
#[must_use]
pub fn encoding(doc: &Doc, font: &Dict) -> Option<Encoding> {
    let name: Option<Vec<u8>> = match doc.lookup(font, b"BaseFont") {
        Obj::Name(n) | Obj::Str(n) => Some(n),
        _ => None,
    };
    let (t, emb) = font_type(doc, font);
    if t >= GfxType::CidType0 {
        return None;
    }
    // a Base 14 font's name, its spaces removed
    let builtin = name.as_ref().and_then(|n| {
        let n: Vec<u8> = n.iter().copied().filter(|&c| c != b' ').collect();
        BASE14.iter().find(|(a, _)| a.as_bytes() == n).map(|&(_, b)| b)
    });
    let enc_obj = doc.lookup(font, b"Encoding");
    let named = |n: &[u8]| -> Option<Encoding> {
        Some(fofi::table(match n {
            b"MacRomanEncoding" => &tables::MAC_ROMAN,
            b"MacExpertEncoding" => &tables::MAC_EXPERT,
            b"WinAnsiEncoding" => &tables::WIN_ANSI,
            _ => return None,
        }))
    };
    let mut base = match &enc_obj {
        Obj::Dict(d) => doc.lookup(d, b"BaseEncoding").as_name().and_then(named),
        Obj::Name(n) => named(n),
        _ => None,
    };
    // (a non-embedded Symbol or ZapfDingbats font never uses one of the
    // non-symbol encodings)
    if builtin.is_some_and(|b| b != Builtin::Standard) && emb.is_none() && !matches!(enc_obj, Obj::Dict(_)) {
        base = None;
    }
    // the embedded file's own (Type 1 and CFF files only)
    let mut from_file = false;
    if base.is_none()
        && let Some(r) = emb
        && matches!(t, GfxType::Type1 | GfxType::Type1C)
        && let Obj::Stream(s) = doc.fetch(r)
    {
        let data = doc.decode(&s);
        base = if t == GfxType::Type1 {
            fofi::type1_encoding(&data)
        } else {
            fofi::type1c_encoding(&data)
        };
        from_file = base.is_some();
    }
    let mut enc = match base {
        Some(e) => e,
        None => match builtin {
            Some(b) if emb.is_none() => b.encoding(),
            _ if t == GfxType::TrueType => fofi::table(&tables::WIN_ANSI),
            _ => fofi::table(&tables::STANDARD),
        },
    };
    // (some Type 1C files have empty encodings: the gaps from
    // StandardEncoding)
    if t == GfxType::Type1C && emb.is_some() && from_file {
        for (e, s) in enc.iter_mut().zip(tables::STANDARD) {
            if e.is_none() && !s.is_empty() {
                *e = Some(s.as_bytes().to_vec());
            }
        }
    }
    if let Obj::Dict(d) = &enc_obj
        && let Obj::Array(diffs) = doc.lookup(d, b"Differences")
    {
        let mut code: i32 = 0;
        for o in &diffs {
            match doc.follow(o) {
                Obj::Int(i) => code = i,
                Obj::Name(n) => {
                    if let Some(e) = usize::try_from(code).ok().and_then(|c| enc.get_mut(c)) {
                        *e = Some(n);
                    }
                    code = code.wrapping_add(1);
                }
                // ("Wrong type in font encoding resource differences")
                _ => {}
            }
        }
    }
    Some(enc)
}
