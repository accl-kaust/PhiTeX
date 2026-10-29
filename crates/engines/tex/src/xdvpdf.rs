//! XeTeX's PDF written from the shipped pages (docs/spec-xetex.md §6):
//! the file `xelatex` makes (xetex, then `xdvipdfmx -q -E`), with no XDV
//! written or read.
//!
//! The end node walks each page as XeTeX's `ship_out` does
//! (`phitex_dvi`'s writer, told to keep no bytes and to say each
//! operation instead) and xdvipdfmx's interpreter runs the operations
//! (`phitex_xdvipdfmx::api::Session::page_ops`): its registers, its
//! fonts' widths and its arithmetic place everything, so the PDF is the
//! one the XDV would give. The pages are a fold over one xdvipdfmx
//! session (its object numbers, font usage and object streams carry from
//! page to page), so the link keeps the session and the walk's writer
//! before each page; an edit goes on from the state before its first
//! changed page, the pages before it keep their bytes, and a build whose
//! pages are all the last one's keeps the last file. The end (the fonts,
//! the document's objects, the cross-reference stream) is written after
//! the last page, as xdvipdfmx writes it.
//!
//! The result is a [`phitex_pdf::Layout`] like pdfTeX's link: the pages'
//! chunks, the end, each object's place, so a watch writes update
//! sections and the exact file when idle (`phitex_pdf::Updater`).
//!
//! A watch's edit is linked live ([`XdvPdf::live`]): xdvipdfmx numbers
//! its objects in page order, so a page that gains or loses one would
//! renumber every page after it and the exact link interprets them all
//! again. Instead the pages whose operations changed since the last exact
//! file are written again after its last page, from the session it left
//! there (`Session::reship`: the page keeps its page object, its new
//! objects are numbered past the file's `/Size`), and the end is written
//! again; the update section holds those pages and what the end changed,
//! and the other pages are not walked or interpreted. A page is written
//! so where it is the page first written ([`PageState`], the specials
//! whose effect is on the document); else the exact link. The exact file
//! of a live layout is made when the watch is idle
//! ([`phitex_pdf::Layout::live`]), from the pages decoded and kept, and is
//! the base the next edits are linked live from.
//!
//! The files xdvipdfmx reads (its maps, fonts, the document's pictures)
//! are the host's, read through a log: a later link checks that each
//! answer is still the same before it keeps anything it made from them.

use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Stop, ValueId, Values};
use phitex_dvi::{DviWriter, Op};
use phitex_xdvipdfmx::api::{Options as DpxOptions, Session};
use phitex_xdvipdfmx::direct::PageState;
use phitex_xdvipdfmx::io::{Files, Format};
use phitex_xdvipdfmx::specials::Again;

use crate::exec::LockWait as _;
use crate::val::{int_of, str_of};

/// [`crate::XeTeX::pdf`]: the PDF made from the pages, as `xelatex` has
/// `xdvipdfmx -q -E -o NAME.pdf` make it of its XDV.
#[derive(Clone)]
pub struct XdvPdf {
    /// The PDF's name as xdvipdfmx is given it (`-o`): the font subsets'
    /// tags and `/ID` hash it.
    pub name: String,
    /// `SOURCE_DATE_EPOCH`: the PDF's dates and the seed of `/ID`.
    pub date: i64,
    /// dvipdfmx's files, a fresh view for each link (the document's
    /// pictures read as they are now).
    pub files: Arc<dyn Fn() -> Arc<dyn Files> + Send + Sync>,
    /// A watch's file, written as update sections: an edit's link writes
    /// the pages that changed again after the last exact file's, and the
    /// exact file is made when asked ([`phitex_pdf::Layout::live`]).
    pub live: bool,
}

/// What the link's state depends on beyond the pages: the values'
/// table, `\mag`, the preamble's comment, the fonts' numbering, the
/// PDF's name and date.
#[derive(Clone, PartialEq, Eq)]
struct Head {
    inst: u64,
    mag: i32,
    comment: String,
    order: Vec<i64>,
    name: String,
    date: i64,
}

/// A question xdvipdfmx asked the host's files.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Asked {
    Find(Vec<u8>, Format, Vec<u8>),
    Read(Vec<u8>),
    Filter(Vec<u8>, Vec<u8>, i32),
}

/// Its answer.
#[derive(Clone)]
enum Answer {
    Path(Option<Vec<u8>>),
    Bytes(Option<Arc<[u8]>>),
}

