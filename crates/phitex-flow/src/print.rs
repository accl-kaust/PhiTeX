//! TeX's printing of numbers, dimensions and glue (tex.web 65-69, 103,
//! 177-178), and of the log (tex.web 57-62, 1280, 1370), as pure functions.

use crate::val::{Glue, Tok};

/// A scaled value, rounded to five digits (tex.web 103).
pub fn scaled(out: &mut String, s: i32) {
    let mut s = i64::from(s);
    if s < 0 {
        out.push('-');
        s = -s;
    }
    let unity = 65536i64;
    out.push_str(&(s / unity).to_string());
    out.push('.');
    s = 10 * (s % unity) + 5;
    let mut delta = 10i64;
    loop {
        if delta > unity {
            s += 0o100_000 - 50000; // (round the last digit)
        }
        out.push(char::from(b'0' + (s / unity) as u8));
        s = 10 * (s % unity);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
}

/// A glue component with its order (tex.web 177).
fn glue_part(out: &mut String, d: i32, order: u8, unit: &str) {
    scaled(out, d);
    if order > 3 {
        out.push_str("foul");
    } else if order > 0 {
        out.push_str("fil");
        for _ in 1..order {
            out.push('l');
        }
    } else {
        out.push_str(unit);
    }
}

/// A glue specification (tex.web 178).
pub fn spec(g: &Glue, unit: &str) -> String {
    let mut out = String::new();
    scaled(&mut out, g.width);
    out.push_str(unit);
    if g.stretch != 0 {
        out.push_str(" plus ");
        glue_part(&mut out, g.stretch, g.stretch_order, unit);
    }
    if g.shrink != 0 {
        out.push_str(" minus ");
        glue_part(&mut out, g.shrink, g.shrink_order, unit);
    }
    out
}

/// Lowercase roman numerals; nothing for a nonpositive number (tex.web 69).
pub fn roman(out: &mut String, mut n: i32) {
    const S: &[u8] = b"m2d5c2l5x2v5i";
    let mut j = 0;
    let mut v = 1000;
    loop {
        while n >= v {
            out.push(char::from(S[j]));
            n -= v;
        }
        if n <= 0 {
            return;
        }
        let mut k = j + 2;
        let mut u = v / i32::from(S[k - 1] - b'0');
        if S[k - 1] == b'2' {
            k += 2;
            u /= i32::from(S[k - 1] - b'0');
        }
        if n + u >= v {
            out.push(char::from(S[k]));
            n += u;
        } else {
            j += 2;
            v /= i32::from(S[j - 1] - b'0');
        }
    }
}

/// `"` and uppercase hex digits (tex.web 67).
pub fn hex(out: &mut String, n: i32) {
    use std::fmt::Write as _;
    out.push('"');
    let _ = write!(out, "{n:X}");
}

/// A string as the tokens `\the` and friends make of it: characters of
/// category 12, spaces of category 10 (tex.web 464).
pub fn str_toks(s: &str) -> Vec<Tok> {
    s.chars().map(|c| if c == ' ' { Tok::Char(10, ' ') } else { Tok::Char(12, c) }).collect()
}

/// The longest line TeX writes to its log (`max_print_line`).
pub const MAX_PRINT_LINE: usize = 79;

/// The log TeX writes for these entries (`\message` texts, and
/// `\immediate\write` lines ending in `\n`), in batch mode, after `start`
/// on the current line: lines broken at 79 characters, a `\message` after
/// a space or on a line of its own if it is long (tex.web 1280), a written
/// line on lines of its own (tex.web 1370). Characters below 32 and 127
/// print as `^^` codes.
pub fn log(start: &str, entries: &[String], end: &str) -> String {
    let mut p = Printer { out: String::new(), offset: 0 };
    p.print(start);
    for e in entries {
        if let Some(raw) = e.strip_prefix(crate::show::RAW) {
            // (print operations: `print_ln`, `print_nl`, a space if the
            // line is not empty, characters)
            for c in raw.chars() {
                match c {
                    crate::show::LN => p.ln(),
                    crate::show::NL => {
                        if p.offset > 0 {
                            p.ln();
                        }
                    }
                    crate::show::SP => {
                        if p.offset > 0 {
                            p.char(' ');
                        }
                    }
                    _ => p.print(&c.to_string()),
                }
            }
        } else if let Some(line) = e.strip_suffix('\n') {
            if p.offset > 0 {
                p.ln();
            }
            p.print(line);
            p.ln();
        } else {
            // (in batch mode nothing reaches the terminal: `term_offset`
            // stays 0)
            if e.chars().count() > MAX_PRINT_LINE - 2 {
                p.ln();
            } else if p.offset > 0 {
                p.char(' ');
            }
            p.print(e);
        }
    }
    p.print(end);
    p.out
}

struct Printer {
    out: String,
    offset: usize,
}

impl Printer {
    fn ln(&mut self) {
        self.out.push('\n');
        self.offset = 0;
    }
    fn char(&mut self, c: char) {
        self.out.push(c);
        self.offset += 1;
        if self.offset == MAX_PRINT_LINE {
            self.ln();
        }
    }
    fn print(&mut self, s: &str) {
        for c in s.chars() {
            let k = u32::from(c);
            if k < 32 || k == 127 {
                self.char('^');
                self.char('^');
                self.char(char::from_u32(if k < 64 { k + 64 } else { k - 64 }).unwrap_or('?'));
            } else {
                self.char(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        let s = |v| {
            let mut o = String::new();
            scaled(&mut o, v);
            o
        };
        assert_eq!(s(65536), "1.0");
        assert_eq!(s(-32768), "-0.5");
        assert_eq!(s(1), "0.00002");
        assert_eq!(s(0x3fff_ffff), "16383.99998");
        let r = |v| {
            let mut o = String::new();
            roman(&mut o, v);
            o
        };
        assert_eq!(r(1984), "mcmlxxxiv");
        assert_eq!(r(0), "");
        assert_eq!(
            spec(&Glue { width: 65536, stretch: 65536, stretch_order: 2, ..Glue::default() }, "pt"),
            "1.0pt plus 1.0fill"
        );
    }
}
