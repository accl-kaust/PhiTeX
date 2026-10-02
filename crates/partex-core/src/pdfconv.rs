//! pdfTeX's expandable conversions (pdfTeX §496–§498): `\expanded`,
//! `\pdfstrcmp`, the escapes, file facts, digests and random deviates.
//!
//! pdfTeX builds each result in the string pool behind the text it was
//! given (`tokens_to_string`, then a C routine of `utils.c` or
//! `texmfmp.c`); here the text is taken out as bytes and each routine is
//! a function from bytes to bytes.

use alloc::vec::Vec;

use crate::host::Host;
use crate::print::NEW_STRING;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// pdfTeX's `\pdftexrevision` (pdfTeX §2: `pdftex_revision`).
const PDFTEX_REVISION: &[u8] = b"29";

/// The kpathsea version web2c's pdfTeX banner names.
const KPATHSEA_VERSION: &[u8] = b" kpathsea version 6.4.2";

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §496: the conversions that are pdfTeX's (`c` is at least
    /// `expanded_code`, and not `job_name_code`).
    pub(crate) fn pdftex_conv_toks(&mut self, c: i32) -> Result<(), Jump> {
        let out: Vec<u8> = match c {
            EXPANDED_CODE => {
                let saved = self.save_ext_scan();
                self.scan_toks(false, true)?;
                let p = self.take_def();
                self.restore_ext_scan(saved);
                return self.ins_list(p);
            }
            PDFTEX_REVISION_CODE => PDFTEX_REVISION.to_vec(),
            PDFTEX_BANNER_CODE => self.pdftex_banner(),
            PDF_FONT_SIZE_CODE => {
                self.scan_font_ident()?;
                if self.cur_val == NULL_FONT {
                    return self.pdf_error(b"font", b"invalid font identifier");
                }
                let f = crate::fonts::fx(self.cur_val);
                let size = self.fonts.metrics[f].size;
                self.printed(|t| {
                    t.print_scaled(size);
                    t.print_str(b"pt");
                })
            }
            PDF_COLORSTACK_INIT_CODE => {
                // pdfTeX §497
                let page = self.scan_keyword(b"page")?;
                let mode = if self.scan_keyword(b"direct")? {
                    crate::pdf::DIRECT_ALWAYS
                } else if self.scan_keyword(b"page")? {
                    crate::pdf::DIRECT_PAGE
                } else {
                    crate::pdf::SET_ORIGIN
                };
                let s = self.scan_ext_string()?;
                let stacks = crate::pdf::val::bit(crate::pdf::val::field::STACKS);
                let made = self.writer_scope(0, stacks, |t| t.pdf.stacks.new_stack(&s, mode, page));
                let n = if let Some(n) = made {
                    n
                } else {
                    self.print_err(b"Too many color stacks");
                    self.help(&[
                        b"The number of color stacks is limited to 32768.",
                        b"I'll use the default color stack 0 here.",
                    ]);
                    self.error()?;
                    0
                };
                self.printed(|t| t.print_int(n))
            }
            PDF_ESCAPE_STRING_CODE => escape_string(&self.scan_ext_string()?),
            PDF_ESCAPE_NAME_CODE => escape_name(&self.scan_ext_string()?),
            PDF_ESCAPE_HEX_CODE => escape_hex(&self.scan_ext_string()?),
            PDF_UNESCAPE_HEX_CODE => unescape_hex(&self.scan_ext_string()?),
            PDF_CREATION_DATE_CODE => {
                let d = self.host.creation_date();
                self.clock_read(crate::track::Query::Now, &d);
                d
            }
            PDF_FILE_MOD_DATE_CODE => {
                let name = file_name(&self.scan_ext_string()?);
                let d = self.host.file_mod_date(&name).unwrap_or_default();
                self.clock_read(crate::track::Query::ModDate(name.into()), &d);
                d
            }
            PDF_FILE_SIZE_CODE => {
                let name = file_name(&self.scan_ext_string()?);
                // (a load: a name the job stores reads its store, DESIGN 3.7)
                let found = self.read_source(&name, false);
                match found {
                    Some(f) => alloc::format!("{}", f.contents.len()).into_bytes(),
                    None => Vec::new(),
                }
            }
            PDF_MDFIVE_SUM_CODE => {
                let saved = self.save_ext_scan();
                let file = self.scan_keyword(b"file")?;
                self.scan_toks(false, true)?;
                let s = self.take_def_ref_string();
                self.restore_ext_scan(saved);
                let digest = if file {
                    let name = file_name(&s);
                    // (a load: a name the job stores reads its store)
                    let found = self.read_source(&name, false);
                    found.map(|f| partex_engine::md5::md5(&f.contents))
                } else {
                    Some(partex_engine::md5::md5(&s))
                };
                digest.map_or_else(Vec::new, |d| escape_hex(&d))
            }
            PDF_FILE_DUMP_CODE => self.pdf_file_dump()?,
            PDF_STRCMP_CODE => {
                // pdfTeX §1537: `compare_strings`
                let saved = self.save_ext_scan();
                let cs = self.cur_cs;
                self.scan_toks(false, true)?;
                let a = self.take_def_ref_string();
                self.cur_cs = cs;
                self.scan_toks(false, true)?;
                let b = self.take_def_ref_string();
                self.restore_ext_scan(saved);
                let v: i32 = match a.cmp(&b) {
                    core::cmp::Ordering::Less => -1,
                    core::cmp::Ordering::Equal => 0,
                    core::cmp::Ordering::Greater => 1,
                };
                self.printed(|t| t.print_int(v))
            }
            PDF_MATCH_CODE => {
                // pdfTeX §497; utils.c's `matchstrings`
                let saved = self.save_ext_scan();
                let icase = self.scan_keyword(b"icase")?;
                let mut count = -1;
                if self.scan_keyword(b"subcount")? {
                    self.scan_int()?;
                    count = self.cur_val;
                }
                self.scan_toks(false, true)?;
                let pattern = self.take_def_ref_string();
                self.scan_toks(false, true)?;
                let text = self.take_def_ref_string();
                self.restore_ext_scan(saved);
                match partex_engine::regex::Regex::new(c_string(&pattern), icase) {
                    Err(e) => {
                        let mut m = b"\\pdfmatch: ".to_vec();
                        m.extend_from_slice(e.message());
                        self.pdftex_warn(&m);
                        b"-1".to_vec()
                    }
                    Ok(re) => {
                        let text = c_string(&text).to_vec();
                        let count = if count < 0 { 10 } else { count };
                        let caps = re.exec(&text);
                        let r = if caps.is_some() { b"1" } else { b"0" };
                        *self.pdf.last_match = Some((text, count, caps));
                        self.writer_wrote(crate::pdf::val::field::LAST_MATCH);
                        r.to_vec()
                    }
                }
            }
            PDF_LAST_MATCH_CODE => {
                // pdfTeX §497; utils.c's `getmatch`
                self.scan_int()?;
                if self.cur_val < 0 {
                    self.print_err(b"Bad match number");
                    self.help(&[
                        b"Since I expected zero or a positive number,",
                        b"I changed this one to zero.",
                    ]);
                    self.int_error(self.cur_val)?;
                    self.cur_val = 0;
                }
                let i = self.cur_val;
                self.writer_read(crate::pdf::val::field::LAST_MATCH);
                let found = match &*self.pdf.last_match {
                    Some((text, count, Some(caps))) if i < *count => usize::try_from(i)
                        .ok()
                        .and_then(|i| caps.get(i).copied().flatten())
                        .map(|(a, b)| (a, text[a..b].to_vec())),
                    _ => None,
                };
                match found {
                    Some((a, bytes)) => {
                        let mut v = alloc::format!("{a}->").into_bytes();
                        v.extend_from_slice(&bytes);
                        v
                    }
                    None => b"-1->".to_vec(),
                }
            }
            UNIFORM_DEVIATE_CODE => {
                self.scan_int()?;
                let v = self.unif_rand(self.cur_val);
                self.printed(|t| t.print_int(v))
            }
            NORMAL_DEVIATE_CODE => {
                let v = self.norm_rand()?;
                self.printed(|t| t.print_int(v))
            }
            LEFT_MARGIN_KERN_CODE | RIGHT_MARGIN_KERN_CODE => {
                self.scan_register_num()?;
                let b = match self.box_reg(self.cur_val).cloned() {
                    Some(b) if !b.vertical => b,
                    _ => return self.pdf_error(b"marginkern", b"a non-empty hbox expected"),
                };
                let open = self.unsealed_box(&b);
                let list = open.as_ref().map_or(&b.list, |o| &o.list);
                let w = margin_kern_width(list, c == LEFT_MARGIN_KERN_CODE);
                self.printed(|t| {
                    match w {
                        Some(w) => t.print_scaled(w),
                        None => t.print_char(b'0'),
                    }
                    t.print_str(b"pt");
                })
            }
            _ => return self.pdf_error(b"conversion", b"not implemented in partex yet"),
        };
        let p = self.text_toks(&out);
        self.ins_list(p)
    }

    /// `pdftex_banner`.
    pub(crate) fn pdftex_banner(&self) -> Vec<u8> {
        let mut b = crate::files::PDFTEX_BANNER.to_vec();
        b.extend_from_slice(self.params.version_string);
        b.extend_from_slice(KPATHSEA_VERSION);
        b
    }

    /// What `f` prints, as bytes.
    pub(crate) fn printed(&mut self, f: impl FnOnce(&mut Self)) -> Vec<u8> {
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        let b = self.pool_ptr;
        f(self);
        self.set_selector(old_setting);
        let s = self.str_pool[b..self.pool_ptr].to_vec();
        self.pool_ptr = b;
        s
    }

    /// pdfTeX §686: `tokens_to_string` of `def_ref`, which is then
    /// dropped (`delete_token_ref`).
    pub(crate) fn take_def_ref_string(&mut self) -> Vec<u8> {
        let p = core::mem::take(&mut self.def_ref);
        self.printed(|t| t.token_show(&p))
    }

    /// The scanner state a general text argument changes.
    fn save_ext_scan(&mut self) -> (i32, i32, Vec<i32>) {
        (
            self.scanner_status,
            self.warning_index,
            core::mem::take(&mut self.def_ref),
        )
    }

    fn restore_ext_scan(&mut self, (status, warning, def_ref): (i32, i32, Vec<i32>)) {
        self.def_ref = def_ref;
        self.warning_index = warning;
        self.scanner_status = status;
    }

    /// pdfTeX §497: `scan_pdf_ext_toks` and `tokens_to_string`, with the
    /// scanner state kept.
    fn scan_ext_string(&mut self) -> Result<Vec<u8>, Jump> {
        let saved = self.save_ext_scan();
        self.scan_toks(false, true)?;
        let s = self.take_def_ref_string();
        self.restore_ext_scan(saved);
        Ok(s)
    }

    /// pdfTeX §497: `\pdffiledump [offset n] [length n] {file}`, and
    /// texmfmp.c's `getfiledump`.
    fn pdf_file_dump(&mut self) -> Result<Vec<u8>, Jump> {
        let saved = self.save_ext_scan();
        let scan = |t: &mut Self, key: &[u8], err: &'static [u8], help: [&'static [u8]; 2]| {
            t.cur_val = 0;
            if t.scan_keyword(key)? {
                t.scan_int()?;
                if t.cur_val < 0 {
                    t.print_err(err);
                    t.help(&help);
                    t.int_error(t.cur_val)?;
                    t.cur_val = 0;
                }
            }
            Ok::<i32, Jump>(t.cur_val)
        };
        let offset = scan(
            self,
            b"offset",
            b"Bad file offset",
            [
                b"A file offset must be between 0 and 2^{31}-1,",
                b"I changed this one to zero.",
            ],
        )?;
        let length = scan(
            self,
            b"length",
            b"Bad dump length",
            [
                b"A dump length must be between 0 and 2^{31}-1,",
                b"I changed this one to zero.",
            ],
        )?;
        self.scan_toks(false, true)?;
        let s = self.take_def_ref_string();
        self.restore_ext_scan(saved);
        if length == 0 {
            return Ok(Vec::new());
        }
        let name = file_name(&s);
        // (a load: a name the job stores reads its store, DESIGN 3.7)
        let found = self.read_source(&name, false);
        let Some(f) = found else {
            return Ok(Vec::new());
        };
        let from = usize::try_from(offset).unwrap_or(0);
        let to = from.saturating_add(usize::try_from(length).unwrap_or(0));
        Ok(escape_hex(
            f.contents
                .get(from..to.min(f.contents.len()))
                .unwrap_or(&[]),
        ))
    }

    /// pdfTeX §1587: `glyph_to_unicode` (`\pdfglyphtounicode{glyph}{code}`)
    /// and utils.c's `deftounicode`: what text glyph `glyph` stands for
    /// in the PDF's `ToUnicode` maps. `code` is hexadecimal: one code
    /// point, or a sequence when it has spaces.
    pub(crate) fn glyph_to_unicode(&mut self) -> Result<(), Jump> {
        let glyph = self.scan_ext_string()?;
        let text = self.scan_ext_string()?;
        let trimmed = text.trim_ascii_matches_spaces();
        let spaced = trimmed.contains(&b' ');
        let valid = trimmed.iter().all(|&c| c == b' ' || c.is_ascii_hexdigit());
        if trimmed.is_empty() || !valid || glyph.is_empty() || glyph == b".notdef" {
            let mut m = b"ToUnicode: invalid parameter(s): `".to_vec();
            m.extend_from_slice(&glyph);
            m.extend_from_slice(b"' => `");
            // (C prints the text from its first non-space on)
            let lead = text.iter().take_while(|&&c| c == b' ').count();
            m.extend_from_slice(&text[lead..]);
            m.push(b'\'');
            self.pdftex_warn(&m);
            return Ok(());
        }
        let entry = if spaced {
            ToUnicode::Text(trimmed.iter().copied().filter(|&c| c != b' ').collect())
        } else {
            // (`sscanf("%lX")`: the digits saturate at `ULONG_MAX`)
            let v = trimmed.iter().fold(0u64, |v, &c| {
                v.saturating_mul(16).saturating_add(u64::from(hex_value(c)))
            });
            if v > 0x10_FFFF {
                let m = alloc::format!("ToUnicode: value out of range [0,10FFFF]: {v:X}");
                self.pdftex_warn(m.as_bytes());
                ToUnicode::Undefined
            } else {
                ToUnicode::Code(u32::try_from(v).unwrap_or(0))
            }
        };
        // (a write of the table, which reads it)
        let w = crate::pdf::val::bit(crate::pdf::val::field::TOUNICODE);
        self.writer_scope(0, w, |t| t.tounicode.insert(glyph, entry));
        Ok(())
    }

    /// utils.c's `pdftex_warn`.
    pub(crate) fn pdftex_warn(&mut self, message: &[u8]) {
        self.pdftex_warn_in(None, message);
    }

    /// utils.c's `pdftex_warn` while reading file `file`
    /// (`cur_file_name`).
    pub(crate) fn pdftex_warn_in(&mut self, file: Option<&[u8]>, message: &[u8]) {
        self.print_ln();
        self.print_ln();
        self.print_str(b"pdfTeX warning: ");
        let name = self.params.invocation_name.clone();
        self.print_str(&name);
        if let Some(f) = file {
            self.print_str(b" (file ");
            self.print_str(f);
            self.print_str(b")");
        }
        self.print_str(b": ");
        self.print_str(message);
        self.print_ln();
    }

    /// pdfTeX §686: `pdf_warning`.
    pub(crate) fn pdf_warning(&mut self, t: &[u8], p: &[u8], prepend_nl: bool, append_nl: bool) {
        // (`wake_up_terminal` does nothing in web2c)
        if prepend_nl {
            self.print_ln();
        }
        self.print_str(b"pdfTeX warning");
        if !t.is_empty() {
            self.print_str(b" (");
            self.print_str(t);
            self.print_str(b")");
        }
        self.print_str(b": ");
        self.print_str(p);
        if append_nl {
            self.print_ln();
        }
        if self.history() == crate::error::SPOTLESS {
            self.set_history(crate::error::WARNING_ISSUED);
        }
    }

    /// pdfTeX §1537: `check_pdfoutput`: PDF-only commands in DVI mode.
    pub(crate) fn check_pdfoutput(&mut self, s: &[u8], is_error: bool) -> Result<(), Jump> {
        if self.int_par(PDF_OUTPUT_CODE) <= 0 {
            let m: &[u8] = if is_error {
                b"not allowed in DVI mode (\\pdfoutput <= 0)"
            } else {
                b"not allowed in DVI mode (\\pdfoutput <= 0); ignoring it"
            };
            if is_error {
                return self.pdf_error(s, m);
            }
            self.pdf_warning(s, m, true, true);
        }
        Ok(())
    }

    /// pdfTeX §688: `pdf_error`: a fatal error.
    pub(crate) fn pdf_error<R>(&mut self, t: &[u8], p: &[u8]) -> Result<R, Jump> {
        self.normalize_selector()?;
        self.print_err(b"pdfTeX error");
        if !t.is_empty() {
            self.print_str(b" (");
            self.print_str(t);
            self.print_str(b")");
        }
        self.print_str(b": ");
        self.print_str(p);
        self.succumb()
    }
}

