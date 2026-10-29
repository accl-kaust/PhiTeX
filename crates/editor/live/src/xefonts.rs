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
    }
}
