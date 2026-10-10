//! `JOB.synctex.gz`, written as a pure function of each page's glyph
//! origins: the records pdfTeX's `synctex.c` writes for runs of
//! characters (`x` records: a file's tag, a line, the run's end point
//! `h,v` in scaled points from the page's top left, the inch of TeX's
//! origin included), in sheets per page, gzipped as zlib's `gzopen(…,
//! "wb")` writes it.
//!
//! The source of the records is phitex-flow's page values: each page's
//! glyph origins (`h`, `v`, font, first character) and the node that made
//! them, with the node's file and line from the Build (agents Q and A are
//! adding both; CORE.md, "pdfTeX's PDF mode"). Until they are there no
//! file is written, and the CLI says so (`session::waiting_notes`).

#![allow(dead_code, reason = "waits on phitex-flow's page origins (agents Q and A)")]

use std::fmt::Write as _;

/// A run of characters on a page: where it ends (sp, from the page's top
/// left), and the input it came from (a tag of [`Sync::inputs`], a line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub tag: u32,
    pub line: u32,
    pub h: i32,
    pub v: i32,
}

/// What a `SyncTeX` file holds: the inputs by tag (from 1), each page's
/// records, the output's kind and the magnification.
#[derive(Clone, Debug, Default)]
pub struct Sync {
    pub inputs: Vec<String>,
    pub pages: Vec<Vec<Record>>,
    pub pdf: bool,
    pub mag: i32,
}

/// The `SyncTeX` text of `s` (synctex.c's preamble, sheets, postamble;
/// `!` anchors give the bytes written before them).
pub fn text(s: &Sync) -> String {
    let mut out = String::new();
    out.push_str("SyncTeX Version:1\n");
    for (k, name) in s.inputs.iter().enumerate() {
        let _ = writeln!(out, "Input:{}:{name}", k + 1);
    }
    let _ = writeln!(out, "Output:{}", if s.pdf { "pdf" } else { "dvi" });
    let _ = writeln!(out, "Magnification:{}", if s.mag > 0 { s.mag } else { 1000 });
    out.push_str("Unit:1\nX Offset:0\nY Offset:0\nContent:\n");
    for (k, page) in s.pages.iter().enumerate() {
        let _ = writeln!(out, "!{}", out.len());
        let _ = writeln!(out, "{{{}", k + 1);
        let mut last_v = None;
        for r in page {
            // (a `v` equal to the last record's is written `=`)
            if last_v == Some(r.v) {
                let _ = writeln!(out, "x{},{}:{},=", r.tag, r.line, r.h);
            } else {
                let _ = writeln!(out, "x{},{}:{},{}", r.tag, r.line, r.h, r.v);
            }
            last_v = Some(r.v);
        }
        let _ = writeln!(out, "}}{}", k + 1);
    }
    let _ = writeln!(out, "!{}", out.len());
    let _ = writeln!(out, "Postamble:\nCount:{}", s.pages.iter().map(Vec::len).sum::<usize>());
    let _ = writeln!(out, "!{}", out.len());
    out.push_str("Post scriptum:\n");
    out
}

/// `data` as one gzip member, as zlib's `gzopen(name, "wb")` writes it
/// (level 6, no name, no time, OS 3).
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let z = phitex_flow::pdf::zlib::compress(data, 6);
    // (the zlib stream's deflate data: without its 2-byte header and
    // 4-byte Adler-32)
    let raw = &z[2..z.len().saturating_sub(4)];
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    out.extend_from_slice(raw);
    out.extend_from_slice(&phitex_codecs::zlib::crc32(data).to_le_bytes());
    out.extend_from_slice(&u32::try_from(data.len() % (1 << 32)).unwrap_or(0).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_gzip() {
        let s = Sync {
            inputs: vec!["/d/a.tex".into()],
            pages: vec![vec![Record { tag: 1, line: 5, h: 100, v: 200 }, Record { tag: 1, line: 6, h: 300, v: 200 }]],
            pdf: true,
            mag: 1000,
        };
        let t = text(&s);
        assert!(t.contains("Input:1:/d/a.tex\n"));
        assert!(t.contains("{1\nx1,5:100,200\nx1,6:300,=\n}1\n"));
        let gz = gzip(t.as_bytes());
        assert_eq!(&gz[..3], &[0x1f, 0x8b, 8]);
        let mut back = Vec::new();
        assert!(phitex_codecs::inflate::inflate_raw(&gz[10..gz.len() - 8], &mut back));
        assert_eq!(back, t.as_bytes());
    }
}