/// `\pdfglyphtounicode`'s table: what each glyph name stands for (a
/// persistent map carrying its version: DESIGN 7.17.12's `tounicode`
/// row, the `TOUNICODE` field).
pub(crate) type ToUnicodeTable = crate::pdf::val::VMap<Vec<u8>, ToUnicode>;

/// What a glyph stands for in the PDF's `ToUnicode` maps (pdfTeX's
/// `glyph_unicode_entry`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ToUnicode {
    /// A code point.
    Code(u32),
    /// Hexadecimal digits of a sequence (`UNI_STRING`).
    Text(Vec<u8>),
    /// Out of range (`UNI_UNDEF`).
    Undefined,
}

partex_engine::persist_enum!(ToUnicode { Code(a0), Text(a0), Undefined });

fn hex_value(c: u8) -> u8 {
    unescapehex_digit(c).unwrap_or(0)
}

trait TrimSpaces {
    fn trim_ascii_matches_spaces(&self) -> &[u8];
}

impl TrimSpaces for [u8] {
    /// Without leading and trailing spaces (only `' '`).
    fn trim_ascii_matches_spaces(&self) -> &[u8] {
        let from = self.iter().take_while(|&&c| c == b' ').count();
        let to = self.len()
            - self[from..]
                .iter()
                .rev()
                .take_while(|&&c| c == b' ')
                .count();
        &self[from..to]
    }
}

