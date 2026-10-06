//! pdfencoding.c, pdfencoding.h: the encodings (`.enc` files and the PDF
//! predefined ones), their `/Encoding` resources and ToUnicode CMaps.
//!
//! The cache is `self.encoding.encodings`, indexed by `enc_id`.

use crate::prelude::*;

/// `FLAG_IS_PREDEFINED`.
pub const FLAG_IS_PREDEFINED: i32 = 1 << 0;
/// `FLAG_USED_BY_TYPE3`.
pub const FLAG_USED_BY_TYPE3: i32 = 1 << 1;
/// `CACHE_ALLOC_SIZE`.
pub const CACHE_ALLOC_SIZE: u32 = 16;
/// `WBUF_SIZE`.
pub const WBUF_SIZE: usize = 1024;

/// `pdf_encoding`.
#[derive(Clone, Debug)]
pub struct PdfEncoding {
    pub ident: Vec<u8>,
    pub enc_name: Vec<u8>,
    pub flags: i32,
    /// `.notdef` is `None`.
    pub glyphs: [Option<Vec<u8>>; 256],
    pub is_used: [u8; 256],
    pub tounicode: Option<Obj>,
    pub resource: Option<Obj>,
}

impl Default for PdfEncoding {
    fn default() -> Self {
        PdfEncoding {
            ident: Vec::new(),
            enc_name: Vec::new(),
            flags: 0,
            glyphs: core::array::from_fn(|_| None),
            is_used: [0; 256],
            tounicode: None,
            resource: None,
        }
    }
}

/// pdfencoding.c's statics: `enc_cache` (count = `encodings.len()`), and
/// `wbuf`, `range_min`, `range_max` (scratch for the ToUnicode CMaps).
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `enc_cache.encodings`.
    pub encodings: Vec<PdfEncoding>,
    /// `enc_cache.capacity`.
    pub capacity: i32,
}

/// `pdf_init_encoding_struct`.
pub fn pdf_init_encoding_struct(encoding: &mut PdfEncoding) {
    encoding.ident = Vec::new();
    encoding.enc_name = Vec::new();
    encoding.glyphs = core::array::from_fn(|_| None);
    encoding.is_used = [0; 256];
    encoding.tounicode = None;
    encoding.resource = None;
    encoding.flags = 0;
}

/// `is_similar_charset` (static).
fn is_similar_charset(enc_vec: &[Option<Vec<u8>>], enc_vec2: &[&[u8]; 256]) -> bool {
    let mut same = 0;
    for code in 0..256 {
        let differs = match &enc_vec[code] {
            Some(g) => g.as_slice() != enc_vec2[code],
            None => false,
        };
        if !differs {
            same += 1;
            if same >= 64 {
                return true;
            }
        }
    }
    false
}

/// A predefined table as `pdf_encoding_new_encoding` takes it.
fn predefined_vec(table: &[&[u8]; 256]) -> Vec<Option<Vec<u8>>> {
    table.iter().map(|g| Some(g.to_vec())).collect()
}

impl Dpx {
    /// `CHECK_ID`.
    fn encoding_check_id(&self, n: i32) -> Result<()> {
        if n < 0 || n as usize >= self.encoding.encodings.len() {
            fatal!("Invalid encoding id: {}", n);
        }
        Ok(())
    }

    /// `create_encoding_resource` (static).
    fn create_encoding_resource(
        &mut self,
        enc_vec: &[Option<Vec<u8>>],
        baseenc_name: Option<&[u8]>,
        baseenc_vec: Option<&[&[u8]; 256]>,
        is_used: &[u8],
    ) -> Result<Option<Obj>> {
        if let Some(b) = baseenc_name
            && b != b"MacRomanEncoding"
            && b != b"MacExpertEncoding"
            && b != b"WinAnsiEncoding"
        {
            fatal!("Invalid encoding name for BaseEncoding");
        }
        let differences = self.make_encoding_differences(enc_vec, baseenc_vec, is_used)?;
        if let Some(differences) = differences {
            let resource = self.o.new_dict();
            if let Some(b) = baseenc_name {
                self.o.put_name(resource, b"BaseEncoding", b)?;
            }
            self.o.put(resource, b"Differences", differences)?;
            Ok(Some(resource))
        } else {
            Ok(baseenc_name.map(|b| self.o.new_name(b)))
        }
    }

