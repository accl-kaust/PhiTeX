//! `JOB.synctex.gz`: the records of pdfTeX's `synctex.c` (`-synctex=1`),
//! made from phitex-flow's shipped pages (`phitex_flow::build::sync`: each
//! page's boxes, characters, kerns, glue, math nodes and rules where
//! shipping puts them, each with the source place of the node that made
//! it), and the two questions editors ask of it (`phitex sync --pdf`,
//! `--tex`; the viewer's double-click and forward search ask the same).
//!
//! The file is pdfTeX's format: a preamble (`Input:` lines, `Output:`,
//! `Magnification:`, `Unit:`, the offsets), `Content:` with a sheet per
//! page (`{k` … `}k`, `!` anchors giving the bytes since the last one),
//! the postamble (`Count:`). The records are pdfTeX's kinds (`[ ] ( ) v
//! h x k g $ r`); besides `synctex.c`'s `x` at the end of each run of
//! characters, each character has an `x` record of its own at its
//! reference point, with the line it was read on (pdfTeX's characters
//! take the place of the glue or kern before them). Input files are
//! numbered as first read (the main file 1), the numbers kept for the
//! session; names as pdfTeX writes them (absolute, `DIR/./NAME` for a file
//! found from the working directory).
//!
//! Incremental: a page whose box is the last build's and whose pieces
//! have the same runs keeps its records (their lines moved by the lines
//! inserted before their pieces); a page's sheet is compressed again only
//! if its text changed. The file is a gzip member per sheet (the
//! preamble, each page, the postamble), which zlib's `gzread` (the
//! `synctex` parser's) reads as one.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use phitex_flow::Build;
use phitex_flow::build::sync::{Dep, Kind, NOWHERE, PageSync};

/// One inch in scaled points, as `synctex.c` offsets DVI positions.
const DVI_OFFSET: i32 = 4_736_287;

/// Scaled points per big point.
const SP_PER_BP: f64 = 65_781.76;

/// A character on a page for the viewer: where (big points from the
/// page's top left, its reference point), and its input's tag and line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub x: f32,
    pub y: f32,
    pub tag: u32,
    pub line: u32,
}

/// What the last build's SyncTeX file says, for the viewer.
#[derive(Debug, Default)]
pub struct Doc {
    /// The inputs' names by tag (from 1: `inputs[tag - 1]`).
    pub inputs: Vec<String>,
    /// Each page's characters.
    pub pages: Vec<Arc<Vec<Glyph>>>,
}

/// A page as the last build wrote it.
struct Cached {
    /// The page's box (kept: a page of a later build with the same is
    /// this one).
    b: Arc<phitex_flow::item::BoxNode>,
    sync: Arc<PageSync>,
    /// Each dependency's tag and lines before its piece, as rendered.
    resolved: Vec<(u32, u32)>,
    body: Arc<Vec<u8>>,
    /// The records `synctex.c` counts (not the `x` ones).
    counted: usize,
    glyphs: Arc<Vec<Glyph>>,
    /// The sheet as compressed: its number, its first anchor's length, its
    /// body's identity, the gzip member.
    member: Option<(usize, usize, usize, Arc<Vec<u8>>)>,
}

#[derive(Default)]
struct Cache {
    tags: Vec<String>,
    tag_of: HashMap<Option<Arc<str>>, u32>,
    pages: Vec<Cached>,
    pre: Option<(Vec<u8>, Arc<Vec<u8>>)>,
    doc: Option<Arc<Doc>>,
}

/// What the last write took.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub ms: f64,
    pub pages: usize,
    /// Pages walked again, and sheets compressed again.
    pub walked: usize,
    pub compressed: usize,
}

/// A session's SyncTeX file, kept between its builds.
#[derive(Default)]
pub struct Writer {
    cache: Mutex<Cache>,
    stats: Mutex<Stats>,
}

/// The name `synctex.c` gives an input found as `found`.
fn input_name(cwd: &Path, found: &str) -> String {
    if found.starts_with('/') {
        found.to_owned()
    } else {
        format!("{}/./{}", cwd.display(), found.trim_start_matches("./"))
    }
}

