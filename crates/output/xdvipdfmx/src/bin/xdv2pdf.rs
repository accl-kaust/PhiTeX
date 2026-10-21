//! `xdv2pdf [-o NAME.pdf] [--dvi NAME.xdv] FILE.xdv > OUT.pdf`: XDV to PDF,
//! as `xelatex` does it (`xdvipdfmx -q -E -o NAME.pdf < NAME.xdv`).
//!
//! The PDF goes to stdout. `-o` is the PDF's name as xdvipdfmx is told it
//! (the subset tags hash it; default: FILE's, with `.pdf`); `--dvi` gives
//! xdvipdfmx a DVI name, as when it reads a file and not a pipe. Files are
//! found by kpathsea in process (`phitex-kpse`, as the CLI's viewer finds
//! them); streams are compressed with the system zlib's `compress2`, as
//! xdvipdfmx compresses them.
#![expect(unsafe_code, reason = "FFI to the system zlib")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, c_int, c_ulong};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use phitex_kpse::{Format as KpseFormat, Kpse};
use phitex_xdvipdfmx::api::{Options, Session, split_xdv};
use phitex_xdvipdfmx::io::{Files, Format};

#[link(name = "z")]
unsafe extern "C" {
    fn compress2(
        dest: *mut u8,
        dest_len: *mut c_ulong,
        source: *const u8,
        source_len: c_ulong,
        level: c_int,
    ) -> c_int;
    fn compressBound(source_len: c_ulong) -> c_ulong;
}

/// zlib's `compress2` (pdfobj.c's `write_stream`).
fn deflate(level: i32, data: &[u8]) -> Vec<u8> {
    // SAFETY: `out` has the `compressBound` bytes zlib asks for; both
    // buffers outlive the call.
    unsafe {
        let mut len = compressBound(data.len() as c_ulong);
        let mut out = vec![0u8; usize::try_from(len).expect("size")];
        let r = compress2(
            out.as_mut_ptr(),
            &raw mut len,
            data.as_ptr(),
            data.len() as c_ulong,
            level,
        );
        assert_eq!(r, 0, "compress2 failed");
        out.truncate(usize::try_from(len).expect("size"));
        out
    }
}

/// A lookup: the name, the format, the program name.
type Query = (Vec<u8>, Format, Vec<u8>);

fn kpse_format(format: Format) -> KpseFormat {
    match format {
        Format::Fontmap => KpseFormat::FontMap,
        Format::Type1 => KpseFormat::Type1,
        Format::TrueType => KpseFormat::TrueType,
        Format::OpenType => KpseFormat::OpenType,
        Format::Cmap => KpseFormat::Cmap,
        Format::Sfd => KpseFormat::Sfd,
        Format::Enc => KpseFormat::Enc,
        Format::Tfm => KpseFormat::Tfm,
        Format::Ofm => KpseFormat::Ofm,
        Format::Vf => KpseFormat::Vf,
        Format::Ovf => KpseFormat::Ovf,
        Format::Pict => KpseFormat::Pict,
        Format::Tex => KpseFormat::Tex,
        Format::ProgramText => KpseFormat::ProgramText,
        Format::ProgramBinary => KpseFormat::ProgramBinary,
    }
}

/// kpathsea in process (`phitex-kpse`) as `dvipdfmx`, from the TeX
/// Live of the `tex` on the PATH, its answers cached.
struct Kpsewhich {
    kpse: Kpse,
    found: HashMap<Query, Option<Vec<u8>>>,
}

impl Default for Kpsewhich {
    fn default() -> Self {
        let dir = std::env::var_os("PATH")
            .and_then(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.join("tex"))
                    .filter_map(|t| t.canonicalize().ok())
                    .find(|t| t.is_file())
                    .and_then(|t| t.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin"));
        Kpsewhich {
            kpse: Kpse::new(&dir, "dvipdfmx", "pdftex"),
            found: HashMap::new(),
        }
    }
}

/// The files of a process: one [`Kpsewhich`], its lookups and the files
/// read kept for every conversion the process makes.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Kpsewhich>>, Rc<RefCell<HashMap<Vec<u8>, Option<Arc<[u8]>>>>>);

impl Files for Shared {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        self.0.borrow_mut().find(name, format, progname)
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.1
            .borrow_mut()
            .entry(path.to_vec())
            .or_insert_with(|| std::fs::read(OsStr::from_bytes(path)).ok().map(Arc::from))
            .clone()
    }
}

impl Files for Kpsewhich {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let key = (name.to_vec(), format, progname.to_vec());
        if let Some(r) = self.found.get(&key) {
            return r.clone();
        }
        self.kpse.reset_program_name(progname);
        let r = self.kpse.find_file(name, kpse_format(format), false);
        self.kpse.reset_program_name(b"dvipdfmx");
        self.found.insert(key, r.clone());
        r
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        std::fs::read(OsStr::from_bytes(path)).ok().map(Arc::from)
    }
}

