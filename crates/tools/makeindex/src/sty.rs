//! Reading the style file (scanst.c).

use alloc::vec::Vec;

use crate::{
    ALPL, ALPU, ARAB, ARRAY_MAX, EOF, LFD, Mk, P, PAGETYPE_MAX, ROML, ROMU, SPC, STRING_MAX, Style,
    TAB, cstr, fmt, tolower,
};

const COMMENT: i32 = b'%' as i32;
const STR_DELIM: i32 = b'"' as i32;
const CHR_DELIM: i32 = b'\'' as i32;
const BSH: i32 = b'\\' as i32;

const ROMAN_LOWER_OFFSET: i32 = 10000;
const ROMAN_UPPER_OFFSET: i32 = 10000;
const ARABIC_OFFSET: i32 = 10000;
const ALPHA_LOWER_OFFSET: i32 = 26;
const ALPHA_UPPER_OFFSET: i32 = 26;

/// What a specifier sets.
#[derive(Clone, Copy)]
enum Attr {
    /// A string, and the line count it sets.
    Str(
        fn(&mut Style) -> &mut Vec<u8>,
        Option<fn(&mut Style) -> &mut i32>,
    ),
    Char(fn(&mut Style) -> &mut u8),
    HeadingsFlag,
    LineMax,
    IndentLength,
}

/// The specifiers, in scanst.c's order.
fn attr(spec: &[u8]) -> Option<Attr> {
    use Attr::{Char, Str};
    Some(match spec {
        b"preamble" => Str(|s| &mut s.preamble, Some(|s| &mut s.prelen)),
        b"postamble" => Str(|s| &mut s.postamble, Some(|s| &mut s.postlen)),
        b"group_skip" => Str(|s| &mut s.group_skip, Some(|s| &mut s.skiplen)),
        b"headings_flag" => Attr::HeadingsFlag,
        b"heading_prefix" => Str(|s| &mut s.heading_pre, Some(|s| &mut s.headprelen)),
        b"heading_suffix" => Str(|s| &mut s.heading_suf, Some(|s| &mut s.headsuflen)),
        b"symhead_positive" => Str(|s| &mut s.symhead_pos, None),
        b"symhead_negative" => Str(|s| &mut s.symhead_neg, None),
        b"numhead_positive" => Str(|s| &mut s.numhead_pos, None),
        b"numhead_negative" => Str(|s| &mut s.numhead_neg, None),
        b"setpage_prefix" => Str(|s| &mut s.setpage_open, Some(|s| &mut s.setpagelen)),
        b"setpage_suffix" => Str(|s| &mut s.setpage_close, Some(|s| &mut s.setpagelen)),
        b"item_0" => Str(|s| &mut s.item_r[0], Some(|s| &mut s.ilen_r[0])),
        b"item_1" => Str(|s| &mut s.item_r[1], Some(|s| &mut s.ilen_r[1])),
        b"item_2" => Str(|s| &mut s.item_r[2], Some(|s| &mut s.ilen_r[2])),
        b"item_01" => Str(|s| &mut s.item_u[1], Some(|s| &mut s.ilen_u[1])),
        b"item_12" => Str(|s| &mut s.item_u[2], Some(|s| &mut s.ilen_u[2])),
        b"item_x1" => Str(|s| &mut s.item_x[1], Some(|s| &mut s.ilen_x[1])),
        b"item_x2" => Str(|s| &mut s.item_x[2], Some(|s| &mut s.ilen_x[2])),
        b"encap_prefix" => Str(|s| &mut s.encap_p, None),
        b"encap_infix" => Str(|s| &mut s.encap_i, None),
        b"encap_suffix" => Str(|s| &mut s.encap_s, None),
        b"delim_0" => Str(|s| &mut s.delim_p[0], None),
        b"delim_1" => Str(|s| &mut s.delim_p[1], None),
        b"delim_2" => Str(|s| &mut s.delim_p[2], None),
        b"delim_n" => Str(|s| &mut s.delim_n, None),
        b"delim_r" => Str(|s| &mut s.delim_r, None),
        b"delim_t" => Str(|s| &mut s.delim_t, None),
        b"suffix_2p" => Str(|s| &mut s.suffix_2p, None),
        b"suffix_3p" => Str(|s| &mut s.suffix_3p, None),
        b"suffix_mp" => Str(|s| &mut s.suffix_mp, None),
        b"line_max" => Attr::LineMax,
        b"indent_length" => Attr::IndentLength,
        b"indent_space" => Str(|s| &mut s.indent_space, None),
        b"page_compositor" => Str(|s| &mut s.page_comp, None),
        b"page_precedence" => Str(|s| &mut s.page_prec, None),
        b"keyword" => Str(|s| &mut s.idx_keyword, None),
        b"arg_open" => Char(|s| &mut s.idx_aopen),
        b"arg_close" => Char(|s| &mut s.idx_aclose),
        b"level" => Char(|s| &mut s.idx_level),
        b"range_open" => Char(|s| &mut s.idx_ropen),
        b"range_close" => Char(|s| &mut s.idx_rclose),
        b"quote" => Char(|s| &mut s.idx_quote),
        b"actual" => Char(|s| &mut s.idx_actual),
        b"encap" => Char(|s| &mut s.idx_encap),
        b"escape" => Char(|s| &mut s.idx_escape),
        _ => return None,
    })
}

