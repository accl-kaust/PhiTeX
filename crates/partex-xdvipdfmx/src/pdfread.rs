//! Reading PDF files (pdfobj.c's `pdf_file`): the cross-reference
//! sections and streams, objects read on demand, and their import into
//! the output (`pdf_import_object`).

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fmt::{atoi, strtol};
use crate::obj::{DecodeParms, Obj, PDF_INDIRECT, PDF_OBJ_MAX_DEPTH, PdfOut};
use crate::parse::{parse_number, parse_unsigned, skip_white};

#[derive(Clone, Copy, Debug, Default)]
pub struct XrefIn {
    pub kind: u8,
    pub field2: u32,
    pub field3: u16,
    pub direct: Option<Obj>,
    pub indirect: Option<Obj>,
}

/// A PDF file read for its objects.
pub struct PdfFile {
    pub ident: Vec<u8>,
    pub data: Arc<[u8]>,
    pub trailer: Option<Obj>,
    pub xref: Vec<XrefIn>,
    pub catalog: Option<Obj>,
    pub version: i32,
}

impl PdfFile {
    fn checklabel(&self, n: u32, g: u16) -> bool {
        let n = n as usize;
        n > 0
            && n < self.xref.len()
            && ((self.xref[n].kind == 1 && self.xref[n].field3 == g)
                || (self.xref[n].kind == 2 && g == 0))
    }
}

/// `mfreadln`: a line without its end, from `pos`; the position after
/// its end. None at the end of the data.
fn readln(d: &[u8], pos: &mut usize, max: usize) -> Option<Vec<u8>> {
    if *pos >= d.len() {
        return None;
    }
    let mut line = Vec::new();
    while *pos < d.len() && d[*pos] != b'\n' && d[*pos] != b'\r' {
        if line.len() < max {
            line.push(d[*pos]);
        }
        *pos += 1;
    }
    if *pos < d.len() && d[*pos] == b'\r' {
        *pos += 1;
        if *pos < d.len() && d[*pos] == b'\n' {
            *pos += 1;
        }
    } else if *pos < d.len() && d[*pos] == b'\n' {
        *pos += 1;
    }
    Some(line)
}

/// `backup_line`: back to the start of the line before `pos`.
fn backup_line(d: &[u8], pos: &mut isize) -> bool {
    let mut ch: i32 = -1;
    if *pos > 1 {
        loop {
            *pos -= 2;
            if *pos < 0 {
                *pos = 0;
            }
            if *pos <= 0 {
                break;
            }
            ch = i32::from(d[*pos as usize]);
            *pos += 1;
            if ch == i32::from(b'\n') || ch == i32::from(b'\r') {
                break;
            }
        }
    }
    ch >= 0
}

/// `find_xref`: the offset `startxref` names.
fn find_xref(d: &[u8]) -> usize {
    let mut pos = d.len() as isize;
    let mut tries = 10;
    loop {
        if !backup_line(d, &mut pos) {
            tries = 0;
            break;
        }
        let matched = d[pos as usize..].starts_with(b"startxref");
        tries -= 1;
        if !(tries > 0 && !matched) {
            break;
        }
    }
    if tries <= 0 {
        return 0;
    }
    let mut p = pos as usize;
    // (the rest of this line, then the next)
    let _ = readln(d, &mut p, 1024);
    let Some(line) = readln(d, &mut p, 1024) else {
        return 0;
    };
    let mut q = 0;
    skip_white(&line, &mut q);
    let n = parse_number(&line, &mut q).unwrap_or_default();
    crate::fmt::atof(&n) as i32 as usize
}

