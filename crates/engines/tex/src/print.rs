//! Printing (§54–§71, §103, §177–§178, §262–§263, §292–§299): the text
//! TeX would print, as character codes in a `String` (each `char` is one
//! code 0–255); unprintable codes and the new-line character are dealt
//! with where the text is finally laid out ([`crate::log`]).

use std::fmt::Write as _;

use phitex_typeset::glue::{Glue, Order};

use crate::val::{MATCH_CAT, OUT_PARAM_CAT, Tok};

pub(crate) fn push_code(out: &mut String, c: u32) {
    out.push(char::from_u32(c).unwrap_or('?'));
}

/// `print_esc` (§63).
pub(crate) fn print_esc(out: &mut String, name: &str, escape: i64) {
    if (0..256).contains(&escape) {
        push_code(out, u32::try_from(escape).unwrap_or(0));
    }
    out.push_str(name.strip_prefix('\u{10FFFE}').unwrap_or(name));
}

/// The name a control sequence prints as: frozen names without their
/// mark, a font identifier (§1257) as its `font_id_text`.
pub(crate) fn shown_name(name: &str) -> &str {
    match name.strip_prefix('\u{10FFFE}') {
        Some(n) => n.split_once('\u{2}').map_or(n, |(_, t)| t),
        None => name,
    }
}

/// `print_cs` (§262): `cat` gives the category code of a one-character
/// name's character.
pub(crate) fn print_cs(out: &mut String, t: &Tok, escape: i64, cat: &mut dyn FnMut(u32) -> i64) {
    match t {
        Tok::S(name) => {
            let n = shown_name(name);
            let mut chars = n.chars();
            match (chars.next(), chars.next()) {
                (None, _) => {
                    print_esc(out, "csname", escape);
                    print_esc(out, "endcsname", escape);
                    out.push(' ');
                }
                (Some(c), None) => {
                    print_esc(out, n, escape);
                    if cat(u32::from(c)) == 11 {
                        out.push(' ');
                    }
                }
                _ => {
                    print_esc(out, n, escape);
                    out.push(' ');
                }
            }
        }
        Tok::C(_) => push_code(out, t.chr()),
    }
}

/// `sprint_cs` (§263).
pub(crate) fn sprint_cs(out: &mut String, t: &Tok, escape: i64) {
    match t {
        Tok::S(name) => {
            let n = shown_name(name);
            if n.is_empty() {
                print_esc(out, "csname", escape);
                print_esc(out, "endcsname", escape);
            } else {
                print_esc(out, n, escape);
            }
        }
        Tok::C(_) => push_code(out, t.chr()),
    }
}

/// `show_token_list` (§292–§294) without a limit.
pub(crate) fn show_toks(out: &mut String, toks: &[Tok], escape: i64, cat: &mut dyn FnMut(u32) -> i64) {
    let mut match_chr = u32::from(b'#');
    let mut n = b'0';
    for t in toks {
        if t.is_cs_or_active() {
            print_cs(out, t, escape, cat);
            continue;
        }
        let c = t.chr();
        match t.cat().unwrap_or(0) {
            1..=4 | 7 | 8 | 10..=12 => push_code(out, c),
            6 => {
                push_code(out, c);
                push_code(out, c);
            }
            OUT_PARAM_CAT => {
                push_code(out, match_chr);
                if c <= 9 {
                    push_code(out, c + u32::from(b'0'));
                } else {
                    out.push('!');
                    return;
                }
            }
            MATCH_CAT => {
                match_chr = c;
                push_code(out, c);
                n += 1;
                out.push(char::from(n));
                if n > b'9' {
                    return;
                }
            }
            15 => out.push_str("->"),
            _ => print_esc(out, "BAD.", escape),
        }
    }
}

/// `print_int` (§65).
pub(crate) fn print_int(out: &mut String, n: i64) {
    let _ = write!(out, "{n}");
}

/// `print_scaled` (§103).
pub(crate) fn print_scaled(out: &mut String, s: i32) {
    let mut s = i64::from(s);
    if s < 0 {
        out.push('-');
        s = -s;
    }
    print_int(out, s / 65536);
    out.push('.');
    s = 10 * (s % 65536) + 5;
    let mut delta = 10;
    loop {
        if delta > 65536 {
            s += 0o100000 - 50000;
        }
        out.push(char::from(b'0' + u8::try_from(s / 65536).unwrap_or(0)));
        s = 10 * (s % 65536);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
}

/// `print_glue` (§177).
pub(crate) fn print_glue(out: &mut String, d: i32, o: Order, unit: &str) {
    print_scaled(out, d);
    match o {
        Order::Normal => out.push_str(unit),
        _ => {
            out.push_str("fil");
            for _ in 1..(o as usize) {
                out.push('l');
            }
        }
    }
}

/// `print_spec` (§178).
pub(crate) fn print_spec(out: &mut String, g: &Glue, unit: &str) {
    print_scaled(out, g.width);
    out.push_str(unit);
    if g.stretch != 0 {
        out.push_str(" plus ");
        print_glue(out, g.stretch, g.stretch_order, unit);
    }
    if g.shrink != 0 {
        out.push_str(" minus ");
        print_glue(out, g.shrink, g.shrink_order, unit);
    }
}

/// `print_roman_int` (§69).
pub(crate) fn print_roman_int(out: &mut String, n: i64) {
    let s = b"m2d5c2l5x2v5i";
    let mut j = 0usize;
    let mut v: i64 = 1000;
    let mut n = n;
    loop {
        while n >= v {
            out.push(char::from(s[j]));
            n -= v;
        }
        if n <= 0 {
            return;
        }
        let mut k = j + 2;
        let mut u = v / i64::from(s[k - 1] - b'0');
        if s[k - 1] == b'2' {
            k += 2;
            u /= i64::from(s[k - 1] - b'0');
        }
        if n + u >= v {
            out.push(char::from(s[k]));
            n += u;
        } else {
            j += 2;
            v /= i64::from(s[j - 1] - b'0');
        }
    }
}

/// `print_hex` (§67).
pub(crate) fn print_hex(out: &mut String, n: i64) {
    let _ = write!(out, "\"{n:X}");
}

/// What the meaning of a character command prints (§298 `chr_cmd`).
pub(crate) fn chr_cmd_name(c: u8) -> Option<&'static str> {
    Some(match c {
        1 => "begin-group character ",
        2 => "end-group character ",
        3 => "math shift character ",
        4 => "alignment tab character ",
        6 => "macro parameter character ",
        7 => "superscript character ",
        8 => "subscript character ",
        10 => "blank space ",
        11 => "the letter ",
        12 => "the character ",
        _ => return None,
    })
}
