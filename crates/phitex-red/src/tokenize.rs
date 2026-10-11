//! B1: characters → tokens (tex.web 332–365 semantics): catcodes read from env,
//! `^^` notation, line ends, `\endlinechar`, states N/M/S.
//!
//! `next_token` is a pure function of (the input value, the values it reads).
//! A text source (`Src::Text`) holds the tokenizer's whole state: the byte
//! position, the state N/M/S, the current line's end (trailing blanks
//! stripped) and its end-of-line char. The end-of-line char sits at the
//! virtual position `line_end`; `pos > line_end` (or `pos == line_end` with
//! `eol == None`) means the line is used up. `line_end == NOT_LOADED` means no
//! line has been read yet: the next line starts at `pos` (see `text_source`).
//!
//! Invalid characters (catcode 15) are returned as `Tok::Char { cat: 15, ch }`
//! with the input advanced past them: the caller (`Ctx::next`) reports TeX's
//! "Text line contains an invalid character" error and asks for the next token
//! (tex.web 346: the character is deleted).
use std::sync::Arc;

use crate::prim::{ip, tp};
use crate::term::{Input, Line, Span, Src, TextId, TokKind};
use crate::tok::{Names, Tok};
use crate::val::{Key, Val};

/// `line_end` of a text whose first line is not read yet.
pub const NOT_LOADED: u32 = u32::MAX;

/// A fresh text source (a file, a `\scantokens` pseudo-file, a `\read` text).
pub fn text_source(text: TextId, data: Arc<str>) -> Src {
    Src::Text { text, data, pos: 0, line: Line::NewLine, line_end: NOT_LOADED, eol: None, ending: false }
}

fn catcode(read: &mut dyn FnMut(&Key) -> Val, c: char) -> u8 {
    read(&Key::Cat(c)).int().clamp(0, 15) as u8
}

fn is_hex(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c)
}

fn hex_val(c: char) -> u32 {
    if c.is_ascii_digit() { c as u32 - '0' as u32 } else { c as u32 - 'a' as u32 + 10 }
}

/// The current line of a text: its characters then the end-of-line char.
struct Cur<'a> {
    data: &'a str,
    line_end: u32,
    eol: Option<char>,
}

impl Cur<'_> {
    /// The character at `p` and the position after it; `None` past the buffer.
    fn at(&self, p: u32) -> Option<(char, u32)> {
        if p < self.line_end {
            let c = self.data[p as usize..].chars().next()?;
            Some((c, p + c.len_utf8() as u32))
        } else if p == self.line_end {
            self.eol.map(|e| (e, p + 1))
        } else {
            None
        }
    }
}

/// tex.web 352: `^^` after a `^`-like char `c` whose successor is at `p`.
fn sup_reduce(cur: &Cur, c: char, p: u32) -> Option<(char, u32)> {
    let (c1, p1) = cur.at(p)?;
    if c1 != c {
        return None;
    }
    let (c2, p2) = cur.at(p1)?;
    if (c2 as u32) >= 128 {
        return None;
    }
    if is_hex(c2) {
        if let Some((c3, p3)) = cur.at(p2) {
            if is_hex(c3) {
                return char::from_u32(hex_val(c2) * 16 + hex_val(c3)).map(|x| (x, p3));
            }
        }
    }
    let v = c2 as u32;
    char::from_u32(if v < 64 { v + 64 } else { v - 64 }).map(|x| (x, p2))
}

fn fill(cur: &Cur, buf: &mut Vec<(char, u32)>, src: &mut u32, k: usize) -> bool {
    while buf.len() <= k {
        match cur.at(*src) {
            Some((c, p)) => {
                buf.push((c, p));
                *src = p;
            }
            None => return false,
        }
    }
    true
}

/// tex.web 355 on `buf` (the buffer from `loc`, each char with the source
/// position after it): reduce an expanded code at `k`, in place.
fn cs_reduce(cur: &Cur, buf: &mut Vec<(char, u32)>, src: &mut u32, k: usize, cat: u8) -> bool {
    if cat != 7 || !fill(cur, buf, src, k + 1) {
        return false;
    }
    let ch = buf[k - 1].0;
    if buf[k].0 != ch {
        return false;
    }
    let c = buf[k + 1].0;
    if (c as u32) >= 128 {
        return false;
    }
    let mut d = 2;
    let mut new = None;
    if is_hex(c) && fill(cur, buf, src, k + 2) && is_hex(buf[k + 2].0) {
        d = 3;
        new = char::from_u32(hex_val(c) * 16 + hex_val(buf[k + 2].0));
    }
    let new = new.or_else(|| {
        let v = c as u32;
        char::from_u32(if v < 64 { v + 64 } else { v - 64 })
    });
    let Some(new) = new else { return false };
    let end = buf[k - 1 + d].1;
    buf[k - 1] = (new, end);
    buf.drain(k..k + d);
    true
}

