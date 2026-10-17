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
pub(crate) fn output(cx: &mut Ctx<'_>, _o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    // the scan's last state first: every continuation before this node
    // may define it, so the read waits, parked, until the last page has
    // fired and placed its finisher; read first, the pages are all there
    // (a node started before would assemble the pages so far and run
    // again at each page added)
    cx.use_field(v.field(crate::pages::SCAN))?;
    let pages = v.seq_to_vec(cx.acc_range(v.field("pages"), None)?);
    let pdf = is_pdf(v, &pages);
    let r = if pdf {
        Ok(field(cx, LINKED)?
            .filter(|&x| x != ValueId::UNIT)
            .unwrap_or_else(|| v.str("")))
    } else {
        dvi_file(cx, &pages)
    };
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "output file: {} pages, {:?}{}",
            pages.len(),
            t0.elapsed(),
            if r.is_ok() { "" } else { " (stopped)" }
        );
    }
    r
}

/// The exact PDF file the link made (`pdf:file`), read by the end node.
pub(crate) const LINKED: &str = "pdf:file";

/// The bytes of the files the link made, by their hash: the file's value
/// is `["bytes", hash, length]`, not the file as a string (copied,
/// interned and hashed as characters, then copied back to bytes: O(file)
/// three times an edit). The latest few kept.
fn linked_files() -> &'static LinkedFiles {
    static M: std::sync::OnceLock<LinkedFiles> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// [`linked_files`]: (hash, bytes), the latest last.
type LinkedFiles = std::sync::Mutex<std::collections::VecDeque<(u64, std::sync::Arc<Vec<u8>>)>>;

/// The value standing for file bytes `b` ([`linked_files`]).
fn bytes_value(v: &phitex_defuse::Values, b: Vec<u8>) -> ValueId {
    let h = phitex_defuse::hash_bytes(0x6c69_6e6b_6564, &b);
    let n = b.len();
    let mut m = linked_files()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !m.iter().any(|e| e.0 == h) {
        m.push_back((h, std::sync::Arc::new(b)));
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

/// The bytes of an output value: a file's string, or the bytes a
/// [`bytes_value`] stands for.
pub(crate) fn file_bytes(v: &phitex_defuse::Values, x: ValueId) -> Vec<u8> {
    if let phitex_defuse::Value::List(xs) = &*v.get(x)
        && xs.len() == 3
        && str_of(v, xs[0]) == "bytes"
    {
        let h = u64::from_ne_bytes(int_of(v, xs[1]).to_ne_bytes());
        return linked_files()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|e| e.0 == h)
            .map(|e| e.1.as_ref().clone())
            .unwrap_or_default();
    }
    str_of(v, x)
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
        .collect()
}

/// The format pdfTeX fixed at the first `\shipout` (`fix_pdfoutput`):
/// a page entry is `[box, counts, fonts, pdf params, pdf data]`, the
/// params' first element whether the file is PDF.
fn is_pdf(v: &Values, pages: &[ValueId]) -> bool {
    pages.first().is_some_and(|&p| {
        list_of(v, p)
            .get(3)
            .is_some_and(|&x| crate::val::int_at(v, x, 0) != 0)
    })
}

/// The link's run (spec-redesign §1, `link`): in exact PDF mode the file
/// from the line's records ([`pdf_exact`]), defined for the end node
/// (`pdf:file`), and the loop-backs it gives the next round; nothing in
/// any other mode.
pub(crate) fn link(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    cx.use_field(v.field(crate::pages::SCAN))?;
    let pages = v.seq_to_vec(cx.acc_range(v.field("pages"), None)?);
    if !is_pdf(v, &pages) {
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
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!("link: {} pages, {:?}", pages.len(), t0.elapsed());
    }
    Ok(ValueId::UNIT)
}

/// A field reaching the end node.
fn field(cx: &mut Ctx<'_>, name: &str) -> Result<Option<ValueId>, Stop> {
    let f = cx.values().field(name);
    cx.use_field(f)
}

fn int_field(cx: &mut Ctx<'_>, name: &str) -> Result<i64, Stop> {
    let v = cx.values();
    Ok(field(cx, name)?.map_or(0, |x| int_of(v, x)))
}

/// Bytes as a string of characters 0–255 (the file as a value).
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

// ----------------------------------------------------------------- DVI

/// The DVI file: the preamble, each page shipped in order, the
/// postamble (§617–§642).
///
/// Example: two pages whose boxes are `b1` and `b2`, with `\count0` 1
/// and 2, give `pre … bop 1 … eop bop 2 … eop post … post_post`; the
/// value is those bytes as characters. No page: no file (§642), the
/// empty string.
fn dvi_file(cx: &mut Ctx<'_>, pages: &[ValueId]) -> Result<ValueId, Stop> {
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
    let head = phitex_defuse::hash_bytes(
        phitex_defuse::hash_words(v.instance(), &[u64::from(mag.cast_unsigned())]),
        comment.as_bytes(),
    );
    let ids: Vec<u64> = pages.iter().map(|p| p.raw()).collect();
    let (mut w, start, mut snaps) = {
        let m = dvi_memo()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        found.unwrap_or_else(|| (phitex_dvi::DviWriter::new(mag, &comment), 0, Vec::new()))
    };
    // (the pages' fonts by id, resolved here from the fonts' fields: a
    // font's fields are defined once, at its load)
    let mut defs: phitex_defuse::FxHashMap<u32, phitex_dvi::FontDef> =
        phitex_defuse::FxHashMap::default();
    for &page in &pages[start..] {
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
            let info = cx.use_field(v.field(&name))?;
            if let Some(i) = crate::fonts::info(v, info)
                && let Some(file) = i.file
                && let Some(l) = crate::fonts::load(v, file, i.s)
            {
                defs.insert(id, crate::fonts::font_def(&l, &i.name));
            }
        }
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
            let mut wi = phitex_dvi::DviWriter::new(mag, &comment);
            for &f in &before[i] {
                if let Some(d) = defs.get(&f) {
                    wi.assume_defined(f, d.clone());
                }
            }
            dvi_page_kept(v, &mut wi, rest[i], &defs)
        })
    };
    for (i, (&page, pb)) in rest.iter().zip(apart).enumerate() {
        let i = i + start;
        if i > start && (i - start).is_multiple_of(DVI_SNAP) {
            snaps.push((i, w.clone()));
        }
        if !pb.is_some_and(|pb| w.ship_kept(&pb)) {
            dvi_page(v, &mut w, page, &defs);
        }
    }
    *dvi_memo()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(DviMemo { head, ids, snaps });
    let bytes = w.finish();
    Ok(v.intern(Value::Str(latin1(&bytes).into_boxed_str())))
}

