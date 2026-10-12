//! The PDF file (pdfTeX §750–§815): the shipped pages through
//! `phitex-pdf`, as `Dvi` writes them through `phitex-dvi`.
//!
//! Each page in the `pages` accumulator carries what `\shipout` resolved
//! of it in PDF mode: the parameters pdfTeX reads per page and the
//! writer's data for each pdfTeX whatsit (a literal with its mode, or a
//! colour stack command). The colour stacks are the writer's state across
//! pages (`utils.c`): a fold over the pages here, as `pdf_ship_out`
//! keeps them.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_pdf::map::{MapEntry, parse_map};
use phitex_pdf::{EndData, FontSource, PdfParams, PdfWriter};
use phitex_tfm::ScaledFont;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::Node;

use crate::fonts::bytes_of;
use crate::nodes::{self, Side};
use crate::val::{int_of, list_of, str_of};

/// `pdftex_banner` of the oracle's pdfTeX.
pub(crate) const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// `COLOR_DEFAULT` (`utils.c`).
const COLOR_DEFAULT: &str = "0 g 0 G";

type MapCache = Mutex<HashMap<u64, Arc<BTreeMap<Vec<u8>, MapEntry>>>>;

/// Parsed map files: a pure function's memo by the file's value.
fn maps() -> &'static MapCache {
    static C: OnceLock<MapCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A file the writer reads (`pdftex.map`, a `.pfb`, an `.enc`).
enum Got {
    /// The client has not supplied it yet: asked for.
    Waiting,
    Absent,
    Text(ValueId),
}

fn file(cx: &mut Ctx<'_>, name: &str) -> Result<Got, Stop> {
    let v = cx.values();
    let Some(x) = cx.use_field(v.field(&format!("file:{name}")))? else {
        let id = v.int(i64::from_ne_bytes(cx.node().raw().to_ne_bytes()));
        let e = v.list(vec![v.str("file"), id, v.str(name)]);
        cx.contribute(v.field("needs"), v.seq_unit(e));
        return Ok(Got::Waiting);
    };
    Ok(list_of(v, x).get(1).map_or(Got::Absent, |&t| Got::Text(t)))
}

/// One font as the writer asks for it.
struct Font {
    name: Vec<u8>,
    metrics: ScaledFont,
    entry: Option<MapEntry>,
    ff: Option<Vec<u8>>,
    enc: Option<Vec<u8>>,
}

#[derive(Default)]
struct Fonts(HashMap<FontId, Font>);

impl FontSource for Fonts {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        static EMPTY: OnceLock<ScaledFont> = OnceLock::new();
        self.0.get(&f).map_or_else(
            || {
                EMPTY.get_or_init(|| ScaledFont {
                    size: 0,
                    design_size: 0,
                    chars: Vec::new(),
                    kerns: Vec::new(),
                    params: Vec::new(),
                })
            },
            |x| &x.metrics,
        )
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.0.get(&f).map_or(&[], |x| &x.name)
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.0.get(&f)?.entry.clone()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.0.get(&f)?.ff.clone()
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.0.get(&f)?.enc.clone()
    }
}

/// A colour stack (`colstack_type`, `utils.c`), page mode only.
struct Stack {
    saved: Vec<Option<String>>,
    current: Option<String>,
    mode: i64,
    page_start: bool,
}

/// A literal as `phitex-pdf` takes it: a special with the mode's prefix
/// (`literal`, pdfTeX §693).
fn special(mode: i64, s: &str) -> Vec<u8> {
    let pre = match mode {
        1 => "PDF:page:",
        2 => "PDF:direct:",
        _ => "PDF:",
    };
    let mut b = pre.as_bytes().to_vec();
    b.extend(bytes_of(s));
    b
}

/// The stacks `\pdfcolorstackinit` made by this page (stack 0 is
/// pdfTeX's own: `colstacks_first_init`).
fn grow_stacks(v: &Values, stacks: &mut Vec<Stack>, defs: Option<ValueId>) {
    if stacks.is_empty() {
        stacks.push(Stack {
            saved: Vec::new(),
            current: Some(COLOR_DEFAULT.to_owned()),
            mode: 2,
            page_start: true,
        });
    }
    let defs = defs.map(|d| list_of(v, d)).unwrap_or_default();
    for d in defs.iter().skip(stacks.len() - 1) {
        let xs = list_of(v, *d);
        let s = xs.get(2).map(|&y| str_of(v, y)).unwrap_or_default();
        stacks.push(Stack {
            saved: Vec::new(),
            current: (!s.is_empty()).then_some(s),
            mode: xs.get(1).map_or(0, |&y| int_of(v, y)),
            page_start: xs.first().is_some_and(|&y| int_of(v, y) != 0),
        });
    }
}

