//! `PARTEX_DISPLAY=1`: display lists (DESIGN 4.6, `partex_core::displist`)
//! kept, and written after each build or rebuild to `<job>.display.jsonl`
//! next to the PDF, for checking them (`cargo xtask display`) and for
//! looking at them (`PARTEX_DISPLAY=keep`: kept, nothing written, what
//! keeping them costs). Its lines:
//! - `{"pages":N,"forms":[id,...]}`;
//! - a line per page: `{"page":N,"hash":"…","length":L,
//!   "literals":[[start,end,mode],...],"media":[x0,y0,x1,y1],"rotate":R,
//!   "items":[...],"glyphs":[...]}`, `N` from 1, `length` the content
//!   stream's bytes, `glyphs` the codes in glyph origins' order
//!   (`[font,code,x,y]`, or `null` for a literal's or an included page's);
//! - a line per form: `{"form":ID,"length":L,"literals":[...],
//!   "media":[...],"items":[...]}`;
//! - a line per font: `{"font":K,"name":…,"tfm":…,"size":…,"file":…,
//!   "program":[bytes,"hash"],"reencoded":…,"encoding":[[code,name],...],
//!   "widths":[...],"slant":…,"extend":…}`;
//! - a line per image: `{"image":ID,"kind":"png"|"jpeg"|"pdf",…}`.
//!
//! Items: `["g",font,code,x,y]`, `["m",[a,b,c,d]]`,
//! `["r",x,y,w,h,stroke,[ctm]]`, `["l",mode,codes,[ctm],"text"]` (each
//! byte a character U+0000–U+00FF), `["x","form"|"image",id,[matrix]]`.
//! Fonts are numbered here in the order the file first names them (not
//! the engine's ids, which depend on what was asked before), so a
//! rebuild's file and a cold build's of the same text are the same bytes.
//! Numbers are Rust's shortest forms that read back to the same `f64`.

use std::collections::BTreeMap;
use std::io::Write;

use partex_core::Tex;
use partex_core::displist::{FontId, ImageKind, Item, LiteralMode, Matrix, XKind};
use partex_core::track::Tracker;

use crate::native::NativeHost;
use crate::origins::json_str;

/// What `PARTEX_DISPLAY` asks for: `1`, the lists kept and the side file
/// written after each build (the time the lists took to make reported on
/// stderr); `keep`, the lists kept and nothing else (what keeping them
/// costs a build). `None`: off.
fn wanted() -> Option<bool> {
    match std::env::var("PARTEX_DISPLAY").as_deref() {
        Ok("1") => Some(true),
        Ok("keep") => Some(false),
        _ => None,
    }
}

/// Turn display lists on for `tex` if they are asked for.
pub fn setup<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
    if wanted().is_some() {
        tex.set_display_lists(true);
    }
}

/// The side file's numbering of fonts: by first use.
#[derive(Default)]
struct Fonts {
    ids: BTreeMap<FontId, usize>,
    order: Vec<FontId>,
}

impl Fonts {
    fn of(&mut self, id: FontId) -> usize {
        *self.ids.entry(id).or_insert_with(|| {
            self.order.push(id);
            self.order.len() - 1
        })
    }
}

fn nums(out: &mut Vec<u8>, xs: &[f64]) {
    out.push(b'[');
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let _ = write!(out, "{x}");
    }
    out.push(b']');
}

/// Bytes as a JSON string, each byte the character of its value.
fn bytes_str(out: &mut Vec<u8>, b: &[u8]) {
    let s: String = b.iter().map(|&c| char::from(c)).collect();
    json_str(out, &s);
}

