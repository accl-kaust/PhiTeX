//! Checks phitex-otf's fonts and shaping against the native words of the
//! oracle's XDV files (crates/output/xdvipdfmx/tools/corpus.sh `shapes`:
//! the corpus's documents run with `\XeTeXtracingfonts=1` and
//! `\XeTeXgenerateactualtext=1`):
//!
//!     scripts/sandbox cargo run -p phitex-otf --release --example otf-xdv -- \
//!         INDEX CORPUS_DIR [--examples N] [--no-memo]
//!
//! For each `NAME.xdv` with its `NAME.log`:
//!
//! - fonts: every `Requested font "SPEC" at S` the log traces, with the
//!   `-> PATH` XeTeX resolved it to, is loaded with
//!   [`xetex::find_native_font`] over INDEX (`otf-index`); the path and
//!   face index must be the ones XeTeX found, and none where XeTeX found a
//!   TFM or nothing. (XeTeX prints a name lookup's path from freed memory:
//!   those are checked by the XDV's native font definitions, each of which
//!   one of our loaded requests must match.)
//! - shaping: every `set_text_and_glyphs` record (the word's text as XeTeX
//!   shaped it, after any mapping) is laid out again with
//!   [`XeTeXFont::layout_word`] in a font the log requested with the same
//!   file, face index and size, and the same color, extend, slant and
//!   embolden as the record's font definition. The width and every
//!   glyph's id, x and y must be equal. Several requests can share those
//!   (feature sets): the word matches if one of them gives the record.
//!   Each candidate keeps its last script across the document's words,
//!   in XDV order (XeTeX's order is the measuring order; a word that
//!   matches only from a fresh state is counted apart).
//!
//! Prints the counts, then the first mismatches: the document, the page,
//! the spec, the text, the first differing glyph (oracle, ours).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::many_single_char_names,
    clippy::cast_precision_loss,
    // (XeTeX's printed paths are compared as it prints them)
    clippy::case_sensitive_file_extension_comparisons
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::cell::RefCell;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use phitex_kpse::{Format, Kpse};
use phitex_otf::index::FontIndex;
use phitex_otf::shape::Shaper;
use phitex_otf::xetex::{self, XeTeXFont, fontmgr::FontManager};
use phitex_otf::{FontSource, KpseFormat};

/// The files: kpathsea in process (`phitex-kpse`) as `xetex`, from the
/// TeX Live of the `tex` on the PATH.
struct Src(RefCell<Kpse>);

impl Src {
    fn new() -> Src {
        let dir = std::env::var_os("PATH")
            .and_then(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("tex"))
                    .filter_map(|t| t.canonicalize().ok())
                    .find(|t| t.is_file())
                    .and_then(|t| t.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin"));
        Src(RefCell::new(Kpse::new(&dir, "xetex", "xetex")))
    }
}

impl FontSource for Src {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        std::fs::read(path).ok().map(Arc::from)
    }
    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        if name.starts_with('/') {
            return Path::new(name).exists().then(|| name.to_string());
        }
        let fmt = match format {
            KpseFormat::OpenType => Format::OpenType,
            KpseFormat::TrueType => Format::TrueType,
            KpseFormat::Type1 => Format::Type1,
            KpseFormat::MiscFonts => Format::MiscFonts,
        };
        let p = self.0.borrow_mut().find_file(name.as_bytes(), fmt, false)?;
        Some(String::from_utf8_lossy(&p).into_owned())
    }
}

/// A native font definition of the XDV.
#[derive(Clone, Debug, Default, PartialEq)]
struct Def {
    size: i32,
    path: String,
    index: u32,
    rgba: Option<u32>,
    extend: Option<i32>,
    slant: Option<i32>,
    embolden: Option<i32>,
}

/// A `set_text_and_glyphs` (or `set_glyphs`, no text) record.
struct Word {
    page: usize,
    font: u32,
    text: Option<Vec<u16>>,
    width: i32,
    glyphs: Vec<(i32, i32, u16)>,
}