/// texmfmp.c's `makecfilename`: the name without its quotes.
fn file_name(s: &[u8]) -> Vec<u8> {
    s.iter().copied().filter(|&c| c != b'"').collect()
}

/// utils.c's `escapestring`: a PDF string's text. Control characters,
/// spaces and bytes above `~` in octal; `(`, `)` and `\` escaped.
fn escape_string(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        if !(b'!'..=b'~').contains(&c) {
            out.extend_from_slice(alloc::format!("\\{c:03o}").as_bytes());
            continue;
        }
        if matches!(c, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(c);
    }
    out
}

/// utils.c's `escapename`: a PDF name (without its slash). NUL is
/// dropped; white space, delimiters, `#` and bytes above `~` become
/// `#XX`.
fn escape_name(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        match c {
            0 => {}
            1..=32
            | 127..=255
            | b'#'
            | b'%'
            | b'('
            | b')'
            | b'/'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}' => out.extend_from_slice(alloc::format!("#{c:02X}").as_bytes()),
            _ => out.push(c),
        }
    }
    out
}

/// utils.c's `escapehex`: two uppercase hex digits per byte.
fn escape_hex(s: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    s.iter()
        .flat_map(|&c| [HEX[usize::from(c >> 4)], HEX[usize::from(c & 15)]])
        .collect()
}

