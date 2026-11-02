//! web2c's character translation (TCX, `--translate-file`, as a `%&`
//! first line gives it: lib/texmfmp.c's `parse_first_line` and
//! `read_tcx_file`, tex.ch's §23–§24 changes). A TCX given at run time
//! takes the place of the format's tables: every byte a file's line holds
//! is made a character by `xord` (`input_line`), and a character written
//! to a file is its `xchr` byte where `xprn` says it is printable, else
//! its `^^` form (§49, §58–§59). pdfTeX only: XeTeX reads and writes
//! UTF-8.

use std::sync::Arc;

use phitex_defuse::ValueId;

use crate::run::{R, Run};

/// The tables a TCX makes: `xord`, `xchr`, `xprn` (tex.ch's defaults,
/// printable ASCII only, with the file's lines applied).
pub(crate) struct Tcx {
    xord: Box<[u8; 256]>,
    xchr: Box<[u8; 256]>,
    xprn: Box<[u8; 256]>,
}

impl Tcx {
    /// `read_tcx_file` on the defaults of tex.ch §23–§24 (no
    /// `-8bit`: the TCX says what prints).
    pub(crate) fn parse(bytes: &[u8]) -> Tcx {
        let (xord, xchr, xprn) = phitex_mf::mktex::tcx_tables(bytes, false);
        Tcx { xord, xchr, xprn }
    }

    /// `input_line`'s `buffer[i]:=xord[buffer[i]]` on a line's bytes.
    pub(crate) fn decode(&self, buf: &mut [u32]) {
        for c in buf {
            if let Ok(b) = u8::try_from(*c) {
                *c = u32::from(self.xord[usize::from(b)]);
            }
        }
    }

    /// A file's text (its bytes as characters) as its lines are read:
    /// each byte through `xord`.
    pub(crate) fn reread(&self, text: &str) -> String {
        text.chars()
            .map(|c| match u8::try_from(c) {
                Ok(b) if c != '\n' => char::from(self.xord[usize::from(b)]),
                _ => c,
            })
            .collect()
    }

    /// A written line's character `c` as `print` puts it in the file
    /// (§58–§59 with web2c's `xprn`): its `xchr` byte where printable,
    /// else its `^^` form.
    pub(crate) fn print_file_char(&self, out: &mut String, c: u32) {
        let Ok(b) = u8::try_from(c) else {
            out.push(char::from_u32(c).unwrap_or('?'));
            return;
        };
        if self.xprn[usize::from(b)] != 0 {
            out.push(char::from(self.xchr[usize::from(b)]));
        } else if b < 64 {
            out.push_str("^^");
            out.push(char::from(b + 64));
        } else if b < 128 {
            out.push_str("^^");
            out.push(char::from(b - 64));
        } else {
            let hex = |d: u8| char::from_digit(u32::from(d), 16).unwrap_or('0');
            out.push_str("^^");
            out.push(hex(b / 16));
            out.push(hex(b % 16));
        }
    }
}

/// The TCX a `%&` first line names (`parse_first_line`): the line split
/// at spaces into at most three parts, a first part not starting with `-`
/// a format's name (passed over), then `--translate-file=NAME`,
/// `-translate-file=NAME` or either with `NAME` as the next part.
/// `None` where the line names none.
#[must_use]
pub fn translate_file(first_line: &str) -> Option<String> {
    let rest = first_line.strip_prefix("%&")?;
    let rest = rest.trim_start_matches([' ', '\t']);
    let mut parts: Vec<&str> = Vec::new();
    let mut s = rest;
    while !s.is_empty() && parts.len() != 3 {
        let end = s.find(' ').unwrap_or(s.len());
        parts.push(&s[..end]);
        s = s[end..].trim_start_matches(' ');
    }
    let mut at = 0;
    if parts.first().is_some_and(|p| !p.starts_with('-')) {
        at = 1;
    }
    let p = *parts.get(at)?;
    let name = if p == "--translate-file" || p == "-translate-file" {
        *parts.get(at + 1)?
    } else {
        p.strip_prefix("--translate-file=")
            .or_else(|| p.strip_prefix("-translate-file="))?
    };
    let name = name.trim_end_matches(['\r', '\n']);
    (!name.is_empty()).then(|| {
        if name.contains('.') {
            name.to_owned()
        } else {
            format!("{name}.tcx")
        }
    })
}

impl Run<'_, '_> {
    /// The job's TCX ([`crate::Options::tcx`], the `tcx` field): `None`
    /// where none was given, the format's tables in force.
    pub(crate) fn tcx(&mut self) -> R<Option<Arc<Tcx>>> {
        if self.xe {
            return Ok(None);
        }
        let Some(x) = self.get("tcx")? else {
            return Ok(None);
        };
        Ok(Some(tables(&self.ops.tcx, self.v, x)))
    }

    /// `input_line`'s `xord` for the line on top, just reached (a file's
    /// line; a pseudo file's or `\read`'s are characters already).
    pub(crate) fn tcx_decode_top(&mut self) -> R<()> {
        let Some(s) = self.input.top_line() else {
            return Ok(());
        };
        if s.rescanned || s.chars || s.buf.iter().all(|&c| c < 0x80) {
            return Ok(());
        }
        let Some(t) = self.tcx()? else {
            return Ok(());
        };
        if let Some(s) = self.input.top_line_mut() {
            t.decode(&mut s.buf);
            s.chars = true;
        }
        Ok(())
    }
}

/// The tables of the `tcx` value `x`, parsed once per build.
fn tables(
    cell: &std::sync::Mutex<Option<(ValueId, Arc<Tcx>)>>,
    v: &phitex_defuse::Values,
    x: ValueId,
) -> Arc<Tcx> {
    let mut c = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &*c {
        Some((id, t)) if *id == x => Arc::clone(t),
        _ => {
            let bytes: Vec<u8> = crate::val::str_of(v, x)
                .chars()
                .map(|ch| u8::try_from(ch).unwrap_or(b'?'))
                .collect();
            let t = Arc::new(Tcx::parse(&bytes));
            *c = Some((x, Arc::clone(&t)));
            t
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_names() {
        assert_eq!(
            translate_file("%& --translate-file=il2-pl").as_deref(),
            Some("il2-pl.tcx")
        );
        assert_eq!(
            translate_file("%&latex -translate-file cp1250pl").as_deref(),
            Some("cp1250pl.tcx")
        );
        assert_eq!(
            translate_file("%&-translate-file=x.tcx").as_deref(),
            Some("x.tcx")
        );
        assert_eq!(translate_file("%&latex"), None);
        assert_eq!(translate_file("% --translate-file=x"), None);
    }

    #[test]
    fn il2_pl_maps_and_prints() {
        let t = Tcx::parse(b"0xb6 0xb1 % sacute\n0xa1 0x81\n");
        let mut b = vec![0xb6, 0x41, 0xa2];
        t.decode(&mut b);
        assert_eq!(b, [0xb1, 0x41, 0xa2]);
        let mut s = String::new();
        for c in [0xb1, 0x41, 0xb6, 9] {
            t.print_file_char(&mut s, c);
        }
        assert_eq!(s, "\u{b6}A^^b6^^I");
        assert_eq!(t.reread("\u{b6}A\n"), "\u{b1}A\n");
    }
}
