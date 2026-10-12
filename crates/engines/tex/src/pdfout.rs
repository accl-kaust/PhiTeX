//! The PDF file as a fold over the document (pdfTeX §750–§815): the
//! writer's state (`phitex_pdf::PdfState`) after each step that touches
//! it, a step being a PDF command when TeX reads it or a `\shipout`.
//!
//! The field `pdf:state` holds a key: the hash of the key before and of
//! the step's input, so the same history gives the same key whatever ran
//! before. The key names a memo entry, the state after the step and the
//! file's bytes so far (`PdfBytes`, append-only: chunks shared between
//! entries). What TeX reads back (`\pdfcolorstackinit`'s number,
//! `\pdffontobjnum`, ...) comes from the state the step made. At the end
//! the `Dvi` node finishes the state that reaches it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Ctx, Stop, Value, ValueId, Values};
use phitex_pdf::map::{MapEntry, parse_line, parse_map};
use phitex_pdf::{ColorStackCmd, FontSource, LiteralMode, PdfBytes, PdfParams, PdfState, Whatsit};
use phitex_tfm::ScaledFont;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::BoxNode;

use crate::fonts::bytes_of;
use crate::nodes::Side;
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};

/// `pdftex_banner` of the oracle's pdfTeX.
pub(crate) const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

/// A font map by TFM name.
type Map = BTreeMap<Vec<u8>, MapEntry>;

type MapCache = Mutex<HashMap<u64, Arc<Map>>>;

/// Parsed map files: a pure function's memo by the file's value.
fn maps() -> &'static MapCache {
    static C: OnceLock<MapCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

type States = Mutex<HashMap<u64, Arc<(PdfState, PdfBytes)>>>;

/// The writer's states by key: a pure function's memo (the key is the
/// hash of the inputs that made the state).
fn states() -> &'static States {
    static S: OnceLock<States> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One font as the writer asks for it.
struct Font {
    name: Vec<u8>,
    metrics: ScaledFont,
    entry: Option<MapEntry>,
    ff: Option<Arc<[u8]>>,
    enc: Option<Arc<[u8]>>,
}

/// The fonts a step needs, the files map lines name, and the space
/// font's number if loaded.
#[derive(Default)]
pub(crate) struct Fonts {
    by_id: HashMap<FontId, Font>,
    files: HashMap<Vec<u8>, Arc<[u8]>>,
    space: Option<FontId>,
}

impl FontSource for Fonts {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        static EMPTY: OnceLock<ScaledFont> = OnceLock::new();
        self.by_id.get(&f).map_or_else(
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
        self.by_id.get(&f).map_or(&[], |x| &x.name)
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.by_id.get(&f)?.entry.clone()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.by_id.get(&f)?.ff.as_ref().map(|b| b.to_vec())
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        self.by_id.get(&f)?.enc.as_ref().map(|b| b.to_vec())
    }
    fn file(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.files.get(name).map(|b| b.to_vec())
    }
    fn space_font(&self) -> Option<FontId> {
        self.space
    }
}

/// A file's bytes, `None` if absent.
fn file_bytes(r: &mut Run<'_, '_>, name: &str) -> R<Option<Arc<[u8]>>> {
    let v = r.v;
    Ok(r.file(name)?.and_then(|x| {
        list_of(v, x)
            .get(1)
            .map(|&t| Arc::from(bytes_of(&str_of(v, t))))
    }))
}

/// `pdftex.map`, parsed.
fn default_map(r: &mut Run<'_, '_>) -> R<Option<Arc<Map>>> {
    let v = r.v;
    let Some(x) = r.file("pdftex.map")? else {
        return Ok(None);
    };
    let Some(&m) = list_of(v, x).get(1) else {
        return Ok(None);
    };
    let key = m.raw();
    if let Some(c) = maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Ok(Some(c.clone()));
    }
    let parsed = Arc::new(parse_map(&bytes_of(&str_of(v, m))));
    maps()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, parsed.clone());
    Ok(Some(parsed))
}

