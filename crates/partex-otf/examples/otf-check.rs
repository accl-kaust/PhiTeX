//! Checks partex-otf against the oracle's XeTeX (DESIGN 4.7).
//!
//!     scripts/sandbox cargo run -p partex-otf --release --example otf-check -- \
//!         INDEX OUTDIR [metrics|shape|names] [--from N] [--count N] [--jobs N]
//!
//! For faces of the index (`otf-index`), it writes plain XeTeX files that
//! print into the log what XeTeX's font functions answer (fontdimens,
//! `\XeTeXglyphbounds`, `\XeTeXglyphname`, `\XeTeXcharglyph`,
//! `\fontcharwd`/`ht`/`dp`/`ic`, the OpenType script, language and
//! feature queries, first/last character, glyph count) and that typeset
//! words (one page per face and text, whose XDV `set_glyphs` hold the
//! shaped glyphs), runs `scripts/xetex/oracle.sh --no-pdf`, and compares
//! every number with what this crate computes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use partex_otf::index::{FaceEntry, FontIndex, LOADABLE, WOFF};
use partex_otf::shape::Shaper;
use partex_otf::xetex::{self, XeTeXFont, fontmgr::FontManager};
use partex_otf::{FontSource, KpseFormat};

struct Src;

impl FontSource for Src {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        std::fs::read(path).ok().map(Arc::from)
    }
    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        if name.starts_with('/') {
            return Path::new(name).exists().then(|| name.to_string());
        }
        let fmt = match format {
            KpseFormat::OpenType => "opentype fonts",
            KpseFormat::TrueType => "truetype fonts",
            KpseFormat::Type1 => "type1 fonts",
            KpseFormat::MiscFonts => "misc fonts",
        };
        let out = Command::new("kpsewhich")
            .arg(format!("-format={fmt}"))
            .arg(name)
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs the oracle on `tex` in `dir`; returns (log, xdv).
fn oracle(tex: &Path, dir: &Path) -> (String, Vec<u8>) {
    let script = root().join("scripts/xetex/oracle.sh");
    let status = Command::new(script)
        .env("max_print_line", "1000000")
        .env("error_line", "254")
        .env("half_error_line", "238")
        .arg("--no-pdf")
        .arg(tex)
        .arg(dir)
        .status()
        .expect("oracle runs");
    let _ = status;
    let stem = tex.file_stem().unwrap().to_string_lossy().to_string();
    let log = std::fs::read(dir.join(format!("{stem}.log"))).unwrap_or_default();
    let xdv = std::fs::read(dir.join(format!("{stem}.xdv"))).unwrap_or_default();
    (String::from_utf8_lossy(&log).into_owned(), xdv)
}