    /// `pdf_flush_encoding` (static).
    fn pdf_flush_encoding(&mut self, enc_id: i32) -> Result<()> {
        let encoding = &mut self.encoding.encodings[enc_id as usize];
        let resource = encoding.resource.take();
        let tounicode = encoding.tounicode.take();
        if let Some(r) = resource {
            self.o.release(r)?;
        }
        if let Some(t) = tounicode {
            self.o.release(t)?;
        }
        Ok(())
    }

    /// `pdf_clean_encoding_struct` (static).
    fn pdf_clean_encoding_struct(&mut self, enc_id: i32) -> Result<()> {
        let encoding = &mut self.encoding.encodings[enc_id as usize];
        if encoding.resource.is_some() {
            fatal!("Object not flushed.");
        }
        let tounicode = encoding.tounicode.take();
        encoding.ident = Vec::new();
        encoding.enc_name = Vec::new();
        encoding.glyphs = core::array::from_fn(|_| None);
        if let Some(t) = tounicode {
            self.o.release(t)?;
        }
        Ok(())
    }

    /// `make_encoding_differences` (static).
    fn make_encoding_differences(
        &mut self,
        enc_vec: &[Option<Vec<u8>>],
        baseenc: Option<&[&[u8]; 256]>,
        is_used: &[u8],
    ) -> Result<Option<Obj>> {
        let mut count = 0;
        let mut skipping = true;
        let differences = self.o.new_array();
        for code in 0..256 {
            match &enc_vec[code] {
                Some(g) if is_used[code] != 0 => {
                    if baseenc.is_none_or(|b| b[code] != g.as_slice()) {
                        if skipping {
                            let n = self.o.new_number(code as f64);
                            self.o.add_array(differences, n)?;
                        }
                        let n = self.o.new_name(g);
                        self.o.add_array(differences, n)?;
                        skipping = false;
                        count += 1;
                    } else {
                        skipping = true;
                    }
                }
                _ => skipping = true,
            }
        }
        if count == 0 {
            self.o.release(differences)?;
            return Ok(None);
        }
        Ok(Some(differences))
    }

    /// `load_encoding_file` (static): the enc_id, or -1.
    fn load_encoding_file(&mut self, filename: &[u8]) -> Result<i32> {
        use crate::parse::{skip_line, skip_white};
        let Some(fp) = self.dpx_open_file(filename, crate::dpxfile::ResType::Enc)? else {
            return Ok(-1);
        };
        let wbuf = fp.data.clone();
        let s = &wbuf[..];
        let mut p = 0;
        skip_white(s, &mut p);
        while p < s.len() && s[p] == b'%' {
            skip_line(s, &mut p);
            skip_white(s, &mut p);
        }
        let mut enc_name = None;
        if p < s.len() && s[p] == b'/' {
            enc_name = self.o.parse_pdf_name(s, &mut p);
        }
        skip_white(s, &mut p);
        let Some(encoding_array) = self.o.parse_pdf_array(s, &mut p, None)? else {
            if let Some(n) = enc_name {
                self.o.release(n)?;
            }
            return Ok(-1);
        };
        let mut enc_vec: Vec<Option<Vec<u8>>> = Vec::with_capacity(256);
        for code in 0..256 {
            let Some(g) = self.o.get_array(encoding_array, code)? else {
                fatal!("typecheck: Invalid object type: -1 4");
            };
            enc_vec.push(Some(self.o.name_value(g)?.to_vec()));
        }
        let name = match enc_name {
            Some(n) => Some(self.o.name_value(n)?.to_vec()),
            None => None,
        };
        let Some(name) = name else {
            // C: strlen(NULL) in pdf_encoding_new_encoding.
            fatal!("Encoding file without a name: crash in C");
        };
        let enc_id = self.pdf_encoding_new_encoding(&name, filename, &enc_vec, 0);
        if let Some(n) = enc_name {
            self.o.release(n)?;
        }
        self.o.release(encoding_array)?;
        Ok(enc_id)
    }

