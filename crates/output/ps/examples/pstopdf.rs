//! `pstopdf e2p|dpx FONTDIR IN.eps OUT.pdf`: a conversion as epstopdf
//! (e2p) or the D line (dpx) runs it, with Ghostscript's Resource/Font
//! in FONTDIR (for comparing with the oracle).

use std::rc::Rc;

struct Dir(String);

impl phitex_ps::Resources for Dir {
    fn font(&self, name: &[u8]) -> Option<Rc<Vec<u8>>> {
        let n = std::str::from_utf8(name).ok()?;
        if n.contains('/') {
            return None;
        }
        std::fs::read(format!("{}/{n}", self.0)).ok().map(Rc::new)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let eps = std::fs::read(&a[3]).expect("input");
    let epoch = 1_758_800_000;
    let out = if a[1] == "e2p" {
        phitex_ps::convert(
            &phitex_ps::epstopdf_feed(&eps),
            phitex_ps::Settings::epstopdf(epoch),
            &Dir(a[2].clone()),
        )
    } else {
        phitex_ps::convert(
            &eps,
            phitex_ps::Settings::dvipdfmx(15, epoch),
            &Dir(a[2].clone()),
        )
    };
    if let Some(e) = &out.error {
        eprintln!("{e}");
    }
    std::fs::write(&a[4], &out.pdf).expect("output");
}