fn xdv_words(d: &[u8]) -> (BTreeMap<u32, Def>, Vec<Word>) {
    let mut defs = BTreeMap::new();
    let mut words = Vec::new();
    let (mut font, mut page) = (0u32, 0usize);
    let mut p = 0usize;
    // (a read past the end, in an XDV XeTeX did not finish, reads 0;
    // the loop stops there)
    let u = |p: usize, n: usize| -> i64 {
        let mut v: i64 = 0;
        for i in 0..n {
            v = (v << 8) | i64::from(d.get(p + i).copied().unwrap_or(0));
        }
        v
    };
    let s = |p: usize, n: usize| -> i64 {
        let v = u(p, n);
        let bits = 8 * n as u32;
        if v >= 1 << (bits - 1) { v - (1 << bits) } else { v }
    };
    while p < d.len() {
        let op = d[p];
        p += 1;
        match op {
            0..=127 | 138 | 141 | 142 | 147 | 152 | 161 | 166 | 140 => {}
            128..=131 => p += (op - 127) as usize,
            132 | 137 => p += 8,
            133..=136 => p += (op - 132) as usize,
            139 => {
                p += 44;
                page += 1;
            }
            143..=146 => p += (op - 142) as usize,
            148..=151 => p += (op - 147) as usize,
            153..=156 => p += (op - 152) as usize,
            157..=160 => p += (op - 156) as usize,
            162..=165 => p += (op - 161) as usize,
            167..=170 => p += (op - 166) as usize,
            171..=234 => font = u32::from(op - 171),
            235..=238 => {
                font = u(p, (op - 234) as usize) as u32;
                p += (op - 234) as usize;
            }
            239..=242 => {
                let n = (op - 238) as usize;
                let k = u(p, n) as usize;
                p += n + k;
            }
            243..=246 => {
                let n = (op - 242) as usize;
                p += n + 12;
                let a = u(p, 1) as usize;
                let l = u(p + 1, 1) as usize;
                p += 2 + a + l;
            }
            247 => {
                p += 14;
                let k = u(p - 1, 1) as usize;
                p += k;
            }
            // (248, the postamble, ends the pages like any op unknown here)
            252 => {
                let k = u(p, 4) as u32;
                let size = s(p + 4, 4) as i32;
                p += 8;
                let flags = u(p, 2) as u16;
                p += 2;
                let l = u(p, 1) as usize;
                let Some(path) = d.get(p + 1..p + 1 + l) else { break };
                let path = String::from_utf8_lossy(path).into_owned();
                let index = u(p + 1 + l, 4) as u32;
                p += 1 + l + 4;
                let mut def = Def { size, path, index, ..Def::default() };
                if flags & 0x0200 != 0 {
                    def.rgba = Some(u(p, 4) as u32);
                    p += 4;
                }
                if flags & 0x1000 != 0 {
                    def.extend = Some(s(p, 4) as i32);
                    p += 4;
                }
                if flags & 0x2000 != 0 {
                    def.slant = Some(s(p, 4) as i32);
                    p += 4;
                }
                if flags & 0x4000 != 0 {
                    def.embolden = Some(s(p, 4) as i32);
                    p += 4;
                }
                defs.insert(k, def);
            }
            253 | 254 => {
                let mut text = None;
                if op == 254 {
                    let l = u(p, 2) as usize;
                    text = Some((0..l).map(|i| u(p + 2 + 2 * i, 2) as u16).collect());
                    p += 2 + 2 * l;
                }
                let width = s(p, 4) as i32;
                let n = u(p + 4, 2) as usize;
                p += 6;
                let mut glyphs: Vec<(i32, i32, u16)> = (0..n)
                    .map(|i| (s(p + 8 * i, 4) as i32, s(p + 8 * i + 4, 4) as i32, 0))
                    .collect();
                p += 8 * n;
                for (i, g) in glyphs.iter_mut().enumerate() {
                    g.2 = u(p + 2 * i, 2) as u16;
                }
                p += 2 * n;
                if p > d.len() {
                    break;
                }
                words.push(Word { page, font, text, width, glyphs });
            }
            // (an unknown op: an XDV cut short; what was read is kept)
            _ => break,
        }
    }
    (defs, words)
}

/// TeX's `round_decimals` and `scan_dimen` for a printed `D.DDDpt`.
fn scaled(s: &str) -> Option<i32> {
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    let ip: i32 = i.parse().ok()?;
    let mut a: i32 = 0;
    for c in f.bytes().take(17).collect::<Vec<_>>().iter().rev() {
        a = (a + i32::from(c - b'0') * (1 << 17)) / 10;
    }
    Some(ip * 65536 + (a + 1) / 2)
}