/// The fonts `ids` as the writer asks for them, with the files of the
/// map lines read so far; `space`: with `pdftexspace` too.
pub(crate) fn fonts_for(r: &mut Run<'_, '_>, ids: &[u32], space: bool) -> R<Fonts> {
    let v = r.v;
    let map = default_map(r)?;
    let mut out = Fonts::default();
    let mut wanted: Vec<(u32, String, ValueId, i32)> = Vec::new();
    for &f in ids {
        if let Some(i) = crate::fonts::info(v, r.get(&format!("fontinfo:{f}"))?)
            && let Some(file) = i.file
        {
            wanted.push((f, i.name, file, i.s));
        }
    }
    if space && let Some(x) = r.file("pdftexspace.tfm")? {
        // REDUCE-GAP: pdfTeX loads the space font at the shipout that
        // first needs it, as the next internal font number; here the
        // number after the fonts so far (the engine's `fontcount` does
        // not count it)
        let n = r.int("fontcount")?;
        let id = u32::try_from(n + 1).unwrap_or(u32::MAX - 1);
        wanted.push((id, "pdftexspace".to_owned(), x, -1000));
        out.space = Some(id);
    }
    for (id, name, file, s) in wanted {
        let Some(l) = crate::fonts::load(v, file, s) else {
            continue;
        };
        let entry = if Some(id) == out.space {
            // `mapfile.c`: the line pdfTeX adds for its space font
            parse_line(b"pdftexspace PdfTeX-Space <pdftexspace.pfb")
        } else {
            map.as_ref().and_then(|m| m.get(name.as_bytes()).cloned())
        };
        let mut get = |n: Option<&[u8]>| -> R<Option<Arc<[u8]>>> {
            match n {
                Some(n) => file_bytes(r, &String::from_utf8_lossy(n)),
                None => Ok(None),
            }
        };
        let ff = get(entry.as_ref().and_then(|e| e.ff_name.as_deref()))?;
        let enc = get(entry.as_ref().and_then(|e| e.encname.as_deref()))?;
        out.by_id.insert(
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
    // the files of `\pdfmapline`s and `\pdfmapfile`s
    for e in r
        .get("pdf:maps")?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
    {
        let ys = list_of(v, e);
        let file = ys.first().map(|&k| str_of(v, k)).as_deref() == Some("file");
        let text = ys.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
        let t = text.trim_start_matches(['+', '=', '-', ' ']);
        let mut lines: Vec<MapEntry> = Vec::new();
        if file {
            let name = t.split(' ').next().unwrap_or_default().to_owned();
            if let Some(b) = file_bytes(r, &name)? {
                out.files.insert(name.into_bytes(), b.clone());
                lines.extend(parse_map(&b).into_values());
            }
        } else {
            lines.extend(parse_line(&bytes_of(t)));
        }
        for m in lines {
            for n in [m.ff_name.clone(), m.encname.clone()].into_iter().flatten() {
                if !out.files.contains_key(&n)
                    && let Some(b) = file_bytes(r, &String::from_utf8_lossy(&n))?
                {
                    out.files.insert(n, b);
                }
            }
        }
    }
    Ok(out)
}

/// A step of the fold: the state after the previous step (a fresh one
/// at the start), `f` applied, the bytes it wrote flushed; `input` is
/// everything the step depends on.
pub(crate) fn step<T>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &Fonts,
    f: impl FnOnce(&mut PdfState, &Fonts) -> T,
) -> R<T> {
    let v = r.v;
    let k0 = u64::from_ne_bytes(r.int("pdf:state")?.to_ne_bytes());
    let before = states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k0)
        .cloned();
    let before_known = before.is_some();
    let (mut st, mut bytes) = if let Some(a) = before {
        (a.0.clone(), a.1.clone())
    } else {
        {
            let date = r
                .get("pdfdate")?
                .map_or_else(|| phitex_pdf::pdf_date(0), |x| str_of(v, x));
            let jobname = r.get("jobname")?.map(|x| str_of(v, x)).unwrap_or_default();
            let p = PdfParams {
                date,
                banner: BANNER.to_owned(),
                output_file_name: format!("{jobname}.pdf"),
                ..PdfParams::default()
            };
            (PdfState::new(p), PdfBytes::new())
        }
    };
    let out = f(&mut st, fonts);
    st.flush(&mut bytes);
    // a fresh state's key names what it was made from
    let seed = if before_known {
        k0
    } else {
        let date = r.get("pdfdate")?.map_or(0, ValueId::raw);
        let job = r.get("jobname")?.map_or(0, ValueId::raw);
        phitex_defuse::hash_words(k0, &[date, job])
    };
    let k1 = phitex_defuse::hash_words(seed, &[input.raw()]).max(1);
    states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(k1)
        .or_insert_with(|| Arc::new((st, bytes)));
    r.set(
        "pdf:state",
        v.int(i64::from_ne_bytes(k1.to_ne_bytes())),
        true,
    );
    Ok(out)
}