impl PdfOut {
    /// `pdf_open`: reads the file's cross-reference and catalog. The
    /// same `ident` again gives the same file.
    pub fn pdf_open(&mut self, ident: Option<&[u8]>, data: Arc<[u8]>) -> Option<u32> {
        if let Some(id) = ident
            && let Some(i) = self.files.iter().position(|f| f.ident == id)
        {
            return Some(i as u32);
        }
        let version = check_for_pdf_version(&data);
        let pf = self.files.len() as u32;
        self.files.push(PdfFile {
            ident: ident.map(<[u8]>::to_vec).unwrap_or_default(),
            data,
            trailer: None,
            xref: Vec::new(),
            catalog: None,
            version,
        });
        let ok = (|| {
            let trailer = self.read_xref(pf)?;
            self.files[pf as usize].trailer = Some(trailer);
            if self.lookup_dict(trailer, b"Encrypt").is_some() {
                return None;
            }
            let root = self.lookup_dict(trailer, b"Root");
            let catalog = self.deref_obj(root);
            self.files[pf as usize].catalog = catalog;
            if !self.is_dict(catalog) {
                return None;
            }
            let v = self.lookup_dict(catalog?, b"Version");
            let nv = self.deref_obj(v);
            if let Some(nv) = nv {
                if !self.is_name(Some(nv)) {
                    self.release(nv);
                    return None;
                }
                let s = self.name_value(nv).to_vec();
                let (major, n) = strtol(&s, 10);
                if n == 0 || s.get(n) != Some(&b'.') {
                    self.release(nv);
                    return None;
                }
                let (minor, m) = strtol(&s[n + 1..], 10);
                if m == 0 {
                    self.release(nv);
                    return None;
                }
                let v = (major * 10 + minor) as i32;
                if self.files[pf as usize].version < v {
                    self.files[pf as usize].version = v;
                }
                self.release(nv);
            }
            Some(())
        })();
        if ok.is_none() {
            self.pdf_file_free(pf);
            if ident.is_none() || self.files.len() as u32 == pf + 1 {
                self.files.pop();
            }
            return None;
        }
        Some(pf)
    }

    fn pdf_file_free(&mut self, pf: u32) {
        let f = &mut self.files[pf as usize];
        let xref = core::mem::take(&mut f.xref);
        let trailer = f.trailer.take();
        let catalog = f.catalog.take();
        for e in xref {
            self.release_opt(e.direct);
            self.release_opt(e.indirect);
        }
        self.release_opt(trailer);
        self.release_opt(catalog);
    }

    #[must_use]
    pub fn pdf_file_version(&self, pf: u32) -> i32 {
        self.files[pf as usize].version
    }
    pub fn pdf_file_trailer(&mut self, pf: u32) -> Option<Obj> {
        let t = self.files[pf as usize].trailer;
        self.link_opt(t)
    }
    #[must_use]
    pub fn pdf_file_catalog(&self, pf: u32) -> Option<Obj> {
        self.files[pf as usize].catalog
    }

    fn extend_xref(&mut self, pf: u32, n: usize) {
        let f = &mut self.files[pf as usize];
        if f.xref.len() < n {
            f.xref.resize(n, XrefIn::default());
        }
    }

    fn read_xref(&mut self, pf: u32) -> Option<Obj> {
        let data = self.files[pf as usize].data.clone();
        let mut xref_pos = find_xref(&data);
        if xref_pos == 0 {
            return None;
        }
        let mut main_trailer: Option<Obj> = None;
        while xref_pos != 0 {
            let res = self.parse_xref_table(pf, &data, xref_pos);
            let trailer;
            if res > 0 {
                let Some(t) = self.parse_trailer(pf, &data) else {
                    self.release_opt(main_trailer);
                    return None;
                };
                trailer = t;
                if main_trailer.is_none() {
                    main_trailer = Some(self.link(t));
                }
                if let Some(xs) = self.lookup_dict(t, b"XRefStm")
                    && self.is_number(Some(xs))
                {
                    let pos = self.number_value(xs) as i32 as usize;
                    if let Some(nt) = self.parse_xref_stream(pf, &data, pos) {
                        self.release(nt);
                    }
                }
            } else if res == 0
                && let Some(t) = self.parse_xref_stream(pf, &data, xref_pos)
            {
                trailer = t;
                if main_trailer.is_none() {
                    main_trailer = Some(self.link(t));
                }
            } else {
                self.release_opt(main_trailer);
                return None;
            }
            if let Some(prev) = self.lookup_dict(trailer, b"Prev") {
                if self.is_number(Some(prev)) {
                    xref_pos = self.number_value(prev) as usize;
                } else {
                    self.release(trailer);
                    self.release_opt(main_trailer);
                    return None;
                }
            } else {
                xref_pos = 0;
            }
            self.release(trailer);
        }
        main_trailer
    }

