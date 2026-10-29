//! The output file, assembled by the end node (`Dvi`) from what the
//! finishers shipped: the `pages` accumulator holds every page in
//! document order (a finisher is a node in the line, so the order is
//! TeX's), and the PDF writers' pieces sit beside it. Nothing here
//! reads a loop-back or a chain of page steps: the end node reads
//! accumulators and the fields reaching it, like any node.
//!
//! - **DVI** (`\pdfoutput=0`): each page's bytes from its box, counts and
//!   fonts (§638–§640 as `phitex_dvi` does them), one writer, the
//!   postamble; the file is the node's output as a string of characters
//!   0–255 ([`dvi_file`]).
//! - **PDF, exact** (byte-identical object numbers): [`pdf_exact`], the
//!   link (spec-redesign §1): the line's records (`crate::pdfout::XPAGES`:
//!   the pages written apart, the read-time commands, the document-level
//!   ones) put on one writer state in order, numbered as pdfTeX numbers
//!   them; what the commands and the pages' writers guessed given back
//!   as loop-backs for the next round.

use crate::exec::LockWait as _;
use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_typeset::node::Node;

use crate::fonts::bytes_of;
use crate::nodes::{self, Side};
use crate::val::{int_of, list_of, str_of};

/// The end node's run: the file as a string value (empty: no file, §642).
/// In exact PDF mode the file is the link's ([`link`], the node before
/// this one: `pdf:file`); the end node defines nothing a piece reads
/// (spec-final §4.4: the cuts are the finishers', the link's loop-backs
/// the link's).
pub(crate) fn output(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    // where the pages end and their finishers have run first: read first,
    // the pages are all there (a node run before gives nothing, and runs
    // again as they come in)
    if !crate::pages::all_finished(cx)? {
        return Ok(ValueId::UNIT);
    }
    // (a PDF's file is the link's: the pages read for their mode only,
    // the first page's, [`pdf_mode`]; a DVI file is made of them all)
    let pdf = pdf_mode(cx)?;
    let mut n = 0;
    let r = if pdf {
        Ok(field(cx, LINKED)?
            .filter(|&x| x != ValueId::UNIT)
            .unwrap_or_else(|| v.str("")))
    } else {
        let pages = v.seq_to_vec(cx.fold(v.field("pages"), None, None)?);
        n = pages.len();
        let origins = o.origins.load(std::sync::atomic::Ordering::Relaxed);
        // (XeTeX's PDF written from the pages, where the client asks for
        // it; else its XDV)
        match o.xe.get().and_then(|x| x.pdf.as_ref()) {
            Some(p) => crate::xdvpdf::file(cx, &pages, o.xe.get(), origins, p),
            None => dvi_file(cx, &pages, o.xe.get(), origins),
        }
    };
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "output file: {} pages (a DVI's), {:?}{}",
            n,
            t0.elapsed(),
            if r.is_ok() { "" } else { " (stopped)" }
        );
    }
    r
}

/// The exact PDF file the link made (`pdf:file`), read by the end node.
pub(crate) const LINKED: &str = "pdf:file";

/// The output file the end node made ([`output`]), which the build reads
/// (`Build::output`); no node reads it.
pub(crate) const END_FILE: &str = "end:file";

/// The bytes of the files the link made, by their values table and hash:
/// the file's value is `["bytes", hash, length]`, not the file as a string
/// (copied, interned and hashed as characters, then copied back to bytes:
/// O(file) three times an edit). The latest few kept per table, until
/// the build that owns the table goes ([`forget_linked`]): what one build
/// links never evicts another's file (with a few kept for all builds, the
/// builds of one process running at once lost their files: an empty
/// output).
fn linked_files() -> &'static LinkedFiles {
    static M: std::sync::OnceLock<LinkedFiles> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// [`linked_files`]: per values table (its instance), its files.
type LinkedFiles = std::sync::Mutex<Vec<(u64, Linked)>>;

/// A table's linked files: (hash, file), the latest last.
type Linked = std::collections::VecDeque<(u64, File)>;

/// A linked file: its bytes, or its layout (a PDF file, the exact bytes
/// made from it when asked).
#[derive(Clone)]
enum File {
    #[cfg_attr(not(test), allow(dead_code))]
    Bytes(std::sync::Arc<Vec<u8>>),
    Layout(phitex_pdf::Layout),
}

impl File {
    /// Whether `o` is this file, compared whole (its hash is the key it
    /// is kept under, [`file_value`]).
    fn same(&self, o: &File) -> bool {
        match (self, o) {
            (File::Bytes(a), File::Bytes(b)) => std::sync::Arc::ptr_eq(a, b) || a == b,
            (File::Layout(a), File::Layout(b)) => a.same_chunks(b),
            _ => false,
        }
    }
}