/// The log's lines that start with `#` (ours), as (key, values).
fn tagged(log: &str) -> Vec<Vec<String>> {
    log.lines()
        .filter(|l| l.starts_with("@@"))
        .map(|l| {
            l[2..]
                .split([' ', ':'])
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .collect()
}

/// A page of an XDV: the `set_glyphs` it holds (width, [(x, y, gid)]).
type Page = Vec<(i32, Vec<(i32, i32, u16)>)>;

fn xdv_pages(d: &[u8]) -> Vec<Page> {
    let mut pages = Vec::new();
    let mut cur: Option<Page> = None;
    let mut p = 0usize;
    let u = |p: usize, n: usize| -> i64 {
        let mut v: i64 = 0;
        for i in 0..n {
            v = (v << 8) | i64::from(d[p + i]);
        }
        v
    };
    let s = |p: usize, n: usize| -> i64 {
        let v = u(p, n);
        let bits = 8 * n as u32;
        if v >= 1 << (bits - 1) {
            v - (1 << bits)
        } else {
            v
        }
    };
    while p < d.len() {
        let op = d[p];
        p += 1;
        match op {
            0..=127 => {}
            128..=131 => p += (op - 127) as usize,
            132 | 137 => p += 8,
            133..=136 => p += (op - 132) as usize,
            138 => {}
            139 => {
                p += 44;
                cur = Some(Vec::new());
            }
            140 => pages.extend(cur.take()),
            141 | 142 => {}
            143..=146 => p += (op - 142) as usize,
            147 | 152 | 161 | 166 => {}
            148..=151 => p += (op - 147) as usize,
            153..=156 => p += (op - 152) as usize,
            157..=160 => p += (op - 156) as usize,
            162..=165 => p += (op - 161) as usize,
            167..=170 => p += (op - 166) as usize,
            171..=234 => {}
            235..=238 => p += (op - 234) as usize,
            239..=242 => {
                let n = (op - 238) as usize;
                let k = u(p, n) as usize;
                p += n + k;
            }
            243..=246 => {
                let n = (op - 242) as usize;
                p += n + 12;
                let a = d[p] as usize;
                let l = d[p + 1] as usize;
                p += 2 + a + l;
            }
            247 => {
                p += 14;
                let k = d[p - 1] as usize;
                p += k;
            }
            248 => break,
            252 => {
                p += 4 + 4;
                let flags = u(p, 2) as u16;
                p += 2;
                let l = d[p] as usize;
                p += 1 + l + 4;
                if flags & 0x0200 != 0 {
                    p += 4;
                }
                for bit in [0x1000, 0x2000, 0x4000] {
                    if flags & bit != 0 {
                        p += 4;
                    }
                }
            }
            253 | 254 => {
                if op == 254 {
                    let l = u(p, 2) as usize;
                    p += 2 + 2 * l;
                }
                let w = s(p, 4) as i32;
                let n = u(p + 4, 2) as usize;
                p += 6;
                let mut g = Vec::with_capacity(n);
                for i in 0..n {
                    g.push((s(p + 8 * i, 4) as i32, s(p + 8 * i + 4, 4) as i32, 0u16));
                }
                p += 8 * n;
                for (i, item) in g.iter_mut().enumerate() {
                    item.2 = u(p + 2 * i, 2) as u16;
                }
                p += 2 * n;
                if let Some(c) = cur.as_mut() {
                    c.push((w, g));
                }
            }
            _ => panic!("XDV op {op} at {}", p - 1),
        }
    }
    pages
}

/// The faces to check: loadable sfnt faces this crate can open.
fn faces(index: &FontIndex) -> Vec<(usize, FaceEntry)> {
    index
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.flags & LOADABLE != 0 && e.flags & WOFF == 0 && e.index >> 16 == 0)
        .map(|(i, e)| (i, e.clone()))
        .collect()
}

fn spec(e: &FaceEntry) -> String {
    format!("[{}:{}]", e.path, e.index)
}

fn load(e: &FaceEntry, size: i32, features: &str) -> Option<XeTeXFont> {
    let mut mgr = FontManager::new(Arc::new(FontIndex::default()));
    let name = format!("{}{features}", spec(e));
    xetex::find_native_font(&mut mgr, &Src, &name, size, 0).0
}

/// The glyphs sampled from a face: the first 48, then spread to ~160.
fn sample_glyphs(n: u32) -> Vec<u32> {
    let mut v: Vec<u32> = (0..n.min(48)).collect();
    if n > 48 {
        let step = ((n - 48) / 112).max(1);
        let mut g = 48;
        while g < n {
            v.push(g);
            g += step;
        }
        v.push(n - 1);
        v.push(n);
    }
    v.dedup();
    v
}

/// The characters sampled: ASCII, Latin-1, and up to 64 more the font maps.
fn sample_chars(f: &XeTeXFont) -> Vec<u32> {
    let mut v: Vec<u32> = (32..127).chain(160..256).collect();
    let mut extra = Vec::new();
    for c in 256..0x30000u32 {
        if (0xD800..0xE000).contains(&c) {
            continue;
        }
        if f.face.char_index(c) != 0 {
            extra.push(c);
        }
    }
    let step = (extra.len() / 64).max(1);
    v.extend(extra.iter().step_by(step).take(64));
    v
}

const PREAMBLE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\^=7\n\\def\\l#1{\\immediate\\write-1{@@#1}}\n\\hsize=100in \\vsize=100in \\parindent=0pt \\XeTeXtracingfonts=0\n";

fn metrics_tex(chunk: &[(usize, FaceEntry)]) -> (String, Vec<Option<XeTeXFont>>) {
    let mut t = String::from(PREAMBLE);
    let mut fonts = Vec::new();
    for (k, (_, e)) in chunk.iter().enumerate() {
        let f = load(e, 655_360, "");
        let _ = writeln!(t, "\\font\\x=\"{}\" at 10pt \\x", spec(e));
        let _ = writeln!(t, "\\l{{F {k} \\fontname\\x}}");
        let mut p = format!("\\l{{P {k}");
        let nparams = if f.as_ref().is_some_and(XeTeXFont::is_math_font) {
            65
        } else {
            8
        };
        for i in 1..=nparams {
            let _ = write!(p, " \\number\\fontdimen{i}\\x:");
        }
        p.push('}');
        let _ = writeln!(t, "{p}");
        let _ = writeln!(
            t,
            "\\l{{R {k} \\number\\XeTeXfirstfontchar\\x:\\number\\XeTeXlastfontchar\\x:\\number\\XeTeXcountglyphs\\x:\\number\\XeTeXOTcountscripts\\x:}}"
        );
        if let Some(f) = &f {
            for g in sample_glyphs(f.face.num_glyphs()) {
                let _ = writeln!(
                    t,
                    "\\l{{G {k} {g}:\\number\\XeTeXglyphbounds1 {g}:\\number\\XeTeXglyphbounds2 {g}:\\number\\XeTeXglyphbounds3 {g}:\\number\\XeTeXglyphbounds4 {g}:[\\XeTeXglyphname\\x {g}]}}"
                );
            }
            for c in sample_chars(f) {
                let _ = writeln!(
                    t,
                    "\\l{{C {k} {c}:\\number\\XeTeXcharglyph{c}:\\number\\fontcharwd\\x{c}:\\number\\fontcharht\\x{c}:\\number\\fontchardp\\x{c}:\\number\\fontcharic\\x{c}:}}"
                );
            }
            let scripts = scripts_of(f);
            for (i, &s) in scripts.iter().enumerate() {
                let _ = writeln!(
                    t,
                    "\\l{{S {k} {i}:\\number\\XeTeXOTscripttag\\x {i}:\\number\\XeTeXOTcountlanguages\\x {s}:}}"
                );
                let nl = f.ot_font_get1(xetex::what::OT_COUNT_LANGUAGES, s as i32);
                let mut langs = vec![0u32];
                for j in 0..nl {
                    let _ = writeln!(
                        t,
                        "\\l{{L {k} {s} {j} \\number\\XeTeXOTlanguagetag\\x {s} {j}:}}"
                    );
                    langs.push(f.ot_font_get2(xetex::what::OT_LANGUAGE_CODE, s as i32, j) as u32);
                }
                for l in langs {
                    let _ = writeln!(
                        t,
                        "\\l{{N {k} {s} {l} \\number\\XeTeXOTcountfeatures\\x {s} {l}:}}"
                    );
                    let nf = f.ot_font_get2(xetex::what::OT_COUNT_FEATURES, s as i32, l as i32);
                    for m in 0..nf {
                        let _ = writeln!(
                            t,
                            "\\l{{T {k} {s} {l} {m} \\number\\XeTeXOTfeaturetag\\x {s} {l} {m}:}}"
                        );
                    }
                }
            }
        }
        fonts.push(f);
    }
    t.push_str("\\end\n");
    (t, fonts)
}

fn scripts_of(f: &XeTeXFont) -> Vec<u32> {
    let n = f.ot_font_get(xetex::what::OT_COUNT_SCRIPTS);
    (0..n)
        .map(|i| f.ot_font_get1(xetex::what::OT_SCRIPT_CODE, i) as u32)
        .collect()
}

/// What this crate answers for a log line.
fn expect(f: &XeTeXFont, line: &[String], shaper: &mut Shaper) -> Option<String> {
    let num = |i: usize| -> i32 { line[i].parse().unwrap_or(0) };
    let size = 655_360;
    Some(match line[0].as_str() {
        "P" => {
            let space = space_width(f, shaper);
            let p = f.native_params(space, size);
            let n = line.len() - 2;
            p.params
                .iter()
                .take(n)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        }
        "R" => format!(
            "{} {} {} {}",
            f.font_char_range(true),
            f.font_char_range(false),
            f.ot_font_get(xetex::what::COUNT_GLYPHS),
            f.ot_font_get(xetex::what::OT_COUNT_SCRIPTS)
        ),
        "G" => {
            let g = num(2) as u32;
            format!(
                "{} {} {} {} [{}]",
                f.glyph_bounds_edge(1, g),
                f.glyph_bounds_edge(2, g),
                f.glyph_bounds_edge(3, g),
                f.glyph_bounds_edge(4, g),
                xetex_chars(&f.glyph_name(g))
            )
        }
        "C" => {
            let c = num(2);
            let p = f.native_params(space_width(f, shaper), size);
            let (ht, dp) = f.char_height_depth(c, p.params[5], p.params[4], p.params[7]);
            format!(
                "{} {} {} {} {}",
                f.map_char_to_glyph(c),
                f.char_wd(c),
                ht,
                dp,
                f.char_ic(c, f.letter_space)
            )
        }
        "S" => {
            let i = num(2);
            let s = f.ot_font_get1(xetex::what::OT_SCRIPT_CODE, i);
            format!(
                "{} {}",
                s,
                f.ot_font_get1(xetex::what::OT_COUNT_LANGUAGES, s)
            )
        }
        "L" => format!(
            "{}",
            f.ot_font_get2(xetex::what::OT_LANGUAGE_CODE, num(2), num(3))
        ),
        "N" => format!(
            "{}",
            f.ot_font_get2(xetex::what::OT_COUNT_FEATURES, num(2), num(3))
        ),
        "T" => format!(
            "{}",
            f.ot_font_get3(xetex::what::OT_FEATURE_CODE, num(2), num(3), num(4))
        ),
        _ => return None,
    })
}

/// Bytes as XeTeX's `printchar` of a C `char` shows them in the log.
fn xetex_chars(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if c < 0x80 {
                char::from(c)
            } else {
                char::from_u32((i32::from(c as i8) & 0xFFFF) as u32).unwrap_or('?')
            }
        })
        .collect()
}