fn items(out: &mut Vec<u8>, items: &[Item], fonts: &mut Fonts) {
    out.push(b'[');
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        match it {
            Item::Glyph { font, code, x, y } => {
                let _ = write!(out, "[\"g\",{},{code},{x},{y}]", fonts.of(*font));
            }
            Item::GlyphMatrix(m) => {
                out.extend_from_slice(b"[\"m\",");
                nums(out, m);
                out.push(b']');
            }
            Item::Rule {
                x,
                y,
                w,
                h,
                stroke,
                ctm,
            } => {
                let _ = write!(out, "[\"r\",{x},{y},{w},{h},{},", u8::from(*stroke));
                nums(out, ctm);
                out.push(b']');
            }
            Item::Literal {
                bytes,
                mode,
                ctm,
                codes,
            } => {
                let m = match mode {
                    LiteralMode::Origin => "origin",
                    LiteralMode::Page => "page",
                    LiteralMode::Direct => "direct",
                };
                let _ = write!(out, "[\"l\",\"{m}\",{codes},");
                nums(out, ctm);
                out.push(b',');
                bytes_str(out, bytes);
                out.push(b']');
            }
            Item::XObject { kind, id, matrix } => {
                let k = match kind {
                    XKind::Form => "form",
                    XKind::Image => "image",
                };
                let _ = write!(out, "[\"x\",\"{k}\",{id},");
                nums(out, matrix);
                out.push(b']');
            }
        }
    }
    out.push(b']');
}

/// A stream's length and literals.
fn stream<T: Tracker>(
    out: &mut Vec<u8>,
    tex: &mut Tex<NativeHost, T>,
    page: usize,
    form: Option<u32>,
) {
    if let Some((bytes, lits)) = tex.display_stream(page, form) {
        let _ = write!(out, "\"length\":{},\"literals\":[", bytes.len());
        for (i, (a, e, m)) in lits.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            let _ = write!(out, "[{a},{e},{m}]");
        }
        out.extend_from_slice(b"],");
    }
}

fn matrix(out: &mut Vec<u8>, key: &str, m: &Matrix) {
    let _ = write!(out, ",\"{key}\":");
    nums(out, m);
}

/// The lists as the file holds them.
pub fn render<T: Tracker>(tex: &mut Tex<NativeHost, T>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut fonts = Fonts::default();
    let mut images = std::collections::BTreeSet::new();
    let forms = tex.display_forms();
    render_pages(&mut out, tex, &mut fonts, &mut images);
    for &id in &forms {
        let _ = write!(out, "{{\"form\":{id},");
        stream(&mut out, tex, 0, Some(id));
        if let Some(l) = tex.display_form(id) {
            out.extend_from_slice(b"\"media\":");
            nums(&mut out, &l.media);
            out.extend_from_slice(b",\"items\":");
            items(&mut out, &l.items, &mut fonts);
            note_images(&l.items, &mut images);
        }
        out.extend_from_slice(b"}\n");
    }
    render_fonts(&mut out, tex, &fonts);
    render_images(&mut out, tex, &images);
    let mut file = Vec::new();
    let _ = writeln!(
        file,
        "{{\"pages\":{},\"forms\":{forms:?}}}",
        tex.display_pages()
    );
    file.extend_from_slice(&out);
    file
}

/// The images `items` draws, noted.
fn note_images(items: &[Item], images: &mut std::collections::BTreeSet<u32>) {
    for it in items {
        if let Item::XObject {
            kind: XKind::Image,
            id,
            ..
        } = it
        {
            images.insert(*id);
        }
    }
}

/// A line per page.
fn render_pages<T: Tracker>(
    out: &mut Vec<u8>,
    tex: &mut Tex<NativeHost, T>,
    fonts: &mut Fonts,
    images: &mut std::collections::BTreeSet<u32>,
) {
    let hashes = tex.display_hashes();
    for n in 0..tex.display_pages() {
        let _ = write!(
            out,
            "{{\"page\":{},\"hash\":\"{:032x}\",",
            n + 1,
            hashes.get(n).copied().unwrap_or(0)
        );
        stream(out, tex, n, None);
        if let Some(l) = tex.display_list(n) {
            out.extend_from_slice(b"\"media\":");
            nums(out, &l.media);
            let _ = write!(out, ",\"rotate\":{},\"items\":", l.rotate);
            items(out, &l.items, fonts);
            note_images(&l.items, images);
        }
        out.extend_from_slice(b",\"glyphs\":[");
        for (i, g) in tex.display_glyphs(n).iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            match g {
                Some(p) => {
                    let _ = write!(out, "[{},{},{},{}]", fonts.of(p.font), p.code, p.x, p.y);
                }
                None => out.extend_from_slice(b"null"),
            }
        }
        out.extend_from_slice(b"]}\n");
    }
}

