//! Reading `.idx` files (scanid.c).

use alloc::vec::Vec;

use crate::{
    ALPL, ALPU, ARAB, ARGUMENT_MAX, ARRAY_MAX, EMPTY, EOF, FIELD_MAX, Field, LFD, Mk, NUL,
    NUMBER_MAX, P, PAGEFIELD_MAX, ROML, ROMU, SPC, TAB, at, cstr, fmt, group_type, strtoint,
};

const ARABIC_MAX: usize = 99;
const ROMAN_MAX: usize = 99;

fn roman_lower_val(c: u8) -> i32 {
    match c {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        b'c' => 100,
        b'd' => 500,
        b'm' => 1000,
        _ => 0,
    }
}

fn roman_upper_val(c: u8) -> i32 {
    roman_lower_val(c.to_ascii_lowercase()) * i32::from(c.is_ascii_uppercase())
}

fn is_roman_lower(c: u8) -> bool {
    b"ivxlcdm".contains(&c)
}

fn is_roman_upper(c: u8) -> bool {
    b"IVXLCDM".contains(&c)
}

fn alpha_val(c: u8) -> i32 {
    if c.is_ascii_uppercase() {
        i32::from(c - b'A')
    } else if c.is_ascii_lowercase() {
        i32::from(c - b'a')
    } else {
        0
    }
}

/// `strspn`.
fn strspn(s: &[u8], set: &[u8]) -> usize {
    cstr(s).iter().take_while(|c| set.contains(c)).count()
}

/// `strchr(s, c) != NULL` for a non-NUL `c`.
fn has(s: &[u8], c: u8) -> bool {
    cstr(s).contains(&c)
}

/// How an `.idx` line ends up.
enum Arg {
    Ok,
    Bad,
}

