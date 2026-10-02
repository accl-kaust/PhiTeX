//! Part 2: The character set (§17–§24), as changed by web2c.

use alloc::string::String;
use alloc::vec::Vec;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
pub use crate::web::INVALID_CODE;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §21, §23, §24: initialize `xchr`, `xord` and `xprn`.
    ///
    /// web2c makes `xchr` the identity, and `xprn` marks printable
    /// characters: visible ASCII, or everything with `-8bit`; then a TCX
    /// file (`-translate-file`) changes them.
    pub(crate) fn init_charset(&mut self) {
        // §21, §23: identity mapping (visible ASCII and web2c's extension).
        for i in 0..=255u8 {
            self.xchr[usize::from(i)] = i;
        }
        // §24
        self.xord = [u8::try_from(INVALID_CODE).unwrap_or(0o177); 256];
        for i in 0o200..=0o377u8 {
            self.xord[usize::from(self.xchr[usize::from(i)])] = i;
        }
        for i in 0..=0o176u8 {
            self.xord[usize::from(self.xchr[usize::from(i)])] = i;
        }
        let eight_bit = self.params.eight_bit;
        for i in 0..=255u8 {
            self.xprn[usize::from(i)] = eight_bit || (b' '..=b'~').contains(&i);
        }
        // texmfmp.c's `readtcxfile`
        if let Some(t) = &self.params.translation {
            for &(first, second) in &t.codes {
                match second {
                    Some((second, printable)) => {
                        self.xord[usize::from(first)] = second;
                        self.xchr[usize::from(second)] = first;
                        self.xprn[usize::from(second)] = printable;
                    }
                    None => self.xprn[usize::from(first)] = true,
                }
            }
        }
    }
}

/// A TCX character translation file (web2c's `-translate-file`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Translation {
    /// The file's name as found (the log names it).
    pub name: Vec<u8>,
    /// Its lines in order: an external code, and the internal code with
    /// whether it is printable; or one code, made printable.
    pub codes: Vec<(u8, Option<(u8, bool)>)>,
}

partex_engine::persist_struct!(Translation { name, codes });

impl Translation {
    /// texmfmp.c's `readtcxfile` of the file `name` with contents
    /// `text`, and the warnings web2c prints on standard error.
    #[must_use]
    pub fn parse(name: &[u8], text: &[u8]) -> (Self, Vec<Vec<u8>>) {
        let mut t = Self {
            name: name.to_vec(),
            codes: Vec::new(),
        };
        let mut warnings = Vec::new();
        for (n, line) in (1..).zip(text.split(|&c| c == b'\n')) {
            // (a `%` starts a comment)
            let line = line.split(|&c| c == b'%').next().unwrap_or(&[]);
            let mut get = |s: &[u8], upb: i64| -> (Option<u8>, usize) {
                match strtol(s) {
                    None => {
                        if s.iter().any(|c| !c.is_ascii_whitespace()) {
                            warnings.push(
                                alloc::format!(
                                    "{}:{n}: Expected numeric constant, not `{}'.",
                                    String::from_utf8_lossy(name),
                                    String::from_utf8_lossy(s)
                                )
                                .into_bytes(),
                            );
                        }
                        (None, 0)
                    }
                    Some((v, used)) if !(0..=upb).contains(&v) => {
                        warnings.push(
                            alloc::format!(
                                "{}:{n}: Destination charcode {} <0 or >{upb}.",
                                String::from_utf8_lossy(name),
                                // (web2c keeps the number in an `int`)
                                i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
                            )
                            .into_bytes(),
                        );
                        (None, used)
                    }
                    Some((v, used)) => (u8::try_from(v).ok(), used),
                }
            };
            let (Some(first), used) = get(line, 255) else {
                continue;
            };
            let rest = &line[used..];
            let (second, used) = get(rest, 255);
            let code = match second {
                Some(second) => {
                    let (printable, _) = get(&rest[used..], 1);
                    // (not a number: printable; visible ASCII always is)
                    let printable =
                        printable.is_none_or(|p| p == 1) || (32..=126).contains(&second);
                    Some((second, printable))
                }
                None => None,
            };
            t.codes.push((first, code));
        }
        (t, warnings)
    }
}

/// C's `strtol(s, &end, 0)`: the number at the start of `s` (after white
/// space and a sign; `0x` for hexadecimal, `0` for octal), and how many
/// bytes it took; `None` if there is none.
pub(crate) fn strtol(s: &[u8]) -> Option<(i64, usize)> {
    let mut i = s
        .iter()
        .take_while(|c| c.is_ascii_whitespace() || **c == 0x0b)
        .count();
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let (radix, from) = match (s.get(i), s.get(i + 1), s.get(i + 2)) {
        (Some(b'0'), Some(b'x' | b'X'), Some(c)) if c.is_ascii_hexdigit() => (16, i + 2),
        (Some(b'0'), ..) => (8, i),
        _ => (10, i),
    };
    let digits = s[from..]
        .iter()
        .take_while(|c| char::from(**c).is_digit(radix))
        .count();
    if digits == 0 {
        return None;
    }
    let v = s[from..from + digits].iter().fold(0i64, |v, &c| {
        let d = i64::from(char::from(c).to_digit(radix).unwrap_or(0));
        v.saturating_mul(i64::from(radix)).saturating_add(d)
    });
    Some((if negative { -v } else { v }, from + digits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcx_lines() {
        let (t, w) = Translation::parse(
            b"x.tcx",
            b"% comment\n0x80 0xC7 1\n\n200\n0301 65 0 % comment\nzz\n300 256\n",
        );
        // (`0301 65 0`: visible ASCII stays printable; `zz` is not a
        // number; 300 is out of range)
        assert_eq!(
            t.codes,
            [
                (0x80, Some((0xC7, true))),
                (200, None),
                (0o301, Some((65, true)))
            ]
        );
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(strtol(b"  -12x"), Some((-12, 5)));
        assert_eq!(strtol(b"0x"), Some((0, 1)));
        assert_eq!(strtol(b"x"), None);
    }
}