/// The files linked in values table `instance` dropped: its build goes.
pub(crate) fn forget_linked(instance: u64) {
    linked_files().locked().retain(|(i, _)| *i != instance);
}

/// The value standing for file `f` ([`linked_files`]), under its hash
/// `h`, or the next free one the hash probes on to where another file
/// of the table holds `h`: equal values, equal files.
fn file_value(v: &phitex_defuse::Values, f: File, mut h: u64, n: usize) -> ValueId {
    let mut all = linked_files().locked();
    let k = if let Some(k) = all.iter().position(|(i, _)| *i == v.instance()) {
        k
    } else {
        all.push((v.instance(), std::collections::VecDeque::new()));
        all.len() - 1
    };
    let m = &mut all[k].1;
    while m.iter().any(|e| e.0 == h && !e.1.same(&f)) {
        h = phitex_defuse::hash_words(h, &[1]);
    }
    if !m.iter().any(|e| e.0 == h) {
        m.push_back((h, f));
        while m.len() > 4 {
            m.pop_front();
        }
    }
    v.list(vec![
        v.str("bytes"),
        v.int(i64::from_ne_bytes(h.to_ne_bytes())),
        v.int(i64::try_from(n).unwrap_or(0)),
    ])
}

/// The value standing for the PDF file of layout `l` ([`linked_files`]).
pub(crate) fn layout_value(v: &phitex_defuse::Values, l: phitex_pdf::Layout) -> ValueId {
    let (h, n) = (l.hash(), l.len());
    file_value(v, File::Layout(l), h, n)
}

/// The value standing for file bytes `b` ([`linked_files`]).
#[cfg(test)]
fn bytes_value(v: &phitex_defuse::Values, b: Vec<u8>, hash: Option<u64>) -> ValueId {
    let h = hash.unwrap_or_else(|| phitex_defuse::hash_bytes(0x6c69_6e6b_6564, &b));
    let n = b.len();
    file_value(v, File::Bytes(std::sync::Arc::new(b)), h, n)
}

/// What an output value stands for ([`file_of`]).
enum Found {
    /// A file a [`file_value`] made.
    File(File),
    /// One no longer kept.
    Gone,
    /// None: the value is the file, as a string.
    Str,
}

/// The file an output value stands for.
fn file_of(v: &phitex_defuse::Values, x: ValueId) -> Found {
    if let phitex_defuse::Value::List(xs) = &*v.get(x)
        && xs.len() == 3
        && str_of(v, xs[0]) == "bytes"
    {
        let h = u64::from_ne_bytes(int_of(v, xs[1]).to_ne_bytes());
        return linked_files()
            .locked()
            .iter()
            .find(|(i, _)| *i == v.instance())
            .and_then(|(_, m)| m.iter().find(|e| e.0 == h).map(|e| e.1.clone()))
            .map_or(Found::Gone, Found::File);
    }
    Found::Str
}

/// The bytes of an output value: a file's string, or the bytes a
/// [`file_value`] stands for (shared, not copied; a layout's exact file).
pub(crate) fn file_bytes_shared(v: &phitex_defuse::Values, x: ValueId) -> std::sync::Arc<Vec<u8>> {
    match file_of(v, x) {
        Found::File(File::Bytes(b)) => b,
        Found::File(File::Layout(l)) => l.exact().unwrap_or_default(),
        Found::Gone => std::sync::Arc::default(),
        Found::Str => std::sync::Arc::new(
            str_of(v, x)
                .chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
                .collect(),
        ),
    }
}

/// The PDF file's layout an output value stands for (none: a DVI file,
/// or no file).
pub(crate) fn file_layout(v: &phitex_defuse::Values, x: ValueId) -> Option<phitex_pdf::Layout> {
    match file_of(v, x) {
        Found::File(File::Layout(l)) => Some(l),
        _ => None,
    }
}

/// The format pdfTeX fixed at the first `\shipout` (`fix_pdfoutput`):
/// a page entry is `[box, counts, fonts, pdf params, pdf data]`, the
/// params' first element whether the file is PDF.
/// Whether the pages are a PDF's: the first page's mode
/// (`pages[0][3][0]`), read through a path, so the reader depends on it
/// alone (no page: not).
fn pdf_mode(cx: &mut Ctx<'_>) -> Result<bool, Stop> {
    let v = cx.values();
    // (the first page's entry: the first contribution, not the fold of
    // them all, which an edit would join again)
    let t0 = std::time::Instant::now();
    let first = cx.next_contribution(v.field("pages"), None)?;
    let r = first
        .and_then(|(_, x)| v.seq_nth(x, 0))
        .map(|e| list_of(v, e))
        .and_then(|xs| xs.get(3).map(|&p| list_of(v, p)))
        .and_then(|ps| ps.first().copied())
        .is_some_and(|x| v.as_int(x).is_some_and(|n| n != 0));
    crate::timeline::note("pdf mode (the first page)", t0.elapsed());
    Ok(r)
}

