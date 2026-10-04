//! Display lists' checks (DESIGN 4.6, `PARTEX_DISPLAY=1`): a run's
//! `<job>.display.jsonl` against its PDF, as a reader of the PDF sees it.
//!
//! - Each page's glyphs (`glyphs`, forms walked where drawn: glyph
//!   origins' order) are the codes `partex_engine::pdftext::page_codes`
//!   finds in the PDF's page, one for one: each one placed shows the same
//!   code at the same point, bit for bit, in the font the page's
//!   resources name (`/BaseFont`, its subset tag dropped); a `null` stands
//!   for a code the PDF shows that no glyph of TeX's made.
//! - Each page's and form's items are what `pdftext::list` makes of the
//!   PDF's own content stream (its length the side file's), with the
//!   PDF's resources (each font's `/Widths`, each `XObject`'s object) and
//!   the side file's literals: the same items, bit for bit.
//! - Each page's media box and `/Rotate`, and each form's `/BBox`, are the
//!   PDF's.
//! - With `<job>.origins.jsonl` there, each page has as many glyphs as
//!   origins.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use partex_engine::pdfread::{Dict, Doc, Obj, Ref};
use partex_engine::pdftext::{self, DocText, Font, FontId, Item, ListRes, Text, XKind};
use serde_json::Value;

/// A stream's part of a side file.
struct Stream {
    length: usize,
    literals: Vec<(u32, u32, u8)>,
    media: Vec<f64>,
    rotate: i64,
    items: Vec<String>,
    glyphs: Vec<Option<(usize, u8, f64, f64)>>,
}

/// A side file: its pages, forms (by object number) and fonts (name and
/// size, by the file's number).
struct Side {
    pages: Vec<Stream>,
    forms: BTreeMap<u32, Stream>,
    fonts: Vec<(String, f64)>,
}

fn f(v: &Value) -> Result<f64> {
    v.as_f64().with_context(|| format!("a number: {v}"))
}

fn nums(v: &Value) -> Result<Vec<f64>> {
    v.as_array()
        .context("an array of numbers")?
        .iter()
        .map(f)
        .collect()
}

/// An item of the side file, as text (fonts by name and size).
fn side_item(v: &Value, fonts: &[(String, f64)]) -> Result<String> {
    let a = v.as_array().context("an item")?;
    let tag = a
        .first()
        .and_then(Value::as_str)
        .context("an item's kind")?;
    let mut s = String::new();
    match tag {
        "g" => {
            let k = usize::try_from(a[1].as_u64().context("a font")?)?;
            let (name, size) = fonts.get(k).cloned().unwrap_or_default();
            write!(
                s,
                "g {name} {size:?} {} {:?} {:?}",
                a[2],
                f(&a[3])?,
                f(&a[4])?
            )?;
        }
        "m" => write!(s, "m {:?}", nums(&a[1])?)?,
        "r" => write!(
            s,
            "r {:?} {:?} {:?} {:?} {} {:?}",
            f(&a[1])?,
            f(&a[2])?,
            f(&a[3])?,
            f(&a[4])?,
            a[5],
            nums(&a[6])?
        )?,
        "l" => {
            let text: Vec<u8> = a[4]
                .as_str()
                .context("a literal's text")?
                .chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                .collect();
            write!(
                s,
                "l {} {} {:?} {:?}",
                a[1],
                a[2],
                nums(&a[3])?,
                String::from_utf8_lossy(&text)
            )?;
        }
        "x" => write!(s, "x {} {} {:?}", a[1], a[2], nums(&a[3])?)?,
        _ => return Err(anyhow!("an item `{tag}`")),
    }
    Ok(s)
}

/// An item made from the PDF, as text.
fn file_item(it: &Item, fonts: &[(String, f64)]) -> String {
    let mode = |m: pdftext::LiteralMode| match m {
        pdftext::LiteralMode::Origin => "\"origin\"",
        pdftext::LiteralMode::Page => "\"page\"",
        pdftext::LiteralMode::Direct => "\"direct\"",
    };
    match it {
        Item::Glyph { font, code, x, y } => {
            let (name, size) = usize::try_from(font.0)
                .ok()
                .and_then(|k| fonts.get(k).cloned())
                .unwrap_or_default();
            format!("g {name} {size:?} {code} {x:?} {y:?}")
        }
        Item::GlyphMatrix(m) => format!("m {:?}", m.to_vec()),
        Item::Rule {
            x,
            y,
            w,
            h,
            stroke,
            ctm,
        } => format!(
            "r {x:?} {y:?} {w:?} {h:?} {} {:?}",
            u8::from(*stroke),
            ctm.to_vec()
        ),
        Item::Literal {
            bytes,
            mode: m,
            ctm,
            codes,
        } => format!(
            "l {} {codes} {:?} {:?}",
            mode(*m),
            ctm.to_vec(),
            String::from_utf8_lossy(bytes)
        ),
        Item::XObject { kind, id, matrix } => format!(
            "x {} {id} {:?}",
            match kind {
                XKind::Form => "\"form\"",
                XKind::Image => "\"image\"",
            },
            matrix.to_vec()
        ),
    }
}

