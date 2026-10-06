//! `<job>.glyphruns.jsonl` (partex's `PARTEX_GLYPH_RUNS=1`: xdvipdfmx's
//! glyph runs, `partex_xdvipdfmx::glyphrun`) against the job's PDF and
//! its `<job>.origins.jsonl`: a run per origin and per code the PDF shows,
//! where the PDF's text and transformation matrices put it, with the text
//! its font's `ToUnicode` `CMap` gives it, and the `/ActualText` spans'.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use partex_engine::pdfread::{Doc, Obj, Page};
use partex_engine::pdftext;
use serde_json::Value;

/// The runs of each page.
fn read_runs(path: &Path) -> Result<Vec<Vec<Value>>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l)?;
            Ok(v["runs"].as_array().cloned().unwrap_or_default())
        })
        .collect()
}

/// The origins' count on each page.
fn origin_counts(path: &Path) -> Result<Vec<usize>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("page").is_some())
        .map(|v| v["glyphs"].as_array().map_or(0, Vec::len))
        .collect())
}

/// A `ToUnicode` `CMap`'s `bfchar` and `bfrange` entries: code to text.
fn tounicode(cmap: &[u8]) -> std::collections::HashMap<u32, String> {
    let s = String::from_utf8_lossy(cmap);
    let mut toks = Vec::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '<' => {
                let end = s[i..].find('>').map_or(s.len(), |e| i + e);
                toks.push(s[i..=end.min(s.len() - 1)].to_owned());
                while chars.peek().is_some_and(|&(j, _)| j <= end) {
                    chars.next();
                }
            }
            '[' | ']' => toks.push(c.to_string()),
            c if c.is_whitespace() => {}
            _ => {
                let end = s[i..]
                    .find(|c: char| c.is_whitespace() || "<[]".contains(c))
                    .map_or(s.len(), |e| i + e);
                toks.push(s[i..end].to_owned());
                while chars.peek().is_some_and(|&(j, _)| j < end) {
                    chars.next();
                }
            }
        }
    }
    let hex = |t: &str| -> Vec<u8> {
        let h: Vec<u8> = t
            .trim_matches(|c| c == '<' || c == '>')
            .bytes()
            .filter(u8::is_ascii_hexdigit)
            .collect();
        h.chunks(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("0"), 16).unwrap_or(0))
            .collect()
    };
    let num = |b: &[u8]| b.iter().fold(0u32, |a, &c| (a << 8) | u32::from(c));
    let mut map = std::collections::HashMap::new();
    let mut k = 0;
    while k < toks.len() {
        match toks[k].as_str() {
            "beginbfchar" => {
                k += 1;
                while k + 1 < toks.len() && toks[k] != "endbfchar" {
                    map.insert(num(&hex(&toks[k])), utf16be(&hex(&toks[k + 1])));
                    k += 2;
                }
            }
            "beginbfrange" => {
                k += 1;
                while k + 2 < toks.len() && toks[k] != "endbfrange" {
                    let (lo, hi) = (num(&hex(&toks[k])), num(&hex(&toks[k + 1])));
                    if toks[k + 2] == "[" {
                        let mut j = k + 3;
                        let mut c = lo;
                        while j < toks.len() && toks[j] != "]" {
                            map.insert(c, utf16be(&hex(&toks[j])));
                            c += 1;
                            j += 1;
                        }
                        k = j + 1;
                    } else {
                        let mut dst = hex(&toks[k + 2]);
                        for c in lo..=hi {
                            map.insert(c, utf16be(&dst));
                            if let Some(last) = dst.last_mut() {
                                *last = last.wrapping_add(1);
                            }
                        }
                        k += 3;
                    }
                }
            }
            _ => k += 1,
        }
    }
    map
}

/// UTF-16BE bytes as text.
fn utf16be(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&u| u16::from_be_bytes(u))
        .collect();
    String::from_utf16_lossy(&units)
}

/// A PDF string's text: UTF-16BE after a BOM, else `PDFDocEncoding` (as
/// Latin-1).
fn pdf_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xfe, 0xff]) {
        utf16be(rest)
    } else {
        b.iter().map(|&c| char::from(c)).collect()
    }
}