/// `pdf_out_colorstack_startpage`.
fn page_start(stacks: &[Stack]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for st in stacks {
        // colorstackskippagestart
        if !st.page_start {
            continue;
        }
        match &st.current {
            Some(c) if c != COLOR_DEFAULT => out.push(special(st.mode, c)),
            _ => {}
        }
    }
    out
}

/// `pdf_out_colorstack` (pdfTeX): the literal a colour stack command
/// writes, if any.
fn colorstack(stacks: &mut [Stack], n: i64, act: &str, text: &str) -> Option<Vec<u8>> {
    // PDF-BACKEND-GAP: "Color stack n is not initialized for use!" goes
    // to the log, which the writer does not reach
    let st = stacks.get_mut(usize::try_from(n).ok()?)?;
    match act {
        "set" => {
            st.current = Some(text.to_owned());
            (!text.is_empty()).then(|| special(st.mode, text))
        }
        "push" => {
            let old = st.current.take();
            st.saved.push(old);
            st.current = (!text.is_empty()).then(|| text.to_owned());
            (!text.is_empty()).then(|| special(st.mode, text))
        }
        "pop" => {
            let prev = st.saved.pop()?;
            st.current = prev;
            st.current
                .as_deref()
                .filter(|c| !c.is_empty())
                .map(|c| special(st.mode, c))
        }
        "current" => st
            .current
            .as_deref()
            .filter(|c| !c.is_empty())
            .map(|c| special(st.mode, c)),
        _ => None,
    }
}

/// The text of a list of strings, run together (`concat_tokens`).
fn joined(v: &Values, x: Option<ValueId>) -> Vec<u8> {
    let mut out = Vec::new();
    for t in x.map(|x| list_of(v, x)).unwrap_or_default() {
        out.extend(bytes_of(&str_of(v, t)));
    }
    out
}

fn param(v: &Values, xs: &[ValueId], k: usize) -> i32 {
    xs.get(k)
        .map_or(0, |&y| i32::try_from(int_of(v, y)).unwrap_or(0))
}

/// The page's parameters as `\shipout` read them.
fn page_params(v: &Values, p: &mut PdfParams, xs: &[ValueId]) {
    p.h_origin = param(v, xs, 1);
    p.v_origin = param(v, xs, 2);
    p.page_width = param(v, xs, 3);
    p.page_height = param(v, xs, 4);
    p.h_offset = param(v, xs, 5);
    p.v_offset = param(v, xs, 6);
    p.compress_level = param(v, xs, 7).clamp(0, 9);
    p.decimal_digits = param(v, xs, 8);
    p.minor_version = param(v, xs, 9);
    p.major_version = param(v, xs, 10);
    p.mag = param(v, xs, 11);
}