    /// `pdf_encoding_new_encoding` (static): the enc_id. `encoding_vec`
    /// holds 256 names (`.notdef` included as in the C tables).
    fn pdf_encoding_new_encoding(
        &mut self,
        enc_name: &[u8],
        ident: &[u8],
        encoding_vec: &[Option<Vec<u8>>],
        flags: i32,
    ) -> i32 {
        let enc_id = self.encoding.encodings.len() as i32;
        if enc_id >= self.encoding.capacity {
            self.encoding.capacity += 16;
        }
        let mut encoding = PdfEncoding::default();
        pdf_init_encoding_struct(&mut encoding);
        encoding.ident = ident.to_vec();
        encoding.enc_name = enc_name.to_vec();
        encoding.flags = flags;
        for code in 0..256 {
            if let Some(g) = &encoding_vec[code]
                && g.as_slice() != b".notdef"
            {
                encoding.glyphs[code] = Some(g.clone());
            }
        }
        if flags & FLAG_IS_PREDEFINED != 0 {
            encoding.resource = Some(self.o.new_name(&encoding.enc_name));
        }
        self.encoding.encodings.push(encoding);
        enc_id
    }

    /// `pdf_init_encodings`.
    pub fn pdf_init_encodings(&mut self) {
        self.encoding.encodings = Vec::new();
        self.encoding.capacity = 3;
        self.pdf_encoding_new_encoding(
            b"WinAnsiEncoding",
            b"WinAnsiEncoding",
            &predefined_vec(&WIN_ANSI_ENCODING),
            FLAG_IS_PREDEFINED,
        );
        self.pdf_encoding_new_encoding(
            b"MacRomanEncoding",
            b"MacRomanEncoding",
            &predefined_vec(&MAC_ROMAN_ENCODING),
            FLAG_IS_PREDEFINED,
        );
        self.pdf_encoding_new_encoding(
            b"MacExpertEncoding",
            b"MacExpertEncoding",
            &predefined_vec(&MAC_EXPERT_ENCODING),
            FLAG_IS_PREDEFINED,
        );
    }

    /// `pdf_encoding_complete`.
    pub fn pdf_encoding_complete(&mut self) -> Result<()> {
        for enc_id in 0..self.encoding.encodings.len() as i32 {
            if !self.pdf_encoding_is_predefined(enc_id)? {
                let encoding = &self.encoding.encodings[enc_id as usize];
                let flags = encoding.flags;
                let glyphs: Vec<Option<Vec<u8>>> = encoding.glyphs.to_vec();
                let is_used = encoding.is_used;
                let enc_name = encoding.enc_name.clone();
                let mut baseenc_name: Option<&[u8]> = None;
                let mut baseenc_vec: Option<&[&[u8]; 256]> = None;
                let with_base = flags & FLAG_USED_BY_TYPE3 == 0 || self.o.check_version(1, 4) >= 0;
                if flags & FLAG_IS_PREDEFINED == 0
                    && with_base
                    && is_similar_charset(&glyphs, &WIN_ANSI_ENCODING)
                {
                    baseenc_name = Some(b"WinAnsiEncoding");
                    baseenc_vec = Some(&WIN_ANSI_ENCODING);
                }
                let resource =
                    self.create_encoding_resource(&glyphs, baseenc_name, baseenc_vec, &is_used)?;
                self.encoding.encodings[enc_id as usize].resource = resource;
                let tounicode =
                    self.pdf_create_ToUnicode_CMap(&enc_name, &glyphs, Some(&is_used))?;
                self.encoding.encodings[enc_id as usize].tounicode = tounicode;
            }
        }
        Ok(())
    }

    /// `pdf_close_encodings`.
    pub fn pdf_close_encodings(&mut self) -> Result<()> {
        for enc_id in 0..self.encoding.encodings.len() as i32 {
            self.pdf_flush_encoding(enc_id)?;
            self.pdf_clean_encoding_struct(enc_id)?;
        }
        self.encoding.encodings = Vec::new();
        self.encoding.capacity = 0;
        Ok(())
    }

    /// `pdf_encoding_findresource`: the enc_id, or -1.
    pub fn pdf_encoding_findresource(&mut self, enc_name: &[u8]) -> Result<i32> {
        for (enc_id, encoding) in self.encoding.encodings.iter().enumerate() {
            if encoding.ident == enc_name || encoding.enc_name == enc_name {
                return Ok(enc_id as i32);
            }
        }
        self.load_encoding_file(enc_name)
    }

