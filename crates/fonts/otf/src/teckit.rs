//! TECkit's runtime (`libs/teckit/source/Engine.cpp`): a compiled mapping
//! (`.tec`, possibly zlib-compressed `zQmp`) run over complete input, as
//! XeTeX calls it (`TECkit_ConvertBuffer` with the input complete and
//! unmapped characters replaced silently, then `TECkit_ResetConverter`).
//!
//! Each pass runs over the whole text, which is what the engine's chain
//! of stages computes when the input is complete. Normalization passes
//! (`NFC `/`NFD ` tables, or a mapping that expects NFC or NFD input) are
//! not ported: no mapping in TeX Live has them, and such a mapping fails
//! to load here ([`Mapping::new`] returns `None`).

use alloc::vec::Vec;

use crate::face::{rd_u16, rd_u32};

const MAGIC: u32 = 0x714d_6170; // 'qMap'
const MAGIC_CMP: u32 = 0x7a51_6d70; // 'zQmp'
const CURRENT_FILE_VERSION: u32 = 0x0003_0000;

const FLAGS_EXPECTS_NFC: u32 = 1;
const FLAGS_EXPECTS_NFD: u32 = 2;
const FLAGS_UNICODE: u32 = 0x0001_0000;

const TABLE_BB: u32 = 0x422d_3e42;
const TABLE_BU: u32 = 0x422d_3e55;
const TABLE_UB: u32 = 0x552d_3e42;
const TABLE_UU: u32 = 0x552d_3e55;
const TABLE_FLAGS_SUPPLEMENTARY: u32 = 1;

const LOOKUP_STRING_RULES: u8 = 0xff;
const LOOKUP_UNMAPPED: u8 = 0xfd;
const LOOKUP_RULE_TYPE_MASK: u8 = 0xc0;
const LOOKUP_EXT_STRING_RULES: u8 = 0x80;
const LOOKUP_EXT_RULE_COUNT_MASK: u8 = 0x3f;
const LOOKUP_ILLEGAL_DBCS: u8 = 0xfe;

const MATCH_NEGATE: u8 = 0x80;
const MATCH_NONLIT: u8 = 0x40;
const MATCH_TYPE_MASK: u8 = 0x3f;
const MATCH_CLASS: u8 = 1;
const MATCH_BGROUP: u8 = 2;
const MATCH_EGROUP: u8 = 3;
const MATCH_OR: u8 = 4;
const MATCH_ANY: u8 = 5;
const MATCH_EOS: u8 = 6;
const USV_MASK: u32 = 0x001f_ffff;

const REP_LITERAL: u8 = 0;
const REP_CLASS: u8 = 1;
const REP_COPY: u8 = 7;
const REP_UNMAPPED: u8 = 0x0f;

const END_OF_TEXT: u32 = 0xffff_ffff;

/// A compiled mapping, one direction of it, ready to convert.
#[derive(Clone, Debug)]
pub struct Mapping {
    table: Vec<u8>,
    /// Offsets of the pipeline's tables.
    passes: Vec<usize>,
    /// The input is Unicode (else bytes).
    input_unicode: bool,
}

/// The status of a conversion, as `TECkit_ConvertBuffer` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// `kStatus_IncompleteChar`: a high surrogate ends the input.
    IncompleteChar,
}