impl Answer {
    fn same(&self, o: &Answer) -> bool {
        match (self, o) {
            (Answer::Path(a), Answer::Path(b)) => a == b,
            (Answer::Bytes(a), Answer::Bytes(b)) => match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                (None, None) => true,
                _ => false,
            },
            _ => false,
        }
    }
}

/// The questions asked since the link's start was made, with their
/// answers.
type Log = Arc<Mutex<crate::FxHashMap<Asked, Answer>>>;

/// The host's files, each question and its answer logged.
struct Logged {
    files: Arc<dyn Files>,
    log: Log,
}

impl Files for Logged {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let r = self.files.find(name, format, progname);
        self.log.locked().insert(
            Asked::Find(name.to_vec(), format, progname.to_vec()),
            Answer::Path(r.clone()),
        );
        r
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        let r = self.files.read(path);
        self.log
            .locked()
            .insert(Asked::Read(path.to_vec()), Answer::Bytes(r.clone()));
        r
    }

    fn apply_filter(&self, template: &[u8], input: &[u8], version: i32) -> Option<Arc<[u8]>> {
        let r = self.files.apply_filter(template, input, version);
        self.log.locked().insert(
            Asked::Filter(template.to_vec(), input.to_vec(), version),
            Answer::Bytes(r.clone()),
        );
        r
    }
}

/// Whether every logged question has the same answer from `files`.
fn still(log: &Log, files: &dyn Files) -> bool {
    let asked: Vec<(Asked, Answer)> = log
        .locked()
        .iter()
        .map(|(q, a)| (q.clone(), a.clone()))
        .collect();
    asked.iter().all(|(q, a)| {
        let now = match q {
            Asked::Find(n, f, p) => Answer::Path(files.find(n, *f, p)),
            Asked::Read(p) => Answer::Bytes(files.read(p)),
            Asked::Filter(t, i, v) => Answer::Bytes(files.apply_filter(t, i, *v)),
        };
        now.same(a)
    })
}

/// A page as the walk ships it, decoded from its value once and kept
/// while the page is the same: its box, counts, margins, paper and
/// `\special`s (by the handle the walk asks for), so a page is walked
/// again (an exact file made at idle, [`phitex_pdf::Layout::live`])
/// without the values it came from.
struct Walk {
    /// The page's identity (`crate::origins` keeps its glyphs by it).
    id: u64,
    /// None: a page the walk doesn't ship (§641's).
    bx: Option<phitex_typeset::node::BoxNode>,
    counts: [i32; 10],
    offsets: (i32, i32),
    size: (i32, i32),
    specials: Vec<Option<Vec<u8>>>,
}

impl Walk {
    fn of(v: &Values, page: ValueId) -> Walk {
        let parts = crate::assemble::dvi_parts(v, page);
        let (bx, counts, specials) = match parts {
            Some((bx, counts, whats)) => {
                let n = u32::try_from(whats.len()).unwrap_or(u32::MAX);
                let specials = (0..n)
                    .map(|h| crate::assemble::special_of(v, &whats, h, true))
                    .collect();
                (Some(bx), counts, specials)
            }
            None => (None, [0; 10], Vec::new()),
        };
        Walk {
            id: page.raw(),
            bx,
            counts,
            offsets: crate::assemble::dvi_offsets(v, page),
            size: crate::assemble::xdv_page_size(v, page),
            specials,
        }
    }
}

/// The fonts' definitions by TeX's font number.
type Defs = phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>;

/// Page `p` walked on `w` (XeTeX's `ship_out`): its operations, none for
/// a page not shipped; with `origins` (the values' instance), its glyphs
/// kept by its identity (`crate::origins`).
fn walk(w: &mut DviWriter, p: &Walk, defs: &Defs, origins: Option<u64>) -> Option<Vec<Op>> {
    let bx = p.bx.as_ref()?;
    (w.h_offset, w.v_offset) = p.offsets;
    w.page_size = p.size;
    let fonts = |f: u32| crate::assemble::font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| p.specials.get(h as usize).cloned().flatten();
    let out = origins.map(|_| {
        let (r, out) = crate::origins::Recorder::new();
        w.set_tap(Some(Box::new(r)));
        out
    });
    let r = w.ship_out(bx, p.counts, &fonts, &mut wh);
    drop(w.set_tap(None));
    if let (Some(inst), Some(g)) = (origins, out.and_then(|o| o.locked().take())) {
        crate::origins::keep(inst, p.id, g);
    }
    let ops = w.take_ops();
    r.ok().map(|()| ops)
}

/// A page's operations but its fonts' definitions (which the page
/// writing a font first has: where it is first written, not what).
fn neutral(ops: &[Op]) -> impl Iterator<Item = &Op> {
    ops.iter()
        .filter(|o| !matches!(o, Op::FontDef { .. } | Op::NativeFontDef { .. }))
}

