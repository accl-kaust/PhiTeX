//! What a page viewer draws (DESIGN 4.8): a PDF page's draw list, the
//! format the `PhiTeX` viewer (`page2.ts`'s `Draws2`, `"v":2`) reads, and
//! each page's hash, so a viewer draws again only the pages a build
//! changed. Made from the PDF's bytes, as a reader of the file sees them,
//! for any engine's PDF.
//!
//! The Overleaf extension's core made these first (its `pdfdraw.rs`,
//! `type1.rs` and `xetex.rs`'s glyph runs); this crate is that code, reading the PDF through
//! `partex_engine::pdfread`, so the CLI's viewer and the extension's draw
//! the same pages from the same functions.

mod image;
pub mod pdfdraw;
pub mod type1;
pub mod xetex;

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

    /// A one-page PDF with `content` as its content stream, and font
    /// `/F1` (CMR10, not embedded: no outlines).
    fn pdf(content: &str) -> Arc<[u8]> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /CMR10 /FirstChar 65 /Widths [750 708] >>"
                .to_owned(),
        ];
        raw_pdf(&objs)
    }

    /// A PDF of objects `objs` (numbered from 1; 1 the catalog), a char
    /// below 256 a byte (binary streams in the tests' text).
    fn raw_pdf(objs: &[String]) -> Arc<[u8]> {
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut at = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            at.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend(
                o.chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')),
            );
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for a in at {
            out.extend_from_slice(format!("{a:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out.into()
    }

    #[test]
    fn draws_paths_and_hashes_pages() {
        let a = pdf("1 0 0 RG 2 w 10 10 m 50 90 l S");
        let p = crate::Pdf::open(&a).unwrap();
        assert_eq!(p.page_count(), 1);
        let d = p.draw(0, &mut crate::Fonts::new()).unwrap();
        // (y from the top: 100 - 10, 100 - 90)
        assert_eq!(
            d,
            "{\"v\":2,\"w\":200,\"h\":100,\"f\":[\"roman\"],\"F\":[\"CMR10\"],\"g\":{},\"t\":[],\
             \"p\":[[\"M10 90L50 10\",null,\"#ff0000\",2]],\"r\":[]}"
        );
        let b = pdf("1 0 0 RG 2 w 10 10 m 50 91 l S");
        let (ha, hb) = (p.hashes(), crate::Pdf::open(&b).unwrap().hashes());
        assert_eq!(ha, crate::pdfdraw::hashes(&a));
        assert_ne!(ha, hb);
        // (nothing changed before the end: the sums kept)
        let sums = p.hashes_since(&[], None);
        assert_eq!(p.hashes_since(&sums, Some(a.len())), sums);
    }

    /// The draw list's contract (`page2.ts`'s `Draws2`), pinned: a text
    /// run `[font, size, y, "x x", text, outlined?, colour?]`, in points
    /// from the page's top left.
    #[test]
    fn text_runs() {
        let a = pdf("BT /F1 10 Tf 72 50 Td (AB) Tj 0 0 1 rg (A) Tj ET");
        let d = crate::Pdf::open(&a)
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        assert_eq!(
            d,
            "{\"v\":2,\"w\":200,\"h\":100,\"f\":[\"roman\"],\"F\":[\"CMR10\"],\"g\":{},\
             \"t\":[[0,10,50,\"72 79.5\",\"AB\"],[0,10,50,\"86.58\",\"A\",0,\"#0000ff\"]],\"p\":[],\"r\":[]}"
        );
    }

    /// The draw list's images, pinned: an image `XObject` (2×1 RGB
    /// samples, no filter) drawn by `cm` and `Do`, then a path: `"r"`'s
    /// entry `["id", a, b, c, d, e, f]` (an SVG unit square, row 0 on top,
    /// to the page from its top left), its PNG in `"I"`, and `"o"`, the
    /// paint order, since the path comes after the image.
    #[test]
    fn images() {
        let content = "q 20 0 0 10 5 5 cm /Im1 Do Q 0 g 0 0 m 1 1 l S";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /XObject << /Im1 5 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Length 6 >>\nstream\n\u{ff}\u{0}\u{0}\u{0}\u{0}\u{ff}\nendstream"
                .to_owned(),
        ];
        let a = raw_pdf(&objs);
        let d = crate::Pdf::open(&a)
            .unwrap()
            .draw(0, &mut crate::Fonts::new())
            .unwrap();
        let (head, tail) = d.split_once(",\"I\":{").unwrap();
        assert_eq!(
            head,
            "{\"v\":2,\"w\":200,\"h\":100,\"f\":[],\"F\":[],\"g\":{},\"t\":[],\
             \"p\":[[\"M0 100L1 99\",null,\"#000000\",1]],\
             \"r\":[[\"i024bae8ca2e00820\",20,0,0,10,5,85]]"
        );
        assert!(
            tail.starts_with("\"i024bae8ca2e00820\":\"data:image/png;base64,iVBORw0KGgo"),
            "{tail}"
        );
        assert!(tail.ends_with("},\"o\":[[1,0,1],[0,0,1]]}"), "{tail}");
    }

    fn run(
        gid: u16,
        x: f64,
        tm: [f64; 4],
        rgba: u32,
        text: &str,
    ) -> partex_xdvipdfmx::api::GlyphRun {
        partex_xdvipdfmx::api::GlyphRun {
            source: partex_xdvipdfmx::api::GlyphSource::Native {
                font_file: b"/nowhere/Font.otf".to_vec(),
                face_index: 0,
                gid,
            },
            x,
            y: 40.0,
            size: 12.0,
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            tm,
            rgba,
            color: partex_xdvipdfmx::glyphrun::ColorSpec::Gray(0.0),
            text: Some(text.into()),
            cluster: u32::from(gid),
            actual_text: false,
        }
    }

    /// `XeTeX`'s glyph runs, pinned: a run `[-1, size, y, "x x", "", font
    /// ref, [glyphs], colour?]` with its text `[0, size, y, "x x", text,
    /// 1]`; a transformed glyph alone, `[-1, size, y, "x", "", ref,
    /// [glyph], colour or null, [a, b, c, d]]` (SVG's `matrix`, y down,
    /// outlines in 1/1000 em). A font that cannot be read: advances of
    /// half an em, no outlines.
    #[test]
    fn xetex_runs() {
        let id = [1.0, 0.0, 0.0, 1.0];
        let runs = [
            run(36, 10.0, id, 0xff, "H"),
            run(73, 16.0, id, 0xff, "i"),
            run(50, 30.0, [0.0, 1.0, -1.0, 0.0], 0xff00_00ff, "R"),
        ];
        let e = crate::xetex::extra(
            &runs,
            100.0,
            3,
            &mut crate::xetex::Faces::new(),
            &mut |_| None,
        );
        assert_eq!(e.fonts, ["xFont_otf_0"]);
        assert_eq!(e.g, "");
        assert_eq!(
            e.t,
            "[-1,12,60,\"10 16\",\"\",3,[36,73]],[0,12,60,\"10 16\",\"Hi\",1],\
             [-1,12,60,\"30\",\"\",3,[50],\"#ff0000\",[0,-0.012,-0.012,-0]]"
        );
    }
}