    /// The position after the last line read, kept for `parse_trailer`.
    fn parse_xref_table(&mut self, pf: u32, d: &[u8], xref_pos: usize) -> i32 {
        let mut pos = xref_pos;
        let Some(line) = readln(d, &mut pos, 255) else {
            return -1;
        };
        if !line.starts_with(b"xref") {
            return 0;
        }
        let mut p = 4;
        skip_white(&line, &mut p);
        if p != line.len() {
            return -1;
        }
        let file_size = d.len();
        loop {
            let current_pos = pos;
            let Some(line) = readln(d, &mut pos, 255) else {
                return -1;
            };
            if line.is_empty() {
                continue;
            }
            let mut p = 0;
            skip_white(&line, &mut p);
            if p == line.len() {
                continue;
            } else if line[p..].starts_with(b"trailer") {
                self.trailer_pos = current_pos + p;
                break;
            }
            let Some(q) = parse_unsigned(&line, &mut p) else {
                return -1;
            };
            let first = atoi(&q) as usize;
            skip_white(&line, &mut p);
            let Some(q) = parse_unsigned(&line, &mut p) else {
                return -1;
            };
            let size = atoi(&q) as usize;
            skip_white(&line, &mut p);
            if p != line.len() {
                return -1;
            }
            if self.files[pf as usize].xref.len() < first + size {
                self.extend_xref(pf, first + size);
            }
            let mut i = first;
            while i < first + size {
                let Some(line) = readln(d, &mut pos, 255) else {
                    return -1;
                };
                if line.is_empty() {
                    continue;
                }
                let mut p = 0;
                skip_white(&line, &mut p);
                if p == line.len() {
                    continue;
                }
                let Some(q) = parse_unsigned(&line, &mut p) else {
                    return -1;
                };
                if q.len() != 10 {
                    return -1;
                }
                let offset = atoi(&q) as i32 as usize;
                skip_white(&line, &mut p);
                let Some(q) = parse_unsigned(&line, &mut p) else {
                    return -1;
                };
                if q.len() != 5 {
                    return -1;
                }
                let obj_gen = atoi(&q) as u32;
                skip_white(&line, &mut p);
                if p == line.len() {
                    return -1;
                }
                let flag = line[p];
                p += 1;
                skip_white(&line, &mut p);
                if p < line.len() {
                    return -1;
                } else if (flag != b'n' && flag != b'f')
                    || (flag == b'n' && (offset >= file_size || (offset > 0 && offset < 4)))
                {
                    return -1;
                }
                let e = &mut self.files[pf as usize].xref[i];
                if e.field2 == 0 {
                    e.kind = u8::from(flag == b'n');
                    e.field2 = offset as u32;
                    e.field3 = obj_gen as u16;
                }
                i += 1;
            }
        }
        1
    }

    fn parse_trailer(&mut self, _pf: u32, d: &[u8]) -> Option<Obj> {
        let start = self.trailer_pos;
        let end = (start + 1024).min(d.len());
        let buf = &d[start..end];
        if buf.is_empty() || !buf.starts_with(b"trailer") {
            return None;
        }
        let mut p = 7;
        skip_white(buf, &mut p);
        self.parse_pdf_dict(buf, &mut p, Some(_pf))
    }

