//! Glyph origins (DESIGN 4.4): for every glyph a PDF page shows, the
//! source bytes it came from, for an editor that maps a click on the page
//! to the source and back.
//!
//! Use: [`Tex::set_origins`]`(true)` before the cold build (off by
//! default, and then nothing is recorded); after each build or rebuild,
//! once linked, [`Tex::origins`]`(page)` (or [`Tex::origin_pages`]) and
//! [`Tex::origin_files`]. PDF output only: a DVI job's pages have none.
//!
//! - **Order.** A page's glyphs are every character code a text-showing
//!   operator (`Tj`, `TJ`, `'`, `"`) shows in its content stream, in the
//!   stream's order; a form `XObject`'s codes come at each `Do` that draws
//!   it, again at each use, recursively (numbers in a `TJ` array are
//!   kerns, not glyphs). A code is one byte in a simple font (Type 1,
//!   TrueType, Type 3: all of pdfTeX's own); in a Type 0 font (an
//!   included page's) its `CMap`'s: two bytes with `Identity-H`/`-V`, an
//!   embedded `CMap`'s codespace ranges, two bytes with any other
//!   predefined `CMap` (`partex_engine::pdftext` counts them). Codes that
//!   no glyph of TeX's made get an entry with no source
//!   ([`GlyphOrigin::NONE`]: `file == u32::MAX`, `start == end == 0`,
//!   synthesized): text inside `\pdfliteral` and `\special{pdf:...}`,
//!   pdfTeX's fake and interword spaces, the text of an included PDF page
//!   (its forms counted too).
//! - **Recording.** When the main loop appends a character, its origin
//!   is that of the token it came from: a character read from a file is
//!   its bytes there (`^^` forms whole); a character token that came from
//!   a file inside a macro's argument keeps its bytes (each argument token
//!   carries its origin, through `back_input` and `\expandafter` too). A
//!   character from a macro body, `\the`, `\number`, `\romannumeral`, a
//!   counter (`\thesection`, `\thepage`) or `\char` is *synthesized*,
//!   its range the call's in the innermost file being read: from the
//!   start of the last command taken from that file (an expandable one
//!   expanded, or one main control executed) to the file's read position
//!   when the glyph was made (`\foo`, `\section{Intro}`). A ligature's
//!   range covers its characters (`fi`); a hyphen at a line break is
//!   synthesized, with the range of the character before it. A box used
//!   again (`\usebox`, `\copy`) shows its glyphs' origins again; math,
//!   rules and virtual fonts' packets (each code a glyph, with the origin
//!   of the character that drew them) need nothing more.
//! - **Files.** [`Tex::origin_files`] names each file as the job asked
//!   the host's `read_file` for it (`glyphs` for the job `glyphs.tex`,
//!   `article.cls`), in the order they were first opened.
//! - **Side channel.** Nodes and token lists hold handles into an
//!   [`OrgTable`], outside their equality and hashing: no version of the
//!   SSA runtime depends on an origin, and an origin never changes what is
//!   typeset or written. Off, the handles stay 0 and the engine never
//!   reads them.
//! - **Incremental.** An origin names a *data*, one version of a file's
//!   contents. A rebuild's edits replace datas (`ssa::rebuild`'s
//!   `Edit`s); an origin is mapped through them, byte by byte where an
//!   edited line kept its start or end, when it is asked for, and written
//!   back. A step that runs again makes fresh origins; one reused keeps
//!   its old ones, which map to where their text is now. Either way a
//!   rebuild's origins are a cold build's of the edited text.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::origin::{Org, OrgTable, Place};

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::{ACTIVE_BASE, CAT_CODE_BASE, ESCAPE, LETTER, SINGLE_BASE, SUP_MARK, TOKEN_LIST};

/// A glyph's origin: bytes `start..end` of file `file` (an index into
/// [`Tex::origin_files`]), in the file's current text; `synthesized` when
/// a macro body, `\the`, a counter or a break hyphen made the glyph, and
/// the range is the call's. `file == u32::MAX`: no source (see the
/// module's documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphOrigin {
    pub file: u32,
    pub start: u32,
    pub end: u32,
    pub synthesized: bool,
}

impl GlyphOrigin {
    /// A glyph with no source.
    pub const NONE: GlyphOrigin = GlyphOrigin {
        file: u32::MAX,
        start: 0,
        end: 0,
        synthesized: true,
    };
}

/// A content stream's glyphs, in the order it shows them (the effect
/// `Effect::Origins`): each an [`OrgTable`] handle (0: no source), a
/// form's glyphs where its `Do` is ([`FORM`]), or a run of glyphs with no
/// source ([`NONE_RUN`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamOrgs {
    /// 0 for a page, else the form's object number.
    pub form: i32,
    pub glyphs: Arc<[u32]>,
}

partex_engine::persist_struct!(StreamOrgs { form, glyphs });

/// [`StreamOrgs::glyphs`]: the form whose object number is the low bits.
pub const FORM: u32 = 0x8000_0000;
/// [`StreamOrgs::glyphs`]: as many glyphs with no source as the low bits.
pub const NONE_RUN: u32 = 0x4000_0000;

/// A data: a file's contents as a load found them.
#[derive(Clone)]
struct Data {
    bytes: Arc<[u8]>,
    file: u32,
    /// The edit that replaced it ([`OrgState::maps`]).
    next: Option<u32>,
}

/// An edit of a data, for origins: the data that replaced it and the
/// runs of changed lines, each with the bytes its old and new text share
/// at its start and its end.
#[derive(Clone)]
struct EditMap {
    new: u32,
    hunks: Vec<Hunk>,
}

#[derive(Clone, Copy)]
struct Hunk {
    of: usize,
    ot: usize,
    nf: usize,
    nt: usize,
    pre: usize,
    suf: usize,
}

impl EditMap {
    fn new(new: u32, old_bytes: &[u8], new_bytes: &[u8], hunks: &[[usize; 4]]) -> EditMap {
        let hunks = hunks
            .iter()
            .map(|&[of, ot, nf, nt]| {
                let (a, b) = (
                    old_bytes.get(of..ot).unwrap_or(&[]),
                    new_bytes.get(nf..nt).unwrap_or(&[]),
                );
                let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
                let room = a.len().min(b.len()) - pre;
                let suf = a
                    .iter()
                    .rev()
                    .zip(b.iter().rev())
                    .take(room)
                    .take_while(|(x, y)| x == y)
                    .count();
                Hunk {
                    of,
                    ot,
                    nf,
                    nt,
                    pre,
                    suf,
                }
            })
            .collect();
        EditMap { new, hunks }
    }

