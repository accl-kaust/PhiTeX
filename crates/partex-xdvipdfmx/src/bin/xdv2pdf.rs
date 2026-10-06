//! `xdv2pdf [-o NAME.pdf] [--dvi NAME.xdv] FILE.xdv > OUT.pdf`: XDV to PDF,
//! as `xelatex` does it (`xdvipdfmx -q -E -o NAME.pdf < NAME.xdv`).
//!
//! The PDF goes to stdout. `-o` is the PDF's name as xdvipdfmx is told it
//! (the subset tags hash it; default: FILE's, with `.pdf`); `--dvi` gives
//! xdvipdfmx a DVI name, as when it reads a file and not a pipe. Files are
//! found with `kpsewhich`; streams are compressed with the system zlib's
//! `compress2`, as xdvipdfmx compresses them.
#![expect(unsafe_code, reason = "FFI to the system zlib")]

use std::collections::HashMap;
use std::ffi::{OsStr, c_int, c_ulong};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;
use std::sync::Arc;

use partex_xdvipdfmx::api::{Options, Session, split_xdv};
use partex_xdvipdfmx::io::{Files, Format};

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

/// kpathsea, through `kpsewhich`, its answers cached.
#[derive(Default)]
struct Kpsewhich {
    found: HashMap<Query, Option<Vec<u8>>>,
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Fontmap => "map",
        Format::Type1 => "type1 fonts",
        Format::TrueType => "truetype fonts",
        Format::OpenType => "opentype fonts",
        Format::Cmap => "cmap files",
        Format::Sfd => "subfont definition files",
        Format::Enc => "enc files",
        Format::Tfm => "tfm",
        Format::Ofm => "ofm",
        Format::Vf => "vf",
        Format::Ovf => "ovf",
        Format::Pict => "graphic/figure",
        Format::Tex => "tex",
        Format::ProgramText => "other text files",
        Format::ProgramBinary => "other binary files",
    }
}

impl Files for Kpsewhich {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let key = (name.to_vec(), format, progname.to_vec());
        if let Some(r) = self.found.get(&key) {
            return r.clone();
        }
        let out = Command::new("kpsewhich")
            .arg(format!("-progname={}", String::from_utf8_lossy(progname)))
            .arg(format!("-format={}", format_name(format)))
            .arg(OsStr::from_bytes(name))
            .output()
            .ok()?;
        let mut path = out.stdout;
        while path.last() == Some(&b'\n') {
            path.pop();
        }
        let r = (out.status.success() && !path.is_empty()).then_some(path);
        self.found.insert(key, r.clone());
        r
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        std::fs::read(OsStr::from_bytes(path)).ok().map(Arc::from)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut pdf, mut dvi, mut input, mut resume) = (None, None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => pdf = args.next(),
            "--dvi" => dvi = args.next(),
            "--check-resume" => resume = true,
            _ => input = Some(a),
        }
    }
    let Some(input) = input else {
        eprintln!("usage: xdv2pdf [-o NAME.pdf] [--dvi NAME.xdv] FILE.xdv > OUT.pdf");
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
    // Page by page, as xdvipdfmx writes: when it stops on an error
    // (`ERROR` prints its message and exits), its output file keeps what
    // was written until then, and so does stdout here.
    let mut stdout = std::io::stdout();
    let (pre, pages) = split_xdv(&xdv);
    let mut session = match Session::new(
        options,
        Box::new(Kpsewhich::default()),
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
        Ok::<(), partex_xdvipdfmx::ctx::Fatal>(())
    })();
    if let Err(e) = result {
        stdout.write_all(&session.written()).expect("stdout");
        eprintln!("{e}");
        std::process::exit(1);
    }
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