    fn parse_xref_stream(&mut self, pf: u32, d: &[u8], xref_pos: usize) -> Option<Obj> {
        let file_size = d.len();
        let xrefstm = self.pdf_read_object(0, 0, pf, xref_pos, file_size);
        if !self.is_stream(xrefstm) {
            self.release_opt(xrefstm);
            return None;
        }
        let xrefstm = xrefstm?;
        let Some(tmp) = self.stream_uncompress(xrefstm) else {
            self.release(xrefstm);
            return None;
        };
        self.release(xrefstm);
        let xrefstm = tmp;
        let sd = self.stream_dict(xrefstm);
        let trailer = self.link(sd);
        let fail = |o: &mut PdfOut| {
            o.release(xrefstm);
            o.release(trailer);
            None
        };
        let Some(size_obj) = self
            .lookup_dict(trailer, b"Size")
            .filter(|&s| self.is_number(Some(s)))
        else {
            return fail(self);
        };
        let size = self.number_value(size_obj) as u32 as i32;
        let mut length = self.stream_length(xrefstm) as i64;
        let Some(w_obj) = self.lookup_dict(trailer, b"W") else {
            return fail(self);
        };
        if !self.is_array(Some(w_obj)) || self.array_length(w_obj) != 3 {
            return fail(self);
        }
        let mut w = [0i32; 3];
        let mut wsum = 0;
        for (i, wi) in w.iter_mut().enumerate() {
            let Some(t) = self
                .get_array(w_obj, i as i32)
                .filter(|&t| self.is_number(Some(t)))
            else {
                return fail(self);
            };
            *wi = self.number_value(t) as i32;
            wsum += *wi;
        }
        let data = self.stream_data(xrefstm).to_vec();
        let mut p = 0usize;
        if let Some(index) = self.lookup_dict(trailer, b"Index") {
            if !self.is_array(Some(index)) || self.array_length(index) % 2 != 0 {
                return fail(self);
            }
            let n = self.array_length(index);
            let mut i = 0;
            while i < n {
                let first = self.get_array(index, i as i32);
                let sz = self.get_array(index, i as i32 + 1);
                i += 2;
                if !self.is_number(first) || !self.is_number(sz) {
                    return fail(self);
                }
                let first = self.number_value(first?) as i32;
                let sz = self.number_value(sz?) as i32;
                if self.parse_xrefstm_subsec(pf, &data, &mut p, &mut length, &w, wsum, first, sz) {
                    return fail(self);
                }
            }
        } else if self.parse_xrefstm_subsec(pf, &data, &mut p, &mut length, &w, wsum, 0, size) {
            return fail(self);
        }
        self.release(xrefstm);
        Some(trailer)
    }

    /// True on error.
    #[allow(clippy::too_many_arguments)]
    fn parse_xrefstm_subsec(
        &mut self,
        pf: u32,
        d: &[u8],
        p: &mut usize,
        length: &mut i64,
        w: &[i32; 3],
        wsum: i32,
        first: i32,
        size: i32,
    ) -> bool {
        *length -= i64::from(wsum) * i64::from(size);
        if *length < 0 {
            return true;
        }
        let (first, size) = (first.max(0) as usize, size.max(0) as usize);
        if self.files[pf as usize].xref.len() < first + size {
            self.extend_xref(pf, first + size);
        }
        let field = |p: &mut usize, len: i32, def: u32| -> u32 {
            if len == 0 {
                return def;
            }
            let mut v: u32 = 0;
            for _ in 0..len {
                v = (v << 8) | u32::from(d.get(*p).copied().unwrap_or(0));
                *p += 1;
            }
            v
        };
        for k in 0..size {
            let kind = field(p, w[0], 1) as u8;
            let f2 = field(p, w[1], 0);
            let f3 = field(p, w[2], 0) as u16;
            let e = &mut self.files[pf as usize].xref[first + k];
            if e.field2 == 0 {
                e.kind = kind;
                e.field2 = f2;
                e.field3 = f3;
            }
        }
        false
    }

    fn next_object_offset(&self, pf: u32, obj_num: u32) -> usize {
        let f = &self.files[pf as usize];
        let mut next = f.data.len() as u32;
        let curr = f.xref[obj_num as usize].field2;
        for e in &f.xref {
            if e.kind == 1 && e.field2 > curr && e.field2 < next {
                next = e.field2;
            }
        }
        next as usize
    }

    fn pdf_read_object(
        &mut self,
        obj_num: u32,
        obj_gen: u16,
        pf: u32,
        offset: usize,
        limit: usize,
    ) -> Option<Obj> {
        if limit <= offset {
            return None;
        }
        let data = self.files[pf as usize].data.clone();
        let buf = &data[offset..limit.min(data.len())];
        let mut q = 0;
        skip_white(buf, &mut q);
        let sp = parse_unsigned(buf, &mut q)?;
        let n = atoi(&sp) as u32;
        skip_white(buf, &mut q);
        let sp = parse_unsigned(buf, &mut q)?;
        let g = atoi(&sp) as u32;
        if obj_num != 0 && (n != obj_num || g != u32::from(obj_gen)) {
            return None;
        }
        let mut p = q;
        skip_white(buf, &mut p);
        if !buf[p..].starts_with(b"obj") {
            return None;
        }
        p += 3;
        let result = self.parse_pdf_object(buf, &mut p, Some(pf));
        skip_white(buf, &mut p);
        if !buf[p.min(buf.len())..].starts_with(b"endobj") {
            self.release_opt(result);
            return None;
        }
        result
    }