impl Writer {
    /// What the last write took.
    pub fn stats(&self) -> Stats {
        *self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The last file's records, for the viewer.
    pub fn doc(&self) -> Option<Arc<Doc>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner).doc.clone()
    }

    /// The SyncTeX file of what `build` last shipped (job `jobname`, main
    /// file `main`, the files the build read `files_read`): its name and
    /// bytes; `threads` compress.
    pub fn file(
        &self,
        build: &Build,
        jobname: &str,
        main: &Path,
        files_read: &[(String, Option<String>)],
        threads: usize,
    ) -> (String, Vec<u8>) {
        let t0 = Instant::now();
        let cwd = std::env::current_dir().unwrap_or_default();
        let mut guard = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let c = &mut *guard;
        // (tags: the main file 1, then the files as first read, kept)
        if c.tags.is_empty() {
            c.tags.push(input_name(&cwd, &main.to_string_lossy()));
            c.tag_of.insert(None, 1);
        }
        for (name, found) in files_read {
            let Some(found) = found else { continue };
            let key = Some(Arc::<str>::from(name.as_str()));
            if !c.tag_of.contains_key(&key) {
                c.tags.push(input_name(&cwd, found));
                c.tag_of.insert(key, c.tags.len() as u32);
            }
        }
        let pieces: HashMap<u32, phitex_flow::build::sync::SyncPiece> =
            build.sync_pieces().into_iter().map(|p| (p.id, p)).collect();
        let n = build.page_count();
        let old: HashMap<usize, usize> = c.pages.iter().enumerate().map(|(i, p)| (Arc::as_ptr(&p.b) as usize, i)).collect();
        let mut olds: Vec<Option<Cached>> = std::mem::take(&mut c.pages).into_iter().map(Some).collect();
        let mut idx = None;
        let mut st = Stats { pages: n, ..Stats::default() };
        let mut pages: Vec<Cached> = Vec::with_capacity(n);
        let same_runs = |deps: &[Dep]| {
            deps.iter().all(|d| pieces.get(&d.id).and_then(|p| p.run.as_ref()).is_some_and(|r| Arc::ptr_eq(r, &d.run)))
        };
        for j in 0..n {
            let Some(b) = build.page_box(j) else { continue };
            let mut kept = old
                .get(&(Arc::as_ptr(&b) as usize))
                .and_then(|&i| olds[i].take())
                .filter(|o| Arc::ptr_eq(&o.b, &b) && same_runs(&o.sync.deps));
            let sync = match &kept {
                Some(o) => o.sync.clone(),
                None => {
                    st.walked += 1;
                    let idx = idx.get_or_insert_with(|| build.sync_index());
                    Arc::new(build.page_sync(j, idx).unwrap_or_default())
                }
            };
            let mut resolved = Vec::with_capacity(sync.deps.len());
            for d in &sync.deps {
                let (file, base) = pieces.get(&d.id).map_or((None, 0), |p| (p.file.clone(), p.base));
                let tag = if let Some(&t) = c.tag_of.get(&file) {
                    t
                } else {
                    let name = file.as_deref().unwrap_or("texput.tex");
                    c.tags.push(input_name(&cwd, name));
                    let t = c.tags.len() as u32;
                    c.tag_of.insert(file, t);
                    t
                };
                resolved.push((tag, base));
            }
            let page = match kept.take() {
                Some(o) if o.resolved == resolved => o,
                o => {
                    let (body, counted, glyphs) = render(&sync, &resolved);
                    Cached {
                        b,
                        sync,
                        resolved,
                        body: Arc::new(body),
                        counted,
                        glyphs: Arc::new(glyphs),
                        member: o.and_then(|o| o.member),
                    }
                }
            };
            pages.push(page);
        }
        let pdf = pages.first().is_none_or(|p| p.sync.pdf);
        let mag = pages.first().map_or(1000, |p| if p.sync.mag > 0 { p.sync.mag } else { 1000 });
        // (the preamble)
        let mut pre = String::from("SyncTeX Version:1\n");
        for (k, name) in c.tags.iter().enumerate() {
            let _ = writeln!(pre, "Input:{}:{name}", k + 1);
        }
        let off = if pdf { 0 } else { DVI_OFFSET };
        let _ = write!(
            pre,
            "Output:{}\nMagnification:{mag}\nUnit:1\nX Offset:{off}\nY Offset:{off}\nContent:\n",
            if pdf { "pdf" } else { "dvi" }
        );
        let pre = pre.into_bytes();
        let pre_gz = match &c.pre {
            Some((t, gz)) if *t == pre => gz.clone(),
            _ => Arc::new(gzip(&pre)),
        };
        // (the sheets: each one's first anchor the bytes since the last)
        let mut lead = pre.len();
        let mut heads: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(n);
        let mut count = 0usize;
        let mut todo: Vec<usize> = Vec::new();
        for (j, p) in pages.iter().enumerate() {
            let k = j + 1;
            let a = format!("!{lead}\n");
            let open = format!("{{{k}\n");
            let since = a.len() + open.len() + p.body.len();
            let head = format!("{a}{open}").into_bytes();
            let tail = format!("!{since}\n}}{k}\n").into_bytes();
            lead = tail.len();
            count += 4 + p.counted;
            let same = p.member.as_ref().is_some_and(|(mk, ml, mb, _)| {
                *mk == k && *ml == head.len() && *mb == Arc::as_ptr(&p.body) as usize
            });
            if !same {
                todo.push(j);
            }
            heads.push((head, tail));
        }
        st.compressed = todo.len();
        let made = compress_all(&pages, &heads, &todo, threads);
        for (&j, gz) in todo.iter().zip(made) {
            pages[j].member = Some((j + 1, heads[j].0.len(), Arc::as_ptr(&pages[j].body) as usize, Arc::new(gz)));
        }
        // (the postamble: `synctex.c` counts its first anchor)
        count += 1;
        let first = format!("!{lead}\n");
        let cl = format!("Count:{count}\n");
        let post = format!("{first}Postamble:\n{cl}!{}\nPost scriptum:\n", first.len() + "Postamble:\n".len() + cl.len());
        let total = pre_gz.len() + pages.iter().map(|p| p.member.as_ref().map_or(0, |m| m.3.len())).sum::<usize>();
        let mut out = Vec::with_capacity(total + 64);
        out.extend_from_slice(&pre_gz);
        for p in &pages {
            if let Some(m) = &p.member {
                out.extend_from_slice(&m.3);
            }
        }
        out.extend_from_slice(&gzip(post.as_bytes()));
        c.doc = Some(Arc::new(Doc { inputs: c.tags.clone(), pages: pages.iter().map(|p| p.glyphs.clone()).collect() }));
        c.pre = Some((pre, pre_gz));
        c.pages = pages;
        st.ms = t0.elapsed().as_secs_f64() * 1e3;
        *self.stats.lock().unwrap_or_else(PoisonError::into_inner) = st;
        (format!("{jobname}.synctex.gz"), out)
    }
}

