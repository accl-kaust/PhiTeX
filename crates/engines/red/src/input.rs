//! B1: `\input`, `\endinput`, `\scantokens`, `\everyeof`, file names.
//!
//! `\input` rewrites to the file's text in front of the rest of the current
//! line (`Ctx::push_text`). `\endinput` marks the current text to end at its
//! line's end. `\scantokens` turns a token list into a pseudo-file text
//! (lines split at `\newlinechar`). `\everyeof` is a token parameter (assigned
//! by `assign.rs`); the tokenizer inserts it where a text ends.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::prim::{ip, Kind, Level, Registry};
use crate::tok::{Tok, Toks};
use crate::val::{Key, Meaning, Val};

pub fn register(r: &mut Registry) {
    r.add("input", 0, Kind::Expand(input_prim), 0);
    r.add("endinput", 1, Kind::Expand(input_prim), 0);
    r.add("scantokens", 2, Kind::Expand(input_prim), 1);
    r.add("inputlineno", 0, Kind::Computed { level: Level::Int, get: input_line_no, set: None }, 1);
}

fn input_prim(c: &mut dyn Ctx, code: u16) {
    match code {
        0 => start_input(c),
        1 => c.end_input(),
        _ => scan_tokens(c),
    }
}

fn input_line_no(c: &mut dyn Ctx, _code: u16) -> Val {
    Val::Int(c.line_no())
}

/// tex.web 537: `\input` a file.
pub fn start_input(c: &mut dyn Ctx) {
    let name = scan_file_name(c);
    match open_tex_file(c, &name) {
        Some((found, data)) => {
            c.push_text(found, decode(&data));
        }
        None => {
            let shown = with_default_ext(&name, ".tex");
            crate::diag::error(
                c,
                &format!("I can't find file `{shown}'."),
                &["The file you asked for could not be found.", "*** (job aborted, file error in nonstop mode)"],
            );
        }
    }
}

/// Opens `name` as TeX does for `\input`: with `.tex` added when it has no
/// extension (tried first), then as given.
pub fn open_tex_file(c: &mut dyn Ctx, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
    let (_, _, ext) = split_name(name);
    if ext.is_empty() {
        if let Some(f) = c.file(&format!("{name}.tex")) {
            return Some(f);
        }
    }
    c.file(name)
}

/// File bytes as text: UTF-8 if valid, else each byte as a char (8-bit TeX).
pub fn decode(b: &[u8]) -> Arc<str> {
    match std::str::from_utf8(b) {
        Ok(s) => Arc::from(s),
        Err(_) => Arc::from(b.iter().map(|&x| x as char).collect::<String>()),
    }
}

/// (area, base, extension) of a file name: area up to the last `/`, the
/// extension from the last `.` after it (web2c), dot included.
pub fn split_name(name: &str) -> (&str, &str, &str) {
    let a = name.rfind('/').map_or(0, |i| i + 1);
    let (area, rest) = name.split_at(a);
    match rest.rfind('.') {
        Some(i) => (area, &rest[..i], &rest[i..]),
        None => (area, rest, ""),
    }
}

/// `name` with `ext` appended when it has no extension.
pub fn with_default_ext(name: &str, ext: &str) -> String {
    if split_name(name).2.is_empty() { format!("{name}{ext}") } else { name.to_string() }
}

/// The job name a first input file gives (`\jobname`): its base name.
pub fn job_name(file: &str) -> String {
    split_name(file).1.to_string()
}

/// tex.web 526 with web2c's extensions: a file name after expansion, blanks
/// skipped first; `{…}` gives a braced (expanded) name; `"` quotes spaces and
/// is dropped; ends at a space (consumed) or a non-character (put back).
pub fn scan_file_name(c: &mut dyn Ctx) -> String {
    let first = loop {
        match c.next_x() {
            Some(Tok::Char { cat: 10, .. }) => continue,
            Some(t) => break t,
            None => return String::new(),
        }
    };
    if let Tok::Char { cat: 1, .. } = first {
        c.back(first);
        let (_, body) = crate::scan::scan_toks(c, false, true);
        return toks_to_string(c, &body).chars().filter(|&ch| ch != '"').collect();
    }
    let mut s = String::new();
    let mut quoted = false;
    let mut t = Some(first);
    while let Some(tok) = t {
        let ch = match tok {
            Tok::Char { ch, .. } => Some(ch),
            Tok::Cs(_) | Tok::Active(_) => match c.meaning(&tok) {
                Meaning::Char { cat, ch } if cat <= 12 => Some(ch),
                _ => None,
            },
            _ => None,
        };
        let Some(ch) = ch else {
            c.back(tok);
            break;
        };
        if ch == ' ' && !quoted {
            break;
        }
        if ch == '"' {
            quoted = !quoted;
        } else {
            s.push(ch);
        }
        t = c.next_x();
    }
    s
}

/// `show_token_list` into a string (as `\scantokens` and `\write` print):
/// control sequences with `\escapechar` and a space after a letter name,
/// parameter chars doubled.
pub fn toks_to_string(c: &mut dyn Ctx, toks: &[Tok]) -> String {
    let esc = c.int(&Key::IntPar(ip::of("escapechar")));
    let esc = if (0..=255).contains(&esc) { char::from_u32(esc as u32) } else { None };
    let mut s = String::new();
    for &t in toks {
        match t {
            Tok::Char { cat, ch } => {
                s.push(ch);
                if cat == 6 {
                    s.push(ch);
                }
            }
            Tok::Active(ch) => s.push(ch),
            Tok::Param(0) => s.push_str("##"),
            Tok::Param(n) => {
                s.push('#');
                s.push((b'0' + n) as char);
            }
            Tok::DontExpand => {}
            Tok::Cs(cs) => {
                let name = c.names().name(cs);
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (None, _) => {
                        s.extend(esc);
                        s.push_str("csname");
                        s.extend(esc);
                        s.push_str("endcsname ");
                    }
                    (Some(ch), None) => {
                        s.extend(esc);
                        s.push(ch);
                        if c.int(&Key::Cat(ch)) == 11 {
                            s.push(' ');
                        }
                    }
                    _ => {
                        s.extend(esc);
                        s.push_str(&name);
                        s.push(' ');
                    }
                }
            }
        }
    }
    s
}

/// e-TeX `\scantokens`: the general text printed, lines split at
/// `\newlinechar`, read as a pseudo-file.
pub fn scan_tokens(c: &mut dyn Ctx) {
    let toks: Toks = crate::scan::scan_general_text(c);
    let mut s = toks_to_string(c, &toks);
    let nl = c.int(&Key::IntPar(ip::of("newlinechar")));
    if (0..=255).contains(&nl) && nl != 10 {
        if let Some(nlc) = char::from_u32(nl as u32) {
            s = s.replace(nlc, "\n");
        }
    }
    c.push_text(Arc::from("\\scantokens"), Arc::from(s));
}