/// The exact file's link, a fold over one xdvipdfmx session: what the
/// file holds and what a later link goes on from.
struct Base {
    /// The pages, by value.
    ids: Vec<u64>,
    walks: Vec<Arc<Walk>>,
    /// Each page's operations (none: not shipped).
    ops: Vec<Option<Arc<[Op]>>>,
    /// [`PageState`] before each page, and after the last.
    states: Vec<PageState>,
    /// Before page `.0`: the session and the walk's writer.
    snaps: Vec<(usize, Session, DviWriter)>,
    /// Each page's bytes (empty: a page not shipped).
    chunks: Vec<Arc<[u8]>>,
    /// The session and the writer after the last page, before the end:
    /// where a page is written again ([`Session::reship`]).
    post: Session,
    post_writer: DviWriter,
    /// The file's `/Size`.
    size: u32,
    layout: phitex_pdf::Layout,
}

/// The link's state between builds.
struct Memo {
    head: Head,
    /// The session as [`Session::direct`] left it (the options, the
    /// config file and the font maps read), kept for a link from the
    /// first page.
    start: Arc<Mutex<Session>>,
    /// The files' questions since `start` was made.
    log: Log,
    /// The exact file's link.
    base: Option<Arc<Mutex<Base>>>,
    /// The last link: its pages and its file (the base's, or a live
    /// one).
    last: Option<(Vec<u64>, Vec<Arc<Walk>>, phitex_pdf::Layout)>,
}

fn memo() -> &'static Mutex<Option<Memo>> {
    static M: std::sync::OnceLock<Mutex<Option<Memo>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// The most states the link keeps (one before every page of a shorter
/// document): a state holds the session's objects so far, so keeping one
/// before each of many pages would cost the square of their number.
const SNAPS: usize = 32;

/// xdvipdfmx's options for `p`: `xdvipdfmx -q -E -o NAME`, dated.
fn options(p: &XdvPdf) -> DpxOptions {
    DpxOptions {
        pdf_filename: Some(p.name.as_bytes().to_vec()),
        source_date_epoch: Some(p.date),
        now: p.date,
        ..DpxOptions::default()
    }
}

/// zlib's `compress2`, as xdvipdfmx links it.
fn deflate() -> phitex_xdvipdfmx::obj::Deflate {
    Box::new(|level, data| phitex_codecs::deflate::compress(data, level))
}

/// The end's streams compressed on the engine's threads.
fn batch() -> phitex_xdvipdfmx::obj::BatchDeflate {
    Box::new(|jobs: &[(i32, &[u8])]| {
        let mut ex = crate::exec::Inner(crate::exec::threads(None));
        crate::exec::map_on(&mut ex, jobs.len(), &|i| {
            phitex_codecs::deflate::compress(jobs[i].1, jobs[i].0)
        })
    })
}

/// A fatal error of xdvipdfmx's, or a page it could not be given.
fn fatal(e: &phitex_xdvipdfmx::ctx::Fatal) -> String {
    format!("xdvipdfmx: {e}")
}

/// What a fold takes beyond the pages.
struct Fold<'a> {
    start: &'a Mutex<Session>,
    base: Option<&'a Base>,
    files: Arc<dyn Files>,
    nums: Arc<std::collections::BTreeMap<u32, u32>>,
    mag: i32,
    comment: &'a str,
    defs: &'a Defs,
    origins: Option<u64>,
}

/// The fold's times: the walk, xdvipdfmx's pages, the snapshots, the
/// end; and the page it began at.
#[derive(Default)]
struct Times {
    from: usize,
    walk: std::time::Duration,
    pages: std::time::Duration,
    snaps: std::time::Duration,
    end: std::time::Duration,
}