/// tex.web 354–356: a control sequence after an escape char; `pos` is after it.
fn control_seq(read: &mut dyn FnMut(&Key) -> Val, names: &Names, cur: &Cur, pos: &mut u32, state: &mut Line) -> Tok {
    let mut buf: Vec<(char, u32)> = Vec::new();
    let mut src = *pos;
    if !fill(cur, &mut buf, &mut src, 0) {
        return Tok::Cs(names.intern(""));
    }
    'start: loop {
        let c0 = buf[0].0;
        let mut cat = catcode(read, c0);
        let mut k = 1usize;
        *state = if cat == 11 || cat == 10 { Line::SkipBlanks } else { Line::MidLine };
        if cat == 11 && fill(cur, &mut buf, &mut src, k) {
            loop {
                cat = catcode(read, buf[k].0);
                k += 1;
                if !(cat == 11 && fill(cur, &mut buf, &mut src, k)) {
                    break;
                }
            }
            if cs_reduce(cur, &mut buf, &mut src, k, cat) {
                continue 'start;
            }
            if cat != 11 {
                k -= 1;
            }
            if k > 1 {
                let name: String = buf[..k].iter().map(|&(c, _)| c).collect();
                *pos = buf[k - 1].1;
                return Tok::Cs(names.intern(&name));
            }
        } else if cs_reduce(cur, &mut buf, &mut src, k, cat) {
            continue 'start;
        }
        *pos = buf[0].1;
        let mut s = [0u8; 4];
        return Tok::Cs(names.intern(c0.encode_utf8(&mut s)));
    }
}

/// tex.web 343–357 within one line. `None`: the line is used up (`pos` past it).
fn lex(
    read: &mut dyn FnMut(&Key) -> Val,
    names: &Names,
    cur: &Cur,
    pos: &mut u32,
    state: &mut Line,
) -> Option<(Tok, u32)> {
    let past = cur.line_end + 1;
    loop {
        let from = *pos;
        let (mut c, p) = cur.at(*pos)?;
        *pos = p;
        loop {
            let k = catcode(read, c);
            match k {
                0 => return Some((control_seq(read, names, cur, pos, state), from)),
                13 => {
                    *state = Line::MidLine;
                    return Some((Tok::Active(c), from));
                }
                7 => {
                    if let Some((c2, p2)) = sup_reduce(cur, c, *pos) {
                        c = c2;
                        *pos = p2;
                        continue;
                    }
                    *state = Line::MidLine;
                    return Some((Tok::Char { cat: 7, ch: c }, from));
                }
                9 => break,
                10 => {
                    if *state == Line::MidLine {
                        *state = Line::SkipBlanks;
                        return Some((Tok::Char { cat: 10, ch: ' ' }, from));
                    }
                    break;
                }
                5 => {
                    *pos = past;
                    return match *state {
                        Line::MidLine => Some((Tok::Char { cat: 10, ch: ' ' }, from)),
                        Line::NewLine => Some((Tok::Cs(names.intern("par")), from)),
                        Line::SkipBlanks => None,
                    };
                }
                14 => {
                    *pos = past;
                    return None;
                }
                15 => return Some((Tok::Char { cat: 15, ch: c }, from)),
                _ => {
                    *state = Line::MidLine;
                    return Some((Tok::Char { cat: k, ch: c }, from));
                }
            }
        }
    }
}

/// The next token of `input`, unexpanded, the input after it, and the source
/// span it was read from (`None` for tokens from token lists). `None`: the
/// input is exhausted. Finished token lists and texts are popped; a text that
/// ends normally (not by `\endinput`) is followed by `\everyeof`.
pub fn next_token(
    read: &mut dyn FnMut(&Key) -> Val,
    names: &Names,
    input: &Input,
) -> Option<(Tok, Input, Option<Span>)> {
    let mut inp = input.clone();
    loop {
        let (src, below) = match inp.0.as_deref() {
            None => return None,
            Some((s, b)) => (s.clone(), b.clone()),
        };
        match src {
            Src::Toks { toks, at, kind } => {
                if (at as usize) < toks.len() {
                    let t = toks[at as usize];
                    return Some((t, below.push(Src::Toks { toks, at: at + 1, kind }), None));
                }
                inp = below;
            }
            Src::Text { text, data, mut pos, mut line, mut line_end, mut eol, ending } => loop {
                let loaded = line_end != NOT_LOADED;
                if loaded {
                    let cur = Cur { data: &data, line_end, eol };
                    if let Some((t, from)) = lex(read, names, &cur, &mut pos, &mut line) {
                        let len = data.len() as u32;
                        let span = Span { text, from: from.min(len), to: pos.min(len) };
                        let s = Src::Text { text, data: data.clone(), pos, line, line_end, eol, ending };
                        return Some((t, below.push(s), Some(span)));
                    }
                }
                // tex.web 360/362: the next line, or the end of the text.
                let len = data.len();
                let start = if loaded {
                    match data[line_end as usize..].find('\n') {
                        Some(i) => line_end as usize + i + 1,
                        None => len,
                    }
                } else {
                    pos as usize
                };
                if ending || start >= len {
                    inp = below.clone();
                    if !ending {
                        let ee = read(&Key::TokPar(tp::of("everyeof"))).toks();
                        if !ee.is_empty() {
                            inp = inp.push(Src::Toks { toks: ee, at: 0, kind: TokKind::EveryEof });
                        }
                    }
                    break;
                }
                let raw = data[start..].find('\n').map_or(len, |i| start + i);
                let mut end = raw;
                while end > start && matches!(data.as_bytes()[end - 1], b' ' | b'\t' | b'\r') {
                    end -= 1;
                }
                let e = read(&Key::IntPar(ip::of("endlinechar"))).int();
                eol = if (0..=255).contains(&e) { char::from_u32(e as u32) } else { None };
                pos = start as u32;
                line_end = end as u32;
                line = Line::NewLine;
            },
        }
    }
}