    /// Where the text that began at old byte `x` begins now (inside a
    /// changed line, the bytes its old and new text share at its start or
    /// end move with it; the rest goes to where the change begins).
    fn start(&self, x: usize) -> usize {
        // (the hunks that end at or before `x`, an insertion at `x` among
        // them: the text at `x` follows what was inserted)
        let k = self
            .hunks
            .partition_point(|h| if h.of < h.ot { h.ot <= x } else { h.of <= x });
        if let Some(h) = self.hunks.get(k)
            && h.of <= x
            && x < h.ot
        {
            let d = x - h.of;
            return if d < h.pre {
                h.nf + d
            } else if h.ot - x <= h.suf {
                h.nt - (h.ot - x)
            } else {
                h.nf + h.pre
            };
        }
        match k.checked_sub(1).and_then(|i| self.hunks.get(i)) {
            Some(h) => x - h.ot + h.nt,
            None => x,
        }
    }

    /// A range `[s, e)` now.
    fn range(&self, s: usize, e: usize) -> (usize, usize) {
        let s2 = self.start(s);
        let e2 = if e > s { self.start(e - 1) + 1 } else { s2 };
        (s2, e2.max(s2))
    }
}

/// The recording state and what the API reads (`Tex::org`).
#[derive(Clone)]
pub(crate) struct OrgState {
    pub(crate) table: OrgTable,
    datas: Vec<Data>,
    by_ptr: BTreeMap<usize, u32>,
    /// The files, by id: the name the job asked for; and each by the
    /// name the host found it by.
    files: Vec<Vec<u8>>,
    by_found: BTreeMap<Vec<u8>, u32>,
    maps: Vec<EditMap>,
    /// The SSA edits taken so far.
    edits_seen: usize,
    /// `macro_call`'s argument being scanned: its tokens' origins.
    pub(crate) args: Vec<Org>,
    /// The handle of the glyph the walk is drawing, and of the one the
    /// encoder is writing.
    pub(crate) cur: u32,
    pub(crate) emit: u32,
    /// The content streams being written, innermost last.
    streams: Vec<Vec<u32>>,
    /// Without a recorder: each page's glyphs, and each form's.
    pages: Vec<Arc<[u32]>>,
    forms: BTreeMap<i32, Arc<[u32]>>,
    /// Each file level's line: the data's address, the level's position
    /// after the line, and where the line begins.
    line_at: Vec<(usize, usize, usize)>,
    /// The origin of the character the main loop takes next, made before
    /// a scan that reads past it (`\char`).
    pub(crate) pending: Option<Org>,
    /// The streams of an SSA build, as last collected: the steps' chunk
    /// versions they were collected from, the pages, the forms.
    cached: Option<(u128, Streams)>,
    /// The codes each included PDF page shows, counted once: the file's
    /// data, the page.
    images: Vec<(Arc<[u8]>, i32, usize)>,
    /// `XeTeX`: the XDV page being built: each glyph item's glyphs'
    /// handles (0: none), by the item's index in the page's items.
    xdv_items: BTreeMap<u32, Vec<u32>>,
    /// `XeTeX`: each TFM font xdvipdfmx draws through a virtual font, by
    /// name (none: drawn as it is); read the first time asked.
    xdv_vfs: BTreeMap<Vec<u8>, Option<Arc<XdvVf>>>,
    /// `XeTeX`: the TFM names xdvipdfmx's font maps have an entry for (it
    /// reads no virtual font for them), read the first time needed.
    xdv_mapped: Option<alloc::collections::BTreeSet<Vec<u8>>>,
}

/// `XeTeX`: a virtual font as xdvipdfmx plays it, for the count of the
/// glyphs a character draws: its fonts' names by number, in the order
/// defined (the first: the packets' font when they begin), and its
/// packets by character.
#[derive(Debug, Default)]
struct XdvVf {
    fonts: Vec<(u32, Vec<u8>)>,
    packets: BTreeMap<u32, Vec<u8>>,
}

impl XdvVf {
    /// vf.c's reading of `data` (`read_header`, `process_vf_file`): none if
    /// it ends early.
    fn parse(data: &[u8]) -> Option<XdvVf> {
        let mut p = 0;
        let mut take = |n: usize| {
            let b = data.get(p..p + n)?;
            p += n;
            Some(b)
        };
        let num = |b: &[u8]| b.iter().fold(0u32, |a, &c| (a << 8) | u32::from(c));
        let mut vf = XdvVf::default();
        if take(2)? == [247, 202] {
            let n = usize::from(take(1)?[0]);
            take(n + 8)?;
        }
        loop {
            let code = take(1).map_or(248, |b| b[0]);
            match code {
                243..=246 => {
                    let k = num(take(usize::from(code - 242))?);
                    take(12)?;
                    let (a, l) = {
                        let b = take(2)?;
                        (usize::from(b[0]), usize::from(b[1]))
                    };
                    take(a)?;
                    let mut name = take(l)?.to_vec();
                    if let Some(z) = name.iter().position(|&c| c == 0) {
                        name.truncate(z);
                    }
                    vf.fonts.push((k, name));
                }
                0..=241 => {
                    let ch = u32::from(take(1)?[0]);
                    take(3)?;
                    let pkt = take(usize::from(code))?;
                    if code > 0 {
                        vf.packets.insert(ch, pkt.to_vec());
                    }
                }
                242 => {
                    let len = num(take(4)?) as usize;
                    let ch = num(take(4)?);
                    take(4)?;
                    let pkt = take(len)?;
                    if len > 0 {
                        vf.packets.insert(ch, pkt.to_vec());
                    }
                }
                _ => return Some(vf),
            }
        }
    }
}

/// The pages' glyph lists, and the forms' by object number.
type Streams = (Vec<Arc<[u32]>>, BTreeMap<i32, Arc<[u32]>>);