/// `count_lfd`.
fn count_lfd(s: &[u8]) -> i32 {
    cstr(s).iter().filter(|&&c| c == b'\n').count() as i32
}

/// The style scanner's counters (scanst.c's statics).
#[derive(Default)]
pub(crate) struct StyState {
    lc: i32,
    tc: i32,
    ec: i32,
    put_dot: bool,
    /// `scan_sty`'s `tmp`, which keeps a failed `fscanf`'s old value.
    tmp: i32,
}

impl Mk<'_> {
    /// `STY_ERROR`.
    fn sty_error(&mut self, ss: &mut StyState, f: &str, args: &[P]) {
        self.error_start();
        let name = self.sty_fn.clone();
        let head = fmt(
            "** Input style error (file = %s, line = %d):\n   -- ",
            &[P::S(&name), ss.lc.into()],
        );
        self.ilg_put(&head);
        let s = fmt(f, args);
        self.ilg_put(&s);
        ss.ec += 1;
        ss.put_dot = false;
    }

    /// `STY_SKIPLINE`.
    fn sty_skipline(&mut self, ss: &mut StyState) {
        loop {
            let a = self.sty.getc();
            if a == LFD || a == EOF {
                break;
            }
        }
        ss.lc += 1;
    }

    /// `next_nonblank`.
    fn next_nonblank(&mut self, ss: &mut StyState) -> i32 {
        loop {
            match self.sty.getc() {
                EOF => return -1,
                LFD => ss.lc += 1,
                SPC | TAB => {}
                c => return c,
            }
        }
    }

    /// `SCAN_NO`: `fscanf(sty_fp, "%d", n)` on the raw stream.
    fn scan_no_sty(&mut self, n: &mut i32) {
        let mut c = self.sty.raw();
        while c != EOF && (c as u8).is_ascii_whitespace() || c == 0x0b {
            c = self.sty.raw();
        }
        let neg = c == i32::from(b'-');
        if neg || c == i32::from(b'+') {
            c = self.sty.raw();
        }
        let mut v: i64 = 0;
        let mut digits = false;
        while c != EOF && (c as u8).is_ascii_digit() {
            v = v.saturating_mul(10).saturating_add(i64::from(c) - 48);
            digits = true;
            c = self.sty.raw();
        }
        self.sty.unget(c);
        if digits {
            *n = (if neg { -v } else { v }) as i32;
        }
    }

    /// `scan_spec`: `None` at the end, `Some(false)` on a too-long name.
    fn scan_spec(&mut self, ss: &mut StyState, spec: &mut Vec<u8>) -> Option<bool> {
        let mut c;
        loop {
            c = self.next_nonblank(ss);
            if c == -1 {
                return None;
            } else if c == COMMENT {
                self.sty_skipline(ss);
            } else {
                break;
            }
        }
        spec.clear();
        spec.push(tolower(c as u8));
        let mut i = 0;
        loop {
            i += 1;
            if i > STRING_MAX {
                break;
            }
            c = self.sty.getc();
            if c == SPC || c == TAB || c == LFD || c == EOF {
                break;
            }
            spec.push(tolower(c as u8));
        }
        if i < STRING_MAX {
            if c == EOF {
                let s = spec.clone();
                self.sty_error(
                    ss,
                    "No attribute for specifier %s (premature EOF)\n",
                    &[P::S(&s)],
                );
                // (scan_spec returns -1, which the loop takes as true)
                return Some(true);
            }
            if c == LFD {
                ss.lc += 1;
            }
            Some(true)
        } else {
            let s = spec.clone();
            self.sty_error(
                ss,
                "Specifier %s too long (max %d).\n",
                &[P::S(&s), (STRING_MAX as i32).into()],
            );
            None
        }
    }

    /// `scan_string`: `Some` with the new value.
    fn scan_string(&mut self, ss: &mut StyState) -> Option<Vec<u8>> {
        let mut clone: Vec<u8> = Vec::new();
        match self.next_nonblank(ss) {
            STR_DELIM => loop {
                match self.sty.getc() {
                    EOF => {
                        self.sty_error(ss, "No closing delimiter in %s.\n", &[P::S(cstr(&clone))]);
                        return None;
                    }
                    STR_DELIM => return Some(cstr(&clone).to_vec()),
                    BSH => match self.sty.getc() {
                        0x74 => clone.push(b'\t'),
                        0x6e => clone.push(b'\n'),
                        c => clone.push(c as u8),
                    },
                    c => {
                        if c == LFD {
                            ss.lc += 1;
                        }
                        if clone.len() < ARRAY_MAX {
                            clone.push(c as u8);
                        } else {
                            self.sty_skipline(ss);
                            self.sty_error(
                                ss,
                                "Attribute string %s too long (max %d).\n",
                                &[P::S(cstr(&clone)), (ARRAY_MAX as i32).into()],
                            );
                            return None;
                        }
                    }
                }
            },
            COMMENT => {
                self.sty_skipline(ss);
                None
            }
            _ => {
                self.sty_skipline(ss);
                self.sty_error(ss, "No opening delimiter.\n", &[]);
                None
            }
        }
    }

    /// `scan_char`.
    fn scan_char(&mut self, ss: &mut StyState) -> Option<u8> {
        match self.next_nonblank(ss) {
            CHR_DELIM => {
                let mut clone = self.sty.getc();
                match clone {
                    CHR_DELIM => {
                        self.sty_skipline(ss);
                        self.sty_error(ss, "Premature closing delimiter.\n", &[]);
                        return None;
                    }
                    LFD | EOF => {
                        if clone == LFD {
                            ss.lc += 1;
                        }
                        self.sty_error(ss, "No character (premature EOF).\n", &[]);
                        return None;
                    }
                    BSH => clone = self.sty.getc(),
                    _ => {}
                }
                if self.sty.getc() == CHR_DELIM {
                    Some(clone as u8)
                } else {
                    self.sty_error(ss, "No closing delimiter or too many letters.\n", &[]);
                    None
                }
            }
            COMMENT => {
                self.sty_skipline(ss);
                None
            }
            _ => {
                self.sty_skipline(ss);
                self.sty_error(ss, "No opening delimiter.\n", &[]);
                None
            }
        }
    }

    /// `scan_sty`.
    pub(crate) fn scan_sty(&mut self) {
        let mut ss = StyState::default();
        let name = self.sty_fn.clone();
        self.message("Scanning style file %s", &[P::S(&name)]);
        let mut spec = Vec::new();
        while self.scan_spec(&mut ss, &mut spec).is_some() {
            ss.tc += 1;
            ss.put_dot = true;
            match attr(&spec) {
                Some(Attr::Str(field, lines)) => {
                    if let Some(s) = self.scan_string(&mut ss) {
                        *field(&mut self.st) = s;
                    }
                    if let Some(lines) = lines {
                        let n = count_lfd(field(&mut self.st));
                        *lines(&mut self.st) = n;
                    }
                }
                Some(Attr::Char(field)) => {
                    if let Some(c) = self.scan_char(&mut ss) {
                        *field(&mut self.st) = c;
                    }
                }
                Some(Attr::HeadingsFlag) => {
                    let mut n = self.st.headings_flag;
                    self.scan_no_sty(&mut n);
                    self.st.headings_flag = n;
                }
                Some(Attr::LineMax) => {
                    let mut t = ss.tmp;
                    self.scan_no_sty(&mut t);
                    ss.tmp = t;
                    if t > 0 {
                        self.st.linemax = t;
                    } else {
                        self.sty_error(
                            &mut ss,
                            "%s must be positive (got %d)",
                            &["line_max".into(), t.into()],
                        );
                    }
                }
                Some(Attr::IndentLength) => {
                    let mut t = ss.tmp;
                    self.scan_no_sty(&mut t);
                    ss.tmp = t;
                    if t >= 0 {
                        self.st.indent_length = t;
                    } else {
                        self.sty_error(
                            &mut ss,
                            "%s must be nonnegative (got %d)",
                            &["indent_length".into(), t.into()],
                        );
                    }
                }
                None => {
                    self.next_nonblank(&mut ss);
                    self.sty_skipline(&mut ss);
                    let s = spec.clone();
                    self.sty_error(&mut ss, "Unknown specifier %s.\n", &[P::S(&s)]);
                    ss.put_dot = false;
                }
            }
            if ss.put_dot {
                self.idx_dot = true;
                if self.verbose {
                    self.out.stderr.push(b'.');
                }
                self.ilg_put(b".");
            }
        }
        self.process_precedence(&mut ss);
        if self.st.idx_quote == self.st.idx_escape {
            let q = self.st.idx_quote;
            self.sty_error(
                &mut ss,
                "Quote and escape symbols must be distinct (both `%c' now).\n",
                &[P::S(&[q])],
            );
            self.st.idx_quote = b'"';
            self.st.idx_escape = b'\\';
        }
        self.done(ss.tc - ss.ec, "attributes redefined", ss.ec, "ignored");
    }

    /// `process_precedence`: the page type offsets from `page_precedence`.
    fn process_precedence(&mut self, ss: &mut StyState) {
        let prec = cstr(&self.st.page_prec).to_vec();
        let mut seen = [false; 5];
        let mut i = 0;
        while i < PAGETYPE_MAX && i < prec.len() {
            let c = prec[i];
            let (slot, shown) = match c {
                b'r' => (0, b'r'),
                b'R' => (1, b'R'),
                b'n' => (2, b'n'),
                b'a' => (3, b'A'),
                b'A' => (4, b'A'),
                _ => {
                    self.sty_skipline(ss);
                    self.sty_error(
                        ss,
                        "Unknow type `%c' in page precedence specification.\n",
                        &[P::S(&[c])],
                    );
                    return;
                }
            };
            if seen[slot] {
                self.sty_skipline(ss);
                self.sty_error(
                    ss,
                    "Multiple instances of type `%c' in page precedence specification `%s'.\n",
                    &[P::S(&[shown]), P::S(&prec)],
                );
                return;
            }
            seen[slot] = true;
            i += 1;
        }
        if i < prec.len() {
            self.sty_skipline(ss);
            self.sty_error(ss, "Page precedence specification string too long.\n", &[]);
            return;
        }
        let mut last = i;
        let mut order = [0i32; PAGETYPE_MAX + 1];
        let mut typ = [0i32; PAGETYPE_MAX + 1];
        let step = |c: u8| match c {
            b'r' => (ROMAN_LOWER_OFFSET, ROML),
            b'R' => (ROMAN_UPPER_OFFSET, ROMU),
            b'n' => (ARABIC_OFFSET, ARAB),
            b'a' => (ALPHA_LOWER_OFFSET, ALPL),
            _ => (ALPHA_LOWER_OFFSET, ALPU),
        };
        if let Some(&c) = prec.first() {
            (order[0], typ[0]) = step(c);
        }
        for i in 1..last {
            let (o, t) = step(prec[i]);
            order[i] = order[i - 1] + o;
            typ[i] = t;
        }
        let off = &mut self.st.page_offset;
        *off = [-1; PAGETYPE_MAX];
        off[typ[0] as usize] = 0;
        for i in 1..last {
            off[typ[i] as usize] = order[i - 1];
        }
        for i in 0..PAGETYPE_MAX {
            if off[i] == -1 {
                let o = match typ[last.saturating_sub(1)] {
                    ROML => ROMAN_LOWER_OFFSET,
                    ROMU => ROMAN_UPPER_OFFSET,
                    ARAB => ARABIC_OFFSET,
                    ALPL => ALPHA_LOWER_OFFSET,
                    _ => ALPHA_UPPER_OFFSET,
                };
                order[last] = order[last.saturating_sub(1)] + o;
                typ[last] = i as i32;
                off[i] = order[last];
                last += 1;
            }
        }
    }
}
