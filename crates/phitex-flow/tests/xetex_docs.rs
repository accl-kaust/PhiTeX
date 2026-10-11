//! XeTeX on the graph (CORE.md, "XeTeX on the graph"), against TeX Live's
//! `xetex` and `xelatex` (`SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1`, batch
//! mode), at every schedule:
//!
//! - the IniTeX documents of `tests/xetex/ini/` (`xetex -ini -etex`), a
//!   family of XeTeX's primitives each: the XDV file, its preamble's
//!   dated comment masked, and the PDF xdvipdfmx makes of it;
//! - the ten plain documents of `tests/plain.rs` (plain.tex with
//!   hyphen.tex, `xetex -ini -etex`): the XDV file;
//! - LaTeX documents on the `xelatex` format (`xelatex.ini`'s job, saved
//!   by `Build::format`): `tests/xetex/docs/` (fontspec, polyglossia's
//!   Arabic and Greek, unicode-math with Latin Modern Math, Japanese and
//!   Chinese with `\XeTeXlinebreaklocale`, pictures) and the ten of
//!   `tests/latex/docs/`, on their first run: the XDV file and the PDF.
//!
//! The oracles are `tests/xetex/oracle/` (`tests/xetex/bless.sh` makes
//! them; `PHITEX_XETEX_ORACLE=DIR` reads another blessing). The installed
//! fonts are given as phitex-otf's font index (`otf-index`, the faces
//! fontconfig lists with `scripts/xetex/fonts.conf`: TeX Live's
//! `fonts/opentype` and `fonts/truetype`), in the file
//! `PHITEX_FONT_INDEX` names; files are found by `kpsewhich`. Without an
//! index the tests say so and pass (they need TeX Live's tree).

#![allow(clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use phitex_flow::build::{CUT_LINES, MAIN_MEMORY};
use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Font, Options, Order, Output, Reads};

/// The schedules: one thread; eight suspending, in priority order and in
/// a random one; eight speculating, in priority order and in a random one.
const RUNS: [Options; 5] = [
    run(1, Reads::Suspend, Order::Priority),
    run(8, Reads::Suspend, Order::Priority),
    run(8, Reads::Speculate, Order::Priority),
    run(8, Reads::Suspend, Order::Random(1)),
    run(8, Reads::Speculate, Order::Random(7)),
];

/// The schedules a document runs at: all, or with `PHITEX_XETEX_ONE` set
/// the first alone (one thread: a hang's diagnosis).
fn runs() -> &'static [Options] {
    if std::env::var_os("PHITEX_XETEX_ONE").is_some() { &RUNS[..1] } else { &RUNS }
}

