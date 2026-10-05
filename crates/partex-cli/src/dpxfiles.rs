//! xdvipdfmx in process, as `xelatex` runs it (`xdvipdfmx -q -E -o
//! NAME.pdf`, the XDV piped in): its files through partex-kpse, set up as
//! dvipdfmx.c's `main` sets kpathsea up, and its streams compressed with
//! zlib's `compress2`, as pdfobj.c's `write_stream` compresses them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use partex_kpse::{Format as KpseFormat, Kpse};
use partex_xdvipdfmx::api::{Options, Session};
use partex_xdvipdfmx::io::{Files, Format};
use partex_xdvipdfmx::obj::Deflate;

/// dvipdfmx.c's `kpse_set_program_name(argv[0], "dvipdfmx")` ("we pretend
/// to be dvipdfmx for kpse purposes"), in the environment `xetex` runs it
/// in (`engine` is `xetex`'s): one instance, as in C, its `texmf.cnf` and
/// `ls-R` read once.
fn kpse() -> &'static Mutex<Kpse> {
    static KPSE: OnceLock<Mutex<Kpse>> = OnceLock::new();
    KPSE.get_or_init(|| Mutex::new(crate::kpse_instance("dvipdfmx", "xetex")))
}

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

/// A lookup: the name, the format, the program name.
type Query = (Vec<u8>, Format, Vec<u8>);

/// What one conversion reads: kpathsea's answers and the files' bytes,
/// each asked for once (fonts are read to be checked, then to be used).
#[derive(Default)]
pub(crate) struct DpxFiles {
    found: HashMap<Query, Option<Vec<u8>>>,
    read: HashMap<Vec<u8>, Option<Arc<[u8]>>>,
}

impl Files for DpxFiles {
    /// `kpse_find_file(name, format, 0)` as `progname`: dpxfile.c's
    /// `dpx_foolsearch` resets the program name, searches, and resets it
    /// to `dvipdfmx`.
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let key = (name.to_vec(), format, progname.to_vec());
        if let Some(r) = self.found.get(&key) {
            return r.clone();
        }
        let mut k = kpse().lock().unwrap_or_else(PoisonError::into_inner);
        k.reset_program_name(progname);
        let r = k.find_file(name, kpse_format(format), false);
        k.reset_program_name(b"dvipdfmx");
        drop(k);
        self.found.insert(key, r.clone());
        r
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.read
            .entry(path.to_vec())
            .or_insert_with(|| std::fs::read(crate::native::path(path)).ok().map(Arc::from))
            .clone()
    }
}

/// pdfobj.c's `write_stream`: `compress2(level)`, "Zlib error" if it
/// fails (zlib's bytes do not depend on how its input is handed in).
pub(crate) fn deflate() -> Deflate {
    Box::new(|level, data| {
        crate::zlib::deflate_stream(level, data).unwrap_or_else(|| panic!("Zlib error"))
    })
}

/// xelatex's `xdvipdfmx -q -E -o NAME.pdf` (`pdf_name`, which the subset
/// tags and the ID hash), the XDV piped in; `SOURCE_DATE_EPOCH` from the
/// environment, else the clock and the local zone.
pub(crate) fn options(pdf_name: &[u8]) -> Options {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    Options {
        pdf_filename: Some(pdf_name.to_vec()),
        dvi_filename: None,
        source_date_epoch: std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        now,
        utc_offset_min: crate::clock::Zone::local().offset(now) / 60,
        ..Options::default()
    }
}

/// The PDF xelatex makes of `xdv` (panics where xdvipdfmx stops with
/// `ERROR`).
pub(crate) fn xdv_to_pdf(xdv: &[u8], pdf_name: &[u8]) -> Vec<u8> {
    Session::convert(
        options(pdf_name),
        Box::new(DpxFiles::default()),
        deflate(),
        xdv,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The lookups made, and what they found.
    type Log = Rc<RefCell<Vec<(Query, Option<Vec<u8>>)>>>;

    /// [`DpxFiles`], each lookup noted.
    struct Noted(DpxFiles, Log);

    impl Files for Noted {
        fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
            let r = self.0.find(name, format, progname);
            let q = (name.to_vec(), format, progname.to_vec());
            self.1.borrow_mut().push((q, r.clone()));
            r
        }

        fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
            self.0.read(path)
        }
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

    /// TeX Live's oracle: each `NAME.xdv` in `PARTEX_DPX_ORACLE` (made by
    /// `xetex -no-pdf`, the directory xdvipdfmx ran in, with the same
    /// environment) gives the bytes of `NAME.pdf` (made by `xetex`), and
    /// each lookup finds what `kpsewhich` finds.
    #[test]
    #[ignore = "needs PARTEX_DPX_ORACLE: XDVs and PDFs from TeX Live's xetex"]
    fn oracle() {
        use std::os::unix::ffi::OsStrExt;
        let dir = std::env::var_os("PARTEX_DPX_ORACLE").expect("PARTEX_DPX_ORACLE");
        std::env::set_current_dir(&dir).expect("oracle directory");
        let mut names: Vec<_> = std::fs::read_dir(".")
            .expect("oracle directory")
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "xdv").then(|| p.with_extension(""))
            })
            .collect();
        names.sort();
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut differ = Vec::new();
        for n in &names {
            let n = n.file_name().expect("name").as_bytes();
            let name = String::from_utf8_lossy(n);
            let xdv = std::fs::read(format!("{name}.xdv")).expect("xdv");
            let want = std::fs::read(format!("{name}.pdf")).expect("pdf");
            let got = Session::convert(
                options(format!("{name}.pdf").as_bytes()),
                Box::new(Noted(DpxFiles::default(), log.clone())),
                deflate(),
                &xdv,
            );
            eprintln!("{name}: {}", if got == want { "same" } else { "DIFFERENT" });
            if got != want {
                differ.push(name.into_owned());
            }
        }
        let mut seen = std::collections::HashSet::new();
        let queries: Vec<_> = log
            .borrow()
            .iter()
            .filter(|q| seen.insert((*q).clone()))
            .cloned()
            .collect();
        let mut wrong = Vec::new();
        for ((name, format, prog), got) in &queries {
            let out = std::process::Command::new("kpsewhich")
                .arg(format!("-progname={}", String::from_utf8_lossy(prog)))
                .arg("-engine=xetex")
                .arg(format!("-format={}", format_name(*format)))
                .arg(std::ffi::OsStr::from_bytes(name))
                .output()
                .expect("kpsewhich");
            let mut want = out.stdout;
            while want.last() == Some(&b'\n') {
                want.pop();
            }
            let want = (!want.is_empty()).then_some(want);
            let show = |p: &Option<Vec<u8>>| {
                p.as_deref()
                    .map(|p| String::from_utf8_lossy(p).into_owned())
            };
            eprintln!(
                "{} {format:?} as {}: {:?}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(prog),
                show(got)
            );
            if *got != want {
                wrong.push(format!(
                    "{} {format:?} as {}: {:?}, kpsewhich {:?}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(prog),
                    show(got),
                    show(&want)
                ));
            }
        }
        eprintln!("{} documents, {} lookups", names.len(), queries.len());
        assert!(differ.is_empty(), "PDFs differ: {differ:?}");
        assert!(wrong.is_empty(), "lookups differ:\n{}", wrong.join("\n"));
    }
}
