//! Shared helpers. Every call into `phitex_red::api` goes through here so
//! an API change is one edit.
#![allow(dead_code)]
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use phitex_red::api::{Build, Edit, Engine, Finder, Format, Options, Output};

/// IniTeX has no grouping/parameter catcodes; unit tests start with these.
pub const PRE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \
\\catcode`\\&=4 \\catcode`\\^=7 \\catcode`\\_=8 \\catcode`\\~=13\n";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// TeX Live's kpathsea, found from the `tex` on `PATH` (fonts, maps, …).
fn kpse() -> &'static Mutex<phitex_kpse::Kpse> {
    static K: OnceLock<Mutex<phitex_kpse::Kpse>> = OnceLock::new();
    K.get_or_init(|| {
        let dir = std::env::var_os("PATH")
            .and_then(|p| std::env::split_paths(&p).find(|d| d.join("tex").is_file()))
            .unwrap_or_else(|| PathBuf::from("/usr/bin"));
        Mutex::new(phitex_kpse::Kpse::new(&dir, "tex", "pdftex"))
    })
}

fn kpse_find(name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
    use phitex_kpse::Format as F;
    let fmt = match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("tfm") => F::Tfm,
        Some("vf") => F::Vf,
        Some("map") => F::FontMap,
        Some("enc") => F::Enc,
        Some("pfb" | "pfa") => F::Type1,
        Some("ttf") => F::TrueType,
        Some("otf") => F::OpenType,
        Some("bib") => F::Bib,
        Some("bst") => F::Bst,
        Some("ist") => F::Ist,
        _ => F::Tex,
    };
    let p = kpse().lock().unwrap().find_file(name.as_bytes(), fmt, false)?;
    let p = String::from_utf8_lossy(&p).into_owned();
    let bytes = std::fs::read(&p).ok()?;
    Some((Arc::from(p.as_str()), Arc::from(bytes)))
}

/// In-memory files first, then TeX Live.
pub fn finder(files: &[(&str, &str)]) -> Finder {
    let m: HashMap<String, Arc<[u8]>> = files.iter().map(|(n, t)| ((*n).to_string(), Arc::from(t.as_bytes()))).collect();
    Arc::new(move |name: &str| match m.get(name) {
        Some(b) => Some((Arc::from(name), b.clone())),
        None => kpse_find(name),
    })
}

pub fn opts(etex: bool, pdf: bool) -> Options {
    let engine = if pdf {
        Engine::Pdftex
    } else if etex {
        Engine::Etex
    } else {
        Engine::Tex
    };
    Options {
        engine,
        format: Format::Ini,
        job_name: Arc::from("job"),
        main_name: Arc::from("job.tex"),
        finder: finder(&[]),
        threads: 1,
        givens: BTreeMap::new(),
        previous: BTreeMap::new(),
    }
}

/// A build over an in-memory main text (plus extra files).
pub fn build_with(o: Options, main: &str, extra: &[(&str, &str)]) -> (Build, Output) {
    let o = Options { finder: finder(extra), ..o };
    let mut b = Build::new(o, Arc::from(main)).unwrap_or_else(|e| panic!("Build::new: {e}"));
    let out = b.update(Vec::new());
    (b, out)
}

/// Replaces `start..end` of the main text with `text`.
pub fn splice(b: &Build, start: usize, end: usize, text: &str) -> Edit {
    let old = b.text();
    let data = format!("{}{}{}", &old[..start], text, &old[end..]);
    Edit::MainSplice { start, old_len: end - start, inserted: text.len(), data: Arc::from(data) }
}

pub fn run(src: &str) -> Output {
    build_with(opts(false, false), &format!("{PRE}{src}\n\\end\n"), &[]).1
}

pub fn run_etex(src: &str) -> Output {
    build_with(opts(true, false), &format!("{PRE}{src}\n\\end\n"), &[]).1
}

/// The texts of every `\errmessage` (and other diagnostic) in order.
pub fn msgs(o: &Output) -> Vec<String> {
    o.diags.iter().map(|d| d.msg.clone()).collect()
}

/// `\errmessage{<expansion of src>}`: what it showed.
pub fn shows(body: &str) -> String {
    let o = run(body);
    msgs(&o).join("|")
}

/// Runs `src` whose test lines each say `\errmessage{...}`; returns them.
pub fn errs(src: &str) -> Vec<String> {
    msgs(&run(src))
}

/// The DVI, if the build made one.
pub fn dvi(o: &Output) -> Option<&[u8]> {
    (!o.dvi.is_empty()).then_some(&o.dvi[..])
}

/// The PDF, if the build made one.
pub fn pdf(o: &Output) -> Option<&[u8]> {
    (!o.pdf.is_empty()).then_some(&o.pdf[..])
}

/// DVI minus the preamble comment (holds a date).
pub fn dvi_body(d: &[u8]) -> Vec<u8> {
    assert!(d.len() > 15 && d[0] == 247, "not a DVI file");
    let k = d[14] as usize;
    let mut v = d[..14].to_vec();
    v.extend_from_slice(&d[15 + k..]);
    v
}

/// Count of `/Type /Page` objects (not `/Pages`).
pub fn pdf_pages(p: &[u8]) -> usize {
    let s = String::from_utf8_lossy(p);
    let mut n = 0;
    for pat in ["/Type /Page", "/Type/Page"] {
        let mut i = 0;
        while let Some(j) = s[i..].find(pat) {
            let at = i + j + pat.len();
            if !s[at..].starts_with('s') {
                n += 1;
            }
            i = at;
        }
    }
    n
}

pub fn fixture(dir: &str, name: &str) -> PathBuf {
    root().join("crates/phitex-flow/tests").join(dir).join(name)
}

pub fn read(p: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).into_owned()
}