/// The log's traced requests: (spec, size as `findnativefont` takes it,
/// the resolved path or none).
fn requests(log: &str) -> Vec<(String, i32, Option<String>)> {
    let mut out: Vec<(String, i32, Option<String>)> = Vec::new();
    let lines: Vec<&str> = log.lines().collect();
    for (k, l) in lines.iter().enumerate() {
        let Some(rest) = l.strip_prefix("Requested font \"") else {
            continue;
        };
        let Some(q) = rest.rfind('"') else { continue };
        let spec = rest[..q].to_string();
        let tail = rest[q + 1..].trim();
        let size = if let Some(v) = tail.strip_prefix("at ") {
            scaled(v.trim_end_matches("pt"))
        } else if let Some(v) = tail.strip_prefix("scaled ") {
            v.parse::<i32>().ok().map(|n| -n)
        } else {
            None
        };
        let Some(size) = size else { continue };
        // (the ` -> PATH` line follows, after the name lookup's own lines)
        let path = lines[k + 1..]
            .iter()
            .take(6)
            .take_while(|x| !x.starts_with("Requested font \""))
            .find_map(|x| x.trim_start().strip_prefix("-> ").map(str::to_string));
        out.push((spec, size, path));
    }
    out
}

#[derive(Default)]
struct Tally {
    fonts: usize,
    fonts_bad: usize,
    fonts_unloaded: usize,
    fonts_extra: usize,
    fonts_unverified: usize,
    defs: usize,
    defs_unmatched: usize,
    words: usize,
    words_ok: usize,
    words_ok_fresh: usize,
    words_bad: usize,
    words_no_font: usize,
    glyph_runs: usize,
    /// Time in `find_native_font` and in `layout_word` (ns), and the
    /// layouts made.
    load_ns: u128,
    shape_ns: u128,
    layouts: usize,
    examples: Vec<String>,
    bad_by_font: BTreeMap<String, usize>,
}

fn same(def: &Def, f: &XeTeXFont) -> bool {
    let fixed = |x: f32| xetex::d2fix(f64::from(x));
    def.path == *f.path
        && def.index == f.index
        && def.size == f.scaled_size
        && def.rgba.unwrap_or(0x0000_00FF) == f.rgba
        && def.extend.unwrap_or(65536) == fixed(f.extend)
        && def.slant.unwrap_or(0) == fixed(f.slant)
        && def.embolden.unwrap_or(0) == fixed(f.embolden)
}

/// A font request loaded (a function of its own: `callgrind
/// --toggle-collect='*load_font*'` counts the loads alone).
#[inline(never)]
fn load_font(mgr: &mut FontManager, src: &Src, spec: &str, size: i32) -> (Option<XeTeXFont>, Vec<xetex::Diagnostic>) {
    xetex::find_native_font(mgr, src, spec, size, 0)
}

/// A word laid out (`--toggle-collect='*shape_word*'`).
#[inline(never)]
fn shape_word(f: &mut XeTeXFont, shaper: &mut Shaper, text: &[u16], ls: i32) -> xetex::NativeLayout {
    f.layout_word(shaper, text, ls)
}

