//! writet1.c's `load_enc_file` and writeenc.c's encoding table: the
//! glyph names of an encoding vector (`.enc`) file.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// `notdef`.
pub(crate) const NOTDEF: &[u8] = b".notdef";

/// The 256 glyph names of an encoding (`.notdef` where none).
pub(crate) type GlyphNames = Arc<Vec<Vec<u8>>>;

/// `enc_getline`'s lines: `append_char_to_buf` normalization, a line
/// break kept at the end; lines shorter than two bytes or starting with
/// `%` skipped.
fn enc_lines(data: &[u8]) -> Vec<Vec<u8>> {
    crate::fontmap::map_file_lines(data)
        .into_iter()
        .filter_map(|mut l| {
            l.push(b'\n');
            (l.len() >= 2 && l[0] != b'%').then_some(l)
        })
        .collect()
}

/// The glyph names in `data`, or what `pdftex_fail` says.
pub(crate) fn parse_enc(data: &[u8]) -> Result<Vec<Vec<u8>>, Vec<u8>> {
    let mut names = alloc::vec![NOTDEF.to_vec(); 256];
    let lines = enc_lines(data);
    let mut lines = lines.iter();
    let eof = || b"unexpected end of file".to_vec();
    let first = lines.next().ok_or_else(eof)?;
    let without_eol = |l: &[u8]| {
        let mut v = l.to_vec();
        if v.last() == Some(&b'\n') {
            v.pop();
        }
        v
    };
    let Some(open) = first
        .iter()
        .position(|&c| c == b'[')
        .filter(|_| first[0] == b'/')
    else {
        let mut m = b"invalid encoding vector (a name or `[' missing): `".to_vec();
        m.extend_from_slice(&without_eol(first));
        m.push(b'\'');
        return Err(m);
    };
    let mut line: &[u8] = first;
    let mut r = open + 1;
    let skip = |line: &[u8], r: &mut usize| {
        if line.get(*r) == Some(&b' ') {
            *r += 1;
        }
    };
    skip(line, &mut r);
    let mut count = 0usize;
    loop {
        while line.get(r) == Some(&b'/') {
            r += 1;
            let start = r;
            while let Some(&c) = line.get(r) {
                if matches!(c, b' ' | b'\n' | b']' | b'/') {
                    break;
                }
                r += 1;
            }
            let name = &line[start..r];
            skip(line, &mut r);
            if count > 255 {
                return Err(b"encoding vector contains more than 256 names".to_vec());
            }
            if name != NOTDEF {
                names[count] = name.to_vec();
            }
            count += 1;
        }
        let c = line.get(r).copied().unwrap_or(b'\n');
        if c != b'\n' && c != b'%' {
            if line[r..].starts_with(b"] def") {
                return Ok(names);
            }
            let mut m = b"invalid encoding vector: a name or `] def' expected: `".to_vec();
            m.extend_from_slice(&without_eol(&line[r..]));
            m.push(b'\'');
            return Err(m);
        }
        line = lines.next().ok_or_else(eof)?;
        r = 0;
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// writeenc.c's `get_fe_entry`: the encoding `name`, read (and
    /// logged) the first time.
    pub(crate) fn get_fe_entry(&mut self, name: &[u8]) -> Result<GlyphNames, Jump> {
        if let Some(e) = self.pdf.ship.encodings.get(&name.to_vec()) {
            return Ok(e.clone());
        }
        let found = self.host.read_file(name, FileKind::Enc);
        if T::VALUES {
            self.tracker
                .load(name, FileKind::Enc, found.as_ref().map(|f| &f.contents));
        }
        let Some(f) = found else {
            return self.pdftex_fail(Some(name), b"cannot open encoding file for reading");
        };
        self.print_str(b"{");
        self.print_str(&f.name);
        let names = match parse_enc(&f.contents) {
            Ok(n) => Arc::new(n),
            Err(m) => return self.pdftex_fail(Some(&f.name), &m),
        };
        self.print_str(b"}");
        self.pdf.ship.encodings.insert(name.to_vec(), names.clone());
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_encoding() {
        let data = b"% comment\n/TeXBase1Encoding [\n/.notdef /dotaccent\n/fi /fl\n% x\n/space /exclam\n] def\n";
        let n = parse_enc(data).unwrap();
        assert_eq!(n[0], NOTDEF);
        assert_eq!(n[1], b"dotaccent");
        assert_eq!(n[4], b"space");
        assert_eq!(n[6], NOTDEF);
        assert!(parse_enc(b"/X\n").is_err());
    }
}
