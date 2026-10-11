//! B3: expandable primitives: `\expandafter` `\noexpand` `\csname`
//! `\endcsname` `\the` `\number` `\romannumeral` `\string` `\meaning`
//! `\fontname` `\jobname` `\unexpanded` `\detokenize` `\marks` reads, pdfTeX's
//! `\expanded` `\pdfstrcmp` … , `\uppercase`/`\lowercase`.
//!
//! Also the printing helpers others use: `print_meaning`, `show_toks`
//! (`show_token_list` with `\escapechar` rules), `print_scaled`, `print_spec`,
//! `str_toks`.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::diag::error;
use crate::prim::{registry, tag, Kind, Level, Registry};
use crate::scan::{self, get_tok, ids};
use crate::term::TokKind;
use crate::tok::{Cs, Tok, Toks};
use crate::val::{FontId, Glue, Key, Meaning, Val};

/// pdfTeX 1.40.28 (TeX Live 2026).
pub const PDFTEX_REVISION: &str = "28";
pub const PDFTEX_BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.28 (TeX Live 2026)";
pub const ETEX_REVISION: &str = ".6";
/// `Key::Given` index of the creation date (`D:YYYYMMDDHHmmSS…`, `Val::Name`).
pub const GIVEN_CREATION_DATE: u8 = 3;

// ---------------------------------------------------------------- expansion

/// Whether a meaning expands (macros, expandable primitives, conditionals,
/// undefined names).
pub fn is_expandable(m: &Meaning) -> bool {
    match m {
        Meaning::Macro(_) | Meaning::Undefined => true,
        Meaning::Prim(id) => matches!(registry().get(*id).kind, Kind::Expand(_) | Kind::If(_)),
        _ => false,
    }
}

/// `expand` (tex.web 366) for one token: an expandable token is expanded,
/// anything else is put back.
pub fn expand(c: &mut dyn Ctx, t: Tok) {
    if !matches!(t, Tok::Cs(_) | Tok::Active(_)) {
        c.back(t);
        return;
    }
    match c.meaning(&t) {
        Meaning::Macro(f) => crate::macros::expand_macro(c, t, &f),
        Meaning::Prim(id) => {
            let d = *registry().get(id);
            match d.kind {
                Kind::Expand(f) => f(c, d.code),
                Kind::If(_) => crate::cond::begin_if(c, id, d.code),
                _ => c.back(t),
            }
        }
        Meaning::Undefined => undefined(c, t),
        _ => c.back(t),
    }
}

/// An undefined control sequence was expanded.
pub fn undefined(c: &mut dyn Ctx, _t: Tok) {
    error(
        c,
        "Undefined control sequence",
        &[
            "The control sequence at the end of the top line",
            "of your error message was never \\def'ed. If you have",
            "misspelled it (e.g., `\\hobx'), type `I' and the correct",
            "spelling (e.g., `I\\hbox'). Otherwise just continue,",
            "and I'll forget about whatever was undefined.",
        ],
    );
}

/// The next token with its `\noexpand` mark, if any.
fn get_marked(c: &mut dyn Ctx) -> Option<(Tok, bool)> {
    let t = c.next()?;
    if t == Tok::DontExpand {
        return c.next().map(|u| (u, true));
    }
    Some((t, false))
}

fn back_marked(c: &mut dyn Ctx, t: Tok, marked: bool) {
    if marked {
        c.push(Arc::from(vec![Tok::DontExpand, t]), TokKind::Backed);
    } else {
        c.back(t);
    }
}

fn expandafter(c: &mut dyn Ctx, _: u16) {
    let Some((t, tm)) = get_marked(c) else { return };
    match get_marked(c) {
        Some((u, false)) => expand(c, u),
        Some((u, true)) => back_marked(c, u, true),
        None => {}
    }
    back_marked(c, t, tm);
}

fn noexpand(c: &mut dyn Ctx, _: u16) {
    let Some((t, _)) = get_marked(c) else { return };
    match t {
        Tok::Cs(_) | Tok::Active(_) => back_marked(c, t, true),
        _ => c.back(t),
    }
}

fn csname(c: &mut dyn Ctx, _: u16) {
    let mut name = String::new();
    let last = loop {
        match c.next_x() {
            Some(Tok::Char { ch, .. }) => name.push(ch),
            other => break other,
        }
    };
    let ok = match last {
        Some(t) => scan::is_prim(c, &t, ids().endcsname),
        None => false,
    };
    if !ok {
        if let Some(t) = last {
            c.back(t);
        }
        let mut e = String::new();
        print_esc(c, &mut e, "endcsname");
        error(
            c,
            &format!("Missing {e} inserted"),
            &["The control sequence marked <to be read again> should", "not appear between \\csname and \\endcsname."],
        );
    }
    let cs = c.names().intern(&name);
    let t = Tok::Cs(cs);
    if c.meaning(&t) == Meaning::Undefined {
        if let Some(relax) = ids().relax {
            c.define(Key::Cs(cs), Val::Meaning(Meaning::Prim(relax)), false);
        }
    }
    c.back(t);
}

