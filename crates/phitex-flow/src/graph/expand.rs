//! Expansion: steps inside a node. A macro call and every expandable
//! primitive rewrite the input (`RState::pending`) and define nothing —
//! but `\csname` of an undefined name, which TeX defines as `\relax`
//! (tex.web 372), a definition of the node the step is in. A conditional
//! met at the top is a node of its own; one met while a command scans is a
//! step of that command's node. Every meaning, code and parameter read
//! goes through `Exec::get`, so the node's operands stay exact.

use std::fmt::Write as _;
use std::sync::Arc;

use super::scan::IVal;
use super::{Cond, Exec, Kind, Mode, Phi, Read, Src};
use crate::print;
use crate::val::{Conv, IfKind, IntPar, Key, Meaning, Prim, Tok, Val};

/// The job's name (`\jobname`): TeX's when it was given no file name.
pub const JOBNAME: &str = "texput";

/// pdfTeX's `\pdftexrevision` and `\pdftexbanner` (TeX Live 2026's, whose
/// `etex` is the oracle).
pub const PDFTEX_REVISION: &str = "29";
pub const PDFTEX_BANNER: &str =
    "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// A string's characters as pdfTeX's bytes (a character below 256 is its
/// byte; another, its UTF-8 bytes).
fn latin1(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match u8::try_from(u32::from(c)) {
            Ok(b) => out.push(b),
            Err(_) => out.extend_from_slice(c.to_string().as_bytes()),
        }
    }
    out
}

// (tex.web 489: `if_limit`)
const IF_CODE: u8 = 1;
const FI_CODE: u8 = 2;
const ELSE_CODE: u8 = 3;
const OR_CODE: u8 = 4;

/// What `\ifx` compares (tex.web 507): a meaning, or a `\noexpand`ed name
/// (`relax` with `no_expand_flag`, equal to no meaning).
#[derive(PartialEq)]
enum Cmp {
    M(Meaning),
    NoExp,
}

/// Whether a meaning is expanded rather than executed.
pub(super) fn expands(m: &Meaning) -> bool {
    match m {
        Meaning::Macro(_) | Meaning::Undefined => true,
        Meaning::Prim(p) => p.expandable(),
        _ => false,
    }
}