fn space_width(f: &XeTeXFont, shaper: &mut Shaper) -> i32 {
    let (l, _) = f.layout(shaper, &[0x20], false, 0);
    l.width
}

/// The number of values (after the tag and the face number) each line kind
/// has before what is compared.
fn keylen(kind: &str) -> usize {
    match kind {
        "P" | "R" => 2,
        "G" | "C" | "S" => 3,
        "L" | "N" => 4,
        "T" => 5,
        _ => 2,
    }
}

#[derive(Default)]
struct Tally {
    checked: BTreeMap<String, usize>,
    bad: BTreeMap<String, usize>,
    faces_bad: BTreeMap<String, usize>,
    examples: Vec<String>,
}

fn check_metrics(chunk: &[(usize, FaceEntry)], dir: &Path, n: usize) -> Tally {
    let (tex, fonts) = metrics_tex(chunk);
    let path = dir.join(format!("m{n}.tex"));
    std::fs::write(&path, tex).unwrap();
    let (log, _) = oracle(&path, dir);
    let mut tally = Tally::default();
    let mut shaper = Shaper::new();
    let mut bad_faces = std::collections::BTreeSet::new();
    for line in tagged(&log) {
        if line.len() < 2 {
            continue;
        }
        let kind = line[0].clone();
        if kind == "F" {
            continue;
        }
        let k: usize = line[1].parse().unwrap_or(usize::MAX);
        let Some(Some(f)) = fonts.get(k) else {
            *tally.bad.entry(format!("{kind}-unloaded")).or_default() += 1;
            continue;
        };
        let Some(want) = expect(f, &line, &mut shaper) else {
            continue;
        };
        let got = line[keylen(&kind)..].join(" ");
        *tally.checked.entry(kind.clone()).or_default() += 1;
        if got != want {
            *tally.bad.entry(kind.clone()).or_default() += 1;
            bad_faces.insert((kind.clone(), k));
            if tally.examples.len() < 30 {
                tally.examples.push(format!(
                    "{} {}: oracle [{got}] mine [{want}]",
                    chunk[k].1.path,
                    line.join(" ")
                ));
            }
        }
    }
    for (kind, _) in bad_faces {
        *tally.faces_bad.entry(kind).or_default() += 1;
    }
    tally
}

