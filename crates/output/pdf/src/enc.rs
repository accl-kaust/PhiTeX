//! Encoding vectors (`writet1.c` `load_enc_file`, `writeenc.c`).

use crate::PdfError;

/// `append_char_to_buf` (`ptexmac.h`): tabs become blanks, carriage
/// returns and the end of the file become newlines, and a blank is kept
/// only after something that isn't a blank. Returns the character as
/// changed.
pub(crate) fn append_char(c: i32, buf: &mut Vec<u8>) -> i32 {
    let c = match c {
        9 => 32,
        13 | -1 => 10,
        c => c,
    };
    if c != 32 || buf.last().is_some_and(|&l| l != 32) {
        buf.push(u8::try_from(c & 0xff).unwrap_or(0));
    }
    c
}

/// `append_eol` (`ptexmac.h`): a newline at the end, and a blank before
/// it removed.
pub(crate) fn append_eol(buf: &mut Vec<u8>) {
    if buf.len() > 1 && buf.last() != Some(&10) {
        buf.push(10);
    }
    let n = buf.len();
    if n > 2 && buf[n - 2] == 32 {
        buf[n - 2] = 10;
        buf.pop();
    }
}

/// The 256 glyph names of an encoding; `None` is `.notdef`.
pub type GlyphNames = Vec<Option<Vec<u8>>>;

/// `load_enc_file` (`writet1.c`): the glyph names of a `.enc` file.
pub fn load_enc(bytes: &[u8]) -> Result<GlyphNames, PdfError> {
    let bad = |m: &str| PdfError::Font(format!("encoding file: {m}"));
    let mut pos = 0usize;
    let mut eof = false;
    // enc_getline
    let mut getline = |line: &mut Vec<u8>| -> Result<(), PdfError> {
        loop {
            if eof {
                return Err(bad("unexpected end of file"));
            }
            line.clear();
            loop {
                let c = if let Some(&b) = bytes.get(pos) {
                    pos += 1;
                    i32::from(b)
                } else {
                    eof = true;
                    -1
                };
                if append_char(c, line) == 10 {
                    break;
                }
            }
            append_eol(line);
            if !(line.len() < 2 || line[0] == b'%') {
                return Ok(());
            }
        }
    };
    let mut names: GlyphNames = vec![None; 256];
    let mut line = Vec::new();
    getline(&mut line)?;
    let Some(open) = line.iter().position(|&c| c == b'[') else {
        return Err(bad("invalid encoding vector (a name or `[' missing)"));
    };
    if line[0] != b'/' {
        return Err(bad("invalid encoding vector (a name or `[' missing)"));
    }
    let mut count = 0usize;
    let mut r = open + 1;
    if line.get(r) == Some(&b' ') {
        r += 1;
    }
    loop {
        while line.get(r) == Some(&b'/') {
            r += 1;
            let st = r;
            while !matches!(line.get(r), Some(b' ' | 10 | b']' | b'/') | None) {
                r += 1;
            }
            let name = line[st..r].to_vec();
            if line.get(r) == Some(&b' ') {
                r += 1;
            }
            if count > 255 {
                return Err(bad("encoding vector contains more than 256 names"));
            }
            if name != b".notdef" {
                names[count] = Some(name);
            }
            count += 1;
        }
        match line.get(r) {
            Some(10 | b'%') | None => {}
            _ => {
                if line[r..].starts_with(b"] def") {
                    return Ok(names);
                }
                return Err(bad("invalid encoding vector: a name or `] def' expected"));
            }
        }
        getline(&mut line)?;
        r = 0;
    }
}
