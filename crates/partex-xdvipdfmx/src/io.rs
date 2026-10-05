//! The files xdvipdfmx reads: kpathsea's lookups and the file system, as
//! the host gives them.

use alloc::sync::Arc;
use alloc::vec::Vec;

/// kpathsea's file formats dvipdfm-x looks files up in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `kpse_fontmap_format` (`.map`; also AGL `.txt` files).
    Fontmap,
    /// `kpse_type1_format` (`.pfb`, `.pfa`).
    Type1,
    /// `kpse_truetype_format` (`.ttf`, `.ttc`, `.dfont`).
    TrueType,
    /// `kpse_opentype_format` (`.otf`).
    OpenType,
    /// `kpse_cmap_format`.
    Cmap,
    /// `kpse_sfd_format`.
    Sfd,
    /// `kpse_enc_format`.
    Enc,
    /// `kpse_tfm_format`.
    Tfm,
    /// `kpse_ofm_format`.
    Ofm,
    /// `kpse_vf_format`.
    Vf,
    /// `kpse_ovf_format`.
    Ovf,
    /// `kpse_pict_format` (graphics).
    Pict,
    /// `kpse_tex_format`.
    Tex,
    /// `kpse_program_text_format`.
    ProgramText,
    /// `kpse_program_binary_format`.
    ProgramBinary,
}

/// What the host gives xdvipdfmx.
pub trait Files {
    /// `kpse_find_file(name, format, 0)` as the program `progname` (dvipdfm-x
    /// searches as `dvipdfmx`, and "fools" kpathsea with other names for
    /// some formats): the path found.
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>>;

    /// A file's contents by path (a path `find` returned, or an absolute
    /// path from the XDV).
    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>>;
}