fn endcsname(c: &mut dyn Ctx, _: u16) {
    let mut e = String::new();
    print_esc(c, &mut e, "endcsname");
    error(c, &format!("Extra {e}"), &["I'm ignoring this, since I wasn't doing a \\csname."]);
}

// ---------------------------------------------------------------- \the

const THE: u16 = 0;
const UNEXPANDED: u16 = 1;
const DETOKENIZE: u16 = 2;

/// `the_toks` (tex.web 465, e-TeX): `\the` 0, `\unexpanded` 1, `\detokenize` 2.
pub fn the_toks(c: &mut dyn Ctx, code: u16) -> Toks {
    if code == UNEXPANDED {
        return scan::scan_general_text(c);
    }
    if code == DETOKENIZE {
        let t = scan::scan_general_text(c);
        let s = show_toks(c, &t);
        return str_toks(&s);
    }
    let Some(t) = c.next_x() else { return Arc::from(Vec::<Tok>::new()) };
    let (v, l) = scan::scan_internal(c, t, Level::Tok, false);
    match l {
        Level::Tok => v.toks(),
        Level::Ident => {
            let f = match v {
                Val::Font(f) => f,
                _ => 0,
            };
            match crate::font::font_id_text(c, f) {
                Some(t) => Arc::from(vec![t]),
                None => Arc::from(vec![Tok::Cs(c.names().intern("nullfont"))]),
            }
        }
        Level::Int => str_toks(&v.int().to_string()),
        Level::Dimen => str_toks(&(print_scaled(v.int()) + "pt")),
        Level::Glue => str_toks(&print_spec(&v.glue(), "pt")),
        Level::Mu => str_toks(&print_spec(&v.glue(), "mu")),
    }
}

fn the(c: &mut dyn Ctx, code: u16) {
    let t = the_toks(c, code);
    if !t.is_empty() {
        c.push(t, TokKind::Inserted);
    }
}

fn expanded(c: &mut dyn Ctx, _: u16) {
    let (_, b) = scan::scan_toks(c, false, true);
    if !b.is_empty() {
        c.push(b, TokKind::Inserted);
    }
}

// ---------------------------------------------------------------- marks

/// `\topmark` 0 … `\splitbotmark` 4; e-TeX `\topmarks` 5 … `\splitbotmarks` 9.
fn top_bot_mark(c: &mut dyn Ctx, code: u16) {
    let (class, which) = if code < 5 { (0, code) } else { (scan::scan_register_num(c), code - 5) };
    let t = c.get(&Key::Mark(class, which as u8)).toks();
    if !t.is_empty() {
        c.push(t, TokKind::Mark);
    }
}

const MARK_NAMES: [&str; 5] = ["topmark", "firstmark", "botmark", "splitfirstmark", "splitbotmark"];
const MARKS_NAMES: [&str; 5] = ["topmarks", "firstmarks", "botmarks", "splitfirstmarks", "splitbotmarks"];

// ---------------------------------------------------------------- \uppercase \lowercase

/// `\lowercase` 0, `\uppercase` 1 (tex.web 1288): a command, not expandable.
fn case_shift(c: &mut dyn Ctx, code: u16) {
    let (_, b) = scan::scan_toks(c, false, false);
    let mut out: Vec<Tok> = Vec::with_capacity(b.len());
    for &t in b.iter() {
        let ch = match t {
            Tok::Char { ch, .. } | Tok::Active(ch) => ch,
            _ => {
                out.push(t);
                continue;
            }
        };
        let k = if code == 1 { Key::Uc(ch) } else { Key::Lc(ch) };
        let v = c.int(&k);
        let nc = if v != 0 { char::from_u32(v as u32).unwrap_or(ch) } else { ch };
        out.push(match t {
            Tok::Char { cat, .. } => Tok::Char { cat, ch: nc },
            _ => Tok::Active(nc),
        });
    }
    if !out.is_empty() {
        c.push(Arc::from(out), TokKind::Backed);
    }
}

// ---------------------------------------------------------------- conversions

const NUMBER: u16 = 0;
const ROMAN: u16 = 1;
const STRING: u16 = 2;
const MEANING: u16 = 3;
const FONTNAME: u16 = 4;
const JOBNAME: u16 = 5;
const ETEXREV: u16 = 6;
const PDFTEXREV: u16 = 7;
const PDFTEXBANNER: u16 = 8;
const STRCMP: u16 = 9;
const ESCSTR: u16 = 10;
const ESCNAME: u16 = 11;
const ESCHEX: u16 = 12;
const UNESCHEX: u16 = 13;
const FILESIZE: u16 = 14;
const MD5: u16 = 15;
const CREATIONDATE: u16 = 16;
const MATCH: u16 = 17;
const LASTMATCH: u16 = 18;