    /// `pdf_encoding_get_encoding`: a copy of the 256 glyph names
    /// (`None` = `.notdef`).
    pub fn pdf_encoding_get_encoding(&mut self, enc_id: i32) -> Result<Vec<Option<Vec<u8>>>> {
        self.encoding_check_id(enc_id)?;
        Ok(self.encoding.encodings[enc_id as usize].glyphs.to_vec())
    }

    /// `pdf_get_encoding_obj`: the `/Encoding` resource (not linked).
    pub fn pdf_get_encoding_obj(&mut self, enc_id: i32) -> Result<Option<Obj>> {
        self.encoding_check_id(enc_id)?;
        Ok(self.encoding.encodings[enc_id as usize].resource)
    }

    /// `pdf_encoding_is_predefined`.
    pub fn pdf_encoding_is_predefined(&mut self, enc_id: i32) -> Result<bool> {
        self.encoding_check_id(enc_id)?;
        Ok(self.encoding.encodings[enc_id as usize].flags & FLAG_IS_PREDEFINED != 0)
    }

    /// `pdf_encoding_used_by_type3`.
    pub fn pdf_encoding_used_by_type3(&mut self, enc_id: i32) -> Result<()> {
        self.encoding_check_id(enc_id)?;
        self.encoding.encodings[enc_id as usize].flags |= FLAG_USED_BY_TYPE3;
        Ok(())
    }

    /// `pdf_encoding_get_name`: a copy.
    pub fn pdf_encoding_get_name(&mut self, enc_id: i32) -> Result<Vec<u8>> {
        self.encoding_check_id(enc_id)?;
        Ok(self.encoding.encodings[enc_id as usize].enc_name.clone())
    }

    /// `pdf_encoding_add_usedchars`: ORs `is_used` (256 bytes) into the
    /// encoding's.
    pub fn pdf_encoding_add_usedchars(&mut self, encoding_id: i32, is_used: &[u8]) -> Result<()> {
        self.encoding_check_id(encoding_id)?;
        if self.pdf_encoding_is_predefined(encoding_id)? {
            return Ok(());
        }
        let encoding = &mut self.encoding.encodings[encoding_id as usize];
        for code in 0..=0xff {
            encoding.is_used[code] |= is_used[code];
        }
        Ok(())
    }

    /// `pdf_encoding_get_tounicode` (not linked).
    pub fn pdf_encoding_get_tounicode(&mut self, encoding_id: i32) -> Result<Option<Obj>> {
        self.encoding_check_id(encoding_id)?;
        Ok(self.encoding.encodings[encoding_id as usize].tounicode)
    }

    /// `pdf_create_ToUnicode_CMap`: a stream object (not a reference).
    /// `is_used` is none for all 256 codes.
    #[allow(non_snake_case)]
    pub fn pdf_create_ToUnicode_CMap(
        &mut self,
        enc_name: &[u8],
        enc_vec: &[Option<Vec<u8>>],
        is_used: Option<&[u8]>,
    ) -> Result<Option<Obj>> {
        use crate::cmap::{CMAP_TYPE_TO_UNICODE, CMap};
        let is_used = some!(is_used);
        let mut cmap_name = enc_name.to_vec();
        cmap_name.extend_from_slice(b"-UTF16");

        let mut cmap = CMap::CMap_new();
        cmap.CMap_set_name(&cmap_name);
        cmap.CMap_set_type(CMAP_TYPE_TO_UNICODE);
        cmap.CMap_set_wmode(0);
        cmap.CMap_set_CIDSysInfo(Some(&crate::cid::CSI_UNICODE()));
        cmap.CMap_add_codespacerange(&[0x00], &[0xff]);

        let mut wbuf = [0u8; WBUF_SIZE];
        let mut count = 0;
        let mut total_fail = 0;
        for code in 0..=0xff {
            if is_used[code] == 0 {
                continue;
            }
            if let Some(g) = &enc_vec[code] {
                wbuf[0] = (code & 0xff) as u8;
                let mut p = 1;
                let (len, fail_count) = self.agl_sput_UTF16BE(g, &mut wbuf, &mut p);
                if len < 1 && fail_count > 0 {
                    total_fail += 1;
                } else {
                    cmap.CMap_add_bfchar(&wbuf[..1], &wbuf[1..1 + len as usize]);
                    count += 1;
                }
            }
        }
        if total_fail > 0 {
            warn!("Glyphs with no Unicode mapping found. Removing ToUnicode CMap.");
        }
        if count == 0 || total_fail > 0 {
            Ok(None)
        } else {
            self.CMap_create_stream(&cmap)
        }
    }