/// utils.c's `unescapehex`: bytes from hex digits; other characters are
/// ignored, and a last lone digit is the high half of a byte.
fn unescapehex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

fn unescape_hex(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2 + 1);
    let mut high = None;
    for d in s.iter().filter_map(|&c| unescapehex_digit(c)) {
        match high.take() {
            None => high = Some(d << 4),
            Some(h) => out.push(h + d),
        }
    }
    out.extend(high);
    out
}

/// `makecstring`: the bytes up to the first null.
pub(crate) fn c_string(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |n| &s[..n])
}

/// pdfTeX's `\leftmarginkern` (`left`) and `\rightmarginkern`: the
/// margin kern at that end of a box's list, past the items the margin
/// searches skip and `\leftskip` or `\rightskip`.
fn margin_kern_width(
    list: &[partex_engine::node::Node],
    left: bool,
) -> Option<crate::arith::Scaled> {
    use partex_engine::margin::{self, At};
    use partex_engine::node::Node;
    let skip = if left {
        LEFT_SKIP_CODE
    } else {
        RIGHT_SKIP_CODE
    };
    let mut p = if left {
        (!list.is_empty()).then(|| At::node(0))
    } else {
        margin::last(list)
    };
    while let Some(a) = p {
        let n = margin::get(list, a);
        let edge_skip = matches!(n, Node::Glue { subtype, .. } if i32::from(*subtype) == skip + 1);
        if !(margin::cp_skipable(n) || edge_skip) {
            break;
        }
        p = if left {
            margin::next(list, a)
        } else {
            margin::prev(list, a)
        };
    }
    match p.map(|a| margin::get(list, a)) {
        Some(Node::MarginKern { width, left: l, .. }) if *l == left => Some(*width),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_as_pdftex() {
        // (outputs of pdfTeX 1.40.29)
        assert_eq!(
            escape_string(b"a(b)\\ c\x01\xff"),
            b"a\\(b\\)\\\\\\040c\\001\\377"
        );
        assert_eq!(escape_name(b"a b#/\x00\x7f"), b"a#20b#23#2F#7F");
        assert_eq!(escape_hex(b"AZ\xff"), b"415AFF");
        assert_eq!(unescape_hex(b"414g2"), b"AB");
        assert_eq!(file_name(b"\"a b\".tex"), b"a b.tex");
    }
}
