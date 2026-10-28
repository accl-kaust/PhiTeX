//! `JOB.synctex.gz` from the build's glyph origins
//! (docs/note-origins.md §5): a sheet per page, each line of glyphs
//! from one file an hbox record with its first word's line, each word in
//! it a void hbox record with its own line, so that `synctex edit` finds
//! a word's line and `synctex view` a line's words. Positions are the
//! writer's (sp from the page's top left, pdfTeX's sheet coordinates).
//!
//! The file is gzip members one after another (the preamble's, each
//! page's, the postamble's): a gzip reader reads them as one stream,
//! and after an edit only the pages whose sheets changed are compressed
//! again.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use phitex_tex::{Build, PageOrigins};

/// The members made last, by what their sheets were made from (the
/// page's origins and the lines of the files up to its last byte), with
/// their record counts.
#[derive(Default)]
pub struct Writer {
    pages: Vec<(Option<Arc<PageOrigins>>, u64, usize, Arc<[u8]>)>,
}

/// What page `p`'s sheet is made from besides its origins: the line
/// starts of each file it names up to its last byte there.
fn sheet_key(k: usize, p: &PageOrigins, lines: &[Option<Vec<usize>>]) -> u64 {
    let mut last: Vec<(u32, u32)> = Vec::new();
    for i in 0..p.len() {
        let f = p.file[i];
        if f == u32::MAX {
            continue;
        }
        match last.iter_mut().find(|x| x.0 == f) {
            Some(x) => x.1 = x.1.max(p.end[i]),
            None => last.push((f, p.end[i])),
        }
    }
    let mut h = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&k, &mut h);
    for (f, at) in last {
        if let Some(Some(ls)) = lines.get(f as usize) {
            let n = ls.partition_point(|&s| s <= at as usize);
            std::hash::Hash::hash(&(f, &ls[..n]), &mut h);
        }
    }
    std::hash::Hasher::finish(&h)
}

/// pt to sp (the origins' points are PDF points).
#[allow(clippy::cast_possible_truncation)]
fn sp(bp: f32) -> i64 {
    (f64::from(bp) * 72.27 / 72.0 * 65536.0).round() as i64
}

/// The line (from 1) of byte `at` of a text with line starts `starts`.
fn line(starts: &[usize], at: u32) -> usize {
    starts.partition_point(|&s| s <= at as usize).max(1)
}

/// Page `k`'s sheet (`k` from 1): its records' text and their count.
fn sheet(k: usize, p: &PageOrigins, lines: &[Option<Vec<usize>>]) -> (String, usize) {
    let mut out = String::new();
    let mut n = 0;
    // (the page's box around the lines: its tag and line the first's)
    let (mut lo_h, mut hi_h, mut lo_v, mut hi_v) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    let mut first: Option<(u32, usize)> = None;
    let line_of = |i: usize| -> Option<usize> {
        let f = p.file[i];
        let ls = lines.get(f as usize)?.as_ref()?;
        Some(line(ls, p.start[i]))
    };
    let mut i = 0;
    while i < p.len() {
        if p.file[i] == u32::MAX || p.size[i] == 0.0 {
            i += 1;
            continue;
        }
        // (a run: one file, one source line, one baseline, left to
        // right; its box's line is the run's, and each word in it an `x`
        // record, as pdfTeX's are)
        let f = p.file[i];
        let l = line_of(i).unwrap_or(1);
        let mut j = i + 1;
        while j < p.len()
            && (p.file[j] == u32::MAX || (p.file[j] == f && line_of(j).unwrap_or(1) == l))
            && (p.y[j] - p.y[i]).abs() < 0.01
            && p.x[j] >= p.x[j - 1]
        {
            j += 1;
        }
        let ks: Vec<usize> = (i..j).filter(|&k| p.file[k] == f && p.size[k] > 0.0).collect();
        let (Some(&a), Some(&z)) = (ks.first(), ks.last()) else {
            i = j;
            continue;
        };
        let size = p.size[a];
        let (h, v) = (sp(p.x[a]), sp(p.y[a]));
        let (ht, dp) = (sp(size * 0.7), sp(size * 0.2));
        let w = sp(p.x[z] + p.w[z]) - h;
        let tag = f + 1;
        first.get_or_insert((tag, l));
        (lo_h, hi_h) = (lo_h.min(h), hi_h.max(h + w));
        (lo_v, hi_v) = (lo_v.min(v - ht), hi_v.max(v + dp));
        let _ = writeln!(out, "({tag},{l}:{h},{v}:{w},{ht},{dp}");
        n += 1;
        // (its words: split where a gap is wider than a sixth of the size)
        let mut s = 0;
        while s < ks.len() {
            let mut e = s + 1;
            while e < ks.len() && p.x[ks[e]] <= p.x[ks[e - 1]] + p.w[ks[e - 1]] + p.size[ks[e - 1]] / 6.0 {
                e += 1;
            }
            let _ = writeln!(out, "x{tag},{l}:{},{v}", sp(p.x[ks[s]]));
            n += 1;
            s = e;
        }
        out.push_str(")\n");
        i = j;
    }
    let mut sheet = format!("{{{k}\n");
    if let Some((tag, l)) = first {
        let _ = writeln!(sheet, "[{tag},{l}:{lo_h},{hi_v}:{},{},0", hi_h - lo_h, hi_v - lo_v);
        sheet.push_str(&out);
        sheet.push_str("]\n");
        n += 1;
    }
    let _ = writeln!(sheet, "}}{k}");
    (sheet, n)
}