const fn run(threads: usize, reads: Reads, order: Order) -> Options {
    Options { threads, reads, order, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn oracle_dir() -> PathBuf {
    std::env::var_os("PHITEX_XETEX_ORACLE").map_or_else(|| root().join("tests/xetex/oracle"), PathBuf::from)
}

fn font() -> Arc<Font> {
    let b = std::fs::read(root().join("testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// Bytes as TeX's characters (one each: XeTeX decodes them as it reads).
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// The font index, read once (`None`: none given, the tests pass).
static INDEX: std::sync::OnceLock<Option<Arc<Vec<u8>>>> = std::sync::OnceLock::new();

fn index() -> Option<Arc<Vec<u8>>> {
    INDEX
        .get_or_init(|| {
            let i = std::env::var_os("PHITEX_FONT_INDEX").and_then(|p| std::fs::read(p).ok()).map(Arc::new);
            if i.is_none() {
                eprintln!("xetex_docs: no PHITEX_FONT_INDEX: not run");
            }
            i
        })
        .clone()
}

/// TeX Live's kpathsea as program `xetex` (`SELFAUTOLOC`: the directory
/// of `kpsewhich` on `PATH`): the native fonts' files, the pictures',
/// xdvipdfmx's.
fn kpathsea() -> Arc<dyn phitex_flow::tfm::Kpse> {
    let dir = std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .filter_map(|d| d.join("kpsewhich").canonicalize().ok())
                .find(|f| f.is_file())
                .and_then(|f| f.parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    Arc::new(phitex_flow::kpse::Kpathsea::new(phitex_kpse::Kpse::new(&dir, "xetex", "xetex")))
}

/// xdvipdfmx's distiller (dvipdfmx.cfg's `D` line: `rungs` for an EPS
/// picture) as `dpx_file_apply_filter` runs it: through `sh -c`, on a copy
/// of the input in a scratch directory, removed after.
fn distiller() -> phitex_flow::native::Distiller {
    std::sync::Arc::new(|template: &[u8], input: &str, data: &[u8], version: i32| -> Option<Vec<u8>> {
        use std::os::unix::ffi::OsStrExt;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = (0..1000u32)
            .map(|k| std::env::temp_dir().join(format!("phitex-distill-{}-{stamp}-{k}", std::process::id())))
            .find(|d| std::fs::create_dir(d).is_ok())?;
        let base = input.rsplit('/').next().unwrap_or(input);
        let inp = dir.join(base);
        let out = dir.join("distilled.pdf");
        let r = std::fs::write(&inp, data).ok().and_then(|()| {
            let cmd = phitex_xdvipdfmx::io::filter_command(
                template,
                inp.as_os_str().as_bytes(),
                out.as_os_str().as_bytes(),
                version,
            );
            let ok = std::process::Command::new("sh")
                .arg("-c")
                .arg(std::ffi::OsStr::from_bytes(&cmd))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if ok { std::fs::read(&out).ok() } else { None }
        });
        let _ = std::fs::remove_dir_all(&dir);
        r
    })
}

/// A XeTeX build with the given fonts, as job `job`, reading `inputs`.
fn xetex_build(job: &str, inputs: Vec<PathBuf>) -> Option<Build> {
    let idx = index()?;
    let kpse: Arc<dyn phitex_flow::tfm::Kpse> = kpathsea();
    let fonts = phitex_flow::native::Fonts::from_index_bytes(&idx, kpse)
        .expect("PHITEX_FONT_INDEX: a font index")
        .with_distiller(distiller());
    let mut b = Build::new(font());
    b.set_xetex(fonts);
    let mut fd = vec![];
    fd.extend(phitex_flow::tfm::default_dirs());
    b.set_font_dirs(fd);
    b.set_input_dirs(inputs);
    b.set_jobname(job);
    b.set_limits(Limits::TEX_LIVE);
    b.set_pdf_date(0);
    b.set_interaction(0);
    b.set_interaction_option(Some(0));
    Some(b)
}

/// The XDV file with its preamble's comment masked (its date).
fn masked(x: &[u8]) -> Vec<u8> {
    let mut d = x.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'.';
        }
    }
    d
}

/// Where two files first differ, with the bytes around it.
fn first_difference(a: &[u8], b: &[u8]) -> String {
    let at = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let show = |x: &[u8]| format!("{:?}", &x[at.saturating_sub(24).min(x.len())..(at + 24).min(x.len())]);
    format!("at byte {at} (lengths {} and {}):\n graph: {}\n xetex: {}", a.len(), b.len(), show(a), show(b))
}

/// Where the graph's files go (`target/tmp/xetex-docs`).
fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("xetex-docs");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The XDV file is the oracle's (and, with `pdf`, the PDF file).
fn compare(what: &str, o: Options, b: &Build, out: &Output, oracle: &Path, pdf: bool) {
    let name = oracle.file_stem().unwrap().to_string_lossy().into_owned();
    let _ = std::fs::write(out_dir().join(format!("{name}.xdv")), &out.dvi);
    let want = std::fs::read(oracle).unwrap_or_else(|_| panic!("no oracle {}", oracle.display()));
    assert!(
        masked(&out.dvi) == masked(&want),
        "{what} ({o:?}): the XDV file is not xetex's {}",
        first_difference(&masked(&out.dvi), &masked(&want))
    );
    if !pdf {
        return;
    }
    let p = oracle.with_extension("pdf");
    let Ok(want) = std::fs::read(&p) else { return };
    let got = b.xdv_pdf(&out.dvi).expect("XeTeX").unwrap_or_else(|e| panic!("{what}: xdvipdfmx: {e}"));
    let _ = std::fs::write(out_dir().join(format!("{name}.pdf")), &got);
    assert!(got == want, "{what} ({o:?}): the PDF file is not xdvipdfmx's {}", first_difference(&got, &want));
}

// ------------------------------------------------------------ IniTeX documents

fn ini(name: &str) {
    let dir = root().join("tests/xetex/ini");
    let text = chars(&std::fs::read(dir.join(format!("{name}.tex"))).expect("NAME.tex"));
    let oracle = oracle_dir().join(format!("ini/{name}.xdv"));
    for (k, &o) in runs().iter().enumerate() {
        let Some(mut b) = xetex_build(name, vec![dir.clone()]) else { return };
        let out = b.update(&text, o);
        compare(name, o, &b, &out, &oracle, k == 0);
    }
}

#[test]
fn ini_tfm() {
    ini("tfm");
}

#[test]
fn ini_chars() {
    ini("chars");
}

#[test]
fn ini_native() {
    ini("native");
}

#[test]
fn ini_features() {
    ini("features");
}

#[test]
fn ini_interchar() {
    ini("interchar");
}

#[test]
fn ini_linebreak() {
    ini("linebreak");
}

#[test]
fn ini_math() {
    ini("math");
}

#[test]
fn ini_pics() {
    ini("pics");
}

#[test]
fn ini_bidi() {
    ini("bidi");
}

#[test]
fn ini_encoding() {
    ini("encoding");
}

// ------------------------------------------------------------ plain TeX

/// plain.tex: `tests/dvi/plaintext.tex` before its body, with
/// `hyphen.tex` (as `tests/plain.rs` and `tests/xetex/bless.sh`).
fn plain_doc(name: &str) -> String {
    let s = chars(&std::fs::read(root().join("tests/dvi/plaintext.tex")).unwrap());
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = chars(&std::fs::read(root().join("tests/hyph/hyphen.tex")).unwrap());
    let plain = s[..at].replace("% (\\input hyphen: no hyphenation patterns on the graph yet)\n", &h);
    let (body, default) = match name.strip_suffix("-default") {
        Some(b) => (b, true),
        None => (name, false),
    };
    let b = chars(&std::fs::read(root().join(format!("tests/plain/{body}.tex"))).unwrap());
    plain + if default { "\\output={}\n" } else { "" } + &b
}

fn plain(name: &str) {
    let text = plain_doc(name);
    let oracle = oracle_dir().join(format!("plain/{name}.xdv"));
    for &o in runs() {
        let Some(mut b) = xetex_build(name, vec![root().join("tests/plain")]) else { return };
        let out = b.update(&text, o);
        compare(name, o, &b, &out, &oracle, false);
    }
}

#[test]
fn plain_story() {
    plain("story");
}

#[test]
fn plain_story_default() {
    plain("story-default");
}

#[test]
fn plain_pages() {
    plain("pages");
}

#[test]
fn plain_pages_default() {
    plain("pages-default");
}

#[test]
fn plain_display() {
    plain("display");
}

#[test]
fn plain_display_default() {
    plain("display-default");
}

#[test]
fn plain_table() {
    plain("table");
}

#[test]
fn plain_table_default() {
    plain("table-default");
}

#[test]
fn plain_hyphens() {
    plain("hyphens");
}

#[test]
fn plain_hyphens_default() {
    plain("hyphens-default");
}

// ------------------------------------------------------------ xelatex

/// The directories the oracle read from (its `.fls`), in order.
fn fls_dirs(fls: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for l in std::fs::read_to_string(fls).unwrap_or_default().lines() {
        if let Some(p) = l.strip_prefix("INPUT /")
            && let Some(d) = Path::new(&format!("/{p}")).parent()
            && !v.iter().any(|x| x == d)
        {
            v.push(d.to_path_buf());
        }
    }
    v
}

/// One heavy build at a time.
static ONE: Mutex<()> = Mutex::new(());

/// The `xelatex` format, made once: `xelatex.ini`'s job as job `xelatex`,
/// its definitions at the `\dump` (`Build::format`).
static FORMAT: Mutex<Option<Arc<Vec<u8>>>> = Mutex::new(None);

fn format_file() -> Option<Arc<Vec<u8>>> {
    let mut f = FORMAT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(b) = &*f {
        return Some(b.clone());
    }
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // (PHITEX_XELATEX_FMT: one made by `xelatex_format` in an earlier
    // process of the same build)
    if let Some(b) = std::env::var_os("PHITEX_XELATEX_FMT").and_then(|p| std::fs::read(p).ok()) {
        let b = Arc::new(b);
        *f = Some(b.clone());
        return Some(b);
    }
    let fls = oracle_dir().join("fmt/xelatex.fls");
    let ini = std::fs::read_to_string(&fls)
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("INPUT ").filter(|p| p.ends_with("xelatex.ini")).map(PathBuf::from))?;
    let mut inputs = vec![root().join("tests/latex")];
    inputs.extend(fls_dirs(&fls));
    let mut b = xetex_build("xelatex", inputs)?;
    b.set_terminal(&["xelatex.ini"]);
    let ini = chars(&std::fs::read(ini).expect("xelatex.ini"));
    let t0 = std::time::Instant::now();
    let out = b.update(&jobs(&[&ini]), RUNS[0]);
    assert!(out.errors.is_empty(), "xelatex.ini's job: {:?}", &out.errors[..out.errors.len().min(20)]);
    let bytes = Arc::new(b.format().expect("the format"));
    eprintln!("xelatex.fmt: built in {:.1} s, {} bytes", t0.elapsed().as_secs_f64(), bytes.len());
    let _ = std::fs::write(out_dir().join("xelatex.fmt"), &*bytes);
    *f = Some(bytes.clone());
    Some(bytes)
}

/// LaTeX document `name` (in `dir`) under `xelatex`, at every schedule:
/// its XDV file and PDF are `xelatex`'s.
fn latex(dir: &Path, name: &str) {
    let Some(fmt) = format_file() else { return };
    let text = chars(&std::fs::read(dir.join(format!("{name}.tex"))).expect("NAME.tex"));
    let oracle = oracle_dir().join(format!("docs/{name}.xdv"));
    let mut inputs = vec![dir.to_path_buf(), root().join("tests/latex")];
    inputs.extend(fls_dirs(&oracle.with_extension("fls")));
    for (k, &o) in runs().iter().enumerate() {
        let Some(mut b) = xetex_build(name, inputs.clone()) else { return };
        b.set_format(&fmt).expect("the format reads");
        b.set_terminal(&[&format!("&xelatex {name}")]);
        let out = b.update(&jobs(&[&text]), o);
        // (the log, for a failure's diagnosis: not compared)
        if k == 0
            && let Some(l) = out.job_logs(&Limits::TEX_LIVE, &format!("{name}.pdf")).pop()
        {
            let _ = std::fs::write(out_dir().join(format!("{name}.log")), l.log.as_bytes());
        }
        compare(name, o, &b, &out, &oracle, k == 0);
        // (the `.aux` file, as the oracle wrote it: UTF-8)
        if let Some((_, t)) =
            out.writes.iter().find(|(n, _)| Path::new(n).extension().is_some_and(|e| e.eq_ignore_ascii_case("aux")))
            && let Ok(want) = std::fs::read(oracle.with_extension("aux"))
        {
            assert!(t.as_bytes() == &want[..], "{name} ({o:?}): the .aux file is not xelatex's");
        }
    }
}

/// The format alone (tests/xetex/job.sh runs it first, with a time limit).
#[test]
fn xelatex_format() {
    let _ = format_file();
}

fn xdoc(name: &str) {
    latex(&root().join("tests/xetex/docs"), name);
}

fn ldoc(name: &str) {
    latex(&root().join("tests/latex/docs"), name);
}

#[test]
fn xelatex_fontspec() {
    xdoc("fontspec");
}

#[test]
fn xelatex_lang() {
    xdoc("lang");
}

#[test]
fn xelatex_umath() {
    xdoc("umath");
}

#[test]
fn xelatex_cjk() {
    xdoc("cjk");
}

#[test]
fn xelatex_pics() {
    xdoc("pics");
}

#[test]
fn xelatex_sections() {
    ldoc("sections");
}

#[test]
fn xelatex_math() {
    ldoc("math");
}

#[test]
fn xelatex_tables() {
    ldoc("tables");
}

#[test]
fn xelatex_figures() {
    ldoc("figures");
}

#[test]
fn xelatex_titlepage() {
    ldoc("titlepage");
}

#[test]
fn xelatex_packages() {
    ldoc("packages");
}

#[test]
fn xelatex_ams() {
    ldoc("ams");
}

#[test]
fn xelatex_gfx() {
    ldoc("gfx");
}

#[test]
fn xelatex_layout() {
    ldoc("layout");
}

#[test]
fn xelatex_rules() {
    ldoc("rules");
}