/// The link's run (spec-redesign §1, `link`): in exact PDF mode the file
/// from the line's records ([`pdf_exact`]), defined for the end node
/// (`pdf:file`), and the loop-backs it gives the next round; nothing in
/// any other mode.
pub(crate) fn link(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    // (before the pages end and their finishers have run: nothing, run
    // again as they come in)
    if !crate::pages::all_finished(cx)? {
        return Ok(ValueId::UNIT);
    }
    if !pdf_mode(cx)? {
        // (its declared definition made in every mode: none here)
        cx.define_with(
            v.field(LINKED),
            ValueId::UNIT,
            phitex_defuse::Locality::Global,
        );
        return Ok(ValueId::UNIT);
    }
    let file = pdf_exact(cx, o)?;
    cx.define_with(v.field(LINKED), file, phitex_defuse::Locality::Global);
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!("link: {:?}", t0.elapsed());
    }
    Ok(ValueId::UNIT)
}

/// A field reaching the end node.
pub(crate) fn field(cx: &mut Ctx<'_>, name: &str) -> Result<Option<ValueId>, Stop> {
    let f = cx.values().field(name);
    cx.read(f)
}

fn int_field(cx: &mut Ctx<'_>, name: &str) -> Result<i64, Stop> {
    let v = cx.values();
    Ok(field(cx, name)?.map_or(0, |x| int_of(v, x)))
}

/// Bytes as a string of characters 0–255 (the file as a value).
fn latin1(bytes: &[u8]) -> String {
    crate::api::latin1(bytes)
}

// ----------------------------------------------------------------- DVI

/// The DVI file: the preamble, each page shipped in order, the
/// postamble (§617–§642).
///
/// Example: two pages whose boxes are `b1` and `b2`, with `\count0` 1
/// and 2, give `pre … bop 1 … eop bop 2 … eop post … post_post`; the
/// value is those bytes as characters. No page: no file (§642), the
/// empty string.
#[allow(clippy::too_many_lines)]
pub(crate) fn dvi_file(
    cx: &mut Ctx<'_>,
    pages: &[ValueId],
    xe: Option<&crate::xetex::XeTeX>,
    origins: bool,
) -> Result<ValueId, Stop> {
    let xdv = xe.is_some();
    let v = cx.values();
    if pages.is_empty() {
        return Ok(v.str(""));
    }
    let comment = field(cx, "dvicomment")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mag = i32::try_from(field(cx, "param:mag")?.map_or(1000, |x| int_of(v, x))).unwrap_or(1000);
    // (spec-final §6.3: the pages the last file of this build shipped,
    // the same from its start, are not shipped again: the writer goes on
    // from its state after them, kept every `DVI_SNAP` pages; the fonts
    // defined before a page are the prefix's)
    let (nums, order) = font_numbers(cx)?;
    let head: DviHead = (v.instance(), mag, xdv, order, comment.clone());
    let ids: Vec<u64> = pages.iter().map(|p| p.raw()).collect();
    let (mut w, start, mut snaps) = {
        let m = dvi_memo().locked();
        let found = m.as_ref().filter(|m| m.head == head).and_then(|m| {
            let same = m.ids.iter().zip(&ids).take_while(|(a, b)| a == b).count();
            m.snaps
                .iter()
                .rev()
                .find(|s| s.0 <= same && s.0 > 0)
                .map(|s| {
                    (
                        s.1.clone(),
                        s.0,
                        m.snaps.iter().filter(|x| x.0 <= s.0).cloned().collect(),
                    )
                })
        });
        found.unwrap_or_else(|| (new_writer(mag, &comment, xdv), 0, Vec::new()))
    };
    w.set_font_numbers(nums.clone());
    // (the pages' fonts by id, resolved here from the fonts' fields: a
    // font's fields are defined once, at its load)
    let mut defs: phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef> =
        phitex_defuse::FxHashMap::default();
    for &page in &pages[start..] {
        page_defs(cx, page, xe, &mut defs)?;
    }
    // (spec-final §6.3: each page written apart, on the executor, the
    // fonts defined before it the prefix of the pages' own font use;
    // then put in order, its bytes as they are but for the pointer to
    // the previous page, or shipped again where they would differ)
    let rest = &pages[start..];
    let mut seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for &page in &pages[..start] {
        seen.extend(page_fonts(v, page));
    }
    let mut before: Vec<Vec<u32>> = Vec::with_capacity(rest.len());
    for &page in rest {
        let own = page_fonts(v, page);
        before.push(own.iter().copied().filter(|f| seen.contains(f)).collect());
        seen.extend(own);
    }
    let apart: Vec<Option<phitex_dvi::PageBytes>> = {
        let mut ex = crate::exec::Inner(crate::exec::threads(None));
        crate::exec::map_on(&mut ex, rest.len(), &|i| {
            let mut wi = new_writer(mag, &comment, xdv);
            wi.set_font_numbers(nums.clone());
            for &f in &before[i] {
                if let Some(d) = defs.get(&f) {
                    wi.assume_defined(f, d.clone());
                }
            }
            dvi_page_kept(v, &mut wi, rest[i], &defs, xdv, origins)
        })
    };
    for (i, (&page, pb)) in rest.iter().zip(apart).enumerate() {
        let i = i + start;
        if i > start && (i - start).is_multiple_of(DVI_SNAP) {
            snaps.push((i, w.clone()));
        }
        (w.h_offset, w.v_offset) = dvi_offsets(v, page);
        if !pb.is_some_and(|pb| w.ship_kept(&pb)) {
            dvi_page(v, &mut w, page, &defs, xdv);
        }
    }
    *dvi_memo().locked() = Some(DviMemo { head, ids, snaps });
    // (origins on: each page's glyphs kept by its value, and the pages'
    // values as the file shows them, `crate::origins`; the pages kept
    // from the last file whose glyphs were not are written for them)
    if origins {
        for &page in &pages[..start] {
            if !crate::origins::has(v.instance(), page.raw()) {
                page_defs(cx, page, xe, &mut defs)?;
                let mut wi = new_writer(mag, &comment, xdv);
                wi.set_font_numbers(nums.clone());
                let _ = dvi_page_kept(v, &mut wi, page, &defs, xdv, true);
            }
        }
        let shown = v.list(pages.iter().map(|p| v.int(i64::from_ne_bytes(p.raw().to_ne_bytes()))).collect());
        cx.contribute(v.field(crate::pdfout::SHOWN), v.seq_unit(shown));
    }
    let bytes = w.finish();
    Ok(v.intern(Value::Str(latin1(&bytes).into_boxed_str())))
}

