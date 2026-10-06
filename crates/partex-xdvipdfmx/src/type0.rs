//! type0.c, type0.h: Type0 (composite) fonts.
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; `cid_id` is
//! the descendant CIDFont's font id.

#![allow(non_snake_case)]

use crate::prelude::*;

/// `CMAP_PART0` (`create_dummy_CMap`'s Adobe-Identity-UCS2 header).
pub const CMAP_PART0: &[u8] = b"%!PS-Adobe-3.0 Resource-CMap\n\
%%DocumentNeededResources: ProcSet (CIDInit)\n\
%%IncludeResource: ProcSet (CIDInit)\n\
%%BeginResource: CMap (Adobe-Identity-UCS2)\n\
%%Title: (Adobe-Identity-UCS2 Adobe UCS2 0)\n\
%%Version: 1.0\n\
%%Copyright:\n\
%% ---\n\
%%EndComments\n\n";

/// `CMAP_PART1`.
pub const CMAP_PART1: &[u8] = b"/CIDInit /ProcSet findresource begin\n\
\n\
12 dict begin\n\nbegincmap\n\n\
/CIDSystemInfo 3 dict dup begin\n  \
/Registry (Adobe) def\n  \
/Ordering (UCS2) def\n  \
/Supplement 0 def\n\
end def\n\n\
/CMapName /Adobe-Identity-UCS2 def\n\
/CMapVersion 1.0 def\n\
/CMapType 2 def\n\n\
2 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n";

/// `CMAP_PART3`.
pub const CMAP_PART3: &[u8] = b"endcmap\n\n\
CMapName currentdict /CMap defineresource pop\n\n\
end\nend\n\n\
%%EndResource\n\
%%EOF\n";

/// A C string: the bytes before the first NUL.
fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `%02X`.
fn hex2(b: &mut Vec<u8>, v: i32) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    b.push(HEX[((v >> 4) & 0xf) as usize]);
    b.push(HEX[(v & 0xf) as usize]);
}

impl Dpx {
    /// `try_load_ToUnicode_file` (static): `<cmap_base>-UTF16`, then
    /// `-UCS2`; a reference.
    fn try_load_ToUnicode_file(&mut self, cmap_base: &[u8]) -> Result<Option<Obj>> {
        let mut cmap_name = cmap_base.to_vec();
        cmap_name.extend_from_slice(b"-UTF16");
        let mut tounicode = self.pdf_read_ToUnicode_file(&cmap_name)?;
        if tounicode.is_none() {
            let mut cmap_name = cmap_base.to_vec();
            cmap_name.extend_from_slice(b"-UCS2");
            tounicode = self.pdf_read_ToUnicode_file(&cmap_name)?;
        }
        Ok(tounicode)
    }

    /// `Type0Font_attach_ToUnicode_stream` (static).
    fn Type0Font_attach_ToUnicode_stream(&mut self, font_id: i32) -> Result<()> {
        let cid_id = self.font.fonts[font_id as usize].type0.descendant;
        assert!(cid_id >= 0);

        // ToUnicode CMaps are usually not required for standard character
        // collections such as Adobe-Japan1. Identity-H is used for UCS
        // ordering CID-keyed fonts. External resource must be loaded for
        // others.
        if self.CIDFont_is_ACCFont(cid_id) {
            // No need to embed ToUnicode
            return Ok(());
        } else if self.CIDFont_is_UCSFont(cid_id) {
            // Old version of dvipdfmx mistakenly used Adobe-Identity as
            // Unicode. (ref returned)
            let tounicode = match self.pdf_read_ToUnicode_file(b"Adobe-Identity-UCS2")? {
                Some(t) => t,
                // This should work
                None => self.o.new_name(b"Identity-H"),
            };
            let resource = self.font.fonts[font_id as usize].resource.unwrap();
            self.o.put(resource, b"ToUnicode", tounicode);
            return Ok(());
        }

        let cidfont = &self.font.fonts[cid_id as usize];
        let registry = cidfont.cid.csi.registry.clone().unwrap_or_default();
        let ordering = cidfont.cid.csi.ordering.clone().unwrap_or_default();
        let cid_fontname = cidfont.fontname.clone().unwrap_or_default();
        let fontname = if cidfont.cid.options.embed != 0 {
            let mut f = cstr(&cidfont.unique_id).to_vec();
            f.push(b'+');
            f.extend_from_slice(&cid_fontname);
            f
        } else {
            cid_fontname.clone()
        };
        let subtype = cidfont.subtype;
        let ident = cidfont.ident.clone().unwrap_or_default();
        let index = cidfont.index;
        let flags = cidfont.flags;
        let usedchars: Vec<u8> = self.font.fonts[font_id as usize]
            .usedchars
            .as_ref()
            .map(|u| u.borrow().clone())
            .unwrap_or_default();

        let tounicode = match subtype {
            crate::pdffont::PDF_FONT_FONTTYPE_CIDTYPE2 => {
                if registry == b"Adobe" && ordering == b"Identity" {
                    self.otf_create_ToUnicode_stream(&ident, index, &fontname, &usedchars)?
                } else {
                    let mut cmap_base = registry.clone();
                    cmap_base.push(b'-');
                    cmap_base.extend_from_slice(&ordering);
                    // In this case glyphs are re-ordered hance otf_create...
                    // won't work
                    self.try_load_ToUnicode_file(&cmap_base)?
                }
            }
            _ => {
                if flags & crate::pdffont::CIDFONT_FLAG_TYPE1C != 0 {
                    self.otf_create_ToUnicode_stream(&ident, index, &fontname, &usedchars)?
                } else if flags & crate::pdffont::CIDFONT_FLAG_TYPE1 != 0 {
                    self.CIDFont_type0_t1create_ToUnicode_stream(&ident, &fontname, &usedchars)?
                } else {
                    let t = self.try_load_ToUnicode_file(&cid_fontname)?;
                    if t.is_none() {
                        self.otf_create_ToUnicode_stream(&ident, index, &fontname, &usedchars)?
                    } else {
                        t
                    }
                }
            }
        };

        if let Some(tounicode) = tounicode {
            let resource = self.font.fonts[font_id as usize].resource.unwrap();
            self.o.put(resource, b"ToUnicode", tounicode);
        } else {
            warn!("Failed to load ToUnicode CMap for font");
        }
        Ok(())
    }