    /// `pdf_load_ToUnicode_stream`: the CMap `ident` as a stream (not a
    /// reference).
    #[allow(non_snake_case)]
    pub fn pdf_load_ToUnicode_stream(&mut self, ident: &[u8]) -> Result<Option<Obj>> {
        use crate::cmap::CMap;
        use crate::cmap_read::CMap_parse_check_sig;
        let mut stream = None;
        let mut fp = some!(self.dpx_open_file(ident, crate::dpxfile::ResType::Cmap)?);
        if CMap_parse_check_sig(&mut fp) < 0 {
            return Ok(None);
        }
        let mut cmap = CMap::CMap_new();
        if self.CMap_parse(&mut cmap, &mut fp)? < 0 {
            warn!("Reading CMap file failed.");
        } else {
            stream = self.CMap_create_stream(&cmap)?;
            if stream.is_none() {
                warn!("Failed to creat ToUnicode CMap stream.");
            }
        }
        Ok(stream)
    }
}

/// `MacRomanEncoding`.
pub static MAC_ROMAN_ENCODING: [&[u8]; 256] = [
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
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
    b".notdef",
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
    b"space",
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
];

/// `MacExpertEncoding`.
pub static MAC_EXPERT_ENCODING: [&[u8]; 256] = [
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"space",
    b"exclamsmall",
    b"Hungarumlautsmall",
    b"centoldstyle",
    b"dollaroldstyle",
    b"dollarsuperior",
    b"ampersandsmall",
    b"Acutesmall",
    b"parenleftsuperior",
    b"parenrightsuperior",
    b"twodotenleader",
    b"onedotenleader",
    b"comma",
    b"hyphen",
    b"period",
    b"fraction",
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
    b"colon",
    b"semicolon",
    b".notdef",
    b"threequartersemdash",
    b".notdef",
    b"questionsmall",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"Ethsmall",
    b".notdef",
    b".notdef",
    b"onequarter",
    b"onehalf",
    b"threequarters",
    b"oneeighth",
    b"threeeighths",
    b"fiveeighths",
    b"seveneighths",
    b"onethird",
    b"twothirds",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"ff",
    b"fi",
    b"fl",
    b"ffi",
    b"ffl",
    b"parenleftinferior",
    b".notdef",
    b"parenrightinferior",
    b"Circumflexsmall",
    b"hypheninferior",
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
    b".notdef",
    b".notdef",
    b"asuperior",
    b"centsuperior",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"Aacutesmall",
    b"Agravesmall",
    b"Acircumflexsmall",
    b"Adieresissmall",
    b"Atildesmall",
    b"Aringsmall",
    b"Ccedillasmall",
    b"Eacutesmall",
    b"Egravesmall",
    b"Ecircumflexsmall",
    b"Edieresissmall",
    b"Iacutesmall",
    b"Igravesmall",
    b"Icircumflexsmall",
    b"Idieresissmall",
    b"Ntildesmall",
    b"Oacutesmall",
    b"Ogravesmall",
    b"Ocircumflexsmall",
    b"Odieresissmall",
    b"Otildesmall",
    b"Uacutesmall",
    b"Ugravesmall",
    b"Ucircumflexsmall",
    b"Udieresissmall",
    b".notdef",
    b"eightsuperior",
    b"fourinferior",
    b"threeinferior",
    b"sixinferior",
    b"eightinferior",
    b"seveninferior",
    b"Scaronsmall",
    b".notdef",
    b"centinferior",
    b"twoinferior",
    b".notdef",
    b"Dieresissmall",
    b".notdef",
    b"Caronsmall",
    b"osuperior",
    b"fiveinferior",
    b".notdef",
    b"commainferior",
    b"periodinferior",
    b"Yacutesmall",
    b".notdef",
    b"dollarinferior",
    b".notdef",
    b".notdef",
    b"Thornsmall",
    b".notdef",
    b"nineinferior",
    b"zeroinferior",
    b"Zcaronsmall",
    b"AEsmall",
    b"Oslashsmall",
    b"questiondownsmall",
    b"oneinferior",
    b"Lslashsmall",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"Cedillasmall",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"OEsmall",
    b"figuredash",
    b"hyphensuperior",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"exclamdownsmall",
    b".notdef",
    b"Ydieresissmall",
    b".notdef",
    b"onesuperior",
    b"twosuperior",
    b"threesuperior",
    b"foursuperior",
    b"fivesuperior",
    b"sixsuperior",
    b"sevensuperior",
    b"ninesuperior",
    b"zerosuperior",
    b".notdef",
    b"esuperior",
    b"rsuperior",
    b"tsuperior",
    b".notdef",
    b".notdef",
    b"isuperior",
    b"ssuperior",
    b"dsuperior",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"lsuperior",
    b"Ogoneksmall",
    b"Brevesmall",
    b"Macronsmall",
    b"bsuperior",
    b"nsuperior",
    b"msuperior",
    b"commasuperior",
    b"periodsuperior",
    b"Dotaccentsmall",
    b"Ringsmall",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
];