/// pdfTeX's `scan_pdf_ext_toks` then `tokens_to_string`.
fn ext_string(c: &mut dyn Ctx) -> String {
    let (_, t) = scan::scan_toks(c, false, true);
    show_toks(c, &t)
}

/// A string's characters as bytes (8-bit characters as themselves).
fn to_bytes(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 256 {
            v.push(ch as u32 as u8);
        } else {
            let mut b = [0u8; 4];
            v.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
        }
    }
    v
}

fn from_bytes(b: &[u8]) -> String {
    b.iter().map(|&x| x as char).collect()
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// `conv_toks` (tex.web 470, e-TeX, pdfTeX).
fn conv(c: &mut dyn Ctx, code: u16) {
    let s: String = match code {
        NUMBER => scan::scan_int(c).to_string(),
        ROMAN => print_roman_int(scan::scan_int(c)),
        STRING => {
            let Some(t) = get_tok(c) else { return };
            let mut s = String::new();
            match t {
                Tok::Cs(cs) => {
                    let esc = escape_char(c);
                    cs_text(c, esc, cs, &mut s, false);
                }
                Tok::Active(ch) | Tok::Char { ch, .. } => s.push(ch),
                Tok::Param(n) => {
                    s.push('#');
                    if n > 0 {
                        s.push((b'0' + n) as char);
                    }
                }
                Tok::DontExpand => {}
            }
            s
        }
        MEANING => {
            let Some(t) = get_tok(c) else { return };
            print_meaning(c, &t)
        }
        FONTNAME => {
            let f = scan::scan_font_ident(c);
            crate::font::font_name_text(c, f)
        }
        JOBNAME => match c.get(&Key::JobName) {
            Val::Name(n) => n.to_string(),
            _ => "texput".to_string(),
        },
        ETEXREV => ETEX_REVISION.to_string(),
        PDFTEXREV => PDFTEX_REVISION.to_string(),
        PDFTEXBANNER => PDFTEX_BANNER.to_string(),
        STRCMP => {
            let a = ext_string(c);
            let b = ext_string(c);
            match to_bytes(&a).cmp(&to_bytes(&b)) {
                std::cmp::Ordering::Less => "-1",
                std::cmp::Ordering::Equal => "0",
                std::cmp::Ordering::Greater => "1",
            }
            .to_string()
        }
        ESCSTR => {
            let a = to_bytes(&ext_string(c));
            let mut o = String::new();
            for b in a {
                if b == b'(' || b == b')' || b == b'\\' {
                    o.push('\\');
                    o.push(b as char);
                } else if !(0x21..=0x7e).contains(&b) {
                    o.push_str(&format!("\\{:03o}", b));
                } else {
                    o.push(b as char);
                }
            }
            o
        }
        ESCNAME => {
            let a = to_bytes(&ext_string(c));
            let mut o = String::new();
            for b in a {
                if b == 0 {
                    continue;
                }
                if b <= 0x20 || b >= 0x7f || b"#%()/<>[]{}".contains(&b) {
                    o.push('#');
                    o.push(HEX[(b >> 4) as usize] as char);
                    o.push(HEX[(b & 15) as usize] as char);
                } else {
                    o.push(b as char);
                }
            }
            o
        }
        ESCHEX => {
            let a = to_bytes(&ext_string(c));
            let mut o = String::with_capacity(a.len() * 2);
            for b in a {
                o.push(HEX[(b >> 4) as usize] as char);
                o.push(HEX[(b & 15) as usize] as char);
            }
            o
        }
        UNESCHEX => {
            let a = to_bytes(&ext_string(c));
            let mut out: Vec<u8> = Vec::new();
            let mut hi: Option<u8> = None;
            for b in a {
                let d = match b {
                    b'0'..=b'9' => b - b'0',
                    b'a'..=b'f' => b - b'a' + 10,
                    b'A'..=b'F' => b - b'A' + 10,
                    _ => continue,
                };
                match hi.take() {
                    None => hi = Some(d),
                    Some(h) => out.push(h << 4 | d),
                }
            }
            if let Some(h) = hi {
                out.push(h << 4);
            }
            from_bytes(&out)
        }
        FILESIZE => {
            let name = ext_string(c);
            match c.file(&name) {
                Some((_, b)) => b.len().to_string(),
                None => String::new(),
            }
        }
        MD5 => {
            let file = scan::scan_keyword(c, "file");
            let a = ext_string(c);
            let data: Option<Vec<u8>> = if file {
                c.file(&a).map(|(_, b)| b.to_vec())
            } else {
                Some(to_bytes(&a))
            };
            match data {
                Some(d) => md5(&d).iter().flat_map(|b| [HEX[(b >> 4) as usize] as char, HEX[(b & 15) as usize] as char]).collect(),
                None => String::new(),
            }
        }
        CREATIONDATE => match c.get(&Key::Given(GIVEN_CREATION_DATE)) {
            Val::Name(n) => n.to_string(),
            _ => "D:19700101000000Z".to_string(),
        },
        MATCH => pdf_match(c),
        LASTMATCH => {
            let n = scan::scan_int(c);
            let caps = c.get(&Key::Other(tag::EXPAND, 0));
            let subj = c.get(&Key::Other(tag::EXPAND, 1));
            let mut o = String::from("-1->");
            if let (Val::Ints(v), Val::Name(sj)) = (caps, subj) {
                if n >= 0 && (2 * n as usize + 1) < v.len() {
                    let (a, b) = (v[2 * n as usize], v[2 * n as usize + 1]);
                    if a >= 0 {
                        let bytes = to_bytes(&sj);
                        let (a, b) = (a as usize, (b as usize).min(bytes.len()));
                        o = format!("{}->{}", a, from_bytes(&bytes[a.min(b)..b]));
                    }
                }
            }
            o
        }
        _ => String::new(),
    };
    if !s.is_empty() {
        c.push(str_toks(&s), TokKind::Inserted);
    }
}

/// `\pdfmatch [icase] [subcount n] {pattern}{string}`: "1", "0", or "-1" on a bad pattern.
fn pdf_match(c: &mut dyn Ctx) -> String {
    let icase = scan::scan_keyword(c, "icase");
    let mut subcount: i32 = -1;
    if scan::scan_keyword(c, "subcount") {
        subcount = scan::scan_int(c);
    }
    let pat = to_bytes(&ext_string(c));
    let subj = ext_string(c);
    let Some(re) = re::Re::new(&pat, icase) else { return "-1".to_string() };
    let sb = to_bytes(&subj);
    match re.exec(&sb) {
        Some(caps) => {
            let count = if subcount > 0 { subcount as usize } else { caps.len() };
            let mut v: Vec<i32> = Vec::with_capacity(count * 2);
            for i in 0..count {
                match caps.get(i).copied().flatten() {
                    Some((a, b)) => {
                        v.push(a as i32);
                        v.push(b as i32);
                    }
                    None => {
                        v.push(-1);
                        v.push(-1);
                    }
                }
            }
            c.define(Key::Other(tag::EXPAND, 0), Val::Ints(Arc::from(v)), true);
            c.define(Key::Other(tag::EXPAND, 1), Val::Name(Arc::from(subj.as_str())), true);
            "1".to_string()
        }
        None => "0".to_string(),
    }
}

// ---------------------------------------------------------------- printing

/// `\escapechar` if it is printable (0..=255).
pub fn escape_char(c: &mut dyn Ctx) -> Option<char> {
    let e = c.int(&Key::IntPar(crate::prim::ip::of("escapechar")));
    if (0..256).contains(&e) { char::from_u32(e as u32) } else { None }
}

/// `print_esc(name)` into `out`.
pub fn print_esc(c: &mut dyn Ctx, out: &mut String, name: &str) {
    if let Some(e) = escape_char(c) {
        out.push(e);
    }
    out.push_str(name);
}

/// `print_cs` (`trailing`: the space after a multi-letter name or a
/// single letter) or `sprint_cs` (no space).
fn cs_text(c: &mut dyn Ctx, esc: Option<char>, cs: Cs, out: &mut String, trailing: bool) {
    let name = c.names().name(cs);
    let mut it = name.chars();
    match (it.next(), it.next()) {
        (None, _) => {
            out.extend(esc);
            out.push_str("csname");
            out.extend(esc);
            out.push_str("endcsname");
            if trailing {
                out.push(' ');
            }
        }
        (Some(ch), None) => {
            out.extend(esc);
            out.push(ch);
            if trailing && c.int(&Key::Cat(ch)) == 11 {
                out.push(' ');
            }
        }
        _ => {
            out.extend(esc);
            out.push_str(&name);
            if trailing {
                out.push(' ');
            }
        }
    }
}

/// `print_cs`: as a name shows in a token list.
pub fn print_cs(c: &mut dyn Ctx, cs: Cs) -> String {
    let esc = escape_char(c);
    let mut s = String::new();
    cs_text(c, esc, cs, &mut s, true);
    s
}

/// `sprint_cs`: as `\string` shows a name.
pub fn sprint_cs(c: &mut dyn Ctx, cs: Cs) -> String {
    let esc = escape_char(c);
    let mut s = String::new();
    cs_text(c, esc, cs, &mut s, false);
    s
}

fn show_into(c: &mut dyn Ctx, esc: Option<char>, toks: &[Tok], out: &mut String) {
    for &t in toks {
        match t {
            Tok::Cs(cs) => cs_text(c, esc, cs, out, true),
            Tok::Active(ch) => out.push(ch),
            Tok::Char { cat: 6, ch } => {
                out.push(ch);
                out.push(ch);
            }
            Tok::Char { ch, .. } => out.push(ch),
            Tok::Param(0) => out.push_str("##"),
            Tok::Param(n) => {
                out.push('#');
                out.push((b'0' + n) as char);
            }
            Tok::DontExpand => {}
        }
    }
}

/// `show_token_list` / `tokens_to_string`: names with `\escapechar` and the
/// space after them, `#` doubled.
pub fn show_toks(c: &mut dyn Ctx, toks: &[Tok]) -> String {
    let esc = escape_char(c);
    let mut s = String::new();
    show_into(c, esc, toks, &mut s);
    s
}

/// `print_cmd_chr` for a character's category (tex.web 298).
pub fn chr_cmd(cat: u8, ch: char) -> String {
    let p = match cat {
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
    };
    format!("{p}{ch}")
}

fn hex(n: u32) -> String {
    format!("\"{:X}", n)
}

/// A macro's prefixes and `macro` (`print_cmd_chr` of a call).
fn macro_head(c: &mut dyn Ctx, m: &crate::val::Macro) -> String {
    let mut s = String::new();
    if m.protected {
        print_esc(c, &mut s, "protected");
    }
    if m.long {
        print_esc(c, &mut s, "long");
    }
    if m.outer {
        print_esc(c, &mut s, "outer");
    }
    if !s.is_empty() {
        s.push(' ');
    }
    s.push_str("macro");
    s
}

/// The text of a meaning (`print_meaning` without the token).
pub fn meaning_text(c: &mut dyn Ctx, m: &Meaning) -> String {
    let mut s = String::new();
    match m {
        Meaning::Undefined => s.push_str("undefined"),
        Meaning::Prim(id) => print_esc(c, &mut s, registry().get(*id).name),
        Meaning::Macro(f) => {
            s = macro_head(c, f);
            s.push(':');
            let esc = escape_char(c);
            show_into(c, esc, &f.params, &mut s);
            if f.brace_end && !matches!(f.params.last(), Some(Tok::Char { cat: 1, .. })) {
                s.push('{');
            }
            s.push_str("->");
            show_into(c, esc, &f.body, &mut s);
        }
        Meaning::Char { cat, ch } => s = chr_cmd(*cat, *ch),
        Meaning::CharGiven(n) => {
            print_esc(c, &mut s, "char");
            s.push_str(&hex(*n));
        }
        Meaning::MathCharGiven(n) => {
            print_esc(c, &mut s, "mathchar");
            s.push_str(&hex(*n));
        }
        Meaning::Reg(b, n) => {
            let name = ["count", "dimen", "skip", "muskip", "toks"][(*b).min(4) as usize];
            print_esc(c, &mut s, name);
            s.push_str(&n.to_string());
        }
        Meaning::Font(f) => {
            s.push_str("select font ");
            s.push_str(&crate::font::font_name_text(c, *f));
        }
    }
    s
}

/// `print_meaning` of a token (`\meaning`): marks show their text too.
pub fn print_meaning(c: &mut dyn Ctx, t: &Tok) -> String {
    match t {
        Tok::Char { cat, ch } => chr_cmd(*cat, *ch),
        Tok::Cs(_) | Tok::Active(_) => {
            let m = c.meaning(t);
            let mut s = meaning_text(c, &m);
            if let Meaning::Prim(id) = m {
                let d = registry().get(id);
                if MARK_NAMES.contains(&d.name) && d.code < 5 {
                    s.push(':');
                    let mk = c.get(&Key::Mark(0, d.code as u8)).toks();
                    s.push_str(&show_toks(c, &mk));
                }
            }
            s
        }
        Tok::Param(n) => format!("macro parameter character #{n}"),
        Tok::DontExpand => String::new(),
    }
}

/// `print_cmd_chr` of a token (errors: "You can't use `…' …").
pub fn print_cmd_chr(c: &mut dyn Ctx, t: &Tok) -> String {
    match t {
        Tok::Char { cat, ch } => chr_cmd(*cat, *ch),
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Macro(f) => macro_head(c, &f),
            m => meaning_text(c, &m),
        },
        _ => String::new(),
    }
}

