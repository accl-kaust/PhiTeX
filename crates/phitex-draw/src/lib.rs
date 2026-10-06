//! What a page viewer draws (DESIGN 4.8): a PDF page's draw list, the
//! format the `PhiTeX` viewer (`page2.ts`'s `Draws2`, `"v":2`) reads, and
//! each page's hash, so a viewer draws again only the pages a build
//! changed. Made from the PDF's bytes, as a reader of the file sees them,
//! for any engine's PDF.
//!
//! The Overleaf extension's core made these first (its `pdfdraw.rs` and
//! `type1.rs`); this crate is that code, reading the PDF through
//! `partex_engine::pdfread`, so the CLI's viewer and the extension's draw
//! the same pages from the same functions.

pub mod pdfdraw;
pub mod type1;

pub use pdfdraw::{Extra, Fonts, PageSum, Pdf};

use std::fmt::Write as _;

/// `s` as a JSON string.
#[must_use]
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    /// A one-page PDF with `content` as its content stream.
    fn pdf(content: &str) -> Arc<[u8]> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut at = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            at.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for a in at {
            out.extend_from_slice(format!("{a:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
        );
        out.into()
    }

    #[test]
    fn draws_paths_and_hashes_pages() {
        let a = pdf("1 0 0 RG 2 w 10 10 m 50 90 l S");
        let p = crate::Pdf::open(&a).unwrap();
        assert_eq!(p.page_count(), 1);
        let d = p.draw(0, &mut crate::Fonts::new()).unwrap();
        assert!(d.starts_with("{\"v\":2,\"w\":200,\"h\":100,"), "{d}");
        // (y from the top: 100 - 10, 100 - 90)
        assert!(d.contains("[\"M10 90L50 10\",null,\"#ff0000\",2]"), "{d}");
        let b = pdf("1 0 0 RG 2 w 10 10 m 50 91 l S");
        let (ha, hb) = (p.hashes(), crate::Pdf::open(&b).unwrap().hashes());
        assert_eq!(ha, crate::pdfdraw::hashes(&a));
        assert_ne!(ha, hb);
        // (nothing changed before the end: the sums kept)
        let sums = p.hashes_since(&[], None);
        assert_eq!(p.hashes_since(&sums, Some(a.len())), sums);
    }
}