/// `WinAnsiEncoding`.
pub static WIN_ANSI_ENCODING: [&[u8]; 256] = [
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
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
    b"bullet",
    b"Euro",
    b"bullet",
    b"quotesinglbase",
    b"florin",
    b"quotedblbase",
    b"ellipsis",
    b"dagger",
    b"daggerdbl",
    b"circumflex",
    b"perthousand",
    b"Scaron",
    b"guilsinglleft",
    b"OE",
    b"bullet",
    b"Zcaron",
    b"bullet",
    b"bullet",
    b"quoteleft",
    b"quoteright",
    b"quotedblleft",
    b"quotedblright",
    b"bullet",
    b"endash",
    b"emdash",
    b"tilde",
    b"trademark",
    b"scaron",
    b"guilsinglright",
    b"oe",
    b"bullet",
    b"zcaron",
    b"Ydieresis",
    b"space",
    b"exclamdown",
    b"cent",
    b"sterling",
    b"currency",
    b"yen",
    b"brokenbar",
    b"section",
    b"dieresis",
    b"copyright",
    b"ordfeminine",
    b"guillemotleft",
    b"logicalnot",
    b"hyphen",
    b"registered",
    b"macron",
    b"degree",
    b"plusminus",
    b"twosuperior",
    b"threesuperior",
    b"acute",
    b"mu",
    b"paragraph",
    b"periodcentered",
    b"cedilla",
    b"onesuperior",
    b"ordmasculine",
    b"guillemotright",
    b"onequarter",
    b"onehalf",
    b"threequarters",
    b"questiondown",
    b"Agrave",
    b"Aacute",
    b"Acircumflex",
    b"Atilde",
    b"Adieresis",
    b"Aring",
    b"AE",
    b"Ccedilla",
    b"Egrave",
    b"Eacute",
    b"Ecircumflex",
    b"Edieresis",
    b"Igrave",
    b"Iacute",
    b"Icircumflex",
    b"Idieresis",
    b"Eth",
    b"Ntilde",
    b"Ograve",
    b"Oacute",
    b"Ocircumflex",
    b"Otilde",
    b"Odieresis",
    b"multiply",
    b"Oslash",
    b"Ugrave",
    b"Uacute",
    b"Ucircumflex",
    b"Udieresis",
    b"Yacute",
    b"Thorn",
    b"germandbls",
    b"agrave",
    b"aacute",
    b"acircumflex",
    b"atilde",
    b"adieresis",
    b"aring",
    b"ae",
    b"ccedilla",
    b"egrave",
    b"eacute",
    b"ecircumflex",
    b"edieresis",
    b"igrave",
    b"iacute",
    b"icircumflex",
    b"idieresis",
    b"eth",
    b"ntilde",
    b"ograve",
    b"oacute",
    b"ocircumflex",
    b"otilde",
    b"odieresis",
    b"divide",
    b"oslash",
    b"ugrave",
    b"uacute",
    b"ucircumflex",
    b"udieresis",
    b"yacute",
    b"thorn",
    b"ydieresis",
];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::io::{Files, Format};
    use alloc::sync::Arc;

    struct NoFiles;
    impl Files for NoFiles {
        fn find(&mut self, _: &[u8], _: Format, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
        fn read(&mut self, _: &[u8]) -> Option<Arc<[u8]>> {
            None
        }
    }

    /// A `Dpx` with no files, every state at its default.
    pub(crate) fn dpx() -> Dpx {
        Dpx::new(Box::new(NoFiles), Box::new(|_, d: &[u8]| d.to_vec()))
    }

    #[test]
    fn differences() {
        let mut d = dpx();
        d.pdf_init_encodings();
        assert_eq!(d.pdf_encoding_findresource(b"MacRomanEncoding").unwrap(), 1);
        assert!(d.pdf_encoding_is_predefined(2).unwrap());
        let mut enc: Vec<Option<Vec<u8>>> = WIN_ANSI_ENCODING
            .iter()
            .map(|g| (*g != b".notdef").then(|| g.to_vec()))
            .collect();
        enc[65] = Some(b"Alpha".to_vec());
        enc[66] = Some(b"Beta".to_vec());
        enc[68] = Some(b"Delta".to_vec());
        assert!(is_similar_charset(&enc, &WIN_ANSI_ENCODING));
        let mut used = [0u8; 256];
        for c in 64..70 {
            used[c] = 1;
        }
        let r = d
            .create_encoding_resource(
                &enc,
                Some(b"WinAnsiEncoding"),
                Some(&WIN_ANSI_ENCODING),
                &used,
            )
            .unwrap()
            .unwrap();
        let diff = d.o.lookup_dict(r, b"Differences").unwrap().unwrap();
        let items: Vec<Vec<u8>> =
            d.o.array_items(diff)
                .unwrap()
                .into_iter()
                .map(|o| {
                    let o = o.unwrap();
                    if d.o.is_number(Some(o)) {
                        format!("{}", d.o.number_value(o).unwrap()).into_bytes()
                    } else {
                        d.o.name_value(o).unwrap().to_vec()
                    }
                })
                .collect();
        let want: [&[u8]; 5] = [b"65", b"Alpha", b"Beta", b"68", b"Delta"];
        assert_eq!(items, want.map(<[u8]>::to_vec));
        // Nothing used: no Differences, the base name alone.
        let r = d
            .create_encoding_resource(&enc, Some(b"WinAnsiEncoding"), None, &[0; 256])
            .unwrap()
            .unwrap();
        assert_eq!(d.o.name_value(r).unwrap(), b"WinAnsiEncoding");
    }
}

