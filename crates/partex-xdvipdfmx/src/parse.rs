//! The PDF object parser (pdfparse.c), over a byte slice and a position
//! (C's `const char **pp, const char *endptr`: the slice ends at
//! `endptr`).

use alloc::vec::Vec;

use crate::obj::{Obj, PdfOut};

/// The handler of a token no PDF object begins with (spc_pdfm.c's
/// `@name` references and the like).
pub type Unknown<'a> = &'a mut dyn FnMut(&mut PdfOut, &[u8], &mut usize) -> Option<Obj>;

#[must_use]
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0)
}

#[must_use]
pub fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'%')
}

#[must_use]
pub fn istokensep(c: u8) -> bool {
    is_space(c) || is_delim(c)
}

/// `PDF_TOKEN_END`.
#[must_use]
pub fn token_end(s: &[u8], p: usize) -> bool {
    p >= s.len() || istokensep(s[p])
}

/// `pdfparse_skip_line`.
pub fn skip_line(s: &[u8], p: &mut usize) {
    while *p < s.len() && s[*p] != b'\n' && s[*p] != b'\r' {
        *p += 1;
    }
    if *p < s.len() && s[*p] == b'\r' {
        *p += 1;
    }
    if *p < s.len() && s[*p] == b'\n' {
        *p += 1;
    }
}

/// `skip_white`: white space and `%` comments.
pub fn skip_white(s: &[u8], p: &mut usize) {
    while *p < s.len() && (is_space(s[*p]) || s[*p] == b'%') {
        if s[*p] == b'%' {
            skip_line(s, p);
        } else {
            *p += 1;
        }
    }
}

/// `parse_number`: the text of a number (sign, digits, a dot, digits),
/// none if empty.
pub fn parse_number(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    skip_white(s, p);
    let mut q = *p;
    if q < s.len() && (s[q] == b'+' || s[q] == b'-') {
        q += 1;
    }
    while q < s.len() && s[q].is_ascii_digit() {
        q += 1;
    }
    if q < s.len() && s[q] == b'.' {
        q += 1;
        while q < s.len() && s[q].is_ascii_digit() {
            q += 1;
        }
    }
    let r = parsed_string(s, *p, q);
    *p = q;
    r
}

/// `parse_unsigned`.
pub fn parse_unsigned(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    skip_white(s, p);
    let mut q = *p;
    while q < s.len() && s[q].is_ascii_digit() {
        q += 1;
    }
    let r = parsed_string(s, *p, q);
    *p = q;
    r
}

fn parsed_string(s: &[u8], a: usize, b: usize) -> Option<Vec<u8>> {
    if b > a { Some(s[a..b].to_vec()) } else { None }
}

fn parse_gen_ident(s: &[u8], p: &mut usize, valid: &[u8]) -> Option<Vec<u8>> {
    let mut q = *p;
    // (`strchr(valid_chars, *p)` also finds the terminating NUL)
    while q < s.len() && (valid.contains(&s[q]) || s[q] == 0) {
        q += 1;
    }
    let r = parsed_string(s, *p, q);
    *p = q;
    r
}

/// `parse_ident`.
pub fn parse_ident(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    parse_gen_ident(
        s,
        p,
        b"!\"#$&'*+,-.0123456789:;=?@ABCDEFGHIJKLMNOPQRSTUVWXYZ\\^_`abcdefghijklmnopqrstuvwxyz|~",
    )
}

/// `parse_val_ident`.
pub fn parse_val_ident(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    parse_gen_ident(
        s,
        p,
        b"!\"#$&'*+,-./0123456789:;?@ABCDEFGHIJKLMNOPQRSTUVWXYZ\\^_`abcdefghijklmnopqrstuvwxyz|~",
    )
}

/// `parse_opt_ident`.
pub fn parse_opt_ident(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    if *p < s.len() && s[*p] == b'@' {
        *p += 1;
        return parse_ident(s, p);
    }
    None
}

/// `xtoi`.
#[must_use]
pub fn xtoi(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => i32::from(c - b'0'),
        b'A'..=b'F' => i32::from(c - b'A') + 10,
        b'a'..=b'f' => i32::from(c - b'a') + 10,
        _ => -1,
    }
}

fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `ps_getescc`: after a backslash at `*p`.
fn ps_getescc(s: &[u8], p: &mut usize) -> i32 {
    let mut q = *p + 1;
    let c = at(s, q);
    let ch = match c {
        b'n' => {
            q += 1;
            i32::from(b'\n')
        }
        b'r' => {
            q += 1;
            i32::from(b'\r')
        }
        b't' => {
            q += 1;
            i32::from(b'\t')
        }
        b'b' => {
            q += 1;
            8
        }
        b'f' => {
            q += 1;
            12
        }
        b'\n' => {
            q += 1;
            -1
        }
        b'\r' => {
            q += 1;
            if q < s.len() && s[q] == b'\n' {
                q += 1;
            }
            -1
        }
        b'\\' | b'(' | b')' => {
            q += 1;
            i32::from(c)
        }
        b'0'..=b'7' => {
            let mut ch = 0i32;
            let mut i = 0;
            while i < 3 && q < s.len() && (b'0'..=b'7').contains(&s[q]) {
                ch = (ch << 3) + i32::from(s[q] - b'0');
                q += 1;
                i += 1;
            }
            ch & 0xff
        }
        _ => {
            q += 1;
            i32::from(c)
        }
    };
    *p = q;
    ch
}

const PDF_NAME_LEN_MAX: usize = 128;
const PDF_STRING_LEN_MAX: usize = 65535;

impl PdfOut {
    /// `parse_pdf_number`.
    pub fn parse_pdf_number(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p >= s.len() || (!s[p].is_ascii_digit() && s[p] != b'.' && s[p] != b'+' && s[p] != b'-')
        {
            return None;
        }
        let mut sign = 1.0;
        if s[p] == b'-' {
            if p + 1 >= s.len() {
                return None;
            }
            sign = -1.0;
            p += 1;
        } else if s[p] == b'+' {
            if p + 1 >= s.len() {
                return None;
            }
            p += 1;
        }
        let mut v = 0.0f64;
        let mut nddigits = 0;
        let mut has_dot = false;
        while p < s.len() && !istokensep(s[p]) {
            let c = s[p];
            if c == b'.' {
                if has_dot {
                    return None;
                }
                has_dot = true;
            } else if c.is_ascii_digit() {
                if has_dot {
                    v += f64::from(c - b'0') / crate::fmt::pow10(nddigits + 1);
                    nddigits += 1;
                } else {
                    v = v * 10.0 + f64::from(c - b'0');
                }
            } else {
                return None;
            }
            p += 1;
        }
        *pp = p;
        Some(self.new_number(sign * v))
    }

    /// `parse_pdf_name`.
    pub fn parse_pdf_name(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        skip_white(s, pp);
        if *pp >= s.len() || s[*pp] != b'/' {
            return None;
        }
        *pp += 1;
        let mut name = Vec::new();
        let mut len = 0usize;
        while *pp < s.len() && !istokensep(s[*pp]) {
            let ch: i32 = if s[*pp] == b'#' {
                if *pp + 2 >= s.len() {
                    *pp = s.len();
                    -1
                } else if !s[*pp + 1].is_ascii_hexdigit() || !s[*pp + 2].is_ascii_hexdigit() {
                    *pp += 3;
                    -1
                } else {
                    let c = (xtoi(s[*pp + 1]) << 4) + xtoi(s[*pp + 2]);
                    *pp += 3;
                    c
                }
            } else {
                let c = i32::from(s[*pp]);
                *pp += 1;
                c
            };
            if !(0..=0xff).contains(&ch) || ch == 0 {
                // (ignored)
            } else if len < PDF_NAME_LEN_MAX {
                name.push(ch as u8);
                len += 1;
            } else {
                len += 1;
            }
        }
        if len < 1 {
            return None;
        }
        Some(self.new_name(&name))
    }

