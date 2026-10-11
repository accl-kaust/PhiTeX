//! Shared helpers. Every call into `phitex_red::api` goes through here so
//! an API change is one edit.
#![allow(dead_code)]
use std::path::{Path, PathBuf};

pub use phitex_red::api::{Build, Change, Options, Output};

/// IniTeX has no grouping/parameter catcodes; unit tests start with these.
pub const PRE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6 \\catcode`\\$=3 \
\\catcode`\\&=4 \\catcode`\\^=7 \\catcode`\\_=8 \\catcode`\\~=13\n";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn opts(etex: bool, pdf: bool) -> Options {
    Options { main: PathBuf::from("job.tex"), etex, pdf, ..Options::default() }
}

/// A build over an in-memory `job.tex` (plus extra files).
pub fn build_with(o: Options, main: &str, extra: &[(&str, &str)]) -> (Build, Output) {
    let mut b = Build::new(o);
    let mut ch = vec![Change::File { path: PathBuf::from("job.tex"), text: main.to_string() }];
    for (p, t) in extra {
        ch.push(Change::File { path: PathBuf::from(p), text: (*t).to_string() });
    }
    let out = b.update(ch);
    (b, out)
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
    let o = run(&format!("{body}"));
    msgs(&o).join("|")
}

/// Runs `src` whose test lines each say `\errmessage{...}`; returns them.
pub fn errs(src: &str) -> Vec<String> {
    msgs(&run(src))
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