/// The definitions of the fonts page entry `page` uses, by id, put in
/// `defs` where not there: resolved from the fonts' fields (a font's
/// fields are defined once, at its load).
pub(crate) fn page_defs(
    cx: &mut Ctx<'_>,
    page: ValueId,
    xe: Option<&crate::xetex::XeTeX>,
    defs: &mut phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>,
) -> Result<(), Stop> {
    let v = cx.values();
    for fx in list_of(v, page)
        .get(2)
        .map(|&l| list_of(v, l))
        .unwrap_or_default()
    {
        let id = u32::try_from(int_of(v, fx)).unwrap_or(0);
        if defs.contains_key(&id) {
            continue;
        }
        let name = crate::fields::font_cell_name(i64::from(id), crate::fields::FontCell::Info);
        let info = cx.read(v.field(&name))?;
        if let Some(xe) = xe
            && let Some(nf) = crate::native::of_info(xe, v, info)
        {
            // (XeTeX's native font: `define_native_font`)
            defs.insert(
                id,
                phitex_dvi::FontDef {
                    checksum: [0; 4],
                    size: nf.size,
                    design_size: nf.font.design_size,
                    area: Vec::new(),
                    name: nf.font.name_of_file.as_bytes().to_vec(),
                    widths: Vec::new(),
                    native: Some(nf.def.clone()),
                },
            );
        } else if let Some(i) = crate::fonts::info(v, info)
            && let Some(l) = crate::fonts::load_info(v, &i)
        {
            defs.insert(id, crate::fonts::font_def(&l, &i.name));
        }
    }
    Ok(())
}

/// A DVI writer, or XeTeX's XDV writer.
fn new_writer(mag: i32, comment: &str, xdv: bool) -> phitex_dvi::DviWriter {
    if xdv {
        phitex_dvi::DviWriter::new_xdv(mag, comment)
    } else {
        phitex_dvi::DviWriter::new(mag, comment)
    }
}

/// XeTeX's page size for page entry `page`'s `pdf:pagesize`
/// (`\pdfpagewidth`, `\pdfpageheight` at its shipout: its PDF
/// parameters' fourth and fifth).
pub(crate) fn xdv_page_size(v: &Values, page: ValueId) -> (i32, i32) {
    let ps = list_of(v, page)
        .get(3)
        .map(|&p| list_of(v, p))
        .unwrap_or_default();
    let n = |k: usize| {
        ps.get(k)
            .map_or(0, |&x| i32::try_from(int_of(v, x)).unwrap_or(0))
    };
    (n(3), n(4))
}