impl Mapping {
    /// `TECkit_CreateConverter(mapping, size, forward, in, out)` with
    /// Unicode in and Unicode (`unicode_out`) or bytes out: `None` where it
    /// fails (bad data, a form that does not match, normalization).
    #[must_use]
    pub fn new(file: &[u8], forward: bool, unicode_out: bool) -> Option<Mapping> {
        let table = if rd_u32(file, 0)? == MAGIC_CMP {
            let len = rd_u32(file, 4)? as usize;
            let z = file.get(8..)?;
            // zlib's `uncompress`: the whole stream, else an error.
            let (&cmf, &flg) = (z.first()?, z.get(1)?);
            if cmf & 0x0f != 8 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
                return None;
            }
            let mut out = Vec::with_capacity(len);
            if !crate::inflate::inflate_raw(&z[2..], &mut out) || out.len() != len {
                return None;
            }
            out
        } else {
            file.to_vec()
        };
        let t = &table;
        if rd_u32(t, 0)? != MAGIC {
            return None;
        }
        if rd_u32(t, 4)? & 0xFFFF_0000 > CURRENT_FILE_VERSION & 0xFFFF_0000 {
            return None;
        }
        let lhs = rd_u32(t, 12)?;
        let rhs = rd_u32(t, 16)?;
        let num_names = rd_u32(t, 20)? as usize;
        let num_fwd = rd_u32(t, 24)? as usize;
        let num_rev = rd_u32(t, 28)? as usize;
        let (base, n) = if forward {
            (32 + 4 * num_names, num_fwd)
        } else {
            (32 + 4 * (num_names + num_fwd), num_rev)
        };
        let target = if forward { rhs } else { lhs };
        if (target & FLAGS_UNICODE != 0) != unicode_out {
            return None;
        }
        let source = if forward { lhs } else { rhs };
        let input_unicode = source & FLAGS_UNICODE != 0;
        if !input_unicode {
            // XeTeX only converts from UTF-16.
            return None;
        }
        if source & (FLAGS_EXPECTS_NFC | FLAGS_EXPECTS_NFD) != 0 {
            return None;
        }
        let mut passes = Vec::with_capacity(n);
        for i in 0..n {
            let off = rd_u32(t, base + 4 * i)? as usize;
            match rd_u32(t, off)? {
                TABLE_BB | TABLE_BU | TABLE_UU | TABLE_UB => passes.push(off),
                _ => return None,
            }
        }
        Some(Mapping {
            table,
            passes,
            input_unicode,
        })
    }

    /// Converts UTF-16 `text` (complete input): the output code points, or
    /// an error (`TECkit_ConvertBuffer`'s status other than no error).
    pub fn convert(&self, text: &[u16]) -> Result<Vec<u32>, Error> {
        // Converter::_getCharFn for UTF-16: a high surrogate takes the next
        // unit whatever it is; one at the end is an incomplete character.
        let mut chars = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let c = u32::from(text[i]);
            if (0xD800..=0xDBFF).contains(&c) {
                let Some(&l) = text.get(i + 1) else {
                    return Err(Error::IncompleteChar);
                };
                chars.push(((c - 0xD800) << 10) + (u32::from(l).wrapping_sub(0xDC00)) + 0x10000);
                i += 2;
            } else {
                chars.push(c);
                i += 1;
            }
        }
        let _ = self.input_unicode;
        for &p in &self.passes {
            chars = Pass::new(&self.table, p).run(&chars);
        }
        Ok(chars)
    }

    /// `applymapping`: `text` mapped, as UTF-16; empty where the conversion
    /// fails (XeTeX then gets length 0).
    #[must_use]
    pub fn apply_utf16(&self, text: &[u16]) -> Vec<u16> {
        let Ok(chars) = self.convert(text) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(chars.len());
        for c in chars {
            if c < 0x10000 {
                out.push(c as u16);
            } else {
                let c = c.wrapping_sub(0x10000);
                out.push(((c >> 10) + 0xD800) as u16);
                out.push(((c & 0x3FF) + 0xDC00) as u16);
            }
        }
        out
    }

    /// `applytfmfontmapping`: character `c` mapped to bytes; the first byte,
    /// or 0 for none.
    #[must_use]
    pub fn apply_byte(&self, c: u16) -> u8 {
        match self.convert(&[c]) {
            Ok(out) => out.first().map_or(0, |&b| b as u8),
            Err(_) => 0,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct MatchInfo {
    class_index: u32,
    group_repeats: i32,
    start: i32,
    limit: i32,
}

/// One mapping pass (`Pass`), over the whole input.
struct Pass<'a> {
    t: &'a [u8],
    hdr: usize,
    input_unicode: bool,
    output_unicode: bool,
    supplementary: bool,
    page_base: usize,
    plane_map: usize,
    num_page_maps: usize,
    lookup_base: usize,
    match_class_base: usize,
    rep_class_base: usize,
    string_list_base: usize,
    string_rule_data: usize,
    replacement: u32,
    // the current input
    input: &'a [u32],
    pos: usize,
    // matching state
    pattern: usize,
    pattern_length: i32,
    direction: i32,
    info: [MatchInfo; 256],
    info_limit: i32,
    match_elems: i32,
    matched_length: i32,
    out: Vec<u32>,
}

impl<'a> Pass<'a> {
    fn new(t: &'a [u8], hdr: usize) -> Self {
        let r32 = |o: usize| rd_u32(t, hdr + o).unwrap_or(0) as usize;
        let ty = rd_u32(t, hdr).unwrap_or(0);
        let input_unicode = (ty >> 24) as u8 == b'U';
        let output_unicode = ty as u8 == b'U';
        let supplementary = rd_u32(t, hdr + 12).unwrap_or(0) & TABLE_FLAGS_SUPPLEMENTARY != 0;
        let mut page_base = hdr + r32(16);
        let mut plane_map = 0;
        let mut num_page_maps = 1;
        if input_unicode && supplementary {
            plane_map = page_base;
            page_base += 20;
            num_page_maps = *t.get(plane_map + 17).unwrap_or(&0) as usize;
        }
        Pass {
            t,
            hdr,
            input_unicode,
            output_unicode,
            supplementary,
            page_base,
            plane_map,
            num_page_maps,
            lookup_base: hdr + r32(20),
            match_class_base: hdr + r32(24),
            rep_class_base: hdr + r32(28),
            string_list_base: hdr + r32(32),
            string_rule_data: hdr + r32(36),
            replacement: rd_u32(t, hdr + 44).unwrap_or(0),
            input: &[],
            pos: 0,
            pattern: 0,
            pattern_length: 0,
            direction: 1,
            info: [MatchInfo::default(); 256],
            info_limit: 0,
            match_elems: 0,
            matched_length: 0,
            out: Vec::new(),
        }
    }

    fn u8(&self, o: usize) -> u8 {
        *self.t.get(o).unwrap_or(&0)
    }
    fn u16(&self, o: usize) -> u16 {
        rd_u16(self.t, o).unwrap_or(0)
    }
    fn u32(&self, o: usize) -> u32 {
        rd_u32(self.t, o).unwrap_or(0)
    }

    fn run(mut self, input: &'a [u32]) -> Vec<u32> {
        self.input = input;
        self.pos = 0;
        while self.pos < input.len() {
            self.do_mapping();
        }
        self.out
    }

    /// `inputChar(i)`: the character `i` places from the current one, or
    /// end of text.
    fn input_char(&self, i: i32) -> u32 {
        let p = self.pos as i64 + i64::from(i);
        if p < 0 || p >= self.input.len() as i64 {
            END_OF_TEXT
        } else {
            self.input[p as usize]
        }
    }

    fn class_match(&self, class: u32, c: u32) -> i64 {
        let base = self.match_class_base;
        let cp = base + self.u32(base + 4 * class as usize) as usize;
        let count = self.u32(cp) as usize;
        let members = cp + 4;
        let width = if self.input_unicode {
            if self.supplementary { 4 } else { 2 }
        } else {
            1
        };
        let read = |i: usize| -> u32 {
            match width {
                4 => self.u32(members + 4 * i),
                2 => u32::from(self.u16(members + 2 * i)),
                _ => u32::from(self.u8(members + i)),
            }
        };
        // TECkit's binary_search: the first member not below `c`.
        let (mut lo, mut n) = (0usize, count);
        while n > 0 {
            let half = n / 2;
            if read(lo + half) < c {
                lo += half + 1;
                n -= half + 1;
            } else {
                n = half;
            }
        }
        if read(lo) == c { lo as i64 } else { -1 }
    }

    fn rep_class_member(&self, class: u32, index: u32) -> u32 {
        let base = self.rep_class_base;
        let cp = base + self.u32(base + 4 * class as usize) as usize;
        let count = self.u32(cp);
        let members = cp + 4;
        if index >= count {
            return 0;
        }
        let i = index as usize;
        if self.output_unicode {
            if self.supplementary {
                self.u32(members + 4 * i)
            } else {
                u32::from(self.u16(members + 2 * i))
            }
        } else {
            u32::from(self.u8(members + i))
        }
    }

    /// The match element at `index` of the current pattern:
    /// (repeat, type byte, bytes 2 and 3, the whole as u32).
    fn elem(&self, index: i32) -> (u8, u8, u8, u8, u32) {
        let o = self.pattern + 4 * index as usize;
        (
            self.u8(o),
            self.u8(o + 1),
            self.u8(o + 2),
            self.u8(o + 3),
            self.u32(o),
        )
    }

    fn elem_matches(&mut self, index: i32, repeats: i32, ty: u8, negate: bool, c: u32) -> bool {
        let (_, _, b2, b3, whole) = self.elem(index);
        let m = match ty {
            0 => whole & USV_MASK == c,
            MATCH_CLASS => {
                let ci = self.class_match(u32::from(u16::from_be_bytes([b2, b3])), c);
                let m = ci != -1;
                if m && repeats == 0 && index < self.info_limit {
                    self.info[index as usize].class_index = ci as u32;
                }
                m
            }
            MATCH_ANY => c != END_OF_TEXT,
            MATCH_EOS => c == END_OF_TEXT,
            _ => false,
        };
        m != negate
    }

    /// `Pass::match`: whether the pattern from `index` matches with
    /// `repeats` repeats so far, at text offset `text_loc`.
    fn matches(&mut self, mut index: i32, mut repeats: i32, mut text_loc: i32) -> bool {
        let rval = 'ret: loop {
            if repeats == 0 {
                if index == self.match_elems {
                    self.matched_length = text_loc;
                }
                if index < self.info_limit {
                    self.info[index as usize].start = text_loc;
                }
            }
            if index >= self.pattern_length {
                break 'ret true;
            }
            let (rep, ty_byte, _, b3, _) = self.elem(index);
            let repeat_min = i32::from(rep >> 4);
            let repeat_max = i32::from(rep & 0x0f);
            let negate = ty_byte & MATCH_NEGATE != 0;
            let ty = if ty_byte & MATCH_NONLIT != 0 {
                ty_byte & MATCH_TYPE_MASK
            } else {
                0
            };
            if ty == MATCH_BGROUP {
                self.info[index as usize].group_repeats = repeats;
                if repeats < repeat_max {
                    let mut alt = index;
                    loop {
                        if self.matches(alt + 1, 0, text_loc) {
                            break 'ret true;
                        }
                        let (_, _, d_next, _, _) = self.elem(alt);
                        alt += i32::from(d_next);
                        let (_, t2, _, _, _) = self.elem(alt);
                        if t2 & MATCH_TYPE_MASK != MATCH_OR {
                            break;
                        }
                    }
                }
                if repeats >= repeat_min {
                    let d_after = i32::from(b3);
                    let mr = self.matches(index + d_after, 0, text_loc);
                    if mr && index < self.info_limit {
                        self.info[index as usize].limit = text_loc;
                        let mut i = index + d_after - 1;
                        while i > index {
                            if i < self.info_limit {
                                let inf = &mut self.info[i as usize];
                                if inf.start > text_loc {
                                    inf.start = text_loc;
                                }
                                if inf.limit > text_loc {
                                    inf.limit = text_loc;
                                }
                            }
                            i -= 1;
                        }
                    }
                    break 'ret mr;
                }
                break 'ret false;
            } else if ty == MATCH_OR || ty == MATCH_EGROUP {
                let start = index - i32::from(b3);
                let gr = self.info[start as usize].group_repeats;
                break 'ret self.matches(start, gr + 1, text_loc);
            }
            while repeats < repeat_min {
                let c = self.input_char(text_loc);
                if !self.elem_matches(index, repeats, ty, negate, c) {
                    break 'ret false;
                }
                repeats += 1;
                text_loc += self.direction;
            }
            if index < self.info_limit {
                self.info[index as usize].limit = text_loc;
            }
            if repeat_min == repeat_max {
                index += 1;
                repeats = 0;
                continue;
            }
            if repeats < repeat_max {
                let c = self.input_char(text_loc);
                if self.elem_matches(index, repeats, ty, negate, c)
                    && self.matches(index, repeats + 1, text_loc + self.direction)
                {
                    break 'ret true;
                }
            }
            break 'ret self.matches(index + 1, 0, text_loc);
        };
        if !rval && index < self.info_limit {
            self.info[index as usize].limit = text_loc;
        }
        rval
    }

    fn output_unmapped(&mut self, c: u32) {
        if self.output_unicode == self.input_unicode {
            self.out.push(c);
        } else {
            self.out.push(self.replacement);
        }
    }

    /// `Pass::DoMapping` at the current character (which exists).
    fn do_mapping(&mut self) {
        let in_char = self.input_char(0);
        self.matched_length = 1;
        let lookup = if self.input_unicode {
            let mut char_index = 0usize;
            if self.lookup_base != self.page_base {
                let plane = in_char >> 16;
                let page_map = if self.supplementary {
                    let pm = self.u8(self.plane_map + plane as usize);
                    (plane < 17 && pm != 0xff).then(|| self.page_base + 256 * pm as usize)
                } else {
                    (plane == 0).then_some(self.page_base)
                };
                if let Some(pm) = page_map {
                    let page = ((in_char >> 8) & 0xff) as usize;
                    let p = self.u8(pm + page);
                    if p != 0xff {
                        let char_map_base = self.page_base + 256 * self.num_page_maps;
                        let char_map = char_map_base + 2 * 256 * p as usize;
                        char_index = self.u16(char_map + 2 * (in_char & 0xff) as usize) as usize;
                    }
                }
            }
            self.lookup_base + 4 * char_index
        } else if self.page_base != self.hdr {
            let page = self.u8(self.page_base + in_char as usize);
            if page == 0 {
                self.lookup_base + 4 * in_char as usize
            } else {
                let next = self.input_char(1);
                if next == END_OF_TEXT {
                    self.lookup_base + 4 * in_char as usize
                } else {
                    let l = self.lookup_base + 4 * (page as usize * 256 + next as usize);
                    if self.u8(l) == LOOKUP_ILLEGAL_DBCS {
                        self.lookup_base + 4 * in_char as usize
                    } else {
                        self.matched_length = 2;
                        l
                    }
                }
            }
        } else {
            self.lookup_base + 4 * in_char as usize
        };
        let rule_type = self.u8(lookup);
        if rule_type == LOOKUP_STRING_RULES
            || rule_type & LOOKUP_RULE_TYPE_MASK == LOOKUP_EXT_STRING_RULES
        {
            let mut rule_list = self.string_list_base + 4 * self.u16(lookup + 2) as usize;
            let mut matched = false;
            let mut allow_insertion = true;
            let mut rule_count = i32::from(self.u8(lookup + 1));
            if rule_type & LOOKUP_RULE_TYPE_MASK == LOOKUP_EXT_STRING_RULES {
                rule_count += 256 * i32::from(rule_type & LOOKUP_EXT_RULE_COUNT_MASK);
            }
            while rule_count > 0 {
                rule_count -= 1;
                let rule = self.string_rule_data + self.u32(rule_list) as usize;
                rule_list += 4;
                let match_len = i32::from(self.u8(rule));
                let post_len = i32::from(self.u8(rule + 1));
                let pre_len = i32::from(self.u8(rule + 2));
                let rep_len = i32::from(self.u8(rule + 3));
                self.match_elems = match_len;
                if match_len == 0 && !allow_insertion {
                    continue;
                }
                self.pattern_length = match_len + post_len;
                self.pattern = rule + 4;
                self.direction = 1;
                self.info_limit = match_len;
                for i in 0..self.info_limit.max(0) as usize {
                    self.info[i].start = 0;
                    self.info[i].limit = 0;
                }
                if !self.matches(0, 0, 0) {
                    continue;
                }
                if self.matched_length == 0 && !allow_insertion {
                    continue;
                }
                self.pattern += 4 * self.pattern_length as usize;
                self.pattern_length = pre_len;
                let mut ok = true;
                if pre_len > 0 {
                    self.direction = -1;
                    self.info_limit = 0;
                    self.match_elems = -1;
                    ok = self.matches(0, 0, -1);
                }
                if !ok {
                    continue;
                }
                let mut r = self.pattern + 4 * self.pattern_length as usize;
                for _ in 0..rep_len {
                    let ty = self.u8(r);
                    match ty {
                        REP_LITERAL => {
                            let v = self.u32(r);
                            self.out.push(v);
                        }
                        REP_CLASS => {
                            let mi = self.info[self.u8(r + 1) as usize];
                            if mi.start < mi.limit {
                                let m = self
                                    .rep_class_member(u32::from(self.u16(r + 2)), mi.class_index);
                                self.out.push(m);
                            }
                        }
                        REP_COPY => {
                            let mi = self.info[self.u8(r + 1) as usize];
                            for i in mi.start..mi.limit {
                                let c = self.input_char(i);
                                self.out.push(c);
                            }
                        }
                        REP_UNMAPPED => self.output_unmapped(in_char),
                        _ => {}
                    }
                    r += 4;
                }
                if self.matched_length > 0 {
                    matched = true;
                    break;
                }
                allow_insertion = false;
            }
            if !matched {
                self.output_unmapped(in_char);
                self.matched_length = 1;
            }
        } else if rule_type == LOOKUP_UNMAPPED {
            self.output_unmapped(in_char);
        } else if self.output_unicode {
            let usv = self.u32(lookup);
            if usv <= 0x0010_ffff {
                self.out.push(usv);
            }
        } else {
            let n = self.u8(lookup);
            for i in 0..n.min(3) as usize {
                let b = self.u8(lookup + 1 + i);
                self.out.push(u32::from(b));
            }
        }
        self.pos += self.matched_length.max(0) as usize;
    }
}
