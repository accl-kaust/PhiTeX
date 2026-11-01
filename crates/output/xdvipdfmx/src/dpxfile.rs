//! Finding and opening files (dpxfile.c), through the host's kpathsea.

use alloc::vec::Vec;

use crate::ctx::{Dpx, Result};
use crate::io::Format;
use crate::some;
use crate::stream::MemFile;

/// `dpx_res_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResType {
    Fontmap,
    T1Font,
    TtFont,
    OtFont,
    PkFont,
    DFont,
    Enc,
    Cmap,
    Sfd,
    Agl,
    IccProfile,
    Binary,
    Text,
}

/// `ensuresuffix`: `sfx` appended unless the name has a dot.
fn ensuresuffix(base: &[u8], sfx: &[u8]) -> Vec<u8> {
    let mut p = base.to_vec();
    if !base.contains(&b'.') && !sfx.is_empty() {
        p.extend_from_slice(sfx);
    }
    p
}

fn is_absolute_path(name: &[u8]) -> bool {
    name.first() == Some(&b'/')
}

impl Dpx {
    fn kpse_find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        self.files.find(name, format, progname)
    }

    fn find_app_xyz(&mut self, filename: &[u8], suffix: &[u8], is_text: bool) -> Option<Vec<u8>> {
        let q = ensuresuffix(filename, suffix);
        let f = if is_text {
            Format::ProgramText
        } else {
            Format::ProgramBinary
        };
        let r = self.kpse_find(&q, f, b"dvipdfmx");
        if r.is_none() && q != filename {
            return self.kpse_find(filename, f, b"dvipdfmx");
        }
        r
    }

    fn foolsearch(&mut self, fool: &[u8], filename: &[u8], is_text: bool) -> Option<Vec<u8>> {
        let f = if is_text {
            Format::ProgramText
        } else {
            Format::ProgramBinary
        };
        self.kpse_find(filename, f, fool)
    }

    fn find_fontmap_file(&mut self, filename: &[u8]) -> Option<Vec<u8>> {
        let q = ensuresuffix(filename, b".map");
        self.kpse_find(&q, Format::Fontmap, b"dvipdfmx")
            .or_else(|| self.find_app_xyz(&q, b".map", true))
    }

    fn find_agl_file(&mut self, filename: &[u8]) -> Option<Vec<u8>> {
        let q = ensuresuffix(filename, b".txt");
        self.kpse_find(&q, Format::Fontmap, b"dvipdfmx")
            .or_else(|| self.find_app_xyz(&q, b".txt", true))
    }

    fn find_cmap_file(&mut self, filename: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut fqpn = self.kpse_find(filename, Format::Cmap, b"dvipdfmx");
        for fool in [&b"cmap"[..], b"tex"] {
            if fqpn.is_some() {
                break;
            }
            fqpn = self.foolsearch(fool, filename, true);
            if let Some(p) = &fqpn
                && !self.qcheck_filetype(&p.clone(), ResType::Cmap)?
            {
                fqpn = None;
            }
        }
        Ok(fqpn)
    }

    fn find_sfd_file(&mut self, filename: &[u8]) -> Option<Vec<u8>> {
        let q = ensuresuffix(filename, b".sfd");
        let mut fqpn = self.kpse_find(&q, Format::Sfd, b"dvipdfmx");
        for fool in [&b"ttf2pk"[..], b"ttf2tfm"] {
            if fqpn.is_some() {
                break;
            }
            fqpn = self.foolsearch(fool, &q, true);
        }
        fqpn
    }

    fn find_enc_file(&mut self, filename: &[u8]) -> Option<Vec<u8>> {
        let q = ensuresuffix(filename, b".enc");
        self.kpse_find(&q, Format::Enc, b"dvipdfmx")
            .or_else(|| self.foolsearch(b"dvips", &q, true))
    }

    fn find_iccp_file(&mut self, filename: &[u8]) -> Option<Vec<u8>> {
        let f = self.find_app_xyz(filename, b"", false);
        if f.is_some() || filename.contains(&b'.') {
            return f;
        }
        self.find_app_xyz(filename, b".icc", false)
            .or_else(|| self.find_app_xyz(filename, b".icm", false))
    }

    /// `dpx_find_type1_file`.
    pub fn dpx_find_type1_file(&mut self, filename: &[u8]) -> Result<Option<Vec<u8>>> {
        let f = some!(if is_absolute_path(filename) {
            Some(filename.to_vec())
        } else {
            self.kpse_find(filename, Format::Type1, b"dvipdfmx")
        });
        Ok(self.qcheck_filetype(&f, ResType::T1Font)?.then_some(f))
    }

    /// `dpx_find_truetype_file`.
    pub fn dpx_find_truetype_file(&mut self, filename: &[u8]) -> Result<Option<Vec<u8>>> {
        let f = some!(if is_absolute_path(filename) {
            Some(filename.to_vec())
        } else {
            self.kpse_find(filename, Format::TrueType, b"dvipdfmx")
        });
        Ok(self.qcheck_filetype(&f, ResType::TtFont)?.then_some(f))
    }

    /// `dpx_find_opentype_file`.
    pub fn dpx_find_opentype_file(&mut self, filename: &[u8]) -> Result<Option<Vec<u8>>> {
        let q = ensuresuffix(filename, b".otf");
        let f = if is_absolute_path(&q) {
            Some(q.clone())
        } else {
            self.kpse_find(&q, Format::OpenType, b"dvipdfmx")
        };
        let f = some!(f.or_else(|| self.foolsearch(b"dvipdfmx", &q, false)));
        Ok(self.qcheck_filetype(&f, ResType::OtFont)?.then_some(f))
    }

    /// `dpx_find_dfont_file`.
    pub fn dpx_find_dfont_file(&mut self, filename: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut f = some!(self.kpse_find(filename, Format::TrueType, b"dvipdfmx"));
        let len = f.len();
        if len > 6 && &f[len - 6..] != b".dfont" {
            f.extend_from_slice(b"/rsrc");
        }
        Ok(self.qcheck_filetype(&f, ResType::DFont)?.then_some(f))
    }

    /// `dpx_open_file`.
    pub fn dpx_open_file(&mut self, filename: &[u8], ty: ResType) -> Result<Option<MemFile>> {
        let fqpn = some!(match ty {
            ResType::Fontmap => self.find_fontmap_file(filename),
            ResType::T1Font => self.dpx_find_type1_file(filename)?,
            ResType::TtFont => self.dpx_find_truetype_file(filename)?,
            ResType::OtFont => self.dpx_find_opentype_file(filename)?,
            ResType::PkFont => None,
            ResType::Cmap => self.find_cmap_file(filename)?,
            ResType::Enc => self.find_enc_file(filename),
            ResType::Sfd => self.find_sfd_file(filename),
            ResType::Agl => self.find_agl_file(filename),
            ResType::IccProfile => self.find_iccp_file(filename),
            ResType::DFont => self.dpx_find_dfont_file(filename)?,
            ResType::Binary => self.find_app_xyz(filename, b"", false),
            ResType::Text => self.find_app_xyz(filename, b"", true),
        });
        let data = some!(self.files.read(&fqpn));
        Ok(Some(MemFile::new(data, &fqpn)))
    }

    /// Opens a file by the path a finder returned (C's `MFOPEN`).
    pub fn mfopen(&mut self, path: &[u8]) -> Option<MemFile> {
        let data = self.files.read(path)?;
        Some(MemFile::new(data, path))
    }

    /// `qcheck_filetype`: the file exists, is not empty, and looks like
    /// its type.
    fn qcheck_filetype(&mut self, fqpn: &[u8], ty: ResType) -> Result<bool> {
        let Some(data) = self.files.read(fqpn) else {
            return Ok(false);
        };
        if data.is_empty() {
            return Ok(false);
        }
        let d = &data[..];
        match ty {
            ResType::T1Font => {
                if d.len() < 21 || d[0] != 0x80 || !(0..=3).contains(&(d[1] as i8)) {
                    return Ok(false);
                }
                Ok(d[6..].starts_with(b"%!PS-AdobeFont")
                    || d[6..].starts_with(b"%!FontType1")
                    || d[6..].starts_with(b"%!PS"))
            }
            ResType::TtFont => Ok(d.len() >= 4
                && (&d[..4] == b"true" || &d[..4] == b"\0\x01\0\0" || &d[..4] == b"ttcf")),
            ResType::OtFont => Ok(d.len() >= 4 && &d[..4] == b"OTTO"),
            ResType::Cmap => {
                let mut f = MemFile::new(data.clone(), fqpn);
                let Some(mut line) = f.mfgets(128) else {
                    return Ok(false);
                };
                line.truncate(127);
                if line.len() < 4 || &line[..4] != b"%!PS" {
                    return Ok(false);
                }
                let mut p = 4;
                while p < line.len() && !crate::fmt::is_c_space(line[p]) {
                    p += 1;
                }
                while p < line.len() && (line[p] == b' ' || line[p] == b'\t') {
                    p += 1;
                }
                Ok(line[p..].starts_with(b"Resource-CMap"))
            }
            ResType::DFont => {
                let mut f = MemFile::new(data.clone(), fqpn);
                if f.len() < 8 {
                    return Ok(false);
                }
                f.get_unsigned_quad()?;
                let pos = f.get_unsigned_quad()? as usize;
                f.seek_absolute(pos + 0x18);
                let off = f.get_unsigned_pair()? as usize;
                f.seek_absolute(pos + off);
                let n = f.get_unsigned_pair()?;
                for _ in 0..=n {
                    if f.get_unsigned_quad()? == 0x7366_6e74 {
                        return Ok(true);
                    }
                    f.get_unsigned_quad()?;
                }
                Ok(false)
            }
            _ => Ok(true),
        }
    }
}