#[cfg(test)]
mod tounicode_tests {
    use super::*;
    use crate::agl::AglName;

    #[test]
    fn tounicode_cmap() {
        let mut d = tests::dpx();
        for (n, u) in [
            (&b"A"[..], 0x41),
            (b"B", 0x42),
            (b"C", 0x43),
            (b"ff", 0xFB00),
            (b"f", 0x66),
        ] {
            let mut a = AglName {
                name: Some(n.to_vec()),
                n_components: 1,
                ..Default::default()
            };
            a.unicodes[0] = u;
            d.agl.aglmap.ht_append_table(n, a);
        }
        let mut enc: Vec<Option<Vec<u8>>> = vec![None; 256];
        enc[0x41] = Some(b"A".to_vec());
        enc[0x42] = Some(b"B".to_vec());
        enc[0x43] = Some(b"C".to_vec());
        enc[0x0b] = Some(b"ff".to_vec());
        enc[0x0c] = Some(b"f_f".to_vec());
        enc[0x0d] = Some(b"uni0041.sc".to_vec());
        let used = [1u8; 256];
        let s = d
            .pdf_create_ToUnicode_CMap(b"Test", &enc, Some(&used))
            .unwrap()
            .unwrap();
        let text = String::from_utf8(d.o.stream_data(s).unwrap().to_vec()).unwrap();
        assert!(text.contains("/CMapName /Test-UTF16 def"), "{text}");
        assert!(
            text.contains("3 beginbfchar\n<0B> <FB00>\n<0C> <00660066>\n<0D> <0041>\nendbfchar\n"),
            "{text}"
        );
        assert!(
            text.contains("1 beginbfrange\n<41> <43> <0041>\nendbfrange\n"),
            "{text}"
        );
        // A glyph with no mapping drops the CMap.
        enc[0x44] = Some(b"nosuchglyph".to_vec());
        assert!(
            d.pdf_create_ToUnicode_CMap(b"Test", &enc, Some(&used))
                .unwrap()
                .is_none()
        );
    }
}