impl Mk<'_> {
    /// `IDX_ERROR`.
    pub(crate) fn idx_error(&mut self, f: &str, args: &[P]) {
        self.error_start();
        let name = self.idx_names[self.idx_fn].clone();
        let head = fmt(
            "!! Input index error (file = %s, line = %d):\n   -- ",
            &[P::S(&name), self.idx_lc.into()],
        );
        self.ilg_put(&head);
        let s = fmt(f, args);
        self.ilg_put(&s);
        self.idx_ec += 1;
    }

    /// `IDX_SKIPLINE` (the caller resets `arg_count`).
    fn idx_skipline(&mut self) {
        loop {
            let c = self.idx.getc();
            if c == LFD || c == EOF {
                break;
            }
        }
        self.idx_lc += 1;
    }

    fn flush_to_eol(&mut self) {
        loop {
            let a = self.idx.getc();
            if a == LFD || a == EOF {
                break;
            }
        }
    }

    /// `scan_idx`.
    pub(crate) fn scan_idx(&mut self) {
        let name = self.idx_names[self.idx_fn].clone();
        self.message("Scanning input file %s...", &[P::S(&name)]);
        self.idx_lc = 0;
        self.idx_tc = 0;
        self.idx_ec = 0;
        self.idx_dc = 0;
        self.comp_len = cstr(&self.st.page_comp).len();
        let mut keyword: Vec<u8> = Vec::new();
        let mut arg_count: i32 = -1;
        loop {
            let c = self.idx.getc();
            match c {
                EOF => {
                    if arg_count == 2 {
                        self.idx_lc += 1;
                        if self.make_key() {
                            self.idx_dot_tick(crate::DOT_MAX);
                        }
                        arg_count = -1;
                    } else {
                        if arg_count > -1 {
                            self.idx_lc += 1;
                            self.idx_error("Missing arguments -- need two (premature EOF).\n", &[]);
                        }
                        break;
                    }
                }
                LFD => {
                    self.idx_lc += 1;
                    if arg_count == 2 {
                        if self.make_key() {
                            self.idx_dot_tick(crate::DOT_MAX);
                        }
                        arg_count = -1;
                    } else if arg_count > -1 {
                        self.idx_error("Missing arguments -- need two (premature LFD).\n", &[]);
                        arg_count = -1;
                    }
                }
                TAB | SPC => {}
                _ => {
                    let ch = c as u8;
                    match arg_count {
                        -1 => {
                            keyword.clear();
                            keyword.push(ch);
                            arg_count += 1;
                            self.idx_tc += 1;
                        }
                        0 => {
                            if ch == self.st.idx_aopen {
                                arg_count += 1;
                                if cstr(&keyword) == cstr(&self.st.idx_keyword) {
                                    if matches!(self.scan_arg1(), Arg::Bad) {
                                        arg_count = -1;
                                    }
                                } else {
                                    self.idx_skipline();
                                    arg_count = -1;
                                    let k = keyword.clone();
                                    self.idx_error(
                                        "Unknown index keyword %s.\n",
                                        &[P::S(cstr(&k))],
                                    );
                                }
                            } else if keyword.len() < ARRAY_MAX {
                                keyword.push(ch);
                            } else {
                                self.idx_skipline();
                                arg_count = -1;
                                let k = keyword.clone();
                                self.idx_error(
                                    "Index keyword %s too long (max %d).\n",
                                    &[P::S(cstr(&k)), (ARRAY_MAX as i32).into()],
                                );
                            }
                        }
                        1 => {
                            if ch == self.st.idx_aopen {
                                arg_count += 1;
                                if matches!(self.scan_arg2(), Arg::Bad) {
                                    arg_count = -1;
                                }
                            } else {
                                self.idx_skipline();
                                arg_count = -1;
                                self.idx_error(
                                    "No opening delimiter for second argument (illegal character `%c').\n",
                                    &[P::S(&[ch])],
                                );
                            }
                        }
                        2 => {
                            self.idx_skipline();
                            arg_count = -1;
                            self.idx_error(
                                "No closing delimiter for second argument (illegal character `%c').\n",
                                &[P::S(&[ch])],
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        self.idx_tt += self.idx_tc;
        self.idx_et += self.idx_ec;
        self.done(
            self.idx_tc - self.idx_ec,
            "entries accepted",
            self.idx_ec,
            "rejected",
        );
    }

    /// `make_key`.
    fn make_key(&mut self) -> bool {
        let mut data = Field {
            typ: EMPTY,
            fn_: self.idx_fn,
            ..Field::default()
        };
        if !self.scan_key(&mut data) {
            return false;
        }
        data.group = group_type(&data.sf[0]);
        data.lpg = cstr(&self.no).to_vec();
        let no = data.lpg.clone();
        let (mut count, mut typ) = (0usize, EMPTY);
        if !self.scan_no(&no, &mut data.npg, &mut count, &mut typ) {
            return false;
        }
        data.count = count;
        data.typ = typ;
        data.lc = self.idx_lc;
        self.entries.push(data);
        true
    }

    /// `scan_key`.
    fn scan_key(&mut self, data: &mut Field) -> bool {
        let mut i = 0usize;
        let mut n = 0usize;
        let mut second_round = false;
        let last = FIELD_MAX - 1;
        let st_encap = self.st.idx_encap;
        let st_actual = self.st.idx_actual;
        loop {
            let k = self.key[n];
            if k == NUL {
                break;
            }
            let len = cstr(&self.key).len();
            if k == st_encap {
                n += 1;
                return match self.scan_field(&mut n, len, false, false, false) {
                    Some(f) => {
                        data.encap = f;
                        self.check_null_fields(data)
                    }
                    None => false,
                };
            }
            if k == st_actual {
                n += 1;
                match self.scan_field(&mut n, len, i != last, true, false) {
                    Some(f) => data.af[i] = f,
                    None => return false,
                }
            } else {
                if second_round {
                    i += 1;
                    n += 1;
                }
                match self.scan_field(&mut n, len, i != last, true, true) {
                    Some(f) => data.sf[i] = f,
                    None => return false,
                }
                second_round = true;
                if self.german_sort && data.sf[i].contains(&b'"') {
                    let (sf, af) = search_quote(&data.sf[i]);
                    data.sf[i] = sf;
                    data.af[i] = af;
                }
            }
        }
        self.check_null_fields(data)
    }

    /// The end of `scan_key`: fields that must not be empty.
    fn check_null_fields(&mut self, data: &Field) -> bool {
        let bad = data.sf[0].is_empty()
            || (1..FIELD_MAX - 1).any(|i| {
                data.sf[i].is_empty() && (!data.af[i].is_empty() || !data.sf[i + 1].is_empty())
            })
            || data.sf[FIELD_MAX - 1].is_empty() && !data.af[FIELD_MAX - 1].is_empty();
        if bad {
            self.idx_error("Illegal null field.\n", &[]);
        }
        !bad
    }

    /// `scan_field`: the field at `key[n..]`, or `None` after an error.
    fn scan_field(
        &mut self,
        n: &mut usize,
        len_field: usize,
        ck_level: bool,
        ck_encap: bool,
        ck_actual: bool,
    ) -> Option<Vec<u8>> {
        let key = |mk: &Self, n: usize| at(&mk.key, n);
        let (level, encap, actual) = (self.st.idx_level, self.st.idx_encap, self.st.idx_actual);
        let (quote, escape) = (self.st.idx_quote, self.st.idx_escape);
        let mut field: Vec<u8> = Vec::new();
        if self.compress_blanks && matches!(key(self, *n), b' ' | b'\t') {
            *n += 1;
        }
        loop {
            let mut overflow = false;
            let mut nbsh = 0;
            while key(self, *n) == escape {
                nbsh += 1;
                field.push(key(self, *n));
                if field.len() > len_field {
                    overflow = true;
                    break;
                }
                *n += 1;
            }
            if !overflow {
                let k = key(self, *n);
                if k == quote {
                    if nbsh % 2 == 0 {
                        *n += 1;
                        field.push(key(self, *n));
                    } else {
                        field.push(k);
                    }
                    overflow = field.len() > len_field;
                } else if (ck_level && k == level)
                    || (ck_encap && k == encap)
                    || (ck_actual && k == actual)
                    || k == NUL
                {
                    if self.compress_blanks && field.last() == Some(&b' ') {
                        field.pop();
                    }
                    return Some(cstr(&field).to_vec());
                } else {
                    field.push(k);
                    overflow = field.len() > len_field;
                    if !overflow {
                        let extra = if !ck_level && k == level {
                            Some(level)
                        } else if !ck_encap && k == encap {
                            Some(encap)
                        } else if !ck_actual && k == actual {
                            Some(actual)
                        } else {
                            None
                        };
                        if let Some(c) = extra {
                            self.idx_error(
                                "Extra `%c' at position %d of first argument.\n",
                                &[P::S(&[c]), (*n as i32 + 1).into()],
                            );
                            return None;
                        }
                    }
                }
            }
            if overflow || field.len() > len_field {
                let len = len_field as i32;
                if !ck_encap {
                    self.idx_error(
                        "Encapsulator of page number too long (max. %d).\n",
                        &[len.into()],
                    );
                } else if ck_actual {
                    self.idx_error("Index sort key too long (max. %d).\n", &[len.into()]);
                } else {
                    self.idx_error("Text of key entry too long (max. %d).\n", &[len.into()]);
                }
                return None;
            }
            *n += 1;
        }
    }

    /// `ENTER`.
    fn enter(
        &mut self,
        no: &[u8],
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
        v: i32,
    ) -> bool {
        if *count >= PAGEFIELD_MAX {
            self.idx_error(
                "Page number %s has too many fields (max. %d).",
                &[P::S(cstr(no)), (PAGEFIELD_MAX as i32).into()],
            );
            return false;
        }
        npg[*count] = v;
        *count += 1;
        true
    }

    /// `IS_COMPOSITOR` at `no[i..]`.
    fn is_comp(&self, no: &[u8], i: usize) -> bool {
        let comp = cstr(&self.st.page_comp);
        let rest = cstr(no.get(i..).unwrap_or(&[]));
        rest.starts_with(comp)
    }

    /// `scan_no`.
    fn scan_no(
        &mut self,
        no: &[u8],
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
        typ: &mut i32,
    ) -> bool {
        let no = cstr(no);
        let c0 = at(no, 0);
        let prec = self.st.page_prec.clone();
        let g = &mut self.type_guess[*count];
        if c0.is_ascii_digit() {
            *g = ARAB;
        } else if is_roman_lower(c0)
            && c0.is_ascii_lowercase()
            && has(&prec, b'r')
            && has(&prec, b'a')
        {
            if strspn(no, b"ivxlcdm") == 1 && *g != ROML && *g != ALPL {
                *g = if strspn(no, b"ivx") == 1 { ROML } else { ALPL };
            }
            if strspn(no, b"ivxlcdm") > 1 {
                *g = ROML;
            }
        } else if is_roman_upper(c0)
            && c0.is_ascii_uppercase()
            && has(&prec, b'R')
            && has(&prec, b'A')
        {
            if strspn(no, b"IVXLCDM") == 1 && *g != ROMU && *g != ALPU {
                *g = if strspn(no, b"IVX") == 1 { ROMU } else { ALPU };
            }
            if strspn(no, b"IVXLCDM") > 1 {
                *g = ROMU;
            }
        } else if is_roman_lower(c0) && has(&prec, b'r') {
            *g = ROML;
        } else if is_roman_upper(c0) && has(&prec, b'R') {
            *g = ROMU;
        } else if c0.is_ascii_lowercase() && has(&prec, b'a') {
            *g = ALPL;
        } else if c0.is_ascii_uppercase() && has(&prec, b'A') {
            *g = ALPU;
        } else {
            *g = EMPTY;
        }
        let g = *g;
        if c0.is_ascii_digit() {
            *typ = ARAB;
            self.scan_arabic(no, npg, count)
        } else if is_roman_lower(c0) && has(&prec, b'r') && (!has(&prec, b'a') || g == ROML) {
            *typ = ROML;
            self.scan_roman(no, npg, count, false)
        } else if is_roman_upper(c0) && has(&prec, b'R') && (!has(&prec, b'A') || g == ROMU) {
            *typ = ROMU;
            self.scan_roman(no, npg, count, true)
        } else if c0.is_ascii_lowercase() && has(&prec, b'a') {
            *typ = ALPL;
            self.scan_alpha(no, npg, count, ALPL)
        } else if c0.is_ascii_uppercase() && has(&prec, b'A') {
            *typ = ALPU;
            self.scan_alpha(no, npg, count, ALPU)
        } else {
            self.idx_error(
                "Illegal page number %s or page_precedence %s.\n",
                &[P::S(no), P::S(cstr(&prec))],
            );
            false
        }
    }

    /// The rest of a composite page number after field `i`.
    fn scan_rest(
        &mut self,
        no: &[u8],
        i: usize,
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
    ) -> bool {
        if self.is_comp(no, i) {
            let rest = no.get(i + self.comp_len..).unwrap_or(&[]).to_vec();
            let mut t = 0;
            self.scan_no(&rest, npg, count, &mut t)
        } else {
            true
        }
    }

    fn scan_arabic(
        &mut self,
        no: &[u8],
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
    ) -> bool {
        let mut i = 0usize;
        while at(no, i) != NUL && i <= ARABIC_MAX && !self.is_comp(no, i) {
            if at(no, i).is_ascii_digit() {
                i += 1;
            } else {
                self.idx_error(
                    "Illegal Arabic digit: position %d in %s.\n",
                    &[(i as i32 + 1).into(), P::S(no)],
                );
                return false;
            }
        }
        if i > ARABIC_MAX {
            self.idx_error(
                "Arabic page number %s too big (max %d digits).\n",
                &[P::S(no), (ARABIC_MAX as i32).into()],
            );
            return false;
        }
        let v = strtoint(&no[..i]).wrapping_add(self.st.page_offset[ARAB as usize]);
        if !self.enter(no, npg, count, v) {
            return false;
        }
        self.scan_rest(no, i, npg, count)
    }

    fn scan_roman(
        &mut self,
        no: &[u8],
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
        upper: bool,
    ) -> bool {
        let (mut i, mut inp, mut prev) = (0usize, 0i32, 0i32);
        while at(no, i) != NUL && i < ROMAN_MAX && !self.is_comp(no, i) {
            let c = at(no, i);
            let (is, val) = if upper {
                (is_roman_upper(c), roman_upper_val(c))
            } else {
                (is_roman_lower(c), roman_lower_val(c))
            };
            if is && val != 0 {
                let mut the_new = val;
                if prev == 0 {
                    prev = the_new;
                } else {
                    if prev < the_new {
                        prev = the_new - prev;
                        the_new = 0;
                    }
                    inp += prev;
                    prev = the_new;
                }
            } else {
                self.idx_error(
                    "Illegal Roman number: position %d in %s.\n",
                    &[(i as i32 + 1).into(), P::S(no)],
                );
                return false;
            }
            i += 1;
        }
        if i == ROMAN_MAX {
            self.idx_error(
                "Roman page number %s too big (max %d digits).\n",
                &[P::S(no), (ROMAN_MAX as i32).into()],
            );
            return false;
        }
        inp += prev;
        let off = self.st.page_offset[if upper { ROMU } else { ROML } as usize];
        if !self.enter(no, npg, count, inp.wrapping_add(off)) {
            return false;
        }
        self.scan_rest(no, i, npg, count)
    }

    fn scan_alpha(
        &mut self,
        no: &[u8],
        npg: &mut [i32; PAGEFIELD_MAX],
        count: &mut usize,
        typ: i32,
    ) -> bool {
        let v = alpha_val(at(no, 0)).wrapping_add(self.st.page_offset[typ as usize]);
        if !self.enter(no, npg, count, v) {
            return false;
        }
        if self.is_comp(no, 1) {
            let rest = no.get(self.comp_len + 1..).unwrap_or(&[]).to_vec();
            let mut t = 0;
            self.scan_no(&rest, npg, count, &mut t)
        } else {
            true
        }
    }

    /// `scan_arg1`: the first argument into `key`.
    fn scan_arg1(&mut self) -> Arg {
        let mut i = 0usize;
        let mut n = 0;
        let mut a;
        if self.compress_blanks {
            loop {
                a = self.idx.getc();
                if a != SPC && a != TAB {
                    break;
                }
            }
        } else {
            a = self.idx.getc();
        }
        let st = &self.st;
        let (quote, escape, aopen, aclose) = (
            i32::from(st.idx_quote),
            i32::from(st.idx_escape),
            i32::from(st.idx_aopen),
            i32::from(st.idx_aclose),
        );
        while i < ARGUMENT_MAX && a != EOF {
            if a == quote || a == escape {
                self.key[i] = a as u8;
                i += 1;
                a = self.idx.getc();
                self.key[i] = a as u8;
                i += 1;
            } else if a == aopen {
                self.key[i] = a as u8;
                i += 1;
                n += 1;
            } else if a == aclose {
                if n == 0 {
                    if self.compress_blanks && i > 0 && self.key[i - 1] == b' ' {
                        self.key[i - 1] = NUL;
                    } else {
                        self.key[i] = NUL;
                    }
                    return Arg::Ok;
                }
                self.key[i] = a as u8;
                i += 1;
                n -= 1;
            } else {
                match a {
                    LFD => {
                        self.idx_lc += 1;
                        self.idx_error("Incomplete first argument (premature LFD).\n", &[]);
                        return Arg::Bad;
                    }
                    TAB | SPC if self.compress_blanks => {
                        if i > 0 && self.key[i - 1] != b' ' && self.key[i - 1] != b'\t' {
                            self.key[i] = b' ';
                            i += 1;
                        }
                    }
                    _ => {
                        self.key[i] = a as u8;
                        i += 1;
                    }
                }
            }
            a = self.idx.getc();
        }
        self.flush_to_eol();
        self.idx_lc += 1;
        self.idx_error(
            "First argument too long (max %d).\n",
            &[(ARGUMENT_MAX as i32).into()],
        );
        Arg::Bad
    }

    /// `scan_arg2`: the second argument into `no`.
    fn scan_arg2(&mut self) -> Arg {
        let mut i = 0usize;
        let mut hit_blank = false;
        let mut a;
        loop {
            a = self.idx.getc();
            if a != SPC && a != TAB {
                break;
            }
        }
        let aclose = i32::from(self.st.idx_aclose);
        while i < NUMBER_MAX {
            if a == aclose {
                self.no[i] = NUL;
                return Arg::Ok;
            }
            match a {
                LFD => {
                    self.idx_lc += 1;
                    self.idx_error("Incomplete second argument (premature LFD).\n", &[]);
                    return Arg::Bad;
                }
                TAB | SPC => hit_blank = true,
                _ => {
                    if hit_blank {
                        self.flush_to_eol();
                        self.idx_lc += 1;
                        self.idx_error("Illegal space within numerals in second argument.\n", &[]);
                        return Arg::Bad;
                    }
                    self.no[i] = a as u8;
                    i += 1;
                }
            }
            a = self.idx.getc();
        }
        self.flush_to_eol();
        self.idx_lc += 1;
        self.idx_error(
            "Second argument too long (max %d).\n",
            &[(NUMBER_MAX as i32).into()],
        );
        Arg::Bad
    }
}

/// `search_quote` (`-g`): German umlauts `"a` sort as `ae`; the actual
/// key keeps the original (or is empty when nothing changed).
fn search_quote(sort_key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let actual = sort_key.to_vec();
    let mut sort = sort_key.to_vec();
    let mut found = false;
    let mut p = sort.iter().position(|&c| c == b'"');
    while let Some(k) = p {
        let rep: Option<&[u8; 2]> = match at(&sort, k + 1) {
            b'a' => Some(b"ae"),
            b'A' => Some(b"Ae"),
            b'o' => Some(b"oe"),
            b'O' => Some(b"Oe"),
            b'u' => Some(b"ue"),
            b'U' => Some(b"Ue"),
            b's' => Some(b"ss"),
            _ => None,
        };
        if let Some(r) = rep {
            found = true;
            sort[k] = r[0];
            sort[k + 1] = r[1];
        }
        p = sort[k + 1..]
            .iter()
            .position(|&c| c == b'"')
            .map(|q| q + k + 1);
    }
    (sort, if found { actual } else { Vec::new() })
}