/// The pages between two of the DVI writer's kept states ([`DviMemo`]).
const DVI_SNAP: usize = 16;

/// The last DVI file of the process: its pages (by value) and the writer
/// every [`DVI_SNAP`] pages.
/// What a DVI writer starts from: the values' instance, `\mag`, whether
/// it writes XDV, the fonts' order ([`font_numbers`]), the comment.
type DviHead = (u64, i32, bool, Vec<i64>, String);

struct DviMemo {
    head: DviHead,
    ids: Vec<u64>,
    snaps: Vec<(usize, phitex_dvi::DviWriter)>,
}

fn dvi_memo() -> &'static std::sync::Mutex<Option<DviMemo>> {
    static M: std::sync::OnceLock<std::sync::Mutex<Option<DviMemo>>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(None))
}

/// The fonts page entry `page` uses, by id.
fn page_fonts(v: &Values, page: ValueId) -> Vec<u32> {
    list_of(v, page)
        .get(2)
        .map(|&l| list_of(v, l))
        .unwrap_or_default()
        .iter()
        .map(|&f| u32::try_from(int_of(v, f)).unwrap_or(0))
        .collect()
}

/// [`dvi_page`] on a writer of its own, its bytes kept
/// (`DviWriter::ship_out_kept`); `None` where it could not be written.
fn dvi_page_kept(
    v: &Values,
    w: &mut phitex_dvi::DviWriter,
    page: ValueId,
    defs: &phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>,
    xdv: bool,
    origins: bool,
) -> Option<phitex_dvi::PageBytes> {
    let (bx, counts, whats) = dvi_parts(v, page)?;
    (w.h_offset, w.v_offset) = dvi_offsets(v, page);
    let fonts = |f: u32| font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| special_of(v, &whats, h, xdv);
    if xdv {
        w.page_size = xdv_page_size(v, page);
    }
    // (origins on: the page's glyphs as dvipdfmx's PDF shows them, kept
    // by the page's value, `crate::origins`)
    let out = origins.then(|| {
        let (r, out) = crate::origins::Recorder::new();
        w.set_tap(Some(Box::new(r)));
        out
    });
    let made = w.ship_out_kept(&bx, counts, &fonts, &mut wh).ok();
    drop(w.set_tap(None));
    if let Some(g) = out.and_then(|o| o.locked().take()) {
        crate::origins::keep(v.instance(), page.raw(), g);
    }
    made
}

/// `\hoffset` and `\voffset` as the page's `\shipout` read them (§640:
/// the page's margins), from its parameters
/// (`Run::pdf_page_params`: `[pdf, hor, vor, width, height, hoffset,
/// voffset, ...]`).
pub(crate) fn dvi_offsets(v: &Values, page: ValueId) -> (i32, i32) {
    let ps = list_of(v, page)
        .get(3)
        .map(|&x| list_of(v, x))
        .unwrap_or_default();
    let at = |k: usize| {
        ps.get(k)
            .map_or(0, |&x| i32::try_from(int_of(v, x)).unwrap_or(0))
    };
    (at(5), at(6))
}

/// The fonts' numbers by key (TeX's `f`, which its DVI file writes as
/// `f - 1`): their ranks in the order of first loads, the format's
/// fonts first ([`crate::api::FONT_ORDER`]), then the line's
/// `fontload` records in TeX's order; and that order.
#[allow(clippy::type_complexity)]
pub(crate) fn font_numbers(
    cx: &mut Ctx<'_>,
) -> Result<
    (
        std::sync::Arc<std::collections::BTreeMap<u32, u32>>,
        Vec<i64>,
    ),
    Stop,
> {
    let v = cx.values();
    let mut order: Vec<i64> = field(cx, crate::api::FONT_ORDER)?
        .map(|x| list_of(v, x).iter().map(|&k| int_of(v, k)).collect())
        .unwrap_or_default();
    let line = crate::val::cat_to_vec(v, cx.fold(v.field(crate::pdfout::XPAGES), None, None)?);
    for x in crate::pdfout::order_line(v, &line) {
        if let Some(k) = crate::pdfout::fontload_key(v, x)
            && !order.contains(&k)
        {
            order.push(k);
        }
    }
    let nums: std::collections::BTreeMap<u32, u32> = order
        .iter()
        .enumerate()
        .filter_map(|(i, &k)| Some((u32::try_from(k).ok()?, u32::try_from(i + 1).ok()?)))
        .collect();
    Ok((std::sync::Arc::new(nums), order))
}

