//! A DVI file shown in the live viewer: made a PDF by dvipdfmx, in
//! process (`partex-xdvipdfmx`), its files through kpathsea as dvipdfmx
//! finds them. Only for the viewer: the build's output is the DVI file.

use std::collections::HashMap;
use std::sync::Arc;

use partex_kpse::{Format as KpseFormat, Kpse};
use partex_xdvipdfmx::api::{Options, Session};
use partex_xdvipdfmx::io::{Files, Format};

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

/// dvipdfmx's files: kpathsea as `dvipdfmx` ("we pretend to be dvipdfmx
/// for kpse purposes"), each lookup and file once.
pub struct DpxFiles {
    kpse: Kpse,
    found: HashMap<(Vec<u8>, Format, Vec<u8>), Option<Vec<u8>>>,
    read: HashMap<Vec<u8>, Option<Arc<[u8]>>>,
}

impl DpxFiles {
    pub fn new() -> DpxFiles {
        DpxFiles { kpse: crate::tex::kpse("dvipdfmx", "pdftex"), found: HashMap::new(), read: HashMap::new() }
    }
}

impl Files for DpxFiles {
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
        self.read
            .entry(path.to_vec())
            .or_insert_with(|| {
                let p = std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path));
                std::fs::read(p).ok().map(Arc::from)
            })
            .clone()
    }
}

/// The PDF dvipdfmx makes of `dvi` (named `pdf_name`), or why not.
pub fn dvi_to_pdf(dvi: &[u8], pdf_name: &str, date: i64) -> Result<Vec<u8>, String> {
    let options = Options {
        pdf_filename: Some(pdf_name.as_bytes().to_vec()),
        dvi_filename: None,
        args: vec![b"-q".to_vec()],
        source_date_epoch: Some(date),
        now: date,
        utc_offset_min: 0,
        system_paper: None,
    };
    Session::convert(
        options,
        Box::new(DpxFiles::new()),
        Box::new(|level, data| phitex_flow::pdf::zlib::compress(data, level)),
        dvi,
    )
    .map_err(|e| format!("dvipdfmx: {e}"))
}