    /// `pdf_font_load_type0`.
    pub fn pdf_font_load_type0(&mut self, font_id: i32) -> Result<()> {
        if font_id < 0 || self.font.fonts[font_id as usize].reference.is_none() {
            return Ok(());
        }

        // FIXME: Should move to pdffont.c
        let resource = self.font.fonts[font_id as usize].resource.unwrap();
        if self.o.lookup_dict(resource, b"ToUnicode").is_none() {
            self.Type0Font_attach_ToUnicode_stream(font_id)?;
        }
        Ok(())
    }

    /// `pdf_font_open_type0`: 0, or -1 when `cid_id < 0`.
    pub fn pdf_font_open_type0(&mut self, font_id: i32, cid_id: i32, wmode: i32) -> i32 {
        if cid_id < 0 {
            return -1;
        }

        {
            let font = &mut self.font.fonts[font_id as usize];
            font.type0.wmode = wmode;
            font.type0.descendant = cid_id;
        }

        // PostScript Font name:
        //  Type0 font's fontname is usually descendant CID-keyed font's
        //  font name appended by -ENCODING.
        let cidfont = &self.font.fonts[cid_id as usize];
        let cid_fontname = cidfont.fontname.clone().unwrap_or_default();
        let fontname = if cidfont.cid.options.embed != 0 {
            let mut f = cstr(&cidfont.unique_id).to_vec();
            f.push(b'+');
            f.extend_from_slice(&cid_fontname);
            f
        } else {
            cid_fontname
        };

        match cidfont.subtype {
            crate::pdffont::PDF_FONT_FONTTYPE_CIDTYPE0 => {
                let mut name = fontname.clone();
                name.push(b'-');
                name.extend_from_slice(if wmode != 0 {
                    b"Identity-V"
                } else {
                    b"Identity-H"
                });
                let usedchars = self.CIDFont_get_usedchars(cid_id);
                let font = &mut self.font.fonts[font_id as usize];
                font.fontname = Some(name);
                font.usedchars = Some(usedchars);
                font.flags |= crate::pdffont::PDF_FONT_FLAG_USEDCHAR_SHARED;
                if wmode != 0 {
                    self.font.fonts[cid_id as usize].cid.need_vmetrics = 1;
                }
            }
            crate::pdffont::PDF_FONT_FONTTYPE_CIDTYPE2 => {
                self.font.fonts[font_id as usize].fontname = Some(fontname.clone());
                // Adobe-Identity here means use GID as CID directly. No need
                // to use GSUB for finding vertical glyphs hence separate
                // used_chars for H and V instances are not needed.
                let csi = &self.font.fonts[cid_id as usize].cid.csi;
                let usedchars = if csi.registry.as_deref() == Some(b"Adobe".as_slice())
                    && csi.ordering.as_deref() == Some(b"Identity".as_slice())
                {
                    self.CIDFont_get_usedchars(cid_id)
                } else if wmode != 0 {
                    self.CIDFont_get_usedchars_v(cid_id)
                } else {
                    self.CIDFont_get_usedchars(cid_id)
                };
                let font = &mut self.font.fonts[font_id as usize];
                font.usedchars = Some(usedchars);
                font.flags |= crate::pdffont::PDF_FONT_FLAG_USEDCHAR_SHARED;
                if wmode != 0 {
                    self.font.fonts[cid_id as usize].cid.need_vmetrics = 1;
                }
            }
            _ => {}
        }

        let resource = self.o.new_dict();
        self.font.fonts[font_id as usize].resource = Some(resource);
        self.o.put_name(resource, b"Type", b"Font");
        self.o.put_name(resource, b"Subtype", b"Type0");
        let fname = self.font.fonts[font_id as usize]
            .fontname
            .clone()
            .unwrap_or_default();
        self.o.put_name(resource, b"BaseFont", &fname);
        self.o.put_name(
            resource,
            b"Encoding",
            if wmode != 0 {
                b"Identity-V"
            } else {
                b"Identity-H"
            },
        );

        0
    }