/// Texts to shape, each used with a face that maps all its characters.
const TEXTS: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "``Quote'' -- dash --- 'single' !` ?` <<guill>> ,,low",
    "AVAST Wa To Ty fi ffl ffi office affluent “quoted” — 1234567890",
    "Æsthetic naïve façade Œuvre Straße ½ ¾ © ® ™ § ¶",
    "Ελληνικά: Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.",
    "Съешь же ещё этих мягких французских булок, да выпей чаю.",
    "עברית: דג סקרן שט בים מאוכזב ולפתע מצא חברה.",
    "العربية: صِف خَلقَ خَودِ كَمِثلِ الشَمسِ إِذ بَزَغَت",
    "हिन्दी: ऋषियों को सताने वाले दुष्ट राक्षसों के राजा रावण का सर्वनाश करने वाले",
    "Mixed עברית and English 123",
    "x² + y² = z² ∑∫∂ αβγ ≤ ≥ ≠ ∞",
    "abcעבריתdef(12) x(שלום)y",
    "العربية (12) [3] 2024 سنة2024",
    "שלום (12) a-b [x]",
];

fn shape_tex(chunk: &[(usize, FaceEntry)]) -> (String, Vec<(usize, String, String)>) {
    let mut t = String::from(PREAMBLE);
    let mut pages = Vec::new();
    for (k, (_, e)) in chunk.iter().enumerate() {
        let Some(f) = load(e, 655_360, "") else {
            continue;
        };
        for (ti, &text) in TEXTS.iter().enumerate() {
            let covered = text
                .chars()
                .all(|c| c == ' ' || f.face.char_index(u32::from(c)) != 0);
            if !covered {
                continue;
            }
            for feat in [
                "",
                ":mapping=tex-text",
                ":+smcp;-liga",
                ":letterspace=5;extend=1.2;slant=0.1",
            ] {
                if ti > 4 && !feat.is_empty() {
                    continue;
                }
                let _ = writeln!(
                    t,
                    "\\font\\x=\"{}{feat}\" at 10pt \\shipout\\hbox{{\\x {text}}}",
                    spec(e)
                );
                pages.push((k, String::from(feat), String::from(text)));
            }
        }
    }
    t.push_str("\\end\n");
    (t, pages)
}