impl Exec<'_> {
    /// The next token with everything before it expanded, and its meaning
    /// (tex.web 380, `get_x_token`): `\relax` for a `\noexpand`ed name. At
    /// the top, a conditional is returned: it is a command, a node.
    pub(super) fn next_x_m(&mut self, top: bool) -> Option<(Tok, Meaning)> {
        loop {
            let t = self.next_raw()?;
            let m = self.tok_meaning(&t);
            if self.noexp && expands(&m) {
                return Some((t, Meaning::Prim(Prim::Relax)));
            }
            // (`\input` at the top is a node of its own, whose result is the
            // file: CORE.md)
            if top && matches!(m, Meaning::Prim(p) if p.conditional() || p == Prim::Input) {
                return Some((t, m));
            }
            if t.key().is_some() && expands(&m) {
                self.expand(&t, &m);
                if self.input.is_some() {
                    // (`\input`: this node ends the piece's run)
                    return None;
                }
                continue;
            }
            return Some((t, m));
        }
    }

    pub(super) fn next_x(&mut self) -> Option<Tok> {
        self.next_x_m(false).map(|(t, _)| t)
    }

    /// One expansion step of `t`, whose meaning is `m` (tex.web 366).
    pub(super) fn expand(&mut self, t: &Tok, m: &Meaning) {
        match m {
            Meaning::Macro(mac) => self.call(t, mac),
            Meaning::Undefined => self.err(format!("Undefined control sequence {t}")),
            Meaning::Prim(p) => match *p {
                Prim::If(_) | Prim::Fi | Prim::Else | Prim::Or | Prim::Unless => self.conditional(*p, t),
                Prim::ExpandAfter => self.expandafter(),
                Prim::NoExpand => {
                    // (tex.web 369: the token, marked not to be expanded once)
                    if let Some(x) = self.next_raw_normal() {
                        if x.key().is_some() {
                            self.rewrite(vec![Tok::DontExpand, x]);
                        } else {
                            self.back(x);
                        }
                    }
                }
                Prim::CsName => self.csname(),
                Prim::Convert(c) => self.convert(c),
                Prim::The => {
                    let toks = self.the_toks();
                    self.rewrite(toks);
                }
                Prim::Input if self.name_in_progress => {
                    // (tex.web 378: `insert_relax`)
                    self.back(t.clone());
                    self.back(Tok::Cs("\u{0}relax".into()));
                }
                Prim::Input => self.input_file(),
                Prim::EndInput => self.end_input(),
                // (`\splitfirstmark`, `\splitbotmark`, e-TeX's
                // `\splitfirstmarks n`, `\splitbotmarks n`: what the last
                // `\vsplit` defined, tex.web 386, etex.web §1493)
                Prim::Mark(k @ (3 | 4 | 8 | 9)) => {
                    let class = if k >= 5 { self.scan_eight_bit_int() } else { 0 };
                    if let Val::SplitMarks(m) = self.get(&Key::SplitMarks)
                        && let Ok(i) = m.binary_search_by_key(&class, |x| x.0)
                    {
                        let t = if k % 5 == 3 { &m[i].1 } else { &m[i].2 };
                        self.rewrite(t.to_vec());
                    }
                }
                // (e-TeX's: etex.web §1418, §1489)
                Prim::Unexpanded | Prim::Detokenize => {
                    let toks = self.etex_the(*p);
                    self.rewrite(toks);
                }
                Prim::ScanTokens => self.scan_tokens(),
                // (tex.web 386, etex.web §1493: `\topmark` and friends, `\topmarks
                // n` and friends: the mark the page builder (or `\vsplit`)
                // defined, a read)
                Prim::Mark(k) => {
                    let (which, class) = if k >= 5 { (k - 5, self.scan_eight_bit_int()) } else { (k, 0) };
                    if let Val::Toks(l) = self.get(&Key::Mark(which, class))
                        && !l.is_empty()
                    {
                        self.rewrite(l.to_vec());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// `\input` (tex.web 537) as a command of its own at a piece's top
    /// level (vertical mode, no group, box or paragraph open, no text
    /// rewritten): the file name, the file a read of `Key::File`; the node
    /// ends the piece's run, the file's pieces follow it in the sequence.
    /// Anywhere else the file is read in the piece ([`Exec::start_input`]).
    fn input_file(&mut self) {
        if !self.at_start || !self.st.plain_boundary() || !self.st.texts.is_empty() {
            self.start_input();
            return;
        }
        let name = self.scan_name();
        let Some((found, data)) = self.find_file(&name) else {
            self.err(format!("I can't find file `{name}'"));
            return;
        };
        let text: String = data.iter().map(|&b| char::from(b)).collect();
        self.input = Some(Arc::new(super::FileText { name: Arc::from(found), text: Arc::from(text) }));
    }

    /// `\expandafter` (tex.web 368).
    fn expandafter(&mut self) {
        let Some(t) = self.next_raw() else { return };
        if let Some(u) = self.next_raw() {
            let m = self.tok_meaning(&u);
            if !self.noexp && u.key().is_some() && expands(&m) {
                // (an `\input` expanded here is read in the piece: the token
                // put back after it is read first, tex.web 368)
                let was = std::mem::replace(&mut self.at_start, false);
                self.expand(&u, &m);
                self.at_start = was;
            } else if self.noexp {
                self.rewrite(vec![Tok::DontExpand, u]);
            } else {
                self.back(u);
            }
        }
        self.back(t);
    }

    /// `\csname ... \endcsname` (tex.web 372).
    fn csname(&mut self) {
        let mut name = String::new();
        self.in_csname += 1;
        loop {
            let Some((t, m)) = self.next_x_m(false) else {
                self.err("Missing \\endcsname inserted".into());
                break;
            };
            if let Tok::Char(_, c) = t {
                name.push(c);
            } else {
                if m != Meaning::Prim(Prim::EndCsName) {
                    self.err("Missing \\endcsname inserted".into());
                    self.back(t);
                }
                break;
            }
        }
        self.in_csname -= 1;
        let t = self.cs(&name);
        let key = t.key().expect("a name");
        if self.meaning(&key) == Meaning::Undefined {
            // (TeX's `eq_define`: local, whatever `\globaldefs` says)
            self.define(key, Val::Meaning(Meaning::Prim(Prim::Relax)), false);
        }
        self.back(t);
    }

    /// `\number`, `\romannumeral`, `\string`, `\meaning`, `\fontname`,
    /// `\jobname` (tex.web 470).
    fn convert(&mut self, c: Conv) {
        let mut s = String::new();
        match c {
            Conv::Number => {
                let v = self.scan_int();
                s.push_str(&v.to_string());
            }
            Conv::Roman => {
                let v = self.scan_int();
                print::roman(&mut s, v);
            }
            Conv::String => {
                if let Some(t) = self.next_raw_normal() {
                    match &t {
                        Tok::Char(_, c) | Tok::Active(c) => s.push(*c),
                        Tok::Cs(n) => self.sprint_cs(&mut s, n),
                        Tok::Param(..) | Tok::DontExpand => {}
                    }
                }
            }
            Conv::Meaning => {
                if let Some(t) = self.next_raw_normal() {
                    let m = if self.noexp && expands(&self.tok_meaning(&t)) {
                        Meaning::Prim(Prim::Relax)
                    } else {
                        self.tok_meaning(&t)
                    };
                    self.meaning_text(&mut s, &m);
                }
            }
            Conv::FontName => self.font_name_text(&mut s),
            Conv::JobName => match self.get(&Key::Job) {
                Val::Name(n) => s.push_str(&n),
                _ => s.push_str(JOBNAME),
            },
            Conv::ETeXRevision => s.push_str(super::etex::ETEX_REVISION),
            Conv::Expanded => {
                // (pdftex.web: `scan_toks(false, true)`, then the list is
                // the input)
                let toks = self.scan_toks_x();
                self.rewrite(toks);
                return;
            }
            Conv::PdfStrcmp => {
                let a = self.pdf_string();
                let b = self.pdf_string();
                s.push_str(match a.as_bytes().cmp(b.as_bytes()) {
                    std::cmp::Ordering::Less => "-1",
                    std::cmp::Ordering::Equal => "0",
                    std::cmp::Ordering::Greater => "1",
                });
            }
            Conv::PdfRevision => s.push_str(PDFTEX_REVISION),
            Conv::PdfBanner => s.push_str(PDFTEX_BANNER),
            Conv::PdfFilesize => {
                let name = self.pdf_string();
                if let Some(n) = self.file_size(&name) {
                    s.push_str(&n.to_string());
                }
            }
            Conv::PdfEscapeString => {
                let a = self.pdf_string();
                for b in latin1(&a) {
                    match b {
                        b'(' | b')' | b'\\' => {
                            s.push('\\');
                            s.push(char::from(b));
                        }
                        0..=32 | 127..=255 => {
                            let _ = write!(s, "\\{b:03o}");
                        }
                        _ => s.push(char::from(b)),
                    }
                }
            }
            Conv::PdfEscapeName => {
                let a = self.pdf_string();
                for b in latin1(&a) {
                    match b {
                        0..=32 | 127..=255 | b'#' | b'%' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' => {
                            let _ = write!(s, "#{b:02X}");
                        }
                        _ => s.push(char::from(b)),
                    }
                }
            }
            Conv::PdfEscapeHex => {
                let a = self.pdf_string();
                for b in latin1(&a) {
                    let _ = write!(s, "{b:02X}");
                }
            }
            Conv::PdfUnescapeHex => {
                let a = self.pdf_string();
                let digits: Vec<u8> = a.chars().filter_map(|c| c.to_digit(16)).map(|d| d as u8).collect();
                for pair in digits.chunks(2) {
                    let b = (pair[0] << 4) | pair.get(1).copied().unwrap_or(0);
                    s.push(char::from(b));
                }
            }
        }
        self.rewrite(print::str_toks(&s));
    }

    /// pdfTeX's `scan_pdf_ext_toks`, as a string: a general text, expanded
    /// (`\protected` macros kept), shown as `\message` shows it.
    fn pdf_string(&mut self) -> String {
        let toks = self.scan_toks_x();
        let mut s = String::new();
        self.show_toks(&mut s, &toks);
        s
    }

    // ------------------------------------------------------------ printing

    /// The escape character, then `s` (tex.web 63).
    pub(super) fn print_esc(&mut self, out: &mut String, s: &str) {
        let e = self.int_par(IntPar::Escapechar);
        if let Some(c) = u8::try_from(e).ok().map(char::from) {
            out.push(c);
        }
        out.push_str(s);
    }

    /// A control sequence as a token list shows it (tex.web 262): a
    /// multiletter name and a one-letter name are followed by a space.
    fn print_cs(&mut self, out: &mut String, n: &str) {
        let mut it = n.chars();
        match (it.next(), it.next()) {
            (None, _) => {
                self.print_esc(out, "csname");
                self.print_esc(out, "endcsname");
                out.push(' ');
            }
            (Some(c), None) => {
                self.print_esc(out, n);
                if self.catcode(c) == 11 {
                    out.push(' ');
                }
            }
            _ => {
                self.print_esc(out, n);
                out.push(' ');
            }
        }
    }

    /// A control sequence as `\string` shows it (tex.web 263).
    pub(super) fn sprint_cs(&mut self, out: &mut String, n: &str) {
        if n.is_empty() {
            self.print_esc(out, "csname");
            self.print_esc(out, "endcsname");
        } else {
            self.print_esc(out, n);
        }
    }

    /// A token list as TeX shows it (tex.web 292-294).
    pub(super) fn show_toks(&mut self, out: &mut String, toks: &[Tok]) {
        let mut match_chr = '#';
        self.show_toks_m(out, toks, &mut match_chr, false);
    }

    /// [`Exec::show_toks`] of a macro's parameter text (`in_params`) or
    /// body, with `match_chr` carried from the one to the other (tex.web
    /// 294).
    fn show_toks_m(&mut self, out: &mut String, toks: &[Tok], match_chr: &mut char, in_params: bool) {
        for t in toks {
            match t {
                Tok::Cs(n) => self.print_cs(out, n),
                Tok::Char(6, c) => {
                    out.push(*c);
                    out.push(*c);
                }
                Tok::Char(_, c) | Tok::Active(c) => out.push(*c),
                Tok::Param(n, c) => {
                    out.push(if in_params { *c } else { *match_chr });
                    if in_params {
                        *match_chr = *c;
                    }
                    out.push(char::from(b'0' + n));
                }
                Tok::DontExpand => {}
            }
        }
    }

    /// A meaning as `\meaning` shows it (tex.web 296, 298, 1295).
    pub(super) fn meaning_text(&mut self, out: &mut String, m: &Meaning) {
        match m {
            Meaning::Undefined => out.push_str("undefined"),
            Meaning::Prim(p) => self.print_esc(out, p.name()),
            Meaning::Char(cat, c) => {
                out.push_str(match cat {
                    1 => "begin-group character ",
                    2 => "end-group character ",
                    3 => "math shift character ",
                    4 => "alignment tab character ",
                    6 => "macro parameter character ",
                    7 => "superscript character ",
                    8 => "subscript character ",
                    10 => "blank space ",
                    11 => "the letter ",
                    _ => "the character ",
                });
                out.push(*c);
            }
            Meaning::CharGiven(c) => {
                self.print_esc(out, "char");
                print::hex(out, *c as i32);
            }
            Meaning::MathCharGiven(c) => {
                self.print_esc(out, "mathchar");
                print::hex(out, *c as i32);
            }
            Meaning::CountRef(n) => self.print_esc(out, &format!("count{n}")),
            Meaning::DimenRef(n) => self.print_esc(out, &format!("dimen{n}")),
            Meaning::SkipRef(n) => self.print_esc(out, &format!("skip{n}")),
            Meaning::MuskipRef(n) => self.print_esc(out, &format!("muskip{n}")),
            Meaning::ToksRef(n) => self.print_esc(out, &format!("toks{n}")),
            Meaning::Font(f) => {
                out.push_str("select font ");
                out.push_str(&f.name);
                if f.size != f.design_size {
                    out.push_str(" at ");
                    print::scaled(out, f.size);
                    out.push_str("pt");
                }
            }
            Meaning::Macro(mac) => {
                if mac.protected {
                    self.print_esc(out, "protected");
                }
                if mac.long {
                    self.print_esc(out, "long");
                }
                if mac.outer {
                    self.print_esc(out, "outer");
                }
                if mac.long || mac.outer || mac.protected {
                    out.push_str(" macro:");
                } else {
                    out.push_str("macro:");
                }
                let mac = mac.clone();
                let mut match_chr = '#';
                self.show_toks_m(out, &mac.params, &mut match_chr, true);
                out.push_str("->");
                self.show_toks_m(out, &mac.body, &mut match_chr, false);
            }
        }
    }

    // ------------------------------------------------------------ conditionals

    /// A conditional primitive: as a command node at the top, or a step
    /// inside another command's scanning (tex.web 498, 510).
    pub(super) fn conditional(&mut self, p: Prim, t: &Tok) {
        match p {
            Prim::If(k) => self.begin_if(k, false),
            Prim::Unless => {
                // (etex.web §1449: before a boolean conditional, negates it)
                let Some(u) = self.next_raw() else { return };
                match self.tok_meaning(&u) {
                    Meaning::Prim(Prim::If(k)) if k != IfKind::Case && !self.noexp => self.begin_if(k, true),
                    _ => {
                        self.err(format!("You can't use `\\unless' before `{u}'"));
                        if self.noexp {
                            self.rewrite(vec![Tok::DontExpand, u]);
                        } else {
                            self.back(u);
                        }
                    }
                }
            }
            Prim::Fi | Prim::Else | Prim::Or => {
                let code = match p {
                    Prim::Fi => FI_CODE,
                    Prim::Else => ELSE_CODE,
                    _ => OR_CODE,
                };
                let limit = self.st.conds.last().map_or(0, |c| c.limit);
                if code > limit {
                    if limit == IF_CODE {
                        // (tex.web 510: the test is still being read)
                        let relax = self.cs("relax");
                        self.rewrite(vec![relax, t.clone()]);
                    } else {
                        self.err(format!("Extra \\{}", p.name()));
                    }
                    return;
                }
                let mut c = code;
                while c != FI_CODE {
                    c = self.pass_text();
                    if c == 0 {
                        return;
                    }
                }
                self.pop_cond();
            }
            _ => {}
        }
    }

    /// An `\if...`: its test, then the taken arm is read and the other is
    /// skipped (tex.web 498); after e-TeX's `\unless`, the test negated.
    fn begin_if(&mut self, k: IfKind, unless: bool) {
        let here = self.chain.nodes.len() as u32;
        let code = k.code() + if unless { 32 } else { 0 };
        self.st.conds.push(Cond { open: here, limit: IF_CODE, test: false, code });
        let depth = self.st.conds.len();
        if k == IfKind::Case {
            let mut n = self.scan_int();
            while n != 0 {
                let c = self.pass_text();
                if c == 0 {
                    return;
                }
                if self.st.conds.len() == depth {
                    if c == OR_CODE {
                        n -= 1;
                    } else {
                        self.common_ending(c, false);
                        return;
                    }
                } else if c == FI_CODE {
                    self.st.conds.pop();
                }
            }
            self.set_cond(depth, OR_CODE, true);
            self.cond_kind(Kind::If);
            self.phi = Some(Box::new(Phi { test: true, keys: Vec::new() }));
            return;
        }
        let b = self.test(k) != unless;
        if b {
            self.set_cond(depth, ELSE_CODE, true);
            self.cond_kind(Kind::If);
            self.phi = Some(Box::new(Phi { test: true, keys: Vec::new() }));
            return;
        }
        // (skip to `\else` or `\fi`)
        loop {
            let c = self.pass_text();
            if c == 0 {
                return;
            }
            if self.st.conds.len() == depth {
                if c != OR_CODE {
                    self.common_ending(c, false);
                    return;
                }
                self.err("Extra \\or".into());
            } else if c == FI_CODE {
                self.st.conds.pop();
            }
        }
    }

    /// After a skip stopped at `\else` (the else arm is read) or `\fi`.
    fn common_ending(&mut self, c: u8, test: bool) {
        if c == FI_CODE {
            self.st.conds.pop();
            self.cond_kind(Kind::IfClosed);
        } else {
            let depth = self.st.conds.len();
            self.set_cond(depth, FI_CODE, test);
            self.cond_kind(Kind::If);
        }
        self.phi = Some(Box::new(Phi { test, keys: Vec::new() }));
    }

    fn set_cond(&mut self, depth: usize, limit: u8, test: bool) {
        if let Some(c) = self.st.conds.get_mut(depth - 1) {
            c.limit = limit;
            c.test = test;
        }
    }

    /// The test of a boolean conditional (tex.web 501-507).
    fn test(&mut self, k: IfKind) -> bool {
        match k {
            IfKind::Char | IfKind::Cat => {
                let (m1, c1) = self.if_operand();
                let (m2, c2) = self.if_operand();
                if k == IfKind::Char { c1 == c2 } else { m1 == m2 }
            }
            IfKind::Num | IfKind::Dim | IfKind::AbsNum | IfKind::AbsDim => {
                let num = matches!(k, IfKind::Num | IfKind::AbsNum);
                // (pdfTeX's `\ifpdfabsnum`, `\ifpdfabsdim`: the absolute values)
                let abs = matches!(k, IfKind::AbsNum | IfKind::AbsDim);
                let a = if num { self.scan_int() } else { self.scan_dimen() };
                let r = loop {
                    match self.next_x_m(false) {
                        Some((t, _)) if t.cat() == Some(10) => {}
                        Some((Tok::Char(12, c @ ('<' | '=' | '>')), _)) => break c,
                        other => {
                            self.err(format!(
                                "Missing = inserted for \\{}",
                                match k {
                                    IfKind::Num => "ifnum",
                                    IfKind::Dim => "ifdim",
                                    IfKind::AbsNum => "ifpdfabsnum",
                                    _ => "ifpdfabsdim",
                                }
                            ));
                            if let Some((t, _)) = other {
                                self.back(t);
                            }
                            break '=';
                        }
                    }
                };
                let b = if num { self.scan_int() } else { self.scan_dimen() };
                let (a, b) = if abs { (a.wrapping_abs(), b.wrapping_abs()) } else { (a, b) };
                match r {
                    '<' => a < b,
                    '>' => a > b,
                    _ => a == b,
                }
            }
            IfKind::Odd => self.scan_int() % 2 != 0,
            IfKind::Vmode => !self.in_write && self.st.mode == Mode::V,
            IfKind::Hmode => !self.in_write && self.st.mode == Mode::H,
            IfKind::Inner => !self.in_write && self.st.inner,
            IfKind::Mmode => !self.in_write && self.st.mode == Mode::M,
            IfKind::False => false,
            IfKind::Void | IfKind::Hbox | IfKind::Vbox => {
                let n = self.scan_eight_bit_int();
                match self.get(&Key::Box(n)) {
                    Val::Box(b) => (k == IfKind::Hbox && !b.vertical) || (k == IfKind::Vbox && b.vertical),
                    _ => k == IfKind::Void,
                }
            }
            IfKind::X => {
                let a = self.ifx_operand();
                let b = self.ifx_operand();
                a == b
            }
            IfKind::Eof => self.if_eof(),
            IfKind::True => true,
            IfKind::Case => unreachable!("\\ifcase has no boolean test"),
            IfKind::Defined => {
                // (etex.web §1450: `get_next`; a `\noexpand`ed name means
                // `\relax`)
                let Some(t) = self.next_raw_normal() else { return false };
                let m = self.tok_meaning(&t);
                (self.noexp && expands(&m)) || m != Meaning::Undefined
            }
            IfKind::Csname => self.if_csname(),
            // (pdfTeX: within `\csname`…`\endcsname` or `\ifcsname`'s name)
            IfKind::InCsname => self.in_csname > 0,
            IfKind::PdfPrimitive => {
                // (pdftex.web: `get_next`; the name's primitive, still its
                // meaning)
                let Some(t) = self.next_raw() else { return false };
                let Tok::Cs(n) = &t else { return false };
                let etex = self.int(&Key::ETeXMode) != 0;
                let p = Prim::by_name(n)
                    .or_else(|| if etex { Prim::by_name_etex(n).or_else(|| Prim::by_name_pdftex(n)) } else { None });
                p.is_some_and(|p| self.tok_meaning(&t) == Meaning::Prim(p))
            }
            IfKind::FontChar => {
                // (etex.web §1452)
                let f = self.scan_font_ident();
                let c = self.scan_char_num();
                f.glyph(c).is_some()
            }
        }
    }

    /// `get_x_token_or_active_char` and the (cmd, chr) `\if`/`\ifcat`
    /// compare (tex.web 506).
    fn if_operand(&mut self) -> (u32, u32) {
        let Some((t, m)) = self.next_x_m(false) else {
            return (0, 256);
        };
        if let (true, Tok::Active(c)) = (self.noexp, &t) {
            return (13, u32::from(*c));
        }
        match m {
            Meaning::Char(cat, c) if u32::from(c) < 256 => (u32::from(cat), u32::from(c)),
            Meaning::Prim(Prim::IgnoreSpaces) => (9, 0),
            _ => (0, 256),
        }
    }

    fn ifx_operand(&mut self) -> Cmp {
        let Some(t) = self.next_raw_normal() else {
            return Cmp::M(Meaning::Undefined);
        };
        let m = self.tok_meaning(&t);
        if self.noexp && expands(&m) { Cmp::NoExp } else { Cmp::M(m) }
    }

    /// Skips text to the next `\fi`, `\else` or `\or` at this level
    /// (tex.web 494); returns which (0: the piece ended).
    fn pass_text(&mut self) -> u8 {
        self.scanning(super::Scanning::Skipping, None, Self::pass_text_in)
    }

    fn pass_text_in(&mut self) -> u8 {
        let mut l = 0usize;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("Incomplete \\if; all text was ignored after the test".into());
                return 0;
            };
            if self.noexp {
                continue;
            }
            let Some(k) = t.key() else { continue };
            match self.meaning(&k) {
                Meaning::Prim(Prim::If(_)) => l += 1,
                Meaning::Prim(p @ (Prim::Fi | Prim::Else | Prim::Or)) => {
                    if l == 0 {
                        return match p {
                            Prim::Fi => FI_CODE,
                            Prim::Else => ELSE_CODE,
                            _ => OR_CODE,
                        };
                    }
                    if p == Prim::Fi {
                        l -= 1;
                    }
                }
                _ => {}
            }
        }
    }

    /// The end of the innermost conditional: a φ for each key its taken
    /// arm defined, over the nodes since the one that opened it.
    fn pop_cond(&mut self) {
        let Some(c) = self.st.conds.pop() else { return };
        self.cond_kind(Kind::EndIf);
        let nodes = &self.chain.nodes;
        let here = nodes.len();
        let at = c.open as usize;
        let depth = u16::try_from(self.st.conds.len()).unwrap_or(u16::MAX);
        if at >= here {
            // (opened and closed within this node: no arm of nodes)
            if self.phi.is_none() {
                self.phi = Some(Box::new(Phi { test: c.test, keys: Vec::new() }));
            }
            return;
        }
        let mut keys: Vec<Key> = Vec::new();
        for n in nodes.range(at, here) {
            for d in n.defs() {
                if !keys.contains(&d.key) {
                    keys.push(d.key.clone());
                }
            }
        }
        let mut phis = Vec::new();
        for k in keys {
            let Some((d, after)) = self.chain.resolve(&k, here) else {
                continue;
            };
            let scope = self.chain.scope_of(d, &k);
            self.reads.push(Arc::new(Read {
                key: k.clone(),
                src: Src::Node(d),
                val: after.clone(),
                scope,
                cond: None,
                field: super::Field::Whole,
            }));
            // (as of the `\if`: the conditional's depth in the node's input
            // state, the same as below the ones it opened and closed)
            let before = super::before_read(self.chain, self.env, self.outside, &k, at, depth);
            let before_val = before.val.clone();
            self.reads.push(before);
            // (the φ is a definition: readers after the `\fi` read it)
            self.defs.push(super::Def { key: k.clone(), val: after.clone(), global: false, level: None });
            phis.push((k, after, before_val));
        }
        match &mut self.phi {
            Some(p) if !p.keys.is_empty() => p.keys.extend(phis),
            _ => self.phi = Some(Box::new(Phi { test: c.test, keys: phis })),
        }
    }

    // ------------------------------------------------------------ \the

    /// `\the`: the tokens of an internal quantity (tex.web 465).
    pub(super) fn the_toks(&mut self) -> Vec<Tok> {
        let Some((t, m)) = self.next_x_m(false) else {
            return Vec::new();
        };
        let Some(v) = self.internal(&t, &m, super::scan::Lv::Tok) else {
            self.err(format!("You can't use `{t}' after \\the"));
            return print::str_toks("0");
        };
        let s = match v {
            IVal::Toks(l) => return l.to_vec(),
            IVal::Font(f) => return vec![self.font_ident_tok(&f)],
            IVal::Int(i) => i.to_string(),
            IVal::Dimen(d) => {
                let mut s = String::new();
                print::scaled(&mut s, d);
                s.push_str("pt");
                s
            }
            IVal::Glue(g) => print::spec(&g, "pt"),
            IVal::Mu(g) => print::spec(&g, "mu"),
        };
        print::str_toks(&s)
    }

    /// A balanced text, expanded as `\edef` and `\message` expand it
    /// (tex.web 477-479): macros and expandable primitives are expanded,
    /// `\the`'s tokens are not, a `\noexpand`ed name is kept. With
    /// `params` (a definition's body with that many parameters), `#n` is
    /// a parameter. Reads to the closing brace; the opening one has been
    /// read.
    pub(super) fn expand_text(&mut self, params: Option<u8>) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut unbalance = 1;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning text".into());
                return out;
            };
            if !self.noexp && t.key().is_some() {
                let m = self.tok_meaning(&t);
                // (etex.web §1417: a `\protected` macro is not expanded here)
                if expands(&m) && !matches!(&m, Meaning::Macro(mac) if mac.protected) {
                    match m {
                        Meaning::Prim(Prim::The) => {
                            let toks = self.the_toks();
                            out.extend(toks);
                        }
                        Meaning::Prim(p @ (Prim::Unexpanded | Prim::Detokenize)) => {
                            let toks = self.etex_the(p);
                            out.extend(toks);
                        }
                        _ => self.expand(&t, &m),
                    }
                    continue;
                }
            }
            match t.cat() {
                Some(1) => unbalance += 1,
                Some(2) => {
                    unbalance -= 1;
                    if unbalance == 0 {
                        return out;
                    }
                }
                Some(6) if params.is_some() => {
                    let n = params.unwrap_or(0);
                    match self.next_x() {
                        Some(x) if x.cat() == Some(6) => out.push(x),
                        Some(Tok::Char(12, d)) if d.is_ascii_digit() && d != '0' && (d as u8 - b'0') <= n => {
                            // (an `out_param` token: no character of its own, tex.web 476)
                            out.push(Tok::Param(d as u8 - b'0', '#'));
                        }
                        other => {
                            self.err("Illegal parameter number in definition".into());
                            if let Some(x) = other {
                                self.back(x);
                            }
                            out.push(t);
                        }
                    }
                    continue;
                }
                _ => {}
            }
            out.push(t);
        }
    }
}