/// The exact file of `walks` (the pages, `ids` their identities): a fold
/// over one xdvipdfmx session, from the state the base kept before the
/// first page that differs from its (the pages before it keep their
/// bytes, a page whose bytes are the base's its chunk).
fn fold(f: &Fold<'_>, ids: &[u64], walks: &[Arc<Walk>]) -> Result<(Base, Times), String> {
    let n = walks.len();
    let same = f.base.map_or(0, |b| {
        b.ids.iter().zip(ids).take_while(|(a, c)| a == c).count()
    });
    // (the state before the first page that differs: the latest kept at
    // or before it)
    let resume = f.base.and_then(|b| {
        b.snaps
            .iter()
            .rev()
            .find(|s| s.0 <= same && s.0 < n)
            .map(|s| (s.0, s.1.snapshot(), s.2.clone()))
    });
    let (k, mut s, mut w) = match resume {
        Some(r) => r,
        None => (
            0,
            f.start.locked().snapshot(),
            DviWriter::new_xdv_ops(f.mag, f.comment),
        ),
    };
    s.set_files(f.files.clone());
    w.set_font_numbers(f.nums.clone());
    let mut t = Times {
        from: k,
        ..Times::default()
    };
    let (mut snaps, mut ops, mut states, mut chunks, old) = match f.base {
        Some(b) => (
            b.snaps.iter().filter(|x| x.0 < k).map(|x| (x.0, x.1.snapshot(), x.2.clone())).collect(),
            b.ops[..k].to_vec(),
            b.states[..k].to_vec(),
            b.chunks[..k].to_vec(),
            &b.chunks[..],
        ),
        None => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), &[][..]),
    };
    let every = n.div_ceil(SNAPS).max(1);
    for (i, p) in walks.iter().enumerate().skip(k) {
        if i.is_multiple_of(every) {
            let t0 = std::time::Instant::now();
            snaps.push((i, s.snapshot(), w.clone()));
            t.snaps += t0.elapsed();
        }
        let t0 = std::time::Instant::now();
        let o = walk(&mut w, p, f.defs, f.origins).map(Arc::<[Op]>::from);
        t.walk += t0.elapsed();
        let st = match &o {
            Some(o) => s.state_before(o),
            None => s.page_state(),
        };
        states.push(st.map_err(|e| fatal(&e))?);
        let t0 = std::time::Instant::now();
        let bytes: Arc<[u8]> = match &o {
            Some(o) => Arc::from(s.page_ops(o).map_err(|e| fatal(&e))?.pdf),
            None => Arc::from(&[][..]),
        };
        t.pages += t0.elapsed();
        // (a page whose bytes are the last file's: that file's bytes, the
        // same allocation, so that an update finds them unchanged at once)
        match old.get(i) {
            Some(x) if **x == *bytes => chunks.push(x.clone()),
            _ => chunks.push(bytes),
        }
        ops.push(o);
    }
    states.push(s.page_state().map_err(|e| fatal(&e))?);
    let t0 = std::time::Instant::now();
    let post = s.snapshot();
    let post_writer = w.clone();
    t.snaps += t0.elapsed();
    let t0 = std::time::Instant::now();
    let tail = s.finish().map_err(|e| fatal(&e))?;
    t.end = t0.elapsed();
    let layout = layout(&chunks, &tail, &s)
        .ok_or("xdvipdfmx: the file's cross-reference section was not written")?;
    Ok((
        Base {
            ids: ids.to_vec(),
            walks: walks.to_vec(),
            ops,
            states,
            snaps,
            chunks,
            post,
            post_writer,
            size: s.next_label(),
            layout,
        },
        t,
    ))
}