/// A line per font.
fn render_fonts<T: Tracker>(out: &mut Vec<u8>, tex: &mut Tex<NativeHost, T>, fonts: &Fonts) {
    for (k, &id) in fonts.order.iter().enumerate() {
        let Some(f) = tex.display_font(id) else {
            let _ = writeln!(out, "{{\"font\":{k}}}");
            continue;
        };
        let _ = write!(out, "{{\"font\":{k},\"name\":");
        json_str(out, &f.name);
        out.extend_from_slice(b",\"tfm\":");
        json_str(out, &f.tfm);
        let _ = write!(out, ",\"size\":{},\"file\":", f.size);
        json_str(out, &f.file);
        match &f.program {
            Some(p) => {
                let _ = write!(
                    out,
                    ",\"program\":[{},\"{:032x}\"]",
                    p.len(),
                    partex_core::persist_hash(&p[..])
                );
            }
            None => out.extend_from_slice(b",\"program\":null"),
        }
        let _ = write!(out, ",\"reencoded\":{},\"encoding\":[", f.reencoded);
        let mut first = true;
        for (c, name) in f.encoding.iter().enumerate() {
            if name != ".notdef" {
                if !first {
                    out.push(b',');
                }
                first = false;
                let _ = write!(out, "[{c},");
                json_str(out, name);
                out.push(b']');
            }
        }
        out.extend_from_slice(b"],\"widths\":");
        nums(out, &f.widths);
        let _ = writeln!(out, ",\"slant\":{},\"extend\":{}}}", f.slant, f.extend);
    }
}

/// A line per image.
fn render_images<T: Tracker>(
    out: &mut Vec<u8>,
    tex: &mut Tex<NativeHost, T>,
    images: &std::collections::BTreeSet<u32>,
) {
    for &id in images {
        let Some(im) = tex.display_image(id) else {
            continue;
        };
        let _ = write!(out, "{{\"image\":{id},\"kind\":");
        match &im.kind {
            ImageKind::Png => out.extend_from_slice(b"\"png\""),
            ImageKind::Jpeg => out.extend_from_slice(b"\"jpeg\""),
            ImageKind::Pdf {
                page,
                bbox,
                matrix: m,
                codes,
            } => {
                let _ = write!(out, "\"pdf\",\"page\":{page},\"codes\":{codes},\"bbox\":");
                nums(out, bbox);
                matrix(out, "matrix", m);
            }
        }
        out.extend_from_slice(b",\"name\":");
        json_str(out, &im.name);
        let _ = writeln!(
            out,
            ",\"bytes\":[{},\"{:032x}\"],\"width\":{},\"height\":{}}}",
            im.data.len(),
            partex_core::persist_hash(&im.data[..]),
            im.width,
            im.height
        );
    }
}

/// Write `<job>.display.jsonl` (if display lists are on).
pub fn write<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
    if !tex.display_lists_on() || wanted() != Some(true) {
        return;
    }
    let Some(job) = tex.job_name_bytes() else {
        return;
    };
    // (what a renderer asks for after a build: every page's list, and
    // the hashes; each made once, then the file from them)
    let t0 = std::time::Instant::now();
    let pages = tex.display_pages();
    let items: usize = (0..pages)
        .map(|n| tex.display_list(n).map_or(0, |l| l.items.len()))
        .sum();
    let t1 = std::time::Instant::now();
    let _ = tex.display_hashes();
    let t2 = std::time::Instant::now();
    let mut name = job;
    name.extend_from_slice(b".display.jsonl");
    let name = tex.host().in_output_dir(&name).unwrap_or(name);
    let text = render(tex);
    eprintln!(
        "partex: display lists: {pages} pages, {items} items made in {:.1} ms, \
         hashed in {:.1} ms; side file {} bytes in {:.1} ms",
        (t1 - t0).as_secs_f64() * 1e3,
        (t2 - t1).as_secs_f64() * 1e3,
        text.len(),
        t2.elapsed().as_secs_f64() * 1e3
    );
    let path = crate::native::path(&name);
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("partex: {}: {e}", path.display());
    }
}