/// `print_scaled` (tex.web 103).
pub fn print_scaled(s: i32) -> String {
    let mut o = String::new();
    let mut s = s as i64;
    if s < 0 {
        o.push('-');
        s = -s;
    }
    o.push_str(&(s / 65536).to_string());
    o.push('.');
    let mut s = 10 * (s % 65536) + 5;
    let mut delta: i64 = 10;
    loop {
        if delta > 65536 {
            s += 0o100000 - 50000;
        }
        o.push((b'0' + (s / 65536) as u8) as char);
        s = 10 * (s % 65536);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    o
}

fn print_glue(d: i32, order: u8, unit: &str) -> String {
    let mut s = print_scaled(d);
    if order > 3 {
        s.push_str("foul");
    } else if order > 0 {
        s.push_str("fil");
        for _ in 1..order {
            s.push('l');
        }
    } else {
        s.push_str(unit);
    }
    s
}

/// `print_spec(g, unit)` (tex.web 178).
pub fn print_spec(g: &Glue, unit: &str) -> String {
    let mut s = print_scaled(g.width);
    s.push_str(unit);
    if g.stretch != 0 {
        s.push_str(" plus ");
        s.push_str(&print_glue(g.stretch, g.stretch_order, unit));
    }
    if g.shrink != 0 {
        s.push_str(" minus ");
        s.push_str(&print_glue(g.shrink, g.shrink_order, unit));
    }
    s
}

/// `print_roman_int` (tex.web 69).
pub fn print_roman_int(n: i32) -> String {
    let p = b"m2d5c2l5x2v5i";
    let mut o = String::new();
    let mut n = n;
    let mut j = 0usize;
    let mut v = 1000;
    loop {
        while n >= v {
            o.push(p[j] as char);
            n -= v;
        }
        if n <= 0 {
            return o;
        }
        let mut k = j + 2;
        let mut u = v / (p[k - 1] - b'0') as i32;
        if p[k - 1] == b'2' {
            k += 2;
            u /= (p[k - 1] - b'0') as i32;
        }
        if n + u >= v {
            o.push(p[k] as char);
            n += u;
        } else {
            j += 2;
            v /= (p[j - 1] - b'0') as i32;
        }
    }
}

/// `str_toks`: characters as other tokens, spaces as space tokens.
pub fn str_toks(s: &str) -> Toks {
    s.chars()
        .map(|ch| if ch == ' ' { Tok::Char { cat: 10, ch } } else { Tok::Char { cat: 12, ch } })
        .collect::<Vec<_>>()
        .into()
}

/// The name of a font for messages (`\fontname` text).
pub fn font_name(c: &mut dyn Ctx, f: FontId) -> String {
    crate::font::font_name_text(c, f)
}

// ---------------------------------------------------------------- MD5 (RFC 1321)

pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = (0..16).map(|i| u32::from_le_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]])).collect();
        let (mut a, mut b, mut cc, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & cc) | (!b & d), i),
                1 => ((d & b) | (!d & cc), (5 * i + 1) % 16),
                2 => (b ^ cc ^ d, (3 * i + 5) % 16),
                _ => (cc ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = cc;
            cc = b;
            b = b.wrapping_add(f2.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(cc);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------- POSIX ERE (for \pdfmatch)

mod re {
    pub type Caps = Vec<Option<(usize, usize)>>;

    #[derive(Debug)]
    enum Node {
        Byte(u8),
        Any,
        Set(Vec<(u8, u8)>, bool),
        Bol,
        Eol,
        Group(Box<Node>, usize),
        Alt(Vec<Node>),
        Cat(Vec<Node>),
        Rep(Box<Node>, u32, Option<u32>),
    }

    pub struct Re {
        root: Node,
        groups: usize,
        icase: bool,
    }

    struct P<'a> {
        s: &'a [u8],
        i: usize,
        groups: usize,
    }

    impl P<'_> {
        fn peek(&self) -> Option<u8> {
            self.s.get(self.i).copied()
        }
        fn alt(&mut self) -> Option<Node> {
            let mut v = vec![self.cat()?];
            while self.peek() == Some(b'|') {
                self.i += 1;
                v.push(self.cat()?);
            }
            Some(if v.len() == 1 { v.pop().unwrap() } else { Node::Alt(v) })
        }
        fn cat(&mut self) -> Option<Node> {
            let mut v = Vec::new();
            while let Some(b) = self.peek() {
                if b == b'|' || b == b')' {
                    break;
                }
                v.push(self.rep()?);
            }
            Some(Node::Cat(v))
        }
        fn num(&mut self) -> Option<u32> {
            let st = self.i;
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.i += 1;
            }
            if self.i == st {
                return None;
            }
            std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok()
        }
        fn rep(&mut self) -> Option<Node> {
            let mut a = self.atom()?;
            loop {
                let (min, max) = match self.peek() {
                    Some(b'*') => {
                        self.i += 1;
                        (0, None)
                    }
                    Some(b'+') => {
                        self.i += 1;
                        (1, None)
                    }
                    Some(b'?') => {
                        self.i += 1;
                        (0, Some(1))
                    }
                    Some(b'{') if self.s.get(self.i + 1).is_some_and(|b| b.is_ascii_digit()) => {
                        self.i += 1;
                        let m = self.num()?;
                        let mx = if self.peek() == Some(b',') {
                            self.i += 1;
                            if self.peek() == Some(b'}') { None } else { Some(self.num()?) }
                        } else {
                            Some(m)
                        };
                        if self.peek() != Some(b'}') {
                            return None;
                        }
                        self.i += 1;
                        if mx.is_some_and(|x| x < m) {
                            return None;
                        }
                        (m, mx)
                    }
                    _ => break,
                };
                a = Node::Rep(Box::new(a), min, max);
            }
            Some(a)
        }
        fn atom(&mut self) -> Option<Node> {
            let b = self.peek()?;
            self.i += 1;
            Some(match b {
                b'(' => {
                    self.groups += 1;
                    let idx = self.groups;
                    let inner = self.alt()?;
                    if self.peek() != Some(b')') {
                        return None;
                    }
                    self.i += 1;
                    Node::Group(Box::new(inner), idx)
                }
                b'[' => self.class()?,
                b'.' => Node::Any,
                b'^' => Node::Bol,
                b'$' => Node::Eol,
                b'\\' => {
                    let c = self.peek()?;
                    self.i += 1;
                    Node::Byte(c)
                }
                b'*' | b'+' | b'?' => return None,
                _ => Node::Byte(b),
            })
        }
        fn class(&mut self) -> Option<Node> {
            let mut neg = false;
            if self.peek() == Some(b'^') {
                neg = true;
                self.i += 1;
            }
            let mut r: Vec<(u8, u8)> = Vec::new();
            let mut first = true;
            loop {
                let b = self.peek()?;
                if b == b']' && !first {
                    self.i += 1;
                    break;
                }
                first = false;
                if b == b'[' && self.s.get(self.i + 1) == Some(&b':') {
                    let rest = &self.s[self.i + 2..];
                    let end = rest.windows(2).position(|w| w == b":]")?;
                    let name = &rest[..end];
                    self.i += 2 + end + 2;
                    let add: &[(u8, u8)] = match name {
                        b"alpha" => &[(b'a', b'z'), (b'A', b'Z')],
                        b"digit" => &[(b'0', b'9')],
                        b"alnum" => &[(b'a', b'z'), (b'A', b'Z'), (b'0', b'9')],
                        b"upper" => &[(b'A', b'Z')],
                        b"lower" => &[(b'a', b'z')],
                        b"space" => &[(b' ', b' '), (9, 13)],
                        b"blank" => &[(b' ', b' '), (9, 9)],
                        b"punct" => &[(b'!', b'/'), (b':', b'@'), (b'[', b'`'), (b'{', b'~')],
                        b"xdigit" => &[(b'0', b'9'), (b'a', b'f'), (b'A', b'F')],
                        b"print" => &[(0x20, 0x7e)],
                        b"graph" => &[(0x21, 0x7e)],
                        b"cntrl" => &[(0, 0x1f), (0x7f, 0x7f)],
                        _ => return None,
                    };
                    r.extend_from_slice(add);
                    continue;
                }
                self.i += 1;
                if self.peek() == Some(b'-') && self.s.get(self.i + 1).is_some_and(|&x| x != b']') {
                    let hi = self.s[self.i + 1];
                    self.i += 2;
                    if hi < b {
                        return None;
                    }
                    r.push((b, hi));
                } else {
                    r.push((b, b));
                }
            }
            Some(Node::Set(r, neg))
        }
    }

    impl Re {
        pub fn new(pat: &[u8], icase: bool) -> Option<Re> {
            let mut p = P { s: pat, i: 0, groups: 0 };
            let root = p.alt()?;
            if p.i != pat.len() {
                return None;
            }
            Some(Re { root, groups: p.groups, icase })
        }

        fn eq(&self, a: u8, b: u8) -> bool {
            if self.icase { a.eq_ignore_ascii_case(&b) } else { a == b }
        }

        fn in_set(&self, r: &[(u8, u8)], b: u8) -> bool {
            let hit = |x: u8| r.iter().any(|&(lo, hi)| lo <= x && x <= hi);
            hit(b) || (self.icase && (hit(b.to_ascii_lowercase()) || hit(b.to_ascii_uppercase())))
        }

        fn m(&self, n: &Node, s: &[u8], p: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
            match n {
                Node::Byte(b) => p < s.len() && self.eq(s[p], *b) && k(p + 1, caps),
                Node::Any => p < s.len() && k(p + 1, caps),
                Node::Set(r, neg) => p < s.len() && self.in_set(r, s[p]) != *neg && k(p + 1, caps),
                Node::Bol => p == 0 && k(p, caps),
                Node::Eol => p == s.len() && k(p, caps),
                Node::Group(inner, i) => {
                    let i = *i;
                    self.m(inner, s, p, caps, &mut |q, caps: &mut Caps| {
                        let old = caps[i];
                        caps[i] = Some((p, q));
                        if k(q, caps) {
                            true
                        } else {
                            caps[i] = old;
                            false
                        }
                    })
                }
                Node::Alt(v) => {
                    for a in v {
                        if self.m(a, s, p, caps, k) {
                            return true;
                        }
                    }
                    false
                }
                Node::Cat(v) => self.cat(v, 0, s, p, caps, k),
                Node::Rep(inner, min, max) => self.rep(inner, *min, *max, 0, s, p, caps, k),
            }
        }

        fn cat(&self, v: &[Node], j: usize, s: &[u8], p: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
            if j == v.len() {
                return k(p, caps);
            }
            self.m(&v[j], s, p, caps, &mut |q, caps: &mut Caps| self.cat(v, j + 1, s, q, caps, &mut *k))
        }

        #[allow(clippy::too_many_arguments)]
        fn rep(
            &self,
            n: &Node,
            min: u32,
            max: Option<u32>,
            cnt: u32,
            s: &[u8],
            p: usize,
            caps: &mut Caps,
            k: &mut dyn FnMut(usize, &mut Caps) -> bool,
        ) -> bool {
            if max.is_none_or(|m| cnt < m) {
                let hit = self.m(n, s, p, caps, &mut |q, caps: &mut Caps| {
                    if q == p && cnt >= min {
                        return false;
                    }
                    self.rep(n, min, max, cnt + 1, s, q, caps, &mut *k)
                });
                if hit {
                    return true;
                }
            }
            cnt >= min && k(p, caps)
        }

        /// Leftmost-longest match: group spans (0: the whole match).
        pub fn exec(&self, s: &[u8]) -> Option<Caps> {
            for start in 0..=s.len() {
                let mut caps: Caps = vec![None; self.groups + 1];
                let mut best: Option<(usize, Caps)> = None;
                self.m(&self.root, s, start, &mut caps, &mut |q, caps: &mut Caps| {
                    if best.as_ref().is_none_or(|b| q > b.0) {
                        best = Some((q, caps.clone()));
                    }
                    false
                });
                if let Some((e, mut c)) = best {
                    c[0] = Some((start, e));
                    return Some(c);
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------- registration

pub fn register(r: &mut Registry) {
    r.add("expandafter", 0, Kind::Expand(expandafter), 0);
    r.add("noexpand", 0, Kind::Expand(noexpand), 0);
    r.add("csname", 0, Kind::Expand(csname), 0);
    r.add("endcsname", 0, Kind::Command(endcsname), 0);
    r.add("the", THE, Kind::Expand(the), 0);
    r.add("number", NUMBER, Kind::Expand(conv), 0);
    r.add("romannumeral", ROMAN, Kind::Expand(conv), 0);
    r.add("string", STRING, Kind::Expand(conv), 0);
    r.add("meaning", MEANING, Kind::Expand(conv), 0);
    r.add("fontname", FONTNAME, Kind::Expand(conv), 0);
    r.add("jobname", JOBNAME, Kind::Expand(conv), 0);
    for (i, n) in MARK_NAMES.iter().enumerate() {
        r.add(n, i as u16, Kind::Expand(top_bot_mark), 0);
    }
    r.add("lowercase", 0, Kind::Command(case_shift), 0);
    r.add("uppercase", 1, Kind::Command(case_shift), 0);
    // e-TeX
    r.add("unexpanded", UNEXPANDED, Kind::Expand(the), 1);
    r.add("detokenize", DETOKENIZE, Kind::Expand(the), 1);
    r.add("eTeXrevision", ETEXREV, Kind::Expand(conv), 1);
    for (i, n) in MARKS_NAMES.iter().enumerate() {
        r.add(n, 5 + i as u16, Kind::Expand(top_bot_mark), 1);
    }
    // pdfTeX
    r.add("expanded", 0, Kind::Expand(expanded), 2);
    r.add("pdfstrcmp", STRCMP, Kind::Expand(conv), 2);
    r.add("pdfescapestring", ESCSTR, Kind::Expand(conv), 2);
    r.add("pdfescapename", ESCNAME, Kind::Expand(conv), 2);
    r.add("pdfescapehex", ESCHEX, Kind::Expand(conv), 2);
    r.add("pdfunescapehex", UNESCHEX, Kind::Expand(conv), 2);
    r.add("pdffilesize", FILESIZE, Kind::Expand(conv), 2);
    r.add("pdfmdfivesum", MD5, Kind::Expand(conv), 2);
    r.add("pdfcreationdate", CREATIONDATE, Kind::Expand(conv), 2);
    r.add("pdftexrevision", PDFTEXREV, Kind::Expand(conv), 2);
    r.add("pdftexbanner", PDFTEXBANNER, Kind::Expand(conv), 2);
    r.add("pdfmatch", MATCH, Kind::Expand(conv), 2);
    r.add("pdflastmatch", LASTMATCH, Kind::Expand(conv), 2);
}