/// The live file of `walks` from base `b` ([`phitex_pdf::Layout::live`]):
/// the pages whose operations differ from the base's written again after
/// its last page ([`Session::reship`]), the end written again, the rest
/// of the base's file kept, so that an update section holds those pages,
/// what the end changed, and nothing else. None where a page can't be
/// written again so (another number of pages, a special whose effect is
/// on the document, a page taking or leaving another state than the
/// base's last page left, [`PageState`]): the exact file's link then.
/// The pages written again, the file and the link's times; or why not.
fn live(
    b: &Base,
    walks: &[Arc<Walk>],
    files: Arc<dyn Files>,
    defs: &Defs,
    origins: Option<u64>,
) -> Result<(usize, phitex_pdf::Layout, String), String> {
    let n = walks.len();
    if b.ids.len() != n {
        return Err(format!("{n} pages, the base's {}", b.ids.len()));
    }
    let t0 = std::time::Instant::now();
    let mut s = b.post.snapshot();
    let t_snap = t0.elapsed();
    s.set_files(files);
    let mut w = b.post_writer.clone();
    let mut again = Vec::new();
    for (i, p) in walks.iter().enumerate() {
        if Arc::ptr_eq(p, &b.walks[i]) {
            continue;
        }
        let ops = walk(&mut w, p, defs, origins);
        let old = b.ops[i].as_deref();
        match (&ops, old) {
            (Some(o), Some(x)) if neutral(o).eq(neutral(x)) => {}
            (Some(o), Some(x)) => again.push((i, o.clone(), x)),
            _ => return Err(format!("page {} shipped or not, as it was not", i + 1)),
        }
    }
    let t_walk = t0.elapsed().saturating_sub(t_snap);
    let last = &b.states[n];
    let mut chunk = Vec::new();
    for (i, ops, old) in &again {
        let k = i + 1;
        let (before, after) = (&b.states[*i], &b.states[k]);
        if !before.same_fixed(last) || !after.same_fixed(last) {
            return Err(format!(
                "page {k} found or left another state than the last page left: {}{}",
                before.differs(last),
                after.differs(last)
            ));
        }
        // (a special whose effect is on the document: the same as the
        // page first had, as the file has it already)
        let (a, z) = (doc_specials(ops, &mut s), doc_specials(old, &mut s));
        match (a, z) {
            (Ok(a), Ok(z)) if a == z => {}
            (Ok(_), Ok(_)) => return Err(format!("page {k}'s specials on the document changed")),
            (Err(x), _) | (_, Err(x)) => {
                return Err(format!("page {k} has a special that can't be done again: {x}"));
            }
        }
        // (given what it was given, it leaves what it left; the last
        // page's then set back)
        s.set_page_state(before);
        chunk.extend(s.reship(*i, ops, b.size).map_err(|e| fatal(&e))?.pdf);
        let now = s.page_state().map_err(|e| fatal(&e))?;
        if now != *after {
            return Err(format!("page {k} written again left another state: {}", now.differs(after)));
        }
        s.set_page_state(last);
    }
    if again.is_empty() {
        return Ok((0, b.layout.clone(), String::new()));
    }
    let t_pages = t0.elapsed().saturating_sub(t_snap + t_walk);
    let t = std::time::Instant::now();
    let tail = s.finish().map_err(|e| fatal(&e))?;
    let t_end = t.elapsed();
    let mut chunks = b.chunks.clone();
    chunks.push(Arc::from(chunk));
    let times = format!(
        "the session taken {t_snap:?}, walk {t_walk:?}, the pages written {t_pages:?}, end {t_end:?}"
    );
    layout(&chunks, &tail, &s)
        .map(|l| (again.len(), l, times))
        .ok_or_else(|| "no cross-reference section".to_owned())
}

/// The specials of `ops` whose effect is on the document, which a page
/// written again does not do again ([`Again::Skip`]); or one that can't
/// be done again ([`Again::Never`]), its start.
fn doc_specials(ops: &[Op], s: &mut Session) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    for o in ops {
        if let Op::Special(x) = o {
            let never = || String::from_utf8_lossy(&x[..x.len().min(48)]).into_owned();
            match s.again(x).map_err(|_| never())? {
                Again::Never => return Err(never()),
                Again::Skip => out.push(x.clone()),
                Again::Do => {}
            }
        }
    }
    Ok(out)
}