fn check_shape(chunk: &[(usize, FaceEntry)], dir: &Path, n: usize) -> Tally {
    let (tex, pages) = shape_tex(chunk);
    let path = dir.join(format!("s{n}.tex"));
    std::fs::write(&path, tex).unwrap();
    let (_, xdv) = oracle(&path, dir);
    let got = xdv_pages(&xdv);
    let mut tally = Tally::default();
    let mut shaper = Shaper::new();
    if got.len() != pages.len() {
        tally.examples.push(format!(
            "chunk {n}: {} pages, expected {}",
            got.len(),
            pages.len()
        ));
        *tally.bad.entry("pages".into()).or_default() += 1;
    }
    let mut bad_faces = std::collections::BTreeSet::new();
    for (page, (k, feat, text)) in got.iter().zip(&pages) {
        let e = &chunk[*k].1;
        let Some(mut f) = load(e, 655_360, feat) else {
            continue;
        };
        let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
        let mut mine = Vec::new();
        for w in &words {
            let u: Vec<u16> = w.encode_utf16().collect();
            let u = f.apply_mapping(&u);
            let ls = f.letter_space;
            let l = f.layout_word(&mut shaper, &u, ls);
            mine.push((
                l.width,
                l.glyphs
                    .iter()
                    .map(|g| (g.x, g.y, g.gid))
                    .collect::<Vec<_>>(),
            ));
        }
        *tally.checked.entry("word".into()).or_default() += page.len();
        let kind = if feat.is_empty() { "word" } else { "word-feat" };
        if std::env::var("OTF_DEBUG").is_ok() && text.starts_with("Mixed") {
            eprintln!(
                "{} {text}\n  oracle {:?}\n  mine   {:?}",
                e.path, page, mine
            );
        }
        if *page != mine {
            *tally.bad.entry(kind.into()).or_default() += 1;
            bad_faces.insert(*k);
            if tally.examples.len() < 20 {
                let first = page
                    .iter()
                    .zip(&mine)
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                tally.examples.push(format!(
                    "{}{feat} \"{text}\" word {first}: oracle {:?} mine {:?}",
                    e.path,
                    page.get(first),
                    mine.get(first)
                ));
            }
        }
    }
    tally.faces_bad.insert("shape".into(), bad_faces.len());
    tally
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = FontIndex::from_bytes(&std::fs::read(&args[1]).expect("index")).expect("an index");
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let mode = args.get(3).map_or("metrics", String::as_str);
    let opt = |name: &str, d: usize| -> usize {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let all = faces(&index);
    let from = opt("--from", 0).min(all.len());
    let count = opt("--count", all.len()).min(all.len() - from);
    let jobs = opt("--jobs", 8);
    let per = opt("--per", 60);
    let sel: Vec<(usize, FaceEntry)> = all[from..from + count].to_vec();
    let chunks: Vec<Vec<(usize, FaceEntry)>> = sel.chunks(per).map(<[_]>::to_vec).collect();
    let results = std::sync::Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(chunk) = chunks.get(i) else { break };
                    let dir = out.join(format!("{mode}{i}"));
                    std::fs::create_dir_all(&dir).unwrap();
                    let t = match mode {
                        "shape" => check_shape(chunk, &dir, i),
                        _ => check_metrics(chunk, &dir, i),
                    };
                    results.lock().unwrap().push(t);
                }
            });
        }
    });
    let mut total = Tally::default();
    for t in results.into_inner().unwrap() {
        for (k, v) in t.checked {
            *total.checked.entry(k).or_default() += v;
        }
        for (k, v) in t.bad {
            *total.bad.entry(k).or_default() += v;
        }
        for (k, v) in t.faces_bad {
            *total.faces_bad.entry(k).or_default() += v;
        }
        total.examples.extend(t.examples);
    }
    for e in total.examples.iter().take(60) {
        println!("{e}");
    }
    println!("faces: {count} (from {from})");
    println!("checked: {:?}", total.checked);
    println!("mismatches: {:?}", total.bad);
    println!("faces with mismatches: {:?}", total.faces_bad);
}