/// The literal string whose text starts at `at` in `content` (after its
/// `(`): its bytes, escapes read, and where it ends (after its `)`).
fn literal(content: &[u8], at: usize) -> (Vec<u8>, usize) {
    let mut bytes = Vec::new();
    let mut pos = at;
    let mut depth = 0;
    while let Some(&byte) = content.get(pos) {
        pos += 1;
        match byte {
            b'\\' => {
                let next = content.get(pos).copied().unwrap_or(0);
                if (b'0'..=b'7').contains(&next) {
                    let mut value = 0u8;
                    for _ in 0..3 {
                        match content.get(pos) {
                            Some(&d @ b'0'..=b'7') => {
                                value = value.wrapping_mul(8).wrapping_add(d - b'0');
                                pos += 1;
                            }
                            _ => break,
                        }
                    }
                    bytes.push(value);
                } else {
                    bytes.push(match next {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                    pos += 1;
                }
            }
            b'(' => {
                depth += 1;
                bytes.push(byte);
            }
            b')' if depth == 0 => break,
            b')' => {
                depth -= 1;
                bytes.push(byte);
            }
            other => bytes.push(other),
        }
    }
    (bytes, pos)
}

/// The `/ActualText` strings of a content stream, in order.
fn actual_texts(content: &[u8]) -> Vec<String> {
    let key = b"/ActualText (";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = content[from..].windows(key.len()).position(|w| w == key) {
        let (bytes, end) = literal(content, from + found + key.len());
        out.push(pdf_text(&bytes));
        from = end;
    }
    out
}

/// Check `dir/<job>.glyphruns.jsonl` against `dir/<job>.pdf` and
/// `dir/<job>.origins.jsonl`: the problems found.
pub fn check(dir: &Path, job: &str) -> Result<Vec<String>> {
    let pages = read_runs(&dir.join(format!("{job}.glyphruns.jsonl")))?;
    let origins = origin_counts(&dir.join(format!("{job}.origins.jsonl")))?;
    let pdf = dir.join(format!("{job}.pdf"));
    let data: Arc<[u8]> = fs::read(&pdf)
        .with_context(|| format!("reading {}", pdf.display()))?
        .into();
    let doc = Doc::open(&data).map_err(|e| anyhow!("{}: {e:?}", pdf.display()))?;
    let mut bad = Vec::new();
    if pages.len() != origins.len() || pages.len() != doc.num_pages() {
        bad.push(format!(
            "{} pages of runs, {} of origins, {} in the PDF",
            pages.len(),
            origins.len(),
            doc.num_pages()
        ));
    }
    for (n, runs) in pages.iter().enumerate() {
        let page_no = n + 1;
        if let Some(&o) = origins.get(n)
            && o != runs.len()
        {
            bad.push(format!("page {page_no}: {} runs, {o} origins", runs.len()));
        }
        if let Some(page) = doc.page(page_no) {
            check_page(&doc, &page, page_no, runs, &mut bad);
        }
    }
    Ok(bad)
}

/// [`check`] of page `page_no`'s runs against the PDF's page `page`.
fn check_page(doc: &Doc, page: &Page, page_no: usize, runs: &[Value], bad: &mut Vec<String>) {
    let shown = pdftext::page_codes(doc, page);
    if shown.len() != runs.len() {
        bad.push(format!(
            "page {page_no}: {} runs, {} codes shown",
            runs.len(),
            shown.len()
        ));
        return;
    }
    // (each font's ToUnicode, by its resource name)
    let mut maps: std::collections::HashMap<Vec<u8>, Option<_>> = std::collections::HashMap::new();
    let fonts = page
        .resources
        .as_ref()
        .map(|res| doc.lookup(res, b"Font"))
        .and_then(|f| f.as_dict().cloned())
        .unwrap_or_default();
    // (a run is where TeX put its glyph; the PDF's string starts there
    // (`Tm`, `Td`, to its precision), its next glyphs where the font's
    // widths take them, each its own TFM width's difference off, under a
    // thousandth of an em)
    let mut last: Option<(f64, f64)> = None;
    for (index, (run, code)) in runs.iter().zip(&shown).enumerate() {
        let at = (
            run["x"].as_f64().unwrap_or(0.0),
            run["y"].as_f64().unwrap_or(0.0),
        );
        let em = run["size"].as_f64().unwrap_or(10.0)
            * run["ctm"][0]
                .as_f64()
                .unwrap_or(1.0)
                .hypot(run["ctm"][1].as_f64().unwrap_or(0.0));
        let off = (code.x - at.0, code.y - at.1);
        let step = last.map_or(off.0.hypot(off.1), |l| (off.0 - l.0).hypot(off.1 - l.1));
        if off.0.hypot(off.1) > 0.02 * em || step.min(off.0.hypot(off.1)) > 0.0015 * em + 0.002 {
            bad.push(format!(
                "page {page_no}, run {index}: at ({:.3}, {:.3}), the PDF's at ({:.3}, {:.3})",
                at.0, at.1, code.x, code.y
            ));
        }
        last = Some(off);
        let id = match run["source"].as_str() {
            Some("native") => run["gid"].as_u64(),
            Some("type1") => run["code"].as_u64(),
            _ => None,
        };
        // (a TrueType/OpenType map entry's code is the TFM's, not the
        // glyph's)
        if id.is_some() && id != Some(u64::from(code.code)) {
            bad.push(format!(
                "page {page_no}, run {index}: code {id:?}, the PDF shows {}",
                code.code
            ));
        }
        if run["actual_text"].as_bool() == Some(true) {
            continue;
        }
        let map = maps.entry(code.font.clone()).or_insert_with(|| {
            let Obj::Dict(font) = doc.lookup(&fonts, &code.font) else {
                return None;
            };
            match doc.lookup(&font, b"ToUnicode") {
                Obj::Stream(st) => Some(tounicode(&doc.decode(&st))),
                _ => None,
            }
        });
        let Some(map) = map else {
            continue;
        };
        let want = map.get(&code.code).cloned();
        let have = run["text"].as_str().map(str::to_owned);
        if want != have {
            bad.push(format!(
                "page {page_no}, run {index}: text {have:?}, the PDF's ToUnicode {want:?}"
            ));
        }
    }
    // the clusters' ActualText, in order
    let spans = actual_texts(&pdftext::page_content(doc, page));
    let mut have = Vec::new();
    let mut cluster = None;
    for run in runs {
        if run["actual_text"].as_bool() == Some(true) && cluster != run["cluster"].as_u64() {
            have.push(run["text"].as_str().unwrap_or_default().to_owned());
            cluster = run["cluster"].as_u64();
        }
    }
    if have != spans {
        bad.push(format!(
            "page {page_no}: ActualText {have:?}, the PDF's {spans:?}"
        ));
    }
}

/// What `tests/e2e/xelatex-runs.tex`'s runs must show besides: the
/// problems found.
pub fn expect_xelatex_runs(dir: &Path) -> Result<Vec<String>> {
    let pages = read_runs(&dir.join("xelatex-runs.glyphruns.jsonl"))?;
    let runs = pages.first().cloned().unwrap_or_default();
    let mut bad = Vec::new();
    let text = |r: &Value| r["text"].as_str().unwrap_or_default().to_owned();
    let file = |r: &Value| r["font_file"].as_str().unwrap_or_default().to_owned();
    let mut ok = |cond: bool, what: &str| {
        if !cond {
            bad.push(what.to_owned());
        }
    };
    // colour: the stack's, a native font's own; RGB, CMYK and gray
    let colored: Vec<&Value> = runs
        .iter()
        .filter(|r| r["rgba"].as_u64() != Some(0xff))
        .collect();
    let s: String = colored.iter().map(|r| text(r)).collect();
    let rgba: Vec<u64> = colored.iter().filter_map(|r| r["rgba"].as_u64()).collect();
    let want: Vec<u64> = [0xff00_00ff_u64; 7]
        .into_iter()
        .chain([0x8080_80ff; 4])
        .chain([0x0000_ffff; 8])
        .collect();
    ok(
        s == "redcmykgrayblueblue" && rgba == want,
        &format!("colours: {s:?} {rgba:x?}"),
    );
    let comps = |r: Option<&&Value>, space: &str| -> Vec<f64> {
        r.and_then(|r| r["color"][space].as_array())
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default()
    };
    ok(
        comps(colored.first(), "rgb") == [1.0, 0.0, 0.0]
            && comps(colored.get(3), "cmyk") == [0.0, 1.0, 1.0, 0.0]
            && comps(colored.get(7), "gray") == [0.5],
        "colour spaces: RGB, CMYK and gray kept",
    );
    // classic math: the summation sign by its name, its text from the AGL
    ok(
        runs.iter().any(|r| {
            file(r).ends_with("cmex10.pfb") && r["glyph_name"] == "summationtext" && text(r) == "∑"
        }),
        "math: cmex10's summationtext as ∑",
    );
    // a virtual font's characters as its base font's glyphs
    ok(
        runs.iter()
            .filter(|r| file(r).ends_with("utmr8a.pfb"))
            .map(text)
            .collect::<String>()
            == "Avirtualfont:ﬁ“q”",
        "virtual font: ptmr8t's glyphs from utmr8a.pfb",
    );
    ok(
        runs.iter()
            .any(|r| r["source"] == "truetype" && file(r).ends_with("Arvo-Regular.ttf"))
            && runs
                .iter()
                .any(|r| r["source"] == "opentype" && file(r).ends_with("GFSBodoni.otf")),
        "TrueType and OpenType map entries",
    );
    // rotated text: its matrix (positions are checked against the PDF)
    let rot = |deg: f64, f: &str| -> String {
        let (s, c) = deg.to_radians().sin_cos();
        runs.iter()
            .filter(|r| {
                file(r).ends_with(f)
                    && (r["ctm"][0].as_f64().unwrap_or(0.0) - c).abs() < 1e-3
                    && (r["ctm"][1].as_f64().unwrap_or(0.0) - s).abs() < 1e-3
                    && (r["ctm"][2].as_f64().unwrap_or(0.0) + s).abs() < 1e-3
            })
            .map(text)
            .collect()
    };
    ok(rot(30.0, "cmr10.pfb") == "Rotated", "rotated Type 1 text");
    ok(rot(-45.0, ".otf") == "native", "rotated native text");
    // right to left: the glyphs in visual order, each its characters;
    // reversed, the logical text (Hebrew; Amiri's contextual forms have
    // no Unicode in its ToUnicode CMap, as in the PDF, without
    // `\XeTeXgenerateactualtext`)
    let rtl = |f: &str| -> String {
        let t: Vec<String> = runs
            .iter()
            .filter(|r| file(r).contains(f) && r["actual_text"] != true)
            .map(text)
            .collect();
        t.into_iter().rev().collect()
    };
    ok(
        rtl("Libertinus") == "שלוםעולם",
        &format!("Hebrew: {:?}", rtl("Libertinus")),
    );
    ok(
        runs.iter()
            .any(|r| file(r).contains("Amiri") && r["actual_text"] == true && text(r) == "مرحبا"),
        "Arabic with ActualText",
    );
    Ok(bad)
}

/// `cargo xtask glyphruns DIR JOB`: [`check`], each problem printed (and
/// [`expect_xelatex_runs`]'s, for the job `xelatex-runs`).
pub fn run(args: &[String]) -> Result<()> {
    let (Some(dir), Some(job)) = (args.first(), args.get(1)) else {
        anyhow::bail!("usage: cargo xtask glyphruns DIR JOB");
    };
    let mut bad = check(Path::new(dir), job)?;
    if job == "xelatex-runs" {
        bad.extend(expect_xelatex_runs(Path::new(dir))?);
    }
    for b in &bad {
        println!("{b}");
    }
    anyhow::ensure!(bad.is_empty(), "{} problems", bad.len());
    println!("glyph runs: ok");
    Ok(())
}