fn check(mgr: &mut FontManager, src: &Src, xdv: &Path, t: &mut Tally, max_ex: usize, memo: bool) {
    let doc = xdv.display().to_string();
    let Ok(log) = std::fs::read(xdv.with_extension("log")) else {
        return;
    };
    let log = String::from_utf8_lossy(&log);
    let d = std::fs::read(xdv).expect("xdv");
    let (defs, words) = xdv_words(&d);
    // the requests, loaded once each (spec and size)
    let mut fonts: Vec<(String, XeTeXFont)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (spec, size, path) in requests(&log) {
        if !seen.insert((spec.clone(), size)) {
            continue;
        }
        t.fonts += 1;
        let t0 = Instant::now();
        let (f, _) = load_font(mgr, src, &spec, size);
        t.load_ns += t0.elapsed().as_nanos();
        // What XeTeX found: a native file (its path), a TFM or nothing
        // (no native font), or a path it printed unreadably (its name
        // lookups print a freed string: checked through the XDV's font
        // definitions below).
        let path = path.unwrap_or_default();
        let native = Path::new(&path).is_file() && !path.ends_with(".tfm");
        let none = path.ends_with(".tfm") || path.starts_with("font not found");
        match (&f, native, none) {
            (None, true, _) => {
                t.fonts_unloaded += 1;
                if t.examples.len() < max_ex {
                    t.examples.push(format!("{doc}: font \"{spec}\": XeTeX {path}, ours none"));
                }
            }
            (Some(f), _, true) => {
                t.fonts_extra += 1;
                if t.examples.len() < max_ex {
                    t.examples.push(format!("{doc}: font \"{spec}\": XeTeX {path}, ours {}", f.path));
                }
            }
            (Some(f), true, _) => {
                // (XeTeX prints the path, with `:INDEX` for a face of a collection)
                let ours = if f.index == 0 { f.path.to_string() } else { format!("{}:{}", f.path, f.index) };
                if ours != path && *f.path != *path {
                    t.fonts_bad += 1;
                    if t.examples.len() < max_ex {
                        t.examples.push(format!("{doc}: font \"{spec}\": XeTeX {path}, ours {ours}"));
                    }
                }
            }
            (_, false, false) => t.fonts_unverified += 1,
            (None, false, true) => {}
        }
        if let Some(f) = f {
            fonts.push((spec, f));
        }
    }
    // every native font the XDV defines is one a request loaded
    for def in defs.values() {
        t.defs += 1;
        if !fonts.iter().any(|(_, f)| same(def, f)) {
            t.defs_unmatched += 1;
            if t.examples.len() < max_ex {
                t.examples.push(format!(
                    "{doc}: XDV font {}:{} at {}sp (rgba {:?} extend {:?} slant {:?} embolden {:?}): no request of ours loads it",
                    def.path, def.index, def.size, def.rgba, def.extend, def.slant, def.embolden
                ));
            }
        }
    }
    let mut shaper = Shaper::new();
    shaper.set_memo(memo);
    for w in &words {
        let Some(text) = &w.text else {
            t.glyph_runs += 1;
            continue;
        };
        t.words += 1;
        let Some(def) = defs.get(&w.font) else {
            t.words_no_font += 1;
            continue;
        };
        let cands: Vec<usize> = (0..fonts.len()).filter(|&i| same(def, &fonts[i].1)).collect();
        if cands.is_empty() {
            t.words_no_font += 1;
            if t.examples.len() < max_ex {
                t.examples.push(format!(
                    "{doc} p{}: no traced request for {}:{} at {}sp",
                    w.page, def.path, def.index, def.size
                ));
            }
            continue;
        }
        let want = (w.width, w.glyphs.clone());
        let mut ok = false;
        let mut first: Option<(String, (i32, Vec<(i32, i32, u16)>))> = None;
        for &i in &cands {
            let f = &mut fonts[i].1;
            let ls = f.letter_space;
            let t0 = Instant::now();
            let l = shape_word(f, &mut shaper, text, ls);
            t.shape_ns += t0.elapsed().as_nanos();
            t.layouts += 1;
            let got = (l.width, l.glyphs.iter().map(|g| (g.x, g.y, g.gid)).collect::<Vec<_>>());
            if got == want {
                ok = true;
            } else if first.is_none() {
                first = Some((fonts[i].0.clone(), got));
            }
        }
        if ok {
            t.words_ok += 1;
            continue;
        }
        // from a fresh state (no last script)
        let fresh = cands.iter().any(|&i| {
            let mut f = fonts[i].1.clone();
            f.last_script = None;
            let ls = f.letter_space;
            let l = f.layout_word(&mut shaper, text, ls);
            (l.width, l.glyphs.iter().map(|g| (g.x, g.y, g.gid)).collect::<Vec<_>>()) == want
        });
        if fresh {
            t.words_ok_fresh += 1;
            continue;
        }
        t.words_bad += 1;
        let (spec, got) = first.expect("a candidate");
        *t.bad_by_font.entry(spec.clone()).or_default() += 1;
        if t.examples.len() < max_ex {
            let k = want.1.iter().zip(&got.1).position(|(a, b)| a != b).unwrap_or(want.1.len().min(got.1.len()));
            t.examples.push(format!(
                "{doc} p{}: \"{spec}\" ({} candidates) \"{}\": width {} ours {}; {} glyphs ours {}; glyph {k}: oracle {:?} ours {:?}",
                w.page,
                cands.len(),
                String::from_utf16_lossy(text),
                want.0,
                got.0,
                want.1.len(),
                got.1.len(),
                want.1.get(k),
                got.1.get(k)
            ));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = FontIndex::from_bytes(&std::fs::read(&args[1]).expect("index")).expect("an index");
    let dir = PathBuf::from(&args[2]);
    let max_ex = args
        .iter()
        .position(|a| a == "--examples")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let memo = !args.iter().any(|a| a == "--no-memo");
    let out = Command::new("find").arg(&dir).args(["-name", "*.xdv"]).output().expect("find");
    let mut xdvs: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout).lines().map(PathBuf::from).collect();
    xdvs.sort();
    let index = Arc::new(index);
    let src = Src::new();
    let mut t = Tally::default();
    let mut per_doc = Vec::new();
    for x in &xdvs {
        let mut mgr = FontManager::new(index.clone());
        let mut dt = Tally::default();
        check(&mut mgr, &src, x, &mut dt, max_ex, memo);
        per_doc.push(format!(
            "{}: fonts {} (path differs {}, unloaded {}, extra {}, by name {}), XDV fonts {} unmatched {}, words {} ok {} (fresh {}) bad {} no font {}; load {} ms, {} layouts {} ms",
            x.strip_prefix(&dir).unwrap_or(x).display(),
            dt.fonts,
            dt.fonts_bad,
            dt.fonts_unloaded,
            dt.fonts_extra,
            dt.fonts_unverified,
            dt.defs,
            dt.defs_unmatched,
            dt.words,
            dt.words_ok,
            dt.words_ok_fresh,
            dt.words_bad,
            dt.words_no_font,
            dt.load_ns / 1_000_000,
            dt.layouts,
            dt.shape_ns / 1_000_000
        ));
        t.fonts += dt.fonts;
        t.fonts_bad += dt.fonts_bad;
        t.fonts_unloaded += dt.fonts_unloaded;
        t.fonts_extra += dt.fonts_extra;
        t.fonts_unverified += dt.fonts_unverified;
        t.defs += dt.defs;
        t.defs_unmatched += dt.defs_unmatched;
        t.words += dt.words;
        t.words_ok += dt.words_ok;
        t.words_ok_fresh += dt.words_ok_fresh;
        t.words_bad += dt.words_bad;
        t.words_no_font += dt.words_no_font;
        t.glyph_runs += dt.glyph_runs;
        t.load_ns += dt.load_ns;
        t.shape_ns += dt.shape_ns;
        t.layouts += dt.layouts;
        for (k, v) in dt.bad_by_font {
            *t.bad_by_font.entry(k).or_default() += v;
        }
        let room = max_ex.saturating_sub(t.examples.len());
        t.examples.extend(dt.examples.into_iter().take(room));
    }
    println!(
        "fonts: {} requests: {} resolved to another file, {} native in XeTeX not loaded by us, {} loaded by us where XeTeX has none, {} name lookups checked through the XDV; XDV native fonts {}, {} with no request of ours loading them",
        t.fonts, t.fonts_bad, t.fonts_unloaded, t.fonts_extra, t.fonts_unverified, t.defs, t.defs_unmatched
    );
    println!(
        "words: {} ({} XDVs), {} identical, {} only from a fresh state, {} differ, {} without a traced font; {} glyph runs without text not checked",
        t.words,
        xdvs.len(),
        t.words_ok,
        t.words_ok_fresh,
        t.words_bad,
        t.words_no_font,
        t.glyph_runs
    );
    println!(
        "time: font loads {} ms ({} fonts), {} layouts {} ms ({:.2} us each; the shaper's memo {})",
        t.load_ns / 1_000_000,
        t.fonts,
        t.layouts,
        t.shape_ns / 1_000_000,
        t.shape_ns as f64 / 1000.0 / t.layouts.max(1) as f64,
        if memo { "on" } else { "off" }
    );
    let mut by: Vec<_> = t.bad_by_font.into_iter().collect();
    by.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (k, v) in by.iter().take(15) {
        println!("  {v} words differ in \"{k}\"");
    }
    for e in &t.examples {
        println!("{e}");
    }
    for l in per_doc {
        println!("doc {l}");
    }
}