fn addr(b: &Arc<[u8]>) -> usize {
    b.as_ptr() as usize
}

impl OrgState {
    fn new() -> Self {
        OrgState {
            table: OrgTable::new(),
            datas: Vec::new(),
            by_ptr: BTreeMap::new(),
            files: Vec::new(),
            by_found: BTreeMap::new(),
            maps: Vec::new(),
            edits_seen: 0,
            args: Vec::new(),
            cur: 0,
            emit: 0,
            streams: Vec::new(),
            pages: Vec::new(),
            forms: BTreeMap::new(),
            line_at: Vec::new(),
            pending: None,
            cached: None,
            images: Vec::new(),
            xdv_items: BTreeMap::new(),
            xdv_vfs: BTreeMap::new(),
            xdv_mapped: None,
        }
    }

    /// The id of the file found as `found`, asked for as `asked`.
    fn file(&mut self, asked: &[u8], found: &[u8]) -> u32 {
        if let Some(&f) = self.by_found.get(found) {
            return f;
        }
        let f = u32::try_from(self.files.len()).unwrap_or(u32::MAX);
        self.files.push(asked.to_vec());
        self.by_found.insert(found.to_vec(), f);
        f
    }

    /// The id of data `bytes`, a file found as `found` (made now if new).
    fn data(&mut self, bytes: &Arc<[u8]>, found: &[u8]) -> Option<u32> {
        if let Some(&d) = self.by_ptr.get(&addr(bytes)) {
            return Some(d);
        }
        if self.datas.len() > Org::MAX_DATA as usize {
            return None;
        }
        let file = self.file(found, found);
        Some(self.add_data(bytes, file))
    }

    fn add_data(&mut self, bytes: &Arc<[u8]>, file: u32) -> u32 {
        let d = u32::try_from(self.datas.len()).unwrap_or(u32::MAX);
        self.datas.push(Data {
            bytes: bytes.clone(),
            file,
            next: None,
        });
        self.by_ptr.insert(addr(bytes), d);
        d
    }

    /// The edits `edits` (old data, new data, the runs of changed lines'
    /// old and new byte ranges), in order: each data an origin names that
    /// one replaced is followed by the other.
    fn take_edits(&mut self, edits: crate::ssa::Edits) {
        for (old, new, hunks) in edits {
            let Some(&d) = self.by_ptr.get(&addr(&old)) else {
                continue;
            };
            if self.datas[d as usize].next.is_some() {
                continue;
            }
            let file = self.datas[d as usize].file;
            let n = if let Some(&n) = self.by_ptr.get(&addr(&new)) {
                n
            } else {
                if self.datas.len() > Org::MAX_DATA as usize {
                    continue;
                }
                self.add_data(&new, file)
            };
            if n == d {
                continue;
            }
            let m = EditMap::new(n, &old, &new, &hunks);
            self.datas[d as usize].next = Some(u32::try_from(self.maps.len()).unwrap_or(0));
            self.maps.push(m);
        }
    }

    /// Where `p` is now: through each edit that replaced its data.
    fn now(&self, mut p: Place) -> Place {
        let mut guard = 0;
        while let Some(m) = self
            .datas
            .get(p.data as usize)
            .and_then(|d| d.next)
            .and_then(|i| self.maps.get(i as usize))
        {
            let (s, e) = m.range(p.start as usize, p.end as usize);
            p.data = m.new;
            p.start = u32::try_from(s).unwrap_or(u32::MAX);
            p.end = u32::try_from(e).unwrap_or(u32::MAX);
            guard += 1;
            if guard > self.maps.len() {
                break;
            }
        }
        p
    }

    /// Glyph handle `h`'s origin now (the entry moved to the data that
    /// holds its text now, and written back).
    fn resolve(&mut self, h: u32) -> GlyphOrigin {
        let o = self.table.get(h);
        let Some(p) = self.table.place(o) else {
            return GlyphOrigin::NONE;
        };
        let q = self.now(p);
        if q != p {
            let moved = self.table.moved(o, q);
            if moved != o {
                self.table.set(h, moved);
            }
        }
        let Some(d) = self.datas.get(q.data as usize) else {
            return GlyphOrigin::NONE;
        };
        let len = u32::try_from(d.bytes.len()).unwrap_or(u32::MAX);
        let start = q.start.min(len);
        GlyphOrigin {
            file: d.file,
            start,
            end: q.end.clamp(start, len),
            synthesized: q.synthesized,
        }
    }

