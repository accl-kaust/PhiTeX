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
//! - **PDF, relaxed** (`pdf:relaxed` on, no `pdf:exact` need): every
//!   page and every read-time object is already a piece with symbolic
//!   handles (`crate::pdfrelaxed`, `pdf:pieces` in document order);
//!   [`pdf_relaxed`] numbers them in one pass with
//!   `phitex_pdf::relaxed::finish_relaxed` and writes the file. A
//!   document the mode cannot write asks for exact mode (a `pdf:exact`
//!   need) and the build evaluates again.
//! - **PDF, exact** (byte-identical object numbers): [`pdf_exact`], the
//!   link (spec-redesign §1): the line's records (`crate::pdfout::XPAGES`:
//!   the pages written apart, the read-time commands, the document-level
//!   ones) put on one writer state in order, numbered as pdfTeX numbers
//!   them; what the commands and the pages' writers guessed given back
//!   as loop-backs for the next round.

use std::sync::Arc;

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_pdf::PdfParams;
use phitex_pdf::relaxed::{DocCmd, Piece, finish_relaxed};
use phitex_typeset::node::Node;

use crate::fonts::bytes_of;
use crate::nodes::{self, Side};
use crate::pdfout::BANNER;
use crate::val::{int_of, list_of, str_of};

/// The end node's run: the file as a string value (empty: no file, §642).
pub(crate) fn output(cx: &mut Ctx<'_>, o: &crate::run::Ops) -> Result<ValueId, Stop> {
    let t0 = std::time::Instant::now();
    let v = cx.values();
    // the scan's last state first: every continuation before this node
    // may define it, so the read waits, parked, until the last page has
    // fired and placed its finisher; read first, the pages are all there
    // (a node started before would assemble the pages so far and run
    // again at each page added)
    cx.use_field(v.field(crate::pages::SCAN))?;
    let pages = v.seq_to_vec(cx.acc_range(v.field("pages"), None)?);
    // the format pdfTeX fixed at the first `\shipout` (`fix_pdfoutput`):
    // a page entry is `[box, counts, fonts, pdf params, pdf data]`, the
    // params' first element whether the file is PDF
    let pdf = pages.first().is_some_and(|&p| {
        list_of(v, p)
            .get(3)
            .is_some_and(|&x| crate::val::int_at(v, x, 0) != 0)
    });
    let r = if !pdf {
        dvi_file(cx, &pages)
    } else if relaxed_mode(cx)? {
        pdf_relaxed(cx, &pages)
    } else {
        pdf_exact(cx, o)
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

/// A field reaching the end node.
fn field(cx: &mut Ctx<'_>, name: &str) -> Result<Option<ValueId>, Stop> {
    let f = cx.values().field(name);
    cx.use_field(f)
}

fn int_field(cx: &mut Ctx<'_>, name: &str) -> Result<i64, Stop> {
    let v = cx.values();
    Ok(field(cx, name)?.map_or(0, |x| int_of(v, x)))
}

/// Whether the relaxed PDF writer is on and no document asked for exact
/// mode.
fn relaxed_mode(cx: &mut Ctx<'_>) -> Result<bool, Stop> {
    Ok(int_field(cx, "pdf:relaxed")? == 1 && int_field(cx, "pdf:exact")? == 0)
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
    let mut w = phitex_dvi::DviWriter::new(mag, &comment);
    for &page in pages {
        dvi_page(v, &mut w, page);
    }
    let bytes = w.finish();
    Ok(v.intern(Value::Str(latin1(&bytes).into_boxed_str())))
}

/// One page shipped on `w` (§638–§640): the box decoded, its counts,
/// its fonts' definitions from the page's font list (`[id, name, file,
/// size]` each), its `\special`s from its whatsits.
fn dvi_page(v: &Values, w: &mut phitex_dvi::DviWriter, page: ValueId) {
    let xs = list_of(v, page);
    let mut side = Side::default();
    // (a page is decoded once: its nodes are not kept in this thread's
    // cache, whose copies cost more than the decoding)
    let was = crate::val::NO_KEEP.with(|c| c.replace(true));
    let bx = xs.first().and_then(|&b| nodes::decode(v, b, &mut side));
    crate::val::NO_KEEP.with(|c| c.set(was));
    let Some(Node::Box(bx)) = bx else {
        return;
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
    let mut defs = phitex_defuse::FxHashMap::default();
    for fx in xs.get(2).map(|&l| list_of(v, l)).unwrap_or_default() {
        let ys = list_of(v, fx);
        let id = u32::try_from(ys.first().map_or(0, |&y| int_of(v, y))).unwrap_or(0);
        let name = ys.get(1).map(|&y| str_of(v, y)).unwrap_or_default();
        let s = i32::try_from(ys.get(3).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
        if let Some(&file) = ys.get(2)
            && let Some(l) = crate::fonts::load(v, file, s)
        {
            defs.insert(id, crate::fonts::font_def(&l, &name));
        }
    }
    let fonts = |f: u32| {
        defs.get(&f).cloned().unwrap_or(phitex_dvi::FontDef {
            checksum: [0; 4],
            size: 0,
            design_size: 0,
            area: Vec::new(),
            name: Vec::new(),
            widths: Vec::new(),
        })
    };
    let whats = side.whats.clone();
    let mut wh = |h: u32, _leaders: bool| -> Option<Vec<u8>> {
        let w = *whats.get(h as usize)?;
        let ys = list_of(v, w);
        if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
            Some(bytes_of(
                &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
            ))
        } else {
            None
        }
    };
    let _ = w.ship_out(&bx, counts, &fonts, &mut wh);
}

// -------------------------------------------------------- PDF, relaxed

/// The PDF file from the pieces: the pages' and the read-time objects'
/// (`pdf:pieces`: `[0, key]` a page, `[1, key]` an object, in document
/// order), the parameters the last page shipped with, the document-level
/// commands as fields reaching here. `Err(reason)` from the writer is a
/// `pdf:exact` need: the build evaluates again in exact mode.
///
/// Example: a two-page document with one `\pdfobj` between them gives
/// pieces `[0,k1] [1,k3] [0,k2]`; the file numbers page 1's objects,
/// page 2's, then the object, then the fonts, the page tree, the
/// catalog and `/Info`.
fn pdf_relaxed(cx: &mut Ctx<'_>, pages_shipped: &[ValueId]) -> Result<ValueId, Stop> {
    let v = cx.values();
    let all = v.seq_to_vec(cx.acc_range(v.field(crate::pdfrelaxed::PIECES), None)?);
    let (mut pages, mut objs): (Vec<Arc<Piece>>, Vec<Arc<Piece>>) = (Vec::new(), Vec::new());
    for e in all {
        let xs = list_of(v, e);
        let (Some(&kind), Some(&k)) = (xs.first(), xs.get(1)) else {
            continue;
        };
        let Some(p) = crate::pdfrelaxed::piece(crate::pdfrelaxed::int_key(v, k)) else {
            continue;
        };
        if int_of(v, kind) == 0 {
            pages.push(p);
        } else {
            objs.push(p);
        }
    }
    // the parameters: the last page's, the fixed ones the first write's
    let date = field(cx, "pdfdate")?.map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = field(cx, "jobname")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let mut p = PdfParams {
        date,
        banner: BANNER.to_owned(),
        output_file_name: format!("{jobname}.pdf"),
        ..PdfParams::default()
    };
    if let Some(&last) = pages_shipped.last()
        && let Some(&pp) = list_of(v, last).get(3)
    {
        crate::pdfout::page_params(v, &mut p, &list_of(v, pp));
    }
    if let Some(x) = field(cx, "pdf:fixed")? {
        p = crate::pdfrelaxed::fixed_of(v, x, &p);
    }
    p.info_omit_date = i32::try_from(int_field(cx, "param:pdfinfoomitdate")?).unwrap_or(0);
    p.suppress_ptex_info = i32::try_from(int_field(cx, "param:pdfsuppressptexinfo")?).unwrap_or(0);
    let cmds = doc_cmds(cx)?;
    match finish_relaxed(&p, &pages, &objs, &cmds) {
        Ok(bytes) => Ok(v.intern(Value::Str(latin1(&bytes).into_boxed_str()))),
        Err(why) => {
            let id = v.int(i64::from_ne_bytes(cx.node().raw().to_ne_bytes()));
            let e = v.list(vec![v.str("pdf:exact"), id, v.str(&why)]);
            cx.contribute(v.field("needs"), v.seq_unit(e));
            Ok(v.str(""))
        }
    }
}

/// The document-level commands (`\pdfinfo`, `\pdfcatalog`, `\pdfnames`,
/// `\pdftrailer`, `\pdftrailerid`, `\pdfoutline`, the font commands),
/// from the fields the text defined, as they reach the end.
fn doc_cmds(cx: &mut Ctx<'_>) -> Result<Vec<DocCmd>, Stop> {
    let v = cx.values();
    let mut out = Vec::new();
    let texts = |cx: &mut Ctx<'_>, f: &str| -> Result<Vec<ValueId>, Stop> {
        Ok(field(cx, f)?.map(|x| list_of(v, x)).unwrap_or_default())
    };
    let b = |x: ValueId| bytes_of(&str_of(v, x));
    for t in texts(cx, "pdf:outlines")? {
        let xs = list_of(v, t);
        let (Some(&at), Some(&act), Some(&count), Some(&title)) =
            (xs.first(), xs.get(1), xs.get(2), xs.get(3))
        else {
            continue;
        };
        if let Some(a) = crate::pdfout::action_of(v, act) {
            let at = Some(b(at)).filter(|a| !a.is_empty());
            let count = i32::try_from(int_of(v, count)).unwrap_or(0);
            out.push(DocCmd::Outline(at, a, count, b(title)));
        }
    }
    for t in texts(cx, "pdf:info")? {
        out.push(DocCmd::Info(b(t)));
    }
    for t in texts(cx, "pdf:catalog")? {
        out.push(DocCmd::Catalog(b(t), None));
    }
    if let Some(a) = field(cx, "pdf:openaction")?.and_then(|x| crate::pdfout::action_of(v, x)) {
        out.push(DocCmd::Catalog(Vec::new(), Some(a)));
    }
    for t in texts(cx, "pdf:names")? {
        out.push(DocCmd::Names(b(t)));
    }
    for t in texts(cx, "pdf:trailer")? {
        out.push(DocCmd::Trailer(b(t)));
    }
    if let Some(t) = field(cx, "pdf:trailerid")? {
        out.push(DocCmd::TrailerId(b(t)));
    }
    // the font commands: the last attributes of each font, the
    // characters included, the fonts without built-in ToUnicode
    let count = int_field(cx, "fontcount")?;
    for f in 1..=count {
        if let Some(t) = field(cx, &format!("pdf:fontattr:{f}"))? {
            out.push(DocCmd::FontAttr(u32::try_from(f).unwrap_or(0), b(t)));
        }
    }
    for t in texts(cx, "pdf:includecharslist")? {
        let xs = list_of(v, t);
        if let (Some(&f), Some(&c)) = (xs.first(), xs.get(1)) {
            out.push(DocCmd::IncludeChars(
                u32::try_from(int_of(v, f)).unwrap_or(0),
                b(c),
            ));
        }
    }
    for t in texts(cx, "pdf:nobuiltinlist")? {
        out.push(DocCmd::NoBuiltinToUnicode(
            u32::try_from(int_of(v, t)).unwrap_or(0),
        ));
    }
    for t in texts(cx, "pdf:glyphtounicode")? {
        let xs = list_of(v, t);
        if let (Some(&a), Some(&g)) = (xs.first(), xs.get(1)) {
            out.push(DocCmd::GlyphToUnicode(b(a), b(g)));
        }
    }
    Ok(out)
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
    let xpages = v.seq_to_vec(cx.acc_range(v.field(crate::pdfout::XPAGES), None)?);
    let date = field(cx, "pdfdate")?.map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = field(cx, "jobname")?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    let g2u = field(cx, "pdf:glyphtounicode")?;
    let f = crate::pdfout::EndFields {
        date,
        jobname,
        g2u,
        fonts: int_field(cx, "fontcount")?,
        info_omit_date: i32::try_from(int_field(cx, "param:pdfinfoomitdate")?).unwrap_or(0),
        suppress_ptex_info: i32::try_from(int_field(cx, "param:pdfsuppressptexinfo")?).unwrap_or(0),
    };
    // a placed page the writer could not write is the job's error, in
    // the log and loudly, naming the node that wrote the page (a page
    // written from another mode's handles is never placed: its run is
    // run again, [`crate::pdfout::PagePiece`])
    for (msg, node) in crate::pdfout::placed_errors(v, &xpages) {
        let e = v.list(vec![v.str("error"), v.str(&msg), v.int(-1)]);
        cx.contribute(v.field("log"), v.seq_unit(e));
        let d = v.list(vec![
            v.int(1),
            v.str(&format!(
                "{msg} (on a page the output places, written by node {})",
                u64::from_ne_bytes(node.to_ne_bytes())
            )),
            ValueId::UNIT,
            v.int(-1),
            v.int(node),
            v.str(""),
            v.int(0),
        ]);
        cx.contribute(v.field("diags"), v.seq_unit(d));
    }
    let l = crate::pdfout::finish_exact(v, &f, &xpages, &o.pdf_cmds);
    // (the loop-backs the link gives the next round)
    for (name, x) in [
        (crate::pdfout::OBJNUMS, l.objnums),
        (crate::pdfout::FONT_TAGS, l.ftags),
    ] {
        cx.publish(v.field(name), x);
    }
    Ok(v.intern(Value::Str(latin1(&l.file).into_boxed_str())))
}