    /// `create_dummy_CMap` (static): the Adobe-Identity-UCS2 stream.
    fn create_dummy_CMap(&mut self) -> Obj {
        let stream = self.o.new_stream(crate::obj::STREAM_COMPRESS);
        self.o.add_stream(stream, CMAP_PART0);
        self.o.add_stream(stream, CMAP_PART1);
        let range = |b: &mut Vec<u8>, i: i32| {
            // "<%02X00> <%02XFF> <%02X00>\n"
            b.push(b'<');
            hex2(b, i);
            b.extend_from_slice(b"00> <");
            hex2(b, i);
            b.extend_from_slice(b"FF> <");
            hex2(b, i);
            b.extend_from_slice(b"00>\n");
        };
        self.o.add_stream(stream, b"\n100 beginbfrange\n");
        for i in 0..0x64 {
            let mut buf = Vec::new();
            range(&mut buf, i);
            self.o.add_stream(stream, &buf);
        }
        self.o.add_stream(stream, b"endbfrange\n\n");

        self.o.add_stream(stream, b"\n100 beginbfrange\n");
        for i in 0x64..0xc8 {
            let mut buf = Vec::new();
            range(&mut buf, i);
            self.o.add_stream(stream, &buf);
        }
        self.o.add_stream(stream, b"endbfrange\n\n");

        self.o.add_stream(stream, b"\n48 beginbfrange\n");
        for i in 0xc8..=0xd7 {
            let mut buf = Vec::new();
            range(&mut buf, i);
            self.o.add_stream(stream, &buf);
        }
        for i in 0xe0..=0xff {
            let mut buf = Vec::new();
            range(&mut buf, i);
            self.o.add_stream(stream, &buf);
        }
        self.o.add_stream(stream, b"endbfrange\n\n");

        self.o.add_stream(stream, CMAP_PART3);

        stream
    }

    /// `pdf_read_ToUnicode_file` (static): a reference to the "CMap"
    /// resource (defined `PDF_RES_FLUSH_IMMEDIATE`), or none.
    fn pdf_read_ToUnicode_file(&mut self, cmap_name: &[u8]) -> Result<Option<Obj>> {
        let mut res_id = self.pdf_findresource(b"CMap", cmap_name)?;
        if res_id < 0 {
            let stream = if cmap_name == b"Adobe-Identity-UCS2" {
                Some(self.create_dummy_CMap())
            } else {
                self.pdf_load_ToUnicode_stream(cmap_name)?
            };
            if let Some(stream) = stream {
                res_id = self.pdf_defineresource(
                    b"CMap",
                    Some(cmap_name),
                    stream,
                    crate::pdfresource::PDF_RES_FLUSH_IMMEDIATE,
                )?;
            }
        }

        if res_id < 0 {
            Ok(None)
        } else {
            self.pdf_get_resource_reference(res_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_cmap_parts() {
        assert!(CMAP_PART1.starts_with(
            b"/CIDInit /ProcSet findresource begin\n\n12 dict begin\n\nbegincmap\n\n/CIDSystemInfo 3 dict dup begin\n  /Registry (Adobe) def\n  /Ordering (UCS2) def\n  /Supplement 0 def\nend def\n\n"
        ));
        assert!(CMAP_PART0.ends_with(b"%% ---\n%%EndComments\n\n"));
        assert_eq!(
            CMAP_PART3,
            b"endcmap\n\nCMapName currentdict /CMap defineresource pop\n\nend\nend\n\n%%EndResource\n%%EOF\n"
        );
        let mut b = Vec::new();
        hex2(&mut b, 0xab);
        assert_eq!(b, b"AB");
    }
}
