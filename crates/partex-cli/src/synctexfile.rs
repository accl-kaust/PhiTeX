//! A `SyncTeX` file read back (DESIGN 4.5, 4.8): its inputs, and each
//! page's records that have a place in a file, where they are on the page.
//! The live viewer places a build's glyphs by them when the build has no
//! glyph origins of its own (machine mode): a double-click finds the line
//! of the record that ends the glyph's run, `phitex sync` the glyphs of a
//! line.

use std::collections::HashMap;

/// A record with a place: its file's tag, its line, and where it is on
/// the page (PDF points from the top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    pub tag: i32,
    pub line: i32,
    pub x: f64,
    pub y: f64,
}

/// A `SyncTeX` file's inputs by tag, and each page's records.
#[derive(Debug, Default)]
pub struct Sync {
    pub inputs: HashMap<i32, Vec<u8>>,
    pub pages: Vec<Vec<Record>>,
}

/// The text of a gzip file (one member, as zlib's `gzopen` writes it).
#[must_use]
pub fn gunzip(gz: &[u8]) -> Option<Vec<u8>> {
    if gz.len() < 18 || gz[..3] != [0x1f, 0x8b, 8] {
        return None;
    }
    let flags = gz[3];
    let mut at = 10;
    if flags & 4 != 0 {
        let n = usize::from(u16::from_le_bytes([*gz.get(at)?, *gz.get(at + 1)?]));
        at += 2 + n;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            at += gz.get(at..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let mut out = Vec::new();
    partex_engine::inflate::inflate_raw(gz.get(at..gz.len() - 8)?, &mut out).then_some(out)
}

/// The text of a `SyncTeX` file's bytes (gzipped or not).
#[must_use]
pub fn text(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        gunzip(bytes)
    } else {
        Some(bytes.to_vec())
    }
}

/// `tag,line:h,v` (`v` may be `=`, the last record's), what a record
/// holds after its kind.
fn place(rest: &[u8], last_v: i64) -> Option<(i32, i32, i64, i64)> {
    let text = std::str::from_utf8(rest).ok()?;
    let (tag_line, point) = text.split_once(':')?;
    let (tag, line) = tag_line.split_once(',')?;
    let point = point.split(':').next()?;
    let (h, v) = point.split_once(',')?;
    let v = if v == "=" { last_v } else { v.parse().ok()? };
    Some((tag.parse().ok()?, line.parse().ok()?, h.parse().ok()?, v))
}

/// Read a `SyncTeX` file's text: its inputs, and each sheet's records of
/// boxes, runs of characters, kerns, glue, math nodes and rules (not those
/// of forms), placed as pdfTeX places the page (`Magnification`, `Unit`,
/// its origin an inch from the top left).
#[must_use]
pub fn parse(text: &[u8]) -> Sync {
    let mut sync = Sync::default();
    let (mut mag, mut unit) = (1000.0, 1.0);
    let mut page: Option<usize> = None;
    let mut forms = 0usize;
    let mut last_v = 0i64;
    for line in text.split(|&c| c == b'\n') {
        let Some((&kind, rest)) = line.split_first() else {
            continue;
        };
        let num = |b: &[u8]| -> Option<f64> { std::str::from_utf8(b).ok()?.trim().parse().ok() };
        if let Some(r) = line.strip_prefix(b"Input:") {
            let mut parts = r.splitn(2, |&c| c == b':');
            if let (Some(t), Some(name)) = (parts.next(), parts.next())
                && let Some(t) = num(t)
            {
                #[allow(clippy::cast_possible_truncation, reason = "a tag")]
                sync.inputs.insert(t as i32, name.to_vec());
            }
            continue;
        }
        if let Some(r) = line.strip_prefix(b"Magnification:") {
            mag = num(r).unwrap_or(1000.0);
            continue;
        }
        if let Some(r) = line.strip_prefix(b"Unit:") {
            unit = num(r).unwrap_or(1.0);
            continue;
        }
        match kind {
            b'{' => {
                let n = num(rest).unwrap_or(0.0);
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a page"
                )]
                let k = (n as usize).saturating_sub(1);
                if sync.pages.len() <= k {
                    sync.pages.resize_with(k + 1, Vec::new);
                }
                page = Some(k);
            }
            b'}' => page = None,
            b'<' => forms += 1,
            b'>' => forms = forms.saturating_sub(1),
            b'x' | b'k' | b'g' | b'$' | b'(' | b'[' | b'h' | b'v' | b'r' => {
                let Some((tag, line_no, h, v)) = place(rest, last_v) else {
                    continue;
                };
                last_v = v;
                let (Some(at), 0) = (page, forms) else {
                    continue;
                };
                // (scaled points to PDF points, from the page's origin)
                #[allow(clippy::cast_precision_loss, reason = "points")]
                let pt = |d: i64| d as f64 * unit * mag / 1000.0 / 65536.0 * 72.0 / 72.27 + 72.0;
                sync.pages[at].push(Record {
                    tag,
                    line: line_no,
                    x: pt(h),
                    y: pt(v),
                });
            }
            _ => {}
        }
    }
    sync
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_by_page() {
        let text = b"SyncTeX Version:1\nInput:1:/d/a.tex\nInput:3:/d/b.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n!100\n{1\n[1,5:0,0:100,200,0\n(1,6:65536,=:10,10,0\nx3,7:131072,65536\n]\n}1\n{2\n<12\ng1,9:0,0\n>\nk1,10:0,0:5\n}2\n";
        let s = parse(text);
        assert_eq!(s.inputs.get(&3).map(Vec::as_slice), Some(&b"/d/b.tex"[..]));
        assert_eq!(s.pages.len(), 2);
        assert_eq!(s.pages[0].len(), 3);
        let x = s.pages[0][2];
        assert_eq!((x.tag, x.line), (3, 7));
        assert!((x.x - (2.0 * 72.0 / 72.27 + 72.0)).abs() < 1e-9);
        // (a compressed v is the last record's)
        assert!((s.pages[0][1].y - 72.0).abs() < 1e-9);
        // (a form's records are not the page's)
        assert_eq!(s.pages[1].len(), 1);
        assert_eq!(s.pages[1][0].line, 10);
    }
}