/// The PDF file of `pages` (the end node's, [`crate::assemble`]): its
/// value, or the empty string with no pages or after xdvipdfmx's fatal
/// error (logged as the job's error).
#[allow(clippy::too_many_lines)]
pub(crate) fn file(
    cx: &mut Ctx<'_>,
    pages: &[ValueId],
    xe: Option<&crate::xetex::XeTeX>,
    origins: bool,
    p: &XdvPdf,
) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    if pages.is_empty() {
        return Ok(v.str(""));
    }
    let comment = crate::assemble::field(cx, "dvicomment")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mag =
        i32::try_from(crate::assemble::field(cx, "param:mag")?.map_or(1000, |x| int_of(v, x)))
            .unwrap_or(1000);
    let (nums, order) = crate::assemble::font_numbers(cx)?;
    let head = Head {
        inst: v.instance(),
        mag,
        comment: comment.clone(),
        order,
        name: p.name.clone(),
        date: p.date,
    };
    let ids: Vec<u64> = pages.iter().map(|x| x.raw()).collect();
    // (the pages' fonts by id, read before the last link's state is taken:
    // a read that stops the node leaves it in place)
    let mut defs = Defs::default();
    for &page in pages {
        crate::assemble::page_defs(cx, page, xe, &mut defs)?;
    }
    let files = (p.files)();
    // (the last link's state, taken: a build linking meanwhile starts
    // its own; another head of the same session's start (a font loaded
    // since, another build's values): the start and its files' log kept,
    // the pages linked again)
    let mut kept = memo().locked().take().and_then(|mut m| {
        if m.head == head {
            return Some(m);
        }
        let h = &m.head;
        if (h.mag, &h.comment, &h.name, h.date) != (head.mag, &head.comment, &head.name, head.date) {
            return None;
        }
        m.head = head.clone();
        m.base = None;
        m.last = None;
        Some(m)
    });
    let t_files = std::time::Instant::now();
    if let Some(m) = &kept
        && !still(&m.log, &*files)
    {
        kept = None;
    }
    let t_files = t_files.elapsed();
    let inst = origins.then(|| v.instance());
    // (every page the last file's: that file)
    if let Some(m) = kept.as_ref()
        && let Some((last, walks, l)) = &m.last
        && *last == ids
    {
        let x = crate::assemble::layout_value(v, l.clone());
        let walks = walks.clone();
        *memo().locked() = kept;
        shown(cx, pages, &walks, &defs, &nums, inst);
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "xdv pdf: the last file kept ({} pages), files checked {t_files:?}, {:?}",
                ids.len(),
                t0.elapsed()
            );
        }
        return Ok(x);
    }
    let log: Log = kept.as_ref().map_or_else(Log::default, |m| m.log.clone());
    let logged: Arc<dyn Files> = Arc::new(Logged {
        files: files.clone(),
        log: log.clone(),
    });
    let prev: Vec<Arc<Walk>> = kept
        .as_ref()
        .and_then(|m| m.last.as_ref())
        .map(|l| l.1.clone())
        .unwrap_or_default();
    let t_start = std::time::Instant::now();
    let (start, base) = if let Some(m) = kept {
        (m.start, m.base)
    } else {
        let dvi_mag = u32::try_from(mag).unwrap_or(0);
        match Session::direct(
            options(p),
            logged.clone(),
            deflate(),
            dvi_mag,
            comment.as_bytes(),
        ) {
            Ok(mut s) => {
                s.set_batch_deflate(batch());
                (Arc::new(Mutex::new(s)), None)
            }
            Err(e) => return Ok(failed(cx, &fatal(&e))),
        }
    };
    let t_start = t_start.elapsed();
    // (each page decoded once while it is the same)
    let t = std::time::Instant::now();
    let walks: Vec<Arc<Walk>> = {
        let b = base.as_ref().map(|b| b.locked());
        let mut old: phitex_defuse::FxHashMap<u64, &Arc<Walk>> =
            prev.iter().map(|w| (w.id, w)).collect();
        if let Some(b) = &b {
            old.extend(b.ids.iter().copied().zip(b.walks.iter()));
        }
        pages
            .iter()
            .map(|pg| {
                old.get(&pg.raw())
                    .map_or_else(|| Arc::new(Walk::of(v, *pg)), |w| Arc::clone(w))
            })
            .collect()
    };
    let t_decode = t.elapsed();
    // (a watch's file: the pages that changed written again after the
    // base's, the exact file made when asked)
    if p.live
        && let Some(b) = &base
    {
        let t = std::time::Instant::now();
        let r = live(&b.locked(), &walks, logged.clone(), &defs, inst);
        if let Err(why) = &r
            && crate::exec::prof_flag!("PHITEX_TEX_TIME")
        {
            eprintln!("xdv pdf: live: not ({why}), {:?}", t.elapsed());
        }
        if let Ok((k, l, times)) = r {
            let make = exact_maker(
                p,
                &head,
                &start,
                b,
                &log,
                &ids,
                &walks,
                &defs,
                &nums,
                &comment,
            );
            let l = if k == 0 { l } else { l.live(make) };
            let x = crate::assemble::layout_value(v, l.clone());
            *memo().locked() = Some(Memo {
                head,
                start,
                log,
                base: base.clone(),
                last: Some((ids, walks.clone(), l)),
            });
            shown(cx, pages, &walks, &defs, &nums, inst);
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                eprintln!(
                    "xdv pdf: live: {k} of {} pages written again: files checked {t_files:?}, pages decoded {t_decode:?}, link {:?} ({times}), in all {:?}",
                    pages.len(),
                    t.elapsed(),
                    t0.elapsed()
                );
            }
            return Ok(x);
        }
    }
    let r = {
        let b = base.as_ref().map(|b| b.locked());
        fold(
            &Fold {
                start: &start,
                base: b.as_deref(),
                files: logged,
                nums: nums.clone(),
                mag,
                comment: &comment,
                defs: &defs,
                origins: inst,
            },
            &ids,
            &walks,
        )
    };
    let (nb, t) = match r {
        Ok(x) => x,
        Err(e) => return Ok(failed(cx, &e)),
    };
    let l = nb.layout.clone();
    let x = crate::assemble::layout_value(v, l.clone());
    *memo().locked() = Some(Memo {
        head,
        start,
        log,
        base: Some(Arc::new(Mutex::new(nb))),
        last: Some((ids, walks.clone(), l)),
    });
    shown(cx, pages, &walks, &defs, &nums, inst);
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "xdv pdf: from page {} of {}: files checked {t_files:?}, start {t_start:?}, pages decoded {t_decode:?}, walk {:?}, xdvipdfmx pages {:?}, snapshots {:?}, end {:?}, in all {:?}",
            t.from + 1,
            pages.len(),
            t.walk,
            t.pages,
            t.snaps,
            t.end,
            t0.elapsed()
        );
    }
    if crate::exec::env_flag!("PHITEX_XDV_DIFF") {
        diff(cx, pages, xe, p, x)?;
    }
    Ok(x)
}