/// A page entry's box (decoded), counts and whatsits.
pub(crate) fn dvi_parts(
    v: &Values,
    page: ValueId,
) -> Option<(phitex_typeset::node::BoxNode, [i32; 10], Vec<ValueId>)> {
    let xs = list_of(v, page);
    let mut side = Side::default();
    // (a page is decoded once: its nodes are not kept in this thread's
    // cache, whose copies cost more than the decoding)
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let bx = xs.first().and_then(|&b| nodes::decode(v, b, &mut side));
    crate::val::NO_KEEP.with(|c| c.set(was));
    let Some(Node::Box(bx)) = bx else {
        return None;
    };
    let mut counts = [0i32; 10];
    for (k, c) in xs
        .get(1)
        .map(|&l| list_of(v, l))
        .unwrap_or_default()
        .iter()
        .enumerate()
        .take(10)
    {
        counts[k] = i32::try_from(int_of(v, *c)).unwrap_or(0);
    }
    Some((*bx, counts, side.whats))
}

/// Font `f`'s definition as the DVI file needs it.
pub(crate) fn font_def_of(
    defs: &phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>,
    f: u32,
) -> phitex_dvi::FontDef {
    defs.get(&f).cloned().unwrap_or(phitex_dvi::FontDef {
        checksum: [0; 4],
        size: 0,
        design_size: 0,
        area: Vec::new(),
        name: Vec::new(),
        widths: Vec::new(),
        native: None,
    })
}

/// A `\special`'s bytes, for whatsit `h` of a page.
pub(crate) fn special_of(v: &Values, whats: &[ValueId], h: u32, xdv: bool) -> Option<Vec<u8>> {
    let w = *whats.get(h as usize)?;
    let ys = list_of(v, w);
    let kind = ys.first().map(|&k| str_of(v, k));
    // (XeTeX's picture: `pic_out`'s special, made with the node)
    if xdv && kind.as_deref() == Some("xpic") {
        return Some(bytes_of(&ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default()));
    }
    if kind.as_deref() == Some("special") {
        let s = ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default();
        // (XeTeX's `special_out`: UTF-8)
        Some(if xdv {
            crate::xetex::special_bytes(&s)
        } else {
            bytes_of(&s)
        })
    } else {
        None
    }
}

/// One page shipped on `w` (§638–§640): the box decoded, its counts,
/// its fonts' definitions from the page's font list (`[id, name, file,
/// size]` each), its `\special`s from its whatsits.
fn dvi_page(
    v: &Values,
    w: &mut phitex_dvi::DviWriter,
    page: ValueId,
    defs: &phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef>,
    xdv: bool,
) {
    let Some((bx, counts, whats)) = dvi_parts(v, page) else {
        return;
    };
    (w.h_offset, w.v_offset) = dvi_offsets(v, page);
    let fonts = |f: u32| font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| special_of(v, &whats, h, xdv);
    if xdv {
        w.page_size = xdv_page_size(v, page);
    }
    let _ = w.ship_out(&bx, counts, &fonts, &mut wh);
}

// ---------------------------------------------------------- PDF, exact

/// The PDF file with pdfTeX's object numbers: the link of the line's
/// records (`crate::pdfout::finish_exact`).
///
/// 1. A node that ships a page writes it apart
///    (`crate::pdfout::ship_apart`: decoded, compressed, as a record)
///    and puts its key in the line's accumulator where it shipped; a
///    read-time command puts its step there where it runs
///    (`crate::pdfout::command`), a document-level one its data.
/// 2. This node puts them on one state in that order (a page's objects
///    numbered after every record before it), names the fonts in
///    pdfTeX's order of first use, writes the tail, and gives back what
///    each command got and the fonts' names (loop-backs: a node that
///    guessed otherwise runs again in the next round).
///
/// Example: pieces `A B`, a page finishing at `A`'s end, `B` running
/// `\immediate\pdfobj`: the line is `A, finish, B`; the file numbers
/// page 1's objects, then `B`'s object.
/// The line's and the pieces' items' last walks ([`pdf_exact`]).
type LineMemos = (
    crate::val::CatMemo<ValueId>,
    crate::val::CatMemo<(i64, ValueId)>,
);
static LINE_MEMO: std::sync::OnceLock<std::sync::Mutex<LineMemos>> = std::sync::OnceLock::new();

/// The last link's line, in pdfTeX's order, and its pieces' items, by
/// piece ([`pdf_exact`]).
#[derive(Default)]
struct LinkInputs {
    inst: u64,
    line: Vec<ValueId>,
    ordered: std::sync::Arc<Vec<ValueId>>,
    items: Vec<(i64, ValueId)>,
    units: std::sync::Arc<crate::FxHashMap<i64, ValueId>>,
}

