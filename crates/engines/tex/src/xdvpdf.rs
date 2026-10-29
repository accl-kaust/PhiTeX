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
//! The files xdvipdfmx reads (its maps, fonts, the document's pictures)
//! are the host's, read through a log: a later link checks that each
//! answer is still the same before it keeps anything it made from them.

use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Stop, ValueId, Values};
use phitex_dvi::{DviWriter, Op};
use phitex_xdvipdfmx::api::{Options as DpxOptions, Session};
use phitex_xdvipdfmx::io::{Files, Format};

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

/// The link's state between builds.
struct Memo {
    head: Head,
    /// The session as [`Session::direct`] left it (the options, the
    /// config file and the font maps read), kept for a link from the
    /// first page.
    start: Session,
    /// The files' questions since `start` was made.
    log: Log,
    /// The pages linked, by value.
    ids: Vec<u64>,
    /// Before page `.0`: the session and the walk's writer.
    snaps: Vec<(usize, Session, DviWriter)>,
    /// Each page's bytes (empty: a page not shipped).
    chunks: Vec<Arc<[u8]>>,
    /// The file.
    layout: Option<phitex_pdf::Layout>,
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
    let mut defs: phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef> =
        phitex_defuse::FxHashMap::default();
    for &page in pages {
        crate::assemble::page_defs(cx, page, xe, &mut defs)?;
    }
    let files = (p.files)();
    // (the last link's state, taken: a build linking meanwhile starts
    // its own)
    let mut kept = memo().locked().take().filter(|m| m.head == head);
    let t_files = std::time::Instant::now();
    if let Some(m) = &kept
        && !still(&m.log, &*files)
    {
        kept = None;
    }
    let t_files = t_files.elapsed();
    let same = kept.as_ref().map_or(0, |m| {
        m.ids.iter().zip(&ids).take_while(|(a, b)| a == b).count()
    });
    // (every page the last file's: that file)
    if let Some(m) = kept.as_mut()
        && same == ids.len()
        && m.ids.len() == ids.len()
        && let Some(l) = m.layout.clone()
    {
        let x = crate::assemble::layout_value(v, l);
        *memo().locked() = kept;
        shown(cx, pages, xe, origins)?;
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
    // (the state before the first page that differs: the latest kept at
    // or before it)
    let resume = kept.as_ref().and_then(|m| {
        m.snaps
            .iter()
            .rev()
            .find(|s| s.0 <= same)
            .map(|s| (s.0, s.1.snapshot(), s.2.clone()))
    });
    let t_start = std::time::Instant::now();
    let (start_session, mut snaps, old) = if let Some(m) = kept {
        let k = resume.as_ref().map_or(0, |r| r.0);
        let snaps: Vec<_> = m.snaps.into_iter().filter(|s| s.0 <= k).collect();
        (m.start, snaps, m.chunks)
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
                (s, Vec::new(), Vec::new())
            }
            Err(e) => return Ok(failed(cx, &fatal(&e))),
        }
    };
    let (k, mut s, mut w) = match resume {
        Some(r) => r,
        None => (
            0,
            start_session.snapshot(),
            DviWriter::new_xdv_ops(mag, &comment),
        ),
    };
    s.set_files(logged.clone());
    w.set_font_numbers(nums.clone());
    let mut chunks: Vec<Arc<[u8]>> = old[..k.min(old.len())].to_vec();
    let t_start = t_start.elapsed();
    let zero = std::time::Duration::ZERO;
    let (mut t_walk, mut t_page, mut t_snap) = (zero, zero, zero);
    let every = pages.len().div_ceil(SNAPS).max(1);
    for (i, &page) in pages.iter().enumerate().skip(k) {
        if i.is_multiple_of(every) && !snaps.iter().any(|x| x.0 == i) {
            let t = std::time::Instant::now();
            snaps.push((i, s.snapshot(), w.clone()));
            t_snap += t.elapsed();
        }
        let t = std::time::Instant::now();
        let ops = walk(v, &mut w, page, &defs, origins);
        t_walk += t.elapsed();
        let t = std::time::Instant::now();
        let bytes: Arc<[u8]> = match ops {
            Some(ops) => match s.page_ops(&ops) {
                Ok(out) => Arc::from(out.pdf),
                Err(e) => return Ok(failed(cx, &fatal(&e))),
            },
            None => Arc::from(&[][..]),
        };
        t_page += t.elapsed();
        // (a page whose bytes are the last file's: that file's bytes, the
        // same allocation, so that an update finds them unchanged at once)
        match old.get(i) {
            Some(o) if **o == *bytes => chunks.push(o.clone()),
            _ => chunks.push(bytes),
        }
    }
    let t = std::time::Instant::now();
    let tail = match s.finish() {
        Ok(t) => t,
        Err(e) => return Ok(failed(cx, &fatal(&e))),
    };
    let t_finish = t.elapsed();
    let Some(layout) = layout(&chunks, &tail, &s) else {
        return Ok(failed(
            cx,
            "xdvipdfmx: the file's cross-reference section was not written",
        ));
    };
    let x = crate::assemble::layout_value(v, layout.clone());
    *memo().locked() = Some(Memo {
        head,
        start: start_session,
        log,
        ids,
        snaps,
        chunks,
        layout: Some(layout),
    });
    shown(cx, pages, xe, origins)?;
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "xdv pdf: from page {} of {}: files checked {t_files:?}, start {t_start:?}, walk {t_walk:?}, xdvipdfmx pages {t_page:?}, snapshots {t_snap:?}, end {t_finish:?}, in all {:?}",
            k + 1,
            pages.len(),
            t0.elapsed()
        );
    }
    if crate::exec::env_flag!("PHITEX_XDV_DIFF") {
        diff(cx, pages, xe, p, x)?;
    }
    Ok(x)
}

