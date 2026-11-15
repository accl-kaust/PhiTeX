//! XeTeX's native fonts for a host without fontconfig or kpathsea (the
//! frontends' wasm core over its Shelf): the font files read and found
//! by the host's closures ([`HostFonts`]), the faces a name
//! (`"Latin Modern Roman/B"`) finds from an index the host ships
//! (`phitex_tex::FontIndex::to_bytes`, the stable `PXFI` format, its
//! paths relative to `TEXMFDIST`: made on a machine with TeX Live and
//! fontconfig by `phitex-otf`'s `otf-index OUT --relative $TEXMFDIST`).
//! The CLI's XeTeX is kpathsea's and fontconfig's (`xefonts.rs` there).

use std::sync::Arc;

use phitex_tex::{FontIndex, FontSource, KpseFormat, XeTeX};

/// A read: the bytes of a font file by the path the index or a lookup
/// gave (a shipped index's paths are relative: `fonts/opentype/…`).
pub type ReadFont = Box<dyn Fn(&str) -> Option<Arc<[u8]>> + Send + Sync>;

/// A lookup: kpathsea's `kpse_find_file(name, format, 0)` as the host
/// answers it (a `[file]` font name, a TECkit mapping): the path, which
/// [`ReadFont`] reads.
pub type FindFont = Box<dyn Fn(&str, KpseFormat) -> Option<String> + Send + Sync>;

/// XeTeX's font files through a host's closures.
pub struct HostFonts {
    read: ReadFont,
    find: FindFont,
}

impl FontSource for HostFonts {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        (self.read)(path)
    }

    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        (self.find)(name, format)
    }
}

/// The XeTeX of a host's fonts: files by `read` and `find`, names looked
/// up in `index` (the shipped index's bytes; none: only `[file]` names
/// load), for its [`crate::host::Host::xetex`].
///
/// # Errors
/// `index` is not an index (`FontIndex::from_bytes`).
pub fn xetex(read: ReadFont, find: FindFont, index: Option<&[u8]>) -> Result<XeTeX, String> {
    let font_index = match index {
        Some(b) => Some(
            FontIndex::from_bytes(b)
                .ok_or_else(|| "the font index is not one (PXFI, version 1 or 2)".to_owned())?,
        ),
        None => None,
    };
    Ok(xetex_of(read, find, font_index))
}

/// [`xetex`] with the index read already (a host that rewrote its paths:
/// the CLI's Shelf names them under TeX Live's tree).
pub fn xetex_of(read: ReadFont, find: FindFont, index: Option<FontIndex>) -> XeTeX {
    XeTeX {
        fonts: Some(Arc::new(HostFonts { read, find })),
        font_index: index.map(Arc::new),
        pdf: None,
        ..XeTeX::default()
    }
}

/// The names kpathsea's `kpse_find_file(name, format, 0)` tries in each
/// directory of the format's path, in order (`kpathsea_find_file_generic`
/// with TeX Live's `try_std_extension_first = t`): the name with each of
/// the format's suffixes unless it ends in one, then the name itself
/// unless the format searches suffixed names only (OpenType's). A host
/// tries them all in the project's directory (kpathsea's `.`, first on
/// every font path) before its tree's.
#[must_use]
pub fn kpse_targets(name: &str, format: KpseFormat) -> Vec<String> {
    let (suffixes, suffix_only): (&[&str], bool) = match format {
        KpseFormat::OpenType => (&[".otf", ".OTF"], true),
        KpseFormat::TrueType => (&[".ttf", ".ttc", ".TTF", ".TTC", ".dfont"], false),
        KpseFormat::Type1 => (&[".pfa", ".pfb"], false),
        KpseFormat::MiscFonts => (&[], false),
    };
    let has = suffixes.iter().any(|s| name.ends_with(s));
    let mut t = Vec::new();
    if !has {
        t.extend(suffixes.iter().map(|s| format!("{name}{s}")));
    }
    if has || !suffix_only {
        t.push(name.to_owned());
    }
    t
}

/// kpathsea's format name of `format`, as Shelf's index keys it.
#[must_use]
pub fn kpse_kind(format: KpseFormat) -> &'static str {
    match format {
        KpseFormat::OpenType => "opentype",
        KpseFormat::TrueType => "truetype",
        KpseFormat::Type1 => "type1",
        KpseFormat::MiscFonts => "misc",
    }
}

/// What a host without the system's fonts says beside an error of a
/// font not found ([`XeTeX::fonts_note`]): the fonts it has, and how a
/// font of one's own is used there.
pub const SHELF_FONTS_NOTE: &str = "Here XeTeX has TeX Live's fonts (from Shelf) and the project's own font files, not the fonts installed on this computer: a font that is not in TeX Live works when its file is in the project and is loaded by file name (\\setmainfont{MyFont.otf}, or [Path=fonts/] for a subfolder). fontconfig does not list the project's folder, so a family name finds only TeX Live's fonts, as with xelatex.";

#[cfg(test)]
mod tests {
    use super::*;

    /// kpathsea's names for a `[file]` font, in its order.
    #[test]
    fn kpse_targets_in_kpathseas_order() {
        assert_eq!(
            kpse_targets("MyFont", KpseFormat::OpenType),
            ["MyFont.otf", "MyFont.OTF"]
        );
        assert_eq!(
            kpse_targets("MyFont.otf", KpseFormat::OpenType),
            ["MyFont.otf"]
        );
        assert_eq!(
            kpse_targets("a.b", KpseFormat::TrueType),
            [
                "a.b.ttf",
                "a.b.ttc",
                "a.b.TTF",
                "a.b.TTC",
                "a.b.dfont",
                "a.b"
            ]
        );
        assert_eq!(kpse_targets("x.pfb", KpseFormat::Type1), ["x.pfb"]);
        assert_eq!(
            kpse_targets("x.otf", KpseFormat::Type1),
            ["x.otf.pfa", "x.otf.pfb", "x.otf"]
        );
        assert_eq!(kpse_targets("m.tec", KpseFormat::MiscFonts), ["m.tec"]);
    }
}
