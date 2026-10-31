//! `phitex-wasm`: the engine as a WebAssembly program. One document is
//! built (its format first, from TeX Live's ini file) and its output and
//! log written, the way `phitex --compat=NAME` runs once:
//!
//! ```text
//! phitex-wasm [-j N] [--engine NAME | --ini] [--epoch SECONDS]
//!             [--fixpoint] [--fmt-cache FILE] [-o DIR] FILE
//! ```
//!
//! Built for `wasm32-wasip1-threads` and run by `scripts/wasm-run.mjs`
//! (Node): files come through WASI (TeX Live's tree by this crate's
//! kpathsea, `phitex-kpse`), and the engine's workers are threads the
//! host starts (`wasi::thread-spawn`, a Node worker each, sharing the
//! module's memory). The output is the native build's, byte for byte, at
//! any thread count. `scripts/wasm.sh` builds it and checks a plain TeX
//! and a LaTeX document against pdfTeX's outputs.
//!
//! It runs in the document's directory (WASI has no current directory
//! to change): `scripts/wasm-run.mjs` maps the relative paths to the
//! directory it is started in. Natively it builds and runs too (the
//! same code, on std's threads).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use phitex_kpse::{Format, Kpse};
use phitex_tex::{Build, Finder, Options, Severity, ToolFinder, ToolSearch};

/// An engine by its program name: its ini file, kpathsea's `$engine`,
/// whether it writes a PDF (TeX Live's `fmtutil.cnf` lines).
const ENGINES: [(&str, &str, &str, bool); 5] = [
    ("tex", "tex.ini", "tex", false),
    ("etex", "etex.ini", "pdftex", false),
    ("pdftex", "pdfetex.ini", "pdftex", true),
    ("latex", "latex.ini", "pdftex", false),
    ("pdflatex", "pdflatex.ini", "pdftex", true),
];

const USAGE: &str = "usage: phitex-wasm [-j N] [--engine NAME | --ini] [--epoch SECONDS] \
[--fixpoint] [--fmt-cache FILE] [-o DIR] FILE
  --engine   tex, etex, pdftex (the default), latex or pdflatex
  --ini      the file is the whole job (no format)
  --epoch    the job's date (else SOURCE_DATE_EPOCH, else now)
  --fixpoint the files the job reads back to their fixpoint (as latexmk)
  --fmt-cache FILE   the format's state: read when there, else written
  -j N       the engine's threads (default: the host's cores)";

struct Args {
    file: String,
    engine: &'static str,
    ini: bool,
    threads: Option<usize>,
    epoch: Option<i64>,
    fixpoint: bool,
    fmt_cache: Option<PathBuf>,
    out_dir: PathBuf,
}

fn usage(e: &str) -> ! {
    eprintln!("phitex-wasm: {e}\n{USAGE}");
    std::process::exit(1);
}

fn parse() -> Args {
    let mut a = Args {
        file: String::new(),
        engine: "pdftex",
        ini: false,
        threads: None,
        epoch: std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        fixpoint: false,
        fmt_cache: None,
        out_dir: PathBuf::from("."),
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .unwrap_or_else(|| usage(&format!("{name} needs a value")))
        };
        match x.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-j" | "--jobs" => {
                a.threads = Some(
                    value("-j")
                        .parse()
                        .unwrap_or_else(|_| usage("-j takes a number")),
                );
            }
            "--engine" => {
                let n = value("--engine");
                a.engine = ENGINES
                    .iter()
                    .find(|e| e.0 == n)
                    .map_or_else(|| usage(&format!("unknown engine `{n}`")), |e| e.0);
            }
            "--ini" => a.ini = true,
            "--epoch" => {
                a.epoch = Some(
                    value("--epoch")
                        .parse()
                        .unwrap_or_else(|_| usage("--epoch takes seconds")),
                );
            }
            "--fixpoint" => a.fixpoint = true,
            "--fmt-cache" => a.fmt_cache = Some(PathBuf::from(value("--fmt-cache"))),
            "-o" | "--output-dir" => a.out_dir = PathBuf::from(value("-o")),
            f if !f.starts_with('-') && a.file.is_empty() => f.clone_into(&mut a.file),
            other => usage(&format!("unexpected `{other}`")),
        }
    }
    if a.file.is_empty() {
        usage("no file given");
    }
    a
}