/// The pages' identities for a viewer (`crate::pdfout::SHOWN`), and the
/// glyph origins of pages kept from an earlier file whose writer kept
/// none.
fn shown(
    cx: &mut Ctx<'_>,
    pages: &[ValueId],
    xe: Option<&crate::xetex::XeTeX>,
    origins: bool,
) -> Result<(), Stop> {
    let v = cx.values();
    if origins {
        let mut defs: phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef> =
            phitex_defuse::FxHashMap::default();
        let (nums, _) = crate::assemble::font_numbers(cx)?;
        for &page in pages {
            if !crate::origins::has(v.instance(), page.raw()) {
                crate::assemble::page_defs(cx, page, xe, &mut defs)?;
                let mut w = DviWriter::new_xdv_ops(1000, "");
                w.set_font_numbers(nums.clone());
                let _ = walk(v, &mut w, page, &defs, true);
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
    Ok(())
}

/// Page entry `page` walked on `w` (XeTeX's `ship_out`): its operations,
/// none for a page not shipped (§641's too large one); with `origins`,
/// its glyphs kept by its value (`crate::origins`).
fn walk(
    v: &Values,
    w: &mut DviWriter,
    page: ValueId,
    defs: &phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>,
    origins: bool,
) -> Option<Vec<Op>> {
    let (bx, counts, whats) = crate::assemble::dvi_parts(v, page)?;
    (w.h_offset, w.v_offset) = crate::assemble::dvi_offsets(v, page);
    w.page_size = crate::assemble::xdv_page_size(v, page);
    let fonts = |f: u32| crate::assemble::font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| crate::assemble::special_of(v, &whats, h, true);
    let out = origins.then(|| {
        let (r, out) = crate::origins::Recorder::new();
        w.set_tap(Some(Box::new(r)));
        out
    });
    let r = w.ship_out(&bx, counts, &fonts, &mut wh);
    drop(w.set_tap(None));
    if let Some(g) = out.and_then(|o| o.locked().take()) {
        crate::origins::keep(v.instance(), page.raw(), g);
    }
    let ops = w.take_ops();
    r.ok().map(|()| ops)
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
        if Some(n) == xw.stream.map(|x| x as usize) {
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