    fn read_objstm(&mut self, pf: u32, num: u32) -> Option<Obj> {
        let e = self.files[pf as usize].xref[num as usize];
        let offset = e.field2 as usize;
        let limit = self.next_object_offset(pf, num);
        let objstm = self.pdf_read_object(num, e.field3, pf, offset, limit);
        let fail = |o: &mut PdfOut, x: Option<Obj>| {
            o.release_opt(x);
            None
        };
        if !self.is_stream(objstm) {
            return fail(self, objstm);
        }
        let objstm = objstm?;
        let Some(tmp) = self.stream_uncompress(objstm) else {
            return fail(self, Some(objstm));
        };
        self.release(objstm);
        let objstm = tmp;
        let dict = self.stream_dict(objstm);
        let ty = self.lookup_dict(dict, b"Type");
        if !self.is_name(ty) || self.name_value(ty?) != b"ObjStm" {
            return fail(self, Some(objstm));
        }
        let Some(n) = self
            .lookup_dict(dict, b"N")
            .filter(|&x| self.is_number(Some(x)))
        else {
            return fail(self, Some(objstm));
        };
        let n = self.number_value(n) as i32;
        let Some(first) = self
            .lookup_dict(dict, b"First")
            .filter(|&x| self.is_number(Some(x)))
        else {
            return fail(self, Some(objstm));
        };
        let first = self.number_value(first) as i32;
        if first as usize >= self.stream_length(objstm) {
            return fail(self, Some(objstm));
        }
        let mut header = Vec::with_capacity(2 * (n as usize + 1));
        header.push(n);
        header.push(first);
        let data = self.stream_data(objstm)[..first as usize].to_vec();
        let mut p = 0;
        for _ in 0..2 * n {
            let (v, k) = crate::fmt::strtol(&data[p..], 10);
            if k == 0 {
                return fail(self, Some(objstm));
            }
            header.push(v as i32);
            p += k;
        }
        skip_white(&data, &mut p);
        if p != data.len() {
            return fail(self, Some(objstm));
        }
        self.stream_mut(objstm).objstm = Some(header);
        self.files[pf as usize].xref[num as usize].direct = Some(objstm);
        Some(objstm)
    }

    /// `pdf_get_object`: a link to the object (a new null if it does
    /// not exist).
    pub fn pdf_get_object(&mut self, pf: u32, obj_num: u32, obj_gen: u16) -> Option<Obj> {
        if !self.files[pf as usize].checklabel(obj_num, obj_gen) {
            return Some(self.new_null());
        }
        if let Some(r) = self.files[pf as usize].xref[obj_num as usize].direct {
            return Some(self.link(r));
        }
        let e = self.files[pf as usize].xref[obj_num as usize];
        let result = if e.kind == 1 {
            let limit = self.next_object_offset(pf, obj_num);
            self.pdf_read_object(obj_num, obj_gen, pf, e.field2 as usize, limit)
        } else {
            let objstm_num = e.field2;
            let index = i32::from(e.field3);
            let f = &self.files[pf as usize];
            if objstm_num as usize >= f.xref.len() || f.xref[objstm_num as usize].kind != 1 {
                return Some(self.new_null());
            }
            let objstm = match f.xref[objstm_num as usize].direct {
                Some(o) => Some(o),
                None => self.read_objstm(pf, objstm_num),
            };
            let Some(objstm) = objstm else {
                return Some(self.new_null());
            };
            let header = self.stream_mut(objstm).objstm.clone().unwrap_or_default();
            let (n, first) = (header[0], header[1]);
            let data = &header[2..];
            if index >= n || data[2 * index as usize] != obj_num as i32 {
                return Some(self.new_null());
            }
            let sdata = self.stream_data(objstm).to_vec();
            let length = sdata.len();
            let p = (first + data[2 * index as usize + 1]) as usize;
            let q = if index == n - 1 {
                length
            } else {
                (first + data[2 * index as usize + 3]) as usize
            };
            let slice = &sdata[..q.min(length)];
            let mut pp = p;
            self.parse_pdf_object(slice, &mut pp, Some(pf))
        };
        let Some(result) = result else {
            return Some(self.new_null());
        };
        let l = self.link(result);
        self.files[pf as usize].xref[obj_num as usize].direct = Some(l);
        Some(result)
    }