/// The pages between two of the DVI writer's kept states ([`DviMemo`]).
const DVI_SNAP: usize = 16;

/// The last DVI file of the process: its pages (by value) and the writer
/// every [`DVI_SNAP`] pages.
struct DviMemo {
    head: u64,
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
) -> Option<phitex_dvi::PageBytes> {
    let (bx, counts, whats) = dvi_parts(v, page)?;
    let fonts = |f: u32| font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| special_of(v, &whats, h);
    w.ship_out_kept(&bx, counts, &fonts, &mut wh).ok()
}

/// A page entry's box (decoded), counts and whatsits.
fn dvi_parts(
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
    Some((bx, counts, side.whats))
}

/// Font `f`'s definition as the DVI file needs it.
fn font_def_of(
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
    })
}

/// A `\special`'s bytes, for whatsit `h` of a page.
fn special_of(v: &Values, whats: &[ValueId], h: u32) -> Option<Vec<u8>> {
    let w = *whats.get(h as usize)?;
    let ys = list_of(v, w);
    if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
        Some(bytes_of(
            &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
        ))
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
) {
    let Some((bx, counts, whats)) = dvi_parts(v, page) else {
        return;
    };
    let fonts = |f: u32| font_def_of(defs, f);
    let mut wh = |h: u32, _leaders: bool| special_of(v, &whats, h);
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
fn pdf_exact(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let v = cx.values();
    // (in pdfTeX's order: a page firing inside a piece before the
    // piece's records after it)
    let xpages = crate::pdfout::order_line(
        v,
        &v.seq_to_vec(cx.acc_range(v.field(crate::pdfout::XPAGES), None)?),
    );
    let date = field(cx, "pdfdate")?.map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = field(cx, "jobname")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let g2u = field(cx, "pdf:glyphtounicode")?;
    let fonts = int_field(cx, "fontcount")?;
    // (the fonts loaded before the line's first load record, a format's:
    // numbered first, by their fields, defined once at their loads)
    let first = xpages.iter().find_map(|&x| {
        let xs = list_of(v, x);
        (xs.first().map(|&k| str_of(v, k)).as_deref() == Some("fontload"))
            .then(|| xs.get(3).map_or(0, |&n| int_of(v, n)))
    });
    let n0 = first.map_or(fonts, |k| k - 1);
    let mut seed = Vec::new();
    for id in 1..=n0 {
        let name = crate::fields::font_cell_name(id, crate::fields::FontCell::Info);
        let info = cx.use_field(v.field(&name))?;
        seed.push(
            crate::fonts::info(v, info)
                .map(|i| (i.name, i.size))
                .unwrap_or_default(),
        );
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
    let l = crate::pdfout::finish_exact(v, &f, &xpages, &o.pdf_cmds);
    // a page the link shipped and the writer could not write is the
    // job's error, in the log and loudly, naming the node that shipped
    // the page
    for (msg, node) in &l.errors {
        let e = v.list(vec![v.str("error"), v.str(msg), v.int(-1)]);
        cx.contribute(v.field("log"), v.seq_unit(e));
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
    // (the loop-backs the link gives the next round)
    for (name, x) in [
        (crate::pdfout::OBJNUMS, l.objnums),
        (crate::pdfout::FONT_TAGS, l.ftags),
    ] {
        cx.publish(v.field(name), x);
    }
    Ok(bytes_value(v, l.file))
}
