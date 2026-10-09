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
/// print as `^^` codes. An entry marked [`ENDJOB`] comes after `end` (the
/// main file's ` )`): what `\end` writes last (tex.web 1335).
pub fn log(start: &str, entries: &[String], end: &str) -> String {
    let mut p = Printer { out: String::new(), offset: 0 };
    p.print(start);
    let (late, now): (Vec<&String>, Vec<&String>) = entries.iter().partition(|e| e.starts_with(ENDJOB));
    for e in now {
        // (the hundredth error ends the log there, without `end`: §82)
        if p.entry(e) {
            return p.out;
        }
    }
    p.print(end);
    for e in late {
        if p.entry(&e[ENDJOB.len_utf8()..]) {
            break;
        }
    }
    p.out
}

/// Marks a log entry that goes after the main file's ` )`
/// (`final_cleanup`'s lines, tex.web 1335).
pub const ENDJOB: char = '\u{E006}';

impl Printer {
    /// One entry of the log; true if it ends the log (`show::HALT`).
    fn entry(&mut self, e: &str) -> bool {
        let p = self;
        if let Some(raw) = e.strip_prefix(crate::show::RAW) {
            // (print operations: `print_ln`, `print_nl`, a space if the
            // line is not empty, characters)
            // (an error's context line: its label's length, what was read,
            // what was not)
            let mut ctx: Option<(usize, String, Option<String>)> = None;
            for c in raw.chars() {
                if c == crate::show::CTX {
                    ctx = match ctx.take() {
                        None => Some((p.offset, String::new(), None)),
                        Some((l, r, None)) => Some((l, r, Some(String::new()))),
                        Some((l, r, Some(u))) => {
                            p.context(l, &r, &u);
                            None
                        }
                    };
                    continue;
                }
                if let Some((_, r, u)) = &mut ctx {
                    u.as_mut().unwrap_or(r).push(c);
                    continue;
                }
                match c {
                    crate::show::HALT => return true,
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
        false
    }
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

    /// tex.web 316–317: a context line after its label (`l` characters):
    /// what was read on the first line (its last `half_error_line`
    /// characters), what was not on the second, under it.
    fn context(&mut self, l: usize, read: &str, unread: &str) {
        const ERROR_LINE: usize = 79;
        const HALF_ERROR_LINE: usize = 50;
        let printed = |s: &str, out: &mut Vec<char>| {
            for c in s.chars() {
                let k = u32::from(c);
                if k < 32 || k == 127 {
                    out.push('^');
                    out.push('^');
                    out.push(char::from_u32(if k < 64 { k + 64 } else { k - 64 }).unwrap_or('?'));
                } else {
                    out.push(c);
                }
            }
        };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        printed(read, &mut a);
        printed(unread, &mut b);
        // (the pseudoprinting: `trick_buf` keeps what is printed while
        // `tally<trick_count`, at `tally mod error_line`)
        let mut buf = [' '; ERROR_LINE];
        let first_count = a.len();
        let trick_count = (first_count + 1 + ERROR_LINE - HALF_ERROR_LINE).max(ERROR_LINE);
        for (tally, &c) in a.iter().chain(&b).enumerate() {
            if tally < trick_count {
                buf[tally % ERROR_LINE] = c;
            }
        }
        let tally = a.len() + b.len();
        let m = if tally < trick_count { tally - first_count } else { trick_count - first_count };
        let (p, n) = if l + first_count <= HALF_ERROR_LINE {
            (0, l + first_count)
        } else {
            self.print("...");
            (l + first_count - HALF_ERROR_LINE + 3, HALF_ERROR_LINE)
        };
        for q in p..first_count {
            self.char(buf[q % ERROR_LINE]);
        }
        self.ln();
        for _ in 0..n {
            self.char(' ');
        }
        let p = if m + n <= ERROR_LINE { first_count + m } else { first_count + (ERROR_LINE - n - 3) };
        for q in first_count..p {
            self.char(buf[q % ERROR_LINE]);
        }
        if m + n > ERROR_LINE {
            self.print("...");
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