/// A stream's line of the side file.
fn stream(v: &Value, fonts: &[(String, f64)]) -> Result<Stream> {
    let literals = v["literals"]
        .as_array()
        .context("literals")?
        .iter()
        .map(|l| -> Result<(u32, u32, u8)> {
            let n = |i: usize| l[i].as_u64().context("a literal's place");
            Ok((
                u32::try_from(n(0)?)?,
                u32::try_from(n(1)?)?,
                u8::try_from(n(2)?)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let items = v["items"]
        .as_array()
        .context("items")?
        .iter()
        .map(|i| side_item(i, fonts))
        .collect::<Result<Vec<_>>>()?;
    let glyphs = match v.get("glyphs").and_then(Value::as_array) {
        Some(g) => g
            .iter()
            .map(|g| -> Result<Option<(usize, u8, f64, f64)>> {
                if g.is_null() {
                    return Ok(None);
                }
                Ok(Some((
                    usize::try_from(g[0].as_u64().context("a font")?)?,
                    u8::try_from(g[1].as_u64().context("a code")?)?,
                    f(&g[2])?,
                    f(&g[3])?,
                )))
            })
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    Ok(Stream {
        length: usize::try_from(v["length"].as_u64().context("a length")?)?,
        literals,
        media: nums(&v["media"])?,
        rotate: v["rotate"].as_i64().unwrap_or(0),
        items,
        glyphs,
    })
}

/// Read side file `path`.
fn read(path: &Path) -> Result<Side> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<Value> = text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    // (the fonts first: the items name them)
    let mut fonts = Vec::new();
    for l in &lines {
        if let Some(k) = l.get("font").and_then(Value::as_u64) {
            let k = usize::try_from(k)?;
            if fonts.len() <= k {
                fonts.resize(k + 1, (String::new(), 0.0));
            }
            fonts[k] = (
                l["name"].as_str().unwrap_or_default().to_owned(),
                l["size"].as_f64().unwrap_or(0.0),
            );
        }
    }
    let mut side = Side {
        pages: Vec::new(),
        forms: BTreeMap::new(),
        fonts,
    };
    for l in &lines {
        if l.get("hash").is_some() {
            side.pages.push(stream(l, &side.fonts)?);
        } else if let Some(id) = l.get("form").and_then(Value::as_u64) {
            side.forms
                .insert(u32::try_from(id)?, stream(l, &side.fonts)?);
        }
    }
    Ok(side)
}

/// A PDF stream's resources, as `pdftext::list` asks for them: the
/// fonts' by `DocText`, ids by `/BaseFont` (subset tag dropped) and size.
struct FileRes<'a> {
    doc: &'a Doc,
    resources: Option<Dict>,
    fonts: &'a mut Vec<(String, f64)>,
}

/// A font's `/BaseFont`, its subset tag dropped.
fn base_font(doc: &Doc, resources: Option<&Dict>, name: &[u8]) -> Option<String> {
    let fonts = doc.lookup(resources?, b"Font");
    let font = doc.lookup(fonts.as_dict()?, name);
    let base = doc.lookup(font.as_dict()?, b"BaseFont");
    let base = String::from_utf8_lossy(base.as_name()?).into_owned();
    Some(match base.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 => rest.to_owned(),
        _ => base,
    })
}

impl ListRes for FileRes<'_> {
    fn font(&mut self, name: &[u8]) -> Font {
        let mut out = Vec::new();
        let mut t = DocText {
            doc: self.doc,
            resources: self.resources.clone(),
            depth: 0,
            out: &mut out,
        };
        Text::font(&mut t, name)
    }

    fn font_id(&mut self, name: &[u8], size: f64) -> FontId {
        let Some(base) = base_font(self.doc, self.resources.as_ref(), name) else {
            return FontId::UNKNOWN;
        };
        let k = self
            .fonts
            .iter()
            .position(|(n, s)| *n == base && s.to_bits() == size.to_bits())
            .unwrap_or_else(|| {
                self.fonts.push((base, size));
                self.fonts.len() - 1
            });
        FontId(u32::try_from(k).unwrap_or(u32::MAX))
    }

    fn xobject(&mut self, name: &[u8]) -> Option<(XKind, u32)> {
        let xs = self.doc.lookup(self.resources.as_ref()?, b"XObject");
        let Some(Obj::Ref(r)) = xs.as_dict()?.get(name) else {
            return None;
        };
        let kind = if name.starts_with(b"Fm") {
            XKind::Form
        } else {
            XKind::Image
        };
        Some((kind, u32::try_from(r.num).ok()?))
    }
}

/// Every font resource name the page and its forms name, to its
/// `/BaseFont` (pdfTeX's `/F<n>` names one font everywhere).
fn font_names(
    doc: &Doc,
    resources: Option<&Dict>,
    depth: usize,
    out: &mut BTreeMap<Vec<u8>, String>,
) {
    let Some(r) = resources else {
        return;
    };
    if let Some(fs) = doc.lookup(r, b"Font").as_dict() {
        for (n, _) in &fs.0 {
            if let Some(b) = base_font(doc, Some(r), n) {
                out.entry(n.clone()).or_insert(b);
            }
        }
    }
    if depth < 8
        && let Some(xs) = doc.lookup(r, b"XObject").as_dict()
    {
        for (n, _) in &xs.0 {
            if let Obj::Stream(s) = doc.lookup(xs, n)
                && let Obj::Dict(d) = doc.lookup(&s.dict, b"Resources")
            {
                font_names(doc, Some(&d), depth + 1, out);
            }
        }
    }
}

/// The items `pdftext::list` makes of stream `content`, with `literals`
/// and these resources, as text (beside the side file's).
fn items_of(
    doc: &Doc,
    content: &[u8],
    side: &Stream,
    resources: Option<Dict>,
    fonts: &mut Vec<(String, f64)>,
) -> Vec<String> {
    let mut res = FileRes {
        doc,
        resources,
        fonts,
    };
    let items = pdftext::list(content, &side.literals, &mut res);
    items.iter().map(|i| file_item(i, res.fonts)).collect()
}

/// Compare two item lists, the first difference.
fn diff_items(what: &str, side: &[String], file: &[String], bad: &mut Vec<String>) {
    if side == file {
        return;
    }
    let at = side.iter().zip(file).take_while(|(a, b)| a == b).count();
    bad.push(format!(
        "{what}: items differ at {at} ({} against {} in the PDF): {:?} / {:?}",
        side.len(),
        file.len(),
        side.get(at),
        file.get(at)
    ));
}

/// Check `dir/<job>.display.jsonl` against `dir/<job>.pdf`: the problems
/// found, and what was checked.
pub fn check(dir: &Path, job: &str) -> Result<(Vec<String>, String)> {
    let side = read(&dir.join(format!("{job}.display.jsonl")))?;
    let path = dir.join(format!("{job}.pdf"));
    let data: Arc<[u8]> = fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .into();
    let doc = Doc::open(&data).map_err(|e| anyhow!("{}: {e:?}", path.display()))?;
    let mut bad = Vec::new();
    if side.pages.len() != doc.num_pages() {
        bad.push(format!(
            "{} pages listed, {} in the PDF",
            side.pages.len(),
            doc.num_pages()
        ));
    }
    let origins = crate::origins::read(&dir.join(format!("{job}.origins.jsonl"))).ok();
    let mut file_fonts = Vec::new();
    let mut placed = 0;
    for (n, s) in side.pages.iter().enumerate() {
        let counted = origins.as_ref().and_then(|o| o.pages.get(n)).map(Vec::len);
        placed += check_page(&doc, &side, n, s, counted, &mut file_fonts, &mut bad);
    }
    for (id, s) in &side.forms {
        check_form(&doc, *id, s, &mut file_fonts, &mut bad)?;
    }
    if placed < 10 {
        bad.push(format!("only {placed} glyphs placed"));
    }
    let items: usize = side
        .pages
        .iter()
        .chain(side.forms.values())
        .map(|s| s.items.len())
        .sum();
    let counts = format!(
        "{} pages, {} forms, {items} items, {placed} glyphs placed, {} fonts",
        side.pages.len(),
        side.forms.len(),
        side.fonts.len()
    );
    Ok((bad, counts))
}

/// Check page `n`'s part `s` of the side file against the PDF (with
/// `counted` glyphs in origins'): the glyphs placed.
fn check_page(
    doc: &Doc,
    side: &Side,
    n: usize,
    s: &Stream,
    counted: Option<usize>,
    file_fonts: &mut Vec<(String, f64)>,
    bad: &mut Vec<String>,
) -> usize {
    let what = format!("page {}", n + 1);
    let Some(page) = doc.page(n + 1) else {
        return 0;
    };
    if s.media.as_slice() != page.media.as_slice() || s.rotate != i64::from(page.rotate) {
        bad.push(format!(
            "{what}: media {:?} turned {}, the PDF's {:?} turned {}",
            s.media, s.rotate, page.media, page.rotate
        ));
    }
    // (the page's content: its one stream, and the walk's newline)
    let content = pdftext::page_content(doc, &page);
    if content.len() != s.length + 1 {
        bad.push(format!(
            "{what}: a stream of {} bytes, the PDF's {}",
            s.length,
            content.len().saturating_sub(1)
        ));
        return 0;
    }
    let file = items_of(doc, &content, s, page.resources.clone(), file_fonts);
    diff_items(&what, &s.items, &file, bad);
    // (the glyphs, against the walk's codes)
    let shown = pdftext::page_codes(doc, &page);
    if shown.len() != s.glyphs.len() {
        bad.push(format!(
            "{what}: {} glyphs, {} codes shown",
            s.glyphs.len(),
            shown.len()
        ));
        return 0;
    }
    if counted.is_some_and(|c| c != s.glyphs.len()) {
        bad.push(format!(
            "{what}: {} glyphs, {counted:?} in origins",
            s.glyphs.len()
        ));
    }
    let mut names = BTreeMap::new();
    font_names(doc, page.resources.as_ref(), 0, &mut names);
    let mut placed = 0;
    for (k, (g, c)) in s.glyphs.iter().zip(&shown).enumerate() {
        let Some((font, code, x, y)) = *g else {
            continue;
        };
        placed += 1;
        let name = side.fonts.get(font).map(|f| f.0.as_str());
        let same = u32::from(code) == c.code
            && x.to_bits() == c.x.to_bits()
            && y.to_bits() == c.y.to_bits()
            && name == names.get(&c.font).map(String::as_str);
        if !same {
            bad.push(format!(
                "{what}, glyph {k}: {:?} in {name:?}, the PDF's {:?} in {:?}",
                (code, x, y),
                (c.code, c.x, c.y),
                names.get(&c.font)
            ));
        }
    }
    placed
}

/// Check form `id`'s part `s` of the side file against the PDF.
fn check_form(
    doc: &Doc,
    id: u32,
    s: &Stream,
    file_fonts: &mut Vec<(String, f64)>,
    bad: &mut Vec<String>,
) -> Result<()> {
    let what = format!("form {id}");
    let Obj::Stream(st) = doc.fetch(Ref {
        num: i32::try_from(id)?,
        generation: 0,
    }) else {
        bad.push(format!("{what}: not a stream in the PDF"));
        return Ok(());
    };
    let content = doc.decode(&st);
    if content.len() != s.length {
        bad.push(format!(
            "{what}: a stream of {} bytes, the PDF's {}",
            s.length,
            content.len()
        ));
        return Ok(());
    }
    let bbox: Vec<f64> = match doc.lookup(&st.dict, b"BBox") {
        Obj::Array(a) => a.iter().filter_map(|o| doc.follow(o).as_num()).collect(),
        _ => Vec::new(),
    };
    if bbox != s.media {
        bad.push(format!("{what}: box {:?}, the PDF's {bbox:?}", s.media));
    }
    let resources = match doc.lookup(&st.dict, b"Resources") {
        Obj::Dict(d) => Some(d),
        _ => None,
    };
    let file = items_of(doc, &content, s, resources, file_fonts);
    diff_items(&what, &s.items, &file, bad);
    Ok(())
}

/// `cargo xtask display DIR JOB`: [`check`], each problem printed.
pub fn run(args: &[String]) -> Result<()> {
    let (Some(dir), Some(job)) = (args.first(), args.get(1)) else {
        anyhow::bail!("usage: cargo xtask display DIR JOB");
    };
    let (bad, counts) = check(Path::new(dir), job)?;
    for b in &bad {
        println!("{b}");
    }
    anyhow::ensure!(bad.is_empty(), "{} problems", bad.len());
    println!("display: ok ({counts})");
    Ok(())
}