    /// `pdf_deref_obj`: a link to the object an indirect reference
    /// leads to; none for a null or a freed object.
    pub fn deref_obj(&mut self, obj: Option<Obj>) -> Option<Obj> {
        let mut count = PDF_OBJ_MAX_DEPTH;
        let mut obj = obj.map(|o| self.link(o));
        while self.type_of(obj) == PDF_INDIRECT {
            count -= 1;
            if count == 0 {
                break;
            }
            let o = obj.expect("indirect");
            let ind = self.indirect(o);
            if let Some(pf) = ind.pf {
                self.release(o);
                obj = self.pdf_get_object(pf, ind.label, ind.generation);
            } else if self.is_freed(ind.label) {
                self.release(o);
                return None;
            } else {
                let next = ind.obj.expect("Undefined object reference");
                self.release(o);
                obj = Some(self.link(next));
            }
        }
        assert!(
            count != 0,
            "Loop in object hierarchy detected. Broken PDF file?"
        );
        if self.is_null(obj) {
            self.release_opt(obj);
            return None;
        }
        obj
    }

    /// `pdf_import_object`.
    pub fn import_object(&mut self, object: Obj) -> Option<Obj> {
        match self.type_of(Some(object)) {
            PDF_INDIRECT => {
                if self.indirect(object).pf.is_some() {
                    self.import_indirect(object)
                } else {
                    Some(self.link(object))
                }
            }
            crate::obj::PDF_STREAM => {
                let sd = self.stream_dict(object);
                let tmp = self.import_object(sd)?;
                let imported = self.new_stream(0);
                let isd = self.stream_dict(imported);
                self.merge_dict(isd, tmp);
                self.release(tmp);
                let data = self.stream_data(object).to_vec();
                self.add_stream(imported, &data);
                Some(imported)
            }
            crate::obj::PDF_DICT => {
                let imported = self.new_dict();
                for (k, v) in self.dict_entries(object) {
                    let tmp = v.and_then(|v| self.import_object(v));
                    let Some(tmp) = tmp else {
                        self.release(imported);
                        return None;
                    };
                    let k = self.link(k);
                    self.add_dict(imported, k, Some(tmp));
                }
                Some(imported)
            }
            crate::obj::PDF_ARRAY => {
                let imported = self.new_array();
                for v in self.array_items(object) {
                    let tmp = v.and_then(|v| self.import_object(v));
                    let Some(tmp) = tmp else {
                        self.release(imported);
                        return None;
                    };
                    self.add_array(imported, tmp);
                }
                Some(imported)
            }
            _ => Some(self.link(object)),
        }
    }

    fn import_indirect(&mut self, object: Obj) -> Option<Obj> {
        let ind = self.indirect(object);
        let pf = ind.pf.expect("file");
        if !self.files[pf as usize].checklabel(ind.label, ind.generation) {
            return Some(self.new_null());
        }
        let r = self.files[pf as usize].xref[ind.label as usize].indirect;
        let r = if let Some(r) = r {
            r
        } else {
            let obj = self.pdf_get_object(pf, ind.label, ind.generation)?;
            let reserved = self.new_null();
            let r = self.ref_obj(reserved);
            self.files[pf as usize].xref[ind.label as usize].indirect = Some(r);
            if let Some(imported) = self.import_object(obj) {
                self.set_indirect_target(r, Some(imported));
                let (l, g) = (self.label(reserved), self.generation(reserved));
                self.set_label(imported, l, g);
                self.set_label(reserved, 0, 0);
                self.release(imported);
            }
            self.release(reserved);
            self.release(obj);
            r
        };
        Some(self.link(r))
    }