/// A live file's exact file ([`phitex_pdf::Layout::live`]): the fold from
/// base `b` made when asked (the watch idle), and the link's base from
/// then on if the link has not moved on meanwhile.
#[allow(clippy::too_many_arguments)]
fn exact_maker(
    p: &XdvPdf,
    head: &Head,
    start: &Arc<Mutex<Session>>,
    b: &Arc<Mutex<Base>>,
    log: &Log,
    ids: &[u64],
    walks: &[Arc<Walk>],
    defs: &Defs,
    nums: &Arc<std::collections::BTreeMap<u32, u32>>,
    comment: &str,
) -> phitex_pdf::MakeExact {
    let (p, head, start, b, log) = (p.clone(), head.clone(), start.clone(), b.clone(), log.clone());
    let (ids, walks, defs, nums, comment) =
        (ids.to_vec(), walks.to_vec(), defs.clone(), nums.clone(), comment.to_owned());
    Arc::new(move || {
        let logged: Arc<dyn Files> = Arc::new(Logged {
            files: (p.files)(),
            log: log.clone(),
        });
        let r = fold(
            &Fold {
                start: &start,
                base: Some(&b.locked()),
                files: logged,
                nums: nums.clone(),
                mag: head.mag,
                comment: &comment,
                defs: &defs,
                origins: None,
            },
            &ids,
            &walks,
        );
        let (nb, _) = r.map_err(|e| phitex_pdf::PdfError::Pdf(String::new(), e))?;
        let l = nb.layout.clone();
        // (the link's base from now on, unless it moved on meanwhile)
        let mut m = memo().locked();
        if let Some(m) = m.as_mut()
            && m.head == head
            && m.base.as_ref().is_some_and(|x| Arc::ptr_eq(x, &b))
        {
            m.base = Some(Arc::new(Mutex::new(nb)));
            if let Some(last) = m.last.as_mut()
                && last.0 == ids
            {
                last.2 = l.clone();
            }
        }
        Ok(l)
    })
}

/// The pages' identities for a viewer (`crate::pdfout::SHOWN`), and the
/// glyph origins of pages kept from an earlier file whose writer kept
/// none.
fn shown(
    cx: &mut Ctx<'_>,
    pages: &[ValueId],
    walks: &[Arc<Walk>],
    defs: &Defs,
    nums: &Arc<std::collections::BTreeMap<u32, u32>>,
    origins: Option<u64>,
) {
    let v = cx.values();
    if let Some(inst) = origins {
        for p in walks {
            if !crate::origins::has(inst, p.id) {
                let mut w = DviWriter::new_xdv_ops(1000, "");
                w.set_font_numbers(nums.clone());
                let _ = walk(&mut w, p, defs, Some(inst));
            }
        }
    }
    let shown = v.list(
        pages
            .iter()
            .map(|p| v.int(i64::from_ne_bytes(p.raw().to_ne_bytes())))
            .collect(),
    );
    cx.contribute(v.field(crate::pdfout::SHOWN), v.seq_unit(shown));
}