    /// The glyphs of stream `glyphs`, forms put in where they are drawn.
    fn expand(
        &mut self,
        glyphs: &[u32],
        forms: &BTreeMap<i32, Arc<[u32]>>,
        depth: usize,
        out: &mut Vec<GlyphOrigin>,
    ) {
        for &g in glyphs {
            if g & FORM != 0 {
                let n = i32::try_from(g & !FORM).unwrap_or(0);
                if depth < 64
                    && let Some(f) = forms.get(&n)
                {
                    let f = f.clone();
                    self.expand(&f, forms, depth + 1, out);
                }
            } else if g & NONE_RUN != 0 {
                let n = (g & !NONE_RUN) as usize;
                out.extend(core::iter::repeat_n(GlyphOrigin::NONE, n));
            } else {
                out.push(self.resolve(g));
            }
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Record glyph origins (DESIGN 4.4) from now on, or stop: before a
    /// build begins. Off by default, and then free.
    pub fn set_origins(&mut self, on: bool) {
        self.org = on.then(|| alloc::boxed::Box::new(OrgState::new()));
    }

    /// Whether glyph origins are recorded.
    #[must_use]
    #[inline]
    pub fn origins_on(&self) -> bool {
        self.org.is_some()
    }

    /// The files glyph origins name, by id ([`GlyphOrigin::file`]): each
    /// as the job asked the host's `read_file` for it.
    #[must_use]
    pub fn origin_files(&self) -> Vec<String> {
        self.org.as_ref().map_or_else(Vec::new, |o| {
            o.files
                .iter()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect()
        })
    }

    /// The origin of each glyph of PDF page `page` (0-based, in shipping
    /// order), in the order the page's content stream shows them (see
    /// [`srcmap`](self)'s documentation), after the last build or rebuild:
    /// ranges in the files' current text. Empty if there is no such page
    /// or origins are off.
    pub fn origins(&mut self, page: usize) -> Vec<GlyphOrigin> {
        let Some((pages, forms)) = self.origin_streams() else {
            return Vec::new();
        };
        let Some(glyphs) = pages.get(page).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(o) = self.org.as_deref_mut() {
            o.expand(&glyphs, &forms, 0, &mut out);
        }
        out
    }

    /// [`Tex::origins`] of every page.
    pub fn origin_pages(&mut self) -> Vec<Vec<GlyphOrigin>> {
        let Some((pages, forms)) = self.origin_streams() else {
            return Vec::new();
        };
        let Some(o) = self.org.as_deref_mut() else {
            return Vec::new();
        };
        pages
            .iter()
            .map(|g| {
                let mut out = Vec::new();
                o.expand(g, &forms, 0, &mut out);
                out
            })
            .collect()
    }

    /// The pages' and forms' glyph lists, with the edits made since the
    /// last time taken.
    fn origin_streams(&mut self) -> Option<Streams> {
        self.org.as_ref()?;
        let Some(ssa) = self.tracker.ssa() else {
            let o = self.org.as_deref()?;
            return Some((o.pages.clone(), o.forms.clone()));
        };
        let rec = ssa.rec.borrow();
        let o = self.org.as_deref_mut()?;
        let edits = crate::ssa::edits_from(&rec, o.edits_seen);
        o.edits_seen += edits.len();
        o.take_edits(edits);
        let chunks = crate::ssa::step_effects_as(&rec, true);
        let key =
            partex_ssa::Version::of(&chunks.iter().map(|(k, e)| (*k, e.0.0)).collect::<Vec<_>>()).0;
        if let Some((k, streams)) = &o.cached
            && *k == key
        {
            return Some(streams.clone());
        }
        let (mut pages, mut forms) = (Vec::new(), BTreeMap::new());
        for (_, c) in &chunks {
            for e in c.1.iter() {
                if let crate::effects::Effect::Origins(s) = e {
                    if s.form == 0 {
                        pages.push(s.glyphs.clone());
                    } else {
                        forms.insert(s.form, s.glyphs.clone());
                    }
                }
            }
        }
        o.cached = Some((key, (pages.clone(), forms.clone())));
        Some((pages, forms))
    }

    // ---- recording ----

    /// A file was opened by `\input`: asked for as `asked`, found as
    /// `found`, with contents `bytes`.
    pub(crate) fn origin_file_opened(&mut self, asked: &[u8], found: &[u8], bytes: &Arc<[u8]>) {
        let Some(o) = self.org.as_deref_mut() else {
            return;
        };
        let f = o.file(asked, found);
        if !o.by_ptr.contains_key(&addr(bytes)) && o.datas.len() <= Org::MAX_DATA as usize {
            o.add_data(bytes, f);
        }
    }

    /// The input level main control is about to read from, if it is a
    /// file's: the current one, or the file below token lists all read.
    fn fetch_level(&self) -> Option<crate::input::InStateRecord> {
        let c = &self.cur_input;
        if c.state != TOKEN_LIST {
            return (c.name > 19).then(|| c.clone());
        }
        if c.loc != crate::mem::NULL {
            return None;
        }
        for r in self.input_stack[..self.input_ptr].iter().rev() {
            if r.state != TOKEN_LIST {
                return (r.name > 19).then(|| r.clone());
            }
            if r.loc != crate::mem::NULL {
                return None;
            }
        }
        None
    }

    /// Main control takes its next command: if from a file, a call begins
    /// there (what a synthesized glyph's range starts at).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn origin_fetch(&mut self) {
        if self.org.is_some() {
            self.origin_fetch_on();
        }
    }

    #[inline(never)]
    fn origin_fetch_on(&mut self) {
        let Some(r) = self.fetch_level() else {
            return;
        };
        let index = crate::input::ux(r.index);
        let Some(at) = self.level_offset(&r) else {
            return;
        };
        if let Some(Some(f)) = self.input_file.get_mut(index) {
            f.call = at;
        }
    }

    /// Where file level `r` (of an `\input` file) reads next, in its
    /// data: its line's start plus its place in the line, or the next
    /// line's start once the line is read.
    fn level_offset(&mut self, r: &crate::input::InStateRecord) -> Option<usize> {
        let index = crate::input::ux(r.index);
        let f = self.input_file.get(index)?.as_ref()?;
        let (pos, len) = (f.pos, f.data.len());
        if r.loc > r.limit {
            return Some(pos);
        }
        let line = self.line_start(index)?;
        let k = usize::try_from(r.loc - r.start).ok()?;
        Some((line + k).min(len))
    }

    /// Where file level `index`'s line in the buffer begins in its data:
    /// before its position, past the line's end.
    fn line_start(&mut self, index: usize) -> Option<usize> {
        let f = self.input_file.get(index)?.as_ref()?;
        let (a, pos) = (addr(&f.data), f.pos);
        let o = self.org.as_deref_mut()?;
        if let Some(&(x, p, s)) = o.line_at.get(index)
            && x == a
            && p == pos
        {
            return Some(s);
        }
        let d = &f.data;
        let mut i = pos.min(d.len());
        // (the line's end: LF, CR or CRLF before the position)
        if i > 0 && d[i - 1] == b'\n' {
            i -= 1;
            if i > 0 && d[i - 1] == b'\r' {
                i -= 1;
            }
        } else if i > 0 && d[i - 1] == b'\r' {
            i -= 1;
        }
        while i > 0 && d[i - 1] != b'\n' && d[i - 1] != b'\r' {
            i -= 1;
        }
        if o.line_at.len() <= index {
            o.line_at.resize(index + 1, (0, 0, 0));
        }
        o.line_at[index] = (a, pos, i);
        Some(i)
    }

    /// The category of character `c`, unread (an origin is not a read).
    fn quiet_cat(&self, c: u32) -> i32 {
        self.peek_code(CAT_CODE_BASE, crate::input::ci(c))
    }

    /// The origin of the token just read from file level `cur_input`
    /// (`want`: the token, which a character's byte must agree with).
    fn file_token_org(&mut self, want: Option<i32>) -> Org {
        let Some((index, s, e)) = self.file_token_span(want) else {
            return Org::NONE;
        };
        let Some(f) = self.input_file.get(index).and_then(Option::as_ref) else {
            return Org::NONE;
        };
        let (data, found) = (f.data.clone(), f.name.clone());
        let Some(o) = self.org.as_deref_mut() else {
            return Org::NONE;
        };
        let Some(d) = o.data(&data, &found) else {
            return Org::NONE;
        };
        o.table.range(
            d,
            u32::try_from(s).unwrap_or(u32::MAX),
            u32::try_from(e).unwrap_or(u32::MAX),
            false,
        )
    }

    /// An expandable command is expanded (a macro called, a primitive):
    /// read from a file, a call begins at its token (what a synthesized
    /// glyph's range starts at).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn origin_expand(&mut self) {
        if self.org.is_some() {
            self.origin_expand_on();
        }
    }