    /// `parse_pdf_boolean`.
    pub fn parse_pdf_boolean(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        skip_white(s, pp);
        let p = *pp;
        if p + 4 <= s.len() && &s[p..p + 4] == b"true" {
            if p + 4 == s.len() || istokensep(s[p + 4]) {
                *pp += 4;
                return Some(self.new_boolean(true));
            }
        } else if p + 5 <= s.len()
            && &s[p..p + 5] == b"false"
            && (p + 5 == s.len() || istokensep(s[p + 5]))
        {
            *pp += 5;
            return Some(self.new_boolean(false));
        }
        None
    }

    /// `parse_pdf_null`.
    pub fn parse_pdf_null(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        skip_white(s, pp);
        let p = *pp;
        if p + 4 > s.len() || (p + 4 < s.len() && !istokensep(s[p + 4])) {
            return None;
        }
        if &s[p..p + 4] == b"null" {
            *pp += 4;
            return Some(self.new_null());
        }
        None
    }

    fn parse_pdf_literal_string(&mut self, s: &[u8], pp: &mut usize, tainted: bool) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p >= s.len() || s[p] != b'(' {
            return None;
        }
        p += 1;
        let mut buf = Vec::new();
        let mut op_count = 0i32;
        while p < s.len() {
            let ch = s[p];
            if ch == b')' && op_count < 1 {
                break;
            }
            if tainted && p + 1 < s.len() && ch & 0x80 != 0 {
                if buf.len() + 2 >= PDF_STRING_LEN_MAX {
                    return None;
                }
                buf.push(s[p]);
                buf.push(s[p + 1]);
                p += 2;
                continue;
            }
            if buf.len() + 1 >= PDF_STRING_LEN_MAX {
                return None;
            }
            match ch {
                b'\\' => {
                    let c = ps_getescc(s, &mut p);
                    if c >= 0 {
                        buf.push((c & 0xff) as u8);
                    }
                }
                b'\r' => {
                    p += 1;
                    if p < s.len() && s[p] == b'\n' {
                        p += 1;
                    }
                    buf.push(b'\n');
                }
                _ => {
                    if ch == b'(' {
                        op_count += 1;
                    } else if ch == b')' {
                        op_count -= 1;
                    }
                    buf.push(ch);
                    p += 1;
                }
            }
        }
        if op_count > 0 || p >= s.len() || s[p] != b')' {
            return None;
        }
        *pp = p + 1;
        Some(self.new_string(&buf))
    }

    fn parse_pdf_hex_string(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p >= s.len() || s[p] != b'<' {
            return None;
        }
        p += 1;
        let mut buf = Vec::new();
        while p < s.len() && s[p] != b'>' && buf.len() < PDF_STRING_LEN_MAX {
            skip_white(s, &mut p);
            if p >= s.len() || s[p] == b'>' {
                break;
            }
            let mut ch = xtoi(s[p]) << 4;
            p += 1;
            skip_white(s, &mut p);
            if p < s.len() && s[p] != b'>' {
                ch += xtoi(s[p]);
                p += 1;
            }
            buf.push((ch & 0xff) as u8);
        }
        if p >= s.len() || s[p] != b'>' {
            return None;
        }
        *pp = p + 1;
        Some(self.new_string(&buf))
    }

    /// `parse_pdf_string`.
    pub fn parse_pdf_string(&mut self, s: &[u8], pp: &mut usize) -> Option<Obj> {
        self.parse_pdf_string_t(s, pp, false)
    }

    fn parse_pdf_string_t(&mut self, s: &[u8], pp: &mut usize, tainted: bool) -> Option<Obj> {
        skip_white(s, pp);
        if *pp + 2 <= s.len() {
            if s[*pp] == b'(' {
                return self.parse_pdf_literal_string(s, pp, tainted);
            } else if s[*pp] == b'<' && (s[*pp + 1] == b'>' || s[*pp + 1].is_ascii_hexdigit()) {
                return self.parse_pdf_hex_string(s, pp);
            }
        }
        None
    }

    fn parse_dict_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        unknown: &mut Option<Unknown<'_>>,
        tainted: bool,
    ) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p + 4 > s.len() || s[p] != b'<' || s[p + 1] != b'<' {
            return None;
        }
        p += 2;
        let result = self.new_dict();
        skip_white(s, &mut p);
        while p < s.len() && s[p] != b'>' {
            skip_white(s, &mut p);
            let Some(key) = self.parse_pdf_name(s, &mut p) else {
                self.release(result);
                return None;
            };
            skip_white(s, &mut p);
            let Some(value) = self.parse_obj_ext(s, &mut p, pf, unknown, tainted) else {
                self.release(key);
                self.release(result);
                return None;
            };
            self.add_dict(result, key, Some(value));
            skip_white(s, &mut p);
        }
        if p + 2 > s.len() || s[p] != b'>' || s[p + 1] != b'>' {
            self.release(result);
            return None;
        }
        *pp = p + 2;
        Some(result)
    }

    fn parse_array_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        unknown: &mut Option<Unknown<'_>>,
        tainted: bool,
    ) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p + 2 > s.len() || s[p] != b'[' {
            return None;
        }
        let result = self.new_array();
        p += 1;
        skip_white(s, &mut p);
        while p < s.len() && s[p] != b']' {
            let Some(elem) = self.parse_obj_ext(s, &mut p, pf, unknown, tainted) else {
                self.release(result);
                return None;
            };
            self.add_array(result, elem);
            skip_white(s, &mut p);
        }
        if p >= s.len() || s[p] != b']' {
            self.release(result);
            return None;
        }
        *pp = p + 1;
        Some(result)
    }

    fn parse_pdf_stream(&mut self, s: &[u8], pp: &mut usize, dict: Obj) -> Option<Obj> {
        let mut p = *pp;
        skip_white(s, &mut p);
        if p + 6 > s.len() || &s[p..p + 6] != b"stream" {
            return None;
        }
        p += 6;
        if p < s.len() && s[p] == b'\n' {
            p += 1;
        } else if p + 1 < s.len() && s[p] == b'\r' && s[p + 1] == b'\n' {
            p += 2;
        }
        let tmp = self.lookup_dict(dict, b"Length")?;
        let tmp2 = self.deref_obj(Some(tmp));
        let stream_length: i64 = if self.type_of(tmp2) == crate::obj::PDF_NUMBER {
            self.number_value(tmp2.expect("number")) as i64
        } else {
            -1
        };
        self.release_opt(tmp2);
        if stream_length < 0 || p + stream_length as usize > s.len() {
            return None;
        }
        let stream_length = stream_length as usize;
        let filters = self.lookup_dict(dict, b"Filter");
        let result = if filters.is_none() && stream_length > 10 {
            self.new_stream(crate::obj::STREAM_COMPRESS)
        } else {
            self.new_stream(0)
        };
        let sd = self.stream_dict(result);
        self.merge_dict(sd, dict);
        self.add_stream(result, &s[p..p + stream_length]);
        p += stream_length;
        if p < s.len() && s[p] == b'\r' {
            p += 1;
        }
        if p < s.len() && s[p] == b'\n' {
            p += 1;
        }
        if p + 9 > s.len() || &s[p..p + 9] != b"endstream" {
            self.release(result);
            return None;
        }
        p += 9;
        *pp = p;
        Some(result)
    }

    fn try_pdf_reference(
        &mut self,
        s: &[u8],
        start: usize,
        endptr: &mut usize,
        pf: u32,
    ) -> Option<Obj> {
        *endptr = start;
        let mut p = start;
        skip_white(s, &mut p);
        if p + 5 > s.len() || !s[p].is_ascii_digit() {
            return None;
        }
        let mut id: u32 = 0;
        while !is_space(at(s, p)) {
            if p >= s.len() || !s[p].is_ascii_digit() {
                return None;
            }
            id = id.wrapping_mul(10).wrapping_add(u32::from(s[p] - b'0'));
            p += 1;
        }
        skip_white(s, &mut p);
        if p >= s.len() || !s[p].is_ascii_digit() {
            return None;
        }
        let mut gen_: u16 = 0;
        while !is_space(at(s, p)) {
            if p >= s.len() || !s[p].is_ascii_digit() {
                return None;
            }
            gen_ = gen_.wrapping_mul(10).wrapping_add(u16::from(s[p] - b'0'));
            p += 1;
        }
        skip_white(s, &mut p);
        if p >= s.len() || s[p] != b'R' {
            return None;
        }
        p += 1;
        if !token_end(s, p) {
            return None;
        }
        *endptr = p;
        Some(self.new_indirect(Some(pf), id, gen_))
    }

    fn parse_obj_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        unknown: &mut Option<Unknown<'_>>,
        tainted: bool,
    ) -> Option<Obj> {
        skip_white(s, pp);
        if *pp >= s.len() {
            return None;
        }
        match s[*pp] {
            b'<' => {
                if at(s, *pp + 1) != b'<' {
                    self.parse_pdf_hex_string(s, pp)
                } else {
                    let result = self.parse_dict_ext(s, pp, pf, unknown, tainted);
                    skip_white(s, pp);
                    if let Some(dict) = result
                        && *pp + 15 <= s.len()
                        && &s[*pp..*pp + 6] == b"stream"
                    {
                        let r = self.parse_pdf_stream(s, pp, dict);
                        self.release(dict);
                        r
                    } else {
                        result
                    }
                }
            }
            b'(' => self.parse_pdf_string_t(s, pp, tainted),
            b'[' => self.parse_array_ext(s, pp, pf, unknown, tainted),
            b'/' => self.parse_pdf_name(s, pp),
            b'n' => self.parse_pdf_null(s, pp),
            b't' | b'f' => self.parse_pdf_boolean(s, pp),
            b'+' | b'-' | b'.' => self.parse_pdf_number(s, pp),
            b'0'..=b'9' => {
                if let Some(pf) = pf {
                    let mut next = *pp;
                    if let Some(r) = self.try_pdf_reference(s, *pp, &mut next, pf) {
                        *pp = next;
                        return Some(r);
                    }
                }
                self.parse_pdf_number(s, pp)
            }
            _ => match unknown {
                Some(h) => h(self, s, pp),
                None => None,
            },
        }
    }

    /// `parse_pdf_object_extended`.
    pub fn parse_pdf_object_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        mut unknown: Option<Unknown<'_>>,
    ) -> Option<Obj> {
        self.parse_obj_ext(s, pp, pf, &mut unknown, false)
    }

    /// `parse_pdf_object`.
    pub fn parse_pdf_object(&mut self, s: &[u8], pp: &mut usize, pf: Option<u32>) -> Option<Obj> {
        self.parse_obj_ext(s, pp, pf, &mut None, false)
    }

    /// `parse_pdf_dict`.
    pub fn parse_pdf_dict(&mut self, s: &[u8], pp: &mut usize, pf: Option<u32>) -> Option<Obj> {
        self.parse_dict_ext(s, pp, pf, &mut None, false)
    }

    /// `parse_pdf_array`.
    pub fn parse_pdf_array(&mut self, s: &[u8], pp: &mut usize, pf: Option<u32>) -> Option<Obj> {
        self.parse_array_ext(s, pp, pf, &mut None, false)
    }

    /// `parse_pdf_dict` with an unknown-token handler (`parse_pdf_dict_extended`).
    pub fn parse_pdf_dict_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        mut unknown: Option<Unknown<'_>>,
    ) -> Option<Obj> {
        self.parse_dict_ext(s, pp, pf, &mut unknown, false)
    }

    /// `parse_pdf_array_extended`.
    pub fn parse_pdf_array_ext(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        pf: Option<u32>,
        mut unknown: Option<Unknown<'_>>,
    ) -> Option<Obj> {
        self.parse_array_ext(s, pp, pf, &mut unknown, false)
    }

    /// `parse_pdf_tainted_dict`: strings may hold two-byte characters
    /// whose second byte is a delimiter.
    pub fn parse_pdf_tainted_dict(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        mut unknown: Option<Unknown<'_>>,
    ) -> Option<Obj> {
        self.parse_dict_ext(s, pp, None, &mut unknown, true)
    }
}