/// The file's layout: the pages' chunks (each with the objects that begin
/// in it, from the cross-reference section), the end before that section,
/// the section, each object's place.
fn layout(chunks: &[Arc<[u8]>], tail: &[u8], s: &Session) -> Option<phitex_pdf::Layout> {
    let xw = s.xref_written()?;
    let mut starts = Vec::with_capacity(chunks.len());
    let mut at = 0usize;
    for c in chunks {
        starts.push(at);
        at += c.len();
    }
    let cut = xw.startxref.checked_sub(at)?;
    if cut > tail.len() {
        return None;
    }
    let mut objs: Vec<Vec<(usize, usize)>> = vec![Vec::new(); chunks.len() + 1];
    let mut places = Vec::with_capacity(xw.entries.len());
    for (n, e) in xw.entries.iter().enumerate() {
        // (the cross-reference stream's own: free, the places staying by
        // number; a live file's pages written again are numbered past it)
        if Some(n) == xw.stream.map(|x| x as usize) {
            places.push(phitex_pdf::Place::Free);
            continue;
        }
        let place = match e.kind {
            1 => {
                let off = e.field2 as usize;
                if off >= at {
                    objs[chunks.len()].push((n, off - at));
                } else {
                    // (the last chunk that begins at or before it: empty
                    // chunks hold no object)
                    let c = starts.partition_point(|&s| s <= off) - 1;
                    let c = (0..=c).rev().find(|&c| !chunks[c].is_empty())?;
                    objs[c].push((n, off - starts[c]));
                }
                phitex_pdf::Place::Direct
            }
            2 => phitex_pdf::Place::In(e.field2 as usize, u32::from(e.field3)),
            _ => phitex_pdf::Place::Free,
        };
        places.push(place);
    }
    // (the cross-reference stream the last object: not among them)
    while places.len() > 1 && places.last() == Some(&phitex_pdf::Place::Free) {
        places.pop();
    }
    for o in &mut objs {
        o.sort_unstable_by_key(|x| x.1);
    }
    let mut bytes = phitex_pdf::PdfBytes::new();
    for (c, o) in chunks.iter().zip(&objs) {
        if !c.is_empty() {
            bytes.push(phitex_pdf::Chunk {
                bytes: c.clone(),
                objs: o.clone(),
            });
        }
    }
    let hex = |b: &[u8]| {
        b.iter().fold(String::new(), |mut s, x| {
            use std::fmt::Write as _;
            let _ = write!(s, "{x:02X}");
            s
        })
    };
    let id = format!("/ID [<{}> <{}>]", hex(&xw.id[0]), hex(&xw.id[1]));
    let end = phitex_pdf::Linked {
        chunk: phitex_pdf::Chunk {
            bytes: Arc::from(&tail[..cut]),
            objs: objs.pop().unwrap_or_default(),
        },
        xref: Arc::from(&tail[cut..]),
        places,
        streams: xw.stream.is_some(),
        root: xw.root as usize,
        info: xw.info.map(|i| i as usize),
        id: Arc::from(id.into_bytes()),
    };
    Some(phitex_pdf::Layout::linked(bytes, end))
}

/// xdvipdfmx stopped: `msg` is the job's error (the log's, a diagnostic),
/// and there is no file, as after `xdvipdfmx` failing.
fn failed(cx: &mut Ctx<'_>, msg: &str) -> ValueId {
    let v = cx.values();
    let e = v.list(vec![v.str("error"), v.str(msg), v.int(-1)]);
    cx.contribute(v.field("log"), v.seq_unit(e));
    cx.contribute(v.field(crate::run::LOG_ERRORS), v.int(1));
    v.str("")
}

/// `PHITEX_XDV_DIFF=1` (the differential check): the same pages written
/// as XDV and made a PDF by the xdvipdfmx port, compared with the file
/// made here, the verdict on standard error (`xdv-diff: same ...` or
/// `xdv-diff: DIFFERENT ...`); with `PHITEX_XDV_DIFF_DIR`, both files
/// written there.
fn diff(
    cx: &mut Ctx<'_>,
    pages: &[ValueId],
    xe: Option<&crate::xetex::XeTeX>,
    p: &XdvPdf,
    direct: ValueId,
) -> Result<(), Stop> {
    let v = cx.values();
    let xdv = crate::assemble::dvi_file(cx, pages, xe, false)?;
    let xdv = crate::assemble::file_bytes_shared(v, xdv);
    let ours = crate::assemble::file_bytes_shared(v, direct);
    let port = Session::convert(options(p), (p.files)(), deflate(), &xdv);
    let line = match &port {
        Ok(b) if *b == *ours => {
            format!("xdv-diff: same ({} bytes, {} pages)", b.len(), pages.len())
        }
        Ok(b) => {
            let at = b
                .iter()
                .zip(ours.iter())
                .take_while(|(a, c)| a == c)
                .count();
            format!(
                "xdv-diff: DIFFERENT at byte {at}: the port's {} bytes, the direct file's {} ({} pages)",
                b.len(),
                ours.len(),
                pages.len()
            )
        }
        Err(e) => format!("xdv-diff: the port failed: {e}"),
    };
    eprintln!("{line}");
    if let Some(d) = std::env::var_os("PHITEX_XDV_DIFF_DIR") {
        let d = std::path::PathBuf::from(d);
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(format!("{}.direct.pdf", p.name)), ours.as_slice());
        let _ = std::fs::write(d.join(format!("{}.xdv", p.name)), xdv.as_slice());
        if let Ok(b) = &port {
            let _ = std::fs::write(d.join(format!("{}.port.pdf", p.name)), b);
        }
    }
    Ok(())
}