    /// `pdf_concat_stream`: appends `src`'s data, decoded, to `dst`.
    pub fn concat_stream(&mut self, dst: Obj, src: Obj) -> i32 {
        if !self.is_stream(Some(dst)) || !self.is_stream(Some(src)) {
            return -1;
        }
        let sd = self.stream_dict(src);
        let Some(filter) = self.lookup_dict(sd, b"Filter") else {
            let d = self.stream_data(src).to_vec();
            self.add_stream(dst, &d);
            return 0;
        };
        let parms = if let Some(dp) = self.lookup_dict(sd, b"DecodeParms") {
            let p = self.deref_obj(Some(dp));
            if p.is_none() || (!self.is_array(p) && !self.is_dict(p)) {
                self.release_opt(p);
                return -1;
            }
            p
        } else {
            None
        };
        let mut data = Some(self.stream_data(src).to_vec());
        if self.is_array(Some(filter)) {
            let num = self.array_length(filter);
            if let Some(pa) = parms
                && (!self.is_array(Some(pa)) || self.array_length(pa) != num)
            {
                self.release(pa);
                return -1;
            }
            for i in 0..num {
                let Some(cur) = data.take() else { break };
                let f = self.get_array(filter, i as i32);
                let tmp1 = self.deref_obj(f);
                let tmp2 = match parms {
                    Some(pa) => {
                        let x = self.get_array(pa, i as i32);
                        self.deref_obj(x)
                    }
                    None => None,
                };
                if self.is_name(tmp1) {
                    let name = self.name_value(tmp1.expect("name")).to_vec();
                    data = self.decode_with(&name, &cur, tmp2);
                } else if self.is_null(tmp1) {
                    data = Some(cur);
                } else {
                    data = None;
                }
                self.release_opt(tmp1);
                self.release_opt(tmp2);
            }
        } else if self.is_name(Some(filter)) {
            let name = self.name_value(filter).to_vec();
            let cur = data.take().unwrap_or_default();
            data = self.decode_with(&name, &cur, parms);
        } else {
            data = None;
        }
        self.release_opt(parms);
        match data {
            Some(d) => {
                self.add_stream(dst, &d);
                0
            }
            None => -1,
        }
    }

    fn decode_with(&mut self, name: &[u8], data: &[u8], parm: Option<Obj>) -> Option<Vec<u8>> {
        match name {
            b"ASCIIHexDecode" => crate::filter::decode_ascii_hex(data),
            b"ASCII85Decode" => crate::filter::decode_ascii85(data),
            b"FlateDecode" => {
                let p = parm.map(|p| self.decode_parms_flate(p));
                crate::filter::decode_flate(data, p.as_ref())
            }
            _ => None,
        }
    }

    fn decode_parms_flate(&mut self, dict: Obj) -> DecodeParms {
        let mut parms = DecodeParms {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        };
        for (key, slot) in [
            (&b"Predictor"[..], 0),
            (&b"Colors"[..], 1),
            (&b"BitsPerComponent"[..], 2),
            (&b"Columns"[..], 3),
        ] {
            let x = self.lookup_dict(dict, key);
            let t = self.deref_obj(x);
            if let Some(t) = t {
                let v = self.number_value(t) as i32;
                match slot {
                    0 => parms.predictor = v,
                    1 => parms.colors = v,
                    2 => parms.bits_per_component = v,
                    _ => parms.columns = v,
                }
                self.release(t);
            }
        }
        parms
    }

    /// `pdf_stream_uncompress`.
    pub fn stream_uncompress(&mut self, src: Obj) -> Option<Obj> {
        let dst = self.new_stream(0);
        let dd = self.stream_dict(dst);
        let sd = self.stream_dict(src);
        self.merge_dict(dd, sd);
        self.remove_dict(dd, b"Length");
        self.concat_stream(dst, src);
        Some(dst)
    }
}

/// `check_for_pdf_version`: `%PDF-M.N` as `M*10+N`, -1 if none.
#[must_use]
pub fn check_for_pdf_version(d: &[u8]) -> i32 {
    if !d.starts_with(b"%PDF-") {
        return -1;
    }
    let (major, n) = strtol(&d[5..], 10);
    if n == 0 || d.get(5 + n) != Some(&b'.') {
        return -1;
    }
    let (minor, m) = strtol(&d[6 + n..], 10);
    if m == 0 {
        return -1;
    }
    (major * 10 + minor) as i32
}