fn main() {
    let t0 = Instant::now();
    let mut args = std::env::args().skip(1);
    let (mut pdf, mut dvi, mut input, mut resume, mut repeat) = (None, None, None, false, 1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => pdf = args.next(),
            "--dvi" => dvi = args.next(),
            "--check-resume" => resume = true,
            "--repeat" => repeat = args.next().and_then(|n| n.parse().ok()).unwrap_or(1),
            _ => input = Some(a),
        }
    }
    let Some(input) = input else {
        eprintln!("usage: xdv2pdf [-o NAME.pdf] [--dvi NAME.xdv] [--repeat N] FILE.xdv > OUT.pdf");
        std::process::exit(2);
    };
    let pdf =
        pdf.unwrap_or_else(|| format!("{}.pdf", input.strip_suffix(".xdv").unwrap_or(&input)));
    let xdv = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("xdv2pdf: {input}: {e}");
        std::process::exit(1);
    });
    let options = Options {
        pdf_filename: Some(pdf.into_bytes()),
        dvi_filename: dvi.map(String::into_bytes),
        source_date_epoch: std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        ..Options::default()
    };
    if resume {
        check_resume(options, &xdv);
        return;
    }
    let files = Shared::default();
    if repeat > 1 {
        // `--repeat N`: the same XDV converted N times in this process,
        // as a watch converts each build: the first conversion cold, the
        // others warm (the host's files and lookups kept). Each one's
        // time to stderr; all must give the same bytes, the last to
        // stdout.
        let first = cold_run(&options, &files, &xdv);
        eprintln!("xdv2pdf: conversion 1: {:.3} ms", ms(t0));
        for k in 2..=repeat {
            let t = Instant::now();
            let again = warm_run(&options, &files, &xdv);
            eprintln!("xdv2pdf: conversion {k}: {:.3} ms", ms(t));
            if again != first {
                eprintln!("xdv2pdf: conversion {k} differs from the first");
                std::process::exit(1);
            }
        }
        let (bytes, err) = first;
        std::io::stdout().write_all(&bytes).expect("stdout");
        if let Some(e) = err {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    // Page by page, as xdvipdfmx writes: when it stops on an error
    // (`ERROR` prints its message and exits), its output file keeps what
    // was written until then, and so does stdout here.
    let mut stdout = std::io::stdout();
    let (pre, pages) = split_xdv(&xdv);
    let mut session = match Session::new(
        options,
        Box::new(files),
        Box::new(deflate),
        &xdv[..pre],
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let result = (|| {
        for &(a, b) in &pages {
            let out = session.page(&xdv[a..b])?;
            stdout.write_all(&out.pdf).expect("stdout");
        }
        stdout.write_all(&session.finish()?).expect("stdout");
        Ok::<(), phitex_xdvipdfmx::ctx::Fatal>(())
    })();
    if let Err(e) = result {
        stdout.write_all(&session.written()).expect("stdout");
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// One whole conversion: the bytes written and the fatal error that
/// stopped it, if one did.
fn convert(options: &Options, files: &Shared, xdv: &[u8]) -> (Vec<u8>, Option<String>) {
    let (pre, pages) = split_xdv(xdv);
    let mut session = match Session::new(
        options.clone(),
        Box::new(files.clone()),
        Box::new(deflate),
        &xdv[..pre],
    ) {
        Ok(s) => s,
        Err(e) => return (Vec::new(), Some(e.to_string())),
    };
    let mut out = Vec::new();
    let result = (|| {
        for &(a, b) in &pages {
            out.extend(session.page(&xdv[a..b])?.pdf);
        }
        out.extend(session.finish()?);
        Ok::<(), phitex_xdvipdfmx::ctx::Fatal>(())
    })();
    match result {
        Ok(()) => (out, None),
        Err(e) => {
            out.extend(session.written());
            (out, Some(e.to_string()))
        }
    }
}

/// The first conversion (a function of its own: profiles tell it apart).
#[inline(never)]
fn cold_run(options: &Options, files: &Shared, xdv: &[u8]) -> (Vec<u8>, Option<String>) {
    convert(options, files, xdv)
}

/// A later conversion in the same process (`callgrind
/// --toggle-collect='*warm_run*'` counts these alone).
#[inline(never)]
fn warm_run(options: &Options, files: &Shared, xdv: &[u8]) -> (Vec<u8>, Option<String>) {
    convert(options, files, xdv)
}

/// `--check-resume`: the incremental link's test. A cold run keeps a
/// snapshot before each page; going on from each snapshot (that page,
/// the pages after it, the end) must give the cold run's bytes. Prints
/// the number of pages checked; exits 1 on a difference.
fn check_resume(options: Options, xdv: &[u8]) {
    let (pre, pages) = split_xdv(xdv);
    let mut s = Session::new(
        options,
        Box::new(Kpsewhich::default()),
        Box::new(deflate),
        &xdv[..pre],
    )
    .expect("xdvipdfmx");
    let mut snaps = Vec::new();
    let mut outs: Vec<Vec<u8>> = Vec::new();
    for &(a, b) in &pages {
        snaps.push(s.snapshot());
        outs.push(s.page(&xdv[a..b]).expect("xdvipdfmx").pdf);
    }
    outs.push(s.finish().expect("xdvipdfmx"));
    for (k, snap) in snaps.into_iter().enumerate() {
        let mut s = snap;
        let mut tail = Vec::new();
        for &(a, b) in &pages[k..] {
            tail.push(s.page(&xdv[a..b]).expect("xdvipdfmx").pdf);
        }
        tail.push(s.finish().expect("xdvipdfmx"));
        if tail[..] != outs[k..] {
            eprintln!("xdv2pdf: resuming before page {} differs", k + 1);
            std::process::exit(1);
        }
    }
    println!("{} pages resumed identically", pages.len());
}