/// The PDF file of `pages` (each `[box, counts, fonts, pdf, data]`).
#[allow(clippy::too_many_lines)]
pub(crate) fn write(cx: &mut Ctx<'_>, pages: &[ValueId]) -> Result<ValueId, Stop> {
    let v = cx.values();
    let date = cx
        .use_field(v.field("pdfdate"))?
        .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
    let jobname = cx
        .use_field(v.field("jobname"))?
        .map(|x| str_of(v, x))
        .unwrap_or_default();
    // the fonts, with their map entries and files
    let mut fonts = Fonts::default();
    let mut waiting = false;
    let map = match file(cx, "pdftex.map")? {
        Got::Waiting => {
            waiting = true;
            None
        }
        Got::Absent => None,
        Got::Text(m) => {
            let key = m.raw();
            let cached = maps()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
                .cloned();
            Some(cached.unwrap_or_else(|| {
                let parsed = Arc::new(parse_map(&bytes_of(&str_of(v, m))));
                maps()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key, parsed.clone());
                parsed
            }))
        }
    };
    for &page in pages {
        let xs = list_of(v, page);
        for fx in xs.get(2).map(|&l| list_of(v, l)).unwrap_or_default() {
            let ys = list_of(v, fx);
            let id = u32::try_from(ys.first().map_or(0, |&y| int_of(v, y))).unwrap_or(0);
            if fonts.0.contains_key(&id) {
                continue;
            }
            let name = ys.get(1).map(|&y| str_of(v, y)).unwrap_or_default();
            let s = i32::try_from(ys.get(3).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
            let Some(l) = ys.get(2).and_then(|&f| crate::fonts::load(v, f, s)) else {
                continue;
            };
            let entry = map.as_ref().and_then(|m| m.get(name.as_bytes()).cloned());
            let mut get = |n: Option<&[u8]>| -> Result<Option<Vec<u8>>, Stop> {
                let Some(n) = n else { return Ok(None) };
                let n = String::from_utf8_lossy(n).into_owned();
                Ok(match file(cx, &n)? {
                    Got::Waiting => {
                        waiting = true;
                        None
                    }
                    Got::Absent => None,
                    Got::Text(b) => Some(bytes_of(&str_of(v, b))),
                })
            };
            let ff = get(entry.as_ref().and_then(|e| e.ff_name.as_deref()))?;
            let enc = get(entry.as_ref().and_then(|e| e.encname.as_deref()))?;
            fonts.0.insert(
                id,
                Font {
                    name: name.into_bytes(),
                    metrics: l.at.clone(),
                    entry,
                    ff,
                    enc,
                },
            );
        }
    }
    if waiting {
        return Ok(ValueId::UNIT);
    }
    let mut params = PdfParams {
        date,
        banner: BANNER.to_owned(),
        output_file_name: format!("{jobname}.pdf"),
        ..PdfParams::default()
    };
    if let Some(first) = pages.first() {
        let xs = list_of(v, *first);
        let p = xs.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
        page_params(v, &mut params, &p);
    }
    let mut w = PdfWriter::new(params);
    let mut stacks: Vec<Stack> = Vec::new();
    for &page in pages {
        let xs = list_of(v, page);
        let mut side = Side::default();
        let Some(Node::Box(bx)) = xs.first().and_then(|&b| nodes::decode(v, b, &mut side)) else {
            continue;
        };
        let p = xs.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
        page_params(v, w.params_mut(), &p);
        grow_stacks(v, &mut stacks, p.get(12).copied());
        w.set_page_start(page_start(&stacks));
        let mut resolved: HashMap<u32, ValueId> = HashMap::new();
        for e in xs.get(4).map(|&l| list_of(v, l)).unwrap_or_default() {
            let ys = list_of(v, e);
            if let (Some(&h), Some(&d)) = (ys.first(), ys.get(1)) {
                resolved.insert(u32::try_from(int_of(v, h)).unwrap_or(0), d);
            }
        }
        let whats = side.whats.clone();
        let mut wh = |h: u32, _leaders: bool| -> Option<Vec<u8>> {
            if let Some(&d) = resolved.get(&h) {
                let ys = list_of(v, d);
                let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
                let text = |k: usize| ys.get(k).map(|&t| str_of(v, t)).unwrap_or_default();
                return match kind.as_str() {
                    "lit" => Some(special(ys.get(1).map_or(0, |&m| int_of(v, m)), &text(2))),
                    "cs" => colorstack(
                        &mut stacks,
                        ys.get(1).map_or(0, |&n| int_of(v, n)),
                        &text(2),
                        &text(3),
                    ),
                    _ => None,
                };
            }
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
        let _ = w.ship_out(&bx, &fonts, &mut wh);
    }
    let get_str = |cx: &mut Ctx<'_>, f: &str| -> Result<Vec<u8>, Stop> {
        let x = cx.use_field(v.field(f))?;
        Ok(joined(v, x))
    };
    let catalog = get_str(cx, "pdf:catalog")?;
    let info = get_str(cx, "pdf:info")?;
    let trailer = get_str(cx, "pdf:trailer")?;
    let int = |cx: &mut Ctx<'_>, f: &str| -> Result<i32, Stop> {
        Ok(cx
            .use_field(v.field(f))?
            .map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0)))
    };
    let end = EndData {
        catalog,
        info,
        trailer,
        info_omit_date: int(cx, "param:pdfinfoomitdate")?,
        suppress_ptex_info: int(cx, "param:pdfsuppressptexinfo")?,
    };
    let bytes = w.finish_with(&end).unwrap_or_default();
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}