/// A read-time command on the writer (`pdf:state` fold step) in PDF
/// mode, with the fonts it names.
pub(crate) fn command<T: Default>(
    r: &mut Run<'_, '_>,
    input: ValueId,
    fonts: &[u32],
    f: impl FnOnce(&mut PdfState, &Fonts) -> T,
) -> R<T> {
    if r.param("pdfoutput")? <= 0 {
        return Ok(T::default());
    }
    let fs = fonts_for(r, fonts, false)?;
    step(r, input, &fs, f)
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

/// `pdf_ship_out` as a step: `page` is the `pages` entry (box, counts,
/// fonts, parameters, whatsit data).
pub(crate) fn ship(
    r: &mut Run<'_, '_>,
    page: ValueId,
    bx: &BoxNode,
    side: &Side,
    ids: &[u32],
) -> R<()> {
    let v = r.v;
    let xs = list_of(v, page);
    let p = xs.get(3).map(|&l| list_of(v, l)).unwrap_or_default();
    let mut resolved: HashMap<u32, ValueId> = HashMap::new();
    for e in xs.get(4).map(|&l| list_of(v, l)).unwrap_or_default() {
        let ys = list_of(v, e);
        if let (Some(&h), Some(&d)) = (ys.first(), ys.get(1)) {
            resolved.insert(u32::try_from(int_of(v, h)).unwrap_or(0), d);
        }
    }
    let space = resolved.values().any(|&d| {
        matches!(
            list_of(v, d).first().map(|&k| str_of(v, k)).as_deref(),
            Some("iwon" | "fake")
        )
    });
    let fonts = fonts_for(r, ids, space)?;
    let whats = side.whats.clone();
    let err = step(r, page, &fonts, |st, fonts| {
        page_params(v, st.params_mut(), &p);
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
        st.ship_out_with(bx, fonts, &mut wh).err()
    })?;
    if let Some(e) = err {
        // PDF-BACKEND-GAP: the writer's errors are pdfTeX's fatal errors;
        // they go to the log, the job going on
        r.log("error", &format!("pdfTeX error: {e}"))?;
    }
    Ok(())
}

/// The PDF file: the state that reaches the end, finished.
pub(crate) fn finish(cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
    let v = cx.values();
    let k = cx
        .use_field(v.field("pdf:state"))?
        .map_or(0, |x| u64::from_ne_bytes(int_of(v, x).to_ne_bytes()));
    let Some(a) = states()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&k)
        .cloned()
    else {
        return Ok(v.str(""));
    };
    let mut st = a.0.clone();
    let int = |cx: &mut Ctx<'_>, f: &str| -> Result<i32, Stop> {
        Ok(cx
            .use_field(v.field(f))?
            .map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0)))
    };
    st.params_mut().info_omit_date = int(cx, "param:pdfinfoomitdate")?;
    st.params_mut().suppress_ptex_info = int(cx, "param:pdfsuppressptexinfo")?;
    let bytes = phitex_pdf::finish(st, &a.1).unwrap_or_else(|e| {
        if std::env::var_os("PHITEX_TEX_TIME").is_some() {
            eprintln!("pdf: {e}");
        }
        Vec::new()
    });
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    Ok(v.intern(Value::Str(s.into_boxed_str())))
}