static LINK_INPUTS: std::sync::OnceLock<std::sync::Mutex<LinkInputs>> = std::sync::OnceLock::new();

/// At a collection ([`crate::collect`]): the link's walks of the line and
/// the items, and its inputs, dropped; the next link reads them anew.
pub(crate) fn clear_line_memos() {
    if let Some(m) = LINE_MEMO.get() {
        *m.locked() = LineMemos::default();
    }
    if let Some(m) = LINK_INPUTS.get() {
        *m.locked() = LinkInputs::default();
    }
}

#[allow(clippy::too_many_lines)]
fn pdf_exact(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let v = cx.values();
    let t0 = std::time::Instant::now();
    // (in pdfTeX's order: a page firing inside a piece before the
    // piece's records after it)
    // (both walked from the last link's: their ropes' nodes this edit
    // left as they were are not decoded again)
    let memo = LINE_MEMO.get_or_init(Default::default);
    let line_seq = cx.fold(v.field(crate::pdfout::XPAGES), None, None)?;
    let items_seq = cx.fold(v.field(crate::pages::LINK_ITEMS), None, None)?;
    crate::timeline::note("link: the line read: the folds", t0.elapsed());
    let (line, items) = {
        let mut g = memo.locked();
        (
            g.0.walk(v, line_seq, Some),
            g.1.walk(v, items_seq, |u| {
                v.with_list(u, |xs| Some((v.as_int(*xs.first()?)?, *xs.get(2)?)))
                    .flatten()
            }),
        )
    };

    let t_acc = t0.elapsed();
    // (the line in pdfTeX's order, and the pieces' items by piece (the
    // pages' sealed lines, `crate::pages::seal`): the last link's where
    // the same, the items of the pieces that changed put in, the link
    // told which those are)
    let (xpages, units, changed) = {
        let mut g = LINK_INPUTS.get_or_init(Default::default).locked();
        if g.inst != v.epoch() {
            *g = LinkInputs {
                inst: v.epoch(),
                ..LinkInputs::default()
            };
        }
        if g.line != line {
            g.ordered = std::sync::Arc::new(crate::pdfout::order_line(v, &line));
            g.line.clone_from(&line);
        }
        let changed = if g.items.len() == items.len() && !g.units.is_empty() {
            let ps: phitex_defuse::FxHashSet<i64> = g
                .items
                .iter()
                .zip(&items)
                .filter(|(a, b)| a != b)
                .flat_map(|(a, b)| [a.0, b.0])
                .collect();
            let units = std::sync::Arc::make_mut(&mut g.units);
            for &p in &ps {
                match items.iter().find(|x| x.0 == p) {
                    Some(&(_, u)) => units.insert(p, u),
                    None => units.remove(&p),
                };
            }
            Some(ps)
        } else {
            let mut units = crate::FxHashMap::default();
            units.reserve(items.len());
            for &(p, u) in &items {
                units.entry(p).or_insert(u);
            }
            g.units = std::sync::Arc::new(units);
            None
        };
        g.items = items;
        (g.ordered.clone(), g.units.clone(), changed)
    };
    if crate::exec::obs_flag!("PHITEX_LINE_TRACE") {
        for (tag, l) in [("in", &line), ("out", &xpages)] {
            for &x in l {
                eprintln!("line {tag}: {}", crate::pdfout::line_show(v, x));
            }
        }
    }
    let t_order = t0.elapsed();
    let date = field(cx, "pdfdate")?.map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = field(cx, "jobname")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let g2u = field(cx, "pdf:glyphtounicode")?;
    // (the format's fonts by key, numbered first in the order it loaded
    // them, by their fields, defined once at their loads; then the
    // line's, in the order of their first loads)
    let order: Vec<i64> = field(cx, crate::api::FONT_ORDER)?
        .map(|x| list_of(v, x).iter().map(|&k| int_of(v, k)).collect())
        .unwrap_or_default();
    let mut seed = Vec::new();
    for &k in &order {
        let name = crate::fields::font_cell_name(k, crate::fields::FontCell::Info);
        let info = cx.read(v.field(&name))?;
        let (n, sz) = crate::fonts::info(v, info)
            .map(|i| (i.name, i.size))
            .unwrap_or_default();
        seed.push((u32::try_from(k).unwrap_or(0), n, sz));
    }
    let mut fonts: Vec<u32> = seed.iter().map(|s| s.0).collect();
    for r in crate::pdfout::recs(v, &xpages) {
        if let crate::pdfout::Rec::FontLoad(k) = r
            && let Ok(k) = u32::try_from(k)
            && !fonts.contains(&k)
        {
            fonts.push(k);
        }
    }
    let f = crate::pdfout::EndFields {
        date,
        jobname,
        g2u,
        fonts,
        seed,
        info_omit_date: i32::try_from(int_field(cx, "param:pdfinfoomitdate")?).unwrap_or(0),
        suppress_ptex_info: i32::try_from(int_field(cx, "param:pdfsuppressptexinfo")?).unwrap_or(0),
    };
    let t_seed = t0.elapsed();
    let l = crate::pdfout::finish_exact(v, &f, &xpages, &units, changed.as_ref(), &o.pdf_cmds);
    let t_fin = t0.elapsed();
    // a page the link shipped and the writer could not write is the
    // job's error, in the log and loudly, naming the node that shipped
    // the page
    for (msg, node) in &l.errors {
        let e = v.list(vec![v.str("error"), v.str(msg), v.int(-1)]);
        cx.contribute(v.field("log"), v.seq_unit(e));
        cx.contribute(v.field(crate::run::LOG_ERRORS), v.int(1));
        let d = v.list(vec![
            v.int(1),
            v.str(&format!(
                "{msg} (on a page the output places, written by node {})",
                u64::from_ne_bytes(node.to_ne_bytes())
            )),
            ValueId::UNIT,
            v.int(-1),
            v.int(*node),
            v.str(""),
            v.int(0),
        ]);
        cx.contribute(v.field("diags"), v.seq_unit(d));
    }
    cx.contribute(
        v.field(crate::pdfout::LINKED),
        v.seq_unit(v.list(vec![
            v.int(i64::try_from(l.placed).unwrap_or(0)),
            v.int(i64::try_from(l.shipped).unwrap_or(0)),
        ])),
    );
    // (each page's identity, for a viewer: `crate::api::Build::pages`)
    // (one list, made once: not a sequence grown page by page)
    let shown = v.list(l.shown.iter().map(|&h| v.int(i64::from_ne_bytes(h.to_ne_bytes()))).collect());
    cx.contribute(v.field(crate::pdfout::SHOWN), v.seq_unit(shown));
    let x = match l.layout {
        Some(lay) => {
            let n = lay.len();
            file_value(v, File::Layout(lay), l.hash.unwrap_or(0), n)
        }
        None => v.str(""),
    };
    {
        use crate::timeline::note;
        note("link: the line read", t_acc);
        note("link: ordered", t_order.saturating_sub(t_acc));
        note("link: the fields", t_seed.saturating_sub(t_order));
        note("link: linked", t_fin.saturating_sub(t_seed));
        note("link: the value", t0.elapsed().saturating_sub(t_fin));
    }
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "pdf exact: line read {t_acc:?}, ordered {:?}, fields {:?}, link {:?}, value {:?}",
            t_order.saturating_sub(t_acc),
            t_seed.saturating_sub(t_order),
            t_fin.saturating_sub(t_seed),
            t0.elapsed().saturating_sub(t_fin)
        );
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build's linked file stays readable however many files other
    /// builds of the process link meanwhile, and goes with its build.
    /// (Kept a few for all builds at once, the latest one of a build was
    /// evicted by the others' when several ran in one process: the build
    /// read an empty output.)
    #[test]
    fn a_linked_file_is_its_builds_whatever_others_link() {
        let mine = phitex_defuse::Values::new();
        let x = bytes_value(&mine, b"%PDF mine".to_vec(), None);
        let others: Vec<phitex_defuse::Values> =
            (0..8).map(|_| phitex_defuse::Values::new()).collect();
        for (k, o) in others.iter().enumerate() {
            for n in 0..6 {
                let _ = bytes_value(o, format!("%PDF {k} {n}").into_bytes(), None);
            }
        }
        assert_eq!(file_bytes_shared(&mine, x).as_slice(), b"%PDF mine");
        forget_linked(mine.instance());
        assert!(file_bytes_shared(&mine, x).is_empty());
        for o in &others {
            forget_linked(o.instance());
        }
    }

    /// Two files of one hash and length (forced: both given hash 7) are
    /// two values, each reading its own bytes; the first again is its
    /// value again. (Kept by the hash alone, the second file was not
    /// kept and its value read the first file's bytes.)
    #[test]
    fn files_whose_hashes_meet_are_two_values() {
        let v = phitex_defuse::Values::new();
        let a = bytes_value(&v, b"%PDF one".to_vec(), Some(7));
        let b = bytes_value(&v, b"%PDF two".to_vec(), Some(7));
        assert_ne!(a, b);
        assert_eq!(file_bytes_shared(&v, a).as_slice(), b"%PDF one");
        assert_eq!(file_bytes_shared(&v, b).as_slice(), b"%PDF two");
        assert_eq!(bytes_value(&v, b"%PDF one".to_vec(), Some(7)), a);
        forget_linked(v.instance());
    }
}