/// CRC-32 (gzip's), by table.
fn crc32(data: &[u8]) -> u32 {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, e) in t.iter_mut().enumerate() {
            let mut c = u32::try_from(n).unwrap_or(0);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// `data` as one gzip member.
fn gzip(data: &[u8]) -> Vec<u8> {
    let z = phitex_codecs::deflate::compress(data, 6);
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    // (the zlib stream's deflate data: its 2-byte header and Adler-32 off)
    out.extend_from_slice(&z[2..z.len() - 4]);
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(data.len() & 0xFFFF_FFFF)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    out
}

impl Writer {
    /// `JOB.synctex.gz` of build `b`, its files named from directory
    /// `dir`; `None` where the build kept no origins.
    pub fn write(&mut self, b: &Build, dir: &Path) -> Option<Vec<u8>> {
        let t0 = std::time::Instant::now();
        let snap = b.origins_snapshot();
        let (pages, files) = (&snap.pages, &snap.files);
        if pages.iter().all(Option::is_none) {
            return None;
        }
        phitex_tex::timeline::note("synctex: the pages' origins", t0.elapsed());
        let lines: Vec<Option<Vec<usize>>> = snap
            .texts
            .iter()
            .map(|t| {
                t.as_ref().map(|t| {
                    std::iter::once(0)
                        .chain(t.iter().enumerate().filter(|x| *x.1 == b'\n').map(|x| x.0 + 1))
                        .collect()
                })
            })
            .collect();
        let mut pre = String::from("SyncTeX Version:1\n");
        for (i, f) in files.iter().enumerate() {
            let p = dir.join(f);
            let _ = writeln!(pre, "Input:{}:{}", i + 1, p.display());
        }
        pre.push_str("Output:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n");
        let mut out = gzip(pre.as_bytes());
        let mut count = 0;
        let mut fresh = 0;
        self.pages.truncate(pages.len());
        for (k, p) in pages.iter().enumerate() {
            let h = p.as_ref().map_or(0, |p| sheet_key(k, p, &lines));
            // (the same answer, kept by the build while it is the same)
            let same = |q: &Option<Arc<PageOrigins>>| matches!((q, p), (Some(a), Some(b)) if Arc::ptr_eq(a, b));
            match self.pages.get(k) {
                Some((q, x, n, m)) if *x == h && same(q) => {
                    count += n;
                    out.extend_from_slice(m);
                }
                _ => {
                    fresh += 1;
                    let (text, n) = match p {
                        Some(p) => sheet(k + 1, p, &lines),
                        None => (format!("{{{0}\n}}{0}\n", k + 1), 0),
                    };
                    count += n;
                    let m: Arc<[u8]> = gzip(text.as_bytes()).into();
                    out.extend_from_slice(&m);
                    if k < self.pages.len() {
                        self.pages[k] = (p.clone(), h, n, m);
                    } else {
                        self.pages.push((p.clone(), h, n, m));
                    }
                }
            }
        }
        out.extend(gzip(
            format!("Postamble:\nCount:{count}\nPost scriptum:\n").as_bytes(),
        ));
        phitex_tex::timeline::note("synctex: the file", t0.elapsed());
        if std::env::var_os("PHITEX_ORIGINS_TIME").is_some() {
            eprintln!("synctex: {:?} in all, {fresh} of {} pages compressed", t0.elapsed(), pages.len());
        }
        Some(out)
    }
}