    #[inline(never)]
    fn origin_expand_on(&mut self) {
        if self.cur_input.state == TOKEN_LIST {
            return;
        }
        let want = (self.cur_cs != 0).then_some(crate::web::CS_TOKEN_FLAG + self.cur_cs);
        if let Some((index, s, _)) = self.file_token_span(want)
            && let Some(Some(f)) = self.input_file.get_mut(index)
        {
            f.call = s;
        }
    }

    /// Where the token just read from file level `cur_input` is in its
    /// data: the file's index, the token's bytes (`want`: the token,
    /// which a character's byte must agree with).
    fn file_token_span(&mut self, want: Option<i32>) -> Option<(usize, usize, usize)> {
        let r = self.cur_input.clone();
        if r.name <= 19 {
            return None;
        }
        let index = crate::input::ux(r.index);
        let line = self.line_start(index)?;
        let (start, end) = (crate::input::ux(r.start), crate::input::ux(r.loc));
        if end <= start || end > self.buffer.len() {
            return None;
        }
        if let Some(t) = want
            && t >= crate::web::CS_TOKEN_FLAG
            && t - crate::web::CS_TOKEN_FLAG != self.cur_cs
        {
            return None;
        }
        let b = |i: usize| self.buffer[i];
        let tok_start =
            if self.cur_cs == 0 || (self.cur_cs >= ACTIVE_BASE && self.cur_cs < SINGLE_BASE) {
                // a character (an active one too): `^^` forms are longer
                let c = b(end - 1);
                if let Some(t) = want
                    && self.cur_cs == 0
                    && crate::web::tok_chr(t) != crate::input::ci(c)
                    && !(end >= start + 3 && b(end - 3) == b(end - 2))
                {
                    return None;
                }
                if end >= start + 4
                    && b(end - 4) == b(end - 3)
                    && self.quiet_cat(b(end - 4)) == SUP_MARK
                    && u8::try_from(b(end - 2)).is_ok_and(|x| x.is_ascii_hexdigit())
                    && u8::try_from(c).is_ok_and(|x| x.is_ascii_hexdigit())
                {
                    end - 4
                } else if end >= start + 3
                    && b(end - 3) == b(end - 2)
                    && self.quiet_cat(b(end - 3)) == SUP_MARK
                {
                    end - 3
                } else {
                    end - 1
                }
            } else {
                // a control sequence: back over its letters to the escape
                let mut k = end;
                while k > start && self.quiet_cat(b(k - 1)) == LETTER {
                    k -= 1;
                }
                if k == end && k > start {
                    // (a control symbol)
                    k -= 1;
                }
                if k > start && self.quiet_cat(b(k - 1)) == ESCAPE {
                    k -= 1;
                }
                k
            };
        let n = self.input_file.get(index)?.as_ref()?.data.len();
        let s = (line + tok_start - start).min(n);
        let e = (line + end - start).min(n).max(s);
        Some((index, s, e))
    }

    /// The origin of the token just read (`cur_tok` when `want`), where
    /// the input level it came from knows it: a file's bytes, or a list's
    /// entry for it (an argument's token). None otherwise.
    pub(crate) fn tok_org(&mut self, want: Option<i32>) -> Org {
        if self.org.is_none() {
            return Org::NONE;
        }
        if self.cur_input.state != TOKEN_LIST {
            return self.file_token_org(want);
        }
        let Some(l) = self.cur_input.list.as_deref() else {
            return Org::NONE;
        };
        let h = l.org();
        if h == 0 {
            return Org::NONE;
        }
        let i = if self.cur_input.loc == crate::mem::NULL {
            l.len().checked_sub(1)
        } else {
            crate::input::ux(self.cur_input.loc).checked_sub(1)
        };
        let Some(i) = i.filter(|&i| i < l.len()) else {
            return Org::NONE;
        };
        if want.is_some_and(|t| l[i] != t) {
            return Org::NONE;
        }
        let i = u32::try_from(i).unwrap_or(0);
        self.org
            .as_deref()
            .map_or(Org::NONE, |o| o.table.get(h + i))
    }

    /// A synthesized origin: the call in the innermost file being read,
    /// from where it last began (an expandable command read from the
    /// file and expanded, or main control taking a command from the file
    /// itself) to where the file is read now.
    pub(crate) fn call_org(&mut self) -> Org {
        if self.org.is_none() {
            return Org::NONE;
        }
        let r = if self.cur_input.state != TOKEN_LIST && self.cur_input.name > 19 {
            Some(self.cur_input.clone())
        } else {
            self.input_stack[..self.input_ptr]
                .iter()
                .rev()
                .find(|r| r.state != TOKEN_LIST && r.name > 19)
                .cloned()
        };
        let Some(r) = r else {
            return Org::NONE;
        };
        let Some(end) = self.level_offset(&r) else {
            return Org::NONE;
        };
        let index = crate::input::ux(r.index);
        let Some(f) = self.input_file.get(index).and_then(Option::as_ref) else {
            return Org::NONE;
        };
        let (data, found, call) = (f.data.clone(), f.name.clone(), f.call);
        let Some(o) = self.org.as_deref_mut() else {
            return Org::NONE;
        };
        let Some(d) = o.data(&data, &found) else {
            return Org::NONE;
        };
        let start = if call <= end { call } else { end };
        o.table.range(
            d,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
            true,
        )
    }