/// The sheets `todo` of `pages` gzipped, on up to `threads` threads.
fn compress_all(pages: &[Cached], heads: &[(Vec<u8>, Vec<u8>)], todo: &[usize], threads: usize) -> Vec<Vec<u8>> {
    let sheet = |j: usize| {
        let (h, t) = &heads[j];
        let mut s = Vec::with_capacity(h.len() + pages[j].body.len() + t.len());
        s.extend_from_slice(h);
        s.extend_from_slice(&pages[j].body);
        s.extend_from_slice(t);
        gzip(&s)
    };
    let threads = threads.clamp(1, 16).min(todo.len().max(1));
    if threads <= 1 || todo.len() < 4 {
        return todo.iter().map(|&j| sheet(j)).collect();
    }
    let chunk = todo.len().div_ceil(threads);
    std::thread::scope(|s| {
        let hs: Vec<_> = todo.chunks(chunk).map(|c| s.spawn(move || c.iter().map(|&j| sheet(j)).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

/// A page's records as text (`resolved`: each dependency's tag and the
/// lines before its piece), how many `synctex.c` counts, and its
/// characters for the viewer.
fn render(p: &PageSync, resolved: &[(u32, u32)]) -> (Vec<u8>, usize, Vec<Glyph>) {
    let mut s = String::with_capacity(p.recs.len() * 24);
    let mut counted = 0;
    let mut glyphs = Vec::new();
    let off = if p.pdf { 0 } else { DVI_OFFSET };
    let mag = if p.mag > 0 { f64::from(p.mag) / 1000.0 } else { 1.0 };
    for r in &p.recs {
        let at = if r.place == NOWHERE { None } else { resolved.get(r.place.dep as usize) };
        let (t, l) = at.map_or((1, 0), |&(tag, base)| (tag, base + r.place.line));
        let (h, v, w, ht, dp) = (r.h, r.v, r.w, r.ht, r.dp);
        match r.kind {
            Kind::HBox | Kind::VBox => {
                let c = if r.kind == Kind::HBox { '(' } else { '[' };
                let _ = writeln!(s, "{c}{t},{l}:{h},{v}:{w},{ht},{dp}");
                counted += 1;
            }
            Kind::EndH => {
                s.push_str(")\n");
                counted += 1;
            }
            Kind::EndV => {
                s.push_str("]\n");
                counted += 1;
            }
            Kind::VoidH | Kind::VoidV if at.is_some() => {
                let c = if r.kind == Kind::VoidH { 'h' } else { 'v' };
                let _ = writeln!(s, "{c}{t},{l}:{h},{v}:{w},{ht},{dp}");
                counted += 1;
            }
            Kind::Glyph | Kind::RunEnd if at.is_some() => {
                let _ = writeln!(s, "x{t},{l}:{h},{v}");
                if r.kind == Kind::Glyph {
                    #[allow(clippy::cast_possible_truncation)]
                    glyphs.push(Glyph {
                        x: (f64::from(h + off) * mag / SP_PER_BP) as f32,
                        y: (f64::from(v + off) * mag / SP_PER_BP) as f32,
                        tag: t,
                        line: l,
                    });
                }
            }
            Kind::Kern if at.is_some() => {
                let _ = writeln!(s, "k{t},{l}:{h},{v}:{w}");
                counted += 1;
            }
            Kind::Glue if at.is_some() => {
                let _ = writeln!(s, "g{t},{l}:{h},{v}");
                counted += 1;
            }
            Kind::Math if at.is_some() => {
                let _ = writeln!(s, "${t},{l}:{h},{v}");
                counted += 1;
            }
            Kind::Rule if at.is_some() => {
                let _ = writeln!(s, "r{t},{l}:{h},{v}:{w},{ht},{dp}");
                counted += 1;
            }
            _ => {}
        }
    }
    (s.into_bytes(), counted, glyphs)
}

/// zlib's CRC-32, by a table.
fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// `data` as one gzip member, as zlib's `gzopen(name, "wb")` writes it
/// (level 6, no name, no time, OS 3).
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let z = phitex_flow::pdf::zlib::compress(data, 6);
    // (the zlib stream's deflate data: without its 2-byte header and
    // 4-byte Adler-32)
    let raw = &z[2..z.len().saturating_sub(4)];
    let mut out = Vec::with_capacity(raw.len() + 18);
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3]);
    out.extend_from_slice(raw);
    out.extend_from_slice(&crc32(data).to_le_bytes());
    #[allow(clippy::cast_possible_truncation, reason = "gzip's ISIZE is the length mod 2^32")]
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// A gzip file's members decoded one after the other (none: not gzip,
/// or broken).
pub fn gunzip(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(b.len() * 6);
    let mut p = 0;
    while p < b.len() {
        if b.get(p..p + 3)? != [0x1f, 0x8b, 8] {
            return None;
        }
        let flags = *b.get(p + 3)?;
        let mut q = p + 10;
        if flags & 4 != 0 {
            let n = usize::from(u16::from_le_bytes([*b.get(q)?, *b.get(q + 1)?]));
            q += 2 + n;
        }
        for bit in [8u8, 16] {
            if flags & bit != 0 {
                q += b.get(q..)?.iter().position(|&c| c == 0)? + 1;
            }
        }
        if flags & 2 != 0 {
            q += 2;
        }
        let used = phitex_codecs::inflate::inflate_raw_len(b.get(q..)?, &mut out)?;
        p = q + used + 8;
    }
    Some(out)
}

// ---- reading a SyncTeX file: `phitex sync --pdf`, `--tex`

/// A record read back.
#[derive(Clone, Copy, Debug)]
struct R {
    kind: u8,
    tag: u32,
    line: u32,
    h: i32,
    v: i32,
    w: i32,
    ht: i32,
    dp: i32,
    /// The box it is in (an index into its page's records).
    parent: usize,
}

/// A SyncTeX file read back.
#[derive(Debug, Default)]
pub struct Parsed {
    inputs: Vec<(u32, String)>,
    xoff: i32,
    yoff: i32,
    mag: i32,
    unit: i32,
    pages: Vec<Vec<R>>,
}

/// The SyncTeX file's text `t` read.
pub fn parse(t: &str) -> Parsed {
    let mut p = Parsed { mag: 1000, unit: 1, ..Parsed::default() };
    let mut page: Option<Vec<R>> = None;
    let mut stack: Vec<usize> = Vec::new();
    let mut last_v = 0;
    let num = |s: &str| s.trim().parse::<i32>().unwrap_or(0);
    for l in t.lines() {
        if let Some(rest) = l.strip_prefix("Input:") {
            if let Some((tag, name)) = rest.split_once(':')
                && let Ok(tag) = tag.parse()
            {
                p.inputs.push((tag, name.to_owned()));
            }
        } else if let Some(v) = l.strip_prefix("X Offset:") {
            p.xoff = num(v);
        } else if let Some(v) = l.strip_prefix("Y Offset:") {
            p.yoff = num(v);
        } else if let Some(v) = l.strip_prefix("Magnification:") {
            p.mag = num(v).max(1);
        } else if let Some(v) = l.strip_prefix("Unit:") {
            p.unit = num(v).max(1);
        } else if l.starts_with('{') {
            page = Some(Vec::new());
            stack.clear();
        } else if l.starts_with('}') {
            if let Some(pg) = page.take() {
                p.pages.push(pg);
            }
        } else if let Some(pg) = page.as_mut() {
            let Some(&kind) = l.as_bytes().first() else { continue };
            if kind == b')' || kind == b']' {
                stack.pop();
                continue;
            }
            if !b"([vhxkg$r".contains(&kind) {
                continue;
            }
            // (`tag,line:h,v[:w[,ht,dp]]`, `=` a `v` like the last)
            let mut parts = l[1..].split(':');
            let (Some(tl), Some(hv)) = (parts.next(), parts.next()) else { continue };
            let Some((tag, line)) = tl.split_once(',') else { continue };
            let Some((h, v)) = hv.split_once(',') else { continue };
            let v = if v == "=" { last_v } else { num(v) };
            last_v = v;
            let dims: Vec<i32> = parts.next().map(|d| d.split(',').map(num).collect()).unwrap_or_default();
            let u = p.unit;
            pg.push(R {
                kind,
                tag: tag.parse().unwrap_or(0),
                line: line.parse().unwrap_or(0),
                h: num(h) * u,
                v: v * u,
                w: dims.first().copied().unwrap_or(0) * u,
                ht: dims.get(1).copied().unwrap_or(0) * u,
                dp: dims.get(2).copied().unwrap_or(0) * u,
                parent: stack.last().copied().unwrap_or(usize::MAX),
            });
            if kind == b'(' || kind == b'[' {
                stack.push(pg.len() - 1);
            }
        }
    }
    p
}

/// Reads a `.synctex.gz` (or `.synctex`) file.
pub fn read(path: &Path) -> Result<Parsed, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = if b.starts_with(&[0x1f, 0x8b]) {
        gunzip(&b).ok_or_else(|| format!("{}: not a gzip file phitex can read", path.display()))?
    } else {
        b
    };
    Ok(parse(&String::from_utf8_lossy(&text)))
}

/// An answer of [`Parsed::view`]: the page, the record's point and its
/// box (big points from the page's top left: `h` the left edge, `v` the
/// bottom, `ht` the height plus depth).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub h: f64,
    pub v: f64,
    pub w: f64,
    pub ht: f64,
}

impl Parsed {
    fn bp(&self, sp: i32, off: i32) -> f64 {
        f64::from(sp) * f64::from(self.mag) / 1000.0 / SP_PER_BP + f64::from(off) / SP_PER_BP
    }

    fn name(&self, tag: u32) -> Option<&str> {
        self.inputs.iter().find(|(t, _)| *t == tag).map(|(_, n)| n.as_str())
    }

    /// Inverse search: the source place (input name, line) of what is at
    /// `(x, y)` (big points from the top left) on page `page` (from 1):
    /// the record at or before it in the smallest line box (`hbox`) under
    /// it, else in the nearest one.
    pub fn edit(&self, page: usize, x: f64, y: f64) -> Vec<(String, u32)> {
        let Some(recs) = page.checked_sub(1).and_then(|k| self.pages.get(k)) else { return Vec::new() };
        let to_sp = |bp: f64, off: i32| {
            #[allow(clippy::cast_possible_truncation)]
            let s = ((bp - f64::from(off) / SP_PER_BP) * SP_PER_BP * 1000.0 / f64::from(self.mag)).round() as i64;
            s
        };
        let (px, py) = (to_sp(x, self.xoff), to_sp(y, self.yoff));
        let dist = |r: &R| {
            let (l, rt) = (i64::from(r.h), i64::from(r.h) + i64::from(r.w));
            let (t, b) = (i64::from(r.v) - i64::from(r.ht), i64::from(r.v) + i64::from(r.dp));
            let dx = if px < l { l - px } else if px > rt { px - rt } else { 0 };
            let dy = if py < t { t - py } else if py > b { py - b } else { 0 };
            (dy, dx)
        };
        let boxes: Vec<usize> = (0..recs.len()).filter(|&i| recs[i].kind == b'(').collect();
        let inside = boxes
            .iter()
            .copied()
            .filter(|&i| dist(&recs[i]) == (0, 0))
            .min_by_key(|&i| i64::from(recs[i].w) * (i64::from(recs[i].ht) + i64::from(recs[i].dp)));
        let Some(b) = inside.or_else(|| boxes.iter().copied().min_by_key(|&i| dist(&recs[i]))) else {
            return Vec::new();
        };
        let kids: Vec<&R> = recs.iter().filter(|r| r.parent == b && b"vhxkg$r".contains(&r.kind)).collect();
        let pick = kids.iter().rev().find(|r| i64::from(r.h) <= px).or_else(|| kids.first()).copied().unwrap_or(&recs[b]);
        self.name(pick.tag).map(|n| vec![(n.to_owned(), pick.line)]).unwrap_or_default()
    }

    /// The tags of input `file` (its name as written, the same file, else
    /// an input of the same base name).
    fn tags_of(&self, file: &str) -> Vec<u32> {
        let norm = |s: &str| s.replace("/./", "/");
        let canon = |s: &str| std::fs::canonicalize(s).ok();
        let f = norm(file);
        let fc = canon(file);
        let mut v: Vec<u32> = self
            .inputs
            .iter()
            .filter(|(_, n)| norm(n) == f || (fc.is_some() && canon(n) == fc))
            .map(|(t, _)| *t)
            .collect();
        if v.is_empty() {
            let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
            let b = base(file);
            v = self.inputs.iter().filter(|(_, n)| base(n) == b).map(|(t, _)| *t).collect();
        }
        v
    }

    /// Forward search: where line `line` of input `file` is shown, as
    /// `synctex view` answers: the hboxes holding its records (one up if
    /// that is in an hbox), its boxes' own records only if it has no
    /// others; else the nearest line's that has some, the next first; in
    /// page order.
    pub fn view(&self, file: &str, line: u32) -> Vec<Shown> {
        let tags = self.tags_of(file);
        if tags.is_empty() {
            return Vec::new();
        }
        let scale = f64::from(self.mag) / 1000.0 / SP_PER_BP;
        for d in 0..60i64 {
            // (the line, then one after, one before, two after…)
            let l = i64::from(line) + if d % 2 == 1 { d / 2 + 1 } else { -(d / 2) };
            if l < 1 {
                continue;
            }
            for boxes in [false, true] {
                let mut out: Vec<Shown> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for (k, recs) in self.pages.iter().enumerate() {
                    for (i, r) in recs.iter().enumerate() {
                        if !tags.contains(&r.tag) || i64::from(r.line) != l || (r.kind == b'(') != boxes || r.kind == b'[' {
                            continue;
                        }
                        // (the hbox holding it, or the one holding that)
                        let mut b = if r.kind == b'(' { i } else { r.parent };
                        while b < recs.len() && recs[b].kind != b'(' {
                            b = recs[b].parent;
                        }
                        if let Some(up) = recs.get(b).map(|x| x.parent).filter(|&p| recs.get(p).is_some_and(|x| x.kind == b'(')) {
                            b = up;
                        }
                        let bx = recs.get(b).unwrap_or(r);
                        if !seen.insert((k, b)) {
                            continue;
                        }
                        out.push(Shown {
                            page: k + 1,
                            x: self.bp(r.h, self.xoff),
                            y: self.bp(r.v, self.yoff),
                            h: self.bp(bx.h, self.xoff),
                            v: self.bp(bx.v + bx.dp, self.yoff),
                            w: f64::from(bx.w) * scale,
                            ht: f64::from(bx.ht + bx.dp) * scale,
                        });
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
        Vec::new()
    }
}

/// `phitex sync --pdf PAGE X Y` (inverse search: the source of what is at
/// `(X, Y)`, big points from the page's top left, on page `PAGE`) and
/// `phitex sync --tex FILE LINE` (forward search: where line `LINE` of
/// `FILE` is shown), answered from the job's `.synctex.gz` and printed as
/// TeX Live's `synctex edit` and `synctex view` print theirs. The file is
/// the job of `[MAIN.tex]` (else `phitex.toml`'s) in its output directory
/// (`-o`), or `--synctex PATH`. The exit status.
pub fn sync_main(args: &[String]) -> i32 {
    let mut pdf: Option<(usize, f64, f64)> = None;
    let mut tex: Option<(String, u32)> = None;
    let mut file: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut path: Option<PathBuf> = None;
    let mut it = args.iter();
    let bad = |m: &str| {
        eprintln!(
            "phitex sync: {m}\nusage: phitex sync --pdf PAGE X Y [MAIN.tex] | --tex FILE LINE [MAIN.tex] [-o DIR] [--synctex PATH]"
        );
        2
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pdf" => {
                let mut n = || it.next().map_or("", String::as_str);
                let (p, x, y) = (n().parse(), n().parse(), n().parse());
                match (p, x, y) {
                    (Ok(p), Ok(x), Ok(y)) => pdf = Some((p, x, y)),
                    _ => return bad("--pdf needs PAGE X Y"),
                }
            }
            "--tex" => {
                let f = it.next().cloned().unwrap_or_default();
                match it.next().and_then(|l| l.split(':').next()?.parse().ok()) {
                    Some(l) if !f.is_empty() => tex = Some((f, l)),
                    _ => return bad("--tex needs FILE LINE"),
                }
            }
            "-o" | "--output-dir" | "--output-directory" => out_dir = it.next().cloned(),
            "--synctex" => path = it.next().map(PathBuf::from),
            f if f.starts_with('-') => return bad(&format!("unknown option `{f}`")),
            f => file = Some(f.to_owned()),
        }
    }
    if pdf.is_none() && tex.is_none() {
        return bad("give --pdf PAGE X Y or --tex FILE LINE");
    }
    // (FILE as named from here, before the main file's directory is entered)
    let tex = tex.map(|(f, l)| (std::fs::canonicalize(&f).map_or(f, |p| p.to_string_lossy().into_owned()), l));
    let path = if let Some(p) = path {
        p
    } else {
        let o = crate::modern::Options { file, output_dir: out_dir, ..crate::modern::Options::default() };
        match crate::modern::resolve(&o) {
            Ok(t) => {
                let dir = t.output_dir.as_ref().map_or_else(|| PathBuf::from("."), PathBuf::from);
                let gz = dir.join(format!("{}.synctex.gz", t.job()));
                if gz.is_file() { gz } else { dir.join(format!("{}.synctex", t.job())) }
            }
            Err(e) => return bad(&e),
        }
    };
    let parsed = match read(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("phitex sync: {e} (`phitex build` writes it, unless --no-synctex)");
            return 1;
        }
    };
    let output = path.to_string_lossy().replace(".synctex.gz", ".pdf").replace(".synctex", ".pdf");
    let mut out = String::from("SyncTeX result begin\n");
    let mut any = false;
    if let Some((page, x, y)) = pdf {
        for (name, line) in parsed.edit(page, x, y) {
            any = true;
            let _ = write!(out, "Output:{output}\nInput:{name}\nLine:{line}\nColumn:-1\nOffset:0\nContext:\n");
        }
    }
    if let Some((f, line)) = tex {
        for s in parsed.view(&f, line) {
            any = true;
            let _ = write!(
                out,
                "Output:{output}\nPage:{}\nx:{:.6}\ny:{:.6}\nh:{:.6}\nv:{:.6}\nW:{:.6}\nH:{:.6}\nbefore:\noffset:-1\nmiddle:\nafter:\n",
                s.page, s.x, s.y, s.h, s.v, s.w, s.ht
            );
        }
    }
    out.push_str("SyncTeX result end\n");
    print!("{out}");
    i32::from(!any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phitex_flow::build::sync::{Place, Rec};

    fn rec(kind: Kind, line: u32, h: i32, v: i32, dims: [i32; 3]) -> Rec {
        Rec { kind, place: Place { dep: 0, line }, h, v, w: dims[0], ht: dims[1], dp: dims[2] }
    }

    /// The writer's format on a hand-made page: a vbox holding a line box
    /// with an empty indent box, two characters, glue, a kern, a math
    /// node and a rule (and glue with no place: no record); its piece has
    /// 10 lines of its file before it. Read back, a point finds its
    /// character's line, a line its box.
    #[test]
    fn a_hand_made_page() {
        let page = PageSync {
            pdf: true,
            mag: 1000,
            deps: Vec::new(),
            recs: vec![
                rec(Kind::VBox, 4, 100, 900, [500, 800, 0]),
                rec(Kind::HBox, 4, 100, 300, [500, 20, 5]),
                rec(Kind::VoidH, 2, 100, 300, [30, 0, 0]),
                rec(Kind::Glyph, 2, 130, 300, [10, 0, 0]),
                rec(Kind::Glyph, 3, 140, 300, [12, 0, 0]),
                rec(Kind::RunEnd, 3, 152, 300, [0; 3]),
                rec(Kind::Glue, 3, 160, 300, [0; 3]),
                rec(Kind::Kern, 3, 163, 300, [3, 0, 0]),
                rec(Kind::Math, 3, 163, 300, [0; 3]),
                rec(Kind::Rule, 3, 170, 300, [7, 9, 2]),
                Rec { place: NOWHERE, ..rec(Kind::Glue, 0, 171, 300, [0; 3]) },
                rec(Kind::EndH, 4, 170, 300, [0; 3]),
                rec(Kind::EndV, 4, 100, 900, [0; 3]),
            ],
        };
        let (body, counted, glyphs) = render(&page, &[(2, 10)]);
        let body = String::from_utf8(body).unwrap();
        assert_eq!(
            body,
            "[2,14:100,900:500,800,0\n(2,14:100,300:500,20,5\nh2,12:100,300:30,0,0\nx2,12:130,300\nx2,13:140,300\n\
             x2,13:152,300\ng2,13:160,300\nk2,13:163,300:3\n$2,13:163,300\nr2,13:170,300:7,9,2\n)\n]\n"
        );
        // (`synctex.c` counts all but the `x` records)
        assert_eq!(counted, 9);
        assert_eq!(glyphs.len(), 2);
        assert_eq!((glyphs[0].tag, glyphs[0].line), (2, 12));
        let text = format!(
            "SyncTeX Version:1\nInput:1:/d/./a.tex\nInput:2:/d/./b.tex\nOutput:pdf\nMagnification:1000\nUnit:1\n\
             X Offset:0\nY Offset:0\nContent:\n!0\n{{1\n{body}!0\n}}1\n"
        );
        let p = parse(&text);
        let bp = |sp: i32| f64::from(sp) / SP_PER_BP;
        assert_eq!(p.edit(1, bp(145), bp(298)), [("/d/./b.tex".to_owned(), 13)]);
        assert_eq!(p.edit(1, bp(135), bp(298)), [("/d/./b.tex".to_owned(), 12)]);
        let v = p.view("/d/b.tex", 12);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].page, 1);
        assert!((v[0].h - bp(100)).abs() < 1e-9 && (v[0].v - bp(305)).abs() < 1e-9, "{v:?}");
        // (gzip members one after the other read as one)
        let mut gz = gzip(b"abc ");
        gz.extend(gzip(b"def"));
        assert_eq!(gunzip(&gz).as_deref(), Some(&b"abc def"[..]));
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
