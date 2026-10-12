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
use phitex_pdf::map::{MapEntry, parse_line, parse_map};
use phitex_pdf::{ColorStackCmd, FontSource, LiteralMode, PdfParams, PdfWriter, Whatsit};
use phitex_tfm::ScaledFont;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::Node;

use crate::fonts::bytes_of;
use crate::nodes::{self, Side};
use crate::val::{int_of, list_of, str_of};

/// `pdftex_banner` of the oracle's pdfTeX.
pub(crate) const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// A number for `pdftexspace` if none is free.
const SPACE_FONT: FontId = FontId::MAX - 1;

/// A font map by TFM name.
type Map = BTreeMap<Vec<u8>, MapEntry>;

type MapCache = Mutex<HashMap<u64, Arc<Map>>>;

/// Parsed map files: a pure function's memo by the file's value.
fn maps() -> &'static MapCache {
    static C: OnceLock<MapCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A file the writer reads (`pdftex.map`, a `.pfb`, an `.enc`).
enum Got {
    Absent,
    Text(ValueId),
}

fn file_of(cx: &mut Ctx<'_>, name: &str) -> Result<Got, Stop> {
    let v = cx.values();
    let Some(x) = cx.use_env(v.field(&format!("file:{name}")))? else {
        return Ok(Got::Absent);
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

/// The fonts, and `\pdfspacefont`'s (`pdftexspace`) number if loaded.
#[derive(Default)]
struct Fonts(HashMap<FontId, Font>, Option<FontId>);

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
    fn space_font(&self) -> Option<FontId> {
        self.1
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
    p.obj_compress_level = param(v, xs, 13);
    p.gen_tounicode = param(v, xs, 14);
    let text = |k: usize| {
        xs.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .filter(|t| !t.is_empty())
    };
    p.page_attr = text(15);
    p.pages_attr = text(16);
    p.page_resources = text(17);
}

/// The writer's data for a whatsit `\shipout` resolved
/// ([`crate::pdfprim::writer_data`]).
fn whatsit(v: &Values, d: ValueId) -> Option<Whatsit> {
    let ys = list_of(v, d);
    let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
    let text = |k: usize| {
        ys.get(k)
            .map(|&t| bytes_of(&str_of(v, t)))
            .unwrap_or_default()
    };
    let int = |k: usize| ys.get(k).map_or(0, |&n| int_of(v, n));
    Some(match kind.as_str() {
        "lit" => Whatsit::Literal(
            match int(1) {
                2 => LiteralMode::DirectAlways,
                1 => LiteralMode::DirectPage,
                _ => LiteralMode::SetOrigin,
            },
            text(2),
        ),
        "cs" => Whatsit::ColorStack(
            usize::try_from(int(1)).unwrap_or(usize::MAX),
            match str_of(v, ys.get(2).copied()?).as_str() {
                "set" => ColorStackCmd::Set(text(3)),
                "push" => ColorStackCmd::Push(text(3)),
                "pop" => ColorStackCmd::Pop,
                _ => ColorStackCmd::Current,
            },
        ),
        "mx" => Whatsit::SetMatrix(text(1)),
        "save" => Whatsit::Save,
        "iwon" => Whatsit::InterwordSpaceOn,
        "iwoff" => Whatsit::InterwordSpaceOff,
        "fake" => Whatsit::FakeSpace,
        "rloff" => Whatsit::RunningLinkOff,
        "rlon" => Whatsit::RunningLinkOn,
        "restore" => Whatsit::Restore,
        _ => return None,
    })
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
    let map = match file_of(cx, "pdftex.map")? {
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
    let items = cx_maps(cx)?;
    let map = apply_map_items(cx, map, items)?;
    // the fonts the pages use, and those the font commands name
    let mut wanted: Vec<(u32, String, ValueId, i32)> = Vec::new();
    for &page in pages {
        let xs = list_of(v, page);
        for fx in xs.get(2).map(|&l| list_of(v, l)).unwrap_or_default() {
            let ys = list_of(v, fx);
            let id = u32::try_from(ys.first().map_or(0, |&y| int_of(v, y))).unwrap_or(0);
            let name = ys.get(1).map(|&y| str_of(v, y)).unwrap_or_default();
            let s = i32::try_from(ys.get(3).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
            if let Some(&file) = ys.get(2) {
                wanted.push((id, name, file, s));
            }
        }
    }
    // `pdf_read_dummy_font`: `pdftexspace` at its design size, for
    // `\pdfinterwordspaceon` and `\pdffakespace`
    // PDF-BACKEND-GAP: `\pdfspacefont` naming another font
    let spaces = pages.iter().any(|&p| {
        list_of(v, p).get(4).is_some_and(|&d| {
            list_of(v, d).iter().any(|&e| {
                list_of(v, e).get(1).is_some_and(|&x| {
                    matches!(
                        list_of(v, x).first().map(|&k| str_of(v, k)).as_deref(),
                        Some("iwon" | "fake")
                    )
                })
            })
        })
    });
    if spaces && let Got::Text(_) = file_of(cx, "pdftexspace.tfm")? {
        let x = cx.use_env(v.field("file:pdftexspace.tfm"))?;
        if let Some(x) = x {
            // REDUCE-GAP: pdfTeX loads the space font at the shipout that
            // first needs it, as the next internal font number; here the
            // number after the job's last font (the engine's `fontcount`
            // does not count it)
            let n = cx
                .use_field(v.field("fontcount"))?
                .map_or(0, |c| int_of(v, c));
            let id = u32::try_from(n + 1).unwrap_or(SPACE_FONT);
            wanted.push((id, "pdftexspace".to_owned(), x, -1000));
            fonts.1 = Some(id);
        }
    }
    // the font commands as read: `\pdffontattr`, `\pdfincludechars`,
    // `\pdfnobuiltintounicode`, `\pdffontobjnum`/`\pdffontname`,
    // `\pdfglyphtounicode`
    // PDF-BACKEND-GAP: given to the writer before the first page, in
    // their order, not between the pages (the writer as a fold over the
    // document)
    let cmds_x = cx.acc_range(v.field("pdfcmds"), None)?;
    let mut cmds: Vec<(String, u32, Vec<u8>, Vec<u8>)> = Vec::new();
    for e in v.seq_to_vec(cmds_x) {
        let ys = list_of(v, e);
        let kind = ys.first().map(|&k| str_of(v, k)).unwrap_or_default();
        let t = |k: usize| {
            ys.get(k)
                .map(|&y| bytes_of(&str_of(v, y)))
                .unwrap_or_default()
        };
        if kind == "glyphtounicode" {
            cmds.push((kind, 0, t(1), t(2)));
            continue;
        }
        if kind == "map" {
            continue;
        }
        let f = ys.get(1).map_or(0, |&y| int_of(v, y));
        let id = u32::try_from(f).unwrap_or(0);
        if let Some(i) = crate::fonts::info(v, cx.use_field(v.field(&format!("fontinfo:{f}")))?)
            && let Some(file) = i.file
        {
            wanted.push((id, i.name, file, i.s));
        }
        cmds.push((kind, id, t(2), Vec::new()));
    }
    for (id, name, file, s) in wanted {
        if fonts.0.contains_key(&id) {
            continue;
        }
        let Some(l) = crate::fonts::load(v, file, s) else {
            continue;
        };
        let entry = if Some(id) == fonts.1 {
            // `mapfile.c`: the line pdfTeX adds for its space font
            parse_line(b"pdftexspace PdfTeX-Space <pdftexspace.pfb")
        } else {
            map.as_ref().and_then(|m| m.get(name.as_bytes()).cloned())
        };
        let mut get = |n: Option<&[u8]>| -> Result<Option<Vec<u8>>, Stop> {
            let Some(n) = n else { return Ok(None) };
            let n = String::from_utf8_lossy(n).into_owned();
            Ok(match file_of(cx, &n)? {
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
    // `\pdfcolorstackinit` (when read: before the pages that use them)
    let last = pages.last().map(|&p| list_of(v, p)).unwrap_or_default();
    let last = last.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
    for d in last.get(12).map(|&l| list_of(v, l)).unwrap_or_default() {
        let xs = list_of(v, d);
        let page = xs.first().is_some_and(|&y| int_of(v, y) != 0);
        let direct = xs.get(1).is_some_and(|&y| int_of(v, y) == 2);
        let init = xs
            .get(2)
            .map(|&y| bytes_of(&str_of(v, y)))
            .unwrap_or_default();
        let _ = w.colorstack_init(page, direct, &init);
    }
    for (kind, id, t, u) in &cmds {
        match kind.as_str() {
            "glyphtounicode" => w.glyph_to_unicode(t, u),
            "fontattr" => w.font_attr(*id, t, &fonts),
            "includechars" => {
                let _ = w.include_chars(*id, t, &fonts);
            }
            "fontobjnum" => {
                let _ = w.font_objnum(*id, &fonts);
            }
            _ => w.no_builtin_tounicode(*id, &fonts),
        }
    }
    for &page in pages {
        let xs = list_of(v, page);
        let mut side = Side::default();
        let Some(Node::Box(bx)) = xs.first().and_then(|&b| nodes::decode(v, b, &mut side)) else {
            continue;
        };
        let p = xs.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
        page_params(v, w.params_mut(), &p);
        let mut resolved: HashMap<u32, ValueId> = HashMap::new();
        for e in xs.get(4).map(|&l| list_of(v, l)).unwrap_or_default() {
            let ys = list_of(v, e);
            if let (Some(&h), Some(&d)) = (ys.first(), ys.get(1)) {
                resolved.insert(u32::try_from(int_of(v, h)).unwrap_or(0), d);
            }
        }
        let whats = side.whats.clone();
        let mut wh = |h: u32, _leaders: bool| -> Option<Whatsit> {
            if let Some(&d) = resolved.get(&h) {
                return whatsit(v, d);
            }
            let w = *whats.get(h as usize)?;
            let ys = list_of(v, w);
            if ys.first().map(|&k| str_of(v, k)).as_deref() == Some("special") {
                Some(Whatsit::Special(bytes_of(
                    &ys.get(1).map(|&s| str_of(v, s)).unwrap_or_default(),
                )))
            } else {
                None
            }
        };
        if let Err(e) = w.ship_out_with(&bx, &fonts, &mut wh) {
            // PDF-BACKEND-GAP: the writer's errors are pdfTeX's fatal
            // errors; they reach no log yet
            if std::env::var_os("PHITEX_TEX_TIME").is_some() {
                eprintln!("pdf: {e}");
            }
        }
    }
    for (f, set) in [
        ("pdf:info", PdfWriter::info as fn(&mut PdfWriter, &[u8])),
        ("pdf:names", PdfWriter::names),
        ("pdf:trailer", PdfWriter::trailer),
        ("pdf:trailerid", PdfWriter::trailer_id),
    ] {
        let x = cx.use_field(v.field(f))?;
        let t = if f == "pdf:trailerid" {
            x.map(|x| bytes_of(&str_of(v, x))).unwrap_or_default()
        } else {
            joined(v, x)
        };
        if !t.is_empty() {
            set(&mut w, &t);
        }
    }
    // PDF-BACKEND-GAP: `\pdfcatalog ... openaction` (the action is
    // written when it is read)
    let catalog = joined(v, cx.use_field(v.field("pdf:catalog"))?);
    if !catalog.is_empty() {
        let _ = w.catalog(&catalog, None);
    }
    let int = |cx: &mut Ctx<'_>, f: &str| -> Result<i32, Stop> {
        Ok(cx
            .use_field(v.field(f))?
            .map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0)))
    };
    let omit = int(cx, "param:pdfinfoomitdate")?;
    let suppress = int(cx, "param:pdfsuppressptexinfo")?;
    w.params_mut().info_omit_date = omit;
    w.params_mut().suppress_ptex_info = suppress;
    let bytes = w.finish().unwrap_or_else(|e| {
        if std::env::var_os("PHITEX_TEX_TIME").is_some() {
            eprintln!("pdf: {e}");
        }
        Vec::new()
    });
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}

/// `\pdfmapfile` and `\pdfmapline` items, in order.
fn cx_maps(cx: &mut Ctx<'_>) -> Result<Vec<(bool, String)>, Stop> {
    let v = cx.values();
    let items = cx.use_field(v.field("pdf:maps"))?;
    Ok(items
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            let ys = list_of(v, e);
            let file = ys.first().map(|&k| str_of(v, k)).as_deref() == Some("file");
            (file, ys.get(1).map(|&t| str_of(v, t)).unwrap_or_default())
        })
        .collect())
}

/// The map after the document's map items (`pdfmapfile`,
/// `pdfmapline`, `mapfile.c`): `+` adds what is not there, `=` replaces,
/// `-` removes; without a prefix the item replaces the default map.
// PDF-BACKEND-GAP: pdfTeX applies an item when it is shipped out, before
// the fonts of later pages; here all of them before the first page.
fn apply_map_items(
    cx: &mut Ctx<'_>,
    map: Option<Arc<Map>>,
    items: Vec<(bool, String)>,
) -> Result<Option<Arc<Map>>, Stop> {
    if items.is_empty() {
        return Ok(map);
    }
    let v = cx.values();
    let mut m: BTreeMap<Vec<u8>, MapEntry> = map.map(|m| (*m).clone()).unwrap_or_default();
    for (is_file, text) in items {
        let t = text.trim_start();
        let (mode, rest) = match t.chars().next() {
            Some(c @ ('+' | '=' | '-')) => (c, &t[1..]),
            _ => (' ', t),
        };
        let entries: Vec<MapEntry> = if is_file {
            match file_of(cx, rest.trim())? {
                Got::Text(b) => parse_map(&bytes_of(&str_of(v, b))).into_values().collect(),
                Got::Absent => Vec::new(),
            }
        } else {
            parse_line(&bytes_of(rest)).into_iter().collect()
        };
        if mode == ' ' {
            m.clear();
        }
        for e in entries {
            match mode {
                '+' => {
                    m.entry(e.tfm_name.clone()).or_insert(e);
                }
                '-' => {
                    m.remove(&e.tfm_name);
                }
                _ => {
                    m.insert(e.tfm_name.clone(), e);
                }
            }
        }
    }
    Ok(Some(Arc::new(m)))
}