    /// The origin of the character the main loop takes now (`cur_chr`,
    /// just read): its token's, else synthesized.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn char_org(&mut self) -> Org {
        if self.org.is_none() {
            return Org::NONE;
        }
        self.char_org_on()
    }

    #[inline(never)]
    fn char_org_on(&mut self) -> Org {
        if let Some(p) = self.org.as_deref_mut().and_then(|o| o.pending.take()) {
            return p;
        }
        let o = self.tok_org(None);
        if o.is_none() { self.call_org() } else { o }
    }

    /// The origin of a character made from a number (`\char`), taken
    /// before the number is scanned: synthesized.
    pub(crate) fn char_num_org(&mut self) -> Org {
        if self.org.is_none() {
            return Org::NONE;
        }
        let o = self.tok_org(None);
        if o.is_none() {
            self.call_org()
        } else {
            OrgTable::synth(o)
        }
    }

    /// `\char` read: the origin of the character it makes, taken now,
    /// before its number is scanned (the main loop takes it next).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn char_num_pending(&mut self) {
        if self.org.is_none() {
            return;
        }
        let o = self.char_num_org();
        if let Some(st) = self.org.as_deref_mut() {
            st.pending = Some(o);
        }
    }

    /// A character glyph `c` of font `f` appended to the current list
    /// from `o` (the main loop's move).
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn push_glyph(&mut self, f: partex_engine::node::FontId, c: u8, o: Org) {
        if self.org.is_none() {
            self.nodes_mut().push_char(f, c);
        } else {
            self.push_glyph_on(f, c, o);
        }
    }

    #[inline(never)]
    fn push_glyph_on(&mut self, f: partex_engine::node::FontId, c: u8, o: Org) {
        // (the current list, read and written, as `nodes_mut`)
        let _ = self.nodes_mut();
        if let Some(st) = self.org.as_deref_mut() {
            self.cur_list.list.push_char_org(f, c, o, &mut st.table);
        }
    }

    /// The handle of origin `o` alone (a ligature's, a single glyph's).
    pub(crate) fn org_handle(&mut self, o: Org) -> u32 {
        match self.org.as_deref_mut() {
            Some(st) if !o.is_none() => st.table.push(o),
            _ => 0,
        }
    }

    /// The origin of entry `h`.
    pub(crate) fn org_at(&self, h: u32) -> Org {
        self.org.as_deref().map_or(Org::NONE, |o| o.table.get(h))
    }

    /// The union of `a` and `b` (a ligature's characters).
    pub(crate) fn org_union(&mut self, a: Org, b: Org) -> Org {
        match self.org.as_deref_mut() {
            Some(st) => st.table.union(a, b),
            None => Org::NONE,
        }
    }

    /// `back_input`'s token's origin, taken before the input moves.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "origins off: one test on the hottest paths, measured (DESIGN 4.4)"
    )]
    pub(crate) fn back_org(&mut self) -> Org {
        if self.org.is_none() {
            return Org::NONE;
        }
        self.back_org_on()
    }

    #[inline(never)]
    fn back_org_on(&mut self) -> Org {
        let t = self.cur_tok;
        self.tok_org(Some(t))
    }

    /// The list `p` (one token, backed up) given origin `o`.
    pub(crate) fn give_org(&mut self, p: &mut crate::tok::Tokens, o: &[Org]) {
        let Some(st) = self.org.as_deref_mut() else {
            return;
        };
        let h = st.table.push_list(o);
        if h != 0
            && let Some(l) = Arc::get_mut(p)
        {
            l.set_org(h);
        }
    }

    /// `macro_call` begins an argument: no tokens' origins yet.
    pub(crate) fn arg_orgs_clear(&mut self) {
        if let Some(o) = self.org.as_deref_mut() {
            o.args.clear();
        }
    }

    /// `macro_call` stores the token just read (`cur_tok`) in its
    /// argument: its origin with it (the argument's tokens stored without
    /// one before it, a delimiter's or a run taken at once, have none).
    pub(crate) fn arg_org(&mut self) {
        let t = self.cur_tok;
        let o = self.tok_org(Some(t));
        let n = self.arg_list.len();
        if let Some(st) = self.org.as_deref_mut() {
            st.args.resize(n, Org::NONE);
            st.args.push(o);
        }
    }

    /// `macro_call` made argument `v` of its `n` tokens (`braced`: the
    /// first and last, its braces, dropped): its tokens' origins with it.
    pub(crate) fn arg_orgs_give(&mut self, v: &mut crate::tok::Tokens, n: usize, braced: bool) {
        let Some(st) = self.org.as_deref_mut() else {
            return;
        };
        let mut orgs = core::mem::take(&mut st.args);
        orgs.resize(n, Org::NONE);
        let part = if braced && n >= 2 {
            &orgs[1..n - 1]
        } else {
            &orgs[..]
        };
        let h = st.table.push_list(part);
        if h != 0
            && let Some(l) = Arc::get_mut(v)
        {
            l.set_org(h);
        }
        orgs.clear();
        st.args = orgs;
    }

    // ---- shipping ----

    /// A content stream begins (a page's or a form's).
    pub(crate) fn origins_stream_begin(&mut self) {
        if let Some(o) = self.org.as_deref_mut() {
            o.streams.push(Vec::new());
            o.cur = 0;
            o.emit = 0;
        }
    }

    /// The content stream ends: its glyphs are the page's (`form` 0) or
    /// the form's.
    pub(crate) fn origins_stream_end(&mut self, form: i32) {
        let Some(o) = self.org.as_deref_mut() else {
            return;
        };
        let Some(g) = o.streams.pop() else {
            return;
        };
        let glyphs: Arc<[u32]> = g.into();
        if T::VALUES {
            if let Some(e) = &mut self.effects {
                e.push(crate::effects::Effect::Origins(StreamOrgs { form, glyphs }));
            }
        } else if form == 0 {
            o.pages.push(glyphs);
        } else {
            o.forms.insert(form, glyphs);
        }
    }

    /// The encoder writes a glyph whose origin is entry `h`.
    #[inline]
    pub(crate) fn origin_glyph(&mut self, h: u32) {
        if let Some(o) = self.org.as_deref_mut()
            && let Some(s) = o.streams.last_mut()
        {
            s.push(h & (NONE_RUN - 1));
        }
    }

    /// `XeTeX`: the origins of a native word's UTF-16 units `orgs` as one
    /// run of entries (as a glyph run's): the handle of the first, 0 if
    /// origins are off or none has a source.
    pub(crate) fn org_run(&mut self, orgs: &[Org]) -> partex_engine::origin::Side {
        let Some(st) = self.org.as_deref_mut() else {
            return partex_engine::origin::Side(0);
        };
        if orgs.iter().all(|o| o.is_none()) {
            return partex_engine::origin::Side(0);
        }
        let mut first = 0;
        for (i, &o) in orgs.iter().enumerate() {
            let h = st.table.push(o);
            if i == 0 {
                first = h;
            }
        }
        partex_engine::origin::Side(first)
    }

    /// `XeTeX`: the origins of native word `w`'s UTF-16 units (none each
    /// when it has none; empty when origins are off).
    pub(crate) fn native_orgs(&self, w: &partex_engine::native::NativeWord) -> Vec<Org> {
        let Some(st) = self.org.as_deref() else {
            return Vec::new();
        };
        let h = w.org.0;
        (0..w.text.len())
            .map(|i| match h {
                0 => Org::NONE,
                h => st.table.get(h + u32::try_from(i).unwrap_or(0)),
            })
            .collect()
    }

    /// `XeTeX`: glyph item `item` (its index in the page's items) of the
    /// XDV page being built draws glyphs whose origins are entries
    /// `handles` (0: none).
    pub(crate) fn origin_xdv_item(&mut self, item: usize, handles: Vec<u32>) {
        if let Some(o) = self.org.as_deref_mut() {
            o.xdv_items
                .insert(u32::try_from(item).unwrap_or(u32::MAX), handles);
        }
    }

    /// `XeTeX`: character `c` of TFM font `f`, its origin entry `h`, is
    /// item `item` of the XDV page being built: an origin for each glyph
    /// xdvipdfmx draws for it ([`Tex::xdv_char_glyphs`]).
    pub(crate) fn origin_xdv_char(&mut self, item: usize, f: i32, c: i32, h: u32) {
        if self.org.is_none() || !self.xdv() {
            return;
        }
        let n = self.xdv_char_glyphs(f, c);
        self.origin_xdv_item(item, alloc::vec![h; n]);
    }

    /// `XeTeX`: how many glyphs xdvipdfmx draws for character `c` of TFM
    /// font `f`: one, or as many as a virtual font's packet sets (`set`,
    /// `put`), each counted the same way in its own font, none for a
    /// character it has no packet for. A font is virtual as `dvi.c`'s
    /// `dvi_locate_font` finds it: no entry in its font maps (TeX Live's
    /// `dvipdfmx.cfg`: `pdftex.map`, `kanjix.map`, `ckx.map`; `pdf:mapline`
    /// and `pdf:mapfile` specials not seen) and a `.vf` file.
    pub(crate) fn xdv_char_glyphs(&mut self, f: i32, c: i32) -> usize {
        let name = self.font_name_bytes(f);
        self.vf_glyphs(&name, u32::try_from(c).unwrap_or(0), 0)
    }

    fn vf_glyphs(&mut self, name: &[u8], c: u32, depth: usize) -> usize {
        let Some(vf) = self.xdv_vf(name) else {
            return 1;
        };
        // (dvi_vf_init: "Virtual fonts nested too deeply!")
        if depth >= 16 {
            return 0;
        }
        let Some(pkt) = vf.packets.get(&c) else {
            return 0;
        };
        let font_of = |k: u32| vf.fonts.iter().find(|(n, _)| *n == k).map(|(_, f)| f);
        let mut font = vf.fonts.first().map(|(_, f)| f);
        let mut n = 0;
        let mut p = 0;
        let num = |p: &mut usize, k: usize| {
            let v = pkt
                .get(*p..*p + k)
                .map_or(0, |b| b.iter().fold(0u32, |a, &c| (a << 8) | u32::from(c)));
            *p += k;
            v
        };
        while let Some(&op) = pkt.get(p) {
            p += 1;
            let ch = match op {
                0..=127 => Some(u32::from(op)),
                128..=130 | 133..=135 => Some(num(&mut p, usize::from((op - 128) % 5 + 1))),
                132 | 137 => {
                    p += 8;
                    None
                }
                // (`right`, `w`, `x`, `down`, `y`, `z`: 1 to 4 bytes)
                143..=146 | 148..=151 | 153..=156 | 157..=160 | 162..=165 | 167..=170 => {
                    let first = match op {
                        143..=146 => 143,
                        148..=151 => 148,
                        153..=156 => 153,
                        157..=160 => 157,
                        162..=165 => 162,
                        _ => 167,
                    };
                    p += usize::from(op - first + 1);
                    None
                }
                171..=234 => {
                    font = font_of(u32::from(op - 171)).or(font);
                    None
                }
                235..=238 => {
                    let k = num(&mut p, usize::from(op - 234));
                    font = font_of(k).or(font);
                    None
                }
                239..=242 => {
                    let len = num(&mut p, usize::from(op - 238)) as usize;
                    p += len;
                    None
                }
                138 | 141 | 142 | 147 | 152 | 161 | 166 => None,
                // (`set4`, `put4` and the rest: xdvipdfmx stops)
                _ => break,
            };
            if let (Some(ch), Some(f)) = (ch, font) {
                let f = f.clone();
                n += self.vf_glyphs(&f, ch, depth + 1);
            }
        }
        n
    }

    /// The virtual font xdvipdfmx draws TFM font `name` through, if any.
    fn xdv_vf(&mut self, name: &[u8]) -> Option<Arc<XdvVf>> {
        if let Some(v) = self.org.as_deref()?.xdv_vfs.get(name) {
            return v.clone();
        }
        let mut file = name.to_vec();
        file.extend_from_slice(b".vf");
        let vf = self
            .host
            .read_file(&file, crate::host::FileKind::Vf)
            .filter(|_| !self.xdv_is_mapped(name))
            .and_then(|f| XdvVf::parse(&f.contents))
            .map(Arc::new);
        let o = self.org.as_deref_mut()?;
        o.xdv_vfs.insert(name.to_vec(), vf.clone());
        vf
    }

    /// Whether xdvipdfmx's font maps have an entry for TFM `name` (the
    /// first word of a line, `%` starting a comment).
    fn xdv_is_mapped(&mut self, name: &[u8]) -> bool {
        if self.org.as_deref().is_some_and(|o| o.xdv_mapped.is_none()) {
            let mut names = alloc::collections::BTreeSet::new();
            for map in [&b"pdftex.map"[..], b"kanjix.map", b"ckx.map"] {
                let Some(f) = self.host.read_file(map, crate::host::FileKind::FontMap) else {
                    continue;
                };
                for line in f.contents.split(|&c| c == b'\n' || c == b'\r') {
                    let line = line.split(|&c| c == b'%').next().unwrap_or(&[]);
                    if let Some(w) = line
                        .split(|&c| c == b' ' || c == b'\t')
                        .find(|w| !w.is_empty())
                    {
                        names.insert(w.to_vec());
                    }
                }
            }
            if let Some(o) = self.org.as_deref_mut() {
                o.xdv_mapped = Some(names);
            }
        }
        self.org
            .as_deref()
            .and_then(|o| o.xdv_mapped.as_ref())
            .is_some_and(|m| m.contains(name))
    }

    /// `XeTeX`: the XDV page is written, its glyph items in the order
    /// `log` gives (`DviWriter::take_native_log`): its glyphs' origins as
    /// the page's stream, one per glyph xdvipdfmx draws, in its order.
    pub(crate) fn origins_xdv_page(&mut self, log: &[u32]) {
        let Some(items) = self
            .org
            .as_deref_mut()
            .map(|o| core::mem::take(&mut o.xdv_items))
        else {
            return;
        };
        self.origins_stream_begin();
        for s in log {
            for &h in items.get(s).map_or(&[][..], Vec::as_slice) {
                if h == 0 {
                    self.origin_none(1);
                } else {
                    self.origin_glyph(h);
                }
            }
        }
        self.origins_stream_end(0);
    }

    /// The encoder writes `n` glyphs with no source.
    pub(crate) fn origin_none(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if let Some(o) = self.org.as_deref_mut()
            && let Some(s) = o.streams.last_mut()
        {
            let n = u32::try_from(n).unwrap_or(NONE_RUN - 1).min(NONE_RUN - 1);
            match s.last_mut() {
                Some(l)
                    if *l & NONE_RUN != 0 && *l & FORM == 0 && (*l & !NONE_RUN) + n < NONE_RUN =>
                {
                    *l += n;
                }
                _ => s.push(NONE_RUN | n),
            }
        }
    }

    /// The encoder draws page `page` of included PDF file `data`: the
    /// codes it shows, with no source.
    pub(crate) fn origin_image(&mut self, data: &Arc<[u8]>, page: i32) {
        let Some(o) = self.org.as_deref_mut() else {
            return;
        };
        let n = if let Some(&(_, _, n)) = o
            .images
            .iter()
            .find(|(d, p, _)| Arc::ptr_eq(d, data) && *p == page)
        {
            n
        } else {
            let n =
                partex_engine::pdftext::page_code_count(data, usize::try_from(page).unwrap_or(0));
            o.images.push((data.clone(), page, n));
            n
        };
        self.origin_none(n);
    }

    /// The encoder draws form `objnum` (`Do`).
    pub(crate) fn origin_form(&mut self, objnum: i32) {
        if let Some(o) = self.org.as_deref_mut()
            && let Some(s) = o.streams.last_mut()
        {
            s.push(FORM | (objnum.cast_unsigned() & !FORM));
        }
    }

    /// The handle of the glyph the walk draws now.
    #[inline]
    pub(crate) fn origin_cur(&self) -> u32 {
        self.org.as_deref().map_or(0, |o| o.cur)
    }

    /// The walk draws character `i` of run `g` (or a ligature, `lig`).
    #[inline]
    pub(crate) fn origin_walk(&mut self, h: u32) {
        if let Some(o) = self.org.as_deref_mut() {
            o.cur = h;
        }
    }

    /// The encoder is about to write the glyph of a drawn item whose
    /// origin is entry `h`.
    #[inline]
    pub(crate) fn origin_emit(&mut self, h: u32) {
        if let Some(o) = self.org.as_deref_mut() {
            o.emit = h;
        }
    }

    /// The encoder's glyph's handle.
    #[inline]
    pub(crate) fn origin_emitting(&self) -> u32 {
        self.org.as_deref().map_or(0, |o| o.emit)
    }

    /// The job's name (`\jobname`), once it is known.
    #[must_use]
    pub fn job_name_bytes(&self) -> Option<Vec<u8>> {
        let j = usize::try_from(self.job_name).ok().filter(|&j| j > 0)?;
        (j < self.str_ptr).then(|| self.str_bytes(j).to_vec())
    }

    /// Bytes the origins hold, roughly (a report).
    #[must_use]
    pub fn origins_bytes(&self) -> usize {
        self.org.as_deref().map_or(0, |o| {
            o.table.bytes()
                + o.pages.iter().map(|p| p.len() * 4).sum::<usize>()
                + o.forms.values().map(|p| p.len() * 4).sum::<usize>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_map_positions() {
        // "abc  def\nxyz\n" -> "abc def\nxyz\n": one changed line
        let old = b"abc  def\nxyz\n";
        let new = b"abc def\nxyz\n";
        let m = EditMap::new(1, old, new, &[[0, 9, 0, 8]]);
        assert_eq!(m.start(5), 4); // `d`, in the shared end
        assert_eq!(m.start(0), 0); // `a`, in the shared start
        assert_eq!(m.start(9), 8); // `x`, after the hunk
        assert_eq!(m.range(5, 8), (4, 7));
        // an insertion at 9: text there moves after it
        let m = EditMap::new(1, b"ab\ncd\n", b"ab\nNEW\ncd\n", &[[3, 3, 3, 7]]);
        assert_eq!(m.start(3), 7);
        assert_eq!(m.range(0, 2), (0, 2));
        assert_eq!(m.range(3, 5), (7, 9));
    }
}