/// kpathsea as program `progname`: `SELFAUTOLOC` is the directory of
/// `tex` on `PATH` (a WebAssembly program has no executable of its own
/// to start from).
fn kpse(progname: &str, engine: &str) -> Kpse {
    // (by hand: WASI's std has no `split_paths`)
    let dir = std::env::var("PATH")
        .ok()
        .and_then(|p| {
            p.split(':')
                .map(PathBuf::from)
                .find(|d| d.join("tex").is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    Kpse::new(&dir, progname, engine)
}

/// The kpathsea format a file's extension names (pdfTeX's choices).
fn format_of(name: &str) -> Format {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "tfm" => Format::Tfm,
        "vf" => Format::Vf,
        "map" => Format::FontMap,
        "pfb" | "pfa" => Format::Type1,
        "enc" => Format::Enc,
        "otf" => Format::OpenType,
        "ttf" | "ttc" => Format::TrueType,
        _ => Format::Tex,
    }
}

/// The formats [`format_of`] names, which a format's own build looks up
/// without the current directory (fmtutil builds in a scratch one).
const FORMATS: [Format; 8] = [
    Format::Tfm,
    Format::Vf,
    Format::FontMap,
    Format::Type1,
    Format::Enc,
    Format::OpenType,
    Format::TrueType,
    Format::Tex,
];

fn found(k: &Mutex<Kpse>, name: &str, format: Format) -> Option<(String, Vec<u8>)> {
    let p = k.lock().unwrap_or_else(PoisonError::into_inner).find_file(
        name.as_bytes(),
        format,
        false,
    )?;
    let p = String::from_utf8(p).ok()?;
    let bytes = std::fs::read(&p).ok()?;
    Some((p, bytes))
}

fn finder(k: Kpse) -> Finder {
    let k = Mutex::new(k);
    Arc::new(move |name: &str| {
        // (a PK font, `cmr10.600pk`: kpathsea's PK search)
        if let Some((font, dpi)) = phitex_kpse::pk_name(name) {
            let (p, _) = k
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .find_pk(font.as_bytes(), dpi)?;
            let p = String::from_utf8(p).ok()?;
            let bytes = std::fs::read(&p).ok()?;
            return Some((p, bytes));
        }
        found(&k, name, format_of(name))
    })
}

fn tool_finder(k: Kpse) -> ToolFinder {
    let k = Mutex::new(k);
    Arc::new(move |search: ToolSearch, name: &str| {
        let format = match search {
            ToolSearch::BibStyle => Format::Bst,
            ToolSearch::BibData => Format::Bib,
            ToolSearch::IndexStyle => Format::Ist,
        };
        found(&k, name, format)
    })
}

/// The DVI preamble's comment for `epoch` (`" TeX output
/// YYYY.MM.DD:HHMM"`, §617).
fn dvi_comment(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // (days to a civil date: Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        " TeX output {y}.{m:02}.{d:02}:{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Bytes as TeX's characters, one each.
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// TeX's characters as bytes (past 255: `?`).
fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

#[allow(clippy::too_many_lines, reason = "one job, in order")]
fn main() {
    // (WebAssembly: std's threads are the host's workers; natively the
    // engine starts its own)
    if cfg!(target_family = "wasm") {
        phitex_tex::set_spawn_worker(Box::new(|f| {
            std::thread::Builder::new()
                .stack_size(8 << 20)
                .spawn(f)
                .expect("a worker thread");
        }));
    }
    let a = parse();
    let path = Path::new(&a.file);
    let path = if path.is_file() {
        path.to_path_buf()
    } else {
        path.with_extension("tex")
    };
    let text = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("! I can't find file `{}' ({e}).", a.file);
        std::process::exit(1);
    });
    // (the build runs in the document's directory, as TeX's does: WASI
    // has no current directory to change, so the program is started
    // there and given the file's own name)
    if path.parent().is_some_and(|d| !d.as_os_str().is_empty()) {
        usage("run phitex-wasm in the document's directory, with the file's own name");
    }
    let out_dir = a.out_dir.clone();
    let base = path
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    let jobname = base.strip_suffix(".tex").unwrap_or(&base).to_owned();
    let &(name, ini, kengine, pdf) = ENGINES
        .iter()
        .find(|e| e.0 == a.engine)
        .expect("parse checked the engine");
    let epoch = a.epoch.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(0)
    });
    let prelude = (!a.ini).then(|| format!("\\input {ini}\n"));
    let mut format_tree = kpse(name, kengine);
    for f in FORMATS {
        format_tree.drop_cwd(f);
    }
    let cached = a.fmt_cache.as_ref().and_then(|p| std::fs::read(p).ok());
    let had = cached.is_some();
    let opts = Options {
        name: format!("./{base}"),
        jobname: jobname.clone(),
        comment: if pdf {
            String::new()
        } else {
            dvi_comment(epoch)
        },
        finder: finder(kpse(name, kengine)),
        prelude_finder: Some(finder(format_tree)),
        tool_finder: Some(tool_finder(kpse(name, kengine))),
        epoch: Some(epoch),
        fixpoint: a.fixpoint,
        prelude_jobname: name.to_owned(),
        program: name.to_owned(),
        prelude_bytes: cached,
        threads: a.threads,
        // (the format's first build in WebAssembly takes its time)
        time_limit: None,
        ..Options::default()
    };
    let b = Build::with(opts, prelude.as_deref(), &chars(&text));
    if let (Some(p), false) = (&a.fmt_cache, had)
        && let Some(state) = b.export_prelude()
        && let Err(e) = std::fs::write(p, state)
    {
        eprintln!("phitex-wasm: can't write {}: {e}", p.display());
    }
    let (out, log) = b.output();
    let mut errors = 0;
    for d in b.diagnostics() {
        match d.severity {
            Severity::Error => {
                errors += 1;
                eprintln!(
                    "{}:{}: {}",
                    d.file,
                    d.line,
                    d.message.lines().next().unwrap_or("")
                );
            }
            Severity::Warning if d.range == (0, 0) => eprintln!("warning: {}", d.message),
            _ => {}
        }
    }
    let write = |ext: &str, data: &[u8]| {
        let p = out_dir.join(format!("{jobname}.{ext}"));
        if let Err(e) = std::fs::write(&p, data) {
            eprintln!("phitex-wasm: can't write {}: {e}", p.display());
            std::process::exit(1);
        }
        p
    };
    let _ = std::fs::create_dir_all(&out_dir);
    write("log", &bytes(&log));
    let pages = b.pages().len();
    if out.is_empty() {
        // (a fatal error said its own last words: no PDF file produced)
        if !b.fatal() {
            println!("No pages of output.");
        }
    } else {
        let p = write(if b.is_pdf() { "pdf" } else { "dvi" }, &out);
        println!(
            "Output written on {} ({pages} page{}, {} bytes).",
            p.display(),
            if pages == 1 { "" } else { "s" },
            out.len()
        );
    }
    // (web2c's exit status: 1 for errors or worse)
    std::process::exit(i32::from(errors > 0));
}
